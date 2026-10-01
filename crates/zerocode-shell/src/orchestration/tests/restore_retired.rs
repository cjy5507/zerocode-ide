//! t-19779: a worker the ledger released stays retired.
//!
//! On 2026-10-01 five released workers' checkouts each gained an agent a few
//! minutes after the worker's last turn (09:22, 10:55, 17:36, 20:36): an empty
//! Claude, a child of the window, in the worktree — about 190 MB each, and the
//! checkout and its build output held past the release, because the reclaimer
//! waits for the last agent inside a directory to go. Nothing restored them.
//! A visit to the checkout did: the window asks the ledger who was last seated
//! there, a released row counted, and the answer it got was "claude" — which
//! it started, whatever the person had chosen to open in a new workspace.
//!
//! Driven through a private window's production doors, as `restore.rs` is: the
//! retire step (`worker-stop`, the same `WorkerTerminal` effect `worker-release`
//! walks), what the window asks the ledger on a visit, and then every road that
//! brings a conversation back — the next boot's reseat, the grace, and the door
//! a person reopens a conversation through. A CLI counted here is one a real
//! entry point cut or started.

use super::restore::{
    Restoring, a_run, a_worker, census_without_commands, deaths_of, row, say, the_window_goes,
};
use super::restore_door::{Did, Door, coordinator_back, the_next_boot};
use super::*;

/// The window's one question about a checkout it is opening, read as the
/// window reads it: the answer's wire form.
fn the_visit_answer(checkout: &str) -> serde_json::Value {
    serde_json::to_value(super::super::last_agent_in_checkout(checkout)).expect("the answer")
}

/// t-19779, red: a seat the ledger let go of is still answered, with the agent
/// that was there — so the window opens that agent again on every visit. The
/// answer is for a seat somebody is coming back to: the live pane while the
/// worker stands, nothing once the retire step has released it — and the same
/// checkout is the reclaimer's at once, the ledger listing it as settled (the
/// reading the beat's sweep acts on).
#[test]
fn a_released_workers_checkout_names_nobody_to_the_window_and_is_the_reclaimers() {
    const LEADER: u32 = 279_100;
    const WORKER: u32 = 279_101;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "retired");
    let (_task, worker, term) = a_worker(
        &host,
        &team,
        "--agent claude",
        Some("19779b0c-0000-4000-8000-000000000001"),
    );
    let at = checkout.path().to_string_lossy().into_owned();

    // Standing: its own live seat, and not the reclaimer's.
    let standing = the_visit_answer(&at);
    assert_eq!(standing["agent"], "claude", "{standing}");
    assert_eq!(standing["term"], term, "{standing}");
    assert!(!super::super::checkout_is_settled(&at));

    // The retire step.
    say(
        &host,
        &team,
        &format!("worker-stop --worker {worker} --reason retired"),
    );
    assert_eq!(row(&worker).state, WorkerState::Released);

    let retired = the_visit_answer(&at);
    assert!(
        retired.is_null(),
        "the window is still told an agent is seated in a checkout the ledger let go of: {retired}"
    );
    assert!(
        super::super::checkout_is_settled(&at),
        "the reclaimer is not offered the checkout a released worker left"
    );
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
}

/// t-19779: a live worker in a checkout is still named after a released one
/// that sat there before it — a person's visit finds the seat that is working
/// — and a sleeper stays reserved for the ledger whatever was released after
/// it (`tests.rs`, "the newer released row hid the interrupted worker").
#[test]
fn a_live_worker_in_the_checkout_is_still_named_after_a_released_one() {
    const LEADER: u32 = 279_105;
    const FIRST: u32 = 279_106;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(checkout.path(), FIRST, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "handed-on");
    let (_task, earlier, earlier_term) = a_worker(&host, &team, "--agent claude", None);
    say(
        &host,
        &team,
        &format!("worker-stop --worker {earlier} --reason handed-on"),
    );
    let (_task, later, later_term) = a_worker(&host, &team, "--agent codex", None);
    assert_ne!(earlier, later);
    let at = checkout.path().to_string_lossy().into_owned();

    let standing = the_visit_answer(&at);
    assert_eq!(standing["agent"], "codex", "{standing}");
    assert_eq!(standing["term"], later_term, "{standing}");
    assert!(!super::super::checkout_is_settled(&at));
    for term in [earlier_term, later_term, LEADER] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-19779: the lifecycle a worker's own briefing names — it reports done from
/// its own pane, then its coordinator releases it — leaves the same answer as
/// the stop above. After the report the pane is idle and reusable and its seat
/// is still the one the window names; after the release nobody is, and the
/// checkout is the reclaimer's.
#[test]
fn a_worker_that_reported_and_was_released_by_its_coordinator_leaves_no_seat_for_the_window() {
    const LEADER: u32 = 279_130;
    const WORKER: u32 = 279_131;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "reported");
    let (_task, worker, term) = a_worker(
        &host,
        &team,
        "--agent claude",
        Some("19779d0c-0000-4000-8000-000000000001"),
    );
    let at = checkout.path().to_string_lossy().into_owned();
    let seat = row(&worker);
    let held = crate::agent_teams::current_pane_capability(&seat.team, &seat.pane)
        .expect("the split minted the worker a capability");

    // It reports from its own pane, presenting its own capability.
    let done = run(
        &host,
        Vec::new(),
        &seat.team,
        &seat.pane,
        &held,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{worker}"
        )),
        clock(),
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    let reported = the_visit_answer(&at);
    assert_eq!(reported["term"], term, "{reported}");
    assert!(!super::super::checkout_is_settled(&at));

    // Its coordinator releases it.
    say(
        &host,
        &team,
        &format!("worker-release --worker {worker} --retry-request rel-{worker}"),
    );
    assert_eq!(row(&worker).state, WorkerState::Released);
    let released = the_visit_answer(&at);
    assert!(
        released.is_null(),
        "the window is still told an agent is seated in a checkout its coordinator released: {released}"
    );
    assert!(super::super::checkout_is_settled(&at));
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
}

/// t-19779: no road that brings a conversation back treats a released worker
/// as one to bring back. The retire step closes its pane and nothing opens
/// another: the restart census does not count it, the next boot's reseat seats
/// nobody, the grace ends nobody and tells nobody, and a person reopening its
/// conversation through the window's resume door gets that person's own tab —
/// the ledger's row stays released, no worker is seated in the new pane, and
/// the door's hand-off to the ledger (t-20088) finds nothing to take.
#[test]
fn a_released_worker_is_brought_back_by_no_restore_road() {
    const OLD_LEADER: u32 = 279_110;
    const WORKER: u32 = 279_111;
    const NEW_LEADER: u32 = 279_112;
    const DOOR: u32 = 279_118;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "retired-restore");
    let session = "19779c0c-0000-4000-8000-000000000001";
    let (_task, worker, term) = a_worker(&host, &team, "--agent claude", Some(session));
    assert!(
        super::super::seated_live_workers()
            .iter()
            .any(|seated| seated.worker == worker),
        "a standing worker is not in the restart census"
    );

    say(
        &host,
        &team,
        &format!("worker-stop --worker {worker} --reason retired"),
    );
    assert_eq!(row(&worker).state, WorkerState::Released);
    let cut_by_now = host.cuts();
    assert_eq!(cut_by_now.len(), 1, "{cut_by_now:?}");
    assert!(
        super::super::seated_live_workers()
            .iter()
            .all(|seated| seated.worker != worker),
        "the restart census would cut a pane the ledger already retired"
    );

    // A restart: the goodbye, the next boot's sweep, the coordinator back
    // in a pane of its own and asking its run's sleepers to be seated.
    the_window_goes(&census_without_commands, &[term]);
    the_next_boot(OLD_LEADER);
    let seated = coordinator_back(&host, "retired-restore", OLD_LEADER, NEW_LEADER);
    assert_eq!(seated, 0, "the reseat brought back a released worker");
    assert_eq!(host.cuts(), cut_by_now, "the reseat cut a pane");

    // The grace: past it, a sleeper nothing seated is ended and told. A
    // released worker is neither.
    let booted = clock();
    let _boot = BootedHere::at(booted);
    let grace = zerocode_core::orchestration::RESEAT_GRACE_MS;
    super::super::tick(&host, &[], booted + grace + 1);
    assert_eq!(row(&worker).state, WorkerState::Released);
    assert!(
        deaths_of(&worker).is_empty(),
        "the grace told a run its released worker died: {:?}",
        deaths_of(&worker)
    );
    assert_eq!(host.cuts(), cut_by_now, "the beat cut a pane");

    // A person reopens its conversation: the window's resume door starts the
    // person's pane, and what the door then asks of the ledger (the pass a
    // returned conversation is handed to) seats nothing.
    let door = Door::new(checkout.path());
    door.wake(DOOR, "claude", session)
        .expect("the person's wake");
    assert_eq!(door.started().len(), 1, "{:?}", door.started());
    assert_eq!(
        door.seated_at_start(DOOR),
        None,
        "the reopened conversation was seated as the released worker"
    );
    assert!(door.did.lock().unwrap().contains(&Did::Rebound(DOOR)));
    let taken = super::super::reseat_sleeping(&host, Vec::new(), DOOR, Some(&test_actor(DOOR)));
    assert_eq!(taken, 0, "the hand-off took a released worker back");
    assert_eq!(row(&worker).state, WorkerState::Released);
    assert_eq!(host.cuts(), cut_by_now, "the hand-off cut a pane");
    for pane in [DOOR, NEW_LEADER, WORKER, OLD_LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}
