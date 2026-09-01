//! D-42's page encryption, as a SQLite VFS.
//!
//! # Why a VFS and not something simpler
//!
//! SQLite removed `SQLITE_HAS_CODEC` in 3.32, so the in-tree hook that page-level encryption
//! used to attach to no longer exists. SQLCipher is a *fork* of SQLite, and adopting it would
//! defeat D-21's reason for bundling the engine in the first place. A VFS is what is left, and
//! it is enough: every byte the engine reads or writes passes through `xRead` and `xWrite`.
//!
//! # The layout
//!
//! ```text
//! offset 0    64 bytes   the crypto header: magic, format version, key id, counter high-water
//! offset 64   88 bytes   the sealed meta block: the file's exact logical length
//! offset 152  4120 each  sealed blocks, one per 4096 bytes of logical file
//! ```
//!
//! A sealed block is `counter ‖ ciphertext(4096) ‖ tag` — 8 + 4096 + 16.
//!
//! **The length is sealed rather than stored in the clear.** It has to be stored: the write-ahead
//! log's header is 32 bytes, and a file whose length is rounded up to the block size reports 4096
//! for it, which breaks log recovery outright. Storing it in the clear would leave one field an
//! attacker could edit to truncate a database without failing authentication — so it is sealed as
//! its own page, under a page number no data block can use.
//!
//! # Why the counter is reserved ahead of use
//!
//! D-76 derives the nonce from `page ‖ counter`, and the counter lives in the header. A run that
//! resumed the counter from the last *written* header would reissue every counter used since that
//! header was written, and reissuing a counter under one key is nonce reuse — which costs
//! confidentiality and authenticity both, and which `page.rs` warns "does not fail a test, does
//! not corrupt a file, and nothing observable goes wrong".
//!
//! So a run reserves a window of counters, records the top of it, and syncs that record **before
//! issuing any of them**. A crash then leaves the mark ahead of reality, which wastes counters;
//! the alternative leaves it behind, which reuses them.
//!
//! # Three pragmas are load-bearing rather than advisory
//!
//! - `locking_mode=EXCLUSIVE`, because it is what removes the `-shm` file. In shared mode the
//!   write-ahead index is a memory-mapped file this layer does not see, and the pages of a
//!   database would be sealed while the index describing them sat in the clear beside it.
//! - `mmap_size=0`, because a memory-mapped read bypasses `xRead` entirely and would hand the
//!   pager ciphertext.
//! - `temp_store=MEMORY`, because a spill file has no key and would be plaintext.
//!
//! [`assert_pragmas`] checks all three against a live connection rather than trusting that
//! somebody set them, because each one fails silently and none of them fails a test.

use rusqlite::ffi;
use sift_crypto::page::{Header, KeyId, PageCipher, PageKey};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// The logical page. Matched by `pragma page_size` so that one engine page is one sealed block.
pub const LOGICAL_BLOCK: usize = 4096;
/// `counter ‖ ciphertext ‖ tag`.
pub const SEALED_BLOCK: usize = 8 + LOGICAL_BLOCK + 16;
/// The crypto header.
const CRYPTO_HEADER: i64 = sift_crypto::page::HEADER_LEN as i64;
/// The sealed meta block: 8 counter + 64 payload + 16 tag.
const META_BLOCK: i64 = 8 + 64 + 16;
/// Where block zero starts.
const DATA_START: i64 = CRYPTO_HEADER + META_BLOCK;
/// The page number the meta block is sealed under. No data block can reach it: a database of
/// 2^32 - 1 blocks is 16 TiB, far above L-3.
const META_PAGE: u32 = u32::MAX;
/// How many counters a run reserves at a time.
///
/// Large enough that extending is rare, small enough that a crash wastes an irrelevant slice of
/// a 64-bit space. At one counter per page write, this is roughly four gigabytes of writes.
const RESERVATION: u64 = 1 << 20;

/// The VFS's name, as SQLite knows it.
pub const VFS_NAME: &str = "sift-sealed";

/// Keys, by the path of the database they belong to.
///
/// A URI parameter would be the obvious channel and is the wrong one: it puts key material in a
/// connection string, which is the kind of value that ends up in a log or a crash dump — and
/// NFR-23 keeps credentials out of both.
static KEYS: OnceLock<Mutex<HashMap<PathBuf, (PageKey, KeyId)>>> = OnceLock::new();

fn keys() -> &'static Mutex<HashMap<PathBuf, (PageKey, KeyId)>> {
    KEYS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Lend a key to the VFS for the duration of an open.
///
/// The write-ahead log and the rollback journal are named by adding a suffix to the database's
/// own path, so one entry covers all three files.
///
/// The identifier travels with the key rather than being derived here, because D-106 derives
/// both from the account key together and a second derivation is a second answer.
pub fn present_key(database: &Path, key: PageKey, key_id: KeyId) {
    if let Ok(mut map) = keys().lock() {
        map.insert(canonical(database), (key, key_id));
    }
}

/// Withdraw a key. The files stay sealed; nothing new can be opened against them.
///
/// The key is dropped here, and `PageKey`'s drop zeroes it — which is the point of doing this
/// rather than leaving the registry to grow for the life of the process.
pub fn withdraw_key(database: &Path) {
    if let Ok(mut map) = keys().lock() {
        map.remove(&canonical(database));
    }
}

/// The path both sides of the registry agree on.
///
/// **This is not tidiness, and it is not theoretical.** SQLite resolves a filename through
/// `xFullPathname` before it opens anything, and on macOS the temporary directory every test
/// and every sandboxed container starts from is `/var/folders/…`, which is a symlink to
/// `/private/var/folders/…`. A registry keyed on the caller's spelling therefore misses on the
/// engine's spelling — and a miss here is not an error. It is a file that quietly opens
/// **unsealed**, writes the user's mail to disk in the clear, and reads back perfectly. Every
/// round-trip test passes; the only one that fails is the one that looks at the bytes.
///
/// The parent is canonicalized rather than the whole path, because the file itself does not
/// exist yet on the open that creates it.
fn canonical(path: &Path) -> PathBuf {
    let Some(parent) = path.parent() else {
        return path.to_path_buf();
    };
    let Some(name) = path.file_name() else {
        return path.to_path_buf();
    };
    parent
        .canonicalize()
        .map_or_else(|_| path.to_path_buf(), |p| p.join(name))
}

/// The database a file belongs to, by stripping the suffix SQLite derived.
fn database_of(name: &Path) -> PathBuf {
    let text = name.to_string_lossy().into_owned();
    let base = ["-wal", "-journal", "-shm"]
        .iter()
        .find_map(|s| text.strip_suffix(s))
        .unwrap_or(&text);
    canonical(Path::new(base))
}

/// The identifier a database is registered under, or `None` if it is not.
fn key_id_of(database: &Path) -> Option<KeyId> {
    keys().lock().ok()?.get(database).map(|(_, id)| *id)
}

/// Build the cipher **without the key leaving the registry**.
///
/// [`PageKey`] is deliberately neither `Copy` nor `Clone` and zeroes itself on drop: key
/// material that can be copied freely is key material that ends up somewhere NFR-23 forbids.
/// So the cipher is constructed under the lock, from a borrow, and only the cipher travels.
fn cipher_for(database: &Path, header: &Header) -> Option<PageCipher> {
    let map = keys().lock().ok()?;
    let (key, _) = map.get(database)?;
    Some(PageCipher::new(key, header))
}

/// What this layer keeps for one open file.
struct Sealed {
    cipher: PageCipher,
    /// The top of the reserved counter window, as recorded in the header on disk.
    reserved: u64,
    /// The file's exact logical length.
    logical: i64,
    /// Whether the header and meta block have been written at least once.
    initialized: bool,
}

/// Our `sqlite3_file`. The base must be first: SQLite casts between the two.
#[repr(C)]
struct File {
    base: ffi::sqlite3_file,
    /// The delegate's file object, allocated separately so its size is the delegate's business.
    inner: *mut ffi::sqlite3_file,
    /// `None` for a file this layer passes through — which, with `temp_store=MEMORY`, is only
    /// the write-ahead index that `locking_mode=EXCLUSIVE` prevents existing at all.
    state: *mut Sealed,
}

// The methods table. One instance, shared by every open file.
static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(x_close),
    xRead: Some(x_read),
    xWrite: Some(x_write),
    xTruncate: Some(x_truncate),
    xSync: Some(x_sync),
    xFileSize: Some(x_file_size),
    xLock: Some(x_lock),
    xUnlock: Some(x_unlock),
    xCheckReservedLock: Some(x_check_reserved_lock),
    xFileControl: Some(x_file_control),
    xSectorSize: Some(x_sector_size),
    xDeviceCharacteristics: Some(x_device_characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

/// Register the VFS. Idempotent, and never the default — a plaintext database is still a
/// legitimate thing for a test to open, and making this the default would seal one by accident.
pub fn register() -> Result<(), String> {
    static REGISTERED: OnceLock<Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            // SAFETY: `sqlite3_vfs_find(null)` returns the default VFS, which lives for the
            // life of the process.
            let default = unsafe { ffi::sqlite3_vfs_find(std::ptr::null()) };
            if default.is_null() {
                return Err("SQLite has no default VFS".to_owned());
            }
            let name = CString::new(VFS_NAME).map_err(|_| "the VFS name has a NUL")?;
            // Leaked deliberately: SQLite keeps the pointer for the life of the process, and
            // the registration is never undone.
            let name = Box::leak(name.into_boxed_c_str());
            // SAFETY: read of a live VFS record.
            let inner = unsafe { &*default };
            let vfs = Box::leak(Box::new(ffi::sqlite3_vfs {
                iVersion: 1,
                // Our file, plus room for the delegate's — allocated separately, so only ours.
                szOsFile: i32::try_from(size_of::<File>()).unwrap_or(i32::MAX),
                mxPathname: inner.mxPathname,
                pNext: std::ptr::null_mut(),
                zName: name.as_ptr(),
                pAppData: default.cast::<c_void>(),
                xOpen: Some(x_open),
                xDelete: inner.xDelete,
                xAccess: inner.xAccess,
                xFullPathname: inner.xFullPathname,
                xDlOpen: inner.xDlOpen,
                xDlError: inner.xDlError,
                xDlSym: inner.xDlSym,
                xDlClose: inner.xDlClose,
                xRandomness: inner.xRandomness,
                xSleep: inner.xSleep,
                xCurrentTime: inner.xCurrentTime,
                xGetLastError: inner.xGetLastError,
                xCurrentTimeInt64: inner.xCurrentTimeInt64,
                xSetSystemCall: None,
                xGetSystemCall: None,
                xNextSystemCall: None,
            }));
            // SAFETY: the record is leaked, so it outlives the registration.
            let rc = unsafe { ffi::sqlite3_vfs_register(vfs, 0) };
            if rc == ffi::SQLITE_OK {
                Ok(())
            } else {
                Err(format!("sqlite3_vfs_register returned {rc}"))
            }
        })
        .clone()
}

/// The three pragmas this layer cannot work without, checked against a live connection.
///
/// Each fails silently. A memory-mapped read that bypassed `xRead` would hand the pager
/// ciphertext and look like corruption; a shared-mode write-ahead index would sit in the clear
/// beside a sealed database and look like nothing at all.
///
/// # Errors
/// Any of the three not holding.
pub fn assert_pragmas(conn: &rusqlite::Connection) -> Result<(), String> {
    let locking: String = conn
        .query_row("PRAGMA locking_mode", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if !locking.eq_ignore_ascii_case("exclusive") {
        return Err(format!(
            "locking_mode is `{locking}`, so the write-ahead index is a file this layer does \
             not see — the pages would be sealed and the index describing them would not"
        ));
    }
    let mmap: i64 = conn
        .query_row("PRAGMA mmap_size", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if mmap != 0 {
        return Err(format!(
            "mmap_size is {mmap}; a memory-mapped read bypasses this layer and hands the pager \
             ciphertext"
        ));
    }
    let temp: i64 = conn
        .query_row("PRAGMA temp_store", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if temp != 2 {
        return Err(format!(
            "temp_store is {temp} rather than MEMORY; a spill file has no key and would be \
             written in the clear"
        ));
    }
    let page: i64 = conn
        .query_row("PRAGMA page_size", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if page != LOGICAL_BLOCK as i64 {
        return Err(format!(
            "page_size is {page} rather than {LOGICAL_BLOCK}; one engine page must be one \
             sealed block or every write is a read-modify-write of two"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// The VFS methods.
// ---------------------------------------------------------------------------------------

unsafe extern "C" fn x_open(
    vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    unsafe {
        let default = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
        let inner_size = usize::try_from((*default).szOsFile).unwrap_or(0);
        // Zeroed rather than uninitialised: SQLite reads `pMethods` before anything else and a
        // stale value there is a jump through garbage.
        let inner = ffi::sqlite3_malloc(i32::try_from(inner_size).unwrap_or(0)).cast::<u8>();
        if inner.is_null() {
            return ffi::SQLITE_NOMEM;
        }
        std::ptr::write_bytes(inner, 0, inner_size);
        let inner = inner.cast::<ffi::sqlite3_file>();

        let rc = ((*default).xOpen.expect("the default VFS opens files"))(
            default, name, inner, flags, out_flags,
        );
        if rc != ffi::SQLITE_OK {
            ffi::sqlite3_free(inner.cast::<c_void>());
            return rc;
        }

        let sealed = sealed_state(name, inner, flags);
        let state = match sealed {
            Ok(s) => s,
            Err(rc) => {
                if let Some(close) = (*(*inner).pMethods).xClose {
                    close(inner);
                }
                ffi::sqlite3_free(inner.cast::<c_void>());
                return rc;
            }
        };

        let f = file.cast::<File>();
        (*f).base.pMethods = &raw const METHODS;
        (*f).inner = inner;
        (*f).state = state;
        ffi::SQLITE_OK
    }
}

/// Decide whether this file is sealed, and set it up if so.
///
/// A file with no key is passed through. That is not a hole: with `temp_store=MEMORY` and
/// `locking_mode=EXCLUSIVE` the only files SQLite opens for an account are the database, its
/// write-ahead log and its rollback journal, and all three resolve to the same key.
unsafe fn sealed_state(
    name: ffi::sqlite3_filename,
    inner: *mut ffi::sqlite3_file,
    flags: c_int,
) -> Result<*mut Sealed, c_int> {
    let sealable = flags
        & (ffi::SQLITE_OPEN_MAIN_DB | ffi::SQLITE_OPEN_WAL | ffi::SQLITE_OPEN_MAIN_JOURNAL)
        != 0;
    if !sealable || name.is_null() {
        return Ok(std::ptr::null_mut());
    }
    // SAFETY: SQLite hands a NUL-terminated path for every named open.
    let path = unsafe { CStr::from_ptr(name.cast::<c_char>()) };
    let Ok(path) = path.to_str() else {
        return Ok(std::ptr::null_mut());
    };
    let database = database_of(Path::new(path));
    let Some(key_id) = key_id_of(&database) else {
        return Ok(std::ptr::null_mut());
    };

    // SAFETY: the delegate opened successfully, so its methods are live.
    let size = unsafe { raw_size(inner) }.map_err(|_| ffi::SQLITE_IOERR_FSTAT)?;

    let state = if size == 0 {
        // A new file. Nothing is written yet — the header lands with the first write, so that
        // a file SQLite opens and never writes to is not left as a Sift store with no content.
        // A key identifier derived from the key rather than zeroed, so that D-22's rotation
        // and FR-4's "recognisable rather than merely unreadable" both hold from the first
        // write. The caller registered the key under this path, so the two agree.
        let header = Header::new(key_id);
        Sealed {
            cipher: cipher_for(&database, &header).ok_or(ffi::SQLITE_NOTADB)?,
            reserved: 0,
            logical: 0,
            initialized: false,
        }
    } else {
        let mut bytes = [0u8; sift_crypto::page::HEADER_LEN];
        // SAFETY: as above.
        unsafe { raw_read(inner, &mut bytes, 0) }.map_err(|_| ffi::SQLITE_IOERR_READ)?;
        let header = Header::decode(&bytes).map_err(|_| ffi::SQLITE_NOTADB)?;
        let cipher = cipher_for(&database, &header).ok_or(ffi::SQLITE_NOTADB)?;

        let mut meta = vec![0u8; META_BLOCK as usize];
        // SAFETY: as above.
        unsafe { raw_read(inner, &mut meta, CRYPTO_HEADER) }.map_err(|_| ffi::SQLITE_IOERR_READ)?;
        // **The one place a wrong key is caught.** A file that fails to authenticate is
        // discarded and never repaired, per D-73 — so this is a refusal rather than a fallback.
        let plain = cipher
            .open(META_PAGE, &meta)
            .map_err(|_| ffi::SQLITE_NOTADB)?;
        let mut logical = [0u8; 8];
        logical.copy_from_slice(&plain[..8]);

        Sealed {
            cipher,
            reserved: header.counter_high_water,
            logical: i64::from_be_bytes(logical),
            initialized: true,
        }
    };
    Ok(Box::into_raw(Box::new(state)))
}

unsafe extern "C" fn x_close(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        let mut rc = ffi::SQLITE_OK;
        if !(*f).state.is_null() {
            let state = &mut *(*f).state;
            if state.initialized && flush_meta((*f).inner, state).is_err() {
                rc = ffi::SQLITE_IOERR_WRITE;
            }
            drop(Box::from_raw((*f).state));
            (*f).state = std::ptr::null_mut();
        }
        let inner = (*f).inner;
        if !inner.is_null() {
            if let Some(close) = (*(*inner).pMethods).xClose {
                let r = close(inner);
                if rc == ffi::SQLITE_OK {
                    rc = r;
                }
            }
            ffi::sqlite3_free(inner.cast::<c_void>());
            (*f).inner = std::ptr::null_mut();
        }
        rc
    }
}

unsafe extern "C" fn x_read(
    file: *mut ffi::sqlite3_file,
    buf: *mut c_void,
    amt: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        if (*f).state.is_null() {
            return ((*(*(*f).inner).pMethods).xRead.expect("xRead"))((*f).inner, buf, amt, offset);
        }
        let state = &mut *(*f).state;
        let amt = usize::try_from(amt).unwrap_or(0);
        let out = std::slice::from_raw_parts_mut(buf.cast::<u8>(), amt);

        // Past the end is a short read, and SQLite requires the tail be zeroed rather than
        // left as whatever the buffer held.
        if offset >= state.logical {
            out.fill(0);
            return ffi::SQLITE_IOERR_SHORT_READ;
        }
        let available = usize::try_from(state.logical - offset).unwrap_or(0);
        let readable = available.min(amt);

        let mut done = 0usize;
        while done < readable {
            let at = offset + done as i64;
            let block = at / LOGICAL_BLOCK as i64;
            let within = usize::try_from(at % LOGICAL_BLOCK as i64).unwrap_or(0);
            let plain = match read_block(&mut *(*f).inner, state, block) {
                Ok(p) => p,
                Err(rc) => return rc,
            };
            let take = (LOGICAL_BLOCK - within).min(readable - done);
            out[done..done + take].copy_from_slice(&plain[within..within + take]);
            done += take;
        }
        if done < amt {
            out[done..].fill(0);
            return ffi::SQLITE_IOERR_SHORT_READ;
        }
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn x_write(
    file: *mut ffi::sqlite3_file,
    buf: *const c_void,
    amt: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        if (*f).state.is_null() {
            return ((*(*(*f).inner).pMethods).xWrite.expect("xWrite"))(
                (*f).inner,
                buf,
                amt,
                offset,
            );
        }
        let state = &mut *(*f).state;
        let amt = usize::try_from(amt).unwrap_or(0);
        let src = std::slice::from_raw_parts(buf.cast::<u8>(), amt);

        let mut done = 0usize;
        while done < amt {
            let at = offset + done as i64;
            let block = at / LOGICAL_BLOCK as i64;
            let within = usize::try_from(at % LOGICAL_BLOCK as i64).unwrap_or(0);
            let take = (LOGICAL_BLOCK - within).min(amt - done);

            // A partial block is read-modify-write. The engine writes whole pages to the
            // database, so this is the log and the journal — and they write at offsets that
            // are not block-aligned, which is the whole reason this layer is byte-oriented.
            let mut plain = if within != 0 || take != LOGICAL_BLOCK {
                match read_block(&mut *(*f).inner, state, block) {
                    Ok(p) => p,
                    Err(rc) => return rc,
                }
            } else {
                vec![0u8; LOGICAL_BLOCK]
            };
            plain[within..within + take].copy_from_slice(&src[done..done + take]);

            if let Err(rc) = write_block(&mut *(*f).inner, state, block, &plain) {
                return rc;
            }
            done += take;
        }

        let end = offset + amt as i64;
        if end > state.logical {
            state.logical = end;
        }
        state.initialized = true;
        if flush_meta((*f).inner, state).is_err() {
            return ffi::SQLITE_IOERR_WRITE;
        }
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn x_truncate(file: *mut ffi::sqlite3_file, size: i64) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        if (*f).state.is_null() {
            return ((*(*(*f).inner).pMethods).xTruncate.expect("xTruncate"))((*f).inner, size);
        }
        let state = &mut *(*f).state;
        state.logical = size.min(state.logical);
        // Rounded up without `div_ceil`, which is not stable on the 1.88 floor.
        let blocks = (size + LOGICAL_BLOCK as i64 - 1) / LOGICAL_BLOCK as i64;
        let physical = DATA_START + blocks * SEALED_BLOCK as i64;
        let rc = ((*(*(*f).inner).pMethods).xTruncate.expect("xTruncate"))((*f).inner, physical);
        if rc != ffi::SQLITE_OK {
            return rc;
        }
        if flush_meta((*f).inner, state).is_err() {
            return ffi::SQLITE_IOERR_WRITE;
        }
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn x_sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        if !(*f).state.is_null() {
            let state = &mut *(*f).state;
            if state.initialized && flush_meta((*f).inner, state).is_err() {
                return ffi::SQLITE_IOERR_WRITE;
            }
        }
        ((*(*(*f).inner).pMethods).xSync.expect("xSync"))((*f).inner, flags)
    }
}

unsafe extern "C" fn x_file_size(file: *mut ffi::sqlite3_file, size: *mut i64) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        if (*f).state.is_null() {
            return ((*(*(*f).inner).pMethods).xFileSize.expect("xFileSize"))((*f).inner, size);
        }
        // The **logical** length. Rounding up to the block would report 4096 for a 32-byte
        // write-ahead log header, and recovery would read past the end of what was written.
        *size = (*(*f).state).logical;
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn x_lock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        ((*(*(*f).inner).pMethods).xLock.expect("xLock"))((*f).inner, level)
    }
}

unsafe extern "C" fn x_unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        ((*(*(*f).inner).pMethods).xUnlock.expect("xUnlock"))((*f).inner, level)
    }
}

unsafe extern "C" fn x_check_reserved_lock(file: *mut ffi::sqlite3_file, out: *mut c_int) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        ((*(*(*f).inner).pMethods)
            .xCheckReservedLock
            .expect("xCheckReservedLock"))((*f).inner, out)
    }
}

unsafe extern "C" fn x_file_control(
    file: *mut ffi::sqlite3_file,
    op: c_int,
    arg: *mut c_void,
) -> c_int {
    unsafe {
        let f = file.cast::<File>();
        ((*(*(*f).inner).pMethods)
            .xFileControl
            .expect("xFileControl"))((*f).inner, op, arg)
    }
}

unsafe extern "C" fn x_sector_size(_file: *mut ffi::sqlite3_file) -> c_int {
    // The block, not the device's sector. What this layer writes atomically — if it writes
    // anything atomically at all — is a sealed block.
    i32::try_from(SEALED_BLOCK).unwrap_or(4096)
}

unsafe extern "C" fn x_device_characteristics(_file: *mut ffi::sqlite3_file) -> c_int {
    // **Zero, deliberately.** The delegate's answer would be about the device's raw sectors,
    // and a sealed block is not one: a page write here is a read, an AEAD seal and a write of
    // 4120 bytes at an offset the device knows nothing about. Passing through a claim of
    // atomic write, safe append or powersafe overwrite would let SQLite skip protection it
    // needs, and the loss would appear as corruption after a power cut — the failure mode
    // that is hardest to attribute and impossible to reproduce on demand.
    0
}

// ---------------------------------------------------------------------------------------
// Block translation.
// ---------------------------------------------------------------------------------------

unsafe fn raw_size(inner: *mut ffi::sqlite3_file) -> Result<i64, c_int> {
    unsafe {
        let mut size = 0i64;
        let rc = ((*(*inner).pMethods).xFileSize.ok_or(ffi::SQLITE_IOERR)?)(inner, &raw mut size);
        if rc == ffi::SQLITE_OK {
            Ok(size)
        } else {
            Err(rc)
        }
    }
}

unsafe fn raw_read(inner: *mut ffi::sqlite3_file, into: &mut [u8], at: i64) -> Result<(), c_int> {
    unsafe {
        let rc = ((*(*inner).pMethods).xRead.ok_or(ffi::SQLITE_IOERR)?)(
            inner,
            into.as_mut_ptr().cast::<c_void>(),
            i32::try_from(into.len()).unwrap_or(0),
            at,
        );
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(rc)
        }
    }
}

unsafe fn raw_write(inner: *mut ffi::sqlite3_file, from: &[u8], at: i64) -> Result<(), c_int> {
    unsafe {
        let rc = ((*(*inner).pMethods).xWrite.ok_or(ffi::SQLITE_IOERR)?)(
            inner,
            from.as_ptr().cast::<c_void>(),
            i32::try_from(from.len()).unwrap_or(0),
            at,
        );
        if rc == ffi::SQLITE_OK {
            Ok(())
        } else {
            Err(rc)
        }
    }
}

/// One block, decrypted. A block past the end of the file reads as zeroes, which is what a
/// sparse region of an ordinary file does.
unsafe fn read_block(
    inner: *mut ffi::sqlite3_file,
    state: &Sealed,
    block: i64,
) -> Result<Vec<u8>, c_int> {
    let at = DATA_START + block * SEALED_BLOCK as i64;
    // SAFETY: the delegate is live for the life of the file.
    let size = unsafe { raw_size(inner) }?;
    if at + SEALED_BLOCK as i64 > size {
        return Ok(vec![0u8; LOGICAL_BLOCK]);
    }
    let mut sealed = vec![0u8; SEALED_BLOCK];
    // SAFETY: as above.
    unsafe { raw_read(inner, &mut sealed, at) }?;
    let number = u32::try_from(block).map_err(|_| ffi::SQLITE_IOERR_READ)?;
    // A block that does not authenticate is not repaired. D-73: a file that fails to
    // authenticate is discarded, and reporting it as an I/O error is what makes the engine
    // stop rather than carry on with something it invented.
    state
        .cipher
        .open(number, &sealed)
        .map_err(|_| ffi::SQLITE_IOERR_READ)
}

unsafe fn write_block(
    inner: *mut ffi::sqlite3_file,
    state: &mut Sealed,
    block: i64,
    plain: &[u8],
) -> Result<(), c_int> {
    // SAFETY: the delegate is live for the life of the file.
    unsafe { reserve(inner, state) }?;
    let number = u32::try_from(block).map_err(|_| ffi::SQLITE_IOERR_WRITE)?;
    let sealed = state
        .cipher
        .seal(number, plain)
        .map_err(|_| ffi::SQLITE_IOERR_WRITE)?;
    let at = DATA_START + block * SEALED_BLOCK as i64;
    // SAFETY: the delegate is live for the life of the file.
    unsafe { raw_write(inner, &sealed, at) }
}

/// Make sure the counter about to be issued is below a mark that is already on disk.
///
/// This is the whole of the nonce-reuse defence across a crash, and the order is the point: the
/// mark is written and synced **before** the counters it covers are used. A run that recorded
/// the mark afterwards would, after a crash, resume below counters it had already issued.
unsafe fn reserve(inner: *mut ffi::sqlite3_file, state: &mut Sealed) -> Result<(), c_int> {
    if state.cipher.high_water() + 1 < state.reserved {
        return Ok(());
    }
    let next = state.reserved.max(state.cipher.high_water()) + RESERVATION;
    let mut header = Header::new(state.cipher.key_id());
    header.counter_high_water = next;
    // SAFETY: the delegate is live for the life of the file.
    unsafe { raw_write(inner, &header.encode(), 0) }?;
    // SAFETY: as above. Synced, because a mark in the page cache is a mark that is not on disk
    // — and this whole mechanism is about what survives a power cut.
    unsafe {
        let rc =
            ((*(*inner).pMethods).xSync.ok_or(ffi::SQLITE_IOERR)?)(inner, ffi::SQLITE_SYNC_NORMAL);
        if rc != ffi::SQLITE_OK {
            return Err(rc);
        }
    }
    state.reserved = next;
    Ok(())
}

/// Write the header and the sealed length.
unsafe fn flush_meta(inner: *mut ffi::sqlite3_file, state: &mut Sealed) -> Result<(), c_int> {
    // SAFETY: the delegate is live for the life of the file.
    unsafe { reserve(inner, state) }?;

    let mut header = Header::new(state.cipher.key_id());
    header.counter_high_water = state.reserved;
    // SAFETY: as above.
    unsafe { raw_write(inner, &header.encode(), 0) }?;

    let mut payload = [0u8; 64];
    payload[..8].copy_from_slice(&state.logical.to_be_bytes());
    let sealed = state
        .cipher
        .seal(META_PAGE, &payload)
        .map_err(|_| ffi::SQLITE_IOERR_WRITE)?;
    // SAFETY: as above.
    unsafe { raw_write(inner, &sealed, CRYPTO_HEADER) }
}
