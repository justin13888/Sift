import AppKit
import CSift

/// One drawn row of FR-6's list.
///
/// The fields FR-6 names, arranged. The spec lists what a row must show and deliberately says
/// nothing about how — D-56 keeps Sift's own presentation out of the layer, so this is the
/// shell's call and is written down here rather than implied by a nib.
///
/// **Every string drawn here arrived already normalized.** NFR-54 runs below the boundary,
/// because the sender, subject and snippet are attacker-controlled and this is native chrome
/// where no sanitizer invariant sees them. Nothing in this file re-checks that, and nothing
/// in this file may undo it — which is why the isolate marks are drawn rather than stripped.
final class MessageRowView: NSTableCellView {
    private let unreadDot = NSView()
    private let sender = NSTextField(labelWithString: "")
    private let date = NSTextField(labelWithString: "")
    private let subject = NSTextField(labelWithString: "")
    private let snippet = NSTextField(labelWithString: "")
    private let marks = NSTextField(labelWithString: "")

    init(identifier: NSUserInterfaceItemIdentifier) {
        super.init(frame: .zero)
        self.identifier = identifier

        unreadDot.wantsLayer = true
        unreadDot.layer?.cornerRadius = 4
        unreadDot.layer?.backgroundColor = NSColor.controlAccentColor.cgColor

        sender.font = .preferredFont(forTextStyle: .body)
        sender.lineBreakMode = .byTruncatingTail
        date.font = .preferredFont(forTextStyle: .caption1)
        date.textColor = .secondaryLabelColor
        date.alignment = .right
        date.setContentHuggingPriority(.required, for: .horizontal)
        subject.font = .preferredFont(forTextStyle: .body)
        subject.lineBreakMode = .byTruncatingTail
        snippet.font = .preferredFont(forTextStyle: .caption1)
        snippet.textColor = .secondaryLabelColor
        snippet.lineBreakMode = .byTruncatingTail
        marks.font = .preferredFont(forTextStyle: .caption1)
        marks.textColor = .secondaryLabelColor
        marks.setContentHuggingPriority(.required, for: .horizontal)

        let top = NSStackView(views: [unreadDot, sender, date])
        top.orientation = .horizontal
        top.spacing = 6
        top.alignment = .firstBaseline

        let bottom = NSStackView(views: [subject, marks])
        bottom.orientation = .horizontal
        bottom.spacing = 6

        let stack = NSStackView(views: [top, bottom, snippet])
        stack.orientation = .vertical
        stack.spacing = 2
        stack.alignment = .leading
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)

        NSLayoutConstraint.activate([
            unreadDot.widthAnchor.constraint(equalToConstant: 8),
            unreadDot.heightAnchor.constraint(equalToConstant: 8),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -10),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("not loaded from a nib") }

    func show(_ row: MessageRow) {
        unreadDot.isHidden = !row.unread
        sender.stringValue = row.sender
        // Unread is weight, not colour: colour alone fails at the one thing an unread marker
        // is for, and NFR-27's readers cannot see it at all.
        sender.font = .preferredFont(forTextStyle: row.unread ? .headline : .body)
        subject.stringValue = row.subject.isEmpty ? "(no subject)" : row.subject
        snippet.stringValue = row.snippet
        date.stringValue = MessageRowView.relative(row.receivedMillis)

        // The trailing cluster. Absent rather than dimmed when it does not apply — the same
        // rule D-98 applies to actions, for the same reason: a marker that is always there
        // stops meaning anything.
        var trailing: [String] = []
        if row.flagged { trailing.append("⚑") }
        if row.hasAttachments { trailing.append("📎") }
        if row.threadCount > 1 { trailing.append("\(row.threadCount)") }
        // D-4: a message in two accounts is **marked, not joined**.
        if row.duplicateAcrossAccounts { trailing.append("⧉") }
        marks.stringValue = trailing.joined(separator: "  ")

        // NFR-27. The row is one thing to a screen reader, and it announces what a sighted
        // reader sees in the order they see it.
        setAccessibilityLabel(
            "\(row.unread ? "Unread. " : "")\(row.sender). \(row.subject). \(row.snippet)"
        )
        setAccessibilityRole(.row)
    }

    /// NFR-51: locale-aware, from the first commit rather than retrofitted.
    private static let formatter: RelativeDateTimeFormatter = {
        let f = RelativeDateTimeFormatter()
        f.unitsStyle = .abbreviated
        return f
    }()

    private static func relative(_ millis: UInt64) -> String {
        let date = Date(timeIntervalSince1970: TimeInterval(millis) / 1000)
        return formatter.localizedString(for: date, relativeTo: Date())
    }
}
