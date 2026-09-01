//! The commands. Each maps onto something a shell does, and the mutating ones go through
//! the action register by identifier — which is what makes this a shell rather than a
//! script that happens to call the same crates.

use sift_app::App;
use sift_credentials::store::CredentialStore as _;
use sift_foundation::identity::LocalId;
use sift_mutations::intent::{Intent, State};
use sift_presentation::action;
use sift_session::{Session, Watching};
use sift_subsystem::Subsystem;

type Output = Result<Vec<String>, String>;

pub fn run(session: &mut Session, line: &str) -> Output {
    let mut parts = line.split_whitespace();
    let Some(verb) = parts.next() else {
        return Ok(vec![]);
    };
    let rest: Vec<&str> = parts.collect();

    // D-24: the tag is task-scoped and re-established at each boundary, not set once. A
    // command is this shell's equivalent of a stage boundary.
    let subsystem = match verb {
        "ingest" | "list" | "row" | "folders" | "watch" => Subsystem::Store,
        "observe" | "poll" => Subsystem::Presentation,
        "do" | "queue" | "flush" | "restart" => Subsystem::Mutations,
        "actions" | "select" | "open" => Subsystem::Presentation,
        "sync" => Subsystem::Sync,
        "body" | "resource" | "close" | "blocked" | "links" => Subsystem::Sanitize,
        "net" => Subsystem::Network,
        _ => Subsystem::Shell,
    };
    sift_alloc::tagged(subsystem, || dispatch(session, verb, &rest))
}

fn dispatch(session: &mut Session, verb: &str, rest: &[&str]) -> Output {
    let rest = rest.to_vec();
    // The two verbs that are the session's rather than the application's: D-18's registry
    // sits above the application and is what a shell actually binds to.
    match verb {
        "observe" => return observe(session, &rest),
        "poll" => return poll(session),
        // D-98's register is presentation-layer, so invoking one is the session's rather than
        // the application's — and a shell binds to the session anyway.
        "do" => return invoke(session, &rest),
        "undo" => return undo(session),
        "actions" => return actions(session),
        _ => {}
    }
    let app = session.app_mut();
    match verb {
        "help" => Ok(help()),
        "account" => account(app, &rest),
        "ingest" => ingest(app, &rest),
        "list" => list(app, &rest),
        "row" => row(app, &rest),
        "select" => select(app, &rest),
        "open" => open(app, &rest),
        "window" => window(app, &rest),
        "queue" => queue(app, &rest),
        "flush" => flush(app, &rest),
        "restart" => restart(app, &rest),
        "memory" => memory(),
        "folders" => folders(app, &rest),
        "watch" => watch(app, &rest),
        "sync" => sync(app, &rest),
        "body" => body(app, &rest),
        "blocked" => blocked(app, &rest),
        "links" => links(app, &rest),
        "attachments" => attachments(app, &rest),
        "save" => save(app, &rest),
        "resource" => resource(app, &rest),
        "close" => close(app, &rest),
        "net" => net(app, &rest),
        other => Err(format!("unknown command `{other}` — try `help`")),
    }
}

fn help() -> Vec<String> {
    [
        "account add <name> <rich|minimal|unstable-ids>   add an account of a capability shape",
        "account add-replayed <name>                      an account backed by D-65's fixture corpus",
        "account writes <name> <on|off>                   authorize writes to a mailbox, or withdraw it",
        "account authorize <name> <client-id>             begin a real authorization; prints the address",
        "account callback <name> <url>                    finish one, from what the scheme handed back",
        "account forget <name>                            FR-4: erase every credential, by enumeration",
        "account list                                     accounts and what they declare",
        "folders <account>                                what the provider enumerates (D-83)",
        "watch <account> <remote-id> <on|off>             FR-43's watched set",
        "sync <account> [max-pages]                       cursor, delta, envelopes, one transaction",
        "body <id|#n>                                     fetch and render through the seven stages",
        "blocked <id|#n>                                  FR-29: what was withheld, and the rule",
        "links <id|#n>                                    FR-30/FR-42: where each link really goes",
        "attachments <id|#n>                              FR-10: what is carried, fetching none of it",
        "save <id|#n> <part> <dir> [write]                NFR-53: the final path, shown before the write",
        "resource <url|#n>                                answer one load, as the scheme handler does",
        "close <token|#>                                  revoke a document — D-90's navigation",
        "net [account]                                    bytes on the wire (FR-36)",
        "ingest <account> <subject>...                    ingest a message (delivered)",
        "list [account]                                   the message list, read THROUGH the overlay",
        "observe <account|*> [limit]                      register a D-18 observation; prints its handle",
        "poll                                             deliver what changed since the last poll",
        "row <account>                                    FR-6's fields, as a shell receives them",
        "select <id>...                                   set the selection (D-99, keyed on identity)",
        "open <id|#n>                                     open a message in the reader",
        "window <open|close>                              a window exists, or does not",
        "actions                                          the palette: a filtered view of the register",
        "undo                                             FR-15: reverse the last gesture, by compensation",
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
        ["writes", name, state] => {
            let enabled = match *state {
                "on" => true,
                "off" => false,
                _ => return Err("account writes <name> <on|off>".to_owned()),
            };
            app.set_writes_enabled(name, enabled)?;
            Ok(vec![if enabled {
                format!(
                    "`{name}` may now be changed: archive, move, flag, label, mark read, \
                     report junk, and move to the provider's own Trash."
                )
            } else {
                format!("`{name}` is watched only. Triage is recorded here and held.")
            }])
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

/// Register a D-18 observation, the way a shell does when a list appears on screen.
fn observe(session: &mut Session, args: &[&str]) -> Output {
    let (target, limit) = match args {
        [target] => (*target, L_LIST),
        [target, limit] => (
            *target,
            limit.parse().map_err(|_| "limit must be a number")?,
        ),
        _ => return Err("observe <account|*> [limit]".to_owned()),
    };
    // `*` is D-4's unified inbox: every account, merged, on one comparator.
    let account = if target == "*" {
        None
    } else {
        Some(session.app_mut().account(target)?.id)
    };
    let id = session.observe(Watching::Messages { account, limit });
    Ok(vec![format!("observing {} as #{}", target, id.0)])
}

/// Deliver what changed, the way the boundary does on the next turn of the shell's loop.
///
/// A poll that changed nothing prints nothing but the count, which is the property D-18
/// exists for: the common case is a signal that touched no window anybody is watching.
fn poll(session: &mut Session) -> Output {
    let deliveries = session.poll()?;
    let mut out = Vec::new();
    for d in &deliveries {
        out.push(format!(
            "#{} generation={} {} change(s), {} incoming",
            d.observation.0,
            d.generation.0,
            d.batch.changes.len(),
            d.batch.incoming.len()
        ));
        for change in &d.batch.changes {
            out.push(format!("   {change:?}"));
        }
    }
    out.push(format!("-- {} delivery(ies)", deliveries.len()));
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

/// FR-15 — what could be taken back, and taking it back.
///
/// Two verbs in one because they answer the same question: a countdown a shell cannot read is
/// a countdown a shell cannot draw.
fn undo(session: &mut Session) -> Output {
    let now = now_millis();
    let Some(record) = session.undoable() else {
        return Err("there is nothing to undo".to_owned());
    };
    let mut out = vec![if record.timed {
        format!(
            "undo `{}` over {} message(s) — {} ms left on FR-15's window",
            record.intent,
            record.messages,
            record.remaining_millis(now)
        )
    } else {
        format!(
            "undo `{}` over {} message(s) — no countdown; it stays reversible either way",
            record.intent, record.messages
        )
    }];
    let gesture = session.undo_last(now)?;
    out.extend(
        gesture
            .enqueued
            .iter()
            .map(|m| format!("{m}: {} enqueued", gesture.intent.unwrap_or_default())),
    );
    out.extend(gesture.skipped.iter().map(|(m, why)| format!("{m}: {why}")));
    // Never a queue retraction. The original may already have reached the server, and a
    // design that tries to cancel in flight has two outcomes to reason about.
    out.push("-- reversed by compensation, not by retraction".to_owned());
    Ok(out)
}

fn actions(session: &mut Session) -> Output {
    let mut out = vec![format!(
        "-- {} of {} available",
        session.palette().len(),
        action::ACTIONS.len()
    )];
    out.extend(session.palette().iter().map(|a| format!("  {}", a.id)));
    Ok(out)
}

fn invoke(session: &mut Session, args: &[&str]) -> Output {
    let [id, extra @ ..] = args else {
        return Err("do <action-id> [arg]".to_owned());
    };
    let parameter = extra.iter().find(|a| !a.starts_with("--")).copied();
    let confirmed = extra.contains(&"--confirmed");

    // D-98 makes an unavailable action **absent**, and a shell shows nothing at all. A harness
    // that also said nothing would be useless, so it reports which of the two gates closed —
    // which is exactly the information D-98 concedes a user does not get.
    if let Some(why) = session.why_unavailable(id) {
        return Err(format!("`{id}` is not available: {}", why.explain()));
    }
    let gesture = session.invoke(id, parameter, confirmed, now_millis())?;

    if !gesture.mutates {
        return Ok(vec![format!(
            "{}: no mutation (a navigation or a surface)",
            gesture.action
        )]);
    }
    let mut out: Vec<String> = gesture
        .enqueued
        .iter()
        .map(|m| format!("{m}: {} enqueued", gesture.intent.unwrap_or_default()))
        .collect();
    out.extend(gesture.skipped.iter().map(|(m, why)| format!("{m}: {why}")));
    out.push(format!(
        "-- optimistic: {}",
        if gesture.optimistic {
            "yes, before any round trip"
        } else {
            "no"
        }
    ));
    Ok(out)
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
    // The read-only posture, checked **before** anything is issued and before the adapter is
    // even taken. Everything up to here already happened: the intents were built, checked
    // against declared capabilities, written durably and applied optimistically. This is the
    // one step that cannot be taken back, and it is the one step an unauthorized account
    // does not take.
    if !app.may_issue(&name) {
        let held = app.held(&name);
        return Ok(vec![
            format!("0 issued — writes are not authorized for `{name}`"),
            format!(
                "{held} intent(s) held. Nothing has been sent to the provider, and nothing \
                 will be until `account writes {name} on`."
            ),
        ]);
    }
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
/// Render a message, and print everything the reader's chrome is drawn from.
///
/// **This calls the same [`App::open_document`] the macOS shell calls**, rather than repeating
/// the seven stages beside it. D-65's claim is that the harness drives the application through
/// the entry points a shell uses; a second render here would make that claim false in the one
/// place it is most worth being true.
fn body(app: &mut App, args: &[&str]) -> Output {
    let [reference] = args else {
        return Err("body <id|#n>".to_owned());
    };
    let id = resolve(app, reference)?;
    let document = app.open_document(id, false)?;

    let mut out = vec![
        format!("stages: {}", document.stages.join(" -> ")),
        format!("token: {}", document.token),
        format!(
            "{} fetching position(s), {} withheld, {} link(s)",
            document.fetching_positions,
            document.blocked,
            document.links.len()
        ),
    ];
    out.extend(withheld_lines(&document));
    out.extend(link_lines(&document));
    out.push(document.html);
    Ok(out)
}

/// FR-29's chrome as text: the count, then each refusal and the rule behind it.
///
/// The count leads because that is what the reader leads with, and the list follows because a
/// count with nothing behind it tells a user something happened without telling them what.
fn blocked(app: &mut App, args: &[&str]) -> Output {
    let [reference] = args else {
        return Err("blocked <id|#n>".to_owned());
    };
    let id = resolve(app, reference)?;
    let document = app.open_document(id, false)?;
    let mut out = vec![match document.blocked {
        0 => "nothing was withheld".to_owned(),
        1 => "1 remote resource not loaded".to_owned(),
        n => format!("{n} remote resources not loaded"),
    }];
    out.extend(withheld_lines(&document));
    out.push(if document.may_always_allow {
        "`always load from this sender` is offered".to_owned()
    } else {
        "`always load from this sender` is absent — nothing authenticated this message, so \
         there is no origin to key a durable allowance on"
            .to_owned()
    });
    Ok(out)
}

/// FR-30 and FR-42 — where each link goes, and where the unsubscribe destination does.
fn links(app: &mut App, args: &[&str]) -> Output {
    let [reference] = args else {
        return Err("links <id|#n>".to_owned());
    };
    let id = resolve(app, reference)?;
    let document = app.open_document(id, false)?;
    let mut out = vec![format!("{} link(s)", document.links.len())];
    out.extend(link_lines(&document));
    out.push(match &document.unsubscribe {
        None => "no unsubscribe destination is declared".to_owned(),
        Some(link) if link.needs_a_mail_handler => format!(
            "unsubscribe: {} — requires a mail handler; Sift will not send it",
            link.displayed
        ),
        Some(link) => format!(
            "unsubscribe: {} — opens in the browser on confirmation; Sift never requests it",
            link.displayed
        ),
    });
    Ok(out)
}

fn withheld_lines(document: &sift_app::document::Document) -> Vec<String> {
    document
        .withheld
        .iter()
        .map(|w| {
            format!(
                "  withheld {}@{}  {}  — {}",
                w.element, w.attribute, w.displayed, w.rule
            )
        })
        .collect()
}

fn link_lines(document: &sift_app::document::Document) -> Vec<String> {
    document
        .links
        .iter()
        .map(|l| {
            let mut line = format!("  link {}", l.displayed);
            if l.displayed != l.target {
                line.push_str(&format!("  -> {}", l.target));
            }
            if let Some(wrapper) = &l.wrapper {
                line.push_str(&format!("  (wrapped by {wrapper})"));
            }
            if l.needs_a_mail_handler {
                line.push_str("  [needs a mail handler]");
            }
            line
        })
        .collect()
}

/// FR-10 — what a message carries, without fetching any of it.
fn attachments(app: &mut App, args: &[&str]) -> Output {
    let [reference] = args else {
        return Err("attachments <id|#n>".to_owned());
    };
    let id = resolve(app, reference)?;
    let listed = app.attachments(id)?;
    if listed.is_empty() {
        return Ok(vec!["no attachments".to_owned()]);
    }
    let mut out = vec![format!("{} attachment(s), none fetched", listed.len())];
    for a in &listed {
        out.push(format!(
            "  {}  {}  {} B  as `{}`",
            a.part, a.media_type, a.declared_size, a.file_name
        ));
        if a.warning.required() {
            let mut why = Vec::new();
            if a.warning.declared {
                why.push("the declared type is executable");
            }
            if a.warning.extension {
                why.push("the name ends in an executable extension");
            }
            if a.warning.disagrees {
                why.push("the declared type and the name disagree");
            }
            out.push(format!("    warn before opening: {}", why.join("; ")));
        }
    }
    Ok(out)
}

/// NFR-53 — resolve the exact final path, show it, and only then write.
///
/// Two verbs rather than one, because the requirement is that the path be shown **before** the
/// write. A single call that saved and then reported would satisfy every test and none of the
/// requirement.
fn save(app: &mut App, args: &[&str]) -> Output {
    let (reference, part, directory, commit) = match args {
        [reference, part, directory] => (reference, part, directory, false),
        [reference, part, directory, "write"] => (reference, part, directory, true),
        _ => return Err("save <id|#n> <part> <directory> [write]".to_owned()),
    };
    let id = resolve(app, reference)?;
    let plan = app.plan_attachment_save(id, part, std::path::Path::new(directory))?;

    let mut out = vec![format!("would write: {}", plan.final_path.display())];
    if plan.renamed {
        out.push("  the name was derived — a sender-supplied name never becomes a path".to_owned());
    }
    if !commit {
        out.push("nothing written — repeat with `write` to confirm".to_owned());
        return Ok(out);
    }
    let (written, warning) = app.write_attachment(&plan)?;
    out.push(format!(
        "wrote {written} B to {}",
        plan.final_path.display()
    ));
    if warning.required() {
        out.push("  opening this needs a warning first (FR-10)".to_owned());
    }
    Ok(out)
}

/// Resolve one address under the internal scheme, as the body view's scheme handler does.
///
/// This is the channel N-1 leaves open, and the only one. It exists as a command so that the
/// property can be asserted rather than observed: a fabricated address resolves to nothing, a
/// revoked one stops resolving, and with no filter engine loaded every remote fetch is
/// refused because D-10 makes an absent authority **deny** rather than fall through.
fn resource(app: &mut App, args: &[&str]) -> Output {
    let [url] = args else {
        return Err("resource <url|#n>".to_owned());
    };
    // `#n` addresses position n of the live document. Tokens are minted per document and are
    // unguessable by design, so a scripted session cannot name one in advance — which is the
    // same problem `#n` solves for message identities, and the same answer.
    let owned;
    let url = if let Some(index) = url.strip_prefix('#') {
        let token = app
            .resources
            .live_tokens()
            .first()
            .map(|t| (*t).to_owned())
            .ok_or("no document is open — render one with `body` first")?;
        owned = format!(
            "{}://{token}/{index}",
            sift_foundation::identifiers::INTERNAL_SCHEME
        );
        owned.as_str()
    } else {
        *url
    };
    let answer = app.resolve_resource(url, None);
    Ok(vec![match answer {
        sift_broker::broker::Answer::Bytes { length } => format!("bytes: {length}"),
        sift_broker::broker::Answer::Blocked(reason) => format!("blocked: {reason:?}"),
        sift_broker::broker::Answer::Unavailable(why) => format!("unavailable: {why:?}"),
    }])
}

/// Revoke a document's token — D-90's navigation, without a view to navigate.
fn close(app: &mut App, args: &[&str]) -> Output {
    let [token] = args else {
        return Err("close <token|#>".to_owned());
    };
    let owned;
    let token: &str = if *token == "#" {
        owned = app
            .resources
            .live_tokens()
            .first()
            .map(|t| (*t).to_owned())
            .ok_or("no document is open")?;
        owned.as_str()
    } else {
        token
    };
    Ok(vec![if app.close_document(token) {
        format!("revoked {token}")
    } else {
        format!("{token} was not open")
    }])
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
