//! Regenerates the byte-for-byte format fixture.
//!
//! Run with `cargo run -p sift-crypto --example bless-fixtures`.
//!
//! **Regenerating is almost never the right response to a failing fixture test.** The
//! fixture is the format's definition, and every store already written is in the format it
//! records. A change to these bytes is a format version bump and a migration, not an edit.
//! The legitimate reasons to run this are exactly two: the fixture does not exist yet, and
//! a deliberate format version bump has already been made.

use sift_crypto::page::{Header, KeyId, PageCipher, PageKey};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn main() {
    // Fixed, unremarkable inputs. They are not secret and are not meant to be: the point
    // is that any implementation of this format, on any platform, produces the same output
    // for them.
    let key = [0x42u8; 32];
    let key_id = [0xABu8; 16];
    let plaintext = b"SQLite format 3\0 a page of a Sift store".to_vec();
    let page_number = 7u32;
    let counter = 42u64;

    let mut header = Header::new(KeyId(key_id));
    header.counter_high_water = counter;

    let cipher = PageCipher::new(&PageKey::from_bytes(key), &Header::new(KeyId(key_id)));
    let sealed = cipher
        .seal_with_counter(page_number, counter, &plaintext)
        .expect("seal");

    let out = format!(
        "# The Sift page format, version 1 — the definition, in bytes.\n\
         #\n\
         # D-75 requires one portable interface producing one byte-identical on-disk format\n\
         # on macOS and on Linux, and docs/build/verification.md requires this check exist\n\
         # from the first commit rather than after the first divergence. R-16 is why: two\n\
         # implementations of one format can disagree, and the disagreement is silent — it\n\
         # writes a store the other platform cannot read, or reads one incorrectly.\n\
         #\n\
         # A platform backend that produces different bytes for these inputs is wrong,\n\
         # however audited its cipher is.\n\
         #\n\
         # Inputs: page number {page_number}, write counter {counter}.\n\
         # Regenerate with `cargo run -p sift-crypto --example bless-fixtures` — but read\n\
         # that file's header first, because regenerating is almost never the right answer.\n\
         \n\
         key = {}\n\
         key_id = {}\n\
         plaintext = {}\n\
         header = {}\n\
         sealed = {}\n",
        hex(&key),
        hex(&key_id),
        hex(&plaintext),
        hex(&header.encode()),
        hex(&sealed),
    );

    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/page-format-v1.txt");
    std::fs::create_dir_all(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures")).expect("mkdir");
    std::fs::write(path, out).expect("write fixture");
    eprintln!("wrote {path}");
}
