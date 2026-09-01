import AppKit
import CSift

/// FR-29's chrome: what the message asked for, what was withheld, and under which rule.
///
/// **Native, and never markup.** A count drawn inside the body is one a sender can write, and
/// a "load images" control drawn inside it is one a sender can counterfeit — which is the
/// reason the resource broker requires every affordance be native. This bar is that
/// requirement made concrete: nothing a sender sends can reach any pixel of it.
///
/// The disclosure exists because a count with nothing behind it tells a user that something
/// happened without telling them what. Each row names the element, the address as it really
/// reads, and the rule — and where no filter list is loaded, it says *that* rather than naming
/// a rule that did not run.
final class BlockedContentBar: NSView {
    private let summary = NSTextField(labelWithString: "")
    private let disclosure = NSButton()
    private let detail = NSStackView()
    private let loadOnce = NSButton()
    private let alwaysAllow = NSButton()

    /// Load the withheld resources for this document only.
    var onLoadOnce: (() -> Void)?
    /// Key a durable allowance on the sender's origin.
    var onAlwaysAllow: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        build()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    private func build() {
        wantsLayer = true
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor

        summary.font = .preferredFont(forTextStyle: .callout)
        summary.lineBreakMode = .byTruncatingTail

        disclosure.bezelStyle = .disclosure
        disclosure.setButtonType(.onOff)
        disclosure.title = ""
        disclosure.target = self
        disclosure.action = #selector(toggleDetail)

        loadOnce.title = "Load Once"
        loadOnce.bezelStyle = .rounded
        loadOnce.controlSize = .small
        loadOnce.target = self
        loadOnce.action = #selector(tapLoadOnce)

        alwaysAllow.title = "Always Load From This Sender"
        alwaysAllow.bezelStyle = .rounded
        alwaysAllow.controlSize = .small
        alwaysAllow.target = self
        alwaysAllow.action = #selector(tapAlwaysAllow)

        detail.orientation = .vertical
        detail.alignment = .leading
        detail.spacing = 3
        detail.isHidden = true

        let top = NSStackView(views: [disclosure, summary, NSView(), loadOnce, alwaysAllow])
        top.orientation = .horizontal
        top.alignment = .centerY
        top.spacing = 8

        let stack = NSStackView(views: [top, detail])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 6
        stack.edgeInsets = NSEdgeInsets(top: 8, left: 24, bottom: 8, right: 24)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
    }

    /// Draw the bar for one document, or hide it where nothing was withheld.
    ///
    /// `mayAlwaysAllow` is false where nothing authenticated the message. The durable-allowance
    /// button is then **removed rather than disabled**: an allowance keyed on nothing would
    /// apply to every sender, which is the opposite of what the button says it does.
    func show(app: OpaquePointer, token: String, blocked: UInt32, mayAlwaysAllow: Bool) {
        guard blocked > 0 else {
            isHidden = true
            return
        }
        isHidden = false
        summary.stringValue =
            blocked == 1
            ? "1 remote resource not loaded"
            : "\(blocked) remote resources not loaded"
        alwaysAllow.isHidden = !mayAlwaysAllow

        detail.arrangedSubviews.forEach { $0.removeFromSuperview() }
        for row in BlockedContentBar.withheld(app: app, token: token) {
            let label = NSTextField(labelWithString: row)
            label.font = .monospacedSystemFont(ofSize: 10, weight: .regular)
            label.textColor = .secondaryLabelColor
            label.lineBreakMode = .byTruncatingMiddle
            detail.addArrangedSubview(label)
        }
        setAccessibilityLabel("\(summary.stringValue). Details available.")
    }

    /// Read the disclosure's rows from the layer.
    ///
    /// The rows borrow from the open document and are valid until it is closed, so each one is
    /// copied into a Swift string here rather than held.
    private static func withheld(app: OpaquePointer, token: String) -> [String] {
        var rows = SiftRows_SiftWithheld()
        let ok = SiftText.withBytes(token) { ptr, len in
            sift_document_withheld(UnsafeMutablePointer(app), ptr, len, &rows) == Ok
        }
        guard ok, let ptr = rows.ptr, rows.len > 0 else { return [] }
        return (0..<rows.len).map { index in
            let row = ptr[index]
            let element = SiftText.string(row.element)
            let attribute = SiftText.string(row.attribute)
            let displayed = SiftText.string(row.displayed)
            let rule = SiftText.string(row.rule)
            return "\(element)@\(attribute)  \(displayed) — \(rule)"
        }
    }

    @objc private func toggleDetail() {
        detail.isHidden = disclosure.state != .on
    }

    @objc private func tapLoadOnce() {
        onLoadOnce?()
    }

    @objc private func tapAlwaysAllow() {
        onAlwaysAllow?()
    }
}

/// Copying a borrowed UTF-8 pointer-and-length, in one place.
///
/// **Never NUL-terminated, and the length is authoritative** — nothing on this boundary scans
/// for a terminator, because a sender can put a NUL in a display name and a scan would truncate
/// there. `String(cString:)` is the exact mistake this exists to prevent.
enum SiftText {
    static func string(_ s: SiftStr) -> String {
        guard let ptr = s.ptr, s.len > 0 else { return "" }
        return String(decoding: UnsafeBufferPointer(start: ptr, count: s.len), as: UTF8.self)
    }

    /// Lend a Swift string to the layer as a pointer and a length, for the duration of `body`.
    ///
    /// The layer copies what it needs before returning, so the borrow never outlives the call
    /// — the same contract as the one above, in the other direction.
    static func withBytes<R>(_ s: String, _ body: (UnsafePointer<UInt8>?, Int) -> R) -> R {
        var s = s
        return s.withUTF8 { body($0.baseAddress, $0.count) }
    }
}
