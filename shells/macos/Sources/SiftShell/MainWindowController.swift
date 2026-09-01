import AppKit
import CSift

/// D-97's main window: sidebar, list, reader.
///
/// **A window is a view onto the presentation layer, never an instance of it.** Several may
/// be open, each with its own observations over the same layer, and closing one destroys its
/// views and its observations and nothing else. That is why nothing here holds application
/// state — the rows a list has are the layer's window, borrowed.
final class MainWindowController: NSObject, NSWindowDelegate {
    private var window: NSWindow?
    private let app: OpaquePointer
    private let split = NSSplitViewController()
    private let sidebar = SidebarViewController()
    private let list = MessageListViewController()
    private let reader = ReaderViewController()

    /// Told when the last window closes, so FR-25's distinction can be honoured: closing a
    /// window lowers Sift back to the menu bar and is **not** quitting.
    var onClose: (() -> Void)?

    init(app: OpaquePointer) {
        self.app = app
        super.init()
    }

    func show() {
        // Default and minimum both stated. The minimum is the point below which three panes
        // stop being three panes rather than a number that looked tidy.
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1280, height: 820),
            styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
            backing: .buffered,
            defer: false
        )
        window.title = "Sift"
        window.minSize = NSSize(width: 900, height: 560)
        window.titlebarAppearsTransparent = false
        window.delegate = self
        // FR-25 again, from the other side: the window owns nothing, so releasing it when it
        // closes would free a controller the close handler is still running inside.
        window.isReleasedWhenClosed = false

        let sidebarItem = NSSplitViewItem(sidebarWithViewController: sidebar)
        sidebarItem.minimumThickness = 180
        sidebarItem.maximumThickness = 360
        sidebarItem.canCollapse = true

        let listItem = NSSplitViewItem(contentListWithViewController: list)
        listItem.minimumThickness = 300
        listItem.canCollapse = false
        // The reader absorbs the surplus when the window grows: a list is a fixed-width
        // scanning column and a body is what benefits from the space. Holding priorities say
        // which pane yields, and without them all three sit at their minimums and the window
        // opens looking like it was laid out by accident.
        listItem.holdingPriority = .init(261)

        let readerItem = NSSplitViewItem(viewController: reader)
        readerItem.minimumThickness = 420
        readerItem.canCollapse = false
        readerItem.holdingPriority = .init(250)

        split.addSplitViewItem(sidebarItem)
        split.addSplitViewItem(listItem)
        split.addSplitViewItem(readerItem)

        window.contentViewController = split
        // **After** the content view controller, not before. Assigning one re-derives the
        // window's frame from the controller's own fitting size, and a split view with three
        // panes that have minimums and no preferred sizes fits to the sum of the minimums —
        // so a window created at 1280x820 opens at 900x560 and nothing says why.
        window.setContentSize(NSSize(width: 1280, height: 820))
        window.setFrameAutosaveName("SiftMainWindow")
        window.center()

        // Sidebar 240, list 380, reader the rest. Stated here because the spec deliberately
        // states no geometry, and a proportion nobody wrote down is one every future change
        // has to re-derive.
        split.splitView.setPosition(240, ofDividerAt: 0)
        split.splitView.setPosition(240 + 380, ofDividerAt: 1)

        // The reader follows the list's selection. D-54: native rows over one body view, and
        // the body view belongs to the reader rather than to the row.
        list.onSelect = { [weak self] row in
            guard let self else { return }
            self.reader.show(row, app: self.app)
            // D-99's selection has to cross, or every message-scoped action stays hidden and
            // the whole Message menu is empty. Keyed on identity rather than index: a row that
            // moves under a selection is the same message, and a selection that followed the
            // index would silently retarget the gesture.
            var ids = row.map { [$0.id] } ?? []
            _ = sift_select(UnsafeMutablePointer(self.app), &ids, ids.count)
        }

        // D-4's unified inbox is the zero anchor: every account, merged on one comparator.
        list.observe(app: app, account: .zero)
        sidebar.reload(app: app)

        window.makeKeyAndOrderFront(nil)
        self.window = window
        // Every `Window`-scoped action in the register turns on this, so a layer that was
        // never told a window exists offers a menu to nobody.
        _ = sift_set_window_present(UnsafeMutablePointer(app), 1)
    }

    /// Bring an existing window forward. "Open Sift" means *show me Sift*, not *make another*.
    func raise() {
        window?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        // Cancellation **first**, and it is synchronous: when it returns no delivery for
        // these observations can arrive, on any thread, ever. Tearing the views down first
        // would leave a delivery in flight with somewhere to land and nothing there.
        list.cancel()
        // FR-25: the window is gone and the process is not. The layer is told, so the actions
        // that need a window stop being offered — including through the tray, which is the
        // surface that is still there.
        _ = sift_set_window_present(UnsafeMutablePointer(app), 0)
        window = nil
        onClose?()
    }
}
