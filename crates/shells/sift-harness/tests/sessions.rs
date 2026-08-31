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
