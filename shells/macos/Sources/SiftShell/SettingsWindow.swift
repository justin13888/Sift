import AppKit
import CSift

/// D-97's settings window, over D-101's enumerated settings.
///
/// **The list comes from the layer, not from this file.** The surface is written twice — once
/// here and once in GTK — and a settings model each shell composes for itself ends up shaped
/// differently in each, which is a capability existing for one shell and not the other by
/// another route. So this renders what `sift_settings` returns, in the order it returns it.
///
/// # Organised by scope, because that is what decides behaviour
///
/// An account setting disappears when the account is removed and an installation setting does
/// not. Grouping by topic would put the cache budget beside the per-sender allowlist, which
/// look related and have opposite lifetimes.
///
/// # Two rows are not preferences
///
/// The per-sender allowlists are records of decisions the user made in context. They are shown
/// as records and are not editable here: bulk-editing a security-relevant list in a screen away
/// from any message is the thing this surface deliberately does not offer.
final class SettingsWindow: NSWindowController {
    private let app: OpaquePointer
    private let stack = NSStackView()
    /// What each control edits. An account row needs two facts to write — which account and
    /// which key — and an identifier is one string; encoding both into it would make the
    /// separator part of the format.
    private var edits: [ObjectIdentifier: (account: String?, key: String)] = [:]

    init(app: OpaquePointer) {
        self.app = app
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 680, height: 520),
            styleMask: [.titled, .closable, .resizable],
            backing: .buffered, defer: false)
        window.title = "Settings"
        window.isReleasedWhenClosed = false
        super.init(window: window)

        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 14
        stack.edgeInsets = NSEdgeInsets(top: 20, left: 24, bottom: 20, right: 24)
        stack.translatesAutoresizingMaskIntoConstraints = false

        let scroll = NSScrollView()
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false

        // **A scroll view does not lay out what it scrolls.** A document view left translating
        // its autoresizing mask keeps the zero frame it was created with, so a stack pinned to
        // its four edges is pinned to nothing, and every row's own width constraint then
        // contradicts a container that is zero wide. That is what the empty settings window
        // was: nineteen rows, all present, laid out into 0x0 and reported by nobody.
        //
        // Every other scroll view in this shell holds a table or a text view, which size
        // themselves. This one holds a plain view, so it is sized here — pinned to the clip
        // view's width, with its height coming from the stack.
        //
        // **Flipped**, because a scroll view's origin is the bottom-left of an ordinary view.
        // With the default geometry the first row lands at the bottom of the document and the
        // window opens showing the end of the list — which reads as a different bug from the
        // one above and has the same fix nowhere near it.
        let document = FlippedView()
        document.translatesAutoresizingMaskIntoConstraints = false
        document.addSubview(stack)
        scroll.documentView = document
        NSLayoutConstraint.activate([
            document.topAnchor.constraint(equalTo: scroll.contentView.topAnchor),
            document.leadingAnchor.constraint(equalTo: scroll.contentView.leadingAnchor),
            document.trailingAnchor.constraint(equalTo: scroll.contentView.trailingAnchor),
            document.widthAnchor.constraint(equalTo: scroll.contentView.widthAnchor),
            stack.topAnchor.constraint(equalTo: document.topAnchor),
            stack.leadingAnchor.constraint(equalTo: document.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: document.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: document.bottomAnchor),
        ])
        window.contentView = scroll
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    func present() {
        reload()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    private func reload() {
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }
        var rows = SiftRows_SiftSetting()
        guard sift_settings(UnsafeMutablePointer(app), &rows) == Ok, let ptr = rows.ptr else {
            stack.addArrangedSubview(
                NSTextField(labelWithString: "Settings are unavailable."))
            return
        }

        edits.removeAll()
        let all = (0..<rows.len).map { ptr[$0] }

        stack.addArrangedSubview(heading("This installation"))
        stack.addArrangedSubview(note("These stay when you remove an account."))
        for row in all where row.account_scoped == 0 {
            add(view(for: row, account: nil))
        }

        // **One group per account, rather than one group for the idea of an account.** These
        // go with the account when it is removed, so a single "Each account" list would be
        // showing one account's values under a heading that claims to speak for all of them —
        // and there is nothing it could write them to.
        let accountRows = all.filter { $0.account_scoped != 0 }
        guard !accountRows.isEmpty else { return }
        for account in Account.all(app: app) {
            stack.addArrangedSubview(heading(account.name))
            stack.addArrangedSubview(
                note("These go with this account when you remove it."))
            for row in accountRows {
                add(view(for: row, account: account.name))
            }
        }
    }

    /// Add a row and give it the stack's width.
    ///
    /// **The width is constrained here rather than where the row is built**, because an anchor
    /// pair needs a common ancestor and a row that has not been added yet has none — activating
    /// it early raises, AppKit swallows the exception at the top of the event loop, and what is
    /// left on screen is however much of the list had been added before it. Which looks exactly
    /// like a settings window that has no settings in it.
    private func add(_ row: NSView) {
        stack.addArrangedSubview(row)
        row.widthAnchor.constraint(
            equalTo: stack.widthAnchor,
            constant: -(stack.edgeInsets.left + stack.edgeInsets.right)
        ).isActive = true
    }

    private func heading(_ text: String) -> NSView {
        let label = NSTextField(labelWithString: text)
        label.font = .preferredFont(forTextStyle: .headline)
        return label
    }

    private func note(_ text: String) -> NSView {
        let label = NSTextField(labelWithString: text)
        label.font = .preferredFont(forTextStyle: .caption1)
        label.textColor = .secondaryLabelColor
        return label
    }

    private func view(for row: SiftSetting, account: String?) -> NSView {
        let key = SiftText.string(row.key)
        let owner = SiftText.string(row.owner)
        // `sift_settings` reports the installation table. An account row's value is that
        // account's, so it is read per account rather than repeated from one of them.
        let value = account.map { held(key, of: $0) } ?? SiftText.string(row.value)

        let title = NSTextField(labelWithString: SettingsWindow.words(key))
        title.font = .preferredFont(forTextStyle: .body)

        let provenance = NSTextField(labelWithString: owner)
        provenance.font = .preferredFont(forTextStyle: .caption1)
        provenance.textColor = .tertiaryLabelColor

        let control: NSView
        if row.security_state != 0 {
            // Shown as a record, and revoked where the decision was made. Not edited here.
            let count = value.isEmpty ? 0 : value.split(separator: ",").count
            let label = NSTextField(
                labelWithString: count == 0 ? "nothing recorded" : "\(count) recorded")
            label.textColor = .secondaryLabelColor
            control = label
        } else if row.kind == UInt32(SIFT_SETTING_FLAG) {
            let toggle = NSButton(
                checkboxWithTitle: "", target: self, action: #selector(toggled(_:)))
            toggle.state = (value == "true") ? .on : .off
            edits[ObjectIdentifier(toggle)] = (account, key)
            control = toggle
        } else {
            let field = NSTextField(string: value)
            field.target = self
            field.action = #selector(edited(_:))
            field.widthAnchor.constraint(equalToConstant: 180).isActive = true
            edits[ObjectIdentifier(field)] = (account, key)
            control = field
        }

        let labels = NSStackView(views: [title, provenance])
        labels.orientation = .vertical
        labels.alignment = .leading
        labels.spacing = 0

        let row = NSStackView(views: [labels, NSView(), control])
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 14
        return row
    }

    /// D-56: the layer returns identified keys and **this shell supplies every word**.
    private static func words(_ key: String) -> String {
        switch key {
        case "cache.budget-bytes": return "Keep at most this many bytes of mail on disk"
        case "cache.budget-days": return "Keep mail for at most this many days"
        case "index.envelope-budget": return "Keep at most this many message summaries"
        case "network.single-fetch-ceiling-bytes": return "Ask before downloading more than"
        case "blocking.standard-lists": return "Use the standard blocking lists"
        case "blocking.email-list": return "Use the bundled mail blocking list"
        case "render.dark-transform": return "Adapt messages to dark mode"
        case "read.mark-read-dwell-millis": return "Mark read after (milliseconds, 0 for never)"
        case "network.data-cap-bytes": return "Data limit (0 for none)"
        case "network.accounting-window-days": return "Data limit period, in days"
        case "notify.quiet-mode": return "Quiet mode"
        case "notify.new-mail-in-inbox": return "Notify about new mail in the inbox"
        case "debug.message-view": return "Show the message details window"
        case "debug.runtime-panel": return "Show the runtime window"
        case "sync.watched-folders": return "Folders to keep up to date"
        case "notify.per-folder-rules": return "Notification rules, by folder"
        case "sync.paused": return "Paused"
        case "render.sender-allowlist": return "Senders whose images you chose to load"
        case "render.sender-dark-choices": return "Senders whose own styling you chose to keep"
        default: return key
        }
    }

    @objc private func toggled(_ sender: NSButton) {
        guard let edit = edits[ObjectIdentifier(sender)] else { return }
        write(edit, sender.state == .on ? "true" : "false")
    }

    @objc private func edited(_ sender: NSTextField) {
        guard let edit = edits[ObjectIdentifier(sender)] else { return }
        write(edit, sender.stringValue)
    }

    /// What one account holds for a key, or the shipped default.
    private func held(_ key: String, of account: String) -> String {
        var out = SiftStr()
        let ok = SiftText.withBytes(account) { namePtr, nameLen in
            SiftText.withBytes(key) { keyPtr, keyLen in
                sift_account_setting(
                    UnsafeMutablePointer(app), namePtr, nameLen, keyPtr, keyLen, &out) == Ok
            }
        }
        return ok ? SiftText.string(out) : ""
    }

    /// **Two entry points, because the scope split is the storage split.** An account setting
    /// goes with the account when it is removed and an installation setting does not, so which
    /// one a key belongs to is a fact the row carries rather than one the layer infers.
    private func write(_ edit: (account: String?, key: String), _ value: String) {
        let ok = SiftText.withBytes(edit.key) { keyPtr, keyLen in
            SiftText.withBytes(value) { valuePtr, valueLen in
                guard let account = edit.account else {
                    return sift_set_setting(
                        UnsafeMutablePointer(app), keyPtr, keyLen, valuePtr, valueLen) == Ok
                }
                return SiftText.withBytes(account) { namePtr, nameLen in
                    sift_set_account_setting(
                        UnsafeMutablePointer(app), namePtr, nameLen, keyPtr, keyLen,
                        valuePtr, valueLen) == Ok
                }
            }
        }
        // A refusal is redrawn rather than reported: the layer refuses a value the setting
        // cannot hold, and putting the old one back says so more clearly than an alert.
        if !ok { reload() }
    }
}

/// A view whose origin is its top-left.
///
/// AppKit's is the bottom-left, which is the right answer for a canvas and the wrong one for a
/// list inside a scroll view: the content is laid out upwards from the bottom and the scroll
/// opens at the end of it.
final class FlippedView: NSView {
    override var isFlipped: Bool { true }
}
