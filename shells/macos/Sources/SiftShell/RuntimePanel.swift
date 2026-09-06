import AppKit
import CSift

/// FR-34's runtime panel, and the reason it is not a developer toy.
///
/// **This is the surface that makes the read-only posture checkable.** An account Sift is only
/// watching accumulates triage in the queue and sends none of it, and being able to open this,
/// see every intent sitting at `Pending`, and count them is the difference between a user being
/// *told* nothing was sent and being able to see it. That is the whole reason the first run
/// against a real mailbox is something a person can watch rather than something they have to
/// trust.
///
/// It refreshes on demand rather than on a timer. A resident process that polls its own state
/// to draw a panel nobody is looking at is exactly the idle wakeup NFR-11 counts.
final class RuntimePanel: NSWindowController {
    private let app: OpaquePointer
    /// **Chosen, not typed.** This was a free-text field, because nothing across the boundary
    /// said what the accounts were called — so the one surface that exists to show a person
    /// their queue asked them to guess its name first.
    private let accountPicker = NSPopUpButton(frame: .zero, pullsDown: false)
    private var accounts: [Account] = []
    /// The one control that changes what Sift may do to a mailbox.
    ///
    /// **It lives here rather than in Settings, and rather than in the action register.** This
    /// is the surface that shows the queue, and authorizing writes is only meaningful beside
    /// the thing it releases — a person turns it on because they have looked at what is
    /// waiting and are satisfied nothing was sent. D-98's action set has no entry for it and
    /// does not grow one: this is a posture, not a gesture.
    private let writesToggle = NSButton(
        checkboxWithTitle: "Sift may change this mailbox", target: nil, action: nil)
    private let sendButton = NSButton(title: "Send Queued Changes", target: nil, action: nil)
    private let queueTable = NSTextView()
    private let memoryTable = NSTextView()
    private let summary = NSTextField(labelWithString: "")

    init(app: OpaquePointer) {
        self.app = app
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 620),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = "Runtime"
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.contentView = build()
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    private func build() -> NSView {
        accountPicker.target = self
        accountPicker.action = #selector(refresh)

        let refreshButton = NSButton(title: "Refresh", target: self, action: #selector(refresh))
        refreshButton.bezelStyle = .rounded

        writesToggle.target = self
        writesToggle.action = #selector(setWrites)
        sendButton.bezelStyle = .rounded
        sendButton.target = self
        sendButton.action = #selector(send)

        summary.font = .preferredFont(forTextStyle: .callout)
        summary.lineBreakMode = .byWordWrapping
        summary.preferredMaxLayoutWidth = 660

        let controls = NSStackView(views: [accountPicker, refreshButton])
        controls.orientation = .horizontal
        controls.spacing = 10
        accountPicker.widthAnchor.constraint(equalToConstant: 220).isActive = true

        let posture = NSStackView(views: [writesToggle, sendButton])
        posture.orientation = .horizontal
        posture.spacing = 14

        let stack = NSStackView(views: [
            controls,
            summary,
            posture,
            heading("The mutation queue"),
            scrolling(queueTable),
            heading("Live bytes, by subsystem"),
            scrolling(memoryTable),
        ])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 10
        stack.edgeInsets = NSEdgeInsets(top: 16, left: 18, bottom: 16, right: 18)
        stack.translatesAutoresizingMaskIntoConstraints = false

        let container = NSView()
        container.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: container.topAnchor),
            stack.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: container.bottomAnchor),
        ])
        return container
    }

    private func heading(_ text: String) -> NSTextField {
        let label = NSTextField(labelWithString: text)
        label.font = .preferredFont(forTextStyle: .headline)
        return label
    }

    private func scrolling(_ view: NSTextView) -> NSScrollView {
        view.isEditable = false
        view.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        let scroll = NSScrollView()
        scroll.documentView = view
        scroll.hasVerticalScroller = true
        scroll.borderType = .bezelBorder
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.heightAnchor.constraint(greaterThanOrEqualToConstant: 200).isActive = true
        scroll.widthAnchor.constraint(equalToConstant: 660).isActive = true
        return scroll
    }

    func present() {
        refresh()
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    /// Refill the picker from the layer, holding whatever was chosen.
    private func reloadAccounts() {
        let chosen = selected()?.name
        accounts = Account.all(app: app)
        accountPicker.removeAllItems()
        accountPicker.addItems(withTitles: accounts.map(\.name))
        if let chosen, let index = accounts.firstIndex(where: { $0.name == chosen }) {
            accountPicker.selectItem(at: index)
        }
    }

    private func selected() -> Account? {
        let index = accountPicker.indexOfSelectedItem
        guard index >= 0, index < accounts.count else { return nil }
        return accounts[index]
    }

    @objc private func refresh() {
        memoryTable.string = memory()
        reloadAccounts()
        guard let chosen = selected() else {
            queueTable.string = "There are no accounts yet."
            summary.stringValue = ""
            writesToggle.isEnabled = false
            sendButton.isEnabled = false
            return
        }
        writesToggle.isEnabled = true
        writesToggle.state = chosen.writesEnabled ? .on : .off
        // Nothing to send is not a fault, and neither is an account that may not send. Both
        // are states the user can see and change, so the control says which by being off
        // rather than by an alert appearing when it is pressed.
        sendButton.isEnabled = chosen.writesEnabled
        let account = chosen.name
        var rows = SiftRows_SiftQueued()
        let ok = SiftText.withBytes(account) { ptr, len in
            sift_queue(UnsafeMutablePointer(app), ptr, len, &rows) == Ok
        }
        guard ok, let ptr = rows.ptr else {
            queueTable.string = "No account named \"\(account)\"."
            summary.stringValue = ""
            return
        }
        guard rows.len > 0 else {
            queueTable.string = "The queue is empty."
            summary.stringValue = "Nothing is waiting to be sent."
            return
        }

        var lines: [String] = ["message                           intent            state       seq  tries"]
        var pending = 0
        for index in 0..<rows.len {
            let row = ptr[index]
            let state = SiftText.string(row.state)
            if state == "Pending" { pending += 1 }
            lines.append(
                String(
                    format: "%@  %-16@  %-10@  %3d  %5d",
                    row.message.key, SiftText.string(row.intent) as NSString,
                    state as NSString, row.sequence, row.attempts))
        }
        queueTable.string = lines.joined(separator: "\n")

        // The sentence a person opens this window to read. `Pending` means durably recorded
        // and **not sent** — an intent that has left carries `Issued`, and one whose answer
        // never came back carries `Reconciling`.
        summary.stringValue =
            pending == Int(rows.len)
            ? "\(rows.len) change(s) recorded, none sent. Every one is still Pending."
            : "\(rows.len) change(s) recorded, \(Int(rows.len) - pending) of them already sent."
    }

    /// Authorize, or withdraw authorization for, writes to the chosen account.
    @objc private func setWrites(_ sender: NSButton) {
        guard let chosen = selected() else { return }
        let enabled: UInt8 = sender.state == .on ? 1 : 0
        let ok = SiftText.withBytes(chosen.name) { ptr, len in
            sift_set_writes_enabled(UnsafeMutablePointer(app), ptr, len, enabled) == Ok
        }
        // A refusal is redrawn rather than reported: the layer holds the posture, and putting
        // the control back where the layer says it is says so more clearly than an alert.
        if !ok { NSSound.beep() }
        refresh()
    }

    /// Send what is queued for the chosen account, once.
    ///
    /// **This blocks while it runs**, which is stated rather than hidden: the work belongs on
    /// a worker under D-19 and the boundary does not carry one yet.
    @objc private func send() {
        guard let chosen = selected() else { return }
        var flushed = SiftFlush()
        let ok = SiftText.withBytes(chosen.name) { ptr, len in
            sift_flush_account(UnsafeMutablePointer(app), ptr, len, &flushed) == Ok
        }
        guard ok else {
            summary.stringValue = "Sift could not send anything for this account."
            return
        }
        refresh()
        // D-56: the layer returns counts and identified states, and every word here is this
        // shell's. Said after the refresh so it is not overwritten by it.
        summary.stringValue = sentence(flushed)
    }

    /// What a flush did, in a sentence a person reads rather than a row of counters.
    private func sentence(_ flushed: SiftFlush) -> String {
        guard flushed.authorized != 0 else {
            return "Nothing was sent — this mailbox is only being watched. "
                + "\(flushed.held) change(s) are held, and they stay held until you say so."
        }
        var text = "\(flushed.issued) sent: \(flushed.applied) applied"
        if flushed.refused > 0 { text += ", \(flushed.refused) refused by the provider" }
        if flushed.deferred > 0 { text += ", \(flushed.deferred) to try again" }
        if flushed.reconciling > 0 {
            text += ", \(flushed.reconciling) sent with no answer back yet"
        }
        if flushed.quarantined > 0 {
            text += ", \(flushed.quarantined) held for a capability this build does not have"
        }
        text += ". \(flushed.queued) still queued."
        if flushed.failed != 0 {
            text += " The last attempt did not finish; what is still queued is safe."
        }
        return text
    }

    private func memory() -> String {
        var rows = SiftRows_SiftSubsystemBytes()
        guard sift_memory(UnsafeMutablePointer(app), &rows) == Ok, let ptr = rows.ptr else {
            return "unavailable"
        }
        var lines: [String] = []
        for index in 0..<rows.len {
            let row = ptr[index]
            lines.append(
                String(
                    format: "%-16@ %12lld",
                    SiftText.string(row.name) as NSString, row.live_bytes))
        }
        return lines.joined(separator: "\n")
    }
}
