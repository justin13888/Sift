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
final class MenuBar: NSObject, NSMenuDelegate {
    private let app: OpaquePointer
    private var onInvoke: (String) -> Void

    init(app: OpaquePointer, onInvoke: @escaping (String) -> Void) {
        self.app = app
        self.onInvoke = onInvoke
        super.init()
    }

    /// Install the bar, and report anything the two sides disagree about.
    func install() -> (unbound: [String], unknown: [String]) {
        let bar = NSMenu()
        for menu in ActionRegister.menus {
            let item = NSMenuItem()
            let submenu = NSMenu(title: menu.title)
            submenu.delegate = self
            submenu.autoenablesItems = false
            item.submenu = submenu
            bar.addItem(item)
            fill(submenu, from: menu)
        }
        NSApp.mainMenu = bar
        return ActionRegister.reconcile()
    }

    private func fill(_ menu: NSMenu, from source: ActionRegister.Menu) {
        menu.removeAllItems()
        var lastWasSeparator = true
        for entry in source.items {
            guard let entry else {
                // A separator between two hidden items is a separator with nothing on either
                // side of it, which reads as a gap rather than a division.
                if !lastWasSeparator {
                    menu.addItem(.separator())
                    lastWasSeparator = true
                }
                continue
            }
            guard available(entry.id) else { continue }
            let item = NSMenuItem(
                title: entry.title, action: #selector(fire(_:)), keyEquivalent: entry.key)
            item.keyEquivalentModifierMask = entry.modifiers
            item.target = self
            item.representedObject = entry.id
            item.isEnabled = true
            menu.addItem(item)
            lastWasSeparator = false
        }
        // A menu whose every item is unavailable is a menu with a trailing separator, or an
        // empty one. Neither is worth showing.
        if let last = menu.items.last, last.isSeparatorItem {
            menu.removeItem(last)
        }
    }

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
