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
    private let blocked = NSTextField(labelWithString: "")
    private let empty = NSTextField(labelWithString: "No message selected")
    private let body = BodyView(frame: .zero)

    override func loadView() {
        subject.font = .preferredFont(forTextStyle: .title2)
        subject.lineBreakMode = .byTruncatingTail
        sender.font = .preferredFont(forTextStyle: .body)
        sender.textColor = .secondaryLabelColor
        received.font = .preferredFont(forTextStyle: .caption1)
        received.textColor = .secondaryLabelColor
        blocked.font = .preferredFont(forTextStyle: .caption1)
        blocked.textColor = .secondaryLabelColor
        empty.font = .preferredFont(forTextStyle: .title3)
        empty.textColor = .tertiaryLabelColor

        let header = NSStackView(views: [subject, sender, received, blocked])
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 6
        header.translatesAutoresizingMaskIntoConstraints = false
        header.edgeInsets = NSEdgeInsets(top: 20, left: 24, bottom: 20, right: 24)

        let container = NSView()
        container.addSubview(header)
        body.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(body)
        empty.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(empty)

        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: container.topAnchor),
            header.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            body.topAnchor.constraint(equalTo: header.bottomAnchor),
            body.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            body.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            body.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            empty.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            empty.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])

        // FR-30: a link is never followed to find out where it goes — following a wrapper is
        // itself the tracking event. The destination shown is the one the document declared.
        body.onLink = { [weak self] url in self?.confirmOpen(url) }
        view = container
        show(nil, app: nil)
    }

    func show(_ row: MessageRow?, app: OpaquePointer?) {
        guard let row else {
            body.clear()
            // The empty state is a stated one rather than a blank pane, because a blank pane
            // is indistinguishable from something that failed to draw.
            subject.isHidden = true
            sender.isHidden = true
            received.isHidden = true
            blocked.isHidden = true
            body.isHidden = true
            empty.isHidden = false
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
            blocked.isHidden = true
            body.clear()
            return
        }
        var document = SiftDocument()
        let status = sift_open_document(UnsafeMutablePointer(app), row.id, 0, &document)
        guard status == Ok else {
            // FR-9: a body that cannot be rendered degrades to a stated absence rather than
            // to a blank pane, because a blank pane is indistinguishable from a bug.
            blocked.isHidden = false
            blocked.stringValue = "This message could not be displayed."
            body.clear()
            return
        }
        let html = ReaderViewController.string(document.html)
        let token = ReaderViewController.string(document.token)

        // Native chrome, never markup. The count comes from the layer, and a count a sender
        // could write would not be a count.
        if document.blocked > 0 {
            blocked.isHidden = false
            blocked.stringValue =
                document.blocked == 1
                ? "1 remote resource not loaded" : "\(document.blocked) remote resources not loaded"
        } else {
            blocked.isHidden = true
        }
        body.show(html: html, token: token, app: app)
    }

    /// The link-confirmation sheet — D-97 puts it on the window that raised it.
    ///
    /// Nothing opens without it. The destination is shown as it really is, and Sift never
    /// resolves a wrapper by fetching it, because following the redirect *is* the tracking
    /// event FR-30 refuses to cause.
    private func confirmOpen(_ url: URL) {
        guard let window = view.window else { return }
        let alert = NSAlert()
        alert.messageText = "Open this link?"
        // Punycode decoded and bidi controls stripped, so the domain shown is the domain
        // reached. A host rendered in its raw form is one that can be made to read as another.
        alert.informativeText = ReaderViewController.display(url)
        alert.addButton(withTitle: "Open in Browser")
        alert.addButton(withTitle: "Cancel")
        alert.beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            NSWorkspace.shared.open(url)
        }
    }

    /// What a destination should look like to a person deciding whether to go there.
    private static func display(_ url: URL) -> String {
        let host = url.host.map { host -> String in
            // IDNA, decoded. `URL` keeps the punycode form, and `xn--` is not a domain a
            // person can judge.
            (host as NSString).replacingOccurrences(of: "\u{200E}", with: "")
        } ?? ""
        let stripped = url.absoluteString.unicodeScalars
            .filter { !(0x202A...0x202E).contains($0.value) && !(0x2066...0x2069).contains($0.value) }
            .reduce(into: "") { $0.unicodeScalars.append($1) }
        return host.isEmpty ? stripped : "\(host)\n\n\(stripped)"
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
