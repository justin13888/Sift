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
    private let accountField = NSTextField(string: "")
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
        accountField.placeholderString = "account"
        accountField.target = self
        accountField.action = #selector(refresh)

        let refreshButton = NSButton(title: "Refresh", target: self, action: #selector(refresh))
        refreshButton.bezelStyle = .rounded

        summary.font = .preferredFont(forTextStyle: .callout)
        summary.lineBreakMode = .byWordWrapping
        summary.preferredMaxLayoutWidth = 660

        let controls = NSStackView(views: [accountField, refreshButton])
        controls.orientation = .horizontal
        controls.spacing = 10
        accountField.widthAnchor.constraint(equalToConstant: 220).isActive = true

        let stack = NSStackView(views: [
            controls,
            summary,
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

    @objc private func refresh() {
        memoryTable.string = memory()
        let account = accountField.stringValue.trimmingCharacters(in: .whitespaces)
        guard !account.isEmpty else {
            queueTable.string = "Type an account name above to see what it holds."
            summary.stringValue = ""
            return
        }
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
