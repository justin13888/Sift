import AppKit
import CSift
import UserNotifications

/// FR-23 — native notifications for new mail, and what happens when one is clicked.
///
/// **Both halves, in one place.** The layer's `new_mail` host callback is the delivery half: once
/// per account per wheel fire that brought delivered-and-unread mail in. The platform's
/// notification response is the activation half, and it arrives here rather than through the
/// layer because the platform delivers it to the process. It carries only the two identifiers
/// the notification was posted with — which is all that survives a relaunch — and the shell
/// reads the row back by identity to open it.
///
/// **Authorization is asked where it is about something.** A prompt at launch arrives with no
/// context, on a process that may not have synced anything yet. It is asked instead when an
/// account's first sync is requested, and — for an installation that predates this, or a
/// person who added the account before the prompt could be answered — at the first new mail,
/// which is the one moment the question cannot be mistaken for anything else.
final class NewMailNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NewMailNotifier()

    private static let accountKey = "sift.account"
    private static let messageKey = "sift.message"

    /// The platform's centre, or nothing outside an application bundle.
    ///
    /// `UNUserNotificationCenter.current()` raises for a process with no bundle identifier,
    /// and the body-view and accessibility probes run this binary in ways that are not a
    /// launched application. A notifier that cannot notify is a notifier that says nothing;
    /// one that crashes the probe is a probe that no longer runs.
    private var center: UNUserNotificationCenter? {
        Bundle.main.bundleIdentifier == nil ? nil : UNUserNotificationCenter.current()
    }

    /// Become the centre's delegate. **Before launch finishes**, because a notification that
    /// launched the process is delivered to whatever delegate is set by then and to nothing
    /// set afterwards.
    func install() {
        center?.delegate = self
    }

    /// Ask for permission to notify, if nobody has answered yet. A decision already made —
    /// either way — is the person's, and is never asked again from here.
    func requestAuthorizationIfUndecided(then: ((Bool) -> Void)? = nil) {
        guard let center else {
            then?(false)
            return
        }
        center.getNotificationSettings { settings in
            switch settings.authorizationStatus {
            case .notDetermined:
                center.requestAuthorization(options: [.alert, .sound]) { granted, _ in
                    DispatchQueue.main.async { then?(granted) }
                }
            case .authorized, .provisional:
                DispatchQueue.main.async { then?(true) }
            case .denied:
                DispatchQueue.main.async { then?(false) }
            @unknown default:
                DispatchQueue.main.async { then?(false) }
            }
        }
    }

    /// Post one account's new mail. Called on the main loop, a turn after the host callback.
    ///
    /// D-56: the layer sends a count and a row, and **this shell supplies every word**.
    func announce(account: SiftId, delivered: UInt32, newest: MessageRow?) {
        guard delivered > 0 else { return }
        let content = UNMutableNotificationContent()
        let sender = newest.map { $0.sender.isEmpty ? "Unknown sender" : $0.sender }
        let subject = newest.map { $0.subject.isEmpty ? "(no subject)" : $0.subject }
        if delivered == 1, let sender, let subject {
            content.title = sender
            content.body = subject
        } else {
            content.title = delivered == 1 ? "1 new message" : "\(delivered) new messages"
            if let sender, let subject {
                content.subtitle = sender
                content.body = subject
            }
        }
        content.sound = .default
        // One group per account, so a busy account stacks rather than burying the others.
        content.threadIdentifier = account.key
        var info: [String: String] = [Self.accountKey: account.key]
        if let newest { info[Self.messageKey] = newest.id.key }
        content.userInfo = info

        // By the message it names, so the same arrival announced twice replaces itself rather
        // than stacking; an announcement with no row to name is unique to its moment.
        let identifier = "new-mail." + (newest?.id.key ?? "\(account.key).\(UUID().uuidString)")
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        requestAuthorizationIfUndecided { [weak self] granted in
            guard granted, let center = self?.center else { return }
            center.add(request, withCompletionHandler: nil)
        }
    }

    // MARK: - UNUserNotificationCenterDelegate

    /// Shown even while Sift is frontmost. The main window's list already shows the row, but
    /// the frontmost window may be Settings, a reader, or a different account's mail.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }

    /// FR-23's activation — **opens that message**, on a process that may have just been
    /// launched to receive it.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let info = response.notification.request.content.userInfo
        let account = (info[Self.accountKey] as? String).flatMap(SiftId.init(key:))
        let message = (info[Self.messageKey] as? String).flatMap(SiftId.init(key:))
        DispatchQueue.main.async {
            if let account, let message {
                ApplicationShell.shared.openNotifiedMessage(account: account, message: message)
            } else {
                // A notification with no message to name still means "show me Sift".
                ApplicationShell.shared.openMainWindow()
            }
        }
        completionHandler()
    }
}

extension SiftId {
    /// The inverse of `key`: sixteen bytes from thirty-two hex digits, or nothing.
    ///
    /// A notification's user info is the one place an identity is stored as text, and it is
    /// read back after a relaunch — so anything that is not exactly a key is refused rather
    /// than half-parsed into an identity that names some other message.
    init?(key: String) {
        let digits = Array(key.utf8)
        guard digits.count == 32 else { return nil }
        var bytes = [UInt8](repeating: 0, count: 16)
        for index in 0..<16 {
            guard let high = Self.nibble(digits[2 * index]),
                  let low = Self.nibble(digits[2 * index + 1]) else { return nil }
            bytes[index] = high << 4 | low
        }
        var id = SiftId.zero
        withUnsafeMutableBytes(of: &id.bytes) { raw in
            raw.copyBytes(from: bytes)
        }
        self = id
    }

    private static func nibble(_ c: UInt8) -> UInt8? {
        switch c {
        case UInt8(ascii: "0")...UInt8(ascii: "9"): return c - UInt8(ascii: "0")
        case UInt8(ascii: "a")...UInt8(ascii: "f"): return c - UInt8(ascii: "a") + 10
        case UInt8(ascii: "A")...UInt8(ascii: "F"): return c - UInt8(ascii: "A") + 10
        default: return nil
        }
    }
}
