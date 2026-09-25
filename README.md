# Sift

Open-source, snappy email client for reading, searching, and triaging mail across multiple accounts.

> Sift is in early development. The design was settled and written down first, and
> [`docs/`](docs/README.md) remains normative — the code follows it rather than the other way
> round. Start there.
>
> **The macOS application runs.** It syncs, renders and triages mail through the hardened
> pipeline, keeps it encrypted on disk, and survives a restart. A new account is **watched
> rather than written to** until you say otherwise: it syncs and shows everything and changes
> nothing in your mailbox, and the queued changes are visible in the runtime window so that
> "nothing was sent" is something you can check rather than something you are told.

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
mise run harness -- "account callback work com.googleusercontent.apps.<id>:/oauth2/callback?state=...&code=..."
```

The callback returns through a **registered URI scheme**, never a loopback address: NFR-24
admits no listening socket for any purpose. The scope set is one scope, it is permanent, and
it cannot send — which is the no-send constraint expressed where you can check it against the
consent screen you are looking at.

## Connecting a real Gmail account

Sift ships with **no OAuth client**, and a build with none runs against the recorded corpus and
says so. A client identifier is not a secret — it appears in every authorization URL, which is
why PKCE exists — but quota, verification status and the consent screen all attach to the
client rather than to the application, so a committed one would make every checkout share all
three.

**1. Create the client.** In the [Google Cloud console](https://console.cloud.google.com):

- Create a project, then enable the **Gmail API** for it.
- Configure the OAuth consent screen as **External**, and add your own address under
  *Test users*. Until the app is verified, only test users can sign in.
- Under *Credentials*, create an **OAuth client ID** with application type **iOS** — not
  "Desktop app". Desktop expects a `127.0.0.1` loopback redirect, and
  [NFR-24](docs/architecture/shell-boundary.md) forbids a listening socket of any kind, for any
  purpose. The iOS/macOS type is the only one that accepts a scheme redirect.
- For the bundle identifier, enter `net.justinchung.sift`.
- Add the scope `https://www.googleapis.com/auth/gmail.modify`. It is the only one Sift asks
  for, and it is the narrowest one that permits triage. **The scope that permits permanent
  deletion also permits sending**, which is why Sift will do neither.

**2. Point the build at it.** Put the identifier where the build will find it — the file is not
committed:

```
echo '123456789-abcdef.apps.googleusercontent.com' > shells/macos/oauth-client.txt
mise run macos -- --run
```

The build derives the callback scheme from the client (`com.googleusercontent.apps.123456789-abcdef`),
registers it in the bundle, then **reads the built bundle back** and fails if it is not there —
so a build that would have sent you to a browser you could not return from stops here instead.
It prints the client, the scheme, and the scheme the bundle actually registers:

```
macos: OAuth client 123456789-abcdef.apps.googleusercontent.com
       callback scheme com.googleusercontent.apps.123456789-abcdef
       registers  com.googleusercontent.apps.123456789-abcdef
```

There is **no client secret** to store: a public client cannot keep one, which is what PKCE
replaces it with. If the console offered you one, you created a *Web application* or *Desktop
app* client — neither accepts a scheme redirect, and the iOS type has no secret to give.

Sift checks the registration again at launch, and refuses to start a sign-in where its bundle
does not claim the scheme its client requires — before opening a browser, rather than after you
have granted consent.

**3. Watch before you write.** The account is added read-only — [D-110](docs/mail/mutations.md).
Let it sync, read some mail, archive something, then open **Runtime** (turn it on in Settings
first — it is off by default) and look at the queue: every intent should read `Pending`, and the
window says so in a sentence.

When you are satisfied, tick **Sift may change this mailbox** in that same window and press
**Send Queued Changes**. Sift can then archive, move, flag, label, mark read, report junk, and
move messages to Gmail's own Trash. It can never permanently delete one, and it can never send
one. Untick it and anything not already issued stops again; what has left cannot be recalled,
and Sift does not pretend otherwise.

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

Contributors should read [`CONTRIBUTING.md`](CONTRIBUTING.md) before opening a pull request; contributors
and agents should also read [`AGENTS.md`](AGENTS.md).

## License

AGPL-3.0 — see [LICENSE](LICENSE). Contributions are accepted only under the contributor licence
agreement in [`CONTRIBUTING.md`](CONTRIBUTING.md), which keeps the Mac App Store channel possible (D-112).
