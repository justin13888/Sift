import AppKit
import CSift

/// The menu bar, built from D-98's register rather than beside it.
///
/// **Enablement hides rather than disables.** D-98 is explicit: an unavailable action is
/// absent from the palette, not shown greyed. The same rule holds here, so `menuNeedsUpdate`
/// rebuilds each menu from what the layer currently says is available. The cost is one the
/// specification records in its own words — "one keystroke does nothing in one account with no
/// visible reason" — and it is taken deliberately: a greyed item advertises a capability the
/// user cannot have and invites them to work out why.
///
/// **The platform's standard commands sit beside the register, not in it (D-118).** Cut, Copy,
/// Paste and Select All, Hide, Services, Minimize, Zoom, Bring All to Front, the window list,
/// Help and About are AppKit's, not Sift's: they carry no register identifier, are left
/// untargeted so they travel the responder chain to whatever has focus, and grey out by
/// AppKit's own validation rather than hiding — whether Copy applies is a question only the
/// focused text field or web view can answer. Without them ⌘C and ⌘V reach no text field at
/// all, because AppKit delivers those keystrokes only through a main-menu item.
final class MenuBar: NSObject, NSMenuDelegate {
    private let app: OpaquePointer
    private var onInvoke: (String) -> Void

    /// Marks every item this class put in a menu, so a rebuild removes exactly those and
    /// leaves AppKit's own — the open-window list it appends to `NSApp.windowsMenu` — alone.
    private static let owned = 0x5_1F7

    /// Where the platform's items go in one register menu. `nil` is a separator.
    private struct Placement {
        var head: [NSMenuItem?] = []
        /// Inserted immediately before the register item with this identifier.
        var before: [String: [NSMenuItem?]] = [:]
        var tail: [NSMenuItem?] = []
    }

    /// Built once: an `NSMenuItem` is reinserted rather than recreated, because the Services
    /// submenu may have only one parent and AppKit keeps its own state on it.
    private var placements: [String: Placement] = [:]
    private let servicesMenu = NSMenu(title: "Services")

    init(app: OpaquePointer, onInvoke: @escaping (String) -> Void) {
        self.app = app
        self.onInvoke = onInvoke
        super.init()
        placements = makePlacements()
    }

    /// Install the bar, and report anything the two sides disagree about.
    ///
    /// The platform items are invisible to the reconciliation by construction: it compares
    /// the layer's identifiers with `ActionRegister`'s, and they are in neither.
    func install() -> (unbound: [String], unknown: [String]) {
        #if DEBUG
        let collisions = bindingCollisions()
        assert(collisions.isEmpty, "Platform menu items take register bindings: \(collisions)")
        #endif
        let bar = NSMenu()
        for menu in ActionRegister.menus {
            let item = NSMenuItem()
            let submenu = NSMenu(title: menu.title)
            submenu.delegate = self
            // On, so the platform's untargeted items validate against the responder chain.
            // The register's own are targeted at this object, which does not validate, so they
            // stay enabled; an unavailable one is still removed rather than disabled.
            submenu.autoenablesItems = true
            item.submenu = submenu
            bar.addItem(item)
            fill(submenu, from: menu)
            if menu.title == "Window" { NSApp.windowsMenu = submenu }
        }
        // Registering it is what makes the system add its menu search. Its one item of Sift's
        // own reveals NFR-55's log and any D-114 report, so a QA pass can collect both without a
        // terminal; it is a platform item beside the register, like About, not a gesture.
        let help = NSMenu(title: "Help")
        let diagnostics = NSMenuItem(
            title: "Show Diagnostics in Finder", action: #selector(showDiagnostics(_:)),
            keyEquivalent: "")
        diagnostics.target = self
        help.addItem(diagnostics)
        let helpItem = NSMenuItem()
        helpItem.submenu = help
        bar.addItem(helpItem)
        NSApp.helpMenu = help
        NSApp.servicesMenu = servicesMenu
        NSApp.mainMenu = bar
        return ActionRegister.reconcile()
    }

    private func fill(_ menu: NSMenu, from source: ActionRegister.Menu) {
        for item in menu.items where item.tag == Self.owned {
            menu.removeItem(item)
        }
        let placement = placements[source.title] ?? Placement()
        var wanted: [NSMenuItem?] = placement.head
        for entry in source.items {
            guard let entry else {
                wanted.append(nil)
                continue
            }
            wanted.append(contentsOf: placement.before[entry.id] ?? [])
            guard available(entry.id) else { continue }
            let item = NSMenuItem(
                title: entry.title, action: #selector(fire(_:)), keyEquivalent: entry.key)
            item.keyEquivalentModifierMask = entry.modifiers
            item.target = self
            item.representedObject = entry.id
            wanted.append(item)
        }
        wanted.append(contentsOf: placement.tail)

        // A separator between two hidden items is a separator with nothing on either side of
        // it, which reads as a gap rather than a division; so is one at either end.
        var built: [NSMenuItem] = []
        var pendingSeparator = false
        for entry in wanted {
            guard let entry else {
                pendingSeparator = !built.isEmpty
                continue
            }
            if pendingSeparator { built.append(.separator()) }
            pendingSeparator = false
            built.append(entry)
        }
        // Ahead of anything AppKit appended, so the window list stays at the bottom.
        for (index, item) in built.enumerated() {
            item.tag = Self.owned
            menu.insertItem(item, at: index)
        }
    }

    /// The platform's standard items, per register menu, in the places macOS users look.
    private func makePlacements() -> [String: Placement] {
        func item(
            _ title: String, _ action: Selector, _ key: String = "",
            _ modifiers: NSEvent.ModifierFlags = .command
        ) -> NSMenuItem {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: key)
            item.keyEquivalentModifierMask = modifiers
            return item
        }

        let about = item("About Sift", #selector(showAbout(_:)))
        about.target = self
        let services = NSMenuItem(title: "Services", action: nil, keyEquivalent: "")
        services.submenu = servicesMenu

        return [
            "Sift": Placement(
                head: [about, nil],
                // Quit stays last: FR-25 fixes its wording, and macOS users find it there.
                before: ["app.quit": [
                    nil,
                    services,
                    nil,
                    item("Hide Sift", #selector(NSApplication.hide(_:)), "h"),
                    item("Hide Others", #selector(NSApplication.hideOtherApplications(_:)), "h",
                         [.option, .command]),
                    item("Show All", #selector(NSApplication.unhideAllApplications(_:))),
                    nil,
                ]]),
            "Edit": Placement(
                before: ["search.begin": [
                    item("Cut", #selector(NSText.cut(_:)), "x"),
                    item("Copy", #selector(NSText.copy(_:)), "c"),
                    item("Paste", #selector(NSText.paste(_:)), "v"),
                    item("Select All", #selector(NSText.selectAll(_:)), "a"),
                    nil,
                ]]),
            "Window": Placement(
                head: [
                    item("Minimize", #selector(NSWindow.performMiniaturize(_:)), "m"),
                    item("Zoom", #selector(NSWindow.performZoom(_:)), "", []),
                    nil,
                ],
                tail: [
                    nil,
                    item("Bring All to Front", #selector(NSApplication.arrangeInFront(_:)), "", []),
                ]),
        ]
    }

    /// The standard panel, with the licence and — once the build records it (D-62) — the
    /// source revision a bug report needs. Version and build come from the bundle itself.
    @objc private func showAbout(_ sender: Any?) {
        var options: [NSApplication.AboutPanelOptionKey: Any] = [:]
        let info = Bundle.main.infoDictionary ?? [:]
        if let revision = info["SiftSourceRevision"] as? String,
            !revision.isEmpty, !revision.hasPrefix("$(")
        {
            let build = info["CFBundleVersion"] as? String ?? ""
            options[.version] = build.isEmpty ? revision : "\(build), \(revision)"
        }
        options[.credits] = NSAttributedString(
            string: "Free software under the GNU Affero General Public License, version 3 only "
                + "(AGPL-3.0-only).",
            attributes: [
                .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize),
                .foregroundColor: NSColor.labelColor,
            ])
        NSApp.orderFrontStandardAboutPanel(options: options)
    }

    @objc private func showDiagnostics(_ sender: Any?) {
        Diagnostics.reveal()
    }

    #if DEBUG
    /// Platform key equivalents that a register item also binds. D-118 forbids any.
    private func bindingCollisions() -> [String] {
        let mask: NSEvent.ModifierFlags = [.command, .shift, .option, .control]
        func chord(_ key: String, _ modifiers: NSEvent.ModifierFlags) -> String {
            "\(modifiers.intersection(mask).rawValue):\(key.lowercased())"
        }
        var register: [String: String] = [:]
        for menu in ActionRegister.menus {
            for case let entry? in menu.items where !entry.key.isEmpty {
                register[chord(entry.key, entry.modifiers)] = entry.id
            }
        }
        var out: [String] = []
        for placement in placements.values {
            let items = placement.head + Array(placement.before.values.joined()) + placement.tail
            for case let item? in items where !item.keyEquivalent.isEmpty {
                let key = chord(item.keyEquivalent, item.keyEquivalentModifierMask)
                if let id = register[key] { out.append("\(item.title) / \(id)") }
            }
        }
        return out
    }
    #endif

    private func available(_ id: String) -> Bool {
        var flag: UInt8 = 0
        let ok = SiftText.withBytes(id) { ptr, len in
            sift_action_available(UnsafeMutablePointer(app), ptr, len, &flag) == Ok
        }
        return ok && flag != 0
    }

    func menuNeedsUpdate(_ menu: NSMenu) {
        guard let source = ActionRegister.menus.first(where: { $0.title == menu.title }) else {
            return
        }
        fill(menu, from: source)
    }

    @objc private func fire(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        onInvoke(id)
    }
}
