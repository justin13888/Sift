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
use sift_crypto::derive::{Role, file_key, file_key_id};
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
