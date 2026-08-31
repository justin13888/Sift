# Dependency review notes

`deps/approved.txt` records *what* is vendored. This records *why it was allowed*, for
the crates where the third gate's questions had a non-obvious answer.

The questions, from `docs/build/workspace.md`:

1. Is it on a hostile-input path, and if so, is it a thin wrapper over unsafe parsing?
2. Does its licence clear the allowlist in `deny.toml`?
3. Does it raise the toolchain floor?
4. Does it start a thread, arm a timer, or open a socket of its own?

Questions 1 and 4 are the ones a licence checker cannot answer. Question 4 matters because
neither a thread nor a timer is visible in a diff, and NFR-11 counts wakeups while NFR-24
admits no listening socket for any purpose.

---

## `unicode-normalization`, `unicode-segmentation`

**Reached by:** NFR-54's normalizer in `sift-foundation`, which is on the hostile-input
path by construction — every string it touches is a sender-controlled display name,
subject, snippet, folder or tag name, or attachment name. The threat model notes this path
reaches native chrome and that **no sanitizer invariant sees it**, so these two get the
same scrutiny a MIME dependency would.

- **Unsafe.** `unicode-segmentation` is `#![deny(missing_docs, unsafe_code)]` with no
  `allow` sites at all. `unicode-normalization` carries the same crate-level `deny` with
  three narrowly scoped `#[allow(unsafe_code)]` sites, all in Hangul syllable composition
  and decomposition, all `char::from_u32_unchecked` on values derived arithmetically from
  the Hangul block rather than from input. That is not the shape the gate is looking for —
  a thin wrapper over an unsafe parser — and both are table-driven pure Rust.
- **Licence.** `MIT OR Apache-2.0` for both. Clears the allowlist.
- **Floor.** `unicode-segmentation` declares `rust-version = 1.85.0`, which is **exactly**
  Sift's stated floor. It does not raise it, and it is now the crate that pins it: a future
  upgrade that moves its floor moves Sift's, and that is a deliberate change rather than a
  consequence. `unicode-normalization` declares 1.36.
- **Threads, timers, sockets.** None. No `std::net`, no `std::fs`, no `std::process`, no
  `thread::spawn`.

**Why not write the normalization by hand.** The alternative is Sift owning a Unicode
table and its update cadence, in a product that cannot self-update (D-33). D-26 chose to
own the HTML tree builder precisely because no crate did what I2 needed; nothing analogous
applies here — NFC is NFC, and the correctness bar is a published standard rather than a
Sift-specific policy.

## `tinyvec`, `tinyvec_macros`

Transitive, through `unicode-normalization`.

- **Unsafe.** `#![forbid(unsafe_code)]` — being a `SmallVec` alternative that forbids
  unsafe is the crate's entire reason to exist.
- **Licence.** `Zlib OR Apache-2.0 OR MIT`. Clears the allowlist.
- **Floor.** Declares none.
- **Threads, timers, sockets.** None.
