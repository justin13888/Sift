//! D-5, D-81 — D-81's tokenizer, registered with the engine's full-text extension.
//!
//! # Why the engine runs Sift's tokenizer rather than its own
//!
//! D-5 puts the index inside the embedded database so it cannot drift from the messages. D-81
//! then fixes what tokenization means: Unicode word segmentation, diacritic folding, the
//! normalization form NFR-54 applies, and a trigram index **required** for the scripts word
//! segmentation cannot segment. The engine's built-in tokenizers each do part of that — and
//! none of them does the trigram half scoped to those scripts, which is the difference between
//! search working in Japanese and returning nothing.
//!
//! So the table is declared over a tokenizer named [`TOKENIZER`], and this registers
//! [`sift_index::ingest::tokenize_spanned`] under that name on every connection. The engine
//! then calls the same function for the text it stores and for every query it is asked, so
//! what was indexed and what is looked up are one reading of the text rather than two.
//!
//! A connection without the registration cannot read or write the table at all — the engine
//! refuses with "no such tokenizer" rather than guessing — which is why [`register`] runs
//! in the one place every account connection passes through, before the schema is touched.

use rusqlite::{Connection, ffi};
use std::ffi::{c_char, c_int, c_void};

/// The name the full-text table is declared with.
pub const TOKENIZER: &str = "sift";

/// Register D-81's tokenizer on a connection.
///
/// # Errors
/// The engine was built without its full-text extension, or refused the registration.
pub fn register(conn: &Connection) -> rusqlite::Result<()> {
    let api = fts5_api(conn)?;
    let mut module = ffi::fts5_tokenizer {
        xCreate: Some(x_create),
        xDelete: Some(x_delete),
        xTokenize: Some(x_tokenize),
    };
    // SAFETY: `api` was handed out by the engine for this connection and lives as long as it.
    // `xCreateTokenizer` copies the module structure before returning, so a stack value is
    // enough; the name is a static C string.
    let rc = unsafe {
        let Some(create) = (*api).xCreateTokenizer else {
            return Err(refused(ffi::SQLITE_ERROR, "no tokenizer registration"));
        };
        create(
            api,
            c"sift".as_ptr(),
            std::ptr::null_mut(),
            &raw mut module,
            None,
        )
    };
    if rc == ffi::SQLITE_OK {
        Ok(())
    } else {
        Err(refused(rc, "the tokenizer was refused"))
    }
}

fn refused(code: c_int, message: &str) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(ffi::Error::new(code), Some(message.to_owned()))
}

/// The extension's API pointer, which the engine hands out through a pointer-typed binding.
fn fts5_api(conn: &Connection) -> rusqlite::Result<*mut ffi::fts5_api> {
    let mut api: *mut ffi::fts5_api = std::ptr::null_mut();
    // SAFETY: the handle is the connection's own and outlives the statement, which is
    // finalized before this returns on every path. The pointer binding writes one pointer into
    // `api`, which is live for the whole step.
    unsafe {
        let db = conn.handle();
        let mut stmt: *mut ffi::sqlite3_stmt = std::ptr::null_mut();
        let sql = c"SELECT fts5(?1)";
        let rc = ffi::sqlite3_prepare_v2(db, sql.as_ptr(), -1, &raw mut stmt, std::ptr::null_mut());
        if rc != ffi::SQLITE_OK {
            return Err(refused(rc, "the full-text extension is absent"));
        }
        ffi::sqlite3_bind_pointer(
            stmt,
            1,
            (&raw mut api).cast::<c_void>(),
            c"fts5_api_ptr".as_ptr(),
            None,
        );
        ffi::sqlite3_step(stmt);
        ffi::sqlite3_finalize(stmt);
    }
    if api.is_null() {
        return Err(refused(
            ffi::SQLITE_ERROR,
            "the full-text extension is absent",
        ));
    }
    Ok(api)
}

/// The tokenizer holds no state, so every instance is the same non-null marker.
static INSTANCE: u8 = 0;

unsafe extern "C" fn x_create(
    _context: *mut c_void,
    _arguments: *mut *const c_char,
    _count: c_int,
    out: *mut *mut ffi::Fts5Tokenizer,
) -> c_int {
    // SAFETY: the engine passes a valid out-pointer. The marker is never dereferenced.
    unsafe {
        *out = (&raw const INSTANCE)
            .cast_mut()
            .cast::<ffi::Fts5Tokenizer>();
    }
    ffi::SQLITE_OK
}

unsafe extern "C" fn x_delete(_tokenizer: *mut ffi::Fts5Tokenizer) {}

type Emit = unsafe extern "C" fn(
    context: *mut c_void,
    flags: c_int,
    token: *const c_char,
    length: c_int,
    start: c_int,
    end: c_int,
) -> c_int;

unsafe extern "C" fn x_tokenize(
    _tokenizer: *mut ffi::Fts5Tokenizer,
    context: *mut c_void,
    _flags: c_int,
    text: *const c_char,
    length: c_int,
    emit: Option<Emit>,
) -> c_int {
    let Some(emit) = emit else {
        return ffi::SQLITE_MISUSE;
    };
    let bytes: &[u8] = match usize::try_from(length) {
        Ok(0) | Err(_) => &[],
        // SAFETY: the engine passes `length` readable bytes at `text`.
        Ok(n) if !text.is_null() => unsafe { std::slice::from_raw_parts(text.cast::<u8>(), n) },
        Ok(_) => &[],
    };
    // A panic must not unwind into C. The tokenizer is written not to panic on any input — its
    // own tests feed it hostile text — and this is what makes that a fault rather than
    // undefined behaviour if it ever does.
    let Ok(tokens) = std::panic::catch_unwind(|| {
        sift_index::ingest::tokenize_spanned(&String::from_utf8_lossy(bytes))
    }) else {
        return ffi::SQLITE_ERROR;
    };
    let clamp = |offset: usize| c_int::try_from(offset.min(bytes.len())).unwrap_or(length);
    for (token, span) in tokens {
        let Ok(size) = c_int::try_from(token.text.len()) else {
            continue;
        };
        // SAFETY: the callback and its context are the engine's own, and the token's bytes
        // are live for the duration of the call, which is all the engine asks.
        let rc = unsafe {
            emit(
                context,
                0,
                token.text.as_ptr().cast::<c_char>(),
                size,
                clamp(span.start),
                clamp(span.end),
            )
        };
        if rc != ffi::SQLITE_OK {
            return rc;
        }
    }
    ffi::SQLITE_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        register(&conn).expect("register");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE t USING fts5(body, tokenize = 'sift');
             INSERT INTO t (rowid, body) VALUES
                (1, 'Le café est fermé'),
                (2, '東京都の天気予報です'),
                (3, 'don''t stop');",
        )
        .expect("table");
        conn
    }

    fn hits(conn: &Connection, expression: &str) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT rowid FROM t WHERE t MATCH ?1 ORDER BY rowid")
            .expect("prepare");
        stmt.query_map([expression], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    #[test]
    fn the_engine_reads_text_the_way_d81_does() {
        let conn = table();
        // Diacritic folding, both directions.
        assert_eq!(hits(&conn, "\"cafe\""), vec![1]);
        assert_eq!(hits(&conn, "\"FERME\""), vec![1]);
        // A substring of an unsegmentable run, by its trigrams.
        assert_eq!(hits(&conn, "\"天気予報\""), vec![2]);
        assert_eq!(hits(&conn, "\"京都\"*"), vec![2]);
        // An apostrophe is inside a word, not between two.
        assert_eq!(hits(&conn, "\"don't\""), vec![3]);
        assert!(hits(&conn, "\"stop don't\"").is_empty());
    }

    #[test]
    fn a_decomposed_query_finds_a_composed_document() {
        // NFR-54's form on both sides: the same string composed and decomposed is one word.
        let conn = table();
        assert_eq!(hits(&conn, "\"ferme\u{0301}\""), vec![1]);
    }

    #[test]
    fn a_connection_without_the_tokenizer_is_refused_rather_than_guessing() {
        let conn = Connection::open_in_memory().expect("open");
        assert!(
            conn.execute_batch("CREATE VIRTUAL TABLE t USING fts5(body, tokenize = 'sift')")
                .is_err()
        );
    }
}
