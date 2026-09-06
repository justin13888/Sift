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

    /// The same, for the two facts the layer is given about this bundle's OAuth configuration.
    private var oauthClientID: [UInt8] = []
    private var registeredSchemes: [UInt8] = []
    /// The platform timers the layer has asked for, by ticket.
    ///
    /// Held because a `DispatchSourceTimer` that nothing retains is cancelled when it is
    /// deallocated, and a cancelled timer fires nothing — which would look exactly like a
    /// wheel that stopped. Each is removed as it fires, so this holds at most one.
    fileprivate var timers: [UInt64: DispatchSourceTimer] = [:]

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

        // D-93's signal, **subscribed to rather than polled**. Polling free memory is both a
        // wakeup counted against NFR-11 and a worse answer than the one the system already
        // has — it knows about pressure before free memory reflects it.
        installMemoryPressureSource()

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
        // D-36, D-71 and D-109: **read out of the bundle, not asserted about it.** This used
        // to be `scheme_is_registered: 1` beside a comment saying the Info.plist registered
        // the scheme. A configuration shipped without it, the layer was told otherwise, and
        // the refusal that exists to happen before a browser opens could not happen at all.
        //
        // What a shell can honestly report is what its own bundle claims. Which scheme the
        // configured client requires, and whether it is among them, is the layer's to decide.
        oauthClientID = Array(Self.configuredClientID.utf8)
        registeredSchemes = Array(Self.claimedURLSchemes().joined(separator: "\n").utf8)
        let status = containerRoot.withUnsafeBufferPointer { bytes -> SiftStatus in
            oauthClientID.withUnsafeBufferPointer { client in
                registeredSchemes.withUnsafeBufferPointer { schemes in
                    let init_ = SiftInit(
                        container_root: SiftStr(ptr: bytes.baseAddress, len: bytes.count),
                        // D-48's hop, and the shell's whole obligation for it: post it, do not
                        // run it. Running it inline would hand a callback back from inside the
                        // call that caused it, which is the reentrancy D-48 forbids.
                        schedule: { _, run, ticket in
                            DispatchQueue.main.async { run?(ticket) }
                        },
                        schedule_context: nil,
                        // D-25's platform timer, which is the one thing only a shell can do.
                        // The layer decides *when* from its own wheel; this asks the platform
                        // for a timer that is **allowed to fire late**, and that permission is
                        // the entire mechanism by which wakeups coalesce. An exact timer here
                        // would satisfy the signature and fail NFR-11.
                        arm_timer: { _, run, ticket, delayMillis, leewayMillis in
                            // **On the main queue, like `schedule` above it.** The layer may
                            // call either from a worker, and `timers` is a Swift dictionary —
                            // an insert racing the event handler's removal is heap corruption
                            // rather than a lost entry. The two closures share one contract
                            // and must share one confinement.
                            DispatchQueue.main.async {
                                let timer = DispatchSource.makeTimerSource(queue: .main)
                                timer.schedule(
                                    deadline: .now() + .milliseconds(Int(delayMillis)),
                                    leeway: .milliseconds(Int(leewayMillis))
                                )
                                timer.setEventHandler {
                                    // Released after it fires. A repeating source would be a
                                    // second schedule beside the wheel's, and the two would
                                    // disagree the moment an account was added.
                                    ApplicationShell.shared.timers.removeValue(forKey: ticket)
                                    run?(ticket)
                                }
                                ApplicationShell.shared.timers[ticket] = timer
                                timer.resume()
                            }
                        },
                        oauth_client_id: SiftStr(ptr: client.baseAddress, len: client.count),
                        registered_schemes: SiftStr(ptr: schemes.baseAddress, len: schemes.count)
                    )
                    return sift_initialize(hostCallbacks(), init_, &handle)
                }
            }
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
        // The badge says what is true of the accounts the container already held. Asked once,
        // here, rather than waited for: the push exists for what changes afterwards, and a
        // shell that only had the push would draw nothing until something did.
        refreshAnnunciator()
        refreshTrayState()
    }

    /// D-36's callback, arriving through the registered URI scheme.
    ///
    /// **This is the whole of how an authorization returns.** NFR-24 admits no listening socket
    /// for any purpose, so there is no loopback redirect and nothing here binds a port — the
    /// platform's own launch machinery hands Sift the address.
    ///
    /// It is also one of only two local attack surfaces Sift has: any process running as the
    /// user can invoke a registered scheme. The layer discards a callback whose state matches
    /// no flow in progress **without comment**, and this method reports nothing either, because
    /// a message here would turn Sift into a way to find out whether a guess was received.
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls {
            addAccount?.callbackArrived(url.absoluteString)
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
        // Cancelled before the layer goes: a source that outlives it would fire into freed
        // memory if the process lingered, and one that never fires holds its own allocation
        // for the life of the process. `sift_shutdown` abandons the tickets on its side.
        for timer in timers.values { timer.cancel() }
        timers.removeAll()
        if let app { _ = sift_shutdown(UnsafeMutablePointer(app)) }
    }

    /// The platform's own memory-pressure source.
    ///
    /// Held for the life of the process: a `DispatchSource` that nothing retains is cancelled
    /// when it is deallocated, and a cancelled source delivers nothing — which would look
    /// exactly like a system that never came under pressure.
    private var pressureSource: DispatchSourceMemoryPressure?

    private func installMemoryPressureSource() {
        let source = DispatchSource.makeMemoryPressureSource(
            eventMask: [.normal, .warning, .critical], queue: .main)
        source.setEventHandler { [weak self] in
            guard let self, let app = self.app else { return }
            let data = source.data
            // Ordered worst-first: the mask can carry more than one bit, and the honest
            // reading of "warning and critical" is critical.
            let level: UInt32 = data.contains(.critical) ? 2 : (data.contains(.warning) ? 1 : 0)
            var tier: UInt32 = 0
            _ = sift_memory_pressure(UnsafeMutablePointer(app), level, &tier)
        }
        source.resume()
        pressureSource = source
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

    /// Whether a preference-gated surface is on. D-101 has both off by default.
    private func settingIsOn(_ key: String) -> Bool {
        guard let app else { return false }
        var rows = SiftRows_SiftSetting()
        guard sift_settings(UnsafeMutablePointer(app), &rows) == Ok, let ptr = rows.ptr else {
            return false
        }
        for index in 0..<rows.len where SiftText.string(ptr[index].key) == key {
            return SiftText.string(ptr[index].value) == "true"
        }
        return false
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
        case "read.open-in-standalone-reader":
            guard let app, let row = windows.first?.selectedRow else { return }
            // A window per message rather than one that retargets: D-97 makes this its own
            // window kind, and a second message opening in the first would be the retarget
            // this exists instead of.
            let reader = StandaloneReader(app: app, row: row)
            standaloneReaders.append(reader)
            reader.showWindow(nil)
            return
        case "app.open-message-debug-view":
            guard let app, let row = windows.first?.selectedRow else { return }
            // Gated here rather than in the menu, so a key equivalent cannot reach past the
            // preference. D-101 has it off by default.
            guard settingIsOn("debug.message-view") else {
                let alert = NSAlert()
                alert.messageText = "Message details are turned off."
                alert.informativeText =
                    "Turn them on in Settings. They are off by default because a debug surface "
                    + "that is on by default is one whose cost nobody measured."
                alert.runModal()
                return
            }
            let window = debugWindow ?? MessageDebugWindow(app: app)
            debugWindow = window
            window.present(row)
            return
        case "app.pause-sync", "app.resume-sync":
            // **D-95 puts the pause on the account, and this menu item is over all of them.**
            // The register has one application-scoped action rather than one per account, so
            // the shell is what fans it out; the layer holds the flag, one per account, and
            // D-49 turns it into a condition the annunciator draws.
            setPaused(id == "app.pause-sync")
            return
        case "undo.last-gesture":
            // **Its own entry point, not the register's.** `undo.last-gesture` is in D-98's
            // set and has no intent behind it, so invoking it across the boundary returned
            // success and reversed nothing — which is what the undo toast was wired to. The
            // reversal acts over D-85's undo group rather than over a message, so a bulk
            // operation reverses as the one gesture FR-17 promises.
            guard let app else { return }
            var reversal = SiftGesture()
            if sift_undo_last(UnsafeMutablePointer(app), &reversal) != Ok {
                // Nothing to reverse, or nothing about it was reversible. D-98's rule for an
                // action that is not available is silence, not an alert announcing a
                // capability the user does not have.
                NSSound.beep()
                return
            }
            for window in windows { window.refreshChrome() }
            return
        case "app.add-account":
            // **The one gesture that was bound to nothing.** It reached the register, crossed
            // the boundary, found an action with no intent behind it and came back `Ok` — so
            // nothing happened and nothing said so. Adding an account is the shell's own work:
            // there is no intent for it, and the layer has no window to open.
            beginAddAccount()
            return
        case "app.open-settings":
            guard let app else { return }
            let window = settingsWindow ?? SettingsWindow(app: app)
            settingsWindow = window
            window.present()
            return
        case "app.open-runtime-panel":
            guard let app else { return }
            guard settingIsOn("debug.runtime-panel") else {
                let alert = NSAlert()
                alert.messageText = "The runtime window is turned off."
                alert.informativeText =
                    "Turn it on in Settings. It is off by default, and it is where you can see "
                    + "that an account Sift is only watching has sent nothing."
                alert.runModal()
                return
            }
            let panel = runtimePanel ?? RuntimePanel(app: app)
            runtimePanel = panel
            panel.present()
            return
        case "read.next-message":
            windows.first?.moveSelection(by: 1, unreadOnly: false)
            return
        case "read.previous-message":
            windows.first?.moveSelection(by: -1, unreadOnly: false)
            return
        case "read.next-unread":
            windows.first?.moveSelection(by: 1, unreadOnly: true)
            return
        case "read.previous-unread":
            windows.first?.moveSelection(by: -1, unreadOnly: true)
            return
        case "navigate.focus-sidebar":
            windows.first?.focus(.sidebar)
            return
        case "navigate.focus-list":
            windows.first?.focus(.list)
            return
        case "navigate.focus-reader":
            windows.first?.focus(.reader)
            return
        case "navigate.unified-inbox":
            windows.first?.showUnifiedInbox()
            return
        case "navigate.next-account":
            windows.first?.stepAccount(by: 1)
            return
        case "navigate.previous-account":
            windows.first?.stepAccount(by: -1)
            return
        case "read.toggle-dark-transform":
            windows.first?.toggleDarkTransform()
            return
        case "search.narrow-to-account":
            windows.first?.narrowSearchToAccount()
            return
        case "read.reply", "read.reply-all", "read.forward":
            // FR-41. **Sift constructs nothing and sends nothing** — there is no compose
            // window and no outgoing server anywhere in this binary, and these hand the
            // message to whatever the platform has registered for mail. A handoff that
            // silently did nothing would be the worst of both: the product's largest adoption
            // objection, answered with a shrug.
            handOff(id)
            return
        case "message.add-tag", "message.remove-tag":
            // **The parameter comes from the gesture, not the register.** These two carry a
            // tag, the boundary takes one, and this shell had nothing to put in it — so both
            // items were offered and both failed when pressed. A tag is a string a person
            // types, which is what makes this a sheet rather than a picker: unlike a folder,
            // there is no set to choose from across this boundary.
            promptForTag(id)
            return
        case "message.permanently-delete":
            // FR-14's single exception, and the only intent with no compensation. **Confirmed
            // before it is issued** rather than undone afterwards, because there is nothing to
            // undo once it has happened — so the layer refuses an unconfirmed one, and this
            // shell was passing `confirmed: 0` and beeping.
            confirmPermanentDelete()
            return
        case "search.begin", "navigate.focus-search":
            windows.first?.focusSearch()
            return
        case "search.clear":
            windows.first?.clearSearch()
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
        // rather than being applied here. The chrome is not an observation, so it is asked
        // once, here, where something is known to have happened.
        for window in windows { window.refreshChrome() }
    }

    /// FR-41's handoff: the message, in the user's own mail application.
    ///
    /// A `mailto:` address and nothing more. Sift has no draft to hand over — it has never
    /// parsed one, it has no place to put one, and building one here would be the first line
    /// of the send path scope forbids outright.
    private func handOff(_ id: String) {
        guard let row = windows.first?.selectedRow else {
            NSSound.beep()
            return
        }
        var components = URLComponents()
        components.scheme = "mailto"
        // Reply goes to the sender; forward goes to nobody, because the user picks. Reply-all
        // needs every recipient, and the envelope this shell is given carries one address —
        // so it is the same address, and the difference is the subject rather than a promise
        // about recipients Sift cannot keep.
        components.path = id == "read.forward" ? "" : address(in: row.sender)
        let prefix = id == "read.forward" ? "Fwd: " : "Re: "
        let subject = row.subject.hasPrefix(prefix) ? row.subject : prefix + row.subject
        components.queryItems = [URLQueryItem(name: "subject", value: subject)]
        guard let url = components.url, NSWorkspace.shared.open(url) else {
            let alert = NSAlert()
            alert.messageText = "Sift could not hand this message to a mail app."
            alert.informativeText =
                "Sift does not send mail — Reply and Forward pass the message to whichever "
                + "app handles mail on this Mac, and this Mac has none set up."
            alert.runModal()
            return
        }
    }

    /// The address out of a `Name <address>` envelope, or the whole string when there is none.
    ///
    /// NFR-54 requires the address be shown beside the display name rather than instead of it,
    /// so the row carries both and this takes the half a mail handler can use.
    private func address(in sender: String) -> String {
        guard let open = sender.lastIndex(of: "<"), let close = sender.lastIndex(of: ">"),
            open < close
        else { return sender.trimmingCharacters(in: .whitespaces) }
        return String(sender[sender.index(after: open)..<close])
    }

    /// Ask for the tag `message.add-tag` and `message.remove-tag` carry.
    private func promptForTag(_ id: String) {
        guard let app, let host = windows.first?.hostWindow else {
            NSSound.beep()
            return
        }
        let adding = id == "message.add-tag"
        let alert = NSAlert()
        alert.messageText = adding ? "Add a tag" : "Remove a tag"
        alert.informativeText = adding
            ? "The tag is applied to every message in the selection."
            : "The tag is removed from every message in the selection that has it."
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 240, height: 24))
        field.placeholderString = "Tag"
        alert.accessoryView = field
        alert.addButton(withTitle: adding ? "Add" : "Remove")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: host) { [weak self] response in
            guard response == .alertFirstButtonReturn, let self else { return }
            let tag = field.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
            // An empty tag is not a tag. Refused here rather than sent, because the layer
            // would refuse it and the refusal would arrive as a beep with no cause.
            guard !tag.isEmpty else { return }
            var gesture = SiftGesture()
            let status = SiftText.withBytes(id) { idPtr, idLen in
                SiftText.withBytes(tag) { tagPtr, tagLen in
                    sift_invoke_action(
                        UnsafeMutablePointer(app), idPtr, idLen, tagPtr, tagLen, 0, &gesture)
                }
            }
            if status != Ok { NSSound.beep() }
            for window in self.windows { window.refreshChrome() }
        }
        // The sheet is up; the field is where the person is going next.
        DispatchQueue.main.async { host.makeFirstResponder(field) }
    }

    /// FR-14's one confirmation, as a sheet on the window that asked.
    private func confirmPermanentDelete() {
        guard let app, let host = windows.first?.hostWindow else {
            NSSound.beep()
            return
        }
        let alert = NSAlert()
        alert.messageText = "Delete permanently?"
        alert.informativeText =
            "This cannot be undone, and it is the one change Sift does not apply before the "
            + "server has accepted it. Every other change can be reversed."
        alert.addButton(withTitle: "Delete Permanently")
        alert.addButton(withTitle: "Cancel")
        alert.alertStyle = .critical
        alert.beginSheetModal(for: host) { [weak self] response in
            guard response == .alertFirstButtonReturn, let self else { return }
            var gesture = SiftGesture()
            let id = "message.permanently-delete"
            let status = SiftText.withBytes(id) { ptr, len in
                sift_invoke_action(UnsafeMutablePointer(app), ptr, len, nil, 0, 1, &gesture)
            }
            if status != Ok { NSSound.beep() }
            for window in self.windows { window.refreshChrome() }
        }
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
        window.onInvoke = { [weak self] id in self?.invoke(id) }
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

    /// The tray's one pause control, which is a toggle rather than a second verb.
    @objc private func pauseSync() {
        invoke(everyAccountPaused() ? "app.resume-sync" : "app.pause-sync")
    }

    /// Whether every account is paused. **Vacuously false with no accounts**: there is nothing
    /// paused, and offering to resume nothing is worse than offering to pause nothing.
    private func everyAccountPaused() -> Bool {
        guard let app else { return false }
        let accounts = Account.all(app: app)
        return !accounts.isEmpty && accounts.allSatisfy { $0.condition == Annunciator.pausedByUser }
    }

    /// Pause or resume every account.
    ///
    /// **It was invoked across the boundary and did nothing.** `app.pause-sync` is in D-98's
    /// register with no intent behind it, so the tray item and the menu entry both reported
    /// success and changed nothing — and there was no account-scoped storage for the flag to
    /// live in either.
    private func setPaused(_ paused: Bool) {
        guard let app else { return }
        let value = paused ? "true" : "false"
        let key = "sync.paused"
        for account in Account.all(app: app) {
            _ = SiftText.withBytes(account.name) { namePtr, nameLen in
                SiftText.withBytes(key) { keyPtr, keyLen in
                    SiftText.withBytes(value) { valuePtr, valueLen in
                        sift_set_account_setting(
                            UnsafeMutablePointer(app), namePtr, nameLen, keyPtr, keyLen,
                            valuePtr, valueLen)
                    }
                }
            }
        }
        // The badge is what says it took. D-49 draws the worst condition across every account,
        // and a pause the user asked for is one of the eight.
        for window in windows { window.refreshAccounts() }
        refreshTrayState()
    }

    /// The menu-bar item, saying what is true of the accounts behind it.
    ///
    /// **The always-on surface has to be able to say something.** FR-22 makes this the surface
    /// that is there when no window is, and D-49 requires the worst condition reach the user —
    /// so an envelope that looks identical whether five accounts are healthy or one needs
    /// signing in is a badge that has already failed.
    fileprivate func refreshStatusItem() {
        guard let app, let button = statusItem?.button else { return }
        var out = SiftAnnunciator()
        guard sift_annunciator(UnsafeMutablePointer(app), &out) == Ok else { return }
        let state = Annunciator.State(
            condition: out.condition,
            accounts: out.accounts,
            asksSomething: out.asks_something_of_the_user != 0,
            reachesTheUserWithoutAWindow: out.reaches_the_user_without_a_window != 0)
        // `healthy` has no appearance, because it draws nothing — which here means the
        // envelope Sift has always had.
        guard let look = Annunciator.appearance(state) else {
            button.image = NSImage(systemSymbolName: "envelope", accessibilityDescription: "Sift")
            button.contentTintColor = nil
            button.toolTip = "Sift"
            return
        }
        let words = Annunciator.words(state)
        button.image = NSImage(
            systemSymbolName: look.symbol, accessibilityDescription: words.title)
        // Colour is the second signal rather than the only one: the symbol changes too, so
        // the badge still says something to a person who cannot tell two tints apart.
        button.contentTintColor = look.colour
        button.toolTip = words.title
    }

    /// The tray offers the verb that is not currently true.
    ///
    /// A menu that always says "Pause syncing" beside a paused account is a menu that lies
    /// about the state it is offering to change.
    private func refreshTrayState() {
        let paused = everyAccountPaused()
        let item = statusItem?.menu?.items.first { $0.action == #selector(pauseSync) }
        item?.title = paused ? "Resume syncing" : "Pause syncing"
        refreshStatusItem()
    }
    @objc private func quit() { invoke("app.quit"); NSApp.terminate(nil) }

    /// Whether the re-authentication alert is on screen. Five accounts whose grants expired
    /// together are five announcements and must not be five stacked alerts.
    fileprivate var presentingReauthentication = false
    private var addAccount: AddAccountWindow?
    private var runtimePanel: RuntimePanel?
    private var settingsWindow: SettingsWindow?
    private var standaloneReaders: [StandaloneReader] = []
    private var debugWindow: MessageDebugWindow?

    /// **The account-less state is the add-account flow**, not an empty inbox with a hint in
    /// it. So first run is a screen, and this is where it opens.
    ///
    /// It is also where `Add Account…` arrives, which is the whole difference between a flow a
    /// user can reach once and one they can reach whenever they want another mailbox. D-97
    /// gives the two cases different frames and the same screen: a window of its own when
    /// there is nothing to attach to, a sheet on the window that asked otherwise.
    private func beginAddAccount() {
        guard let app else { return }
        if let existing = addAccount {
            existing.raise()
            return
        }
        let window = AddAccountWindow(
            app: app,
            onAdded: { [weak self] in
                self?.openMainWindow()
                // The sidebar is where the account has to appear, and it is asked rather
                // than told: the list it draws is the layer's.
                self?.windows.forEach { $0.refreshAccounts() }
            },
            onDismissed: { [weak self] in
                self?.addAccount = nil
                // First run with nothing added leaves no window, and an accessory with no
                // window is one the dock does not show. Put the policy back where the window
                // count says it should be rather than leaving Sift stranded as regular.
                self?.syncActivationPolicy()
            })
        addAccount = window
        // **A main window, or none.** D-97 puts this on the window that started it, and the
        // window that started it is a main window or the tray — `keyWindow` may be Settings or
        // the runtime panel, which are their own window kinds and are not what a new account
        // belongs to. Nil is the first-run frame, and it is also the honest answer when the
        // gesture came from the menu bar with every window closed.
        let host = windows.first(where: { $0.hostWindow === NSApp.keyWindow })?.hostWindow
            ?? windows.last?.hostWindow
        NSApp.setActivationPolicy(.regular)
        window.present(over: host)
        NSApp.activate(ignoringOtherApps: true)
    }

    /// **Asked, not remembered.** This was a counter starting at zero on every launch, so an
    /// account added in a previous run was invisible to the next one: the container had the
    /// mail and the queue, and the first-run screen was drawn over them anyway. The layer knows
    /// how many accounts the container holds, because it is what opened it.
    private func hasAnyAccount() -> Bool {
        guard let app else { return false }
        return sift_account_count(UnsafeMutablePointer(app)) > 0
    }

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
    }

    /// The OAuth client this bundle was built with, or the empty string where there is none.
    ///
    /// A build with none is a legitimate state: it runs against the recorded corpus and says
    /// so. The key is written by the build from `shells/macos/oauth-client.txt`.
    static var configuredClientID: String {
        Bundle.main.object(forInfoDictionaryKey: "SiftOAuthClientID") as? String ?? ""
    }

    /// Every URI scheme this bundle's `CFBundleURLTypes` claims.
    ///
    /// **The fact the layer is given.** Read from the running bundle rather than from what the
    /// build intended, because the two disagreeing is exactly the failure this replaced — a
    /// bundle whose Info.plist key expanded to nothing is a bundle with the key *removed*, and
    /// nothing said so until a browser did.
    static func claimedURLSchemes() -> [String] {
        let types = Bundle.main.object(forInfoDictionaryKey: "CFBundleURLTypes") as? [[String: Any]]
        return (types ?? [])
            .compactMap { $0["CFBundleURLSchemes"] as? [String] }
            .flatMap { $0 }
            .filter { !$0.isEmpty }
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
    /// FR-2 — **the one condition that must reach the user with no window open.**
    ///
    /// So it raises Sift to a regular application and activates before it says anything, the
    /// same way a startup failure does: an accessory has no dock icon and no switcher entry,
    /// and a modal it runs can sit behind whatever is frontmost with no ordinary way to reach
    /// it. A prompt the user cannot find is the failure this is answering, not a smaller
    /// version of it.
    ///
    /// One at a time. The layer announces a change per account, and five accounts whose grants
    /// expired together would otherwise be five stacked alerts — which is NFR-34's cascade
    /// wearing a different coat.
    func raiseReauthenticationPrompt() {
        guard !presentingReauthentication else { return }
        presentingReauthentication = true
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)

        let alert = NSAlert()
        // D-56: the layer returns identified states and this shell supplies every word.
        alert.messageText = "Sign in again"
        alert.informativeText =
            "Sift can no longer reach one of your accounts. It keeps everything it has already "
            + "downloaded, and nothing has been changed or lost — signing in again is what lets "
            + "it start syncing that account once more."
        alert.addButton(withTitle: "Sign In…")
        alert.addButton(withTitle: "Later")
        let response = alert.runModal()
        presentingReauthentication = false
        // Adding the account again is the sign-in: D-89 makes re-adding a new account rather
        // than a repair, and the alternative — a flow that re-attaches a grant to an existing
        // identity — is a second authorization path with its own failure modes.
        if response == .alertFirstButtonReturn { beginAddAccount() }
        syncActivationPolicy()
    }

    /// FR-26 — the bundle was replaced underneath the running process.
    ///
    /// Sift implements no self-update; a platform channel replaced it, and continuing against
    /// resources that no longer match the executable is what this prevents.
    ///
    /// **Nothing raises it yet**, and that is stated rather than implied: detecting the
    /// replacement needs something watching the bundle path, the core is not given one, and a
    /// handler with no trigger is a handler whose absence would otherwise look like a working
    /// feature.
    func raiseRestartPrompt() {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = "Sift was updated and needs to restart"
        alert.informativeText =
            "The application was replaced while it was running. Quitting now loses nothing: "
            + "every change you have made is recorded on disk and is applied when Sift starts "
            + "again."
        alert.addButton(withTitle: "Quit Sift")
        alert.addButton(withTitle: "Later")
        if alert.runModal() == .alertFirstButtonReturn { quit() }
        syncActivationPolicy()
    }

    /// D-49's badge, redrawn where it is drawn.
    ///
    /// Every window has one, and the menu-bar item is the surface that is still there when
    /// none of them is — which is the whole reason the condition arrives as a callback rather
    /// than as something a window polls.
    func refreshAnnunciator() {
        for window in windows { window.refreshChrome() }
        refreshStatusItem()
    }
}

