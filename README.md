# Sift

Open-source, snappy email client for reading, searching, and triaging mail across multiple accounts.

> Sift is in early development. The design was settled and written down first, and
> [`docs/`](docs/README.md) remains normative — the code follows it rather than the other way
> round. Start there.

Sift does not send mail. Reading, search, and triage are the product; composition, calendar, and contacts
are [out of scope by design](docs/product/scope.md), not deferred.

## What it aims to be

- **Multi-account, multi-provider** — Gmail, Microsoft 365 / Outlook.com, JMAP, and generic IMAP behind
  one capability-driven abstraction. Gmail speaks its protocol today; the other three declare
  their capabilities and are waiting on their wire protocols
  (issues [#39](https://github.com/justin13888/Sift/issues/39),
  [#40](https://github.com/justin13888/Sift/issues/40),
  [#41](https://github.com/justin13888/Sift/issues/41))
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

Commands are [`mise`](https://mise.jdx.org) tasks, defined once in
[`mise.toml`](mise.toml) and run by CI and the git hooks as well as by hand. `mise install`
fetches the pinned toolchain and tools and installs the hooks; `mise tasks` lists everything.

```
mise run test      # the whole core
mise run gates     # the crate graph, the prohibitions, the dependency gates,
                   # the generated header, and requirement traceability
mise run check     # the above plus formatting and lints — the pre-push sweep
mise run harness   # drive the application with no window (D-65)
```

The harness is a shell rather than a script: it links the same presentation layer a native
shell links and invokes the same actions by the same identifiers, which is what makes
FR-24's testability claim real.

```
mise run harness -- "account add work rich" "ingest work Newsletter" \
                    "select #1" "do message.archive" "list" "queue"
```

### Driving a real account

An account can be added, synced, read, mutated and flushed without a window and without a
pointer. Against the fixture corpus, which needs no network and no credentials — this
session is `mise run demo`:

```
mise run harness -- "account add-replayed mail" "folders mail" "sync mail" \
                    "list mail" "body #1" "select #1" "do message.archive" "flush mail"
```

Against a real Gmail account, which needs an OAuth client identifier and a bundle that has
registered the callback scheme — this binary is not one, so it says so rather than opening a
browser you would return from to nothing:

```
mise run harness -- "account authorize work <client-id>"
mise run harness -- "account callback work net.justinchung.sift:/oauth2/callback?state=...&code=..."
```

The callback returns through a **registered URI scheme**, never a loopback address: NFR-24
admits no listening socket for any purpose. The scope set is one scope, it is permanent, and
it cannot send — which is the no-send constraint expressed where you can check it against the
consent screen you are looking at.

The two native shells are [`crates/shells/sift-gtk`](crates/shells/sift-gtk) and
[`shells/macos`](shells/macos/README.md), built by `mise run linux` and `mise run macos`.
D-61 puts the Linux binary under Cargo and gives the macOS bundle to the platform toolchain,
which links the Rust core as a static library — which is why those are two tasks rather than
one, and why only one of them is a plain `cargo build`.

## Documentation

The [`docs/`](docs/README.md) tree is the specification: scope, architecture, provider model, storage,
the rendering pipeline, runtime behaviour, and security — plus a
[decision log](docs/decisions.md) recording what was rejected and why each choice is contestable, and a
list of [open questions](docs/open-questions.md).

Contributors and agents should also read [`AGENTS.md`](AGENTS.md).

## License

AGPL-3.0 — see [LICENSE](LICENSE).
