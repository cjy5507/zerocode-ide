//! t-11548 and t-11537: a window restart that nobody talks to afterwards.
//!
//! 2026-09-28 03:32 the window restarted with a run at work. A Codex worker
//! at rest (`exit: worker w-11242 on terminal 26 (codex) · turn rest · 0
//! command(s)`) came back into terminal 2, and the status its coordinator
//! sent it five minutes later was never pointed at: "could not be pointed at
//! terminal 2 — the window has heard nothing from this pane". The
//! coordinator's own conversation came back too, and sat: no word reached it
//! and the timers it had set died with its process, so the run waited on the
//! person to say something.
//!
//! Driven through a private window's production doors, as `restore.rs` is:
//! the goodbye with its census, the ledger closed and opened again from its
//! SQLite store, the next boot's sweep, a door's wake through the fake
//! launcher, and the beat's own pointer pass. A line counted here is one the
//! pointer or the wake actually typed.

use super::restore::{
    Restoring, a_run, a_worker, census_without_commands, row, say, the_window_goes,
};
use super::restore_door::{Door, coordinator_back, the_next_boot};
use super::*;
use std::collections::{HashMap, VecDeque};
use zerocode_pty::DeliveryOutcome;

/// The window's ledger closes and opens again from its SQLite store: the
/// actor ended and a new one started on the same file, the way a relaunched
/// window's boot starts one. A ledger kept in memory proves nothing about
/// what the disk kept (traps 52 and 54).
fn the_ledger_reopens(store: &zerocode_orchestrator::workflow_store::WorkflowStore) {
    let seat = super::super::runtime_cell()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .take()
        .expect("the private runtime");
    assert_eq!(
        std::sync::Arc::strong_count(&seat),
        1,
        "something still holds the old actor, which would write beside the new one"
    );
    let overrides = std::sync::Arc::clone(&seat.overrides);
    let usage = seat.usage.clone();
    // The last holder: the actor's thread is joined as it drops.
    drop(seat);
    let actor = super::super::RuntimeActor::start(
        store,
        "main-ledger",
        super::super::RuntimeBoot::Cutover {
            legacy: None,
            now_ms: clock(),
        },
        super::super::MAX_RUNTIME_MAILBOX,
        Box::new(super::super::ShellPaneTable),
        Box::new(super::super::LiveCatalog {
            overrides: std::sync::Arc::clone(&overrides),
            ledger_dir: std::env::temp_dir(),
            headroom: super::super::HeadroomSource::Fixed(TEST_HEADROOM_BYTES),
            usage: usage.clone(),
        }),
    )
    .expect("the ledger opens again from its store");
    super::super::install_runtime(actor, overrides, usage);
}

/// A window whose pointer has pointed at nothing yet. A private window
/// mints its run and message ids from one again, so a watermark another
/// scenario left in the shared data root could name this one's mail.
fn a_pointer_that_has_said_nothing() {
    if let Some(path) = super::super::delivered_watermarks_path() {
        let _ = std::fs::remove_file(path);
    }
    super::super::restart_pointer_memory_for_tests();
}

/// The window's log lines that contain `needle`.
fn logged(needle: &str) -> usize {
    let log = super::super::BLACKBOX
        .get()
        .expect("the bench window's black box")
        .join("window-errors.log");
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(needle))
        .count()
}

/// The restore host as the beat's pointer meets it: every road the restore
/// took goes through `inner`, and each advice line the pointer hands the pump
/// is answered from a script — the pump's own answers, one per delivery,
/// `Delivered` once the script for that pane runs out — and written down.
struct Beat<'a> {
    inner: &'a Restoring,
    /// The door whose wakes' receipts this window is waiting on, if any.
    waking: Option<&'a Door>,
    script: Mutex<HashMap<u32, VecDeque<DeliveryOutcome>>>,
    pointed: Mutex<Vec<(u32, String, DeliveryOutcome)>>,
}

impl<'a> Beat<'a> {
    fn new(inner: &'a Restoring) -> Self {
        Self {
            inner,
            waking: None,
            script: Mutex::new(HashMap::new()),
            pointed: Mutex::new(Vec::new()),
        }
    }

    /// The same window, whose wakes through `door` wait on their receipts.
    fn waking(inner: &'a Restoring, door: &'a Door) -> Self {
        Self {
            waking: Some(door),
            ..Self::new(inner)
        }
    }

    /// What the pump answers the next deliveries at `term`, in order.
    fn answering(&self, term: u32, said: &[DeliveryOutcome]) {
        self.script
            .lock()
            .unwrap()
            .insert(term, said.iter().copied().collect());
    }

    /// Advice lines handed to the pump for `term`, and how many landed.
    fn pointed_at(&self, term: u32) -> (usize, usize) {
        let pointed = self.pointed.lock().unwrap();
        let asked: Vec<_> = pointed.iter().filter(|(at, ..)| *at == term).collect();
        for (_, line, _) in &asked {
            assert!(line.contains("orchestration message"), "{line}");
        }
        let landed = asked
            .iter()
            .filter(|(.., outcome)| *outcome == DeliveryOutcome::Delivered)
            .count();
        (asked.len(), landed)
    }

    /// Beats of the window, one after another.
    fn beats(&self, count: usize) {
        for _ in 0..count {
            super::super::tick(self, &[], clock());
        }
    }
}

impl Host for Beat<'_> {
    fn split(
        &self,
        team: &str,
        leader_term: u32,
        from_term: u32,
        pane: &str,
        direction: zerocode_core::agent_teams::Direction,
        command: &str,
        token: &str,
    ) -> Option<u32> {
        self.inner.split(
            team,
            leader_term,
            from_term,
            pane,
            direction,
            command,
            token,
        )
    }
    fn send(&self, term: u32, text: &str) -> bool {
        self.inner.send(term, text)
    }
    fn point(
        &self,
        term: u32,
        line: &str,
        _submit: bool,
    ) -> Option<std::sync::mpsc::Receiver<DeliveryOutcome>> {
        let outcome = self
            .script
            .lock()
            .unwrap()
            .get_mut(&term)
            .and_then(VecDeque::pop_front)
            .unwrap_or(DeliveryOutcome::Delivered);
        self.pointed
            .lock()
            .unwrap()
            .push((term, line.to_string(), outcome));
        let (said, heard) = std::sync::mpsc::sync_channel(1);
        said.send(outcome).expect("the pump's answer");
        Some(heard)
    }
    fn capture(&self, term: u32) -> Option<String> {
        self.inner.capture(term)
    }
    fn focus(&self, term: u32) -> bool {
        self.inner.focus(term)
    }
    fn close(&self, term: u32) {
        self.inner.close(term);
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        self.inner.actor_for(term)
    }
    fn wake_words_pending(&self, term: u32) -> bool {
        self.waking
            .is_some_and(|door| crate::restart_nudge_runtime::WakeReceipts::rows(door).holds(term))
    }
}

/// The coordinator's leader team once it is back, as `coordinator_back`
/// seats it.
fn back_team(name: &str, new_leader: u32) -> String {
    format!("team-t7812-{name}-back-{new_leader}")
}

/// A status from the coordinator's seat to `worker`, as the coordinator sent
/// m-11539 at 03:37.
fn a_status_to(host: &dyn Host, team: &str, worker: &str) {
    say(
        host,
        team,
        &format!("send --to worker:{worker} --type status --body carry-on"),
    );
}

/// t-11548, the morning's shape: Codex workers whose turns had ended when
/// the window went — two on their own, one under a person's hand — come back
/// into panes this window has heard nothing from, over a ledger that closed
/// and opened again from its store: two through a door, one through the
/// ledger's own reseat when the coordinator returns. A status for each one
/// at rest is pointed at on the next beat, once; the one a person
/// interrupted stays the person's, and the log says so once, in those words.
///
/// Before, both panes were never-heard: the status waited under "the window
/// has heard nothing from this pane" until the coordinator replaced the
/// worker, five minutes later.
#[test]
fn a_worker_at_rest_when_the_window_went_is_pointed_at_its_mail_once_it_is_back() {
    const OLD_LEADER: u32 = 1_154_800;
    const WORKER: u32 = 1_154_801;
    const RESTED: u32 = 1_154_806;
    const HELD: u32 = 1_154_807;
    const NEW_LEADER: u32 = 1_154_810;
    let (_window, store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    a_pointer_that_has_said_nothing();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "at-rest");
    let rested_session = "session-t11548-rested";
    let held_session = "session-t11548-held";
    let (_, rested, rested_term) = a_worker(&host, &team, "--agent codex", Some(rested_session));
    let (_, held, held_term) = a_worker(&host, &team, "--agent codex", Some(held_session));
    let (_, reseated, reseated_term) = a_worker(
        &host,
        &team,
        "--agent codex",
        Some("session-t11548-reseated"),
    );
    for (term, interrupted) in [
        (rested_term, false),
        (held_term, true),
        (reseated_term, false),
    ] {
        super::super::pane_turn_began(term, clock());
        super::super::pane_turn_ended(term, clock(), interrupted, clock());
    }
    the_window_goes(
        &census_without_commands,
        &[rested_term, held_term, reseated_term],
    );
    the_ledger_reopens(&store);
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();

    let door = Door::new(checkout.path());
    door.wake(RESTED, "codex", rested_session)
        .expect("the rested worker's wake");
    door.wake(HELD, "codex", held_session)
        .expect("the held worker's wake");
    door.settle();
    for (worker, term) in [(&rested, RESTED), (&held, HELD)] {
        assert_eq!(row(worker).state, WorkerState::Active);
        assert_eq!(door.typed(term).0, 0, "{worker}, idle, was told to go on");
    }
    assert_eq!(
        coordinator_back(&host, "at-rest", OLD_LEADER, NEW_LEADER),
        1
    );
    let cut = host.cuts();
    assert_eq!(cut.len(), 1, "{cut:?}");
    let reseated_at = cut[0].0;
    assert!(
        host.typed_at(reseated_at).is_empty(),
        "an idle worker was told to go on"
    );
    let back = back_team("at-rest", NEW_LEADER);
    for worker in [&rested, &held, &reseated] {
        a_status_to(&host, &back, worker);
    }

    let beat = Beat::new(&host);
    beat.beats(3);
    assert_eq!(
        beat.pointed_at(RESTED),
        (1, 1),
        "the status for a worker at rest was not pointed at exactly once"
    );
    assert_eq!(
        beat.pointed_at(reseated_at),
        (1, 1),
        "the status for a worker the ledger reseated at rest was not pointed at exactly once"
    );
    assert_eq!(
        beat.pointed_at(HELD),
        (0, 0),
        "a pane a person's hand had stopped was typed at"
    );
    assert_eq!(
        logged(&format!(
            "could not be pointed at terminal {HELD} {}",
            super::super::THE_PERSON_HOLDS_IT
        )),
        1,
        "the held pane's silence was not said once, as the person's"
    );
    assert_eq!(
        logged(&format!(
            "terminal {RESTED} {}",
            super::super::NOTHING_WAS_EVER_HEARD
        )),
        0,
        "the pane at rest was still read as never heard"
    );
    for term in [RESTED, HELD, reseated_at, NEW_LEADER] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-11548 (c) and (d): the pane is back but its composer is not up yet, and
/// a person is writing in another restored pane.
///
/// The pointer does not decide readiness; the pump's guard does, at the
/// write, and says so in its answer. A pump that answers "never became
/// ready" is asked again on a later beat, and the line lands once when the
/// composer stands. A pump that answers "a person's draft is on the line"
/// types nothing there for as long as it stands. A pane a person's hand
/// reached is never asked at all.
#[test]
fn a_restored_pane_is_pointed_at_once_its_composer_stands_and_never_under_a_persons_hand() {
    const OLD_LEADER: u32 = 1_154_820;
    const WORKER: u32 = 1_154_821;
    const MOUNTING: u32 = 1_154_826;
    const DRAFTING: u32 = 1_154_827;
    const TAKEN: u32 = 1_154_828;
    const NEW_LEADER: u32 = 1_154_830;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    a_pointer_that_has_said_nothing();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "mounting");
    let mut workers = Vec::new();
    for (name, term) in [
        ("mounting", MOUNTING),
        ("drafting", DRAFTING),
        ("taken", TAKEN),
    ] {
        let session = format!("session-t11548-{name}");
        let (_, worker, at) = a_worker(&host, &team, "--agent codex", Some(&session));
        super::super::pane_turn_began(at, clock());
        super::super::pane_turn_ended(at, clock(), false, clock());
        workers.push((worker, at, session, term));
    }
    let old: Vec<u32> = workers.iter().map(|(_, at, ..)| *at).collect();
    the_window_goes(&census_without_commands, &old);
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();
    let door = Door::new(checkout.path());
    for (_, _, session, term) in &workers {
        door.wake(*term, "codex", session).expect("the wake");
    }
    door.settle();
    // A person's key in the third restored pane.
    super::super::pane_taken_over(TAKEN, clock());
    coordinator_back(&host, "mounting", OLD_LEADER, NEW_LEADER);
    let back = back_team("mounting", NEW_LEADER);
    for (worker, ..) in &workers {
        a_status_to(&host, &back, worker);
    }

    let beat = Beat::new(&host);
    beat.answering(MOUNTING, &[DeliveryOutcome::TimedOut]);
    let draft = DeliveryOutcome::Refused(zerocode_pty::ready::Refusal::HoldsADraft);
    beat.answering(DRAFTING, &[draft, draft, draft, draft]);
    beat.beats(1);
    assert_eq!(
        beat.pointed_at(MOUNTING).1,
        0,
        "a line landed before the composer stood"
    );
    beat.beats(3);
    assert_eq!(
        beat.pointed_at(MOUNTING),
        (2, 1),
        "the pane whose composer came up was not pointed at once it stood, and only once"
    );
    let (asked, landed) = beat.pointed_at(DRAFTING);
    assert!(asked >= 1, "the drafting pane was never offered the line");
    assert_eq!(landed, 0, "a line landed over a person's draft");
    assert_eq!(
        beat.pointed_at(TAKEN),
        (0, 0),
        "a pane a person's hand reached was offered the line"
    );
    for term in [MOUNTING, DRAFTING, TAKEN, NEW_LEADER] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-11548 (b): a worker whose turn the restart cut comes back with its
/// continuation, and the continuation is the only line it hears about its
/// mail until that turn is over — the pane is not written down at rest on
/// the restart's account, so the pointer has nothing to type until the
/// pane's own hooks say the continued turn ended.
#[test]
fn a_worker_cut_mid_turn_hears_its_continuation_and_no_pointer_beside_it() {
    const OLD_LEADER: u32 = 1_154_840;
    const WORKER: u32 = 1_154_841;
    const DOOR: u32 = 1_154_846;
    const NEW_LEADER: u32 = 1_154_850;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    a_pointer_that_has_said_nothing();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "mid-turn");
    let session = "session-t11548-mid-turn";
    let (_, worker, term) = a_worker(&host, &team, "--agent codex", Some(session));
    super::super::pane_turn_began(term, clock());
    the_window_goes(&census_without_commands, &[term]);
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();
    let door = Door::new(checkout.path());
    door.wake(DOOR, "codex", session).expect("the wake");
    door.settle();
    let (asked, reached) = door.typed(DOOR);
    assert_eq!(asked, 1, "the continuation was not typed once");
    assert!(reached[0].starts_with(crate::RESTART_NUDGE), "{reached:?}");
    assert!(
        super::super::pane_turns()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&DOOR)
            .is_none(),
        "a pane whose turn the restart cut was written down at rest"
    );
    coordinator_back(&host, "mid-turn", OLD_LEADER, NEW_LEADER);
    a_status_to(&host, &back_team("mid-turn", NEW_LEADER), &worker);

    let beat = Beat::new(&host);
    beat.beats(2);
    assert_eq!(
        beat.pointed_at(DOOR),
        (0, 0),
        "a pointer was typed beside the continuation"
    );
    // The continued turn runs, and ends: the ordinary road speaks, once.
    super::super::pane_turn_began(DOOR, clock());
    beat.beats(1);
    assert_eq!(beat.pointed_at(DOOR), (0, 0), "typed into the running turn");
    super::super::pane_turn_ended(DOOR, clock(), false, clock());
    beat.beats(2);
    assert_eq!(beat.pointed_at(DOOR), (1, 1));
    for term in [DOOR, NEW_LEADER] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-11537, the breath between a resumed Claude's two reports: its
/// `SessionStart` lands as the idle boundary it is, and the prompt its argv
/// carried begins the continued turn a moment later. A pointer typed in
/// that breath is a second line behind the continuation — the coordinator
/// told its run's mail twice. The wake's words own the composer until the
/// pane is heard taking them; then the ordinary road speaks, once, when the
/// continued turn is over.
#[test]
fn a_pointer_waits_until_the_pane_has_taken_its_continuation() {
    const OLD_LEADER: u32 = 1_153_780;
    const WORKER: u32 = 1_153_781;
    const DOOR: u32 = 1_153_786;
    const NEW_LEADER: u32 = 1_153_790;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    a_pointer_that_has_said_nothing();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "breath");
    let session = "11537b0c-0000-4000-8000-000000000786";
    let (_, worker, term) = a_worker(&host, &team, "--agent claude", Some(session));
    super::super::pane_turn_began(term, clock());
    the_window_goes(&census_without_commands, &[term]);
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();
    let door = Door::new(checkout.path());
    door.wake(DOOR, "claude", session).expect("the wake");
    assert_eq!(
        door.started()[0].1.matches(crate::RESTART_NUDGE).count(),
        1,
        "the continuation did not ride the launch"
    );
    coordinator_back(&host, "breath", OLD_LEADER, NEW_LEADER);
    a_status_to(&host, &back_team("breath", NEW_LEADER), &worker);

    let beat = Beat::waking(&host, &door);
    // The boundary: a rest, heard before the argv's prompt is.
    super::super::pane_turn_ended(DOOR, clock(), false, clock());
    beat.beats(2);
    assert_eq!(
        beat.pointed_at(DOOR),
        (0, 0),
        "a pointer was typed while the continuation waited for its pane"
    );
    // The pane takes the prompt: the wake's receipt, and the turn.
    let _ = crate::restart_nudge_runtime::WakeReceipts::rows(&door)
        .working(DOOR, std::time::Instant::now());
    super::super::pane_turn_began(DOOR, clock());
    beat.beats(1);
    assert_eq!(beat.pointed_at(DOOR), (0, 0), "typed into the running turn");
    super::super::pane_turn_ended(DOOR, clock(), false, clock());
    beat.beats(2);
    assert_eq!(beat.pointed_at(DOOR), (1, 1));
    for term in [DOOR, NEW_LEADER] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-11548: the rest a goodbye kept fills a silence and nothing else. A
/// pane already heard in this window — its turn begun before its wake got
/// round to writing the rest down — keeps what it said; the note is spent
/// either way, so no later wake writes that rest over a later word.
#[test]
fn a_rest_the_goodbye_kept_never_overwrites_what_the_pane_said_since() {
    const SPOKE: u32 = 1_154_860;
    const SILENT: u32 = 1_154_861;
    let root = tempfile::tempdir().expect("a data root");
    let at_rest = |worker: &str, term: u32| restart_census::WorkerCut {
        worker: worker.to_string(),
        agent: "codex".to_string(),
        term,
        turn: restart_census::Turn::Rest,
        commands: Some(Vec::new()),
    };
    restart_census::leave_cut(
        root.path(),
        &restart_census::RestartCensus {
            workers: vec![at_rest("w-spoke", SPOKE), at_rest("w-silent", SILENT)],
            coordinators: Vec::new(),
            took_ms: 0,
        },
        &|_| false,
    )
    .expect("the goodbye");
    super::super::pane_turn_began(SPOKE, clock());
    super::super::resumed_at_rest(root.path(), SPOKE, "w-spoke");
    super::super::resumed_at_rest(root.path(), SILENT, "w-silent");
    let turns = super::super::pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone();
    assert!(
        matches!(turns.get(&SPOKE), Some(PaneTurn::Running { .. })),
        "the goodbye's rest was written over a turn the pane began"
    );
    assert!(matches!(
        turns.get(&SILENT),
        Some(PaneTurn::Ended { interrupted: false })
    ));
    for (worker, term) in [("w-spoke", SPOKE), ("w-silent", SILENT)] {
        assert!(
            restart_census::peek_cut(root.path(), worker)
                .rest()
                .is_none(),
            "{worker}'s rest is still there to write down"
        );
        super::super::pane_turns()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .remove(&term);
    }
}

/// The coordinator's conversation, as its tab resumes it: the session the
/// test host names the old leader pane's agent by.
fn coordinator_session(old_leader: u32) -> String {
    format!("test-session-of-term-{old_leader}")
}

/// t-11537 (a) and (c): a coordinator whose run was at work when the window
/// went is told so once, by the launch its tab resumes with — how many
/// workers still carry a dispatch, how many tasks are dispatched, the mail
/// waiting for it as the pointer says it, and that the timers of its own
/// session did not survive. Whether its own turn was cut decides the first
/// sentence. The ledger closed and opened again from its store between the
/// goodbye and the wake. A second wake of the same conversation for the same
/// restart — the tab reopened — is told nothing.
#[test]
fn a_coordinator_whose_run_was_working_is_told_once_to_carry_on() {
    const OLD_LEADER: u32 = 1_153_700;
    const WORKER: u32 = 1_153_701;
    const TAB: u32 = 1_153_706;
    const TAB_AGAIN: u32 = 1_153_707;
    for (at, cut) in [false, true].into_iter().enumerate() {
        let old_leader = OLD_LEADER + u32::try_from(at).expect("a shape") * 20;
        let tab = TAB + u32::try_from(at).expect("a shape") * 20;
        let tab_again = TAB_AGAIN + u32::try_from(at).expect("a shape") * 20;
        let (_window, store) = PrivateWindow::boot();
        let _beat = one_beat_at_a_time();
        let checkout = tempfile::tempdir().expect("the worker's checkout");
        let host = Restoring::new(
            checkout.path(),
            WORKER + u32::try_from(at).expect("a shape") * 20,
            (old_leader, test_actor(old_leader)),
        );
        let (team, run) = a_run(&host, old_leader, &format!("coordinating-{at}"));
        let (_, _worker, term) = a_worker(
            &host,
            &team,
            "--agent codex",
            Some(&format!("session-t11537-worker-{at}")),
        );
        super::super::pane_turn_began(term, clock());
        // The worker's status to its run, which the coordinator has not read.
        let pane = crate::agent_teams::teams()
            .get(&team)
            .and_then(|held| {
                held.panes()
                    .find(|pane| pane.term == term)
                    .map(|pane| pane.id.clone())
            })
            .expect("the worker's pane");
        let capability = crate::agent_teams::current_pane_capability(&team, &pane)
            .expect("the worker's capability");
        let posted = run_verb(
            &host,
            &team,
            &pane,
            &capability,
            "send --type status --body underway",
        );
        assert_eq!(posted, 0);
        super::super::pane_turn_began(old_leader, clock());
        if !cut {
            super::super::pane_turn_ended(old_leader, clock(), false, clock());
        }
        the_window_goes(&census_without_commands, &[term]);
        assert_eq!(
            logged(&format!(
                "exit: coordinator of run:{run} on terminal {old_leader} · turn {}",
                if cut { "running" } else { "rest" }
            )),
            1,
            "the goodbye did not name the coordinator's pane"
        );
        the_ledger_reopens(&store);
        the_next_boot(old_leader);

        let door = Door::new(checkout.path());
        let session = coordinator_session(old_leader);
        door.wake(tab, "claude", &session)
            .expect("the coordinator's wake");
        door.settle();
        let started = door.started();
        assert_eq!(started.len(), 1, "{started:?}");
        let command = &started[0].1;
        assert!(
            command.contains(&format!(
                "run:{run} has 1 worker(s) carrying a dispatch and 1 task(s) dispatched."
            )),
            "the coordinator was not told what its run holds: {command}"
        );
        // The mail, said the way the pointer says it; how many the ledger
        // counts is its own business (it writes news to the run as well).
        assert_eq!(
            command.matches("orchestration message").count(),
            1,
            "{command}"
        );
        assert_eq!(
            command.matches(crate::RESTART_NUDGE).count(),
            usize::from(cut),
            "its own turn's cut was said wrongly: {command}"
        );
        assert_eq!(door.typed(tab).0, 0, "argv words were typed too");

        // The same restart, the tab opened again: nothing more is owed.
        crate::agent_teams::forget_term(tab);
        let again = Door::new(checkout.path());
        again
            .wake(tab_again, "claude", &session)
            .expect("the second wake");
        let started = again.started();
        assert_eq!(started.len(), 1);
        assert!(
            !started[0].1.contains("carrying a dispatch"),
            "the same restart was said twice: {}",
            started[0].1
        );
        crate::agent_teams::forget_term(tab_again);
    }
}

/// One verb from `team`/`pane` with the capability that pane holds, answered
/// with its exit code.
fn run_verb(host: &dyn Host, team: &str, pane: &str, capability: &str, line: &str) -> i32 {
    run(
        host,
        Vec::new(),
        team,
        pane,
        capability,
        &words(line),
        clock(),
    )
    .exit_code
}

/// t-11537 (b), (e) and the person's hand: a coordinator is told nothing
/// when its run stood idle, when a person's hand ended its last turn, or
/// when the conversation coordinated no run this window's goodbye wrote down.
#[test]
fn a_coordinator_is_told_nothing_when_its_run_was_idle_or_its_pane_was_the_persons() {
    const OLD_LEADER: u32 = 1_153_760;
    const WORKER: u32 = 1_153_761;
    const TAB: u32 = 1_153_766;
    for (at, shape) in ["idle", "held", "nobody"].into_iter().enumerate() {
        let shift = u32::try_from(at).expect("a shape") * 20;
        let old_leader = OLD_LEADER + shift;
        let tab = TAB + shift;
        let (_window, _store) = PrivateWindow::boot();
        let _beat = one_beat_at_a_time();
        let checkout = tempfile::tempdir().expect("the worker's checkout");
        let host = Restoring::new(
            checkout.path(),
            WORKER + shift,
            (old_leader, test_actor(old_leader)),
        );
        let (team, _run) = a_run(&host, old_leader, &format!("told-nothing-{shape}"));
        let mut panes = Vec::new();
        if shape != "idle" {
            let (_, _, term) = a_worker(
                &host,
                &team,
                "--agent codex",
                Some(&format!("session-t11537-{shape}")),
            );
            super::super::pane_turn_began(term, clock());
            panes.push(term);
        }
        super::super::pane_turn_began(old_leader, clock());
        super::super::pane_turn_ended(old_leader, clock(), shape == "held", clock());
        the_window_goes(&census_without_commands, &panes);
        the_next_boot(old_leader);

        let door = Door::new(checkout.path());
        let session = if shape == "nobody" {
            "session-t11537-coordinates-nothing".to_string()
        } else {
            coordinator_session(old_leader)
        };
        door.wake(tab, "claude", &session).expect("the wake");
        door.settle();
        let started = door.started();
        assert_eq!(started.len(), 1, "{shape}: {started:?}");
        assert!(
            !started[0].1.contains("carrying a dispatch")
                && !started[0].1.contains(crate::RESTART_NUDGE),
            "{shape}: the coordinator was told to go on: {}",
            started[0].1
        );
        assert_eq!(door.typed(tab).0, 0, "{shape}");
        crate::agent_teams::forget_term(tab);
    }
}
