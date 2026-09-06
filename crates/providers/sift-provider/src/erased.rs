//! The adapter contract with its error type erased, so that nothing above the adapters has
//! to name one.
//!
//! [`Adapter`](crate::adapter::Adapter) is already object-safe — `classify` takes `&self`
//! for exactly that reason — but it carries an associated `Error`, and a
//! `Box<dyn Adapter<Error = E>>` still forces its holder to name `E`. Naming `E` means
//! naming the provider that defines it, which D-12 forbids in every layer above the
//! adapters and `cargo xtask invariants` enforces in `application`, `presentation` and
//! `abi`.
//!
//! That is why the assembled application could only ever live in a shell: the harness holds
//! `Box<dyn Adapter<Error = sift_gmail::GmailError>>`, and a shell is the one layer the rule
//! does not reach. Erasing the error is what lets the assembly move below the boundary.
//!
//! **The classification happens at the erasure point**, which is the last place the concrete
//! error is still in hand. D-87's split survives untouched: the adapter still says *what
//! kind* of failure it was, and the scheduler still says when to try again.

use core::fmt;

use crate::adapter::{
    Adapter, Cursor, Delta, Envelope, Failure, MutationOutcome, PartDescriptor, RemoteFolder,
    RemoteFolderId, RemoteMessageId, WireMutation,
};
use crate::capability::Capabilities;

/// A failure from an adapter whose concrete error type is no longer visible.
///
/// It carries the classification rather than the error, because the classification is the
/// only thing any caller above the adapters was ever allowed to branch on.
///
/// `detail` is a **content value** under D-68 — it may quote what the provider said, and
/// what the provider said is attacker-influenced. It is never Sift's own prose, and nothing
/// may parse it to recover the kind: that is what `failure` is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub failure: Failure,
    pub detail: String,
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The kind, then what the provider said. A reader needs both and only the first is
        // Sift's.
        write!(f, "{:?}: {}", self.failure, self.detail)
    }
}

impl core::error::Error for ProviderError {}

/// The six responsibilities, with the error type erased.
///
/// Deliberately **not** `classify`: there is nothing left to classify, because every method
/// already returns a [`ProviderError`] that carries its own classification.
pub trait ErasedAdapter {
    fn capabilities(&self) -> &Capabilities;
    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, ProviderError>;
    fn delta(
        &self,
        folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, ProviderError>;
    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, ProviderError>;
    fn structure(&self, id: &RemoteMessageId) -> Result<Vec<PartDescriptor>, ProviderError>;
    fn fetch_part(&self, id: &RemoteMessageId, part: &str) -> Result<Vec<u8>, ProviderError>;
    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, ProviderError>;
    fn watch(&self, folders: &[RemoteFolderId]) -> Result<(), ProviderError>;

    /// NFR-23's one path by which a secret reaches an adapter. Not fallible, so not erased.
    fn present_credential(&self, secret: &str);

    /// FR-36's bytes on the wire. Not fallible, so not erased.
    fn wire_bytes(&self) -> (u64, u64);
}

/// Every adapter is an erased adapter, and the erasure is where `classify` is applied.
impl<A> ErasedAdapter for A
where
    A: Adapter,
    A::Error: fmt::Display,
{
    fn capabilities(&self) -> &Capabilities {
        Adapter::capabilities(self)
    }

    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, ProviderError> {
        Adapter::enumerate_folders(self).map_err(|e| self.erase(&e))
    }

    fn delta(
        &self,
        folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, ProviderError> {
        Adapter::delta(self, folder, cursor).map_err(|e| self.erase(&e))
    }

    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, ProviderError> {
        Adapter::fetch_envelopes(self, ids).map_err(|e| self.erase(&e))
    }

    fn structure(&self, id: &RemoteMessageId) -> Result<Vec<PartDescriptor>, ProviderError> {
        Adapter::structure(self, id).map_err(|e| self.erase(&e))
    }

    fn fetch_part(&self, id: &RemoteMessageId, part: &str) -> Result<Vec<u8>, ProviderError> {
        Adapter::fetch_part(self, id, part).map_err(|e| self.erase(&e))
    }

    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, ProviderError> {
        Adapter::apply(self, batch).map_err(|e| self.erase(&e))
    }

    fn watch(&self, folders: &[RemoteFolderId]) -> Result<(), ProviderError> {
        Adapter::watch(self, folders).map_err(|e| self.erase(&e))
    }

    fn present_credential(&self, secret: &str) {
        Adapter::present_credential(self, secret);
    }

    fn wire_bytes(&self) -> (u64, u64) {
        Adapter::wire_bytes(self)
    }
}

/// The erasure itself, kept in one place so no method can forget to classify.
trait Erase: Adapter {
    fn erase(&self, error: &Self::Error) -> ProviderError;
}

impl<A> Erase for A
where
    A: Adapter,
    A::Error: fmt::Display,
{
    fn erase(&self, error: &Self::Error) -> ProviderError {
        ProviderError {
            failure: self.classify(error),
            detail: error.to_string(),
        }
    }
}

/// The erased trait object is itself an adapter, so every generic caller keeps working.
///
/// This is what lets `sift_sync::run` and `sift_mutations::flush` stay generic over
/// [`Adapter`] while the application holds an adapter whose provider it cannot name.
/// `classify` is total and allocation-free here because the classification was already made,
/// once, where the concrete error still existed.
impl Adapter for dyn ErasedAdapter + '_ {
    type Error = ProviderError;

    fn capabilities(&self) -> &Capabilities {
        ErasedAdapter::capabilities(self)
    }

    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, Self::Error> {
        ErasedAdapter::enumerate_folders(self)
    }

    fn delta(
        &self,
        folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, Self::Error> {
        ErasedAdapter::delta(self, folder, cursor)
    }

    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, Self::Error> {
        ErasedAdapter::fetch_envelopes(self, ids)
    }

    fn structure(&self, id: &RemoteMessageId) -> Result<Vec<PartDescriptor>, Self::Error> {
        ErasedAdapter::structure(self, id)
    }

    fn fetch_part(&self, id: &RemoteMessageId, part: &str) -> Result<Vec<u8>, Self::Error> {
        ErasedAdapter::fetch_part(self, id, part)
    }

    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, Self::Error> {
        ErasedAdapter::apply(self, batch)
    }

    fn watch(&self, folders: &[RemoteFolderId]) -> Result<(), Self::Error> {
        ErasedAdapter::watch(self, folders)
    }

    fn present_credential(&self, secret: &str) {
        ErasedAdapter::present_credential(self, secret);
    }

    fn wire_bytes(&self) -> (u64, u64) {
        ErasedAdapter::wire_bytes(self)
    }

    /// The identity over what the error already carries.
    ///
    /// Answering anything else here would discard the adapter's own verdict — a permanent
    /// failure retried until L-17 expired it, or a cursor invalidation degrading an account
    /// that only needed to recover.
    fn classify(&self, error: &Self::Error) -> Failure {
        error.failure
    }
}

/// A box holding an adapter is an adapter, whether or not what it holds is sized.
///
/// Written once, generically, rather than for `Box<dyn ErasedAdapter>` alone: the callers
/// that hold an owned adapter and the callers that borrow one should not need to know which
/// they have.
impl<A: Adapter + ?Sized> Adapter for Box<A> {
    type Error = A::Error;

    fn capabilities(&self) -> &Capabilities {
        (**self).capabilities()
    }

    fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, Self::Error> {
        (**self).enumerate_folders()
    }

    fn delta(
        &self,
        folder: &RemoteFolderId,
        cursor: Option<&Cursor>,
    ) -> Result<Delta, Self::Error> {
        (**self).delta(folder, cursor)
    }

    fn fetch_envelopes(&self, ids: &[RemoteMessageId]) -> Result<Vec<Envelope>, Self::Error> {
        (**self).fetch_envelopes(ids)
    }

    fn structure(&self, id: &RemoteMessageId) -> Result<Vec<PartDescriptor>, Self::Error> {
        (**self).structure(id)
    }

    fn fetch_part(&self, id: &RemoteMessageId, part: &str) -> Result<Vec<u8>, Self::Error> {
        (**self).fetch_part(id, part)
    }

    fn apply(&self, batch: &[WireMutation]) -> Result<Vec<MutationOutcome>, Self::Error> {
        (**self).apply(batch)
    }

    fn watch(&self, folders: &[RemoteFolderId]) -> Result<(), Self::Error> {
        (**self).watch(folders)
    }

    fn present_credential(&self, secret: &str) {
        (**self).present_credential(secret);
    }

    fn wire_bytes(&self) -> (u64, u64) {
        (**self).wire_bytes()
    }

    fn classify(&self, error: &Self::Error) -> Failure {
        (**self).classify(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{
        ArchiveSemantics, Capabilities, DeltaMechanism, IdStability, JunkReporting,
        LocationCardinality, Magnitude, PushMechanism, SnippetSource, TagSupport, ThreadOperations,
        TrashSemantics,
    };
    use core::cell::Cell;

    /// A capability set is required to build an adapter and is irrelevant to every test
    /// here — the erasure never reads one.
    fn any_capabilities() -> Capabilities {
        Capabilities {
            location_cardinality: LocationCardinality::OneOrMore,
            tag_support: TagSupport::ReadWrite,
            archive: ArchiveSemantics::RemoveFromInbox,
            trash: TrashSemantics::MoveToTrash,
            permanent_delete: false,
            thread_operations: ThreadOperations::Native,
            junk_reporting: JunkReporting::NativeReport,
            delta: DeltaMechanism::ChangesQuery,
            push: PushMechanism::EventStream,
            id_stability: IdStability::StableGlobally,
            server_search: true,
            max_batch_size: Magnitude::Unknown,
            request_budget: Magnitude::Unknown,
            snippet_source: SnippetSource::ProviderSupplied,
            unrecognised: Vec::new(),
        }
    }

    /// An adapter whose every call fails, with a classification the test chooses.
    struct Failing {
        capabilities: Capabilities,
        verdict: Failure,
        classified: Cell<usize>,
    }

    #[derive(Debug)]
    struct Wire(&'static str);

    impl fmt::Display for Wire {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl Failing {
        fn new(verdict: Failure) -> Self {
            Self {
                capabilities: any_capabilities(),
                verdict,
                classified: Cell::new(0),
            }
        }
    }

    impl Adapter for Failing {
        type Error = Wire;

        fn capabilities(&self) -> &Capabilities {
            &self.capabilities
        }
        fn enumerate_folders(&self) -> Result<Vec<RemoteFolder>, Wire> {
            Err(Wire("the provider said no"))
        }
        fn delta(&self, _: &RemoteFolderId, _: Option<&Cursor>) -> Result<Delta, Wire> {
            Err(Wire("the provider said no"))
        }
        fn fetch_envelopes(&self, _: &[RemoteMessageId]) -> Result<Vec<Envelope>, Wire> {
            Err(Wire("the provider said no"))
        }
        fn structure(&self, _: &RemoteMessageId) -> Result<Vec<PartDescriptor>, Wire> {
            Err(Wire("the provider said no"))
        }
        fn fetch_part(&self, _: &RemoteMessageId, _: &str) -> Result<Vec<u8>, Wire> {
            Err(Wire("the provider said no"))
        }
        fn apply(&self, _: &[WireMutation]) -> Result<Vec<MutationOutcome>, Wire> {
            Err(Wire("the provider said no"))
        }
        fn watch(&self, _: &[RemoteFolderId]) -> Result<(), Wire> {
            Err(Wire("the provider said no"))
        }
        fn classify(&self, _: &Wire) -> Failure {
            self.classified.set(self.classified.get() + 1);
            self.verdict
        }
    }

    #[test]
    fn the_adapters_own_classification_is_what_crosses_the_erasure() {
        // D-87: the adapter says what kind. If the erasure invented a classification of its
        // own, a cursor invalidation would arrive above as a fault and NFR-18's silent
        // recovery would become a degraded account.
        let a = Failing::new(Failure::CursorInvalidated);
        let e = ErasedAdapter::enumerate_folders(&a).unwrap_err();
        assert_eq!(e.failure, Failure::CursorInvalidated);
        assert_eq!(
            a.classified.get(),
            1,
            "classify runs exactly once, at the erasure"
        );
    }

    #[test]
    fn every_method_classifies_rather_than_defaulting() {
        // A method that forgot to classify would answer `Transient` forever, which retries a
        // permanent failure until L-17 expires it. Assert each one individually.
        let a = Failing::new(Failure::Permanent);
        let folder = RemoteFolderId::default();
        let message = RemoteMessageId::default();

        let calls: Vec<Failure> = vec![
            ErasedAdapter::enumerate_folders(&a).unwrap_err().failure,
            ErasedAdapter::delta(&a, &folder, None).unwrap_err().failure,
            ErasedAdapter::fetch_envelopes(&a, &[]).unwrap_err().failure,
            ErasedAdapter::structure(&a, &message).unwrap_err().failure,
            ErasedAdapter::fetch_part(&a, &message, "1")
                .unwrap_err()
                .failure,
            ErasedAdapter::apply(&a, &[]).unwrap_err().failure,
            ErasedAdapter::watch(&a, &[]).unwrap_err().failure,
        ];

        assert_eq!(calls.len(), 7, "the six responsibilities, plus watch");
        assert!(calls.iter().all(|f| *f == Failure::Permanent));
        assert_eq!(a.classified.get(), 7);
    }

    #[test]
    fn what_the_provider_said_is_carried_but_is_not_the_classification() {
        // D-68: the detail is a content value. It may be quoted; it may not be parsed to
        // recover the kind, which is why both travel together rather than one being derived.
        let a = Failing::new(Failure::Throttled {
            retry_after_millis: 30_000,
        });
        let e = ErasedAdapter::watch(&a, &[]).unwrap_err();
        assert_eq!(e.detail, "the provider said no");
        assert_eq!(
            e.failure,
            Failure::Throttled {
                retry_after_millis: 30_000
            }
        );
    }

    #[test]
    fn a_boxed_erased_adapter_is_still_an_adapter() {
        // This is the property the whole module exists for: `sift_sync::run` and
        // `sift_mutations::flush` stay generic over `Adapter`, and the application holds a
        // box whose provider it cannot name.
        let boxed: Box<dyn ErasedAdapter> = Box::new(Failing::new(Failure::Unknown));
        let e = Adapter::apply(&boxed, &[]).unwrap_err();
        assert_eq!(Adapter::classify(&boxed, &e), Failure::Unknown);
    }

    #[test]
    fn reclassifying_an_already_erased_failure_does_not_lose_it() {
        // The boxed adapter's `classify` must be the identity over what it already carries.
        // Answering `Transient` here would silently retry a permanent failure forever.
        for verdict in [
            Failure::CursorInvalidated,
            Failure::CredentialRefused,
            Failure::Transient,
            Failure::Unknown,
            Failure::Permanent,
        ] {
            let boxed: Box<dyn ErasedAdapter> = Box::new(Failing::new(verdict));
            let e = Adapter::watch(&boxed, &[]).unwrap_err();
            assert_eq!(Adapter::classify(&boxed, &e), verdict);
        }
    }
}
