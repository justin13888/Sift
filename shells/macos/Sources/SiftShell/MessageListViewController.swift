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
    /// The layer sends only the rows entering the window, and today it sends them whole on
    /// first fill. Applying the change vocabulary row by row — so a move animates as a move
    /// and a cell keeps its identity — is what the batch is for and is wired next; until
    /// then this keeps the array and the layer's window equal, which is the invariant that
    /// matters most.
    private func received(_ incoming: [MessageRow]) {
        guard !incoming.isEmpty else { return }
        let previous = selectedRow()
        rows.append(contentsOf: incoming)
        table.reloadData()
        restore(previous)
    }

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
