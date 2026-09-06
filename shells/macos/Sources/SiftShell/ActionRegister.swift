import AppKit
import CSift

/// D-98's register, as macOS sees it: a title, a key equivalent, and where the item goes.
///
/// **The register is the layer's; the words and the keys are the shell's.** D-56 and D-68 put
/// all user-facing wording on this side of the boundary, and the key equivalents follow the
/// macOS Mail idiom rather than anything the specification states — the specification
/// deliberately states none.
///
/// The identifiers here are checked against the layer at startup, so an action added below the
/// boundary and forgotten here is a failure at launch rather than an item nobody notices is
/// missing.
enum ActionRegister {
    struct Item {
        let id: String
        let title: String
        let key: String
        let modifiers: NSEvent.ModifierFlags

        init(_ id: String, _ title: String, _ key: String = "", _ modifiers: NSEvent.ModifierFlags = []) {
            self.id = id
            self.title = title
            self.key = key
            self.modifiers = modifiers
        }
    }

    /// One menu, and what goes in it. `nil` is a separator.
    struct Menu {
        let title: String
        let items: [Item?]
    }

    static let command: NSEvent.ModifierFlags = [.command]
    static let shiftCommand: NSEvent.ModifierFlags = [.shift, .command]
    static let controlCommand: NSEvent.ModifierFlags = [.control, .command]
    static let optionCommand: NSEvent.ModifierFlags = [.option, .command]
    static let controlShiftCommand: NSEvent.ModifierFlags = [.control, .shift, .command]
    static let optionShiftCommand: NSEvent.ModifierFlags = [.option, .shift, .command]

    /// The menu bar.
    ///
    /// **"Close Window" and "Quit Sift Entirely" are worded to be told apart.** FR-25 calls
    /// the close-versus-quit distinction the single most likely source of user distrust in the
    /// whole design: an always-on client whose window closes looks like it stopped, and one
    /// that keeps running when the user meant to stop it is worse. Neither item is called
    /// anything ambiguous, in either place they appear.
    static let menus: [Menu] = [
        Menu(
            title: "Sift",
            items: [
                Item("app.add-account", "Add Account…", "n", controlCommand),
                Item("app.open-settings", "Settings…", ",", command),
                nil,
                Item("app.pause-sync", "Pause Syncing", "p", controlCommand),
                Item("app.resume-sync", "Resume Syncing"),
                nil,
                Item("app.open-runtime-panel", "Runtime…", "r", optionCommand),
                Item("app.open-message-debug-view", "Message Details…", "i", optionCommand),
                nil,
                Item("app.quit", "Quit Sift Entirely", "q", command),
            ]),
        Menu(
            title: "Message",
            items: [
                Item("message.archive", "Archive", "a", controlCommand),
                Item("message.delete-to-trash", "Move to Trash", "\u{8}", []),
                Item("message.permanently-delete", "Delete Permanently…", "\u{8}", shiftCommand),
                Item("message.move-to-folder", "Move to Folder…", "m", shiftCommand),
                nil,
                Item("message.flag", "Flag", "l", shiftCommand),
                Item("message.mark-read", "Mark as Read", "r", controlCommand),
                Item("message.mark-unread", "Mark as Unread", "u", shiftCommand),
                nil,
                Item("message.add-tag", "Add Tag…", "t", controlCommand),
                Item("message.remove-tag", "Remove Tag…", "t", controlShiftCommand),
                nil,
                Item("message.report-junk", "Move to Junk", "j", shiftCommand),
                Item("message.report-not-junk", "Not Junk", "j", optionShiftCommand),
            ]),
        Menu(
            title: "Read",
            items: [
                Item("read.open-message", "Open", "\r", []),
                Item("read.open-in-standalone-reader", "Open in New Window", "\r", command),
                nil,
                Item("read.next-message", "Next Message", "\u{f701}", [.option]),
                Item("read.previous-message", "Previous Message", "\u{f700}", [.option]),
                Item("read.next-unread", "Next Unread", "\u{f701}", shiftCommand),
                Item("read.previous-unread", "Previous Unread", "\u{f700}", shiftCommand),
                nil,
                Item("read.expand-thread", "Expand Thread", "\u{f703}", command),
                Item("read.collapse-thread", "Collapse Thread", "\u{f702}", command),
                nil,
                Item("read.show-plain-text", "Show Plain Text", "p", optionCommand),
                Item("read.show-raw-source", "Show Raw Source", "u", optionCommand),
                Item("read.toggle-dark-transform", "Adapt to Dark Mode", "d", controlCommand),
                nil,
                Item("read.allow-remote-content-for-sender", "Always Load Images From Sender", "i", shiftCommand),
                Item("read.find-in-message", "Find in Message…", "f", command),
                nil,
                Item("read.reply", "Reply…", "r", command),
                Item("read.reply-all", "Reply All…", "r", shiftCommand),
                Item("read.forward", "Forward…", "f", shiftCommand),
            ]),
        Menu(
            title: "Edit",
            items: [
                Item("undo.last-gesture", "Undo", "z", command),
                nil,
                Item("search.begin", "Search…", "f", optionCommand),
                Item("search.clear", "Clear Search"),
                Item("search.narrow-to-account", "Search This Account"),
                Item("search.narrow-to-folder", "Search This Folder"),
            ]),
        Menu(
            title: "Window",
            items: [
                Item("app.new-window", "New Window", "n", command),
                Item("app.close-window", "Close Window", "w", command),
                nil,
                Item("navigate.focus-sidebar", "Go to Sidebar", "1", command),
                Item("navigate.focus-list", "Go to Message List", "2", command),
                Item("navigate.focus-reader", "Go to Reader", "3", command),
                Item("navigate.focus-search", "Go to Search", "0", optionCommand),
                nil,
                Item("navigate.unified-inbox", "All Inboxes", "0", command),
                Item("navigate.next-folder", "Next Folder", "\u{f701}", [.control]),
                Item("navigate.previous-folder", "Previous Folder", "\u{f700}", [.control]),
                Item("navigate.next-account", "Next Account", "\u{f701}", [.control, .shift]),
                Item("navigate.previous-account", "Previous Account", "\u{f700}", [.control, .shift]),
                nil,
                Item("app.command-palette", "Command Palette…", "k", command),
            ]),
    ]

    /// Every identifier this file binds, in menu order.
    static var identifiers: [String] {
        menus.flatMap { $0.items.compactMap { $0?.id } }
    }

    /// Titles by identifier, for the palette — which shows the same words as the menu, because
    /// two names for one action is two actions as far as a user is concerned.
    static let titles: [String: String] = {
        var out: [String: String] = [:]
        for menu in menus {
            for case let item? in menu.items { out[item.id] = item.title }
        }
        return out
    }()

    /// Check this file against the register, at launch.
    ///
    /// D-98 makes the identifiers stable and never reused, so an action that exists below the
    /// boundary and is bound to nothing here is a capability the user cannot reach — and one
    /// bound here that the layer does not have is a menu item that fails when pressed. Both
    /// are reported rather than either being tolerated.
    static func reconcile() -> (unbound: [String], unknown: [String]) {
        var declared: Set<String> = []
        for index in 0..<sift_action_count() {
            var id = SiftStr()
            guard sift_action_id(index, &id) == Ok else { continue }
            declared.insert(SiftText.string(id))
        }
        let bound = Set(identifiers)
        return (unbound: declared.subtracting(bound).sorted(),
                unknown: bound.subtracting(declared).sorted())
    }
}
