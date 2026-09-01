import AppKit
import CSift

/// The shell that is resident for the life of the process.
///
/// D-1 makes this native rather than a web UI, and the three arguments are in stated
/// priority: idle energy first — a native view hierarchy with nothing invalidated *does
/// nothing* — then scroll at scale, then memory, which D-1 itself calls the weakest of the
/// three and a hypothesis rather than a measurement.
///
/// It owns the tray item, the application menu, notification delivery and D-67's six host
/// callbacks. It holds **no view hierarchy, no window, and nothing authoritative** — which is
/// what makes L3 safe: destroying every window shell loses nothing only the network could
/// restore.
///
/// L3 destroys every window shell and **not** this one. Removing the always-on surface would
/// leave the application unreachable, and a resident process the user can only kill is worse
/// than one that used more memory.
final class ApplicationShell: NSObject, NSApplicationDelegate {
    private var app: OpaquePointer?

    /// The container path's bytes. The layer copies during `sift_initialize`; this simply
    /// keeps them alive for the duration of that call.
    private var containerRoot: [UInt8] = []
    private var statusItem: NSStatusItem?
    private var windows: [MainWindowController] = []

    /// The one instance, so the C callbacks below have somewhere to arrive.
    ///
    /// D-67's callbacks are process-scoped and carry no observation, so they have no
    /// generation to be discarded by — which is precisely why the set is closed and
    /// unregistered only at shutdown.
    static let shared = ApplicationShell()

    func applicationDidFinishLaunching(_ notification: Notification) {
        // FR-22's always-on surface. Not late polish: FR-25 distinguishes closing a window
        // from quitting *through this surface*, and FR-2 raises re-authentication through it
        // because Sift may be resident with nothing on screen. A resident window-less build
        // without it can be neither quit deliberately nor re-authenticated.
        installStatusItem()

        // A minimal bar first, so that a launch which fails before the layer exists is still
        // quittable from the keyboard — FR-24 does not allow an application that is frontmost
        // with no way out of it. The register-driven bar replaces this once there is a layer
        // to ask about availability.
        installApplicationMenu()

        var handle: UnsafeMutablePointer<SiftApp>?
        // The container is the shell's to name. The layer never computes one — under the
        // sandbox this resolves inside the app's own container, and a path derived from
        // $HOME would be wrong there and wrong again under Flatpak on the other shell.
        //
        // Application Support and not Caches: the layout MUST NOT live anywhere the
        // operating system may purge on its own, because a purge would remove blobs while
        // leaving the encrypted index referencing them.
        guard let root = Self.containerRoot() else {
            present(startupFailure: Failed)
            return
        }
        containerRoot = Array(root.utf8)
        let status = containerRoot.withUnsafeBufferPointer { bytes -> SiftStatus in
            let init_ = SiftInit(
                container_root: SiftStr(ptr: bytes.baseAddress, len: bytes.count),
                // D-48's hop, and the shell's whole obligation for it: post it, do not run
                // it. Running it inline would hand a callback back from inside the call
                // that caused it, which is the reentrancy D-48 forbids.
                schedule: { _, run, ticket in
                    DispatchQueue.main.async { run?(ticket) }
                },
                schedule_context: nil,
                // D-36 and D-71. The Info.plist registers the scheme, so this bundle has
                // somewhere for an authorization to return to.
                scheme_is_registered: 1
            )
            return sift_initialize(hostCallbacks(), init_, &handle)
        }
        guard status == Ok else {
            // A caught panic is its own status, everywhere. Reporting it as an ordinary
            // failure would erase exactly the distinction D-47 insists on.
            present(startupFailure: status)
            return
        }
        app = OpaquePointer(handle)
        installRegisterMenu()

        addFixtureAccountIfAsked()

        // Both branches open a window; what differs is what the window is *for*. The
        // account-less state *is* the add-account flow rather than an empty inbox, because an
        // empty inbox tells a new user the product is broken.
        if hasAnyAccount() {
            openMainWindow()
        } else {
            beginAddAccount()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        // FR-25, and `docs/architecture/process-model.md` calls this the single most likely
        // source of user distrust in the whole design. Closing the last window sheds the
        // window shell; it does not quit. Sync continues, the queue continues, and the tray
        // is how the user gets back — or leaves deliberately.
        return false
    }

    func applicationWillTerminate(_ notification: Notification) {
        // D-70's teardown is bounded and flushes nothing. Nothing the user watched succeed
        // can be lost, because an intent is durably enqueued *before* it is applied
        // optimistically.
        if let app { _ = sift_shutdown(UnsafeMutablePointer(app)) }
    }

    // MARK: - The always-on surface

    private func installStatusItem() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.image = NSImage(systemSymbolName: "envelope", accessibilityDescription: "Sift")

        let menu = NSMenu()
        // The minimum FR-22 requires: open, quit, and pause sync.
        menu.addItem(withTitle: "Open Sift", action: #selector(openMainWindow), keyEquivalent: "")
        menu.addItem(withTitle: "Pause syncing", action: #selector(pauseSync), keyEquivalent: "")
        menu.addItem(.separator())
        // Deliberately worded as the distinct thing it is. Neither action may be the silent
        // consequence of the other.
        menu.addItem(withTitle: "Quit Sift entirely", action: #selector(quit), keyEquivalent: "q")
        for item in menu.items { item.target = self }
        item.menu = menu
        statusItem = item
    }

    /// The application menu. Deliberately the same three verbs as the tray, plus the window
    /// commands the platform expects to find here.
    ///
    /// "Quit Sift entirely" is worded identically in both places for the reason FR-25 gives:
    /// closing a window and quitting are distinct actions, and neither may be the silent
    /// consequence of the other. Two different words for one of them would undo that.
    private func installApplicationMenu() {
        let applicationMenu = NSMenu()
        applicationMenu.addItem(withTitle: "Open Sift", action: #selector(openMainWindow), keyEquivalent: "")
        applicationMenu.addItem(withTitle: "Pause syncing", action: #selector(pauseSync), keyEquivalent: "")
        applicationMenu.addItem(.separator())
        applicationMenu.addItem(withTitle: "Quit Sift entirely", action: #selector(quit), keyEquivalent: "q")
        for item in applicationMenu.items { item.target = self }

        // Left untargeted on purpose: these travel the responder chain to whichever window is
        // key, which is the only correct answer when windows are plural.
        let windowMenu = NSMenu(title: "Window")
        windowMenu.addItem(withTitle: "Close", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        windowMenu.addItem(
            withTitle: "Minimize",
            action: #selector(NSWindow.performMiniaturize(_:)),
            keyEquivalent: "m"
        )

        let mainMenu = NSMenu()
        // The first submenu is the application menu, and the platform titles it itself.
        let applicationItem = NSMenuItem()
        applicationItem.submenu = applicationMenu
        mainMenu.addItem(applicationItem)

        let windowItem = NSMenuItem()
        windowItem.submenu = windowMenu
        mainMenu.addItem(windowItem)

        NSApp.mainMenu = mainMenu
        NSApp.windowsMenu = windowMenu
    }

    // MARK: - D-98's register

    private var menuBar: MenuBar?
    private var palette: CommandPalette?

    /// Replace the bootstrap menu with one built from the register.
    ///
    /// The reconciliation is checked rather than assumed. An action the layer offers that this
    /// shell binds to nothing is a capability the user cannot reach; one bound here that the
    /// layer does not have is an item that fails when pressed. Neither is tolerable, and
    /// neither announces itself, so the check runs at every launch and the disagreement is
    /// reported where a developer will see it rather than swallowed.
    private func installRegisterMenu() {
        guard let app else { return }
        let bar = MenuBar(app: app) { [weak self] id in self?.invoke(id) }
        let (unbound, unknown) = bar.install()
        menuBar = bar
        palette = CommandPalette(app: app) { [weak self] id in self?.invoke(id) }

        #if DEBUG
        if !unbound.isEmpty || !unknown.isEmpty {
            let alert = NSAlert()
            alert.messageText = "The menu and the action register disagree."
            alert.informativeText = """
                Actions the layer offers with no menu item: \(unbound.joined(separator: ", "))
                Menu items the layer does not know: \(unknown.joined(separator: ", "))
                """
            alert.runModal()
        }
        #endif
    }

    /// Invoke an action by identifier — the one path every gesture takes.
    ///
    /// D-98 makes the action set an ABI surface; the palette is a filtered view of the same
    /// register, and `sift-harness` drives the application through this same entry point
    /// rather than a test-only door. That is what makes FR-24's testability claim real.
    ///
    /// Menus, keys and the palette all arrive here, because three ways to reach an action must
    /// not be three implementations of it. The few actions the shell owns outright — opening
    /// the palette, quitting — are handled before the boundary, and everything else crosses.
    func invoke(_ id: String) {
        switch id {
        case "app.command-palette":
            palette?.present(over: NSApp.keyWindow)
            return
        case "app.quit":
            quit()
            return
        case "app.close-window":
            NSApp.keyWindow?.performClose(nil)
            return
        case "app.new-window", "read.open-message":
            openMainWindow()
            return
        default:
            break
        }
        guard let app else { return }
        var gesture = SiftGesture()
        let status = SiftText.withBytes(id) { ptr, len in
            sift_invoke_action(
                UnsafeMutablePointer(app), ptr, len, nil, 0, 0, &gesture)
        }
        guard status == Ok else {
            // D-98 hides an unavailable action, so reaching one through a key equivalent that
            // the platform matched before the menu rebuilt is the case left over. Saying
            // nothing is the right answer: the action is absent, and an alert would announce
            // a capability the user does not have.
            NSSound.beep()
            return
        }
        // The list is an observation, so the optimistic effect arrives through D-48's hop
        // rather than being applied here. Nothing to do but let it.
    }

    @objc func openMainWindow() {
        // "Open Sift" means *show me Sift*, and it is reachable from the tray, from the
        // application menu and from FR-23's notification. Windows are plural by design, but a
        // second identical window ordered in on top of the first is not plurality — it is one
        // more live observation per click, and on screen it looks like nothing happened.
        // Opening a genuinely new window is a distinct action and will need a distinct verb.
        if let existing = windows.last {
            existing.raise()
            DispatchQueue.main.async { NSApp.activate(ignoringOtherApps: true) }
            return
        }

        guard let app else { return }
        let window = MainWindowController(app: app)
        window.onClose = { [weak self, weak window] in
            guard let window else { return }
            self?.forget(window)
        }
        windows.append(window)

        // Raise the policy *before* the window is ordered in. Putting Sift in the dock and the
        // application switcher is what makes it findable at all, and a window ordered in while
        // the process is still an accessory is a window the activation machinery treats as
        // belonging to something the user cannot switch to.
        syncActivationPolicy()
        window.show()

        // And activate on the next turn of the loop rather than in this one. A policy change is
        // not complete until the current turn ends, so activating inline is activating something
        // the system still considers an accessory — it is accepted and does nothing, which is
        // exactly how "it launched, and nothing came to the front" happens.
        DispatchQueue.main.async { NSApp.activate(ignoringOtherApps: true) }
    }

    /// A window shell is finished with.
    ///
    /// Not bookkeeping: this array is what `syncActivationPolicy` reads, so a shell left in it
    /// after its window closed would hold Sift in the dock with nothing on screen — and it
    /// would hold the shell itself alive along with everything L1 and L3 expect a closing
    /// window to release.
    private func forget(_ shell: MainWindowController) {
        windows.removeAll { $0 === shell }
        syncActivationPolicy()
    }

    /// `main.swift` starts the process as an accessory and says it is one *"until one opens"*.
    /// This is where that stops being a comment.
    ///
    /// The dock reflects whether a window exists, and closing the last one returns Sift to the
    /// menu bar rather than quitting it — FR-25, which
    /// `docs/architecture/process-model.md` calls the single most likely source of user
    /// distrust in the whole design, and which is only credible if the user can *see* the
    /// difference between a closed window and a quit application.
    private func syncActivationPolicy() {
        NSApp.setActivationPolicy(windows.isEmpty ? .accessory : .regular)
    }

    @objc private func pauseSync() { invoke("app.pause-sync") }
    @objc private func quit() { invoke("app.quit"); NSApp.terminate(nil) }

    private func beginAddAccount() {
        invoke("app.add-account")
        // `docs/architecture/ui-shell.md`: **the account-less state is the add-account flow**,
        // not an empty inbox with a hint in it. So first run is a screen, and this is where it
        // opens. The flow's own content is not built yet, so what opens is the main window —
        // honest about the stage rather than invisible about it, because a first run that
        // presents nothing at all is indistinguishable from a launch that failed.
        openMainWindow()
    }

    private var accounts = 0

    private func hasAnyAccount() -> Bool { accounts > 0 }

    /// D-65's recorded corpus, as an account.
    ///
    /// **Off unless asked for.** This is how Sift is driven, and looked at, before a real
    /// mailbox is ever connected: a real adapter over recorded exchanges, with no network, no
    /// credential and nobody's mail in it. It is gated on the environment rather than on a
    /// build configuration so that the thing being looked at is the shipping binary.
    private func addFixtureAccountIfAsked() {
        guard ProcessInfo.processInfo.environment["SIFT_FIXTURES"] != nil, let app else { return }
        let label = "fixtures"
        var id = SiftId.zero
        let bytes = Array(label.utf8)
        let added = bytes.withUnsafeBufferPointer { p in
            sift_add_replayed_account(UnsafeMutablePointer(app), p.baseAddress, p.count, &id)
        }
        guard added == Ok else { return }
        // Blocking, and on the main thread, which is a limitation stated where it happens:
        // the walk belongs on a worker under D-19. Against the recorded corpus it returns
        // immediately, which is why it is tolerable here and would not be against a socket.
        _ = bytes.withUnsafeBufferPointer { p in
            sift_sync_account(UnsafeMutablePointer(app), p.baseAddress, p.count)
        }
        accounts += 1
    }

    /// The container the application's files live under.
    ///
    /// Application Support rather than Caches, because the layout MUST NOT live anywhere the
    /// operating system may purge on its own. Under the sandbox this already resolves inside
    /// the app's own container; outside it, the bundle identifier keeps it to itself.
    private static func containerRoot() -> String? {
        guard
            let base = FileManager.default.urls(
                for: .applicationSupportDirectory, in: .userDomainMask
            ).first
        else { return nil }
        let root = base.appendingPathComponent("net.justinchung.sift", isDirectory: true)
        do {
            try FileManager.default.createDirectory(
                at: root, withIntermediateDirectories: true
            )
        } catch {
            return nil
        }
        return root.path
    }

    private func present(startupFailure status: SiftStatus) {
        // This alert is the only thing Sift puts on screen on this path, and an accessory
        // application has no dock icon and no switcher entry — so a modal it runs can sit
        // behind whatever is frontmost with no ordinary way to reach it. Becoming regular and
        // activating first is the difference between a reported failure and a silent one,
        // which is the whole complaint this shell has just finished answering.
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)

        let alert = NSAlert()
        // D-56: the layer returns identified states and **this shell supplies every word**.
        // No user-visible string crosses the boundary, which is what makes NFR-51's locale
        // awareness and the second shell possible without the layer knowing a language.
        alert.messageText = status == Panicked
            ? "Sift could not start, and the failure was not one it anticipated."
            : "Sift could not start."
        alert.informativeText = "No mail has been changed or lost."
        alert.runModal()
    }
}

// MARK: - D-67's six host callbacks
//
// Registered once, at initialization. Not scoped to any view, not cancellable, and they
// survive the destruction of every window — which is the whole reason they exist rather than
// being observations. Three of them *must* work with no window open, and a window-scoped
// mechanism could deliver none of the three.
//
// Delivery is under D-48's rules: on this shell's main loop, and **not reentrant**. The rule
// inside one of these is *receive, record, return; act on the next turn of the loop.*

private func hostCallbacks() -> SiftHostCallbacks {
    SiftHostCallbacks(
        context: nil,
        destroy_every_window: { _ in
            // L3. Destroys every window shell and leaves this one, because removing the tray
            // would leave the application unreachable.
            DispatchQueue.main.async { NSApp.windows.forEach { $0.close() } }
        },
        reauthentication_needed: { _, _ in
            // FR-2 — the one account condition that must reach the user with **no window
            // open**, which is why it arrives here rather than through a view.
            DispatchQueue.main.async { ApplicationShell.shared.raiseReauthenticationPrompt() }
        },
        bundle_replaced: { _ in
            // FR-26. Sift implements no self-update; the platform channel replaced the bundle
            // underneath the running process, and continuing against replaced resources is
            // what this prevents.
            DispatchQueue.main.async { ApplicationShell.shared.raiseRestartPrompt() }
        },
        notification_activated: { _, _, _ in
            // FR-23. Opens that message — which under FR-25 may mean opening a window on a
            // process that has none.
            DispatchQueue.main.async { ApplicationShell.shared.openMainWindow() }
        },
        account_condition_changed: { _, _, _ in
            // D-49. One condition per account, from an enumerated precedence-ordered set, and
            // under D-66 this shell handles every one **exhaustively at build time** — there
            // is no runtime fallback for an unrecognised value and one must not be added.
            DispatchQueue.main.async { ApplicationShell.shared.refreshAnnunciator() }
        },
        authorization_callback: { _, _ in
            // D-36. Delivered by the platform's launch machinery through the registered URI
            // scheme — not a socket, because NFR-24 admits none for any purpose.
            //
            // Any local process can invoke a registered scheme, which is why D-88's state
            // parameter is doing real work: a callback matching no flow in progress is
            // discarded **without comment**, since reporting it would turn this into a way to
            // make Sift say things.
        }
    )
}

extension ApplicationShell {
    func raiseReauthenticationPrompt() {}
    func raiseRestartPrompt() {}
    func refreshAnnunciator() {}
}
