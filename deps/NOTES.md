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

---

## `aes-gcm`, `aes`, and the RustCrypto tree beneath them

**Reached by:** `sift-crypto`'s page format. Hostile-input adjacent rather than
hostile-input facing: it decrypts bytes an attacker with filesystem write access may have
altered, and the whole point of authenticating every page is that such bytes fail rather
than parse.

- **Unsafe.** Present, in the AES round functions and in `cpufeatures`' runtime detection.
  This is the one place in the tree where that is expected rather than a finding: a cipher
  that refuses to use AES-NI is a cipher that runs an order of magnitude slower, and the
  RustCrypto AES implementation is the one the Rust ecosystem has standardised on and
  audited. `sift-crypto` is itself one of the four crates permitted unsafe, so the
  dependency does not widen the exception.
- **Licence.** `Apache-2.0 OR MIT` throughout. Clears the allowlist.
- **Floor.** None above Sift's.
- **Threads, timers, sockets.** None.

## `blake3`

**Reached by:** D-23's content address and D-43's keyed derivation, over attacker-supplied
blob bytes.

- **Built with `default-features = false, features = ["std", "pure"]`.** `pure` turns off
  the hand-written SIMD assembly, and the reason is D-23's own: BLAKE3 was chosen "to avoid
  a hardware mandate rather than for speed", and the decision states plainly that **nothing
  hashes at idle** — hashing a body is a rounding error against NFR-3's 80 ms, and hashing
  a large attachment is under 2% of its transfer time. The assembly therefore buys
  throughput the design does not need, in exchange for a C toolchain in the build and an
  unsafe SIMD path on a surface that hashes attacker-influenced bytes. **Verified
  empirically** that no C is compiled: blake3's build script runs, its `out` directory is
  empty, and no `.o` or `libblake3*` is produced anywhere in `target/`.
- **`cc` and `find-msvc-tools` still appear in the approved list.** They are in the
  resolved graph because `cargo metadata` reports the union across platforms and features;
  neither is invoked. The gate being conservative here is the right direction — it reviews
  more than ships, never less.
- **Licence.** `CC0-1.0 OR Apache-2.0 OR Apache-2.0 WITH LLVM-exception`. Clears the
  allowlist.
- **Floor, threads, timers, sockets.** None.

## `subtle`

Constant-time comparison primitives. `BSD-3-Clause`. No unsafe of consequence, no floor,
no syscalls.

---

## `rusqlite` and the SQLite tree beneath it

**Reached by:** `sift-store`. D-21 names SQLite explicitly, which is the case
`docs/build/README.md` describes as a dependency named "where the choice **is** the
decision".

- **Unsafe.** Extensive, and expected: this is the C engine's FFI, which is **one of the
  four places `docs/architecture/overview.md` permits unsafe**. It does not widen the
  exception.
- **Is it on a hostile-input path?** No, and D-21 argues this deliberately: the mitigation
  for naming a C dependency is that **the database never parses attacker-controlled bytes**.
  Everything reaching it has already been through the rendering pipeline, and what is stored
  is normalized text and Sift's own identifiers.
- **`bundled`.** The engine is compiled from vendored source rather than linked against the
  platform's. This is the same argument D-15 makes for pinning WebKitGTK under Flatpak:
  version skew in a component that owns the on-disk format is worse than owning the update
  cadence. It also means the store format is Sift's rather than moving with an OS update.
  It does compile C, unlike blake3 — unavoidable, since SQLite *is* C.
- **Licence.** `MIT` for rusqlite; SQLite itself is public domain. Clears the allowlist.
- **Floor.** None above Sift's.
- **Threads, timers, sockets.** No sockets. SQLite's own threading is configured through
  pragmas rather than by spawning; nothing here arms a timer, which is what NFR-11 counts.

---

## `html5ever` and `markup5ever_rcdom`

**Reached by:** `sift-sanitize`. **The most hostile-input-facing dependency in the tree** —
every byte it parses was chosen by an unauthenticated sender with unlimited attempts.

- **Why a dependency at all, given D-26.** D-26 rejects adopting an existing
  general-purpose *sanitizer*, for a stated reason: they have no notion of rewriting every
  fetching position to an internal scheme (I2), and no CSS pipeline at all. It does **not**
  reject the tree builder — it requires one, and requires it be *spec-conformant*, because
  I8 needs the parser to implement the same algorithm the rendering engine does and NFR-40's
  dual-parser divergence test needs the same property. Writing a second conformant HTML5
  tree builder would be building the thing whose correctness is hardest to establish and
  whose bugs are exactly the mutation-XSS class I8 exists to close. The allowlist policy
  over it is Sift's own and lives in this crate.
- **Unsafe.** Present, in the string interning and tendril machinery. This is the one place
  in the tree where that deserves ongoing attention rather than a one-time answer, and it is
  the argument for NFR-40 method 4 — fuzzing seeded with the fidelity corpus — being a real
  obligation rather than a nice-to-have.
- **Licence.** `MIT OR Apache-2.0`. Clears the allowlist.
- **Floor, threads, timers, sockets.** No floor above Sift's. No network, no filesystem, no
  threads.

**A defect found in it while wiring this up, recorded because it will recur.** `Node`'s
`Drop` is iterative so that a deeply nested tree does not overflow the stack, and it
achieves that by **emptying the children of every node it walks** — including nodes that are
still alive and referenced elsewhere. Any code that reparents a subtree and then lets the old
parent be released will silently lose that subtree's contents. `sanitize.rs` takes the
children out of an unwrapped element rather than cloning them, and says why at the site.

---

## `adblock`

**Reached by:** `sift-block`. D-10 names it: "an established Rust filter engine — the one
powering a shipping browser's native blocker" — as the authority, with rules compiled into
the web engine's own content-rule format as an independent backstop. The `content-blocking`
feature performs that conversion, so **both halves of D-10 come from one source**, which is
what makes a disagreement between them a real signal rather than two parsers differing about
syntax.

- **Licence: MPL-2.0, and it tripped the gate.** That is the gate working, and the
  resolution is recorded in `deny.toml` rather than here: the first version of the allowlist
  treated "copyleft" as a synonym for "incompatible with the store's channel", which is
  wrong for MPL. The distinction that matters is between a licence that restricts *the
  Larger Work's* terms (the GPL family, and LGPL's relinking requirement) and one that
  attaches obligations only to its own files. **This is exactly the shape Q-21 predicted** —
  a vendored crate making the channel decision — and it is worth noting that the crate in
  question is the one D-10 names, so a stricter list would have silently overruled a
  decision the specification already made.
- **Unsafe.** Present. It is a matcher over attacker-supplied URLs rather than a parser of
  attacker-supplied *structure*, which is a materially smaller surface than the HTML tree
  builder — but it is on the hostile-input path and belongs in NFR-40 method 4's fuzzing
  scope.
- **Floor, threads, timers, sockets.** No floor above Sift's. No network of its own: it
  answers questions about URLs and never fetches one, which is the property that lets the
  broker remain the only component in the core that fetches anything.
- **Size.** It is the largest single addition to the tree so far. R-12's warning is about
  what Sift *builds*; this is the other side of that ledger, and the count in
  `deps/approved.txt` is the only place it is visible.

---

## `cssparser`

**Reached by:** `sift-css`. The same argument D-26 makes for the HTML tree builder, applied
to CSS — this is attacker-controlled input, and the tokenizer has to be the specification's
rather than a reading of it.

- **Why a dependency, given D-27.** D-27 commits to *resolving the cascade* — selector
  matching, specificity, importance, shorthand expansion, media-query evaluation,
  inheritance. **None of that is here**; it is `sift-css`, and it is the largest single
  commitment in the pipeline. What this crate supplies is CSS Syntax Level 3 tokenization,
  which is the part where hostile input is dangerous and where a hand-rolled reading would
  differ from the engine rendering the same document beside us.
- **Unsafe.** Present, in tokenizer fast paths. On the hostile-input path, and in NFR-40
  method 4's fuzzing scope alongside the HTML builder.
- **Licence.** `MPL-2.0`, admitted for the reason recorded in `deny.toml`.
- **Floor, threads, timers, sockets.** None.

---

## The toolchain floor, and the crate that moved it

`adblock` raised Sift's floor from **1.85 to 1.88**. It uses let-chains, stabilised in 1.88,
and **declares no `rust-version` of its own** — so nothing announced the change. The
per-change job that builds against the stated floor is the only reason it was caught rather
than discovered by a contributor on an older toolchain.

This is the case `docs/build/workspace.md` anticipates in one sentence: *a vendored
dependency's own floor becomes Sift's on the day it is vendored.* Two things make it worth
recording rather than just fixing:

- **The dependency was not optional.** D-10 names that engine, so declining it would have
  meant declining the decision.
- **The floor moved without a declaration.** A crate that declares its floor makes this a
  visible diff in `deps/approved.txt`; one that does not makes it a build failure somewhere
  else, later, on somebody else's machine.

The rule that a build failing on the floor is a defect rather than an invitation to raise it
still holds — it is about **Sift's own code**. When a vendored dependency genuinely needs
more, the floor moves, deliberately, with the reason written down. This is that.

## The TLS stack, and the trust store D-30 requires

`rustls`, `ring`, `rustls-platform-verifier` and the platform crates under them arrived with
`sift-http`. There is no way to reach a provider without them and the questions the gate asks
have real answers here rather than shrugs.

**Hostile-input path: yes, entirely.** Everything below `sift-http` parses bytes from
somebody else's computer. That is why the HTTP layer above them is hand-written and bounded
by `docs/limits.md` rather than borrowed: a general-purpose client's limits are its own.

**Sockets and threads: one outbound connect, no threads, no timers.** `sift-http` calls
`TcpStream::connect` and nothing else touches the network stack. NFR-24 is checked by
`cargo xtask invariants` over the source, and the crate states the property in a `const fn`
so it is greppable.

**The crypto provider is `ring` rather than `aws-lc-rs`.** The latter needs a C toolchain and
CMake at build time, which a vendored Flatpak build — no network, per D-33 — cannot rely on.

**`webpki-root-certs` and `jni` appear in the resolve graph and compile into nothing Sift
ships.** They are target-conditional dependencies of the platform verifier, for Android and
wasm. The first is a **bundled root store**, which is exactly what D-30 rejects, so its
absence from the shipped targets is now a gate rather than an observation:
`cargo xtask deps` resolves the tree for each of D-46's three targets and fails if either
name appears. Verified to fire by adding `webpki-roots` and watching it.

On macOS the verifier is `security-framework` — the platform's own. On Linux it is
`rustls-native-certs`, which reads the system store. Both are D-30.

## `flate2` — and why the decompressed length is capped twice

`accept-encoding: gzip` is worth roughly five times on a JSON change feed, which matters
directly to FR-35 and FR-36 on a metered link. The pure-Rust backend (`miniz_oxide`) is
selected explicitly: the C one would be a second build-time toolchain requirement for the
same reason `ring` was chosen over `aws-lc-rs`.

**A client that bounded only the transfer would have admitted a decompression bomb inside
NFR-39's ceiling.** So `sift-http` caps the decompressed output at L-13 as well as the
transferred bytes, and there is a test that builds a 4 MB bomb under 64 KB and watches the
cap refuse it.

## `sha2` and `getrandom` — PKCE, and nothing else

The S256 challenge names its hash in the parameter sent to the provider, so the algorithm is
not a choice. RustCrypto is the family `aes-gcm` already brought in under D-76, so this adds
a hash rather than a second cryptographic ecosystem. The implementation is checked against
RFC 7636's own worked example rather than against itself.

`getrandom` generates the verifier and D-36's state parameter. Where it fails, the flow
**refuses** rather than substituting anything: D-71 makes an absent security guarantee a
refusal, and a predictable verifier is a PKCE exchange that proves nothing.

## `serde_json` and `base64` — the Gmail wire format

Every byte a provider returns is attacker-influenced: a subject, a display name and a snippet
are all written by whoever sent the mail. Parsed with a real parser rather than a hand-rolled
one, for the same reason D-26 chose a spec-conformant HTML tree builder.

`base64` was already in the tree under `adblock`; this is the first direct use.
