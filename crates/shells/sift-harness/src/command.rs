//! The commands. Each maps onto something a shell does, and the mutating ones go through
//! the action register by identifier — which is what makes this a shell rather than a
//! script that happens to call the same crates.

use sift_app::App;
use sift_credentials::store::CredentialStore as _;
use sift_foundation::identity::LocalId;
use sift_mutations::intent::{Intent, State};
use sift_presentation::action::{self, Context, MutationKind};
use sift_subsystem::Subsystem;

type Output = Result<Vec<String>, String>;

pub fn run(app: &mut App, line: &str) -> Output {
    let mut parts = line.split_whitespace();
    let Some(verb) = parts.next() else {
        return Ok(vec![]);
    };
    let rest: Vec<&str> = parts.collect();

    // D-24: the tag is task-scoped and re-established at each boundary, not set once. A
    // command is this shell's equivalent of a stage boundary.
    let subsystem = match verb {
        "ingest" | "list" | "row" | "folders" | "watch" => Subsystem::Store,
        "do" | "queue" | "flush" | "restart" => Subsystem::Mutations,
        "actions" | "select" | "open" => Subsystem::Presentation,
        "sync" => Subsystem::Sync,
        "body" => Subsystem::Sanitize,
        "net" => Subsystem::Network,
        _ => Subsystem::Shell,
    };
    sift_alloc::tagged(subsystem, || dispatch(app, verb, &rest))
}

fn dispatch(app: &mut App, verb: &str, rest: &[&str]) -> Output {
    let rest = rest.to_vec();
    match verb {
        "help" => Ok(help()),
        "account" => account(app, &rest),
        "ingest" => ingest(app, &rest),
        "list" => list(app, &rest),
        "row" => row(app, &rest),
        "select" => select(app, &rest),
        "open" => open(app, &rest),
        "window" => window(app, &rest),
        "actions" => actions(app),
        "do" => invoke(app, &rest),
        "queue" => queue(app, &rest),
        "flush" => flush(app, &rest),
        "restart" => restart(app, &rest),
        "memory" => memory(),
        "folders" => folders(app, &rest),
        "watch" => watch(app, &rest),
        "sync" => sync(app, &rest),
        "body" => body(app, &rest),
        "net" => net(app, &rest),
        other => Err(format!("unknown command `{other}` — try `help`")),
    }
}

fn help() -> Vec<String> {
    [
        "account add <name> <rich|minimal|unstable-ids>   add an account of a capability shape",
        "account add-replayed <name>                      an account backed by D-65's fixture corpus",
        "account authorize <name> <client-id>             begin a real authorization; prints the address",
        "account callback <name> <url>                    finish one, from what the scheme handed back",
        "account forget <name>                            FR-4: erase every credential, by enumeration",
        "account list                                     accounts and what they declare",
        "folders <account>                                what the provider enumerates (D-83)",
        "watch <account> <remote-id> <on|off>             FR-43's watched set",
        "sync <account> [max-pages]                       cursor, delta, envelopes, one transaction",
        "body <id|#n>                                     fetch and render through the seven stages",
        "net [account]                                    bytes on the wire (FR-36)",
        "ingest <account> <subject>...                    ingest a message (delivered)",
        "list [account]                                   the message list, read THROUGH the overlay",
        "row <account>                                    FR-6's fields, as a shell receives them",
        "select <id>...                                   set the selection (D-99, keyed on identity)",
        "open <id|#n>                                     open a message in the reader",
        "window <open|close>                              a window exists, or does not",
        "actions                                          the palette: a filtered view of the register",
        "do <action-id> [arg]                             invoke an action by identifier (D-98)",
        "queue [account]                                  the durable queue, by state",
        "flush <account> [--leave-in-flight]              issue one batch; optionally stop there",
        "restart <account>                                what a crash leaves: Issued -> Reconciling",
        "memory                                           per-subsystem attribution (D-24)",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

fn account(app: &mut App, args: &[&str]) -> Output {
    match args {
        ["add", name, shape] => {
            app.add_account(name, shape)?;
            Ok(vec![format!("added `{name}` ({shape})")])
        }
        ["add-replayed", name] => {
            // D-65. Not a convenience: a cursor outside the retained window, a throttle and
            // a batch answered out of order cannot be arranged against a real account on
            // demand, and a shell drivable only against one would leave every one of them
            // untested end to end.
            let adapter = crate::account::replayed();
            let id = app.add_provider_account(name, adapter)?;
            Ok(vec![format!("added `{name}` (replayed)  id={id}")])
        }
        ["authorize", name, client_id] => {
            // D-36's registration is checked *before* the flow starts. This shell has not
            // registered the scheme with the system — a bundle does that, and a bundle is
            // what this binary is not — so it says so rather than opening a browser the user
            // would return from to nothing.
            let registered = std::env::var("SIFT_CALLBACK_SCHEME_REGISTERED").is_ok();
            let url = crate::account::begin(&mut app.broker, client_id, registered, now_millis())
                .map_err(|e| e.to_string())?;
            app.pending_authorization
                .insert((*name).to_owned(), (*client_id).to_owned());
            Ok(vec![
                format!(
                    "the callback returns through the registered scheme{}",
                    if crate::account::callback_arrives_on_a_socket() {
                        " and a socket"
                    } else {
                        ", never a socket — NFR-24 admits none for any purpose"
                    }
                ),
                format!(
                    "open this, then paste what comes back to `account callback {name} <url>`:"
                ),
                url,
            ])
        }
        ["callback", name, callback] => {
            let client_id = app
                .pending_authorization
                .get(*name)
                .cloned()
                .ok_or_else(|| format!("no authorization is in progress for `{name}`"))?;
            let id = app.reserve_identity();
            let adapter =
                crate::account::complete(&mut app.broker, &client_id, id, callback, now_millis())
                    .map_err(|e| e.to_string())?;
            app.pending_authorization.remove(*name);
            let id = app.add_provider_account(name, adapter)?;
            Ok(vec![format!("added `{name}`  id={id}")])
        }
        ["forget", name] => {
            let id = app.account(name)?.id;
            // FR-4's erasure is local and provable, and it does **not** block on revoking
            // the grant at the provider: that would mean an account the user asked to remove
            // staying until a server answered.
            app.broker.erase(id).map_err(|e| e.to_string())?;
            let remaining = app.broker.store().remaining(id);
            Ok(vec![format!(
                "{name}: {} credential item(s) remain",
                remaining.len()
            )])
        }
        ["list"] => {
            let mut out = Vec::new();
            for (name, a) in app.accounts() {
                out.push(format!(
                    "{name}  id={}  tags={}  junk-report={}  batch={}  connections/3-folders={}",
                    a.id,
                    a.capabilities.offers_tags(),
                    a.capabilities.offers_junk_report(),
                    a.capabilities.batch_size(),
                    a.capabilities.connections_for(3),
                ));
            }
            if out.is_empty() {
                out.push("no accounts".to_owned());
            }
            Ok(out)
        }
        _ => Err("account add <name> <shape> | account list".to_owned()),
    }
}

fn ingest(app: &mut App, args: &[&str]) -> Output {
    let [name, subject @ ..] = args else {
        return Err("ingest <account> <subject>...".to_owned());
    };
    if subject.is_empty() {
        return Err("a message needs a subject".to_owned());
    }
    let subject = subject.join(" ");
    let a = app.account(name)?;
    let id = a.ids.next();
    // Normalized once, here, before anything renders or indexes it — NFR-54.
    let display = sift_foundation::normalize::for_display(&subject);
    let stored = display.as_str().to_owned();
    sift_app::insert_message(a, id, &stored, id.millis())?;
    a.subjects.insert(id, stored);
    Ok(vec![format!("{id}  delivered")])
}

fn list(app: &mut App, args: &[&str]) -> Output {
    let names: Vec<String> = match args {
        [] => app.account_names().into_iter().map(str::to_owned).collect(),
        [name] => vec![(*name).to_owned()],
        _ => return Err("list [account]".to_owned()),
    };
    let mut out = Vec::new();
    for name in names {
        let a = app.account(&name)?;
        // D-51 and D-55 both live in the projection now, rather than being reimplemented
        // here: a shell that reads through the overlay itself is a shell that can disagree
        // with the other one about what a person sees.
        let rows = sift_app::rows::message_rows(a, L_LIST)?;
        for row in &rows {
            let suffix = if row.pending.is_empty() {
                String::new()
            } else {
                format!("   [pending: {}]", row.pending.join(", "))
            };
            out.push(format!("{name}  {}  {}{suffix}", row.id, row.subject));
        }
    }
    if out.is_empty() {
        out.push("(empty)".to_owned());
    }
    Ok(out)
}

/// FR-6's row, field by field, as it crosses the boundary.
///
/// `list` prints what a person recognises; this prints what a shell is actually handed, so a
/// test can assert the fields rather than the sentence they were formatted into.
fn row(app: &mut App, args: &[&str]) -> Output {
    let [name] = args else {
        return Err("row <account>".to_owned());
    };
    let account = app.account(name)?;
    let rows = sift_app::rows::message_rows(account, L_LIST)?;
    let mut out = Vec::new();
    for r in &rows {
        out.push(format!(
            "{}  received={} origination={} unread={} flagged={} attachments={} thread={} \
             sender={} subject={} snippet={}",
            r.id,
            r.received_millis,
            r.origination_millis,
            r.unread,
            r.flagged,
            r.has_attachments,
            r.thread_count,
            r.sender,
            r.subject,
            r.snippet,
        ));
    }
    if out.is_empty() {
        out.push("(empty)".to_owned());
    }
    Ok(out)
}

/// How many rows a harness listing asks for.
///
/// A window rather than everything, because the projection is windowed and a harness that
/// asked for an unbounded set would be exercising a path no shell uses.
const L_LIST: u32 = 500;

fn parse_id(s: &str) -> Result<LocalId, String> {
    u128::from_str_radix(s, 16)
        .map(LocalId::from_u128)
        .map_err(|_| format!("`{s}` is not a message identity"))
}

fn select(app: &mut App, args: &[&str]) -> Output {
    // Identities are minted per run, so a scripted session cannot name them in advance.
    // `#n` selects the nth row of the current list instead, which is what a person driving
    // this by hand means anyway — and what makes a QA session reproducible.
    let mut ids = Vec::new();
    for a in args {
        if let Some(n) = a.strip_prefix('#') {
            let n: usize = n
                .parse()
                .map_err(|_| format!("`{a}` is not a row number"))?;
            let rows = visible_rows(app)?;
            let row = rows
                .get(n.checked_sub(1).ok_or("rows are numbered from 1")?)
                .ok_or_else(|| format!("there is no row {n}; the list has {}", rows.len()))?;
            ids.push(row.1);
        } else {
            ids.push(parse_id(a)?);
        }
    }
    app.selection = ids;
    Ok(vec![format!("{} selected", app.selection.len())])
}

/// Resolve `#n` against the current list, or a raw identity.
fn resolve(app: &mut App, reference: &str) -> Result<LocalId, String> {
    let Some(n) = reference.strip_prefix('#') else {
        return parse_id(reference);
    };
    let n: usize = n
        .parse()
        .map_err(|_| format!("`{reference}` is not a row number"))?;
    let rows = visible_rows(app)?;
    rows.get(n.checked_sub(1).ok_or("rows are numbered from 1")?)
        .map(|r| r.1)
        .ok_or_else(|| format!("there is no row {n}; the list has {}", rows.len()))
}

/// The rows a shell would be showing, in D-55's order, read through the overlay.
fn visible_rows(app: &mut App) -> Result<Vec<(String, LocalId)>, String> {
    let names: Vec<String> = app.account_names().into_iter().map(str::to_owned).collect();
    let mut rows = Vec::new();
    for name in names {
        let a = app.account(&name)?;
        let overlay = a.queue.overlay();
        for (id, _) in sift_app::list_messages(a)? {
            if overlay
                .for_message(id)
                .iter()
                .any(Intent::removes_from_view)
            {
                continue;
            }
            rows.push((name.clone(), id));
        }
    }
    Ok(rows)
}

fn open(app: &mut App, args: &[&str]) -> Output {
    let [reference] = args else {
        return Err("open <id|#n>".to_owned());
    };
    // The same references `select` takes. A command that accepted only raw identities would
    // be one no scripted session could use, which is the trap this whole shell exists to
    // avoid.
    let id = resolve(app, reference)?;
    app.open_message = Some(id);
    app.selection = vec![id];
    Ok(vec![format!("open {id}")])
}

fn window(app: &mut App, args: &[&str]) -> Output {
    match args {
        ["open"] => {
            app.has_window = true;
            Ok(vec!["window open".to_owned()])
        }
        ["close"] => {
            app.has_window = false;
            // FR-25: closing the window is not quitting. Sync continues, the queue
            // continues, and the process stays resident.
            Ok(vec![
                "window closed — still resident, still syncing".to_owned(),
            ])
        }
        _ => Err("window <open|close>".to_owned()),
    }
}

fn context<'a>(
    app: &App,
    caps: &'a Option<sift_provider::capability::Capabilities>,
) -> Context<'a> {
    Context {
        selection_len: app.selection.len(),
        has_open_message: app.open_message.is_some(),
        has_window: app.has_window,
        capabilities: caps.as_ref(),
    }
}

fn actions(app: &mut App) -> Output {
    let caps = app.selection_capabilities();
    let ctx = context(app, &caps);
    let mut out: Vec<String> = action::palette(&ctx)
        .iter()
        .map(|a| a.id.to_owned())
        .collect();
    out.push(format!(
        "-- {} of {} available",
        out.len(),
        action::ACTIONS.len()
    ));
    Ok(out)
}

fn invoke(app: &mut App, args: &[&str]) -> Output {
    let [id, extra @ ..] = args else {
        return Err("do <action-id> [arg]".to_owned());
    };
    let Some(action) = action::by_id(id) else {
        return Err(format!("no action `{id}` in the register"));
    };

    let caps = app.selection_capabilities();
    let ctx = context(app, &caps);
    if !action.is_available(&ctx) {
        // Not "disabled": absent. D-98 makes an unavailable action absent from the palette,
        // and a shell would show nothing at all. A harness that also said nothing would be
        // useless, so it says which of the two gates closed — which is exactly the
        // information D-98 concedes a user does not get ("one keystroke does nothing in one
        // account with no visible reason").
        let reason = if caps.is_none() && action.mutates.is_some() {
            "nothing in the selection belongs to an account, so no capabilities are known"
        } else if action.mutates.is_some() && caps.is_some() {
            "the account's declared capabilities do not permit it"
        } else {
            "its scope is not satisfied — check the selection, the open message, or the window"
        };
        return Err(format!("`{id}` is not available: {reason}"));
    }

    let Some(kind) = action.mutates else {
        return Ok(vec![format!(
            "{id}: no mutation (a navigation or a surface)"
        )]);
    };

    let intent = build_intent(kind, extra)?;
    if intent.requires_confirmation() {
        // FR-14's single exception. Confirmed before it is issued rather than undone after,
        // and never applied optimistically ahead of the server.
        let confirmed = extra.contains(&"--confirmed");
        if !confirmed {
            return Err(format!(
                "`{id}` requires confirmation before it is issued — repeat with --confirmed"
            ));
        }
    }

    let now = now_millis();
    let mut out = Vec::new();
    let selection = app.selection.clone();
    // D-85's undo group is assigned **at the gesture**, which is what makes FR-17's bulk
    // operation one undoable unit rather than a hundred.
    let undo_group = app.next_intent_id();
    for message in selection {
        let Some(owner) = app
            .owner_of(message)
            .cloned()
            .or_else(|| app.owner_of_stored(message))
        else {
            out.push(format!("{message}: no account holds this message"));
            continue;
        };
        let intent_id = app.next_intent_id();
        let a = app.account(&owner)?;
        let sequence = a.queue.enqueue(intent_id, message, intent.clone(), now);

        // **Journal first, store second** — D-74's ordering, and the reverse would lose a
        // mutation the user watched succeed. The failure this order can leave is an intent
        // enqueued whose optimistic effect was never applied, which is invisible and
        // self-correcting because the overlay is derived from the queue.
        let durable =
            a.store.journal.execute(
                "INSERT INTO intent (id, undo_group, message_id, operation, intent_version,
                                 state, created_millis, per_message_seq, expires_millis)
             VALUES (?1, ?2, ?3, ?4, 1, 'Pending', ?5, ?6, ?7)",
                rusqlite::params![
                    intent_id.to_be_bytes().to_vec(),
                    undo_group.to_be_bytes().to_vec(),
                    message.to_bytes().to_vec(),
                    intent.name(),
                    i64::try_from(now).unwrap_or(i64::MAX),
                    i64::try_from(sequence).unwrap_or(i64::MAX),
                    i64::try_from(now.saturating_add(
                        sift_foundation::limits::L17_INTENT_EXPIRY.as_millis() as u64
                    ))
                    .unwrap_or(i64::MAX),
                ],
            );
        if let Err(e) = durable {
            return Err(format!("the journal refused the intent: {e}"));
        }
        out.push(format!("{message}: {} enqueued", intent.name()));
    }
    out.push(format!(
        "-- optimistic: {}",
        if intent.applies_optimistically() {
            "yes, before any round trip"
        } else {
            "no"
        }
    ));
    Ok(out)
}

fn build_intent(kind: MutationKind, extra: &[&str]) -> Result<Intent, String> {
    let arg = extra.iter().find(|a| !a.starts_with("--"));
    Ok(match kind {
        MutationKind::Archive => Intent::Archive,
        MutationKind::DeleteToTrash => Intent::DeleteToTrash,
        MutationKind::PermanentlyDelete => Intent::PermanentlyDelete,
        MutationKind::MoveTo => Intent::MoveTo {
            folder: arg
                .ok_or("move-to needs a folder")?
                .parse()
                .map_err(|_| "folder must be a number")?,
        },
        MutationKind::Flag => Intent::Flag,
        MutationKind::MarkRead => Intent::MarkRead,
        MutationKind::MarkUnread => Intent::MarkUnread,
        MutationKind::AddTag => Intent::AddTag {
            name: (*arg.ok_or("add-tag needs a name")?).to_owned(),
        },
        MutationKind::RemoveTag => Intent::RemoveTag {
            name: (*arg.ok_or("remove-tag needs a name")?).to_owned(),
        },
        MutationKind::ReportJunk => Intent::ReportJunk,
        MutationKind::ReportNotJunk => Intent::ReportNotJunk,
    })
}

fn queue(app: &mut App, args: &[&str]) -> Output {
    let names: Vec<String> = match args {
        [] => app.account_names().into_iter().map(str::to_owned).collect(),
        [name] => vec![(*name).to_owned()],
        _ => return Err("queue [account]".to_owned()),
    };
    let mut out = Vec::new();
    for name in names {
        let a = app.account(&name)?;
        for e in a.queue.entries() {
            out.push(format!(
                "{name}  {}  seq={}  {}  {}",
                e.message,
                e.sequence,
                e.intent.name(),
                e.state.name()
            ));
        }
    }
    if out.is_empty() {
        out.push("(queue empty)".to_owned());
    }
    Ok(out)
}

fn flush(app: &mut App, args: &[&str]) -> Output {
    let (name, in_flight) = match args {
        [name] => ((*name).to_owned(), false),
        // Stop after issuing, so the crash path can actually be driven. Without this the
        // Issued state is unobservable from outside, and D-85's durable marker — the write
        // that costs something on every intent forever — would be untestable from a shell.
        [name, "--leave-in-flight"] => ((*name).to_owned(), true),
        _ => return Err("flush <account> [--leave-in-flight]".to_owned()),
    };
    let account = app.account(&name)?;

    // An account with no provider behind it: the queue's own state machine, driven without a
    // wire. This is what the planner tests use, and it is deliberately still a real queue.
    let Some(adapter) = account.adapter.take() else {
        let limit = account.capabilities.batch_size();
        let batch: Vec<u128> = account
            .queue
            .next_batch(limit)
            .iter()
            .map(|q| q.id)
            .collect();
        let mut out = vec![format!("issuing {} of at most {limit}", batch.len())];
        for id in &batch {
            account.queue.set_state(*id, State::Issued);
        }
        if in_flight {
            out.push(format!(
                "{} left Issued — the request went out and no answer came back",
                batch.len()
            ));
            return Ok(out);
        }
        for id in &batch {
            account.queue.set_state(*id, State::Settled);
        }
        account.queue.collect_settled();
        out.push(format!(
            "{} settled, {} still queued",
            batch.len(),
            account.queue.len()
        ));
        return Ok(out);
    };

    if in_flight {
        // Against a real provider there is nothing to stop between the marker and the
        // request: the whole point of D-85's ordering is that the two are not separable from
        // outside. The crash path is driven with `restart` instead.
        account.adapter = Some(adapter);
        return Err(
            "--leave-in-flight is for an account with no provider; use `restart` to drive              what a crash leaves"
                .to_owned(),
        );
    }

    // The durable half of D-85's marker. It is a callback because the journal is the store's
    // and the queue cannot reach it — the two sit side by side in the application layer and
    // D-59 gives neither an edge to the other.
    let journal = &account.store.journal;
    let mut mark_issued = |ids: &[u128]| -> Result<(), String> {
        let transaction = journal.unchecked_transaction().map_err(|e| e.to_string())?;
        for id in ids {
            transaction
                .execute(
                    "UPDATE intent SET state = 'Issued' WHERE id = ?1",
                    rusqlite::params![id.to_be_bytes().to_vec()],
                )
                .map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())
    };

    let resolve = sift_app::Remote(&account.store.store);
    let outcome = sift_mutations::flush::flush_once(
        adapter.as_ref(),
        &mut account.queue,
        &resolve,
        &mut mark_issued,
    );
    account.adapter = Some(adapter);

    let (report, failure) = match outcome {
        Ok(report) => (report, None),
        Err(failure) => (failure.report.clone(), Some(failure.error)),
    };

    let mut out = vec![format!(
        "{} issued: {} applied, {} refused, {} deferred, {} reconciling, {} quarantined",
        report.issued,
        report.applied,
        report.refused,
        report.deferred,
        report.reconciling,
        report.quarantined
    )];
    if let Some(delay) = report.retry_after_millis {
        out.push(format!(
            "the provider stated {delay} ms — a deadline on the wheel, never a sleep"
        ));
    }
    if let Some(error) = failure {
        out.push(format!("failed: {error}"));
    }
    out.push(format!("{} still queued", account.queue.len()));
    Ok(out)
}

fn restart(app: &mut App, args: &[&str]) -> Output {
    let [name] = args else {
        return Err("restart <account>".to_owned());
    };
    let name = (*name).to_owned();
    let a = app.account(&name)?;
    // What a crash leaves. A request that went out and whose answer never came back is
    // exactly this, and inferring sent-versus-unsent at startup cannot be done correctly.
    a.queue.recover_after_restart();
    let reconciling = a
        .queue
        .entries()
        .iter()
        .filter(|q| q.state == State::Reconciling)
        .count();
    Ok(vec![format!(
        "{reconciling} intent(s) moved to Reconciling"
    )])
}

fn memory() -> Output {
    let mut out = Vec::new();
    for s in Subsystem::ALL {
        let bytes = sift_alloc::live_bytes(*s);
        if bytes != 0 {
            out.push(format!("{:<14} {bytes:>12}", s.name()));
        }
    }
    out.push(format!(
        "{:<14} {:>12}",
        "-- total",
        sift_alloc::total_attributed()
    ));
    out.push(
        "   (the residual — footprint minus this — is what the soak gate sends you to)".to_owned(),
    );
    Ok(out)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

// ---------------------------------------------------------------------------
// A live account: folders, the watched set, syncing, and reading a message.
// ---------------------------------------------------------------------------

/// D-83's enumeration, reconciled into the store.
fn folders(app: &mut App, args: &[&str]) -> Output {
    let [name] = args else {
        return Err("folders <account>".to_owned());
    };
    let account = app.account(name)?;
    let adapter = account
        .adapter
        .as_ref()
        .ok_or("this account has no provider behind it")?;
    let report = sift_sync::run::discover_folders(adapter.as_ref(), &account.store)
        .map_err(|e| e.to_string())?;

    let mut out = vec![format!(
        "{} discovered, {} retired, {} unchanged",
        report.discovered.len(),
        report.retired.len(),
        report.unchanged
    )];
    let mut stmt = account
        .store
        .store
        .prepare(
            "SELECT id, remote_id, special_use, display_name, watched, retired
             FROM folder ORDER BY id",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)? != 0,
                r.get::<_, i64>(5)? != 0,
            ))
        })
        .map_err(|e| e.to_string())?;
    for row in rows {
        let (id, remote, special, display, watched, retired) = row.map_err(|e| e.to_string())?;
        out.push(format!(
            "  {id}  {remote:<20} {display:<16} use={special:<8} {}{}",
            if watched { "watched" } else { "-" },
            // D-83: retired, never deleted. Its messages stop being present *in it* and stay
            // reachable through search and threads.
            if retired { "  retired" } else { "" }
        ));
    }
    Ok(out)
}

/// FR-43's watched set, per account and persisted.
fn watch(app: &mut App, args: &[&str]) -> Output {
    let [name, remote, state] = args else {
        return Err("watch <account> <remote-id> <on|off>".to_owned());
    };
    let on = match *state {
        "on" => true,
        "off" => false,
        _ => return Err("watch <account> <remote-id> <on|off>".to_owned()),
    };
    let account = app.account(name)?;
    let folder = sift_sync::ingest::folder_local_id(
        &account.store.store,
        &sift_provider::adapter::RemoteFolderId((*remote).to_owned()),
    )
    .map_err(|e| e.to_string())?;
    sift_sync::ingest::set_watched(&account.store.store, folder, on).map_err(|e| e.to_string())?;
    Ok(vec![format!(
        "{remote} {}",
        if on {
            "watched"
        } else {
            // Deliberately not a seventh folder state: an unwatched folder stops being
            // scheduled and its stored state is *retained*, so re-watching does not restart
            // from Unsynced.
            "unwatched — its state is kept, so re-watching resumes"
        }
    )])
}

/// One turn of the sync: cursor, delta, envelopes, one transaction.
fn sync(app: &mut App, args: &[&str]) -> Output {
    let (name, pages) = match args {
        [name] => ((*name).to_owned(), 20usize),
        [name, pages] => (
            (*name).to_owned(),
            pages.parse().map_err(|_| "max-pages must be a number")?,
        ),
        _ => return Err("sync <account> [max-pages]".to_owned()),
    };
    let account = app.account(&name)?;
    let adapter = account
        .adapter
        .take()
        .ok_or("this account has no provider behind it")?;

    // Folders first: a delta needs somewhere to put what it finds, and D-83 assigns local
    // identity on discovery rather than on first use.
    fn turn(
        adapter: &crate::account::Live,
        account: &mut sift_app::OpenAccount,
        pages: usize,
    ) -> Result<sift_sync::ingest::PageReport, sift_sync::run::RunError> {
        sift_sync::run::discover_folders(adapter.as_ref(), &account.store).and_then(|_| {
            sift_sync::run::sync_account(adapter.as_ref(), &mut account.store, &account.ids, pages)
        })
    }
    let mut outcome = turn(&adapter, account, pages);

    // The token expired. D-88's single-flight refresh runs in the broker — this shell asks
    // for one and tries again **once**, because a second failure after a fresh credential is
    // not about the credential.
    //
    // The account is not torn down and the user is not prompted: only the refresh itself can
    // conclude that the *grant* is gone, and treating a rejected access token as that
    // conclusion is how an application appears to have lost the user's credentials when it
    // has merely been running for an hour.
    let mut refreshed = false;
    if matches!(
        &outcome,
        Err(sift_sync::run::RunError::Provider {
            failure: sift_provider::adapter::Failure::CredentialRefused,
            ..
        })
    ) {
        let account_id = account.id;
        match refresh_credential(app, &name, account_id) {
            Ok(access) => {
                adapter.present_credential(&access);
                outcome = turn(&adapter, app.account(&name)?, pages);
                refreshed = true;
            }
            Err(why) => {
                let account = app.account(&name)?;
                account.adapter = Some(adapter);
                return Err(why);
            }
        }
    }

    let account = app.account(&name)?;
    let account_id = account.id;
    account.adapter = Some(adapter);
    let report = outcome.map_err(|e| e.to_string())?;
    if refreshed {
        // The new pair completed a request, so D-88's superseded one can go. It is retained
        // until exactly this moment, because against a provider that rotates refresh tokens
        // a crash in between would leave the account holding one the provider has already
        // invalidated, and no way back.
        let _ = app.broker.confirm(account_id);
    }

    Ok(vec![
        format!(
            "{} inserted, {} updated, {} removed",
            report.inserted, report.updated, report.removed
        ),
        format!(
            "{} delivered — FR-23's new mail, which is delivered-and-unread at this moment \
             and cannot be reconstructed later",
            report.delivered
        ),
    ])
}

/// Ask the broker for a fresh access token.
///
/// Every rule about how that happens is the broker's — this only says which registration to
/// refresh against, which is the shell's because the client identifier is.
fn refresh_credential(
    app: &mut App,
    name: &str,
    account: sift_foundation::identity::AccountId,
) -> Result<String, String> {
    let client_id = app
        .pending_authorization
        .get(name)
        .cloned()
        .or_else(|| std::env::var("SIFT_OAUTH_CLIENT_ID").ok())
        .ok_or("no client identifier is known for this account, so it cannot be refreshed")?;
    let registration = crate::account::registration(crate::account::default_kind(), &client_id)?;
    let mut transport = sift_http::Https::to(&registration.profile.token.host)
        .map_err(|why| format!("the trust store could not be consulted: {why}"))?;
    app.broker
        .refresh(&mut transport, &registration, account)
        .map(|pair| pair.access)
        .map_err(|e| e.to_string())
}

/// Fetch a message's body and run it through the seven stages.
fn body(app: &mut App, args: &[&str]) -> Output {
    let [reference] = args else {
        return Err("body <id|#n>".to_owned());
    };
    let id = resolve(app, reference)?;
    let owner = app
        .owner_of_stored(id)
        .ok_or("no account holds this message")?;
    let account = app.account(&owner)?;
    let remote: String = account
        .store
        .store
        .query_row(
            "SELECT remote_id FROM message WHERE id = ?1",
            rusqlite::params![id.to_bytes().to_vec()],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
        .flatten()
        .ok_or("this message has no remote identifier yet — sync first")?;
    let adapter = account
        .adapter
        .as_ref()
        .ok_or("this account has no provider behind it")?;

    // **Structure first, and then one part.** The structure costs a few kilobytes; the
    // attachment beside it costs nothing until somebody asks for it, which is a claim about
    // *requests* rather than about intentions.
    let remote_id = sift_provider::adapter::RemoteMessageId(remote);
    let parts = adapter.structure(&remote_id).map_err(|e| e.to_string())?;
    // Stage 2, and the same rule a parsed tree goes through: HTML preferred, plain text as
    // the fallback, and plain text where the HTML exceeds L-1 — which rejects rather than
    // truncating, and here the rejection has somewhere honest to land.
    let chosen =
        sift_mime::select::choose(&parts).ok_or("this message carries no renderable part")?;
    let bytes = adapter
        .fetch_part(&remote_id, &chosen.part)
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let selected = sift_pipeline::Selected {
        html: chosen.is_html.then(|| text.clone()),
        text: (!chosen.is_html).then_some(text),
        reason: Some(format!("{:?}", chosen.reason)),
    };

    let mut broker = sift_broker::broker::Broker::new();
    let mut context = sift_pipeline::Context {
        // Nothing has authenticated this message yet, so the origin is null and every
        // resource is third-party under the strictest rules. That is the correct answer
        // rather than a placeholder: D-11's fourth priority is exactly this case.
        origin: sift_block::origin::Origin::Null,
        blocker: None,
        dark: false,
        broker: &mut broker,
    };
    let rendered = sift_pipeline::render(&selected, &mut context).map_err(|e| e.to_string())?;

    let mut out = vec![
        format!(
            "part {} ({}) — {}",
            chosen.part,
            if chosen.is_html {
                "text/html"
            } else {
                "text/plain"
            },
            selected.reason.clone().unwrap_or_default()
        ),
        format!("{} part(s) described, {} fetched", parts.len(), 1),
        format!("stages: {}", rendered.stages.join(" -> ")),
        format!("token: {}", rendered.token.as_str()),
        format!(
            "{} fetching position(s), {} link(s), {} removal(s)",
            rendered.positions.len(),
            rendered.links.len(),
            rendered.removals.len()
        ),
    ];
    for (position, verdict) in rendered.positions.iter().zip(rendered.verdicts.iter()) {
        out.push(format!(
            "  [{}] {}@{}  {}  -> {}",
            position.index,
            position.element,
            position.attribute,
            position.original,
            if verdict.permits_fetch() {
                "allowed"
            } else {
                "blocked"
            }
        ));
    }
    for removal in &rendered.removals {
        out.push(format!("  removed {} — {}", removal.what, removal.rule));
    }
    out.push(rendered.html);
    Ok(out)
}

/// FR-36 counts bytes on the wire.
fn net(app: &mut App, args: &[&str]) -> Output {
    let names: Vec<String> = match args {
        [] => app.account_names().into_iter().map(str::to_owned).collect(),
        [name] => vec![(*name).to_owned()],
        _ => return Err("net [account]".to_owned()),
    };
    let mut out = Vec::new();
    for name in names {
        let account = app.account(&name)?;
        let Some(adapter) = account.adapter.as_ref() else {
            continue;
        };
        let (sent, received) = adapter.wire_bytes();
        out.push(format!("{name}  sent {sent}  received {received}"));
    }
    if out.is_empty() {
        out.push("no account has a provider behind it".to_owned());
    }
    out.push(
        "   (bytes on the interface, so the handshake, the framing and the headers are in \
         there — a count above the encryption would understate a metered link)"
            .to_owned(),
    );
    Ok(out)
}
