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

    private struct Entry {
        let title: String
        let symbol: String
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
    /// Folders arrive through their own observation once the boundary carries one; until then
    /// the unified inbox is what the sidebar can honestly offer, and offering a folder tree
    /// that is not wired would be worse than offering none.
    func reload(app: OpaquePointer?) {
        entries = [Entry(title: "All Inboxes", symbol: "tray.2")]
        outline.reloadData()
        outline.selectRowIndexes([0], byExtendingSelection: false)
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
        cell.imageView?.image = NSImage(
            systemSymbolName: entry.symbol, accessibilityDescription: entry.title)
        return cell
    }
}
