import AppKit
import CSift

/// FR-10's attachment list, and NFR-53's save.
///
/// **Nothing here has been downloaded.** The list is built from the structure the sync already
/// holds; a forty-megabyte part costs nothing until somebody presses Save, which is a claim
/// about requests rather than about intentions.
///
/// The save is two gestures because the requirement is two gestures: the exact final path is
/// resolved and **shown** before anything is written. A chooser that let the user type a name
/// would put a sender-supplied name back into the path by the back door — so the user chooses
/// the *directory* and Sift derives the name, then shows exactly what it derived.
final class AttachmentBar: NSView {
    private let stack = NSStackView()
    private var app: OpaquePointer?
    private var message = SiftId.zero

    override init(frame: NSRect) {
        super.init(frame: frame)
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 10, left: 24, bottom: 10, right: 24)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    func show(app: OpaquePointer, message: SiftId) {
        self.app = app
        self.message = message
        stack.arrangedSubviews.forEach { $0.removeFromSuperview() }

        var rows = SiftRows_SiftAttachment()
        guard sift_message_attachments(UnsafeMutablePointer(app), message, &rows) == Ok,
            let ptr = rows.ptr, rows.len > 0
        else {
            isHidden = true
            return
        }
        isHidden = false

        let heading = NSTextField(
            labelWithString: rows.len == 1 ? "1 attachment" : "\(rows.len) attachments")
        heading.font = .preferredFont(forTextStyle: .caption1)
        heading.textColor = .secondaryLabelColor
        stack.addArrangedSubview(heading)

        for index in 0..<rows.len {
            stack.addArrangedSubview(row(ptr[index]))
        }
    }

    private func row(_ attachment: SiftAttachment) -> NSView {
        let part = SiftText.string(attachment.part)
        let displayName = SiftText.string(attachment.display_name)
        let fileName = SiftText.string(attachment.file_name)

        let name = NSTextField(labelWithString: displayName.isEmpty ? fileName : displayName)
        name.font = .preferredFont(forTextStyle: .body)
        name.lineBreakMode = .byTruncatingMiddle

        let size = NSTextField(labelWithString: AttachmentBar.bytes(attachment.declared_size))
        size.font = .preferredFont(forTextStyle: .caption1)
        size.textColor = .secondaryLabelColor

        let save = NSButton(title: "Save…", target: self, action: #selector(tapSave(_:)))
        save.bezelStyle = .rounded
        save.controlSize = .small
        save.identifier = NSUserInterfaceItemIdentifier(part)

        let row = NSStackView(views: [name, size, NSView(), save])
        row.orientation = .horizontal
        row.alignment = .centerY
        row.spacing = 10

        guard attachment.warning != 0 else { return row }

        // FR-10's warning, stated in the list rather than only at the moment of opening — a
        // user deciding whether to save should be told before they decide, not after.
        let warning = NSTextField(labelWithString: AttachmentBar.why(attachment.warning))
        warning.font = .preferredFont(forTextStyle: .caption1)
        warning.textColor = .systemOrange
        warning.lineBreakMode = .byWordWrapping
        warning.setAccessibilityLabel("Warning. \(AttachmentBar.why(attachment.warning))")

        let column = NSStackView(views: [row, warning])
        column.orientation = .vertical
        column.alignment = .leading
        column.spacing = 2
        return column
    }

    /// The three sources, said separately. "This is a program" and "this says it is a document
    /// and is named like a program" are different sentences, and the second is the one worth
    /// reading — a sender who labels an executable as a document has told you something.
    private static func why(_ bits: UInt32) -> String {
        var reasons: [String] = []
        if bits & UInt32(SIFT_WARN_DISAGREES) != 0 {
            reasons.append("its declared type and its name disagree")
        }
        if bits & UInt32(SIFT_WARN_EXTENSION) != 0 {
            reasons.append("its name ends in an extension the system will execute")
        }
        if bits & UInt32(SIFT_WARN_DECLARED) != 0 && reasons.isEmpty {
            reasons.append("it is a program")
        }
        return "Careful — " + reasons.joined(separator: ", and ") + "."
    }

    private static func bytes(_ n: UInt64) -> String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter.string(fromByteCount: Int64(bitPattern: n))
    }

    @objc private func tapSave(_ sender: NSButton) {
        guard let app, let window = self.window,
            let part = sender.identifier?.rawValue
        else { return }

        // A directory rather than a file. NFR-53 derives the name, and a panel that let the
        // user type one would hand the naming back to whoever chose the sender's.
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.prompt = "Choose"
        panel.message = "Where should Sift save this attachment?"
        panel.beginSheetModal(for: window) { [weak self] response in
            guard response == .OK, let directory = panel.url else { return }
            self?.confirm(app: app, part: part, directory: directory, window: window)
        }
    }

    /// Resolve the path, show it, and write only on confirmation.
    private func confirm(app: OpaquePointer, part: String, directory: URL, window: NSWindow) {
        var plan = SiftSavePlan()
        let planned = SiftText.withBytes(part) { partPtr, partLen in
            SiftText.withBytes(directory.path) { pathPtr, pathLen in
                sift_plan_attachment_save(
                    UnsafeMutablePointer(app), message,
                    partPtr, partLen, pathPtr, pathLen, &plan) == Ok
            }
        }
        guard planned else {
            AttachmentBar.report("Sift could not work out where to put this file.", on: window)
            return
        }

        let alert = NSAlert()
        alert.messageText = "Save this attachment?"
        var text = "Sift will write:\n\n\(SiftText.string(plan.final_path))"
        if plan.renamed != 0 {
            // The requirement's own words: a sender-supplied filename never becomes a path.
            // Saying so is what makes the difference visible rather than surprising.
            text += "\n\nThe name comes from the sender, so Sift has derived a safe one."
        }
        alert.informativeText = text
        alert.addButton(withTitle: "Save")
        alert.addButton(withTitle: "Cancel")
        let handle = plan.plan
        alert.beginSheetModal(for: window) { response in
            guard response == .alertFirstButtonReturn else { return }
            var outcome = SiftSaveOutcome()
            guard sift_write_attachment(UnsafeMutablePointer(app), handle, &outcome) == Ok else {
                AttachmentBar.report(
                    "Sift could not write the file. Nothing was overwritten.", on: window)
                return
            }
            if outcome.warning != 0 {
                AttachmentBar.report(
                    "Saved. \(AttachmentBar.why(outcome.warning)) Sift will not open it for you.",
                    on: window)
            }
        }
    }

    private static func report(_ message: String, on window: NSWindow) {
        let alert = NSAlert()
        alert.messageText = message
        alert.addButton(withTitle: "OK")
        alert.beginSheetModal(for: window, completionHandler: nil)
    }
}
