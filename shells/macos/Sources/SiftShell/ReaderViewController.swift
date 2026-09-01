import AppKit
import CSift

/// D-97's reader.
///
/// Today it draws the envelope a selection names. The body view — D-3's separate hardened
/// document under N-1, with its own scheme handler and no network capability at all — is the
/// next thing to land here, and it is deliberately absent rather than approximated: a reader
/// that rendered message HTML into this view's own hierarchy would be exactly the design D-3
/// rejects, and having it "work" would make it harder to replace than having it missing.
final class ReaderViewController: NSViewController {
    private let sender = NSTextField(labelWithString: "")
    private let subject = NSTextField(labelWithString: "")
    private let received = NSTextField(labelWithString: "")
    private let placeholder = NSTextField(labelWithString: "")
    private let empty = NSTextField(labelWithString: "No message selected")

    override func loadView() {
        subject.font = .preferredFont(forTextStyle: .title2)
        subject.lineBreakMode = .byTruncatingTail
        sender.font = .preferredFont(forTextStyle: .body)
        sender.textColor = .secondaryLabelColor
        received.font = .preferredFont(forTextStyle: .caption1)
        received.textColor = .secondaryLabelColor
        placeholder.font = .preferredFont(forTextStyle: .body)
        placeholder.textColor = .tertiaryLabelColor
        placeholder.stringValue = "The body view is not wired yet."
        empty.font = .preferredFont(forTextStyle: .title3)
        empty.textColor = .tertiaryLabelColor

        let header = NSStackView(views: [subject, sender, received, placeholder])
        header.orientation = .vertical
        header.alignment = .leading
        header.spacing = 6
        header.translatesAutoresizingMaskIntoConstraints = false
        header.edgeInsets = NSEdgeInsets(top: 20, left: 24, bottom: 20, right: 24)

        let container = NSView()
        container.addSubview(header)
        empty.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(empty)

        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: container.topAnchor),
            header.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: container.trailingAnchor),
            empty.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            empty.centerYAnchor.constraint(equalTo: container.centerYAnchor),
        ])
        view = container
        show(nil, app: nil)
    }

    func show(_ row: MessageRow?, app: OpaquePointer?) {
        guard let row else {
            // The empty state is a stated one rather than a blank pane, because a blank pane
            // is indistinguishable from something that failed to draw.
            subject.isHidden = true
            sender.isHidden = true
            received.isHidden = true
            placeholder.isHidden = true
            empty.isHidden = false
            return
        }
        empty.isHidden = true
        subject.isHidden = false
        sender.isHidden = false
        received.isHidden = false
        placeholder.isHidden = false

        subject.stringValue = row.subject.isEmpty ? "(no subject)" : row.subject
        // NFR-54 requires the address be shown beside the display name rather than instead of
        // it: a display name alone is a sender's own claim about who they are.
        sender.stringValue = row.sender
        received.stringValue = ReaderViewController.absolute(row.receivedMillis)
        view.setAccessibilityLabel("\(row.subject), from \(row.sender)")
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
