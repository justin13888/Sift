# Release QA checklist

These are the checks that `mise run check` and CI cannot make. Each one needs an installed, signed,
sandboxed and notarized Sift that is signed in to real accounts. Run the checklist on every release
candidate before it is published to the Cask, and keep each run's results here.

This file is **not normative.** Every check traces to a requirement (FR, NFR), a decision (D), an
invariant (N) or an issue. A failure is filed against that identifier, and the identifier's owning
document in [`docs/`](../docs/README.md) says what is required. If this file and `docs/` disagree,
`docs/` is right and this file is the defect.

## How to record a run

1. Add a row to [Runs](#runs) and a result column headed with its run number (`R1`, `R2`, …) to
   every table below. Keep earlier columns. They are the history.
2. Write one of these in each cell:

   | Result | Meaning |
   |---|---|
   | `pass` | The *Pass when* column held. |
   | `fail #N` | It did not hold, and issue #N is filed against the identifier in *Traces to*. |
   | `known #N` | It failed the way the open issue in *Known open* says it would. Nothing new to file. |
   | `n/a: reason` | It could not be run, for example because no work account was available. |
   | a number | For [Resources](#resources), the figure observed, with its unit. |

3. A `known #N` whose issue has since closed is a `fail`: the fix did not reach the build.
4. Check IDs are permanent. A check that no longer applies is struck through, and its ID is never
   reused.

**Resource figures are observations, not validation.** Every number in `docs/` is a hypothesis until
it is measured on the [reference environment](../docs/product/reference-environment.md). A figure
recorded here says what one machine showed. It neither confirms nor changes a budget.

## Runs

| Run | Version | Build | Revision | Date | Mac (model, chip, macOS) | Tester |
|---|---|---|---|---|---|---|
| R1 | 0.1.0 | | | | | @justin13888 |

## Before you start

- **The artefacts** from `mise run release-rc -- <version>` on the release machine:
  `target/release-rc/<version>/Sift-<version>.dmg` and its `.sha256`. [Cask](#cask) needs the
  pre-release published first, using the commands `release-rc` prints.
- **Accounts:**
  - A Google account on the OAuth client's test-user list, and one that is not on it.
  - A personal Outlook.com account.
  - A Microsoft 365 work or school account, if one is available.
  - Every triage action in this checklist is **undone** after it is checked, so real mail ends
    where it started.
- **A second way to see each mailbox**, such as the provider's web client, to confirm what reached
  the server.
- **A way to send yourself test mail** from outside Sift. Sift cannot send. Test mail needs:
  - an ordinary HTML newsletter;
  - a message with an attachment;
  - the hostile message described in [READ-2](#reading-and-rendering).
- **A request-capture URL** that you control and that logs every hit, such as a request bin, for
  READ-2.
- **A clean start.** No Sift data from a development build: delete
  `~/Library/Application Support/net.justinchung.sift` and `~/Library/Containers/net.justinchung.sift`.
  Also remove any Keychain item whose service is `net.justinchung.sift`.

Commands below assume the app is at `/Applications/Sift.app` and `V` is the version under test,
for example `V=0.1.0`.

## Install

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| INS-1 | `shasum -a 256 -c Sift-$V.dmg.sha256` | `OK`, and the digest equals the Cask's `sha256` line | D-62 | | |
| INS-2 | `xcrun stapler validate Sift-$V.dmg` and `spctl -a -t open --context context:primary-signature -vv Sift-$V.dmg` | Validated; `accepted` with `source=Notarized Developer ID` | D-45 | | |
| INS-3 | Open the DMG | It mounts and shows *Sift* with the app icon, beside an *Applications* link. Dragging one onto the other installs it | D-33, #62 | #132 (no background or layout) | |
| INS-4 | `codesign --verify --deep --strict --verbose=2 /Applications/Sift.app` and `codesign -dv /Applications/Sift.app` | Valid on disk, satisfies its designated requirement, `TeamIdentifier=854G577S2Y`, flags include `runtime` | D-45 | | |
| INS-5 | `codesign -d --entitlements - /Applications/Sift.app` | `com.apple.security.app-sandbox` is true, and the Keychain access group is `854G577S2Y.net.justinchung.sift`. No `get-task-allow`, `disable-library-validation` or `allow-unsigned-executable-memory` | D-45, NFR-23 | | |
| INS-6 | `spctl -a -vv -t exec /Applications/Sift.app` and `xcrun stapler validate /Applications/Sift.app` | `accepted`, `source=Notarized Developer ID`; validated | D-45 | | |
| INS-7 | Mark the DMG as downloaded before mounting (`xattr -w com.apple.quarantine "0081;$(printf %x "$(date +%s)");Safari;" Sift-$V.dmg`), install from it, turn networking off, then open Sift for the first time | Gatekeeper shows only its "downloaded from the internet" confirmation. It shows no "cannot be verified" or malware warning, and Sift opens with no network | D-45, D-46 | | |
| INS-8 | `lipo -archs /Applications/Sift.app/Contents/MacOS/Sift` | `x86_64 arm64` | D-46 | | |
| INS-9 | On Apple silicon, `arch -x86_64 /Applications/Sift.app/Contents/MacOS/Sift` (Rosetta). On an Intel Mac, launch normally | The Intel slice launches and reaches its window. Quit it with *Quit Sift Entirely* | D-46 | | |
| INS-10 | *Sift → About Sift* | Shows version `$V` and the build number the run row records | D-62 | | |
| INS-11 | Activity Monitor, with the *Sandbox* column shown | Sift reads `Yes` | D-45, NFR-25 | | |
| INS-12 | While Sift is running, drag the same or a newer `Sift.app` over the installed one | Sift says *Sift was updated and needs to restart* instead of running on against the replaced bundle | FR-26 | | |

### Cask

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| CSK-1 | `brew style --except-cops Style/FrozenStringLiteralComment packaging/homebrew/sift.rb` | No offences | D-33 | | |
| CSK-2 | With the DMG-installed copy removed, install from a local tap holding the release commit's Cask: `brew tap-new --no-git local/sift`, copy `packaging/homebrew/sift.rb` into `$(brew --repository local/sift)/Casks/`, then `brew install --cask local/sift/sift` | The download's checksum verifies against the Cask's `sha256` line, and the installed app passes INS-4 to INS-6 | D-33, D-62 | | |

## First run

With no Sift data present (see [Before you start](#before-you-start)).

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| RUN-1 | Launch Sift | The first window is *Add an Account*, not an empty inbox | [ui-shell First run](../docs/architecture/ui-shell.md#first-run), FR-1 | | |
| RUN-2 | Read the *Add an Account* window | It says *Sift reads your mail. It does not send it.* | FR-41 | | |
| RUN-3 | The add-account choices | Gmail and Microsoft are offered. JMAP and IMAP are not | FR-1 | | |
| RUN-4 | Watch for system permission prompts from launch until an account is added | None at launch. Notifications are asked for only when the first account is added or first syncs. Contacts are asked for only when a name is first resolved, if at all | ui-shell First run, FR-23, FR-40 | | |

## Sign-in

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| AUTH-1 | Add the Gmail test user | The browser opens Google's consent. Continue through *Google hasn't verified this app* using *Advanced*. The consent asks for one Gmail permission. The browser returns to Sift, and the account appears and starts syncing | FR-2, D-36, D-109, D-88, #16 | #78 (scope wording) | |
| AUTH-2 | Try to add a Google account that is not on the test-user list | Google refuses it. Sift is left with no half-added account and can start again | FR-2 | | |
| AUTH-3 | Add the Outlook.com account | Same as AUTH-1, through Microsoft's consent | FR-1, FR-2, D-36 | | |
| AUTH-4 | Add the Microsoft 365 work or school account | Same as AUTH-3, through the same sign-in. No separate work option | FR-1, D-12 | | |
| AUTH-5 | Start adding an account, then close the browser tab or cancel in Sift | Sift returns to where it was, with no account added and nothing stuck waiting | FR-2 | | |
| AUTH-6 | *Quit Sift Entirely*, then relaunch | Every account syncs, with no Keychain password prompt and no browser | FR-2, NFR-23 | | |
| AUTH-7 | In Keychain Access, search `net.justinchung.sift`. Then `grep -rIl -e 'ya29\.' -e 'EwB' ~/Library/Containers/net.justinchung.sift` | Keychain items exist for the accounts. The grep finds nothing: no token is stored outside the Keychain | NFR-23, [credentials](../docs/security/credentials.md) | | |
| AUTH-8 | Leave Sift running for more than an hour, then trigger a sync | It syncs without any prompt, because the refresh happened silently | FR-2 | | |
| AUTH-9 | Revoke Sift's access at the provider (Google: *Third-party apps & services*; Microsoft: *Apps and services you've given access*). Close Sift's windows | The account turns *Needs authentication*, and this reaches you from the menu bar with no window open | D-49, [failure-model](../docs/runtime/failure-model.md) | | |
| AUTH-10 | *Sign In Again…* on that account | Sign-in restores it, and Sift offers to remove the old account it replaces | FR-2, FR-4, #60 | | |
| AUTH-11 | Add a mailbox that is already in Sift | Sift warns that the mailbox seems to be configured already, and lets you continue | D-89 | #126 | |
| AUTH-12 | Record `ls -R ~/Library/Containers/net.justinchung.sift/Data/Library` and the Keychain items. Then use *Remove Account…* on one account and confirm | A confirmation appears first. Afterwards the account's database, the blobs only it referenced, and its Keychain items are gone, and the other accounts are untouched | FR-4 | | |
| AUTH-13 | Seven days after AUTH-1 | The Gmail account shows *Needs authentication*. This is expected for a test user and is not a defect. *Sign In Again…* restores it | D-49, #16 | | |

## Sync and triage

Every intent is undone after it is checked. Do WRITE-1 before anything else in this section.

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| SYNC-1 | Watch a new account's first sync | Progress is visible, newest mail arrives first, and the account reads *Recovering* until the backfill ends | D-53, NFR-18 | | |
| SYNC-2 | Send yourself a message from outside Sift | It appears on a later poll without any action from you. Record how long it took | [scheduling](../docs/runtime/scheduling.md) | | |
| SYNC-3 | While a large account syncs, click through the list and use the menus | Sift keeps responding | D-19 | #114 | |
| WRITE-1 | On a newly added account, archive a message, then open *Sift → Runtime…* | The message leaves Sift's list but is still in the inbox on the web. The runtime window lists the intent as *Pending* and says *Nothing was sent — this mailbox is only being watched* | D-110 | | |
| WRITE-2 | Turn on *Sift may change this mailbox* for that account | The pending intent is sent to the provider, and the web client shows it archived. Undo it (⌘Z) | D-110, FR-15 | | |
| WRITE-3 | Turn the switch off again and triage a message | Nothing reaches the server, and the intent waits as *Pending* | D-110 | | |
| TRI-1 | *Archive* (⌃⌘A) | Leaves the list at once and reaches the server. Undo puts it back in both places | FR-13, FR-15, NFR-7 | | |
| TRI-2 | *Move to Folder…* (⇧⌘M) | Same as TRI-1, into the chosen folder | FR-13, FR-15 | #115 (Outlook: selection lost) | |
| TRI-3 | *Move to Trash* (⌫) | Same as TRI-1, into Trash | FR-13, FR-15 | | |
| TRI-4 | *Flag* (⇧⌘L) | The flag shows at once and on the server, and undo clears it | FR-13, FR-15 | | |
| TRI-5 | *Mark as Read* (⌃⌘R) and *Mark as Unread* (⇧⌘U) | Reflected at once and on the server, and undo reverses each | FR-13, FR-15 | | |
| TRI-6 | *Add Tag…* (⌃⌘T) and *Remove Tag…* (⌃⇧⌘T) | A Gmail label or Outlook category is added or removed, and undo reverses each | FR-13, FR-15, FR-37 | | |
| TRI-7 | *Move to Junk* (⇧⌘J) and *Not Junk* (⌥⇧⌘J) | The message moves to or from Junk on the server, and undo reverses each | FR-13, FR-15, FR-39 | | |
| TRI-8 | *Delete Permanently…* (⇧⌘⌫), on a message you sent yourself for this purpose | On Microsoft: a confirmation, then the message is gone with no undo. On Gmail: the action is unavailable rather than approximated | FR-13, FR-15, NFR-7 | | |
| TRI-9 | Select several messages and archive them together, then undo | All of them move and all of them come back | FR-17 | | |
| TRI-10 | Archive a thread from a collapsed row, then undo | Every message in the thread moves and comes back | FR-38 | | |
| SYNC-4 | *Pause Syncing* from the menu bar icon, then *Resume Syncing* | Accounts read *Paused by the user* and stop polling, then resume | FR-22, D-49 | | |
| SYNC-5 | Turn networking off. Read cached mail, open an uncached message, and archive something | Cached mail reads normally. The uncached one says it is not cached, as opposed to not available. The archive applies locally and waits | FR-12, FR-14 | | |
| SYNC-6 | Turn networking back on | The waiting intent is sent and no account turns *Needs authentication*. Undo it | NFR-33, NFR-34 | | |
| SYNC-7 | With syncing paused and an intent waiting, `kill -9 $(pgrep -x Sift)` and relaunch | The intent is still queued, and after resuming it is applied once, not twice. Undo it | NFR-16, FR-18 | | |
| SYNC-8 | Leave Sift running with its window closed and send yourself a message | One notification is posted, and clicking it opens that message | FR-23 | | |
| SYNC-9 | Look for quiet mode and per-account or per-folder notification rules | They exist in *Settings…* | FR-23 | #123 | |

## Reading and rendering

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| READ-1 | Open an ordinary HTML newsletter | It renders legibly, with remote images blocked and a bar outside the message saying so | FR-8, N-1 | | |
| READ-2 | Open a message you built to be hostile. It should contain `<script>`, an `onload=` handler, `<img>`, `<link rel=stylesheet>`, a CSS `background:url()`, `@import`, a `<meta http-equiv=refresh>`, an `<iframe>`, and a `<form action>`, each pointing at your request-capture URL. Scroll and hover over all of it | Nothing runs, and the capture URL receives **no request at all**, including after the message has been open for a minute | N-1, NFR-20, NFR-21 | | |
| READ-3 | *Always Load Images From Sender* (⇧⌘I) on READ-1's newsletter | Its images load, but only for that sender | FR-8 | #119 | |
| READ-4 | Click a link in a message, and hover over one whose text differs from its target | It opens in the default browser, never inside Sift. The shown destination is unwrapped, and an internationalized domain appears decoded | FR-30 | | |
| READ-5 | Set the system to dark appearance | Sift's own interface turns dark. Messages are unchanged by default | FR-31 | | |
| READ-6 | *Adapt to Dark Mode* (⌃⌘D) on a light message, and open a message that declares its own dark styling | The first is transformed and stays legible. The second uses the sender's own dark styling | FR-31, FR-32, NFR-47 | #83 (Increase contrast) | |
| READ-7 | *Show Plain Text* (⌥⌘P) and *Show Raw Source* (⌥⌘U) | Both views show this message | FR-9 | | |
| READ-8 | *Find in Message…* (⌘F) | Matches in the body are found and highlighted | D-50 | | |
| READ-9 | *Expand Thread* (⌘→) and *Collapse Thread* (⌘←) | Threads open and close | FR-11 | | |
| READ-10 | Save an attachment. Then save one whose name contains `../`, and one with the same name twice | The saved path is shown. The name never becomes a path outside the chosen folder, and nothing is overwritten | FR-10, NFR-53 | | |
| READ-11 | `xattr -p com.apple.quarantine <saved attachment>` | A quarantine marking is present | NFR-49 | #134 | |
| READ-12 | *Reply…* (⌘R), *Reply All…* (⇧⌘R), *Forward…* (⇧⌘F) | The default mail app opens with the subject filled in. With no mail app set up, Sift says it does not send mail | FR-41 | | |
| READ-13 | Open a message whose `<html>` or `<body>` sets `dir="rtl"` and `lang` | It reads right to left, in that language | NFR-50, NFR-28 | #91 | |
| READ-14 | *Message Details…* (⌥⌘I) | The per-message debug view opens, showing the pipeline's stages | FR-33 | | |

## Search

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| SRCH-1 | *Search…* (⌥⌘F), and type a word slowly | Results update as you type | FR-19 | | |
| SRCH-2 | Each of `from:`, `to:`, `subject:`, `has:attachment`, `is:unread`, `is:read`, `in:`, `before:YYYY-MM-DD`, `after:YYYY-MM-DD`, and a `"quoted phrase"` | Each narrows the results to what it names | FR-20 | | |
| SRCH-3 | Search for a word that appears only in the body of a message you have opened | That message is found | FR-19 | | |
| SRCH-4 | Search for a word that is only in an old message Sift never downloaded | Results that came from the server are labelled that way, and the search says what it could not cover | FR-21 | #129 | |
| SRCH-5 | *Search This Account* and *Search This Folder* | Results narrow to that scope | FR-20 | | |
| SRCH-6 | Select all the results of a search, mark them read, then undo | All of them change and all of them come back | FR-17 | | |

## Keyboard and accessibility

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| KEY-1 | Without touching the pointer, reach every item in every menu | Each one is reachable, and each works where it applies | FR-24 | | |
| KEY-2 | *Command Palette…* (⌘K), then type part of an action's name | The action is listed under the same words as its menu item and runs from there | FR-24 | | |
| KEY-3 | ⌘1, ⌘2, ⌘3 and ⌥⌘0, then arrows in the list | Focus moves to the sidebar, the list, the reader and search, and the arrows move through messages | FR-24 | | |
| KEY-4 | Cut, Copy, Paste and Select All in the search field, and Copy from a message body. Also *About Sift*, *Hide Sift* (⌘H), *Hide Others*, *Minimize* (⌘M) and *Zoom* | Each does what it does in any Mac app | D-118, #61 | | |
| KEY-5 | Turn on VoiceOver (⌘F5) and move through the sidebar, the list, the reader header and the body | Every row is announced with its sender, subject, date and unread state. Headings and links in the body can be reached with VoiceOver's own navigation | NFR-27, NFR-50 | | |

## Residency

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| RES-1 | Close the last window (⌘W) | Sift's icon stays in the menu bar and mail keeps arriving (SYNC-8) | FR-22, FR-25 | | |
| RES-2 | Open the menu bar icon's menu | It offers *Open Sift*, *Pause syncing* and *Quit Sift entirely* | FR-22 | | |
| RES-3 | *Quit Sift Entirely* (⌘Q), then `pgrep -x Sift` | No process is left | FR-25 | | |
| RES-4 | Turn on *Open at Login*, log out and back in | Sift starts with its menu bar icon and no window | FR-22, [process-model](../docs/architecture/process-model.md) | #35 | |
| RES-5 | While Sift runs, `lsof -nP -a -p "$(pgrep -x Sift)" -iTCP -sTCP:LISTEN` and `lsof -nP -a -p "$(pgrep -x Sift)" -U` | No listening TCP socket, and no Unix-domain socket Sift created to listen on | NFR-24 | | |

## Resources

Record each figure with its unit. The *Budget* column names the hypothesis the figure is set against.
It is not a threshold this run passes or fails.

| ID | Observation | How | Budget | Known open | R1 |
|---|---|---|---|---|---|
| PERF-1 | Cold start to the list | Time from a click on the Dock icon to the list being shown, three times | NFR-1 | | |
| PERF-2 | Idle footprint, no window, after an hour | `footprint -p "$(pgrep -x Sift)"` (phys_footprint), and the live bytes by subsystem in *Runtime…* | NFR-8 | #98 | |
| PERF-3 | Idle footprint, window open, no message selected, after an hour | As PERF-2 | NFR-9 | #98 | |
| PERF-4 | Idle CPU and wakeups over five minutes | `top -l 2 -s 300 -pid "$(pgrep -x Sift)" -stats pid,cpu,idlew`, second sample, divided by the number of accounts | NFR-10, NFR-11 | | |
| PERF-5 | Idle network over ten minutes | Activity Monitor → Network, Sift's *Sent Bytes* and *Rcvd Bytes* at the start and at the end | NFR-15 | | |
| PERF-6 | Memory after simulated pressure | With a message open, `sudo memory_pressure -S -l critical`, then PERF-3's reading, and whether a WebKit content process for Sift remains | NFR-13, NFR-46 | #98 | |
| PERF-7 | Growth over the run | PERF-2 repeated at the end of the run, and the difference | NFR-12 | #89 | |

## Diagnostics

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| DIAG-1 | *Help → Show Diagnostics in Finder*, and the Diagnostics section of *Runtime…* | Finder opens Sift's `Diagnostics` folder. The panel reports that core dumps are off | D-35, Q-20 | #63 | |
| DIAG-2 | `kill -SEGV "$(pgrep -x Sift)"`, then relaunch | Sift shows the crash report it wrote, with *Save a Copy…* and *Dismiss*. The report has a backtrace, the version and the build | D-35, D-114 | #63 | |
| DIAG-3 | Search the Diagnostics folder and the saved crash report for an account address, its domain, and a subject Sift has shown: `grep -rIil -e <address> -e <domain> -e '<subject>' <path>` | Nothing is found | NFR-55, NFR-22 | #138 (no log written) | |
| DIAG-4 | `ls ~/Library/Logs/DiagnosticReports/ \| grep -i sift` after DIAG-2 | Record whether macOS wrote its own report. Whether a sandboxed build can turn the platform's collector off is Q-20's open question, so this is an observation | D-35, Q-20 | #13 | |

## Uninstall

| ID | Check | Pass when | Traces to | Known open | R1 |
|---|---|---|---|---|---|
| UNI-1 | `brew uninstall --cask sift` | The app is gone. `~/Library/Containers/net.justinchung.sift` and the Keychain items remain | D-33, [README](README.md#what-the-cask-publishes) | | |
| UNI-2 | `brew install --cask local/sift/sift` again, and launch it | Every account is still there, and no one has to sign in again | D-33, D-45 | | |
| UNI-3 | `brew uninstall --zap --cask sift` | Every path in the Cask's `zap` stanza is gone | #19 | | |
| UNI-4 | After UNI-3, `security find-generic-password -s net.justinchung.sift` and a Keychain Access search for `net.justinchung.sift` | Neither finds anything. The Keychain Access search also covers the data-protection keychain, which the `security` command does not list | #19, NFR-23 | #141 (zap cannot reach data-protection items) | |

## Findings

One line for each `fail`: the run, the check, and the issue it was filed as.

| Run | Check | Issue |
|---|---|---|
