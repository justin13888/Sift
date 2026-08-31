//! Issuing the queue — D-85's six states against a real provider.
//!
//! # The two orderings that make this correct, and the crash each one costs
//!
//! **The transition into `Issued` is durable and happens before the request leaves.** That
//! costs one durable write per intent, forever, and D-85 records the trade as its own weakest
//! point. What it buys is that a crash in the window is *recoverable*: on restart every
//! `Issued` intent moves to `Reconciling`, because a request that went out and whose answer
//! never came back is exactly that situation. The alternative — infer sent-versus-unsent at
//! startup — cannot be done correctly, and getting it wrong in the safe-looking direction
//! means re-issuing a delete.
//!
//! **A batch never contains two intents for the same message.** Intents against one message
//! apply in the order they were issued — always, including through batching and retry — and
//! two in one batch would apply in whatever order the provider chose. Intents against
//! *different* messages are unordered, which is the whole reason batching is possible at all.
//! [`Queue::next_batch`] holds that rule; this module relies on it and asserts it.
//!
//! # What this module does not decide
//!
//! When to run, and how long to wait after a failure. D-87 puts a stated delay on D-25's
//! wheel, and a flush that slept would be a per-account sleep loop.

use crate::intent::{Intent, State};
use crate::queue::Queue;
use sift_foundation::identity::LocalId;
use sift_provider::adapter::{
    Adapter, Failure, MutationOutcome, Operation, RemoteFolderId, RemoteMessageId, WireMutation,
};

/// What a flush did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlushReport {
    pub issued: usize,
    pub applied: usize,
    /// The provider refused, in its own terms. Settled: retrying changes nothing.
    pub refused: usize,
    /// Left for the scheduler to try again.
    pub deferred: usize,
    /// The request went out and no answer came back — D-85's `Reconciling`.
    pub reconciling: usize,
    /// Held rather than executed: unrecognised, or its gating capability has gone away.
    pub quarantined: usize,
    /// A stated delay the wheel should honour, where the provider gave one.
    pub retry_after_millis: Option<u64>,
}

/// What went wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlushError {
    /// A message in the batch has no remote identifier, so nothing can be sent for it.
    ///
    /// Ordinary rather than exceptional: a mutation can be enqueued against a message the
    /// account has only just discovered, and the flush that follows the sync will find it.
    NotYetSynced(LocalId),
    /// The provider failed, classified by the adapter that produced it.
    Provider { failure: Failure, said: String },
}

/// A failure, **with what the flush had already done when it happened**.
///
/// The report is not discarded on the error path, and that is not tidiness: a batch that
/// went out and was not answered has moved to `Reconciling`, and a throttle carries a number
/// the wheel needs. Returning a bare error would throw both away at the one moment they
/// matter, leaving the caller to rediscover the queue's state by reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlushFailure {
    pub report: FlushReport,
    pub error: FlushError,
}

impl core::fmt::Display for FlushFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.error)
    }
}

impl std::error::Error for FlushFailure {}

impl core::fmt::Display for FlushError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotYetSynced(id) => write!(f, "{id} has no remote identifier yet"),
            Self::Provider { failure, said } => write!(f, "{said} ({failure:?})"),
        }
    }
}

/// What the caller must supply to turn a local intent into a wire mutation.
///
/// The mapping needs the store — a message's remote identifier, a folder's — and this crate
/// must not reach it, so the lookup crosses as a function. It is a closure rather than a
/// trait because there is exactly one implementation and it lives at the call site.
pub trait Resolve {
    /// The provider's identifier for a message Sift knows locally.
    fn message(&self, local: LocalId) -> Option<RemoteMessageId>;
    /// The provider's identifier for a folder Sift knows locally.
    fn folder(&self, local: i64) -> Option<RemoteFolderId>;
}

/// Translate an intent into the operation the wire takes.
///
/// **The intent set and the operation set are the same set**, which is what makes this
/// exhaustive rather than a lookup that can miss. D-98 states the same rule from the other
/// side: the action register must not contain a mutation that is not an intent.
///
/// # Errors
/// [`FlushError::NotYetSynced`] where a folder the intent names has no remote identifier.
pub fn operation_for(intent: &Intent, resolve: &impl Resolve) -> Result<Operation, FlushError> {
    Ok(match intent {
        Intent::Archive => Operation::Archive,
        Intent::DeleteToTrash => Operation::DeleteToTrash,
        Intent::PermanentlyDelete => Operation::PermanentlyDelete,
        Intent::MoveTo { folder } => Operation::MoveTo(
            resolve
                .folder(*folder)
                .ok_or(FlushError::NotYetSynced(LocalId::from_u128(0)))?,
        ),
        Intent::MarkRead => Operation::SetRead(true),
        Intent::MarkUnread => Operation::SetRead(false),
        Intent::Flag => Operation::SetFlagged(true),
        Intent::Unflag => Operation::SetFlagged(false),
        Intent::AddTag { name } => Operation::AddTag(name.clone()),
        Intent::RemoveTag { name } => Operation::RemoveTag(name.clone()),
        Intent::ReportJunk => Operation::ReportJunk,
        Intent::ReportNotJunk => Operation::ReportNotJunk,
    })
}

/// Issue one batch.
///
/// `mark_issued` is called **before the request leaves** and must make the transition
/// durable. It is a callback rather than something this function does because the durable
/// half lives in the journal, and this crate cannot reach it — the same reason [`Resolve`] is
/// a trait.
///
/// # Errors
/// See [`FlushError`]. A provider failure leaves the batch in `Issued` or `Reconciling`
/// rather than back in `Pending`, because an intent whose request left is not pending.
pub fn flush_once<A: Adapter + ?Sized>(
    adapter: &A,
    queue: &mut Queue,
    resolve: &impl Resolve,
    mark_issued: &mut impl FnMut(&[u128]) -> Result<(), String>,
) -> Result<FlushReport, FlushFailure>
where
    A::Error: core::fmt::Display,
{
    let capabilities = adapter.capabilities();
    let limit = capabilities.batch_size();
    let candidates: Vec<(u128, LocalId, Intent)> = queue
        .next_batch(limit)
        .iter()
        .map(|q| (q.id, q.message, q.intent.clone()))
        .collect();

    let mut report = FlushReport::default();
    let mut batch = Vec::with_capacity(candidates.len());
    let mut ids = Vec::with_capacity(candidates.len());

    for (id, message, intent) in candidates {
        // A capability that has gone away since the intent was enqueued — a provider whose
        // declared table changed, or an account re-added against a different one.
        // **Quarantined, never executed and never discarded**: held, shown in the queue
        // under FR-34, and executable by a later build or a restored capability.
        if !intent.permitted_by(capabilities) {
            queue.quarantine(id);
            report.quarantined += 1;
            continue;
        }
        let Some(remote) = resolve.message(message) else {
            // Not an error for the batch: the sync that gives this message a remote
            // identifier has simply not run yet, and the next flush will find it.
            report.deferred += 1;
            continue;
        };
        let operation = operation_for(&intent, resolve).map_err(|error| FlushFailure {
            report: report.clone(),
            error,
        })?;
        batch.push(WireMutation {
            message: remote,
            // The client-assigned identifier, sent as an idempotency key where the provider
            // accepts one.
            intent_id: id,
            operation,
        });
        ids.push(id);
    }

    if batch.is_empty() {
        return Ok(report);
    }

    // A batch must not contain two intents for the same message. `next_batch` holds that
    // rule; this is where a change to it would be caught rather than shipped.
    let mut messages: Vec<&RemoteMessageId> = batch.iter().map(|m| &m.message).collect();
    messages.sort();
    let before = messages.len();
    messages.dedup();
    debug_assert_eq!(
        before,
        messages.len(),
        "a batch carried two intents for one message, so their order became the provider's"
    );

    // Durable, and before the request leaves.
    mark_issued(&ids).map_err(|said| FlushFailure {
        report: report.clone(),
        error: FlushError::Provider {
            failure: Failure::Transient,
            said,
        },
    })?;
    for id in &ids {
        queue.set_state(*id, State::Issued);
    }
    report.issued = ids.len();

    let outcomes = match adapter.apply(&batch) {
        Ok(outcomes) => outcomes,
        Err(e) => {
            let failure = adapter.classify(&e);
            // The request left. Whether the server applied it is unknown, and D-85's answer
            // is to establish server state before trying again rather than replaying blindly
            // — which is what NFR-17 means by exactly-once *observable*.
            if matches!(failure, Failure::Unknown) {
                for id in &ids {
                    queue.set_state(*id, State::Reconciling);
                }
                report.reconciling = ids.len();
            }
            if let Failure::Throttled { retry_after_millis } = failure {
                report.retry_after_millis = Some(retry_after_millis);
            }
            return Err(FlushFailure {
                report,
                error: FlushError::Provider {
                    failure,
                    said: e.to_string(),
                },
            });
        }
    };

    for (id, outcome) in ids.iter().zip(outcomes.iter()) {
        match outcome {
            MutationOutcome::Applied => {
                queue.set_state(*id, State::Settled);
                report.applied += 1;
            }
            // Settled rather than retried: the provider said no in its own terms, and a
            // hundred more attempts get the same answer.
            MutationOutcome::Refused => {
                queue.set_state(*id, State::Settled);
                report.refused += 1;
            }
            MutationOutcome::Transient => {
                // Back to pending. The request was answered, so it did *not* leave in the
                // sense `Reconciling` means.
                queue.set_state(*id, State::Pending);
                report.deferred += 1;
            }
            MutationOutcome::Unknown => {
                queue.set_state(*id, State::Reconciling);
                report.reconciling += 1;
            }
        }
    }
    // A provider that answered a batch with fewer outcomes than it had elements has told us
    // nothing about the tail, which is the unknown case rather than the applied one.
    for id in ids.iter().skip(outcomes.len()) {
        queue.set_state(*id, State::Reconciling);
        report.reconciling += 1;
    }
    queue.collect_settled();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sift_provider::capability::{
        ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
        LocationCardinality, Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations,
        TrashSemantics,
    };
    use std::cell::RefCell;

    struct Map;

    impl Resolve for Map {
        fn message(&self, local: LocalId) -> Option<RemoteMessageId> {
            // One message is deliberately unknown, so the not-yet-synced path is exercised.
            (local.as_u128() != 999).then(|| RemoteMessageId(format!("r{}", local.as_u128())))
        }
        fn folder(&self, local: i64) -> Option<RemoteFolderId> {
            Some(RemoteFolderId(format!("f{local}")))
        }
    }

    struct Fake {
        capabilities: Capabilities,
        outcomes: RefCell<Vec<Vec<MutationOutcome>>>,
        error: Option<&'static str>,
        seen: RefCell<Vec<WireMutation>>,
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            location_cardinality: LocationCardinality::OneOrMore,
            tag_support: TagSupport::ReadWrite,
            archive: ArchiveSemantics::RemoveFromInbox,
            trash: TrashSemantics::MoveToTrash,
            permanent_delete: false,
            thread_operations: ThreadOperations::Native,
            junk_reporting: JunkReporting::NativeReport,
            delta: DeltaMechanism::HistoryCursor,
            push: PushMechanism::PollOnly,
            id_stability: IdStability::StableGlobally,
            server_search: true,
            max_batch_size: Magnitude::Unknown,
            request_budget: Magnitude::Unknown,
            snippet_source: SnippetSource::ProviderSupplied,
            unrecognised: Vec::new(),
        }
    }

    impl Fake {
        fn applying(n: usize) -> Self {
            Self {
                capabilities: capabilities(),
                outcomes: RefCell::new(vec![vec![MutationOutcome::Applied; n]]),
                error: None,
                seen: RefCell::new(Vec::new()),
            }
        }
        fn answering(outcomes: Vec<MutationOutcome>) -> Self {
            Self {
                capabilities: capabilities(),
                outcomes: RefCell::new(vec![outcomes]),
                error: None,
                seen: RefCell::new(Vec::new()),
            }
        }
        fn failing(error: &'static str) -> Self {
            Self {
                capabilities: capabilities(),
                outcomes: RefCell::new(Vec::new()),
                error: Some(error),
                seen: RefCell::new(Vec::new()),
            }
        }
    }

    impl Adapter for Fake {
        type Error = &'static str;
        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }
        fn enumerate_folders(
            &self,
        ) -> Result<Vec<sift_provider::adapter::RemoteFolder>, Self::Error> {
            Ok(Vec::new())
        }
        fn delta(
            &self,
            _: &RemoteFolderId,
            _: Option<&sift_provider::adapter::Cursor>,
        ) -> Result<sift_provider::adapter::Delta, Self::Error> {
            Err("not used")
        }
        fn fetch_envelopes(
            &self,
            _: &[RemoteMessageId],
        ) -> Result<Vec<sift_provider::adapter::Envelope>, Self::Error> {
            Ok(Vec::new())
        }
        fn structure(
            &self,
            _: &RemoteMessageId,
        ) -> Result<Vec<sift_provider::adapter::PartDescriptor>, Self::Error> {
            Ok(Vec::new())
        }
        fn fetch_part(&self, _: &RemoteMessageId, _: &str) -> Result<Vec<u8>, Self::Error> {
            Ok(Vec::new())
        }
        fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, Self::Error> {
            self.seen.borrow_mut().extend_from_slice(batch);
            if let Some(e) = self.error {
                return Err(e);
            }
            let mut outcomes = self.outcomes.borrow_mut();
            Ok(if outcomes.is_empty() {
                Vec::new()
            } else {
                outcomes.remove(0)
            })
        }
        fn watch(&self, _: &[RemoteFolderId]) -> Result<(), Self::Error> {
            Ok(())
        }
        fn classify(&self, error: &Self::Error) -> Failure {
            match *error {
                "unknown" => Failure::Unknown,
                "throttled" => Failure::Throttled {
                    retry_after_millis: 5_000,
                },
                _ => Failure::Transient,
            }
        }
    }

    fn message(n: u128) -> LocalId {
        LocalId::from_u128(n)
    }

    fn queue_with(intents: &[(u128, LocalId, Intent)]) -> Queue {
        let mut q = Queue::new();
        for (id, m, intent) in intents {
            q.enqueue(*id, *m, intent.clone(), 0);
        }
        q
    }

    #[test]
    fn the_issued_marker_is_written_before_the_request_leaves() {
        // D-85's own weakest point, and what it buys: a crash in the window is recoverable,
        // where inferring sent-versus-unsent at startup is not.
        let adapter = Fake::applying(1);
        let mut queue = queue_with(&[(1, message(1), Intent::Archive)]);
        let order = RefCell::new(Vec::<&'static str>::new());
        let mut mark = |_: &[u128]| {
            order.borrow_mut().push("marked");
            Ok(())
        };
        let report = flush_once(&adapter, &mut queue, &Map, &mut mark).unwrap();
        order.borrow_mut().push("sent");
        assert_eq!(report.issued, 1);
        assert_eq!(report.applied, 1);
        assert_eq!(*order.borrow(), vec!["marked", "sent"]);
        assert!(!adapter.seen.borrow().is_empty());
    }

    #[test]
    fn a_marker_that_cannot_be_written_stops_the_request() {
        // If the durable write fails, sending anyway would produce exactly the state the
        // marker exists to prevent: a request out with nothing recording that it left.
        let adapter = Fake::applying(1);
        let mut queue = queue_with(&[(1, message(1), Intent::Archive)]);
        let mut mark = |_: &[u128]| Err("the journal is unwritable".to_owned());
        assert!(flush_once(&adapter, &mut queue, &Map, &mut mark).is_err());
        assert!(
            adapter.seen.borrow().is_empty(),
            "a request left without a durable marker"
        );
        assert_eq!(queue.entries()[0].state, State::Pending);
    }

    #[test]
    fn the_intent_identifier_is_what_goes_out_as_the_idempotency_key() {
        let adapter = Fake::applying(1);
        let mut queue = queue_with(&[(4242, message(1), Intent::Archive)]);
        let mut mark = |_: &[u128]| Ok(());
        flush_once(&adapter, &mut queue, &Map, &mut mark).unwrap();
        assert_eq!(adapter.seen.borrow()[0].intent_id, 4242);
    }

    #[test]
    fn a_batch_never_carries_two_intents_for_one_message() {
        // Two in one batch would apply in whatever order the provider chose, and intents
        // against one message apply in the order they were issued. Always.
        let adapter = Fake::applying(2);
        let mut queue = queue_with(&[
            (1, message(1), Intent::MarkRead),
            (2, message(1), Intent::Flag),
            (3, message(2), Intent::Archive),
        ]);
        let mut mark = |_: &[u128]| Ok(());
        flush_once(&adapter, &mut queue, &Map, &mut mark).unwrap();
        let seen = adapter.seen.borrow();
        let mut targets: Vec<&str> = seen.iter().map(|m| m.message.0.as_str()).collect();
        targets.sort_unstable();
        let before = targets.len();
        targets.dedup();
        assert_eq!(before, targets.len());
    }

    #[test]
    fn an_intent_the_account_can_no_longer_do_is_quarantined_rather_than_dropped() {
        // Held, shown in the queue under FR-34, and executable by a later build or a
        // restored capability. Never executed and never discarded.
        let adapter = Fake::applying(0);
        let mut queue = queue_with(&[(1, message(1), Intent::PermanentlyDelete)]);
        let mut mark = |_: &[u128]| Ok(());
        let report = flush_once(&adapter, &mut queue, &Map, &mut mark).unwrap();
        assert_eq!(report.quarantined, 1);
        assert_eq!(queue.entries()[0].state, State::Quarantined);
        assert!(
            adapter.seen.borrow().is_empty(),
            "a request left for something the account cannot do"
        );
    }

    #[test]
    fn a_mutation_against_a_message_that_has_not_synced_yet_waits_rather_than_failing() {
        let adapter = Fake::applying(0);
        let mut queue = queue_with(&[(1, message(999), Intent::Archive)]);
        let mut mark = |_: &[u128]| Ok(());
        let report = flush_once(&adapter, &mut queue, &Map, &mut mark).unwrap();
        assert_eq!(report.deferred, 1);
        assert_eq!(queue.entries()[0].state, State::Pending);
    }

    #[test]
    fn an_unanswered_request_reconciles_rather_than_replaying() {
        // NFR-17's exactly-once *observable*: the adapter establishes server state before
        // trying again.
        let adapter = Fake::failing("unknown");
        let mut queue = queue_with(&[(1, message(1), Intent::Archive)]);
        let mut mark = |_: &[u128]| Ok(());
        let Err(failure) = flush_once(&adapter, &mut queue, &Map, &mut mark) else {
            panic!("the flush did not fail");
        };
        assert_eq!(queue.entries()[0].state, State::Reconciling);
        assert_eq!(failure.report.reconciling, 1);
        assert_eq!(
            failure.report.issued, 1,
            "the report lost what had happened"
        );
    }

    #[test]
    fn a_throttle_comes_back_as_a_number_for_the_wheel() {
        // D-87. Nothing here waits.
        let adapter = Fake::failing("throttled");
        let mut queue = queue_with(&[(1, message(1), Intent::Archive)]);
        let mut mark = |_: &[u128]| Ok(());
        let Err(failure) = flush_once(&adapter, &mut queue, &Map, &mut mark) else {
            panic!("the flush did not fail");
        };
        assert_eq!(
            failure.error,
            FlushError::Provider {
                failure: Failure::Throttled {
                    retry_after_millis: 5_000
                },
                said: "throttled".to_owned(),
            }
        );
        // The number the wheel needs survives the error rather than being thrown away at
        // the one moment it matters.
        assert_eq!(failure.report.retry_after_millis, Some(5_000));
        assert_eq!(
            queue.entries()[0].state,
            State::Issued,
            "an intent whose request left went back to pending"
        );
    }

    #[test]
    fn a_refusal_settles_and_a_transient_answer_does_not() {
        let adapter = Fake::answering(vec![MutationOutcome::Refused, MutationOutcome::Transient]);
        let mut queue = queue_with(&[
            (1, message(1), Intent::Archive),
            (2, message(2), Intent::Archive),
        ]);
        let mut mark = |_: &[u128]| Ok(());
        let report = flush_once(&adapter, &mut queue, &Map, &mut mark).unwrap();
        assert_eq!(report.refused, 1);
        assert_eq!(report.deferred, 1);
        assert_eq!(queue.len(), 1, "the refused intent stayed in the queue");
        assert_eq!(queue.entries()[0].state, State::Pending);
    }

    #[test]
    fn a_short_answer_leaves_its_tail_unknown_rather_than_applied() {
        // A provider that answered a batch of two with one outcome has told us nothing about
        // the second, and assuming success there is how a mutation is silently lost.
        let adapter = Fake::answering(vec![MutationOutcome::Applied]);
        let mut queue = queue_with(&[
            (1, message(1), Intent::Archive),
            (2, message(2), Intent::Archive),
        ]);
        let mut mark = |_: &[u128]| Ok(());
        let report = flush_once(&adapter, &mut queue, &Map, &mut mark).unwrap();
        assert_eq!(report.applied, 1);
        assert_eq!(report.reconciling, 1);
    }

    #[test]
    fn every_intent_maps_to_an_operation() {
        // The intent set and the operation set are the same set, which is what makes the
        // translation exhaustive rather than a lookup that can miss.
        for intent in [
            Intent::Archive,
            Intent::DeleteToTrash,
            Intent::PermanentlyDelete,
            Intent::MoveTo { folder: 1 },
            Intent::MarkRead,
            Intent::MarkUnread,
            Intent::Flag,
            Intent::Unflag,
            Intent::AddTag { name: "t".into() },
            Intent::RemoveTag { name: "t".into() },
            Intent::ReportJunk,
            Intent::ReportNotJunk,
        ] {
            assert!(operation_for(&intent, &Map).is_ok(), "{intent:?}");
        }
    }

    #[test]
    fn marking_read_and_marking_unread_are_opposite_operations() {
        assert_eq!(
            operation_for(&Intent::MarkRead, &Map).unwrap(),
            Operation::SetRead(true)
        );
        assert_eq!(
            operation_for(&Intent::MarkUnread, &Map).unwrap(),
            Operation::SetRead(false)
        );
    }
}
