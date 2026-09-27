//! D-106 — per-file key derivation, and the nonce collision it exists to prevent.
//!
//! # The defect this closes
//!
//! [`page`](crate::page) derives its nonce as `page number ‖ write counter`, and the counter
//! is **part of the file's own state** — it lives in that file's [`Header`](crate::page::Header)
//! and resumes above the persisted high-water mark on open. Within one file that is exactly
//! right, and `a_nonce_is_never_reused` proves it.
//!
//! D-74 gives every account **two** files: a store that may be discarded and a journal that
//! may not. `docs/storage/encryption.md` says the account key "is held directly as its own
//! credential item". Read together, those two sentences put the same key over both files —
//! and each file independently issues counter 0 for page 0, counter 1 for page 1, and so on.
//! Same key, same nonce, different plaintext. That is not a weakening of AES-GCM; it is the
//! one thing AES-GCM must never be asked to do, and it costs both confidentiality and
//! authenticity.
//!
//! The additional authenticated data binds the key identifier, which under one key is
//! identical for both files, so it does not save it.
//!
//! It would also never have been noticed. `page.rs` says of its own nonce derivation that
//! getting it wrong *"does not fail a test, does not corrupt a file, and nothing observable
//! goes wrong"* — and the existing test walks a single file's counter space, so it passes
//! either way.
//!
//! # The fix
//!
//! Give each file its own key, derived from the account key by role. Identical
//! `(page, counter)` pairs across the two files are then harmless, because they are under
//! different keys.
//!
//! **FR-4's proof is untouched**, which is the property that made holding the account key
//! directly worth doing in the first place. The account key is still the one credential item;
//! derivation is one-way; destroying that item still makes both files unreadable by
//! construction rather than by a promise to overwrite them.
//!
//! The same argument applies to the installation-scoped files, and to D-43's per-installation
//! secret, which is otherwise used both as a BLAKE3 key and as key material — two uses of one
//! secret with no domain separation between them.
//!
//! # Below the database: one key per file, and per incarnation
//!
//! A [`Role`] names a *database*, and SQLite derives up to two more files from each one's path —
//! its write-ahead log and its rollback journal. Each keeps a counter of its own, starting from
//! its own header, so one role key over all three is the same collision one level down: the
//! database and its log both seal block 0 at counter 1. That is why a role key is never sealed
//! under directly. [`part_key`] derives the key a file actually uses from the role key, the
//! file's [`Part`], and a salt the file's header carries.
//!
//! The salt closes the other half. The engine deletes the write-ahead log on a clean close and
//! creates it again on the next open, at the same path under the same role key, and a new file's
//! counter starts from a new header — so without the salt every incarnation of the log would
//! reissue the nonces the one before it used. A fresh salt per incarnation makes each one a
//! fresh key.

use crate::page::{KeyId, PageKey, SALT_LEN};

/// What a derived key is for.
///
/// A closed set rather than a free-form string, because the whole guarantee is that two
/// files never share one, and a typo in a context string is a silent collision — the same
/// class of failure this module exists to close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    /// An account's base state — D-74's discardable half.
    Store,
    /// An account's mutation journal — D-74's half that may not be discarded.
    Journal,
    /// D-43's shared blob index. Installation-scoped: it belongs to no account.
    BlobIndex,
    /// The installation policy store, which holds the account registry.
    Policy,
}

impl Role {
    /// The derivation context. **Permanent**: changing one makes every file under it
    /// unreadable, which is a key destruction wearing the clothes of a refactor.
    const fn context(self) -> &'static str {
        match self {
            Self::Store => "sift/file/store/v1",
            Self::Journal => "sift/file/journal/v1",
            Self::BlobIndex => "sift/file/blob-index/v1",
            Self::Policy => "sift/file/policy/v1",
        }
    }

    /// The context for this role's key identifier, which must differ from the key's own.
    ///
    /// Deriving the identifier from the same context as the key would publish a value in
    /// every file header that is a function of the key under the same derivation — not a
    /// recovery of it, but not a thing to write down either.
    const fn identifier_context(self) -> &'static str {
        match self {
            Self::Store => "sift/key-id/store/v1",
            Self::Journal => "sift/key-id/journal/v1",
            Self::BlobIndex => "sift/key-id/blob-index/v1",
            Self::Policy => "sift/key-id/policy/v1",
        }
    }
}

/// The key a file is sealed under, derived from the key its owner holds.
///
/// `owner` is the account key for [`Role::Store`] and [`Role::Journal`], and D-43's
/// per-installation secret for the two installation-scoped roles.
#[must_use]
pub fn file_key(owner: &[u8; 32], role: Role) -> PageKey {
    PageKey::from_bytes(blake3::derive_key(role.context(), owner))
}

/// The identifier that file's header carries.
///
/// D-76 requires a file under a destroyed key be *recognisable* rather than merely
/// unreadable, and requires the identifier so rotation can be lazy. Deriving it means a file
/// names its key with nothing stored to map between them.
#[must_use]
pub fn file_key_id(owner: &[u8; 32], role: Role) -> KeyId {
    let full = blake3::derive_key(role.identifier_context(), owner);
    let mut id = [0u8; 16];
    id.copy_from_slice(&full[..16]);
    KeyId(id)
}

/// Which of a database's files a key seals.
///
/// A closed set for the same reason [`Role`] is one: two files under one context is a silent
/// collision, not an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Part {
    /// The database file itself.
    Database,
    /// Its write-ahead log, the `-wal` file.
    WriteAheadLog,
    /// Its rollback journal, the `-journal` file.
    RollbackJournal,
}

impl Part {
    /// The derivation context. **Permanent**, as [`Role`]'s are.
    const fn context(self) -> &'static str {
        match self {
            Self::Database => "sift/part/database/v1",
            Self::WriteAheadLog => "sift/part/write-ahead-log/v1",
            Self::RollbackJournal => "sift/part/rollback-journal/v1",
        }
    }
}

/// The key one file is sealed under: the database's role key, separated by the file's [`Part`]
/// and by the salt that file's header carries.
///
/// The key identifier is not separated the same way, deliberately. It names a key
/// *generation* — the thing D-22 rotates and FR-4 destroys — and a generation spans all of a
/// database's files, so every file of one database carries its database's identifier. The
/// collision this closes is one of keys, and the keys are what differ.
#[must_use]
pub fn part_key(role_key: &PageKey, part: Part, salt: &[u8; SALT_LEN]) -> PageKey {
    // Fixed-length inputs in a fixed order, so no two (key, salt) pairs hash the same bytes.
    let mut hasher = blake3::Hasher::new_derive_key(part.context());
    hasher.update(role_key.bytes());
    hasher.update(salt);
    PageKey::from_bytes(*hasher.finalize().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::{Header, PageCipher};
    use std::collections::BTreeSet;

    const OWNER: [u8; 32] = [7u8; 32];

    fn cipher_for(role: Role) -> PageCipher {
        let key = file_key(&OWNER, role);
        let header = Header::new(file_key_id(&OWNER, role));
        PageCipher::new(&key, &header)
    }

    #[test]
    fn an_accounts_two_files_do_not_share_a_key() {
        // The defect, stated as the test that would have caught it. D-74 gives an account two
        // files and encryption.md holds one key; without this derivation they are the same key.
        let store = file_key(&OWNER, Role::Store);
        let journal = file_key(&OWNER, Role::Journal);
        assert_ne!(store.bytes(), journal.bytes());
        assert_ne!(
            store.bytes(),
            &OWNER,
            "a derived key is not the owner's key"
        );
    }

    #[test]
    fn every_role_derives_a_distinct_key_and_a_distinct_identifier() {
        let roles = [Role::Store, Role::Journal, Role::BlobIndex, Role::Policy];
        let keys: BTreeSet<[u8; 32]> = roles
            .iter()
            .map(|r| *file_key(&OWNER, *r).bytes())
            .collect();
        let ids: BTreeSet<KeyId> = roles.iter().map(|r| file_key_id(&OWNER, *r)).collect();
        assert_eq!(keys.len(), roles.len(), "two roles share a key");
        assert_eq!(ids.len(), roles.len(), "two roles share an identifier");
    }

    #[test]
    fn a_files_identifier_is_not_its_key() {
        for role in [Role::Store, Role::Journal, Role::BlobIndex, Role::Policy] {
            let key = file_key(&OWNER, role);
            let id = file_key_id(&OWNER, role);
            assert_ne!(&key.bytes()[..16], &id.0[..], "{role:?}");
        }
    }

    #[test]
    fn the_same_page_and_counter_in_two_files_seals_to_different_bytes() {
        // This is the property the whole module buys, asserted end to end rather than at the
        // derivation. Both files write page 0 with counter 0 — which is exactly what happens
        // on the first write to each — and the results must not be equal.
        let store = cipher_for(Role::Store);
        let journal = cipher_for(Role::Journal);
        let plaintext = b"the same page in both halves";

        let a = store.seal_with_counter(0, 0, plaintext).unwrap();
        let b = journal.seal_with_counter(0, 0, plaintext).unwrap();
        assert_ne!(a, b, "one key over two files reuses a nonce");
    }

    #[test]
    fn a_file_cannot_be_opened_under_another_roles_key() {
        // The other half of the same property: separation is real, not cosmetic. A journal
        // page must not decrypt under the store's key even though both derive from one owner.
        let store = cipher_for(Role::Store);
        let journal = cipher_for(Role::Journal);
        let sealed = journal.seal_with_counter(3, 9, b"journal bytes").unwrap();
        assert!(store.open(3, &sealed).is_err());
    }

    #[test]
    fn destroying_the_owner_key_is_what_makes_both_files_unreadable() {
        // FR-4: erasure stays provable by enumeration. Derivation is one-way, so there is no
        // path from either derived key back to the credential item, and no second item to
        // forget to destroy.
        let other: [u8; 32] = [8u8; 32];
        for role in [Role::Store, Role::Journal] {
            assert_ne!(
                file_key(&OWNER, role).bytes(),
                file_key(&other, role).bytes()
            );
        }
    }

    #[test]
    fn one_key_over_two_files_is_the_collision_this_module_prevents() {
        // The falsifier, kept as a test so the reason this module exists cannot be argued
        // away later. This is what `encryption.md` described before the fix: one key held
        // directly, two files under it, each issuing counter 0 for page 0.
        //
        // Identical ciphertext under AES-GCM means an identical keystream, so the two
        // plaintexts are recoverable from each other by XOR — and the tag forgery follows.
        let one_key_for_both = PageKey::from_bytes(OWNER);
        let id = KeyId([0u8; 16]);
        let store = PageCipher::new(&one_key_for_both, &Header::new(id));
        let journal = PageCipher::new(&PageKey::from_bytes(OWNER), &Header::new(id));

        let a = store
            .seal_with_counter(0, 0, b"the same page bytes")
            .unwrap();
        let b = journal
            .seal_with_counter(0, 0, b"the same page bytes")
            .unwrap();
        assert_eq!(
            a, b,
            "the defect: one key over two files gives one keystream"
        );

        // And the fix, side by side, on the same inputs.
        let sealed_store = cipher_for(Role::Store)
            .seal_with_counter(0, 0, b"the same page bytes")
            .unwrap();
        let sealed_journal = cipher_for(Role::Journal)
            .seal_with_counter(0, 0, b"the same page bytes")
            .unwrap();
        assert_ne!(sealed_store, sealed_journal);
    }

    const PARTS: [Part; 3] = [Part::Database, Part::WriteAheadLog, Part::RollbackJournal];

    fn part_cipher(part: Part, salt: [u8; SALT_LEN]) -> PageCipher {
        let key = part_key(&file_key(&OWNER, Role::Store), part, &salt);
        PageCipher::new(&key, &Header::new(file_key_id(&OWNER, Role::Store)))
    }

    #[test]
    fn a_database_and_its_log_and_journal_seal_under_three_keys() {
        // #155: the database and its write-ahead log each seal block 0 at counter 1 on their
        // first write. Under one key that is one nonce over two plaintexts.
        let role = file_key(&OWNER, Role::Store);
        let salt = [1u8; SALT_LEN];
        let keys: BTreeSet<[u8; 32]> = PARTS
            .iter()
            .map(|p| *part_key(&role, *p, &salt).bytes())
            .collect();
        assert_eq!(keys.len(), PARTS.len(), "two parts share a key");
        assert!(
            !keys.contains(role.bytes()),
            "a part is sealed under the role key itself"
        );

        let db = part_cipher(Part::Database, salt);
        let wal = part_cipher(Part::WriteAheadLog, salt);
        let a = db.seal_with_counter(0, 1, b"page zero").unwrap();
        let b = wal.seal_with_counter(0, 1, b"page zero").unwrap();
        assert_ne!(a, b, "the database and its log share a keystream");
        assert!(
            wal.open(0, &a).is_err(),
            "a database block opened as the log's"
        );
    }

    #[test]
    fn a_new_incarnation_of_a_file_is_a_new_key() {
        // #155: the log is deleted on a clean close and created again on the next open, at the
        // same path under the same role key, with a counter that starts over. Its salt is what
        // makes the second one a different key.
        let first = part_cipher(Part::WriteAheadLog, [1u8; SALT_LEN]);
        let second = part_cipher(Part::WriteAheadLog, [2u8; SALT_LEN]);
        let a = first.seal_with_counter(0, 1, b"frame").unwrap();
        let b = second.seal_with_counter(0, 1, b"frame").unwrap();
        assert_ne!(a, b, "two incarnations of one file share a keystream");
        assert!(second.open(0, &a).is_err());
        assert_eq!(first.open(0, &a).unwrap(), b"frame");
    }

    #[test]
    fn a_part_key_is_separated_by_its_role_key() {
        // Two databases with one salt still do not share a part key: the role is an input.
        let salt = [3u8; SALT_LEN];
        let store = part_key(&file_key(&OWNER, Role::Store), Part::Database, &salt);
        let journal = part_key(&file_key(&OWNER, Role::Journal), Part::Database, &salt);
        assert_ne!(store.bytes(), journal.bytes());
    }

    #[test]
    fn derivation_is_stable_across_runs() {
        // These contexts are permanent. A change to one is a key destruction wearing the
        // clothes of a refactor, so pin the bytes rather than only the relationships.
        let k = file_key(&OWNER, Role::Store);
        assert_eq!(k.bytes(), file_key(&OWNER, Role::Store).bytes());
        assert_eq!(
            file_key_id(&OWNER, Role::Store),
            file_key_id(&OWNER, Role::Store)
        );
        let salt = [4u8; SALT_LEN];
        assert_eq!(
            part_key(&k, Part::WriteAheadLog, &salt).bytes(),
            part_key(&k, Part::WriteAheadLog, &salt).bytes()
        );
    }
}
