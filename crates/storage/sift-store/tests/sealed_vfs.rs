//! D-42's page encryption, exercised through the engine rather than through the cipher.
//!
//! The unit tests in `sift-crypto` prove that a page round-trips and that a wrong key is
//! refused. What they cannot prove is the part that loses mail: that SQLite, driving this VFS
//! at the offsets and sizes it actually uses — a 100-byte header read, a 32-byte write-ahead
//! log header, a checkpoint, a recovery — reads back exactly what it wrote.
//!
//! So every test here goes through `rusqlite`, and several of them look at the bytes on disk
//! afterwards. A test that only asked SQLite whether it was happy would pass against a VFS
//! that wrote everything in the clear.

use rusqlite::Connection;
use sift_crypto::derive::{Part, Role, file_key, file_key_id, part_key};
use sift_crypto::page::{Header, PageCipher};
use sift_store::vfs;
use std::path::{Path, PathBuf};

fn scratch(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let d = std::env::temp_dir().join(format!(
        "sift-vfs-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).expect("scratch");
    d
}

fn owner(seed: u8) -> [u8; 32] {
    [seed; 32]
}

/// Open a sealed connection the way the account store will.
fn open_sealed(path: &Path, owner: &[u8; 32]) -> Connection {
    vfs::register().expect("the VFS registers");
    vfs::present_key(
        path,
        file_key(owner, Role::Store),
        file_key_id(owner, Role::Store),
    );
    let conn = Connection::open_with_flags_and_vfs(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
        vfs::VFS_NAME,
    )
    .expect("open through the sealed VFS");
    apply_pragmas(&conn);
    conn
}

/// The ones the layer cannot work without. Each fails silently, so each is asserted.
fn apply_pragmas(conn: &Connection) {
    conn.pragma_update(None, "page_size", 4096i64)
        .expect("page_size");
    conn.pragma_update(None, "temp_store", "MEMORY")
        .expect("temp_store");
    conn.pragma_update(None, "mmap_size", 0i64)
        .expect("mmap_size");
    let _: String = conn
        .pragma_update_and_check(None, "locking_mode", "EXCLUSIVE", |r| r.get(0))
        .expect("locking_mode");
    let _: String = conn
        .pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))
        .expect("journal_mode");
    vfs::assert_pragmas(conn).expect("the load-bearing pragmas hold");
}

#[test]
fn a_database_round_trips_through_the_seal() {
    let dir = scratch("roundtrip");
    let path = dir.join("account.store");
    let owner = owner(1);

    {
        let conn = open_sealed(&path, &owner);
        conn.execute_batch("CREATE TABLE message (id INTEGER PRIMARY KEY, subject TEXT NOT NULL);")
            .expect("schema");
        for i in 0..500 {
            conn.execute(
                "INSERT INTO message (id, subject) VALUES (?1, ?2)",
                rusqlite::params![i, format!("subject number {i}")],
            )
            .expect("insert");
        }
    }
    vfs::withdraw_key(&path);

    // A second process, in effect: nothing carried over but the file and the key.
    let conn = open_sealed(&path, &owner);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM message", [], |r| r.get(0))
        .expect("count");
    assert_eq!(count, 500);
    let subject: String = conn
        .query_row("SELECT subject FROM message WHERE id = 499", [], |r| {
            r.get(0)
        })
        .expect("read");
    assert_eq!(subject, "subject number 499");
    drop(conn);
    vfs::withdraw_key(&path);
    std::fs::remove_dir_all(&dir).ok();
}

/// The test that would still pass if the VFS wrote everything in the clear, unless it looks at
/// the bytes. It looks at the bytes.
#[test]
fn nothing_a_sender_wrote_is_on_disk_in_the_clear() {
    let dir = scratch("plaintext");
    let path = dir.join("account.store");
    let owner = owner(2);
    let secret = "Quarterly-Report-Do-Not-Disclose";

    {
        let conn = open_sealed(&path, &owner);
        conn.execute_batch("CREATE TABLE message (id INTEGER PRIMARY KEY, subject TEXT);")
            .expect("schema");
        conn.execute("INSERT INTO message (id, subject) VALUES (1, ?1)", [secret])
            .expect("insert");
    }
    vfs::withdraw_key(&path);

    // Every file the engine produced, not only the database: a write-ahead log left in the
    // clear beside a sealed database is the whole content of the database in the clear.
    let mut inspected = 0;
    for entry in std::fs::read_dir(&dir).expect("readable") {
        let file = entry.expect("entry").path();
        let bytes = std::fs::read(&file).expect("read");
        inspected += 1;
        assert!(
            !contains(&bytes, secret.as_bytes()),
            "{} holds the subject in the clear",
            file.display()
        );
        assert!(
            !contains(&bytes, b"CREATE TABLE message"),
            "{} holds the schema in the clear",
            file.display()
        );
        assert!(
            !bytes.starts_with(b"SQLite format 3"),
            "{} is an ordinary SQLite database",
            file.display()
        );
    }
    assert!(inspected > 0, "the engine produced no files to inspect");
    std::fs::remove_dir_all(&dir).ok();
}

/// D-73: a file that fails to authenticate is discarded, never repaired. So the wrong key is a
/// refusal rather than a database that reads as empty.
#[test]
fn the_wrong_key_is_refused_rather_than_producing_an_empty_database() {
    let dir = scratch("wrongkey");
    let path = dir.join("account.store");
    {
        let conn = open_sealed(&path, &owner(3));
        conn.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1);")
            .expect("write");
    }
    vfs::withdraw_key(&path);

    let other = owner(4);
    vfs::present_key(
        &path,
        file_key(&other, Role::Store),
        file_key_id(&other, Role::Store),
    );
    let opened = Connection::open_with_flags_and_vfs(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        vfs::VFS_NAME,
    );
    let refused = match opened {
        Err(_) => true,
        Ok(conn) => conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))
            .is_err(),
    };
    assert!(
        refused,
        "a database under the wrong key opened and answered, which is worse than failing"
    );
    vfs::withdraw_key(&path);
    std::fs::remove_dir_all(&dir).ok();
}

/// D-106, at the level that matters: the two files of one account are sealed under keys derived
/// for different roles, so neither file's counter space is the other's.
#[test]
fn the_store_and_the_journal_do_not_share_a_key() {
    let owner = owner(5);
    assert_ne!(
        file_key_id(&owner, Role::Store),
        file_key_id(&owner, Role::Journal),
        "one key over two files is the nonce reuse D-106 exists to prevent"
    );
}

// ---------------------------------------------------------------------------------------
// #155: D-106 below the database — the database, its log and its journal, and each
// incarnation of any of them, are sealed under keys of their own.
// ---------------------------------------------------------------------------------------

/// Where the sealed meta block starts, and where block zero does.
const META_AT: usize = sift_crypto::page::HEADER_LEN;
const DATA_AT: usize = META_AT + 88;
const META_PAGE: u32 = u32::MAX;

/// Every sealed unit in a file's bytes: `(page number, counter, sealed bytes)`.
fn sealed_units(bytes: &[u8]) -> Vec<(u32, u64, Vec<u8>)> {
    let counter = |at: usize| {
        let mut c = [0u8; 8];
        c.copy_from_slice(&bytes[at..at + 8]);
        u64::from_be_bytes(c)
    };
    let mut out = Vec::new();
    if bytes.len() >= DATA_AT {
        out.push((
            META_PAGE,
            counter(META_AT),
            bytes[META_AT..DATA_AT].to_vec(),
        ));
    }
    let mut at = DATA_AT;
    let mut block = 0u32;
    while at + vfs::SEALED_BLOCK <= bytes.len() {
        out.push((
            block,
            counter(at),
            bytes[at..at + vfs::SEALED_BLOCK].to_vec(),
        ));
        at += vfs::SEALED_BLOCK;
        block += 1;
    }
    out
}

/// The nonce identity of every unit a file holds, under the one owner key a test uses: which
/// key sealed it is the file's part and its salt, and the nonce is the page and the counter.
fn nonces(bytes: &[u8], part: Part) -> Vec<(Part, [u8; 6], u32, u64)> {
    let salt = Header::decode(bytes).expect("a Sift header").salt;
    sealed_units(bytes)
        .into_iter()
        .filter(|(_, counter, _)| *counter != 0)
        .map(|(page, counter, _)| (part, salt, page, counter))
        .collect()
}

/// The cipher a file's own header says it is sealed under, rebuilt from outside the VFS.
fn cipher_of(owner: &[u8; 32], bytes: &[u8], part: Part) -> PageCipher {
    let header = Header::decode(bytes).expect("a Sift header");
    let key = part_key(&file_key(owner, Role::Store), part, &header.salt);
    PageCipher::new(&key, &header)
}

fn wal_of(path: &Path) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push("-wal");
    PathBuf::from(p)
}

/// The issue's first test. While the connection is open, the database and its write-ahead log
/// are both on disk, and each seals under its own key — so a unit of one does not open under
/// the other's, and neither is under the key the database was registered with.
#[test]
fn a_database_and_its_write_ahead_log_are_sealed_under_different_keys() {
    let dir = scratch("wal-key");
    let path = dir.join("account.store");
    let owner = owner(40);
    let conn = open_sealed(&path, &owner);
    conn.execute_batch("CREATE TABLE t (x TEXT);")
        .expect("schema");
    for i in 0..20 {
        conn.execute("INSERT INTO t VALUES (?1)", [format!("row {i}")])
            .expect("insert");
    }

    let db = std::fs::read(&path).expect("the database");
    let wal = std::fs::read(wal_of(&path)).expect("the log, while the connection is open");
    let (db_cipher, wal_cipher) = (
        cipher_of(&owner, &db, Part::Database),
        cipher_of(&owner, &wal, Part::WriteAheadLog),
    );
    let registered = PageCipher::new(
        &file_key(&owner, Role::Store),
        &Header::decode(&db).expect("header"),
    );

    let (db_units, wal_units) = (sealed_units(&db), sealed_units(&wal));
    assert!(
        db_units.len() > 1 && wal_units.len() > 1,
        "nothing to compare"
    );
    for (page, _, sealed) in &db_units {
        db_cipher
            .open(*page, sealed)
            .expect("a database unit opens as the database's");
        assert!(
            wal_cipher.open(*page, sealed).is_err(),
            "a database unit opened as the log's"
        );
        assert!(
            registered.open(*page, sealed).is_err(),
            "sealed under the registered key"
        );
    }
    for (page, _, sealed) in &wal_units {
        wal_cipher
            .open(*page, sealed)
            .expect("a log unit opens as the log's");
        assert!(
            db_cipher.open(*page, sealed).is_err(),
            "a log unit opened as the database's"
        );
    }

    // Both files carry the database's key identifier: it names the generation, which D-22
    // rotates across all of them, and the separation is in the key.
    assert_eq!(
        Header::decode(&db).expect("header").key_id,
        Header::decode(&wal).expect("header").key_id
    );
    drop(conn);
    vfs::withdraw_key(&path);
    std::fs::remove_dir_all(&dir).ok();
}

/// The issue's second test. A clean close deletes the write-ahead log, and the next open creates
/// it again at the same path under the same registered key. No nonce the first incarnation
/// issued — no `(key, page, counter)` — is issued again by a later one, or by the database.
#[test]
fn a_write_ahead_log_created_again_at_the_same_path_reissues_no_nonce() {
    let dir = scratch("wal-again");
    let path = dir.join("account.store");
    let owner = owner(41);
    let mut issued = std::collections::BTreeSet::new();
    let mut salts = std::collections::BTreeSet::new();

    for round in 0..4 {
        let conn = open_sealed(&path, &owner);
        conn.execute_batch("CREATE TABLE IF NOT EXISTS t (x TEXT);")
            .expect("schema");
        for i in 0..10 {
            conn.execute("INSERT INTO t VALUES (?1)", [format!("{round}-{i}")])
                .expect("insert");
        }
        let wal = std::fs::read(wal_of(&path)).expect("the log, while the connection is open");
        salts.insert(Header::decode(&wal).expect("header").salt);
        for nonce in nonces(&wal, Part::WriteAheadLog) {
            assert!(issued.insert(nonce), "round {round} reissued {nonce:?}");
        }
        drop(conn);
        vfs::withdraw_key(&path);
        assert!(
            !wal_of(&path).exists(),
            "the log survived the close, so this is not the recreation under test"
        );
    }
    assert_eq!(salts.len(), 4, "an incarnation of the log reused a salt");

    let db = std::fs::read(&path).expect("the database");
    for nonce in nonces(&db, Part::Database) {
        assert!(issued.insert(nonce), "the database reissued {nonce:?}");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// The same property for the database itself: D-73's discard-and-refill deletes a store and
/// creates it again at the same path under the same account key.
#[test]
fn a_database_created_again_at_the_same_path_is_a_new_key() {
    let dir = scratch("db-again");
    let path = dir.join("account.store");
    let owner = owner(42);
    let mut incarnations = Vec::new();

    for _ in 0..2 {
        {
            let conn = open_sealed(&path, &owner);
            conn.execute_batch("CREATE TABLE t (x TEXT); INSERT INTO t VALUES ('refilled');")
                .expect("write");
        }
        vfs::withdraw_key(&path);
        incarnations.push(std::fs::read(&path).expect("the database"));
        std::fs::remove_file(&path).expect("discard");
    }

    let (first, second) = (&incarnations[0], &incarnations[1]);
    assert_ne!(
        Header::decode(first).expect("header").salt,
        Header::decode(second).expect("header").salt
    );
    let second_cipher = cipher_of(&owner, second, Part::Database);
    for (page, _, sealed) in sealed_units(first) {
        assert!(
            second_cipher.open(page, &sealed).is_err(),
            "the discarded store's page {page} opened under the refilled store's key"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// The write-ahead log's header is 32 bytes. A layer that reported lengths rounded up to its
/// block size would report 4096 for it, and recovery would read past what was written.
#[test]
fn a_short_file_reports_its_real_length() {
    let dir = scratch("shortlen");
    let path = dir.join("account.store");
    let owner = owner(6);
    {
        let conn = open_sealed(&path, &owner);
        conn.execute_batch("CREATE TABLE t (x TEXT);")
            .expect("schema");
        conn.execute("INSERT INTO t VALUES ('a')", [])
            .expect("insert");
    }
    vfs::withdraw_key(&path);

    // Reopening exercises recovery over whatever the log holds. A wrong length surfaces here,
    // as a corrupt database rather than as a wrong number.
    let conn = open_sealed(&path, &owner);
    let n: i64 = conn
        .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
        .expect("recovered");
    assert_eq!(n, 1);
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("integrity_check runs");
    assert_eq!(integrity, "ok");
    drop(conn);
    vfs::withdraw_key(&path);
    std::fs::remove_dir_all(&dir).ok();
}

/// A checkpoint moves every page of the log into the database, which is the largest single
/// rearrangement the engine performs and the one most likely to expose an offset mistake.
#[test]
fn a_checkpoint_moves_the_log_into_the_database_intact() {
    let dir = scratch("checkpoint");
    let path = dir.join("account.store");
    let owner = owner(7);
    let conn = open_sealed(&path, &owner);
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, blob BLOB);")
        .expect("schema");
    // Rows larger than a page, so the engine spills onto overflow pages of its own.
    for i in 0..200i64 {
        conn.execute(
            "INSERT INTO t (id, blob) VALUES (?1, ?2)",
            rusqlite::params![i, vec![u8::try_from(i % 251).unwrap_or(0); 9000]],
        )
        .expect("insert");
    }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .expect("checkpoint");

    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("integrity_check runs");
    assert_eq!(integrity, "ok", "the checkpoint corrupted the database");

    for i in [0i64, 99, 199] {
        let blob: Vec<u8> = conn
            .query_row("SELECT blob FROM t WHERE id = ?1", [i], |r| r.get(0))
            .expect("read back");
        assert_eq!(blob.len(), 9000);
        let expected = u8::try_from(i % 251).unwrap_or(0);
        assert!(blob.iter().all(|b| *b == expected));
    }
    drop(conn);
    vfs::withdraw_key(&path);
    std::fs::remove_dir_all(&dir).ok();
}

/// The property the reservation exists for: a counter is never issued twice under one key.
///
/// A run that resumed from the last written header would reissue every counter used since it
/// was written — and reissuing a counter is nonce reuse, which costs confidentiality and
/// authenticity both.
#[test]
fn a_counter_is_never_issued_twice_across_reopening() {
    let dir = scratch("counters");
    let path = dir.join("account.store");
    let owner = owner(8);
    let mut marks = Vec::new();

    for round in 0..5 {
        let conn = open_sealed(&path, &owner);
        conn.execute_batch("CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, v TEXT);")
            .expect("schema");
        conn.execute("INSERT INTO t (v) VALUES (?1)", [format!("round {round}")])
            .expect("insert");
        drop(conn);
        vfs::withdraw_key(&path);

        let bytes = std::fs::read(&path).expect("read");
        let header = sift_crypto::page::Header::decode(&bytes).expect("a Sift header");
        marks.push(header.counter_high_water);
    }

    assert!(
        marks.windows(2).all(|w| w[1] > w[0]),
        "the mark did not move forward every run: {marks:?}"
    );
    assert!(marks[0] > 0, "the first run reserved nothing");
    std::fs::remove_dir_all(&dir).ok();
}

/// The pragma assertions are the point, so they are asserted to actually fail.
#[test]
fn the_load_bearing_pragmas_are_checked_rather_than_assumed() {
    let dir = scratch("pragmas");
    let path = dir.join("account.store");
    vfs::register().expect("registers");
    let owner = owner(9);
    vfs::present_key(
        &path,
        file_key(&owner, Role::Store),
        file_key_id(&owner, Role::Store),
    );
    let conn = Connection::open_with_flags_and_vfs(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
        vfs::VFS_NAME,
    )
    .expect("open");

    // Nothing set: the default locking mode is normal, which is the one that leaves the
    // write-ahead index in a file this layer never sees.
    let complaint = vfs::assert_pragmas(&conn).expect_err("an unconfigured connection is refused");
    assert!(complaint.contains("locking_mode"), "{complaint}");

    drop(conn);
    vfs::withdraw_key(&path);
    std::fs::remove_dir_all(&dir).ok();
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// The on-disk shape, stated rather than implied: a Sift store begins with the crypto header,
/// and its length is the header plus the meta block plus a whole number of sealed blocks.
#[test]
fn the_file_on_disk_has_the_layout_this_module_documents() {
    let dir = scratch("layout");
    let path = dir.join("account.store");
    let owner = owner(10);
    {
        let conn = open_sealed(&path, &owner);
        conn.execute_batch("CREATE TABLE t (x TEXT); INSERT INTO t VALUES ('hello');")
            .expect("write");
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);").ok();
    }
    vfs::withdraw_key(&path);

    let bytes = std::fs::read(&path).expect("read");
    assert_eq!(
        &bytes[..8],
        &sift_crypto::page::MAGIC,
        "the file does not begin with the crypto header, so it was never sealed"
    );
    let data_start = sift_crypto::page::HEADER_LEN as u64 + 88;
    let payload = bytes.len() as u64 - data_start;
    assert_eq!(
        payload % vfs::SEALED_BLOCK as u64,
        0,
        "the file is not a whole number of sealed blocks: {} bytes after {data_start}",
        payload
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The account, opened the way the application opens it: two files, two derived keys, sealed.
#[test]
fn an_account_opens_sealed_and_both_of_its_files_are() {
    use sift_foundation::identity::AccountId;
    use sift_store::account::{Account, AccountPaths};

    let dir = scratch("account");
    let id = AccountId::from_u128(0x0102_0304_0506_0708_090a_0b0c_0d0e_0f10);
    let paths = AccountPaths::under(&dir, id);
    let owner = owner(11);

    {
        let account = Account::open_sealed(&paths, id, &owner).expect("open sealed");
        account
            .store
            .execute(
                "INSERT INTO folder (id, remote_id, special_use, display_name, retired, watched)
                 VALUES (1, 'INBOX', 'inbox', 'Inbox', 0, 1)",
                [],
            )
            .expect("a row with a display name in it");
    }

    for file in [&paths.store, &paths.journal] {
        let bytes = std::fs::read(file).expect("read");
        assert_eq!(
            &bytes[..8],
            &sift_crypto::page::MAGIC,
            "{} is not sealed",
            file.display()
        );
    }
    // And what was written is readable again under the same derived keys.
    let account = Account::open_sealed(&paths, id, &owner).expect("reopen sealed");
    let name: String = account
        .store
        .query_row("SELECT display_name FROM folder WHERE id = 1", [], |r| {
            r.get(0)
        })
        .expect("read back");
    assert_eq!(name, "Inbox");
    drop(account);
    std::fs::remove_dir_all(&dir).ok();
}

/// A different account's key does not open this one. FR-4's erasure rests on exactly this:
/// destroying the credential makes the files unreadable rather than merely unreferenced.
#[test]
fn an_accounts_files_do_not_open_under_another_accounts_key() {
    use sift_foundation::identity::AccountId;
    use sift_store::account::{Account, AccountPaths};

    let dir = scratch("erasure");
    let id = AccountId::from_u128(7);
    let paths = AccountPaths::under(&dir, id);
    {
        let _ = Account::open_sealed(&paths, id, &owner(12)).expect("open sealed");
    }
    let refused = Account::open_sealed(&paths, id, &owner(13)).is_err();
    assert!(
        refused,
        "another account's key opened these files, so erasure by credential destruction proves nothing"
    );
    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------------------
// D-22's lazy rotation.
// ---------------------------------------------------------------------------------------

/// Open a sealed connection with both generations of a rotating key.
fn open_rotating(path: &Path, current: &[u8; 32], retiring: &[u8; 32]) -> Connection {
    vfs::register().expect("the VFS registers");
    vfs::present_rotating_key(
        path,
        file_key(current, Role::Store),
        file_key_id(current, Role::Store),
        file_key(retiring, Role::Store),
        file_key_id(retiring, Role::Store),
    );
    let conn = Connection::open_with_flags_and_vfs(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
        vfs::VFS_NAME,
    )
    .expect("open through the sealed VFS");
    apply_pragmas(&conn);
    conn
}

/// Rows large enough that the table spans many pages, so a rotation that rewrites a few of them
/// leaves the rest under the old key.
fn fill(conn: &Connection) {
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v BLOB NOT NULL);")
        .expect("schema");
    for i in 0..200i64 {
        conn.execute(
            "INSERT INTO t (id, v) VALUES (?1, ?2)",
            rusqlite::params![i, vec![u8::try_from(i % 251).unwrap_or(0); 1500]],
        )
        .expect("insert");
    }
}

/// Every row reads back as `fill` wrote it, or as `touch` rewrote it.
fn assert_intact(conn: &Connection) {
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .expect("integrity_check runs");
    assert_eq!(integrity, "ok");
    let mut stmt = conn
        .prepare("SELECT id, v FROM t ORDER BY id")
        .expect("prepare");
    let rows: Vec<(i64, Vec<u8>)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("every row reads");
    assert_eq!(rows.len(), 200);
    for (id, v) in rows {
        let expected = if id < 10 {
            0xEE
        } else {
            u8::try_from(id % 251).unwrap_or(0)
        };
        assert!(v.iter().all(|b| *b == expected), "row {id} changed");
    }
}

/// Rewrite the first few rows, which re-seals the pages that hold them and no others.
fn touch(conn: &Connection) {
    conn.execute("UPDATE t SET v = ?1 WHERE id < 10", [vec![0xEEu8; 1500]])
        .expect("update");
}

/// The issue's first test: a store half re-sealed, across a restart.
///
/// Written under the old key, then opened with both and partly rewritten, then closed — which is
/// the restart — and opened again. Both generations must be on disk at once, and both must read.
#[test]
fn a_store_half_re_sealed_across_a_restart_reads_under_both_generations() {
    let dir = scratch("rotate-half");
    let path = dir.join("account.store");
    let (old, new) = (owner(20), owner(21));
    let (old_id, new_id) = (
        file_key_id(&old, Role::Store),
        file_key_id(&new, Role::Store),
    );

    {
        let conn = open_sealed(&path, &old);
        fill(&conn);
    }
    vfs::withdraw_key(&path);
    let before = vfs::generations(&path).expect("count");
    assert_eq!(before.keys().copied().collect::<Vec<_>>(), vec![old_id]);

    {
        let conn = open_rotating(&path, &new, &old);
        touch(&conn);
    }
    vfs::withdraw_key(&path);

    let half = vfs::generations(&path).expect("count");
    let (under_old, under_new) = (half[&old_id], half[&new_id]);
    assert!(under_new > 0, "nothing was re-sealed under the new key");
    assert!(
        under_old > 0,
        "every page was rewritten, so this is not the half-re-sealed store under test"
    );
    assert!(
        under_old < before[&old_id],
        "the rewritten pages still count as the old key's"
    );
    let header = vfs::headers(&path).expect("headers")[0];
    assert_eq!(header.key_id, new_id, "the header still names the old key");
    assert_eq!(header.retiring.map(|r| r.key_id), Some(old_id));

    // The restart: nothing carried over but the files and both keys.
    {
        let conn = open_rotating(&path, &new, &old);
        assert_intact(&conn);
    }
    vfs::withdraw_key(&path);
    assert_eq!(
        vfs::generations(&path).expect("count")[&old_id],
        under_old,
        "reading re-sealed pages, or lost the count across the restart"
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// The retirement rule: the old key goes only when no page carries it, and after it goes the
/// store opens under the new key alone.
#[test]
fn the_old_generation_is_retired_only_once_no_page_carries_it() {
    let dir = scratch("rotate-retire");
    let path = dir.join("account.store");
    let (old, new) = (owner(22), owner(23));
    let old_id = file_key_id(&old, Role::Store);

    {
        let conn = open_sealed(&path, &old);
        fill(&conn);
    }
    vfs::withdraw_key(&path);
    {
        let conn = open_rotating(&path, &new, &old);
        touch(&conn);
    }
    vfs::withdraw_key(&path);

    let refused = vfs::forget_retired(&path, old_id).expect_err("pages still carry the old key");
    assert_eq!(refused.kind(), std::io::ErrorKind::InvalidInput);
    assert!(vfs::headers(&path).expect("headers")[0].retiring.is_some());

    // Rewriting every page is what finishes a lazy rotation; VACUUM is the engine doing so.
    {
        let conn = open_rotating(&path, &new, &old);
        conn.execute_batch("VACUUM;").expect("vacuum");
    }
    vfs::withdraw_key(&path);
    assert_eq!(vfs::generations(&path).expect("count").get(&old_id), None);

    vfs::forget_retired(&path, old_id).expect("nothing carries it now");
    assert_eq!(vfs::headers(&path).expect("headers")[0].retiring, None);

    {
        let conn = open_sealed(&path, &new);
        assert_intact(&conn);
    }
    vfs::withdraw_key(&path);

    // With the old generation struck, the next rotation starts from one generation rather than
    // being refused as a third.
    let newer = owner(29);
    {
        let conn = open_rotating(&path, &newer, &new);
        touch(&conn);
        assert_intact(&conn);
    }
    vfs::withdraw_key(&path);
    let header = vfs::headers(&path).expect("headers")[0];
    assert_eq!(header.key_id, file_key_id(&newer, Role::Store));
    assert_eq!(
        header.retiring.map(|r| r.key_id),
        Some(file_key_id(&new, Role::Store))
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// Two generations is what the header has room for. A file whose previous rotation still has
/// pages is refused a third rather than left with pages nothing can open.
#[test]
fn a_third_generation_is_refused_while_the_second_still_carries_pages() {
    let dir = scratch("rotate-third");
    let path = dir.join("account.store");
    let (first, second, third) = (owner(24), owner(25), owner(26));

    {
        let conn = open_sealed(&path, &first);
        fill(&conn);
    }
    vfs::withdraw_key(&path);
    {
        let conn = open_rotating(&path, &second, &first);
        touch(&conn);
    }
    vfs::withdraw_key(&path);

    vfs::register().expect("registers");
    vfs::present_rotating_key(
        &path,
        file_key(&third, Role::Store),
        file_key_id(&third, Role::Store),
        file_key(&second, Role::Store),
        file_key_id(&second, Role::Store),
    );
    let opened = Connection::open_with_flags_and_vfs(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        vfs::VFS_NAME,
    );
    let refused = match opened {
        Err(_) => true,
        Ok(conn) => conn
            .query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))
            .is_err(),
    };
    assert!(
        refused,
        "a third generation was started over live pages of the first"
    );
    vfs::withdraw_key(&path);
    std::fs::remove_dir_all(&dir).ok();
}

/// The issue's second test: a page under a destroyed old key is recognised rather than reported
/// as corrupt. The account refuses to open and says which key its pages need, and how many.
#[test]
fn an_account_under_a_destroyed_old_key_is_recognised_rather_than_corrupt() {
    use sift_foundation::identity::AccountId;
    use sift_store::account::{Account, AccountPaths, OpenError};

    let dir = scratch("rotate-destroyed");
    let id = AccountId::from_u128(0x107);
    let paths = AccountPaths::under(&dir, id);
    let (old, new) = (owner(27), owner(28));

    {
        let a = Account::open_sealed(&paths, id, &old).expect("create under the old key");
        for i in 0..200 {
            a.store
                .execute(
                    "INSERT INTO tag (name) VALUES (?1)",
                    [format!("{i}-{}", "x".repeat(400))],
                )
                .expect("a row");
        }
    }
    {
        let a = Account::open_sealed_rotating(&paths, id, &new, Some(&old)).expect("rotate");
        a.store
            .execute("INSERT INTO tag (name) VALUES ('after the rotation')", [])
            .expect("a write re-seals what it touches");
    }
    let left = Account::retiring_pages(&paths, &old).expect("count");
    assert!(left > 0, "every page was rewritten; nothing is under test");

    // The old key is destroyed too early. The open names the key and the pages, rather than
    // opening and failing at whichever page the first query happens to reach.
    match Account::open_sealed(&paths, id, &new) {
        Err(OpenError::KeyNotHeld { key_id, pages }) => {
            assert!(
                key_id == file_key_id(&old, Role::Store)
                    || key_id == file_key_id(&old, Role::Journal),
                "named a key that is not the retired one"
            );
            assert!(pages > 0);
        }
        other => panic!("expected the retired key to be recognised, got {other:?}"),
    }

    // Held again, both generations read.
    let a = Account::open_sealed_rotating(&paths, id, &new, Some(&old)).expect("reopen");
    let n: i64 = a
        .store
        .query_row("SELECT count(*) FROM tag", [], |r| r.get(0))
        .expect("count");
    assert!(n >= 201, "rows were lost: {n}");
    drop(a);
    std::fs::remove_dir_all(&dir).ok();
}
