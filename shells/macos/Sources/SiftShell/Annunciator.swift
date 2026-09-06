import AppKit
import CSift

/// D-49's annunciator: **one badge, showing the single highest-precedence condition.**
///
/// Not a list, and not one badge per account. The conditions are ordered, the worst one wins,
/// and `healthy` draws nothing at all — a badge that is always present is a badge nobody
/// reads, which is the failure mode this design is avoiding rather than the tidiness it looks
/// like.
///
/// `recovering` is the case worth reading the code for. It is the only non-healthy condition
/// that asks nothing of the user, so it is drawn in a neutral colour and worded as progress.
/// Presenting a backfill as a fault would be dishonest, and a user who learns that the badge
/// sometimes means nothing stops reading it for the times it means something.
enum Annunciator {
    // The header emits the eight as `#define`s, which arrive in Swift as `Int32`. Narrowing
    // them once, here, keeps every use below a comparison between two `SiftCondition`s rather
    // than a cast repeated eight times — and leaves one place to look when the set grows.
    static let needsAuthentication = SiftCondition(SiftCondition_NEEDS_AUTHENTICATION)
    static let storageUnavailable = SiftCondition(SiftCondition_STORAGE_UNAVAILABLE)
    static let pausedByUser = SiftCondition(SiftCondition_PAUSED_BY_USER)
    static let pausedByDataCap = SiftCondition(SiftCondition_PAUSED_BY_DATA_CAP)
    static let recovering = SiftCondition(SiftCondition_RECOVERING)
    static let degraded = SiftCondition(SiftCondition_DEGRADED)
    static let attention = SiftCondition(SiftCondition_ATTENTION)
    static let healthy = SiftCondition(SiftCondition_HEALTHY)

    struct State {
        let condition: SiftCondition
        let accounts: UInt32
        let asksSomething: Bool
        let reachesTheUserWithoutAWindow: Bool

        var isHealthy: Bool { condition == Annunciator.healthy }
    }

    static func read(app: OpaquePointer) -> State? {
        var out = SiftAnnunciator()
        guard sift_annunciator(UnsafeMutablePointer(app), &out) == Ok else { return nil }
        return State(
            condition: out.condition,
            accounts: out.accounts,
            asksSomething: out.asks_something_of_the_user != 0,
            reachesTheUserWithoutAWindow: out.reaches_the_user_without_a_window != 0)
    }

    /// D-56: the layer returns identified states and **this shell supplies every word**.
    static func words(_ state: State) -> (title: String, detail: String) {
        let many = state.accounts > 1
        let subject = many ? "\(state.accounts) accounts" : "This account"
        switch state.condition {
        case Annunciator.needsAuthentication:
            return (
                many ? "\(state.accounts) accounts need signing in again" : "Sign in again",
                "\(subject) can no longer reach the mail server. Sift will keep what it has until you sign in."
            )
        case Annunciator.storageUnavailable:
            return (
                "Sift cannot write to disk",
                "Sift has stopped accepting changes rather than showing you ones it cannot save. Check free space."
            )
        case Annunciator.pausedByUser:
            return ("Syncing is paused", "You paused it. It will not resume on its own.")
        case Annunciator.pausedByDataCap:
            return (
                "Paused — data limit reached",
                "Syncing resumes on its own when the period rolls over, or when you raise the limit."
            )
        case Annunciator.recovering:
            // Progress, not a fault. The user action is nothing, and the words say so.
            return ("Catching up…", "Sift is fetching mail it does not have yet. Nothing to do.")
        case Annunciator.degraded:
            return (
                "Working, with less than usual",
                "The server withdrew something Sift was using. Mail still arrives; it costs more to keep current."
            )
        case Annunciator.attention:
            return (
                "Something needs you",
                "Either a change could not be applied, or triage is being held on an account Sift is only watching."
            )
        default:
            return ("", "")
        }
    }

    /// The symbol and colour. `healthy` has neither, because it draws nothing.
    static func appearance(_ state: State) -> (symbol: String, colour: NSColor)? {
        switch state.condition {
        case Annunciator.needsAuthentication: return ("person.crop.circle.badge.exclamationmark", .systemRed)
        case Annunciator.storageUnavailable: return ("externaldrive.badge.xmark", .systemRed)
        case Annunciator.pausedByUser: return ("pause.circle", .secondaryLabelColor)
        case Annunciator.pausedByDataCap: return ("pause.circle", .systemOrange)
        case Annunciator.recovering: return ("arrow.triangle.2.circlepath", .secondaryLabelColor)
        case Annunciator.degraded: return ("exclamationmark.triangle", .systemOrange)
        case Annunciator.attention: return ("exclamationmark.circle", .systemOrange)
        default: return nil
        }
    }
}

/// The in-window badge. Absent when healthy rather than present and blank.
final class AnnunciatorView: NSView {
    private let icon = NSImageView()
    private let label = NSTextField(labelWithString: "")
    var onClick: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        label.font = .preferredFont(forTextStyle: .caption1)
        label.lineBreakMode = .byTruncatingTail
        let stack = NSStackView(views: [icon, label])
        stack.orientation = .horizontal
        stack.alignment = .centerY
        stack.spacing = 6
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            stack.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -8),
            stack.centerYAnchor.constraint(equalTo: centerYAnchor),
            heightAnchor.constraint(equalToConstant: 28),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { nil }

    func show(app: OpaquePointer) {
        guard let state = Annunciator.read(app: app), !state.isHealthy,
            let look = Annunciator.appearance(state)
        else {
            isHidden = true
            return
        }
        isHidden = false
        let words = Annunciator.words(state)
        icon.image = NSImage(systemSymbolName: look.symbol, accessibilityDescription: words.title)
        icon.contentTintColor = look.colour
        label.stringValue = words.title
        label.textColor = state.asksSomething ? .labelColor : .secondaryLabelColor
        toolTip = words.detail
        setAccessibilityLabel("\(words.title). \(words.detail)")
    }

    override func mouseDown(with event: NSEvent) {
        onClick?()
    }
}
