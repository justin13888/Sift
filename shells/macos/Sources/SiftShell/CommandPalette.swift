import AppKit
import CSift

/// D-98's palette: **a filtered view of the register, never a list of its own.**
///
/// That distinction is the whole design. A palette with its own list is a second register that
/// drifts from the first — an action added below the boundary is missing from it, and one
/// removed lingers in it and fails when chosen. So this asks the layer which actions are
/// available, in register order, and shows exactly those.
///
/// It shows the same words the menu does, because two names for one action is two actions as
/// far as a user is concerned.
final class CommandPalette: NSWindowController {
    private let app: OpaquePointer
    private let onInvoke: (String) -> Void
    private let field = NSSearchField()
    private let table = NSTableView()
    private var available: [(id: String, title: String)] = []
    private var shown: [(id: String, title: String)] = []

    init(app: OpaquePointer, onInvoke: @escaping (String) -> Void) {
        self.app = app
        self.onInvoke = onInvoke
        let window = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 560, height: 360),
            styleMask: [.titled, .closable, .fullSizeContentView, .nonactivatingPanel],
            backing: .buffered, defer: false)
        window.titleVisibility = .hidden
        window.titlebarAppearsTransparent = true
        window.isFloatingPanel = true
        window.level = .floating
        super.init(window: window)
        build()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    private func build() {
        field.placeholderString = "Command"
        field.target = self
        field.action = #selector(filter)
        field.sendsSearchStringImmediately = true

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("command"))
        column.resizingMask = .autoresizingMask
        table.addTableColumn(column)
        table.headerView = nil
        table.rowHeight = 28
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(run)
        table.style = .plain

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        field.translatesAutoresizingMaskIntoConstraints = false

        let content = NSView()
        content.addSubview(field)
        content.addSubview(scroll)
        NSLayoutConstraint.activate([
            field.topAnchor.constraint(equalTo: content.topAnchor, constant: 12),
            field.leadingAnchor.constraint(equalTo: content.leadingAnchor, constant: 12),
            field.trailingAnchor.constraint(equalTo: content.trailingAnchor, constant: -12),
            scroll.topAnchor.constraint(equalTo: field.bottomAnchor, constant: 10),
            scroll.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])
        window?.contentView = content
    }

    /// Refresh from the layer and show. The list is read at the moment it is shown, because
    /// availability turns on the selection and the selection changed a keystroke ago.
    func present(over parent: NSWindow?) {
        available = CommandPalette.read(app: app)
        field.stringValue = ""
        shown = available
        table.reloadData()
        if !shown.isEmpty {
            table.selectRowIndexes([0], byExtendingSelection: false)
        }
        guard let window else { return }
        if let parent {
            let frame = parent.frame
            window.setFrameTopLeftPoint(
                NSPoint(
                    x: frame.midX - window.frame.width / 2,
                    y: frame.maxY - 120))
        } else {
            window.center()
        }
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(field)
    }

    /// The available actions, in register order, with the shell's words.
    ///
    /// An action the layer offers and this shell has no title for still appears, under its
    /// identifier. Hiding it would make the palette a list of its own again — silently, and
    /// exactly for the action nobody had got round to naming.
    private static func read(app: OpaquePointer) -> [(id: String, title: String)] {
        var out: [(String, String)] = []
        for index in 0..<sift_action_count() {
            var raw = SiftStr()
            guard sift_action_id(index, &raw) == Ok else { continue }
            let id = SiftText.string(raw)
            var flag: UInt8 = 0
            let ok = SiftText.withBytes(id) { ptr, len in
                sift_action_available(UnsafeMutablePointer(app), ptr, len, &flag) == Ok
            }
            guard ok, flag != 0 else { continue }
            out.append((id, ActionRegister.titles[id] ?? id))
        }
        return out
    }

    @objc private func filter() {
        let query = field.stringValue.lowercased()
        shown =
            query.isEmpty
            ? available
            : available.filter {
                $0.title.lowercased().contains(query) || $0.id.lowercased().contains(query)
            }
        table.reloadData()
        if !shown.isEmpty {
            table.selectRowIndexes([0], byExtendingSelection: false)
        }
    }

    @objc private func run() {
        let index = table.selectedRow
        guard index >= 0, index < shown.count else { return }
        let id = shown[index].id
        window?.orderOut(nil)
        onInvoke(id)
    }
}

extension CommandPalette: NSTableViewDataSource, NSTableViewDelegate {
    func numberOfRows(in tableView: NSTableView) -> Int { shown.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int)
        -> NSView?
    {
        let label = NSTextField(labelWithString: shown[row].title)
        label.font = .preferredFont(forTextStyle: .body)
        let cell = NSTableCellView()
        cell.addSubview(label)
        label.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 10),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        return cell
    }
}
