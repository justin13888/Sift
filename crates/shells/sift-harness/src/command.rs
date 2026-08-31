//! The commands. Each maps onto something a shell does, and the mutating ones go through
//! the action register by identifier — which is what makes this a shell rather than a
//! script that happens to call the same crates.

use crate::app::App;
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
        "ingest" | "list" => Subsystem::Store,
        "do" | "queue" | "flush" | "restart" => Subsystem::Mutations,
        "actions" | "select" | "open" => Subsystem::Presentation,
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
        "select" => select(app, &rest),
        "open" => open(app, &rest),
        "window" => window(app, &rest),
        "actions" => actions(app),
        "do" => invoke(app, &rest),
        "queue" => queue(app, &rest),
        "flush" => flush(app, &rest),
        "restart" => restart(app, &rest),
        "memory" => memory(),
        other => Err(format!("unknown command `{other}` — try `help`")),
    }
}

fn help() -> Vec<String> {
    [
        "account add <name> <rich|minimal|unstable-ids>   add an account of a capability shape",
        "account list                                     accounts and what they declare",
        "ingest <account> <subject>...                    ingest a message (delivered)",
        "list [account]                                   the message list, read THROUGH the overlay",
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
    crate::app::insert_message(a, id, &stored, id.millis())?;
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
        // D-51: everything the user sees reads **through** the overlay. A read path that
        // forgets shows the server's opinion instead of the user's.
        let overlay = a.queue.overlay();
        // Read back out of the store, in D-55's order, rather than out of a map the
        // harness kept — otherwise the ordering this prints would prove nothing.
        let rows = crate::app::list_messages(a)?;
        for (id, subject) in &rows {
            let pending = overlay.for_message(*id);
            if pending.iter().any(|i| i.removes_from_view()) {
                // Optimistically gone. The row left the list before any round trip — NFR-7.
                continue;
            }
            let marks: Vec<&str> = pending.iter().map(Intent::name).collect();
            let suffix = if marks.is_empty() {
                String::new()
            } else {
                format!("   [pending: {}]", marks.join(", "))
            };
            out.push(format!("{name}  {id}  {subject}{suffix}"));
        }
    }
    if out.is_empty() {
        out.push("(empty)".to_owned());
    }
    Ok(out)
}

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
        for (id, _) in crate::app::list_messages(a)? {
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
    for message in selection {
        let Some(owner) = app.owner_of(message).cloned() else {
            out.push(format!("{message}: no account holds this message"));
            continue;
        };
        let intent_id = app.next_intent_id();
        let a = app.account(&owner)?;
        a.queue.enqueue(intent_id, message, intent.clone(), now);
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
    let a = app.account(&name)?;
    let limit = a.capabilities.batch_size();
    let batch: Vec<u128> = a.queue.next_batch(limit).iter().map(|q| q.id).collect();
    let mut out = vec![format!("issuing {} of at most {limit}", batch.len())];
    // D-85: the transition into Issued is durable and happens **before the request leaves**.
    for id in &batch {
        a.queue.set_state(*id, State::Issued);
    }
    if in_flight {
        out.push(format!(
            "{} left Issued — the request went out and no answer came back",
            batch.len()
        ));
        return Ok(out);
    }
    for id in &batch {
        a.queue.set_state(*id, State::Settled);
    }
    a.queue.collect_settled();
    out.push(format!(
        "{} settled, {} still queued",
        batch.len(),
        a.queue.len()
    ));
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
