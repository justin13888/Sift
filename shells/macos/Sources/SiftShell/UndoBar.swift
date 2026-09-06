import AppKit
import CSift

/// FR-15's timed undo, as a transient in-window affordance.
///
/// **The record is the layer's, not this view's.** D-86 is explicit about why: a window shell
/// is destroyed when its window closes, in a product that runs with no window at all, so a
/// countdown owned by a view would die with the view — and a user who archives a message and
/// closes the window would silently lose the undo. This reads `sift_undoable` and draws it.
///
/// **The countdown is not a deadline on reversibility.** Every intent but permanent delete
/// stays reversible for as long as the message exists, through the ordinary interface. Only
/// the toast is transient, and only for intents that take the message out of view — where the
/// user has nothing left to click. A countdown on every message the reader marks read would
/// make the mechanism worthless by making it constant.
final class UndoBar: NSView {
    private let label = NSTextField(labelWithString: "")
    private let button = NSButton()
    private var timer: Timer?
    private var app: OpaquePointer?
    var onUndo: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 8
        layer?.backgroundColor = NSColor.controlBackgroundColor.cgColor

        label.font = .preferredFont(forTextStyle: .callout)
        button.title = "Undo"
        button.bezelStyle = .rounded
        button.controlSize = .small
        button.target = self
        button.action = #selector(tap)
        button.keyEquivalent = "z"
        button.keyEquivalentModifierMask = [.command]

        let stack = NSStackView(views: [label, button])
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 12
        stack.edgeInsets = NSEdgeInsets(top: 8, left: 14, bottom: 8, right: 10)
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
        ])
        isHidden = true
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    /// Refresh from the layer. Called after every gesture, and on the countdown's own tick.
    func refresh(app: OpaquePointer) {
        self.app = app
        var record = SiftUndoable()
        guard sift_undoable(UnsafeMutablePointer(app), &record) == Ok else {
            hide()
            return
        }
        // No countdown for an intent that leaves the message in front of the user. It is still
        // reversible — through the menu, which is where the affordance belongs for something
        // with no deadline on it.
        guard record.timed != 0 else {
            hide()
            return
        }
        guard record.remaining_millis > 0 else {
            hide()
            return
        }
        isHidden = false
        let seconds = Int((record.remaining_millis + 999) / 1000)
        let what = SiftText.string(record.intent)
        let subject =
            record.messages == 1 ? "1 message" : "\(record.messages) messages"
        label.stringValue = "\(UndoBar.verb(what)) \(subject) · \(seconds)s"
        setAccessibilityLabel("\(label.stringValue). Undo available.")
        startTicking()
    }

    /// The layer names the intent; the shell supplies the sentence.
    private static func verb(_ intent: String) -> String {
        switch intent {
        case "archive": return "Archived"
        case "delete-to-trash": return "Moved to Trash"
        case "move-to": return "Moved"
        case "report-junk": return "Moved to Junk"
        case "report-not-junk": return "Marked not junk"
        default: return "Changed"
        }
    }

    private func startTicking() {
        guard timer == nil else { return }
        // Half-second, so the visible number never lags the truth by a whole second. The timer
        // exists only while a countdown does — an always-on timer in a resident process is
        // exactly the wakeup NFR-11 counts.
        let timer = Timer(timeInterval: 0.5, repeats: true) { [weak self] _ in
            guard let self, let app = self.app else { return }
            self.refresh(app: app)
        }
        RunLoop.main.add(timer, forMode: .common)
        self.timer = timer
    }

    private func hide() {
        isHidden = true
        timer?.invalidate()
        timer = nil
    }

    @objc private func tap() {
        onUndo?()
    }

    deinit {
        timer?.invalidate()
    }
}
