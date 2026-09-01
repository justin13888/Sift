import Foundation
import CSift

/// One list row, copied out of the layer's borrowed record.
///
/// D-66 lends every string for the duration of the delivery only: *"a shell that needs a
/// value beyond the callback copies it."* This is that copy, and it happens once per row per
/// delivery rather than per field per draw — which is the arithmetic D-66 used to reject an
/// opaque row handle with per-field accessors, at FR-6's ten fields against NFR-6's
/// 10,000-row fling.
struct MessageRow: Equatable {
    let id: SiftId
    let account: SiftId
    /// D-55's sort key. Server-assigned, and what the list is ordered by.
    let receivedMillis: UInt64
    /// The sender's `Date` header. **Displayed only** — it is the sender's claim.
    let originationMillis: UInt64
    let sender: String
    let subject: String
    let snippet: String
    let unread: Bool
    let flagged: Bool
    let hasAttachments: Bool
    /// D-4: marked rather than joined.
    let duplicateAcrossAccounts: Bool
    let threadCount: UInt32

    init(_ raw: SiftMessageRow) {
        id = raw.id
        account = raw.account
        receivedMillis = raw.received_millis
        originationMillis = raw.origination_millis
        sender = MessageRow.string(raw.sender)
        subject = MessageRow.string(raw.subject)
        snippet = MessageRow.string(raw.snippet)
        unread = raw.unread != 0
        flagged = raw.flagged != 0
        hasAttachments = raw.has_attachments != 0
        duplicateAcrossAccounts = raw.duplicate_across_accounts != 0
        threadCount = raw.thread_count
    }

    /// A borrowed UTF-8 pointer-and-length, copied.
    ///
    /// **Never NUL-terminated**, so the length is authoritative and `String(cString:)` would
    /// read past the end of the value into whatever the layer put next to it.
    private static func string(_ s: SiftStr) -> String {
        guard let ptr = s.ptr, s.len > 0 else { return "" }
        return String(decoding: UnsafeBufferPointer(start: ptr, count: s.len), as: UTF8.self)
    }

    static func == (a: MessageRow, b: MessageRow) -> Bool {
        withUnsafeBytes(of: a.id.bytes) { l in
            withUnsafeBytes(of: b.id.bytes) { r in l.elementsEqual(r) }
        }
    }
}

extension SiftId {
    /// A stable key for a dictionary or a selection set.
    var key: String {
        withUnsafeBytes(of: bytes) { raw in
            raw.map { String(format: "%02x", $0) }.joined()
        }
    }

    static var zero: SiftId {
        SiftId(bytes: (0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0))
    }
}
