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
        "account writes work on",
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
    // Every position, and every one of them refused. The pair is the assertion: a document
    // reporting "0 fetching positions, 0 withheld" would pass a check for the absence of
    // "allowed" while proving nothing at all.
    let line = out
        .lines()
        .find(|l| l.contains("fetching position(s)"))
        .unwrap_or_else(|| panic!("{out}"));
    let counts: Vec<u32> = line
        .split_whitespace()
        .filter_map(|w| w.parse().ok())
        .collect();
    assert!(counts[0] > 0, "there is something to refuse: {line}");
    assert_eq!(counts[0], counts[1], "all of them were refused: {line}");
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
        // Writes are authorized explicitly, because a new account is watched-only. Every
        // test that reaches the wire says so out loud, which is the point of the default.
        "account writes mail on",
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
        "account writes shape on",
        "ingest shape A message",
        "select #1",
        "do message.archive",
        "flush shape --leave-in-flight",
        "restart shape",
        "queue shape",
    ]);
    assert!(out.contains("1 intent(s) moved to Reconciling"), "{out}");
}

// ---------------------------------------------------------------------------
// FR-6's row, as a shell receives it. `list` prints a sentence a person
// recognises; these assert the fields the sentence was formatted from, because
// a shell binds to the fields.
// ---------------------------------------------------------------------------

#[test]
fn the_list_is_ordered_by_the_time_the_server_assigned_and_not_the_one_the_sender_claimed() {
    // D-55. The `Date` header is the sender's claim and is trivially forged; ordering on it
    // would let anyone put their mail at the top of the list.
    let mut cmds = live();
    cmds.extend(["folders mail", "sync mail", "row mail"]);
    let out = session(&cmds);

    let received: Vec<u64> = out
        .lines()
        .filter_map(|l| l.split("received=").nth(1))
        .filter_map(|r| r.split_whitespace().next())
        .filter_map(|r| r.parse().ok())
        .collect();
    assert_eq!(received.len(), 3, "{out}");
    assert!(
        received.windows(2).all(|w| w[0] >= w[1]),
        "newest first: {received:?}\n{out}"
    );

    // And the sender's own claim is carried beside it rather than being what was sorted on.
    assert!(out.contains("origination="), "{out}");
}

#[test]
fn a_thread_reports_how_many_messages_it_holds() {
    // FR-6 lists a thread count among the row's fields, and D-54's reader is native rows
    // over one body view — so the count is what tells a shell there are rows to draw.
    let mut cmds = live();
    cmds.extend(["folders mail", "sync mail", "row mail"]);
    let out = session(&cmds);
    assert!(out.contains("thread=2"), "the threaded pair: {out}");
    assert!(
        out.contains("thread=1"),
        "the message that is its own thread: {out}"
    );
}

#[test]
fn read_state_comes_from_the_provider_before_anyone_has_touched_it() {
    let mut cmds = live();
    cmds.extend(["folders mail", "sync mail", "row mail"]);
    let out = session(&cmds);
    assert!(out.contains("unread=true"), "{out}");
    assert!(out.contains("unread=false"), "{out}");
}

#[test]
fn the_overlay_answers_for_read_state_before_the_server_has_been_told() {
    // D-51 and NFR-7 together, on a field rather than on the row's presence. Marking a
    // message read must show as read immediately — and it must do so without the base row
    // in the store having changed, which is what makes it an overlay rather than a write.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "select #3",
        "do message.mark-read",
        "row mail",
    ]);
    let out = session(&cmds);
    let marked = out
        .lines()
        .find(|l| l.contains("A receipt") && !l.contains("Re:"))
        .unwrap_or_else(|| panic!("the row is missing:\n{out}"));
    assert!(
        marked.contains("unread=false"),
        "the gesture had not reached the row:\n{marked}"
    );
}

#[test]
fn every_field_a_shell_draws_is_normalized_before_it_arrives() {
    // NFR-54. Sender, subject and snippet are all attacker-controlled, and the threat model
    // notes this path reaches native chrome where no sanitizer invariant sees it. The
    // normalizer's isolate marks are the evidence it ran.
    let mut cmds = live();
    cmds.extend(["folders mail", "sync mail", "row mail"]);
    let out = session(&cmds);
    let row = out
        .lines()
        .find(|l| l.contains("subject="))
        .unwrap_or_else(|| panic!("no row:\n{out}"));
    assert!(
        row.contains('\u{2068}') && row.contains('\u{2069}'),
        "attacker-controlled text reached the row unisolated:\n{row}"
    );
}

// ---------------------------------------------------------------------------
// D-18, driven the way a shell drives it: register an observation, and be told
// what changed rather than being handed the set again.
// ---------------------------------------------------------------------------

#[test]
fn an_observation_is_told_what_changed_and_not_what_the_world_holds() {
    let mut cmds = live();
    cmds.extend([
        "observe mail",
        "poll",
        "folders mail",
        "sync mail",
        "poll",
        "poll",
    ]);
    let out = session(&cmds);

    // Nothing had arrived yet, so there was nothing to say. A registry that answered with
    // an empty window here would have a shell repainting a list on every unrelated signal.
    assert!(out.contains("-- 0 delivery(ies)"), "{out}");
    // Then three rows arrived, as inserts at their post-batch indices.
    assert!(out.contains("Insert { to: 0 }"), "{out}");
    assert!(out.contains("Insert { to: 2 }"), "{out}");
    // And the poll after that is quiet, because nothing changed between them.
    assert!(
        out.trim_end().ends_with("-- 0 delivery(ies)"),
        "a quiet poll produced a delivery:\n{out}"
    );
}

#[test]
fn a_triage_gesture_reaches_the_observation_as_a_delete_before_any_round_trip() {
    // NFR-7 and D-51 arriving at a shell through D-18 rather than through a re-read. The
    // row leaves the window because the overlay removes it, and it leaves as a *delete* at
    // a stated index, which is what a table view needs to animate one row going.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "observe mail",
        "poll",
        "select #1",
        "do message.archive",
        "poll",
    ]);
    let out = session(&cmds);
    assert!(out.contains("Delete { from: 0 }"), "{out}");
}

#[test]
fn a_change_that_keeps_a_rows_identity_is_an_update_and_not_a_delete_and_an_insert() {
    // The distinction D-18 exists to preserve. Marking read changes what the row draws and
    // not which row it is — so the cell is updated in place, selection survives, and the
    // list does not animate a row leaving and another arriving.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "observe mail",
        "poll",
        "select #2",
        "do message.mark-read",
        "poll",
    ]);
    let out = session(&cmds);
    let after = out.rsplit("mark-read enqueued").next().unwrap_or("");
    assert!(after.contains("Update { at: 1 }"), "{out}");
    assert!(
        !after.contains("Delete") && !after.contains("Insert"),
        "an in-place change was expressed as a departure and an arrival:\n{out}"
    );
}

#[test]
fn cancelling_one_observation_leaves_the_other_delivering() {
    // The defect the ABI carried while an observation had no identity of its own: cancelling
    // by generation would have taken every observation or none of them.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "observe mail",
        "observe *",
        "poll",
    ]);
    let out = session(&cmds);
    assert!(out.contains("observing mail as #1"), "{out}");
    assert!(out.contains("observing * as #2"), "{out}");
    assert!(out.contains("-- 2 delivery(ies)"), "both were told: {out}");
}

// ---------------------------------------------------------------------------
// N-1: the body view's only channel out, and what it answers.
// ---------------------------------------------------------------------------

#[test]
fn a_remote_resource_is_refused_rather_than_fetched() {
    // FR-8, at the one place it can actually be enforced. The document was rewritten to
    // address the pixel through the internal scheme, and when the body view asks for it the
    // answer is a refusal — not a fetch that fails, and not a fetch at all.
    let mut cmds = live();
    cmds.extend(["folders mail", "sync mail", "body #1", "resource #0"]);
    let out = session(&cmds);
    assert!(out.contains("blocked:"), "{out}");
    assert!(
        !out.contains("bytes:"),
        "a remote resource was fetched:\n{out}"
    );
}

#[test]
fn a_fabricated_address_resolves_to_nothing() {
    // D-28's whole point: the internal scheme's addressing is a security boundary rather
    // than a naming convenience. A guessed token is not a valid one, and the answer is
    // *unavailable* rather than blocked — because a defect being caught and a resource being
    // refused are different facts, and conflating them would hide the first.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "body #1",
        "resource sift-resource://ffffffffffffffffffffffffffffffff/0",
    ]);
    let out = session(&cmds);
    assert!(out.contains("unavailable:"), "{out}");
}

#[test]
fn revoking_a_document_kills_its_address_space() {
    // D-90: revocation happens at navigation, which is earlier and more often than teardown.
    // Message A's addresses must be dead before message B's document exists, whether or not
    // the view survives — that is what keeps "two messages share no address space" true
    // across a reused body view.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "body #1",
        "resource #0",
        "close #",
        "resource sift-resource://00000000000000000000000000000000/0",
    ]);
    let out = session(&cmds);
    assert!(out.contains("revoked "), "{out}");
    let after = out.rsplit("revoked ").next().unwrap_or("");
    assert!(
        after.contains("unavailable:"),
        "an address survived its document:\n{out}"
    );
}

#[test]
fn a_link_is_not_a_fetching_position_and_keeps_its_real_address() {
    // FR-30 and the link-confirmation sheet together. A link is followed on an explicit
    // confirmation and never in place, so it is not rewritten — the destination shown to the
    // user has to be the real one, and Sift must never resolve a wrapper by fetching it,
    // because following the redirect *is* the tracking event.
    let mut cmds = live();
    cmds.extend(["folders mail", "sync mail", "body #1"]);
    let out = session(&cmds);
    assert!(out.contains("href=\"https://example.test/read\""), "{out}");
    assert!(
        !out.contains("src=\"https://tracker.test"),
        "a fetching position kept an external scheme:\n{out}"
    );
}

// ---------------------------------------------------------------------------
// The read-only posture: a mailbox can be connected, synced, read and triaged
// before anything is authorized to change it.
// ---------------------------------------------------------------------------

#[test]
fn a_new_account_is_watched_and_not_written_to() {
    // The default, and the reason for it: connecting a real mailbox should not, by itself,
    // authorize anything to alter it. Everything a person can see still works — this is not
    // a read-only *mode* that disables triage, it is a queue that is not yet allowed to
    // drain.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "select #1",
        "do message.archive",
        "flush mail",
    ]);
    let out = session(&cmds);
    assert!(
        out.contains("optimistic: yes"),
        "the gesture still applied: {out}"
    );
    assert!(out.contains("0 issued"), "{out}");
    assert!(out.contains("held"), "{out}");
    assert!(
        !out.contains("1 applied"),
        "a write reached the provider without authorization:\n{out}"
    );
}

#[test]
fn a_held_intent_is_kept_rather_than_dropped() {
    // The queue is the thing that must not lose a gesture. Holding is not discarding, and a
    // held intent stays visible and stays in the order it was made.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "select #1",
        "do message.archive",
        "flush mail",
        "queue mail",
    ]);
    let out = session(&cmds);
    assert!(out.contains("archive  Pending"), "{out}");
}

#[test]
fn authorizing_writes_lets_the_held_queue_drain() {
    // And the other half: the authorization is what releases it, and what was held goes out
    // in the order it was made rather than being re-derived from current state.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "select #1",
        "do message.archive",
        "flush mail",
        "account writes mail on",
        "flush mail",
    ]);
    let out = session(&cmds);
    assert!(out.contains("0 issued"), "{out}");
    let after = out.rsplit("may now be changed").next().unwrap_or("");
    assert!(after.contains("1 issued: 1 applied"), "{out}");
}

#[test]
fn withdrawing_authorization_stops_the_next_flush() {
    // A person who turns it back off must see it take effect on anything not yet issued.
    let mut cmds = live();
    cmds.extend([
        "folders mail",
        "sync mail",
        "account writes mail on",
        "account writes mail off",
        "select #1",
        "do message.archive",
        "flush mail",
    ]);
    let out = session(&cmds);
    assert!(out.contains("0 issued"), "{out}");
    assert!(out.contains("watched only"), "{out}");
}

#[test]
fn what_authorization_grants_is_stated_in_the_terms_it_grants_it_in() {
    // FR-4 requires removal to say what is lost "in those terms"; the same standard applies
    // to the gesture that authorizes writing to somebody's mail. Naming the operations is
    // what makes the consent informed, and permanent deletion is not among them.
    let out = session(&["account add-replayed mail", "account writes mail on"]);
    for granted in [
        "archive",
        "move",
        "flag",
        "label",
        "mark read",
        "report junk",
        "Trash",
    ] {
        assert!(out.contains(granted), "`{granted}` was not stated:\n{out}");
    }
    assert!(
        !out.to_lowercase().contains("permanently delete"),
        "authorization claimed a power the provider does not give it:\n{out}"
    );
}

// ---------------------------------------------------------------------------------------
// The reader's chrome: what was withheld, where a link goes, and what is attached.
//
// Every one of these drives the hostile fixture — a click wrapper, a homograph host, a
// `mailto:` unsubscribe and an attachment named with a right-to-left override, in one
// message. A corpus of only well-behaved mail tests the happy path of a product whose whole
// reason for existing is the other one.
// ---------------------------------------------------------------------------------------

/// The receipt is the hostile one, and it sorts last under D-55 — newest first, and it is the
/// oldest of the three.
fn hostile() -> Vec<&'static str> {
    vec!["account add-replayed mail", "sync mail"]
}

#[test]
fn a_withheld_resource_is_reported_with_the_reason_that_is_actually_true() {
    let mut cmds = hostile();
    cmds.push("blocked #3");
    let out = session(&cmds);

    assert!(out.contains("1 remote resource not loaded"), "{out}");
    assert!(out.contains("beacon.tracker.test"), "{out}");
    // D-10 makes an absent authority **deny** rather than fall through, and the chrome says
    // *that* rather than claiming a rule matched. A user who believes a filter list caught
    // something believes Sift is protecting them in a way it currently is not.
    assert!(
        out.contains("no filter list is loaded"),
        "the reason names the shed rather than inventing a rule: {out}"
    );
}

/// The control is **absent** rather than present-and-ineffective. An allowance keyed on
/// nothing would apply to everyone, which is the opposite of what the button says.
#[test]
fn always_load_from_this_sender_is_absent_when_there_is_no_sender_to_key_it_on() {
    let mut cmds = hostile();
    cmds.push("blocked #3");
    let out = session(&cmds);
    assert!(
        out.contains("`always load from this sender` is absent"),
        "{out}"
    );
    assert!(
        out.contains("no origin to key a durable allowance on"),
        "{out}"
    );
}

/// FR-30. The destination is recovered from the wrapper's own text, and the wrapper stays
/// available — a user who cannot see that a link was wrapped cannot judge who wrapped it.
#[test]
fn a_click_wrapper_is_unwrapped_locally_and_the_wrapper_is_still_shown() {
    let mut cmds = hostile();
    cmds.push("links #3");
    let out = session(&cmds);

    assert!(out.contains("link https://example.test/offer"), "{out}");
    assert!(
        out.contains("wrapped by https://click.tracker.test"),
        "{out}"
    );
}

/// The falsifier for the test above. This wrapper carries nothing recoverable from its own
/// text, and a Sift that resolved wrappers by *fetching* them would resolve it anyway — so the
/// assertion is that it stays unresolved. A byte counter cannot make this claim against a
/// replayed transport, and an intention is not evidence; an unrecoverable wrapper is.
#[test]
fn a_wrapper_with_nothing_recoverable_in_it_is_not_followed_to_find_out() {
    let mut cmds = hostile();
    cmds.push("links #3");
    let out = session(&cmds);

    let line = out
        .lines()
        .find(|l| l.contains("click.tracker.test/x/9f2c41"))
        .unwrap_or_else(|| panic!("the opaque wrapper is shown: {out}"));
    assert!(
        !line.contains("->"),
        "it resolved to something, which it can only have done by asking: {line}"
    );
    assert!(
        !line.contains("wrapped by"),
        "nothing was unwrapped, so nothing claims to have been: {line}"
    );
}

/// A punycode label renders as one script and resolves as another. The display form marks it
/// rather than rendering it, which is the whole of the defence: a user cannot compare two
/// strings they are only shown one of.
#[test]
fn a_homograph_host_is_not_displayed_as_the_script_it_imitates() {
    let mut cmds = hostile();
    cmds.push("links #3");
    let out = session(&cmds);

    let line = out
        .lines()
        .find(|l| l.contains("xn--80ak6aa92e"))
        .unwrap_or_else(|| panic!("the real host is shown: {out}"));
    assert!(
        line.contains("[80ak6aa92e]"),
        "the displayed form marks the label rather than rendering it: {line}"
    );
}

/// FR-42. Shown, reported as needing a mail handler, and never sent — the historical form of
/// unsubscribing is a message, which the no-send constraint forbids outright.
#[test]
fn a_mailto_unsubscribe_is_shown_and_reported_rather_than_omitted() {
    let mut cmds = hostile();
    cmds.push("links #3");
    let out = session(&cmds);

    assert!(
        out.contains("unsubscribe: mailto:unsubscribe@list.test"),
        "{out}"
    );
    assert!(out.contains("requires a mail handler"), "{out}");
    assert!(out.contains("Sift will not send it"), "{out}");
}

/// FR-10's list is built from the structure, and the structure is a few kilobytes. The
/// forty-megabyte part beside it costs nothing until somebody asks — which is a claim about
/// requests rather than about intentions, so it is asserted against the byte counter.
#[test]
fn listing_attachments_does_not_download_them() {
    let mut cmds = hostile();
    cmds.extend(["attachments #3", "net mail"]);
    let out = session(&cmds);

    assert!(out.contains("none fetched"), "{out}");
    let received: u64 = out
        .lines()
        .filter_map(|l| l.split_once("received "))
        .filter_map(|(_, r)| r.split_whitespace().next())
        .filter_map(|n| n.parse().ok())
        .max()
        .unwrap_or_default();
    // The whole corpus is a few kilobytes. An attachment fetch would be 8 bytes here, but the
    // declared size is 8 and the structure says so — what matters is that the *request* is
    // absent, and a bound well under the declared corpus proves no extra one was made.
    assert!(received < 64 * 1024, "{received} bytes on the wire: {out}");
}

/// The case FR-10 wrote its three-source rule for. The declared type is `application/pdf`,
/// the name renders as `invoice.pdf`, and the extension the platform acts on is `.exe`. A
/// type check sees a document; the user sees a document; the machine runs a program.
#[test]
fn an_executable_disguised_as_a_document_is_caught_by_the_source_a_type_check_never_sees() {
    let mut cmds = hostile();
    cmds.push("attachments #3");
    let out = session(&cmds);

    assert!(
        out.contains("application/pdf"),
        "the declared type is a document: {out}"
    );
    assert!(out.contains("warn before opening"), "{out}");
    assert!(
        out.contains("the name ends in an executable extension"),
        "the extension is the source that decides what the platform does: {out}"
    );
    assert!(
        out.contains("the declared type and the name disagree"),
        "a sender who labels an executable as a document has told Sift something: {out}"
    );
}

/// NFR-53, in the words of the requirement: a sender-supplied filename never becomes a path.
/// The override that made it render as a document is gone from the name that is written.
#[test]
fn a_sender_supplied_name_never_becomes_the_path_it_is_written_under() {
    let mut cmds = hostile();
    cmds.push("attachments #3");
    let out = session(&cmds);

    let line = out
        .lines()
        .find(|l| l.contains("as `"))
        .unwrap_or_else(|| panic!("a derived name is shown: {out}"));
    assert!(
        !line.contains('\u{202E}'),
        "the override is gone from the derived name: {line:?}"
    );
    assert!(line.contains("invoicefdp.exe"), "{line}");
}

/// The requirement is that the exact final path be shown **before** the write. A single call
/// that saved and then reported would satisfy every test and none of the requirement, so the
/// planning verb is asserted to write nothing.
#[test]
fn the_final_path_is_shown_before_anything_is_written() {
    let directory = std::env::temp_dir().join(format!(
        "sift-save-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&directory).expect("a directory to save into");
    let plan = format!("save #3 2 {}", directory.display());

    let mut cmds = hostile();
    cmds.push(&plan);
    let out = session(&cmds);

    assert!(out.contains("would write:"), "{out}");
    assert!(out.contains("nothing written"), "{out}");
    assert_eq!(
        std::fs::read_dir(&directory).expect("readable").count(),
        0,
        "planning wrote a file: {out}"
    );
    std::fs::remove_dir_all(&directory).ok();
}

/// "An existing file MUST NOT be overwritten." Held by `create_new` rather than by a check,
/// because a check before a write is a race and the file that appears between the two is the
/// one somebody cared about.
#[test]
fn saving_twice_writes_twice_and_overwrites_nothing() {
    let directory = std::env::temp_dir().join(format!(
        "sift-save-twice-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&directory).expect("a directory to save into");
    let write = format!("save #3 2 {} write", directory.display());

    let mut cmds = hostile();
    cmds.extend([write.as_str(), write.as_str()]);
    let out = session(&cmds);

    let written: Vec<String> = std::fs::read_dir(&directory)
        .expect("readable")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(written.len(), 2, "{written:?} — {out}");
    assert!(written.iter().any(|n| n == "invoicefdp.exe"), "{written:?}");
    assert!(
        written.iter().any(|n| n == "invoicefdp (2).exe"),
        "the suffix goes before the extension, or the file opens with the wrong application: {written:?}"
    );
    std::fs::remove_dir_all(&directory).ok();
}

/// The bytes are a PE header under a `.pdf` type. All three of FR-10's sources now disagree,
/// and the fourth — the content — is the one that settles it.
#[test]
fn the_content_is_the_last_source_and_it_only_exists_once_the_bytes_are_here() {
    let directory = std::env::temp_dir().join(format!(
        "sift-save-sniff-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&directory).expect("a directory to save into");
    let write = format!("save #3 2 {} write", directory.display());

    let mut cmds = hostile();
    cmds.push(&write);
    let out = session(&cmds);

    assert!(out.contains("needs a warning first"), "{out}");
    let path = directory.join("invoicefdp.exe");
    assert_eq!(
        std::fs::read(&path).expect("written").get(..2),
        Some(b"MZ".as_slice()),
        "the bytes are what the sniff saw"
    );
    std::fs::remove_dir_all(&directory).ok();
}
