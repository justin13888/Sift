import AppKit
import CSift

/// D-97's account and folder sidebar.
///
/// The unified inbox sits above the accounts rather than inside one, because D-4 makes it the
/// primary reason a person runs a multi-account client at all — putting it under an account
/// would be filing it under one of the things it exists to combine.
final class SidebarViewController: NSViewController {
    private let outline = NSOutlineView()
    private let scroll = NSScrollView()
    private var entries: [Entry] = []

    /// Told which account to show — D-4's zero identity is every account merged.
    var onSelect: ((SiftId) -> Void)?

    private struct Entry {
        let title: String
        let symbol: String
        /// The observation anchor. Zero is the unified inbox.
        let account: SiftId
        /// D-49's condition for this account, drawn beside it. `nil` for the unified row,
        /// which is not an account and has no condition of its own.
        let condition: SiftCondition?
    }

    override func loadView() {
        outline.headerView = nil
        outline.rowSizeStyle = .default
        outline.floatsGroupRows = false
        outline.indentationPerLevel = 14
        outline.dataSource = self
        outline.delegate = self
        outline.selectionHighlightStyle = .sourceList

        let column = NSTableColumn(identifier: .init("item"))
        column.resizingMask = .autoresizingMask
        outline.addTableColumn(column)
        outline.outlineTableColumn = column

        scroll.documentView = outline
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false

        // The sidebar material, which is what makes a sidebar read as one on this platform.
        let effect = NSVisualEffectView()
        effect.material = .sidebar
        effect.blendingMode = .behindWindow
        effect.autoresizingMask = [.width, .height]
        scroll.frame = effect.bounds
        scroll.autoresizingMask = [.width, .height]
        effect.addSubview(scroll)
        view = effect
    }

    /// Fill from what the layer holds.
    ///
    /// **The accounts are asked for rather than assumed.** This used to be one hardcoded row
    /// that ignored the argument it was handed, so an account a person had signed in to was
    /// invisible in the one surface whose job is to show it, and there was no way to look at
    /// one mailbox rather than all of them.
    ///
    /// Folders arrive through their own observation once the boundary carries one; until then
    /// an account is a leaf, and offering a folder tree that is not wired would be worse than
    /// offering none.
    func reload(app: OpaquePointer?) {
        let previous = selectedAccount()
        // D-4's unified inbox is above the accounts rather than inside one: putting it under
        // an account would be filing it under one of the things it exists to combine.
        entries = [Entry(title: "All Inboxes", symbol: "tray.2", account: .zero, condition: nil)]
        for account in Account.all(app: app) {
            entries.append(
                Entry(
                    title: account.name,
                    symbol: "tray",
                    account: account.id,
                    condition: account.condition))
        }
        outline.reloadData()
        // Hold the selection across a reload. An account added while the unified inbox was
        // showing must not silently retarget the list to something else.
        let row = entries.firstIndex { $0.account.same(as: previous) } ?? 0
        outline.selectRowIndexes([row], byExtendingSelection: false)
    }

    private func selectedAccount() -> SiftId {
        let row = outline.selectedRow
        guard row >= 0, row < entries.count else { return .zero }
        return entries[row].account
    }
}

extension SidebarViewController: NSOutlineViewDataSource, NSOutlineViewDelegate {
    func outlineView(_ outlineView: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
        item == nil ? entries.count : 0
    }

    func outlineView(_ outlineView: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
        index
    }

    func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool { false }

    func outlineViewSelectionDidChange(_ notification: Notification) {
        onSelect?(selectedAccount())
    }

    func outlineView(
        _ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any
    ) -> NSView? {
        guard let index = item as? Int, index < entries.count else { return nil }
        let entry = entries[index]
        let identifier = NSUserInterfaceItemIdentifier("sidebar")
        let cell =
            outlineView.makeView(withIdentifier: identifier, owner: self) as? NSTableCellView
            ?? {
                let c = NSTableCellView()
                c.identifier = identifier
                let image = NSImageView()
                let text = NSTextField(labelWithString: "")
                text.font = .preferredFont(forTextStyle: .body)
                let stack = NSStackView(views: [image, text])
                stack.orientation = .horizontal
                stack.spacing = 6
                stack.translatesAutoresizingMaskIntoConstraints = false
                c.addSubview(stack)
                c.textField = text
                c.imageView = image
                NSLayoutConstraint.activate([
                    stack.leadingAnchor.constraint(equalTo: c.leadingAnchor, constant: 4),
                    stack.centerYAnchor.constraint(equalTo: c.centerYAnchor),
                    stack.trailingAnchor.constraint(
                        lessThanOrEqualTo: c.trailingAnchor, constant: -4),
                ])
                return c
            }()
        cell.textField?.stringValue = entry.title
        // D-49 draws the worst condition once, in the annunciator. Here it is only a tint on
        // the row, so an account that needs something is findable in a list of five without
        // the sidebar growing a second badge that says the same thing in a different place.
        cell.textField?.textColor =
            (entry.condition.map { $0 != Annunciator.healthy } ?? false)
            ? .systemOrange : .labelColor
        cell.imageView?.image = NSImage(
            systemSymbolName: entry.symbol, accessibilityDescription: entry.title)
        return cell
    }
}
