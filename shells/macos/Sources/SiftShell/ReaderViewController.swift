import AppKit
import CSift

/// D-97's reader: the envelope in native chrome, the body in its own hardened document.
///
/// **The chrome is native and the body is not, and that split is the security boundary.** The
/// blocked-content count, the sender, the date and every affordance are drawn here, where a
/// sender cannot reach them. D-3's body view holds only what the message said, in a document
/// with no network capability, no script and no persistent storage.
///
/// A control drawn *inside* the body would be one a sender could counterfeit — which is why
/// the resource broker requires every affordance be native, and why "12 images not loaded" is
/// a label in this file rather than markup in that one.
final class ReaderViewController: NSViewController {
    private let sender = NSTextField(labelWithString: "")
    private let subject = NSTextField(labelWithString: "")
    private let received = NSTextField(labelWithString: "")
    private let failure = NSTextField(labelWithString: "")
    private let unsubscribe = NSButton()
    private let blockedBar = BlockedContentBar(frame: .zero)
    private let attachments = AttachmentBar(frame: .zero)
    private let empty = NSTextField(labelWithString: "No message selected")
    private let body = BodyView(frame: .zero)
    /// This document's links, copied out of the layer while the document is open.
    ///
    /// Copied rather than read at click time because the rows borrow from the document, and
    /// the document can be revoked between the render and the click. A dangling read is not a
    /// risk worth taking to save four strings.
    private var links: [Link] = []

    /// One navigation target, as FR-30 requires it be shown.
    private struct Link {
        /// Punycode-decoded and bidi-stripped **by the layer**. The shell does not have its
        /// own opinion about how a URL reads: one implementation of that rule, below the
        /// boundary, where it is asserted by test.
        let displayed: String
        let target: String
        let wrapper: String?
        let needsAMailHandler: Bool

        /// What the document's markup says, which is the wrapper where there was one.
        var asWritten: String { wrapper ?? target }
    }

    override func loadView() {
        subject.font = .preferredFont(forTextStyle: .title2)
        subject.lineBreakMode = .byTruncatingTail
        sender.font = .preferredFont(forTextStyle: .body)
        sender.textColor = .secondaryLabelColor
        received.font = .preferredFont(forTextStyle: .caption1)
        received.textColor = .secondaryLabelColor
        failure.font = .preferredFont(forTextStyle: .caption1)
        failure.textColor = .secondaryLabelColor

        unsubscribe.bezelStyle = .rounded
        unsubscribe.controlSize = .small
        unsubscribe.target = self
        unsubscribe.action = #selector(tapUnsubscribe)
        unsubscribe.isHidden = true
        empty.font = .preferredFont(forTextStyle: .title3)
        empty.textColor = .tertiaryLabelColor

        let header = NSStackView(views: [subject, sender, received, failure, unsubscribe])
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 6
        header.translatesAutoresizingMaskIntoConstraints = false
        header.edgeInsets = NSEdgeInsets(top: 20, left: 24, bottom: 20, right: 24)

        // The order is the security argument made visible: chrome, then what was withheld,
        // then the body, then what is attached. Everything a sender wrote is in the one band
        // in the middle, and every control is outside it.
        blockedBar.translatesAutoresizingMaskIntoConstraints = false
        attachments.translatesAutoresizingMaskIntoConstraints = false
        body.translatesAutoresizingMaskIntoConstraints = false

        let column = NSStackView(views: [header, blockedBar, body, attachments])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 0
        column.translatesAutoresizingMaskIntoConstraints = false
        column.setHuggingPriority(.defaultLow, for: .vertical)
        // The body takes what is left; the bars take what they need.
        column.setVisibilityPriority(.mustHold, for: body)

        let container = NSView()
        container.addSubview(column)
        empty.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(empty)

        NSLayoutConstraint.activate([
            column.topAnchor.constraint(equalTo: container.topAnchor),
            column.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            column.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            column.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            header.widthAnchor.constraint(equalTo: column.widthAnchor),
            blockedBar.widthAnchor.constraint(equalTo: column.widthAnchor),
            attachments.widthAnchor.constraint(equalTo: column.widthAnchor),
            body.widthAnchor.constraint(equalTo: column.widthAnchor),
            empty.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            empty.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])

        // FR-30: a link is never followed to find out where it goes — following a wrapper is
        // itself the tracking event. The destination shown is the one the document declared.
        body.onLink = { [weak self] url in self?.confirmOpen(url) }
        view = container
        // **Not `show(nil:)`.** `show` reads `view`, and reading `view` is what runs this
        // method — so a `show(nil:)` here runs *after* the `show(row:)` that triggered the
        // load and wipes the message that was just drawn. The reader then says "No message
        // selected" about a row the list has selected. Setting the state directly is the
        // whole fix, and `show` calls `loadViewIfNeeded()` so the ordering cannot recur.
        showNothing()
    }

    /// The empty state, which is a *stated* one rather than a blank pane — a blank pane is
    /// indistinguishable from something that failed to draw.
    private func showNothing() {
        body.clear()
        subject.isHidden = true
        sender.isHidden = true
        received.isHidden = true
        failure.isHidden = true
        unsubscribe.isHidden = true
        blockedBar.isHidden = true
        attachments.isHidden = true
        body.isHidden = true
        empty.isHidden = false
        links = []
    }

    func show(_ row: MessageRow?, app: OpaquePointer?) {
        // Force the load *first*, so it cannot happen part-way through this method and undo
        // it on the way out. `loadViewIfNeeded()` says this more clearly and is macOS 14; the
        // deployment target is 13 under D-46, and reading `view` is the same gesture.
        _ = view
        guard let row else {
            showNothing()
            return
        }
        empty.isHidden = true
        subject.isHidden = false
        sender.isHidden = false
        received.isHidden = false
        body.isHidden = false

        subject.stringValue = row.subject.isEmpty ? "(no subject)" : row.subject
        // NFR-54 requires the address be shown beside the display name rather than instead of
        // it: a display name alone is a sender's own claim about who they are.
        sender.stringValue = row.sender
        received.stringValue = ReaderViewController.absolute(row.receivedMillis)
        view.setAccessibilityLabel("\(row.subject), from \(row.sender)")

        guard let app else {
            failure.isHidden = true
            blockedBar.isHidden = true
            attachments.isHidden = true
            body.clear()
            return
        }
        var document = SiftDocument()
        let status = sift_open_document(UnsafeMutablePointer(app), row.id, 0, &document)
        guard status == Ok else {
            // FR-9: a body that cannot be rendered degrades to a stated absence rather than
            // to a blank pane, because a blank pane is indistinguishable from a bug.
            failure.isHidden = false
            failure.stringValue = "This message could not be displayed."
            blockedBar.isHidden = true
            attachments.isHidden = true
            body.clear()
            links = []
            return
        }
        failure.isHidden = true
        let html = ReaderViewController.string(document.html)
        let token = ReaderViewController.string(document.token)

        // Native chrome, never markup. Every number here comes from the layer, and a number a
        // sender could write would not be a number.
        links = ReaderViewController.links(app: app, token: token)
        blockedBar.show(
            app: app, token: token, blocked: document.blocked,
            mayAlwaysAllow: document.may_always_allow != 0)
        attachments.show(app: app, message: row.id)
        showUnsubscribe(app: app, token: token, declared: document.has_unsubscribe != 0)
        body.show(html: html, token: token, app: app)
    }

    /// FR-42. Shown where the message declares one, and **never requested**.
    ///
    /// A `mailto:` destination says so on the button rather than being omitted: a user who
    /// cannot see that an unsubscribe address exists cannot know it existed. Sift will not
    /// send it either way — the historical form is a message, which is forbidden outright, and
    /// the modern form is a request to an address carrying a per-recipient token, which is
    /// exactly what the blocker treats as evidence of tracking.
    private func showUnsubscribe(app: OpaquePointer, token: String, declared: Bool) {
        guard declared else {
            unsubscribe.isHidden = true
            unsubscribeDestination = nil
            return
        }
        var link = SiftLink()
        let read = SiftText.withBytes(token) { ptr, len in
            sift_document_unsubscribe(UnsafeMutablePointer(app), ptr, len, &link) == Ok
        }
        guard read else {
            unsubscribe.isHidden = true
            unsubscribeDestination = nil
            return
        }
        let destination = Link(
            displayed: SiftText.string(link.displayed),
            target: SiftText.string(link.target),
            wrapper: link.wrapper.ptr == nil ? nil : SiftText.string(link.wrapper),
            needsAMailHandler: link.needs_a_mail_handler != 0)
        unsubscribeDestination = destination
        unsubscribe.isHidden = false
        unsubscribe.title =
            destination.needsAMailHandler
            ? "Unsubscribe… (opens your mail app)" : "Unsubscribe…"
    }

    private var unsubscribeDestination: Link?

    @objc private func tapUnsubscribe() {
        guard let destination = unsubscribeDestination else { return }
        confirm(destination, verb: "Unsubscribe")
    }

    /// This document's links, copied out of the layer.
    private static func links(app: OpaquePointer, token: String) -> [Link] {
        var rows = SiftRows_SiftLink()
        let ok = SiftText.withBytes(token) { ptr, len in
            sift_document_links(UnsafeMutablePointer(app), ptr, len, &rows) == Ok
        }
        guard ok, let ptr = rows.ptr, rows.len > 0 else { return [] }
        return (0..<rows.len).map { index in
            let row = ptr[index]
            return Link(
                displayed: SiftText.string(row.displayed),
                target: SiftText.string(row.target),
                wrapper: row.wrapper.ptr == nil ? nil : SiftText.string(row.wrapper),
                needsAMailHandler: row.needs_a_mail_handler != 0)
        }
    }

    /// The link-confirmation sheet — D-97 puts it on the window that raised it.
    ///
    /// Nothing opens without it, and **the shell has no opinion about how a URL reads**. The
    /// displayed form comes from the layer, where punycode decoding and bidi stripping are one
    /// implementation asserted by test. A second implementation here would be a second answer
    /// to "what does this link say", and the two would disagree on exactly the inputs that
    /// matter.
    private func confirmOpen(_ url: URL) {
        let written = url.absoluteString
        guard let link = links.first(where: { $0.asWritten == written || $0.target == written })
        else {
            // A navigation the layer never listed. The document is sender-controlled and the
            // link table is not, so the honest answer is to refuse rather than to fall back on
            // the shell's own reading of an address the layer did not vouch for.
            return
        }
        confirm(link, verb: "Open")
    }

    private func confirm(_ link: Link, verb: String) {
        guard let window = view.window else { return }
        let alert = NSAlert()
        alert.messageText = link.needsAMailHandler ? "Open this in your mail app?" : "Open this link?"

        var text = link.displayed
        if let wrapper = link.wrapper {
            // FR-30 keeps the wrapper available on request rather than discarding it: a user
            // who cannot see that a link was wrapped cannot judge who wrapped it. Sift
            // recovered the destination from the wrapper's own text and never followed it,
            // because following the redirect *is* the tracking event.
            text += "\n\nThis link was wrapped by a tracker:\n\(wrapper)"
        }
        if link.needsAMailHandler {
            text += "\n\nThis needs a mail app. Sift does not send mail."
        }
        alert.informativeText = text
        alert.addButton(withTitle: link.needsAMailHandler ? "\(verb) in Mail App" : "\(verb) in Browser")
        alert.addButton(withTitle: "Cancel")
        let target = link.target
        alert.beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn, let url = URL(string: target) else {
                return
            }
            NSWorkspace.shared.open(url)
        }
    }

    /// A borrowed UTF-8 pointer-and-length, copied. **Never NUL-terminated.**
    private static func string(_ s: SiftStr) -> String {
        guard let ptr = s.ptr, s.len > 0 else { return "" }
        return String(decoding: UnsafeBufferPointer(start: ptr, count: s.len), as: UTF8.self)
    }

    /// NFR-51, and the reader shows the absolute time rather than the list's relative one:
    /// "2 hours ago" is what a list needs and is not what a person checking a receipt does.
    private static let formatter: DateFormatter = {
        let f = DateFormatter()
        f.dateStyle = .medium
        f.timeStyle = .short
        return f
    }()

    private static func absolute(_ millis: UInt64) -> String {
        formatter.string(from: Date(timeIntervalSince1970: TimeInterval(millis) / 1000))
    }
}
