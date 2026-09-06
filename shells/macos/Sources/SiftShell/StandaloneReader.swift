import AppKit
import CSift

/// D-97's third window kind: one message, on its own.
///
/// **Its own reader, and therefore its own body view.** D-54 allows at most one body view per
/// window, and this is a window — so it gets one rather than borrowing the main window's. The
/// alternative, moving the main window's view here, would mean opening a message in a second
/// window silently emptied the first, which is a thing a user watches happen.
///
/// The document it opens is its own too, with its own D-28 token. Two windows showing two
/// messages share no address space, which is the property the token exists to hold — and it
/// holds across windows for the same reason it holds across navigations in one.
final class StandaloneReader: NSWindowController {
    private let reader = ReaderViewController()
    private let app: OpaquePointer

    init(app: OpaquePointer, row: MessageRow) {
        self.app = app
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 720, height: 860),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = row.subject.isEmpty ? "(no subject)" : row.subject
        window.isReleasedWhenClosed = false
        super.init(window: window)
        window.contentViewController = reader
        window.setContentSize(NSSize(width: 720, height: 860))
        window.center()
        reader.show(row, app: app)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }
}

/// FR-33's per-message debug view, available in release builds behind a preference.
///
/// # What it is for
///
/// The pipeline makes decisions a user cannot see the reasons for: this alternative rather than
/// that one, this resource withheld under that rule, this link unwrapped from that wrapper.
/// When a message renders wrongly — or renders and should not have — this is the difference
/// between a bug report that says "it looked odd" and one that names the stage.
///
/// It is **preference-gated and off by default**, per D-101, and the gate is checked here
/// rather than in the menu so that a keyboard shortcut cannot reach past it.
final class MessageDebugWindow: NSWindowController {
    private let app: OpaquePointer
    private let text = NSTextView()

    init(app: OpaquePointer) {
        self.app = app
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 760, height: 640),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false)
        window.title = "Message Details"
        window.isReleasedWhenClosed = false
        super.init(window: window)

        text.isEditable = false
        text.font = .monospacedSystemFont(ofSize: 11, weight: .regular)
        let scroll = NSScrollView()
        scroll.documentView = text
        scroll.hasVerticalScroller = true
        window.contentView = scroll
        window.center()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    /// Show what the pipeline did to one message.
    ///
    /// Opens its own document rather than reading the reader's: the reader's is revoked when
    /// its view navigates, and a debug window holding a revoked token would show a message the
    /// user has already moved on from — or nothing, with no way to tell which.
    func present(_ row: MessageRow) {
        var document = SiftDocument()
        guard sift_open_document(UnsafeMutablePointer(app), row.id, 0, &document) == Ok else {
            text.string = "This message could not be rendered, so there is nothing to show."
            showWindow(nil)
            return
        }
        let token = SiftText.string(document.token)
        var lines: [String] = [
            "subject   \(row.subject)",
            "from      \(row.sender)",
            "token     \(token)",
            "",
            "STAGES",
        ]
        lines.append(contentsOf: stages(token).map { "  \($0)" })

        lines.append("")
        lines.append("FETCHING POSITIONS — \(document.fetching_positions) found, \(document.blocked) withheld")
        var withheld = SiftRows_SiftWithheld()
        if SiftText.withBytes(token, { ptr, len in
            sift_document_withheld(UnsafeMutablePointer(app), ptr, len, &withheld) == Ok
        }), let ptr = withheld.ptr {
            for index in 0..<withheld.len {
                let row = ptr[index]
                lines.append(
                    "  \(SiftText.string(row.element))@\(SiftText.string(row.attribute))  "
                        + "\(SiftText.string(row.displayed))")
                // FR-33 item 4: a rule identifier next to every removal.
                lines.append("      rule: \(SiftText.string(row.rule))")
            }
        }

        lines.append("")
        lines.append("LINKS — displayed, resolved, and the wrapper each came from")
        var links = SiftRows_SiftLink()
        if SiftText.withBytes(token, { ptr, len in
            sift_document_links(UnsafeMutablePointer(app), ptr, len, &links) == Ok
        }), let ptr = links.ptr {
            for index in 0..<links.len {
                let link = ptr[index]
                lines.append("  shown     \(SiftText.string(link.displayed))")
                lines.append("  resolves  \(SiftText.string(link.target))")
                if link.wrapper.ptr != nil {
                    lines.append("  wrapper   \(SiftText.string(link.wrapper))")
                }
                if link.needs_a_mail_handler != 0 {
                    lines.append("  needs a mail handler; Sift will not send it")
                }
                lines.append("")
            }
        }

        lines.append("SANITIZED DOCUMENT")
        lines.append(SiftText.string(document.html))

        text.string = lines.joined(separator: "\n")
        // Closed as soon as it has been read from. The token is per document and revoked at
        // navigation, and holding one open here would keep an address space alive for a
        // message nobody is looking at.
        _ = SiftText.withBytes(token) { ptr, len in
            sift_close_document(UnsafeMutablePointer(app), ptr, len)
        }
        showWindow(nil)
        window?.makeKeyAndOrderFront(nil)
    }

    private func stages(_ token: String) -> [String] {
        var rows = SiftRows_SiftStr()
        guard SiftText.withBytes(token, { ptr, len in
            sift_document_stages(UnsafeMutablePointer(app), ptr, len, &rows) == Ok
        }), let ptr = rows.ptr else { return [] }
        return (0..<rows.len).map { SiftText.string(ptr[$0]) }
    }
}
