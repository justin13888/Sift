# The macOS shell

D-61: **the platform's own application toolchain owns the bundle**, and the Rust core is
built into a static library it links. The Rust workspace never produces the `.app`.

```
cargo build --release -p sift-abi        # produces libsift_abi.a
cargo xtask header                       # asserts the committed header matches the ABI
swift build -c release --package-path shells/macos
```

## What this costs, from D-61

- The macOS build is **not reproducible from `cargo build` alone**.
- A contributor needs the platform toolchain.
- **CI needs a macOS host for even a smoke build.**
- A build-time concern — a feature flag, a vendored source path, an embedded asset — has to
  be expressed **twice**, and can drift the way D-17 refuses to let the shells drift. The
  mitigation is that the list is short and enumerated in `docs/build/workspace.md`.

## Two channels, one identity

D-45 gives the App Store build and the Homebrew Cask build **one bundle identifier, one team
identifier, and sandboxing for both**. Two identifiers would mean two containers, two
per-installation secrets, and under D-43 a channel switch that destroys the blob cache and
re-authenticates every account. The cost accepted is that the two builds cannot be installed
side by side.

The Cask build is not reviewed by the platform, so it needs its own path: a distribution
certificate under the same team identifier, the hardened runtime, and **notarization with
stapling** — so a first launch does not depend on the user being online.

**The hardened runtime is load-bearing rather than hygiene.** The threat model puts an
in-address-space attacker out of scope *because the platform prevents it*; an entitlement
re-permitting debugger attachment would move a whole class of attack into scope silently.

## Entitlements

The app sandbox itself; outgoing network connections; user-selected read-write file access
for FR-10's save; Keychain access groups for NFR-23; and the address-book entitlement **with
its usage description**, without which FR-40 is silently absent rather than degraded.

## Not built here

`sift-harness` drives the whole application with no window at all (D-65), and the Linux shell
is an ordinary Cargo binary (`crates/shells/sift-gtk`).
