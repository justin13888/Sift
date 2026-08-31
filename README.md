# Sift

Open-source, snappy email client for reading, searching, and triaging mail across multiple accounts.

> Sift is in early development. The design was settled and written down first, and
> [`docs/`](docs/README.md) remains normative — the code follows it rather than the other way
> round. Start there.

Sift does not send mail. Reading, search, and triage are the product; composition, calendar, and contacts
are [out of scope by design](docs/product/scope.md), not deferred.

## What it aims to be

- **Multi-account, multi-provider** — Gmail, Microsoft 365 / Outlook.com, JMAP, and generic IMAP behind
  one capability-driven abstraction
- **Genuinely light while resident** — it is meant to run all the time, so idle memory, CPU wakeups, and
  network use rank above feature breadth
- **Safe with hostile mail** — message bodies render with no JavaScript and no network access of their
  own, behind a sanitizer with asserted, tested invariants
- **Private by default** — remote content and trackers blocked out of the box, no telemetry containing
  anything about your mail
- **Fast to search** — local full-text search over cached mail, merged with server-side search for the
  uncached tail

Platform scope is **macOS first, Linux second**. Windows is out of scope.

## Building

```
cargo test --workspace     # the whole core
cargo xtask all            # the crate graph, the prohibitions, the dependency gates,
                           # the generated header, and requirement traceability
cargo run -p sift-harness  # drive the application with no window (D-65)
```

The harness is a shell rather than a script: it links the same presentation layer a native
shell links and invokes the same actions by the same identifiers, which is what makes
FR-24's testability claim real.

```
sift-harness "account add work rich" "ingest work Newsletter" \
             "select #1" "do message.archive" "list" "queue"
```

The two native shells are [`crates/shells/sift-gtk`](crates/shells/sift-gtk) and
[`shells/macos`](shells/macos/README.md). D-61 puts the Linux binary under Cargo and gives
the macOS bundle to the platform toolchain, which links the Rust core as a static library.

## Documentation

The [`docs/`](docs/README.md) tree is the specification: scope, architecture, provider model, storage,
the rendering pipeline, runtime behaviour, and security — plus a
[decision log](docs/decisions.md) recording what was rejected and why each choice is contestable, and a
list of [open questions](docs/open-questions.md).

Contributors and agents should also read [`AGENTS.md`](AGENTS.md).

## License

AGPL-3.0 — see [LICENSE](LICENSE).
