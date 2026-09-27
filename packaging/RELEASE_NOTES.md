# Release notes

Newest release first. Each release is a `##` section headed by its D-62 version, and the text under
that heading is what `gh release create` publishes (see [the README](README.md#release-notes-and-qa)).
Every section has the same five parts, in this order: **Where it runs**, **What changed**,
**Security**, **Known limitations** and **Before you install**. A part with nothing to say keeps its
heading and says "None."

The notes are written for someone installing Sift. They are not normative. [`docs/`](../docs/README.md)
is the specification, and an issue number is where a limitation is tracked.

---

## 0.1.0 — release candidate

The first build of Sift offered for installation. It is a **release candidate**: it exists so that
its maintainer can install and test it on real mailboxes before anyone else does.

### Where it runs

- **macOS 13 (Ventura) or later**, as one universal app for Apple silicon and Intel Macs (D-46).
- **Two ways to install**, both the same signed build: a DMG you drag to Applications, or the
  Homebrew Cask in [`homebrew/sift.rb`](homebrew/sift.rb). The app is signed with Developer ID and
  notarized by Apple, and the notarization is stapled, so opening it for the first time does not
  need a network connection (D-45).
- **No Mac App Store build.** The owner deferred that channel on 2026-09-24. It has not been ruled
  out (D-33).
- **No Linux build** in this release.
- **Sift does not update itself.** Upgrades come through the channel you installed from. If the app
  is replaced while it is running, Sift asks you to restart (FR-26).

**Mail providers:**

- **Gmail.** Sift's Google client is **unverified** and uses a **test-user list** for this release.
  Only Google accounts on that list can sign in, and Google shows its "Google hasn't verified this
  app" warning first. Choose *Advanced*, then continue to Sift. Google also ends a test user's
  sign-in after seven days, so a Gmail account shows **Needs authentication** once a week, and
  *Sign In Again…* fixes it. The decision on Google's restricted-scope assessment has been deferred
  to general availability (#16).
- **Microsoft Outlook.com and Microsoft 365.** Personal and work or school accounts both use the
  same sign-in.
- JMAP and generic IMAP accounts cannot be added in this release.

### What changed

This is the first release, so everything is new. Sift can:

- Sign in to several Gmail and Microsoft accounts through your browser. Tokens are stored only in
  your macOS Keychain (FR-1, FR-2).
- Show a unified inbox across accounts, with threads, a message list and a reader (FR-6, FR-7,
  FR-11).
- Render HTML mail with scripts disabled in the engine and remote content blocked. It also shows a
  message as plain text or as its raw source (FR-8, FR-9, NFR-20).
- Archive, move, trash, flag, mark read or unread, tag, and report junk or not junk. It can also
  delete permanently where the provider allows it (Gmail does not). Every action except permanent delete can be undone (FR-13, FR-15).
- Keep a **new account read-only** until you allow Sift to change that mailbox. Until then, your
  actions show in Sift but are never sent to the server. The *Runtime…* window lists what is
  waiting (D-110).
- Search local mail as you type, with operators such as `from:`, `to:`, `subject:`,
  `has:attachment`, `is:unread`, `in:`, `before:` and `after:` (FR-19, FR-20).
- Keep running in the menu bar after you close its last window. *Quit Sift Entirely* stops it
  (FR-22, FR-25).
- Post a notification when new mail arrives (FR-23).
- Remove an account and erase what Sift stored for it (FR-4).
- Let you do every action from the keyboard and the command palette (⌘K) (FR-24).

**Sift does not send mail.** It has no code that sends, and it never will. *Reply…*, *Reply All…*
and *Forward…* open the message in the Mac's default mail app. If no mail app is set up, Sift
says so (FR-41).

### Security

- **RUSTSEC-2026-0285 (GHSA-2mjx-qc3c-rqvc) in rustls is fixed.** rustls accepted TLS 1.3
  handshake messages sent at the wrong encryption level. As a result, a peer could send in
  plaintext handshake messages that should have been encrypted. The handshake transcript is still
  authenticated, so an attacker could not change or complete a handshake this way. Every
  connection Sift makes uses rustls. This release uses rustls 0.23.45, which contains the fix
  (#54). No earlier Sift release was published. Builds made from source before revision `2c9ea4b`
  used the affected 0.23.43.

### Known limitations

These were open when this release was cut. They are listed by what you will notice, and each
issue number is where the fix is tracked.

**Reading**

- **Remote images do not load, even after you allow them.** *Always Load Images From Sender*
  records your choice, but the image is still reported as unavailable (#119).
- **Messages that set right-to-left direction or a language only on the whole document** lose it.
  They show left to right and without a language (#91).
- **The dark-mode adaptation does not follow *Increase contrast*** in Accessibility settings (#83).
- **Only Sift's own email-tracking list blocks content.** EasyList and EasyPrivacy are not bundled
  yet (#120).
- **Saved attachments are not quarantined.** Files saved from a message do not get the macOS
  "downloaded from the internet" marking, so Gatekeeper does not check them when you open them
  (#134).

**Search**

- **Search does not ask the server.** It covers only the mail Sift has downloaded. It does not show
  where each result came from, or say that older mail is missing (#129). Related: when a message
  is found by the server search and later deleted on the server, it stays in Sift's local copy
  (#130).

**Accounts and sync**

- **The app can briefly stop responding to clicks or keys while an account syncs.** A click waits
  for the provider's reply (#114).
- **Adding a mailbox that is already in Sift does not warn you.** You get a second copy of the
  account (#126).
- **Moving an Outlook message to another folder** makes it a new message in Sift, and any selection
  on it is lost (#115).
- **The "also in another account" marker in the unified inbox can be wrong** (#106).
- **Google describes the Gmail permission Sift asks for as broader than what Sift uses**, and that
  description includes sending (#78). Sift has no code that sends mail.

**Staying in the background**

- **Sift does not start when you log in.** You open it yourself after each login (#35).
- **Notifications have no settings.** There is no quiet mode and no per-account or per-folder rule.
  Once you allow notifications, Sift posts one whenever new unread mail arrives in any account
  (#123).
- **Under memory pressure, Sift does not always release the message view's memory** (#98). Memory
  also grows a little with each sync, each message read and each action (#89).

**Diagnostics**

- **No diagnostic log is written yet, and crash reports carry only the crashing thread** (#63,
  #138).

**Installing**

- **The DMG window has no background art or arranged icons.** Drag *Sift* onto *Applications*
  (#132).
- **`brew uninstall --zap` leaves your accounts' sign-in credentials in the Keychain.** Remove
  each account in Sift before you uninstall, and they are deleted with it (#141).

### Before you install

- **Accounts from development builds of Sift do not carry over, so add them again.** This build is
  sandboxed and signed under the team's Keychain access group (#55). A development build used a
  different signature and kept its data outside the sandbox, so this build cannot see those
  accounts, and their Keychain items are not readable here. If you ran a development build, you
  can delete its data at `~/Library/Application Support/net.justinchung.sift`.
- **Uninstalling keeps your data unless you ask.** `brew uninstall --cask sift` and
  `brew upgrade` remove only the app. `brew uninstall --zap --cask sift` also removes the
  mailboxes Sift stored, but not the accounts' sign-in credentials in the Keychain (#141). To
  remove those, use *Remove Account…* on every account before you uninstall.
