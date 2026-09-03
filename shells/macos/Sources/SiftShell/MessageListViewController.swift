import AppKit
import CSift

/// FR-6's message list: a virtualized table fed by a D-18 observation.
///
/// # What this does not do
///
/// It does not query, sort, filter, or decide what a row looks like beyond drawing it. The
/// layer windows the list, orders it on D-55's server-assigned time, resolves it through
/// D-51's overlay and normalizes every attacker-controlled string under NFR-54 before any of
/// it arrives. A shell that re-sorted would be a shell the other one could disagree with.
///
/// It also never asks. Rows arrive as change batches; the only thing sent the other way is
/// the declared window.
final class MessageListViewController: NSViewController {
    /// The rows as last delivered. The layer's window and this array are the same list, and
    /// keeping them so is the whole of what applying a batch means.
    private(set) var rows: [MessageRow] = []

    private let table = NSTableView()
    private let scroll = NSScrollView()
    private var app: OpaquePointer?
    private var observation = SiftObservation(SiftObservation_NONE)

    /// Told when the selection changes, so the reader can follow it.
    var onSelect: ((MessageRow?) -> Void)?

    /// FR-19 — search replaces the list's contents rather than opening a window.
    ///
    /// The two things shown beside the results are not decoration. **A query that found
    /// nothing and one that was misread look identical from the results alone**, so the
    /// interpretation is shown; and empty results and unevaluated filters also look identical,
    /// so what the build could not answer is shown too. A person searching `has:attachment`,
    /// getting nothing, and concluding they have no attachments has been misled by a filter
    /// that never ran.
    private var searching = false

    func search(_ query: String, app: OpaquePointer, account: SiftId = .zero) {
        guard !query.trimmingCharacters(in: .whitespaces).isEmpty else {
            clearSearch()
            return
        }
        var found = SiftSearch()
        let ok = SiftText.withBytes(query) { ptr, len in
            sift_search(UnsafeMutablePointer(app), ptr, len, account, 200, &found) == Ok
        }
        guard ok else { return }

        // The observation keeps delivering while a search is on screen, and its batches would
        // fight the results for the same table. Cancelling is synchronous: when it returns, no
        // further delivery for it can arrive on any thread.
        cancel()
        searching = true
        rows = []
        if let ptr = found.rows.ptr {
            for index in 0..<found.rows.len {
                rows.append(MessageRow(ptr[index]))
            }
        }
        table.reloadData()
        onSearchReport?(
            SiftText.string(found.interpretation), SiftText.string(found.caveats), rows.count)
        onSelect?(nil)
    }

    /// Where the interpretation and the caveats go. The list draws rows; the window draws
    /// sentences about them.
    var onSearchReport: ((String, String, Int) -> Void)?
    var onSearchCleared: (() -> Void)?

    func clearSearch() {
        guard searching else { return }
        searching = false
        rows = []
        table.reloadData()
        onSearchCleared?()
    }

    // Row geometry. Two lines plus the breathing room a mail list needs to be scannable;
    // sized from the body font so that the system's larger-text setting grows the row rather
    // than clipping what is drawn in it.
    private var rowHeight: CGFloat {
        max(72, NSFont.preferredFont(forTextStyle: .body).boundingRectForFont.height * 3.4)
    }

    override func loadView() {
        table.headerView = nil
        table.rowHeight = rowHeight
        table.style = .inset
        table.usesAlternatingRowBackgroundColors = false
        table.allowsMultipleSelection = true
        table.selectionHighlightStyle = .regular
        table.dataSource = self
        table.delegate = self

        let column = NSTableColumn(identifier: .init("message"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)

        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        // NFR-6 wants 60 fps with zero dropped frames over a 10,000-row fling. Live resize
        // redraw is the thing most likely to cost it, and a list has nothing that needs to
        // reflow mid-drag.
        scroll.contentView.postsBoundsChangedNotifications = false

        view = scroll
    }

    /// Register the observation. **Anchored, not a range** — D-18.
    func observe(app: OpaquePointer, account: SiftId, window: UInt32 = 500) {
        self.app = app
        cancel()

        var handle = SiftObservation(SiftObservation_NONE)
        // The context is this controller, unretained: the layer never dereferences it and
        // hands it straight back, and cancellation in `cancel()` is what guarantees no
        // delivery outlives the controller.
        let context = Unmanaged.passUnretained(self).toOpaque()
        let status = sift_observe_messages(
            UnsafeMutablePointer(app),
            account,
            window,
            { context, _, _, rows in
                guard let context else { return }
                let me = Unmanaged<MessageListViewController>
                    .fromOpaque(context).takeUnretainedValue()
                // **Receive, record, return.** Calling back into the layer from here is what
                // D-48 forbids; anything this triggers happens on the next turn of the loop.
                var copied: [MessageRow] = []
                if let ptr = rows.ptr, rows.len > 0 {
                    copied.reserveCapacity(rows.len)
                    for i in 0..<rows.len { copied.append(MessageRow(ptr[i])) }
                }
                me.received(copied)
            },
            context,
            &handle
        )
        if status == Ok { observation = handle }
    }

    /// D-48's synchronous cancellation: when this returns, nothing further arrives, ever.
    func cancel() {
        guard let app, observation.rawValue != SiftObservation_NONE else { return }
        _ = sift_cancel_observation(UnsafeMutablePointer(app), observation)
        observation = SiftObservation(SiftObservation_NONE)
    }

    deinit { cancel() }

    /// A delivery arrived.
    ///
    /// **The window, replaced.** This used to append what arrived, which is only correct while
    /// nothing ever leaves: a sync that removes a message left its row on screen, and opening
    /// it failed with the store saying no account holds that message — because none did any
    /// more. The layer now sends the whole window, and this array and that window are the same
    /// list, which is the invariant that matters most.
    ///
    /// D-18's change vocabulary is still the right answer and is still unwired: a move drawn
    /// as a move keeps a row's identity and a cell's state, and `reloadData` keeps neither.
    /// What it costs today is animation. What appending cost was correctness.
    private func received(_ incoming: [MessageRow]) {
        let previous = selectedRow()
        let wasEmpty = rows.isEmpty
        rows = incoming
        table.reloadData()
        restore(previous)

        // The newest message, on first fill. A reader that opens on "nothing selected" makes
        // a person click before the product does anything, and the top of the list is what
        // they were going to click. It is safe to do here because opening a message is not
        // marking it read — D-52 puts a dwell between those, and nothing has dwelled.
        if wasEmpty, previous == nil, !rows.isEmpty {
            table.selectRowIndexes([0], byExtendingSelection: false)
        }
    }

    /// What is selected, for the window's own surfaces.
    var selection: MessageRow? { selectedRow() }

    /// Move the selection — `read.next-message` and the three beside it.
    ///
    /// **The shell's own work, and it was crossing the boundary to nobody.** These are in
    /// D-98's register, they have no intent behind them, and invoking them returned success
    /// and moved nothing. Where the caret goes in a list this shell draws is not something the
    /// layer can answer.
    ///
    /// `unreadOnly` walks to the next row the list says is unread rather than to the next row,
    /// which is what ⇧⌘↓ means in a mail client.
    func move(by step: Int, unreadOnly: Bool) {
        guard !rows.isEmpty else { return }
        let current = table.selectedRow
        var index = current < 0 ? (step > 0 ? -1 : rows.count) : current
        while true {
            index += step
            guard index >= 0, index < rows.count else { return }
            if !unreadOnly || rows[index].unread { break }
        }
        table.selectRowIndexes([index], byExtendingSelection: false)
        table.scrollRowToVisible(index)
    }

    /// Where the keyboard goes when the list is asked for — `navigate.focus-list`.
    var focusTarget: NSView { table }

    private func selectedRow() -> MessageRow? {
        let index = table.selectedRow
        guard index >= 0, index < rows.count else { return nil }
        return rows[index]
    }

    /// Selection follows the **message**, not the cell index. A list that restored an index
    /// would move the selection whenever anything above it arrived or left.
    private func restore(_ row: MessageRow?) {
        guard let row, let index = rows.firstIndex(where: { $0 == row }) else { return }
        table.selectRowIndexes([index], byExtendingSelection: false)
    }
}

extension MessageListViewController: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(
        _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
    ) -> NSView? {
        guard row < rows.count else { return nil }
        let identifier = NSUserInterfaceItemIdentifier("row")
        let cell =
            tableView.makeView(withIdentifier: identifier, owner: self) as? MessageRowView
            ?? MessageRowView(identifier: identifier)
        cell.show(rows[row])
        return cell
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        onSelect?(selectedRow())
    }
}

extension SiftObservation {
    var rawValue: UInt64 { self }
}
