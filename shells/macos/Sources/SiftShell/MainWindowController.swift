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
    /// Where a gesture from the undo toast goes. The toast does not invoke directly, because
    /// three ways to reach an action must not be three implementations of it.
    var onInvoke: ((String) -> Void)?
    private let annunciator = AnnunciatorView(frame: .zero)
    /// What the list is anchored on. D-4's zero identity is every account merged, and it is
    /// where a window starts.
    private var account: SiftId = .zero
    /// Accounts this process has already fetched for.
    ///
    /// **One round trip per account per run, and no more.** Nothing across this boundary runs
    /// periodically yet — the scheduler is not wired to it — so an account that is never synced
    /// from a gesture is never synced at all, and a person who signed in yesterday opens Sift
    /// to the mail they had yesterday. Until the scheduler is behind this, the gestures that
    /// mean *show me this mailbox* are what fetch it.
    private var synced: Set<String> = []
    private let searchField = NSSearchField()
    private let searchReport = NSTextField(labelWithString: "")
    private let undoBar = UndoBar(frame: .zero)

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
        // The search field sits where the title would be, which is the mail-client idiom and
        // is also the only way to reach it without a toolbar. Showing both put the field on
        // top of the word "Sift" — the window is still named for the switcher and the Window
        // menu, and the name is simply not drawn twice.
        window.titleVisibility = .hidden
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

        // FR-19's search is in the window's own chrome rather than in a pane: it replaces
        // what the list shows, so it cannot live inside the thing it replaces.
        searchField.placeholderString = "Search — from: subject: is:unread in:inbox"
        searchField.target = self
        searchField.action = #selector(runSearch)
        searchField.sendsSearchStringImmediately = false
        searchReport.font = .preferredFont(forTextStyle: .caption1)
        searchReport.textColor = .secondaryLabelColor
        searchReport.lineBreakMode = .byWordWrapping
        searchReport.isHidden = true

        list.onSearchReport = { [weak self] interpretation, caveats, count in
            guard let self else { return }
            self.searchReport.isHidden = false
            // Both, and in this order. The interpretation answers "did it read what I meant",
            // and the caveats answer "is nothing there, or was nothing looked at".
            var text = "\(count) result(s) · read as: \(interpretation)"
            if !caveats.isEmpty { text += "\n\(caveats)" }
            self.searchReport.stringValue = text
        }
        list.onSearchCleared = { [weak self] in
            guard let self else { return }
            self.searchReport.isHidden = true
            self.searchField.stringValue = ""
            // Back to what the sidebar is showing, not to the unified inbox — clearing a
            // search inside one account and landing in every account is a retarget the user
            // did not ask for.
            self.list.observe(app: self.app, account: self.account)
        }

        // The annunciator and the undo toast are window chrome, not panes: they sit over the
        // split view so that neither steals width from a pane, and both are absent rather than
        // blank when there is nothing to say.
        let container = NSView()
        let splitView = split.view
        for view in [splitView, annunciator, undoBar, searchField, searchReport] as [NSView] {
            view.translatesAutoresizingMaskIntoConstraints = false
            // **Every one of them, before any constraint mentions one.** A constraint between
            // two views with no common ancestor raises, and AppKit catches that exception at
            // the top of the event loop and carries on — so the symptom is not a crash and not
            // a layout complaint. It is a window that is never ordered in, from a method that
            // simply stops running, with nothing on stderr. Two of these were missing.
            container.addSubview(view)
        }
        NSLayoutConstraint.activate([
            splitView.topAnchor.constraint(equalTo: container.topAnchor),
            splitView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            splitView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            splitView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            searchField.topAnchor.constraint(equalTo: container.topAnchor, constant: 6),
            searchField.leadingAnchor.constraint(equalTo: container.leadingAnchor, constant: 250),
            searchField.widthAnchor.constraint(equalToConstant: 320),
            searchReport.topAnchor.constraint(equalTo: searchField.bottomAnchor, constant: 4),
            searchReport.leadingAnchor.constraint(equalTo: searchField.leadingAnchor),
            searchReport.widthAnchor.constraint(equalToConstant: 460),
            annunciator.topAnchor.constraint(equalTo: container.topAnchor, constant: 6),
            annunciator.trailingAnchor.constraint(equalTo: container.trailingAnchor, constant: -12),
            undoBar.bottomAnchor.constraint(equalTo: container.bottomAnchor, constant: -20),
            undoBar.centerXAnchor.constraint(equalTo: container.centerXAnchor),
        ])

        let host = NSViewController()
        host.view = container
        host.addChild(split)
        window.contentViewController = host
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
        sidebar.onSelect = { [weak self] account in
            guard let self, !account.same(as: self.account) else { return }
            self.account = account
            self.list.clearSearch()
            self.list.observe(app: self.app, account: account)
            self.fetchOnce(matching: account)
        }
        sidebar.reload(app: app)
        annunciator.show(app: app)
        undoBar.onUndo = { [weak self] in self?.onInvoke?("undo.last-gesture") }

        window.makeKeyAndOrderFront(nil)
        self.window = window
        // **After the window is on screen, not before it.** A fetch blocks this thread, and a
        // launch that waits on the network before drawing anything is the opposite of what a
        // resident mail client should do. Ordering the window in first means a person sees
        // Sift and then sees it fill, rather than seeing nothing and wondering.
        DispatchQueue.main.async { [weak self] in self?.fetchOnce(matching: .zero) }
        // Every `Window`-scoped action in the register turns on this, so a layer that was
        // never told a window exists offers a menu to nobody.
        _ = sift_set_window_present(UnsafeMutablePointer(app), 1)
    }

    /// Close this shell's window — L3's shed, issued by the application shell.
    ///
    /// Goes through the window so that `windowWillClose` runs and the shell is forgotten the
    /// same way a user closing it would be forgotten. Tearing the view hierarchy down without
    /// that would leave this controller in the array `syncActivationPolicy` reads, holding
    /// Sift in the dock with nothing on screen.
    func close() {
        // **A window with a sheet attached is a user interaction in progress**, and on this
        // window that is almost always the add-account flow. The platform disables the close
        // button while a sheet is up, so `performClose` here is a beep and nothing else —
        // memory pressure making the machine chime. Forcing it would be worse: ending the
        // sheet cancels the flow through `windowWillClose`, and the user who then grants
        // consent in the browser comes back to nothing, which is D-71's failure arriving from
        // the memory governor.
        guard window?.attachedSheet == nil else { return }
        window?.performClose(nil)
    }

    /// Bring an existing window forward. "Open Sift" means *show me Sift*, not *make another*.
    func raise() {
        window?.makeKeyAndOrderFront(nil)
    }

    /// What a sheet attaches to. D-97 puts the add-account flow on *the window that started
    /// it*, and `NSApp.keyWindow` is nil when the gesture came from the tray.
    var hostWindow: NSWindow? { window }

    /// Whether the next search is scoped to this window's account — `search.narrow-to-account`.
    private var narrowed = false

    @objc private func runSearch() {
        let query = searchField.stringValue
        if query.trimmingCharacters(in: .whitespaces).isEmpty {
            list.clearSearch()
        } else {
            list.search(query, app: app, account: narrowed ? account : .zero)
        }
    }

    /// Put the caret in the search field — `search.begin`.
    func focusSearch() {
        window?.makeFirstResponder(searchField)
    }

    /// `search.clear`: back to the list the observation maintains.
    func clearSearch() {
        narrowed = false
        list.clearSearch()
    }

    // MARK: - The gestures this window owns
    //
    // Every one of these is in D-98's register with no intent behind it, so invoking them
    // across the boundary returned success and did nothing. Where the caret is, which pane has
    // the keyboard, and which account a window is looking at are facts about this window; the
    // layer cannot answer them and should not be asked to.

    /// `read.next-message`, `read.previous-message`, and the two that skip to unread.
    func moveSelection(by step: Int, unreadOnly: Bool) {
        list.move(by: step, unreadOnly: unreadOnly)
    }

    /// `navigate.focus-sidebar`, `navigate.focus-list`, `navigate.focus-reader`.
    func focus(_ pane: Pane) {
        switch pane {
        case .sidebar: window?.makeFirstResponder(sidebar.focusTarget)
        case .list: window?.makeFirstResponder(list.focusTarget)
        case .reader: window?.makeFirstResponder(reader.view)
        }
    }

    enum Pane { case sidebar, list, reader }

    /// `navigate.next-account` and `navigate.previous-account`.
    func stepAccount(by delta: Int) {
        sidebar.step(by: delta)
    }

    /// `navigate.unified-inbox` — D-4's merged stream, which is the zero anchor.
    func showUnifiedInbox() {
        sidebar.selectUnified()
    }

    /// `read.toggle-dark-transform` — FR-31, per message.
    func toggleDarkTransform() {
        reader.toggleDarkTransform(app: app)
    }

    /// `search.narrow-to-account`: search the account this window is looking at.
    ///
    /// **A scope, not a query term.** The boundary takes the same anchor the list observation
    /// does, so narrowing is a fact the window already holds rather than a word to spell,
    /// parse, translate and explain in FR-21's interpretation.
    func narrowSearchToAccount() {
        narrowed = true
        if searchField.stringValue.trimmingCharacters(in: .whitespaces).isEmpty {
            window?.makeFirstResponder(searchField)
            return
        }
        runSearch()
    }

    /// Redraw what a gesture may have changed.
    ///
    /// Called after every invocation rather than on a timer: a resident process that polls its
    /// own state is exactly the idle wakeup NFR-11 counts, and neither of these changes without
    /// something happening.
    /// The row the user is looking at, for the surfaces that act on one.
    var selectedRow: MessageRow? { list.selection }

    func refreshChrome() {
        annunciator.show(app: app)
        undoBar.refresh(app: app)
    }

    /// The account list changed — one was added, or its condition did.
    ///
    /// Called rather than polled: a resident process that watches its own state is the idle
    /// wakeup NFR-11 counts, and neither of those changes without something happening.
    /// Fetch the accounts this anchor covers, once each per run.
    ///
    /// **This blocks the main thread for the length of a walk**, which is stated rather than
    /// hidden behind a spinner: the work belongs on a worker under D-19, and moving it there
    /// changes nothing a shell can see because every delivery already arrives through D-48's
    /// hop rather than out of this call.
    private func fetchOnce(matching anchor: SiftId) {
        let unified = anchor.same(as: .zero)
        for account in Account.all(app: app) {
            guard unified || account.id.same(as: anchor) else { continue }
            guard synced.insert(account.name).inserted else { continue }
            _ = SiftText.withBytes(account.name) { ptr, len in
                sift_sync_account(UnsafeMutablePointer(app), ptr, len)
            }
        }
        sidebar.reload(app: app)
        refreshChrome()
    }

    func refreshAccounts() {
        sidebar.reload(app: app)
        refreshChrome()
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
