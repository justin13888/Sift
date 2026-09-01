//! FR-6's list row, read out of the store and **through** the overlay.
//!
//! D-51 is the rule that makes this more than a query: everything the user sees reads through
//! the pending overlay. A read path that forgets shows the server's opinion instead of the
//! user's — a message the user archived a moment ago reappears, which is the single most
//! visible way an optimistic client can betray the person using it.
//!
//! The ordering is D-55's: the **server-assigned received time**, newest first, with identity
//! breaking ties. Not the `Date` header, which is the sender's claim and is carried beside it
//! for display only.

use sift_foundation::identity::{AccountId, LocalId};
use sift_mutations::queue::Overlay;

use crate::OpenAccount;

/// One row of the message list.
///
/// The fields are FR-6's, and they travel together as one record rather than being fetched
/// per field: D-66 excluded the per-field accessor arithmetically, at ten fields against
/// NFR-6's 10,000-row fling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub id: LocalId,
    pub account: AccountId,
    /// D-55's sort key. Server-assigned.
    pub received_millis: u64,
    /// The sender's `Date` header. **Displayed only.**
    pub origination_millis: u64,
    pub sender: String,
    pub subject: String,
    pub snippet: String,
    pub unread: bool,
    pub flagged: bool,
    pub has_attachments: bool,
    /// D-44's corroborating digest, used at display time to mark cross-account duplicates.
    pub digest: u64,
    /// How many messages the thread holds. One for a message that is its own thread.
    pub thread_count: u32,
    /// The intents standing against this row that have not settled — D-51's overlay, named
    /// so a shell can say *why* a row looks the way it does.
    pub pending: Vec<&'static str>,
}

/// Read one account's rows, in D-55's order, through the overlay.
///
/// A row every pending intent removes from view is **absent**, not marked: that is NFR-7's
/// optimistic feedback, and it happens here rather than in a shell so that both shells cannot
/// disagree about it.
///
/// # Errors
/// The store could not be read.
pub fn message_rows(account: &OpenAccount, limit: u32) -> Result<Vec<MessageRow>, String> {
    let overlay: Overlay = account.queue.overlay();
    let mut stmt = account
        .store
        .store
        .prepare(
            "SELECT m.id, m.received_at_millis, m.origination_millis, m.sender, m.subject,
                    m.snippet, m.flags, m.has_attachments, m.fallback_digest,
                    (SELECT COUNT(*) FROM message t WHERE t.thread_id = m.thread_id)
             FROM message m
             ORDER BY m.received_at_millis DESC, m.id DESC
             LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;

    let raw = stmt
        .query_map([i64::from(limit)], |r| {
            let key: Vec<u8> = r.get(0)?;
            let received: i64 = r.get(1)?;
            let origination: Option<i64> = r.get(2)?;
            let sender: String = r.get(3)?;
            let subject: Option<String> = r.get(4)?;
            let snippet: Option<String> = r.get(5)?;
            let flags: i64 = r.get(6)?;
            let attachments: i64 = r.get(7)?;
            let digest: Vec<u8> = r.get(8)?;
            let thread_count: i64 = r.get(9)?;
            Ok((
                key,
                received,
                origination,
                sender,
                subject,
                snippet,
                flags,
                attachments,
                digest,
                thread_count,
            ))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in raw {
        let (
            key,
            received,
            origination,
            sender,
            subject,
            snippet,
            flags,
            attachments,
            digest,
            threads,
        ) = row.map_err(|e| e.to_string())?;
        let bytes: [u8; 16] = key.try_into().map_err(|_| "identity is not 16 bytes")?;
        let id = LocalId::from_bytes(bytes);

        // D-51. The overlay is consulted before anything is formatted, because a row it
        // removes is not a row a shell should be told about at all.
        let pending = overlay.for_message(id);
        if pending
            .iter()
            .any(sift_mutations::intent::Intent::removes_from_view)
        {
            continue;
        }

        let base_unread = !sift_store::flags::is_read(flags);
        let base_flagged = sift_store::flags::is_flagged(flags);
        out.push(MessageRow {
            id,
            account: account.id,
            received_millis: u64::try_from(received).unwrap_or(0),
            origination_millis: origination.and_then(|o| u64::try_from(o).ok()).unwrap_or(0),
            sender,
            subject: subject.unwrap_or_default(),
            snippet: snippet.unwrap_or_default(),
            // The overlay decides read and flagged state where it holds an opinion, because
            // the base row is the server's and the overlay is the user's.
            unread: overlay_read_state(pending).unwrap_or(base_unread),
            flagged: overlay_flagged(pending).unwrap_or(base_flagged),
            has_attachments: attachments != 0,
            digest: digest_of(&digest),
            thread_count: u32::try_from(threads).unwrap_or(1),
            pending: pending
                .iter()
                .map(sift_mutations::intent::Intent::name)
                .collect(),
        });
    }
    Ok(out)
}

/// The unread state a pending intent asserts, where one does.
///
/// The **last** such intent wins, because that is the order they will reach the server in and
/// the overlay's whole job is to show what the user will end up with.
fn overlay_read_state(pending: &[sift_mutations::intent::Intent]) -> Option<bool> {
    use sift_mutations::intent::Intent;
    pending.iter().rev().find_map(|i| match i {
        Intent::MarkRead => Some(false),
        Intent::MarkUnread => Some(true),
        _ => None,
    })
}

/// The flag state a pending intent asserts, where one does.
fn overlay_flagged(pending: &[sift_mutations::intent::Intent]) -> Option<bool> {
    use sift_mutations::intent::Intent;
    pending.iter().rev().find_map(|i| match i {
        Intent::Flag => Some(true),
        Intent::Unflag => Some(false),
        _ => None,
    })
}

/// The first eight bytes of D-44's digest, which is what a display-time comparison needs.
fn digest_of(bytes: &[u8]) -> u64 {
    let mut head = [0u8; 8];
    let take = bytes.len().min(8);
    head[..take].copy_from_slice(&bytes[..take]);
    u64::from_be_bytes(head)
}
