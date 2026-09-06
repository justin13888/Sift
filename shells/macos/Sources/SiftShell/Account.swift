import Foundation
import CSift

/// One account, copied out of the layer's borrowed record.
///
/// **The label is the handle.** Every account-taking entry point across the boundary names an
/// account by the label it was added under — sync, the queue, the write authorization — and
/// until the layer carried this list, nothing on this side knew what those labels were. The
/// runtime panel asked the user to type one, the sidebar showed a single hardcoded row, and an
/// account added through a sign-in could never be named again.
///
/// D-66 lends the strings for the duration of the call only, so this is the copy.
struct Account: Equatable {
    /// D-89's Sift-assigned identity. This is what a message-list observation anchors on;
    /// the zero identity is D-4's unified inbox rather than an account nobody has.
    let id: SiftId
    let name: String
    /// The reconnection route the container recorded. **Not something to branch on** — the
    /// provider model plans against declared capabilities, and this shell reads no meaning
    /// into it beyond telling the recorded corpus apart from a real mailbox.
    let kind: String
    let condition: SiftCondition
    /// Whether Sift may change this mailbox. An account is added watching and nothing else.
    let writesEnabled: Bool
    /// Intents recorded and held because writes are not authorized.
    let held: UInt32

    init(_ raw: SiftAccount) {
        id = raw.id
        name = SiftText.string(raw.name)
        kind = SiftText.string(raw.kind)
        condition = raw.condition
        writesEnabled = raw.writes_enabled != 0
        held = raw.held
    }

    /// Every account the container holds, in the order the layer returns them.
    static func all(app: OpaquePointer?) -> [Account] {
        guard let app else { return [] }
        var rows = SiftRows_SiftAccount()
        guard sift_accounts(UnsafeMutablePointer(app), &rows) == Ok, let ptr = rows.ptr else {
            return []
        }
        return (0..<rows.len).map { Account(ptr[$0]) }
    }

    static func == (a: Account, b: Account) -> Bool {
        a.name == b.name && a.writesEnabled == b.writesEnabled && a.held == b.held
            && a.condition == b.condition
    }
}
