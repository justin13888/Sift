//! Whole sessions, driven the way a person drives the harness.
//!
//! These are the QA sessions this binary exists for, run in CI rather than by hand. Each
//! one asserts a property the specification states, through the same commands and the same
//! action identifiers a shell would use — no test-only entry points, and nothing reaching
//! past the register into the crates beneath it.

use std::process::Command;

/// Run a session and return its transcript.
fn session(commands: &[&str]) -> String {
    let exe = env!("CARGO_BIN_EXE_sift-harness");
    let out = Command::new(exe)
        .args(commands)
        .output()
        .expect("harness runs");
    assert!(out.status.success(), "harness exited with {}", out.status);
    String::from_utf8(out.stdout).expect("utf-8")
}

fn base() -> Vec<&'static str> {
    vec![
        "account add work rich",
        "account add legacy minimal",
        "window open",
    ]
}

#[test]
fn a_triage_gesture_leaves_the_list_before_any_round_trip() {
    // NFR-7: the action is reflected in the UI within 16 ms, optimistically, before any
    // network. D-51 is what makes that possible — the row is hidden by the overlay, not by
    // anything the server said.
    let mut cmds = base();
    cmds.extend([
        "ingest work Quarterly report",
        "ingest work Newsletter",
        // The list is newest-first under D-55, so this one is row 1.
        "select #1",
        "do message.archive",
        "list",
    ]);
    let out = session(&cmds);
    assert!(out.contains("archive enqueued"));
    assert!(out.contains("optimistic: yes"));
    let after = out
        .rsplit("optimistic: yes, before any round trip")
        .next()
        .unwrap();
    assert!(
        !after.contains("Newsletter"),
        "the archived message was still listed"
    );
    assert!(
        after.contains("Quarterly report"),
        "an unrelated message vanished"
    );
}

#[test]
fn the_same_action_is_offered_on_one_account_and_absent_on_another() {
    // D-12's binding rule, observable from outside: the interface changes shape because the
    // account's declared capabilities differ, not because anything matched on a provider.
    let mut cmds = base();
    cmds.extend([
        "ingest legacy Server notice",
        "select #1",
        "do message.add-tag urgent",
    ]);
    let refused = session(&cmds);
    assert!(
        refused.contains("declared capabilities do not permit it"),
        "an account without tag support accepted a tag: {refused}"
    );

    let mut cmds = base();
    cmds.extend([
        "ingest work Quarterly report",
        "select #1",
        "do message.add-tag urgent",
    ]);
    assert!(session(&cmds).contains("add-tag enqueued"));
}

#[test]
fn permanent_delete_is_confirmed_before_it_is_issued_and_never_optimistic() {
    // FR-14's single exception, and the only intent with no compensation. Confirmed in
    // advance rather than undone afterwards, because there is nothing to undo.
    let mut cmds = base();
    cmds.extend([
        "ingest work Doomed",
        "select #1",
        "do message.permanently-delete",
    ]);
    let out = session(&cmds);
    assert!(out.contains("requires confirmation before it is issued"));

    let mut cmds = base();
    cmds.extend([
        "ingest work Doomed",
        "select #1",
        "do message.permanently-delete --confirmed",
    ]);
    let out = session(&cmds);
    assert!(out.contains("permanently-delete enqueued"));
    assert!(
        out.contains("optimistic: no"),
        "permanent delete was applied optimistically"
    );
}

#[test]
fn a_crash_between_issuing_and_answering_leaves_intents_reconciling() {
    // D-85: restart moves Issued to Reconciling, because a request that went out and whose
    // answer never came back is exactly that — and inferring sent-versus-unsent at startup
    // cannot be done correctly, which is what the durable marker is paying for.
    let mut cmds = base();
    cmds.extend([
        "ingest work A",
        "ingest work B",
        "select #1 #2",
        "do message.archive",
        "flush work --leave-in-flight",
        "queue work",
        "restart work",
        "queue work",
    ]);
    let out = session(&cmds);
    assert!(
        out.contains("archive  Issued"),
        "nothing was left in flight"
    );
    assert!(out.contains("2 intent(s) moved to Reconciling"));
    assert!(out.contains("archive  Reconciling"));
}

#[test]
fn an_undo_inside_one_flush_interval_reaches_the_server_as_nothing() {
    // D-86's saving, end to end: mark-read then mark-unread has no net effect at the server,
    // so the pair collapses and nothing is issued — with no mechanism that knows what an
    // undo window is.
    let mut cmds = base();
    cmds.extend([
        "ingest work A",
        "select #1",
        "do message.mark-read",
        "do message.mark-unread",
        "queue work",
    ]);
    assert!(
        session(&cmds).contains("(queue empty)"),
        "the pair survived to become two round trips"
    );
}

#[test]
fn a_junk_report_and_its_opposite_do_not_collapse() {
    // The pair the coalescing rule exists to exclude: a report has a durable remote effect
    // and no final state to reason about, so the two together are two things that happened.
    let mut cmds = base();
    cmds.extend([
        "ingest work A",
        "select #1",
        "do message.report-junk",
        "do message.report-not-junk",
        "queue work",
    ]);
    let out = session(&cmds);
    assert!(out.contains("report-junk"), "the report was coalesced away");
    assert!(out.contains("report-not-junk"));
}

#[test]
fn a_selection_spanning_accounts_resolves_to_the_capability_intersection() {
    // D-99. The recorded cost is that this shrinks silently as a selection widens, which is
    // exactly what the counts below show.
    let mut cmds = base();
    cmds.extend(["ingest legacy B", "ingest work A", "select #1", "actions"]);
    let minimal_only = session(&cmds);

    let mut cmds = base();
    cmds.extend(["ingest legacy B", "ingest work A", "select #2", "actions"]);
    let rich_only = session(&cmds);

    let mut cmds = base();
    cmds.extend([
        "ingest legacy B",
        "ingest work A",
        "select #1 #2",
        "actions",
    ]);
    let both = session(&cmds);

    let count = |s: &str| -> usize {
        s.lines()
            .find(|l| l.starts_with("-- "))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|n| n.parse().ok())
            .expect("a count line")
    };
    assert!(
        count(&rich_only) > count(&minimal_only),
        "the two shapes offer the same actions"
    );
    assert_eq!(
        count(&both),
        count(&minimal_only),
        "a mixed selection offered more than the weaker account can do"
    );
}

#[test]
fn closing_the_window_is_not_quitting() {
    // FR-25, which docs/architecture/process-model.md calls the single most likely source of
    // user distrust in the whole design.
    let out = session(&["account add work rich", "window open", "window close"]);
    assert!(out.contains("still resident, still syncing"));
}

#[test]
fn application_actions_survive_the_window_closing() {
    // Sift is resident with nothing on screen, and three host callbacks must work in exactly
    // that state. An action set that needed a window would make FR-25's distinction a lie.
    let out = session(&[
        "account add work rich",
        "window close",
        "do app.pause-sync",
        "do app.quit",
    ]);
    assert!(
        !out.contains("error:"),
        "an application action needed a window: {out}"
    );
}

#[test]
fn every_action_in_the_register_is_reachable_by_identifier() {
    // FR-24's testability claim: a keyboard-complete application is one that can be driven
    // without UI automation at all. If an action existed that the harness could not name,
    // that claim would be false.
    let out = session(&[
        "account add work rich",
        "window open",
        "ingest work A",
        "select #1",
        "actions",
    ]);
    assert!(out.contains("message.archive"));
    assert!(out.contains("app.command-palette"));
    assert!(out.contains("undo.last-gesture"));
}

#[test]
fn there_is_no_way_to_send_a_message() {
    // The constraint the whole product is defined by, checked from the outside. FR-41's
    // handoff actions exist; nothing constructs or transmits anything.
    let out = session(&[
        "account add work rich",
        "window open",
        "ingest work A",
        "open #1",
        "actions",
    ]);
    for forbidden in ["compose", "message.send", "draft", "outbox"] {
        assert!(
            !out.contains(forbidden),
            "the register offers `{forbidden}`"
        );
    }
    assert!(out.contains("read.reply"), "FR-41's handoff is missing");
    assert!(out.contains("read.forward"));
}

#[test]
fn allocation_is_attributed_to_a_subsystem() {
    // D-24, end to end. The residual is total footprint minus this, and it is what the soak
    // gate sends a maintainer to look at — so a total of zero would mean the gate points
    // nowhere.
    let out = session(&["account add work rich", "ingest work A", "list", "memory"]);
    let total: i64 = out
        .lines()
        .find(|l| l.starts_with("-- total"))
        .and_then(|l| l.split_whitespace().last())
        .and_then(|n| n.parse().ok())
        .expect("a total line");
    assert!(total > 0, "nothing was attributed at all");
    assert!(
        out.contains("store"),
        "the store's allocations were not attributed to it"
    );
}

// ---------------------------------------------------------------------------
// A live account, driven the whole way: added, folders enumerated, backfilled,
// delta-synced, read, mutated and flushed.
//
// The account is backed by D-65's fixture corpus rather than by a socket. That
// is the point rather than a compromise: `docs/build/verification.md` puts the
// harness in P0 **before the adapters it tests**, because it makes a provider's
// behaviour assertable "against servers nobody has" — and every property below
// is one nobody can arrange against a real account on demand.
// ---------------------------------------------------------------------------

fn live() -> Vec<&'static str> {
    vec!["account add-replayed mail", "window open"]
}

#[test]
fn an_account_can_be_added_synced_and_read_without_a_person_touching_a_pointer() {
    // FR-24's claim, taken at its word: a keyboard-complete application is one that can be
    // driven with no UI automation at all.
    let mut cmds = live();
    cmds.extend(["folders mail", "sync mail", "list mail"]);
    let out = session(&cmds);
    assert!(out.contains("4 discovered"), "{out}");
    assert!(out.contains("A receipt"), "{out}");
}

#[test]
fn only_the_inbox_is_watched_when_an_account_is_added() {
    // FR-43: a discovered folder is discovered, not adopted. The inbox is the exception,
    // because an account whose inbox is not watched shows nothing.
    let mut cmds = live();
    cmds.push("folders mail");
    let out = session(&cmds);
    let watched = out.lines().filter(|l| l.contains("watched")).count();
    assert_eq!(watched, 1, "{out}");
    assert!(
        out.lines()
            .any(|l| l.contains("INBOX") && l.contains("watched")),
        "{out}"
    );
}

#[test]
fn a_backfill_discovers_and_only_a_delta_delivers() {
    // FR-23 defines new mail as delivered-and-unread at that moment, and it cannot be
    // reconstructed afterwards without a resync NFR-18 forbids. A backfill that counted as
    // delivery would announce the user's whole mailbox the first time they added an account.
    let mut cmds = live();
    cmds.extend(["sync mail", "sync mail"]);
    let out = session(&cmds);
    let delivered: Vec<&str> = out
        .lines()
        .filter(|l| l.contains("delivered — FR-23"))
        .collect();
    assert_eq!(delivered.len(), 2, "{out}");
    assert!(
        delivered[0].starts_with("0 delivered"),
        "the backfill announced the whole mailbox: {}",
        delivered[0]
    );
    assert!(
        delivered[1].starts_with("1 delivered"),
        "the delta delivered nothing: {}",
        delivered[1]
    );
}

#[test]
fn a_delta_applies_an_arrival_a_flag_change_and_a_departure_in_one_turn() {
    let mut cmds = live();
    cmds.extend(["sync mail", "sync mail", "list mail"]);
    let out = session(&cmds);
    assert!(out.contains("1 inserted, 1 updated, 1 removed"), "{out}");
    let after = out.rsplit("delivered — FR-23").next().unwrap();
    assert!(
        after.contains("Something new"),
        "the arrival is not listed: {out}"
    );
    assert!(
        !after.contains("Re: A receipt"),
        "a message that left the folder is still listed: {out}"
    );
}

#[test]
fn a_second_sync_over_a_quiet_delta_changes_nothing() {
    // Reapplication is safe, which is the property D-82 leans on when it takes the cursor
    // before the walk — and records as the weak point of that choice.
    let mut cmds = live();
    cmds.extend(["sync mail", "sync mail", "sync mail", "list mail"]);
    let out = session(&cmds);
    let turns: Vec<&str> = out.lines().filter(|l| l.contains("inserted,")).collect();
    assert_eq!(turns.len(), 3, "{out}");
    assert!(
        turns[2].starts_with("0 inserted, 0 updated, 0 removed"),
        "the third sync was not quiet: {}",
        turns[2]
    );
}

#[test]
fn the_body_view_receives_no_external_scheme_in_any_fetching_position() {
    // I2, end to end. Every fetching position is rewritten inside stage 3 — including one a
    // filter rule would condemn, because the verdict is the broker's at request time and
    // FR-8's allowlist changes without the message changing.
    let mut cmds = live();
    cmds.extend(["sync mail", "body #1"]);
    let out = session(&cmds);
    let document = out.lines().last().unwrap_or_default();
    assert!(document.contains("sift-resource:/"), "{document}");
    assert!(
        !document.contains("src=\"https://"),
        "an external scheme survived a fetching position: {document}"
    );
}

#[test]
fn a_link_keeps_its_real_address_because_a_link_is_not_a_fetch() {
    // Navigation never happens in place, and it happens on an explicit confirmation. A link
    // rewritten to the internal scheme would be a link Sift could not honestly display.
    let mut cmds = live();
    cmds.extend(["sync mail", "body #1"]);
    let out = session(&cmds);
    let document = out.lines().last().unwrap_or_default();
    assert!(
        document.contains("href=\"https://example.test/read\""),
        "{document}"
    );
}

#[test]
fn with_no_filter_engine_loaded_every_remote_fetch_is_refused() {
    // D-10: an absent authority denies. It does not fall through to the backstop, and it
    // does not reload forty megabytes in response to a pressure signal.
    let mut cmds = live();
    cmds.extend(["sync mail", "body #1"]);
    let out = session(&cmds);
    assert!(out.contains("-> blocked"), "{out}");
    assert!(!out.contains("-> allowed"), "{out}");
}

#[test]
fn rendering_a_body_does_not_fetch_the_attachment() {
    // "Sift MUST NOT fetch whole messages" is a claim about requests. The fixture's message
    // carries a forty-megabyte attachment; the body view costs a few kilobytes.
    let mut cmds = live();
    cmds.extend(["sync mail", "body #1", "net mail"]);
    let out = session(&cmds);
    assert!(
        out.contains("stages: sanitize"),
        "the body did not render: {out}"
    );
    // The replay transport is not on anybody's data plan, so this asserts the shape of the
    // answer rather than a byte count — the count itself is asserted against the real
    // transport in `sift-http`.
    assert!(out.contains("sent 0  received 0"), "{out}");
}

#[test]
fn the_operation_this_provider_will_not_do_is_absent_rather_than_offered() {
    // The scope that permits an immediate permanent delete also authorizes sending, which
    // D-88 forbids requesting. docs/mail/mutations.md requires an unsupported operation be
    // *absent* rather than approximated, and the capability model is what makes that happen
    // without anybody writing a special case.
    let mut cmds = live();
    cmds.extend(["sync mail", "select #1", "actions"]);
    let out = session(&cmds);
    assert!(out.contains("message.archive"), "{out}");
    assert!(
        !out.contains("message.permanently-delete"),
        "an action the account cannot perform was offered: {out}"
    );
}

#[test]
fn a_mutation_goes_out_over_the_wire_and_settles() {
    let mut cmds = live();
    cmds.extend([
        "sync mail",
        "select #1",
        "do message.archive",
        "queue mail",
        "flush mail",
        "queue mail",
    ]);
    let out = session(&cmds);
    assert!(out.contains("archive  Pending"), "{out}");
    assert!(out.contains("1 issued: 1 applied"), "{out}");
    assert!(
        out.rsplit("1 issued")
            .next()
            .unwrap()
            .contains("queue empty"),
        "{out}"
    );
}

#[test]
fn a_mutation_is_durably_enqueued_before_it_is_applied_optimistically() {
    // The failure model states it from both sides: an intent MUST be durably enqueued before
    // it is applied optimistically to local state, not after. The reverse would leave local
    // state saying a mutation happened with the queue not holding it.
    let mut cmds = live();
    cmds.extend(["sync mail", "select #1", "do message.archive", "list mail"]);
    let out = session(&cmds);
    let enqueued = out.find("enqueued").expect("nothing was enqueued");
    let optimistic = out.find("optimistic: yes").expect("nothing was optimistic");
    assert!(enqueued < optimistic, "{out}");
}

#[test]
fn a_restart_moves_what_was_in_flight_to_reconciling() {
    // What a crash leaves. A request that went out and whose answer never came back is
    // exactly this, and inferring sent-versus-unsent at startup cannot be done correctly.
    let mut cmds = live();
    cmds.extend([
        "sync mail",
        "select #1",
        "do message.mark-read",
        "restart mail",
        "queue mail",
    ]);
    let out = session(&cmds);
    // Nothing had been issued, so nothing reconciles — which is the honest answer and the
    // one that proves the transition is driven by the durable marker rather than by age.
    assert!(out.contains("0 intent(s) moved to Reconciling"), "{out}");
}

#[test]
fn a_folder_can_be_taken_out_of_the_watched_set_without_losing_its_place() {
    // An unwatched folder is deliberately not a seventh folder state: it stops being
    // scheduled and its stored state is retained, so re-watching does not restart from
    // Unsynced.
    let mut cmds = live();
    cmds.extend([
        "sync mail",
        "watch mail INBOX off",
        "sync mail",
        "watch mail INBOX on",
        "list mail",
    ]);
    let out = session(&cmds);
    assert!(
        out.contains("its state is kept, so re-watching resumes"),
        "{out}"
    );
    assert!(
        out.contains("A receipt"),
        "unwatching lost the folder's mail: {out}"
    );
}

#[test]
fn the_subject_a_sender_wrote_is_normalized_before_anything_shows_it() {
    // NFR-54 and D-100: bidi controls are isolated rather than stripped, so a subject
    // containing an override cannot reorder the row around it.
    let mut cmds = live();
    cmds.extend(["sync mail", "list mail"]);
    let out = session(&cmds);
    let row = out
        .lines()
        .find(|l| l.contains("A receipt"))
        .expect("no row");
    assert!(
        row.contains('\u{2068}') && row.contains('\u{2069}'),
        "the subject crossed unisolated: {row:?}"
    );
}

#[test]
fn an_account_with_no_provider_behind_it_still_drives_the_queue() {
    // The capability-shape account. It is not a mock of an adapter — it is a real store and
    // a real queue with nothing behind them, which is what the planner tests need.
    let out = session(&[
        "account add shape rich",
        "ingest shape A message",
        "select #1",
        "do message.archive",
        "flush shape --leave-in-flight",
        "restart shape",
        "queue shape",
    ]);
    assert!(out.contains("1 intent(s) moved to Reconciling"), "{out}");
}
