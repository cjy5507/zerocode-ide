//! t-7812: a window restart brings every ledger worker back as itself,
//! through one road per conversation — the ledger's reseat for the panes it
//! seated, a resumed pane as the witness for a conversation a door reopened —
//! and nothing comes back that was not there: no empty CLI where a worker's
//! conversation was, no second process on one transcript, no continuation
//! typed at a worker whose turn had ended.
//!
//! Everything here drives the production doors of a private window — its
//! real actor and store — through hosts that write down what the roads did
//! through them: the ledger's reseat through [`Restoring`], a door's wake
//! (`wake_conversation`, the body of `resume_session`) through the fake
//! launcher in `restore_door.rs`. A CLI counted here is one a real entry
//! point started (t-7812 r2): nothing is seated or resumed by hand.

use super::restore_door::{Door, coordinator_back, the_next_boot};
use super::*;
use crate::conversation_wake::ConversationWake;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The order things happened in across every road of one restore (t-7812
/// F): each host's events on one line, in the order they landed.
#[derive(Default)]
pub(super) struct Timeline(Mutex<Vec<String>>);

impl Timeline {
    pub(super) fn mark(&self, what: String) {
        self.0.lock().unwrap().push(what);
    }

    pub(super) fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

/// The worker the ledger holds live in the seat `term` is, if any — read at
/// the moment a host sees something happen there.
pub(super) fn worker_on(term: u32) -> Option<String> {
    let seat = crate::agent_teams::teams().iter().find_map(|(id, team)| {
        team.panes()
            .find(|pane| pane.term == term)
            .map(|pane| (id.clone(), pane.id.clone()))
    })?;
    the_rows()
        .workers
        .into_iter()
        .find(|worker| worker.team == seat.0 && worker.pane == seat.1 && worker.state.is_live())
        .map(|worker| worker.id)
}

/// A host that writes down what the restore roads did through it: the panes
/// cut, and the words typed into each. `during_split` runs inside a cut,
/// before it answers — the door that arrives while the ledger is cutting.
pub(super) struct Restoring {
    pub(super) checkout: String,
    returned_actor: Option<(u32, String)>,
    next: AtomicU32,
    pub(super) cut: Mutex<Vec<(u32, String)>>,
    pub(super) typed: Mutex<Vec<(u32, String)>>,
    pub(super) during_split: Mutex<Option<Box<dyn Fn() + Send>>>,
    /// How many of the next cuts the host refuses — a pty table that is
    /// full, a fork the kernel will not make: nothing starts.
    refusing: AtomicU32,
    /// Where this host's cuts and keys are written in order, beside the
    /// other roads' (t-7812 F).
    pub(super) timeline: Mutex<Option<Arc<Timeline>>>,
}

impl Restoring {
    pub(super) fn new(
        checkout: &std::path::Path,
        first_term: u32,
        returned: (u32, String),
    ) -> Self {
        Self {
            checkout: checkout.to_string_lossy().into_owned(),
            returned_actor: Some(returned),
            next: AtomicU32::new(first_term),
            cut: Mutex::new(Vec::new()),
            typed: Mutex::new(Vec::new()),
            during_split: Mutex::new(None),
            refusing: AtomicU32::new(0),
            timeline: Mutex::new(None),
        }
    }

    fn mark(&self, what: String) {
        if let Some(timeline) = self.timeline.lock().unwrap().as_ref() {
            timeline.mark(what);
        }
    }

    pub(super) fn cuts(&self) -> Vec<(u32, String)> {
        self.cut.lock().unwrap().clone()
    }

    /// Refuse the next `count` cuts.
    pub(super) fn refuse_splits(&self, count: u32) {
        self.refusing.store(count, Ordering::SeqCst);
    }

    pub(super) fn typed_at(&self, term: u32) -> Vec<String> {
        self.typed
            .lock()
            .unwrap()
            .iter()
            .filter(|(at, _)| *at == term)
            .map(|(_, words)| words.clone())
            .collect()
    }
}

impl Host for Restoring {
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        command: &str,
        token: &str,
    ) -> Option<u32> {
        let _ = crate::agent_teams::take_worker_host_ask(token);
        if self
            .refusing
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
        {
            return None;
        }
        crate::agent_teams::place_seat_checkout(token, self.checkout.clone());
        if let Some(during) = self.during_split.lock().unwrap().as_ref() {
            during();
        }
        let term = self.next.fetch_add(1, Ordering::SeqCst);
        self.cut.lock().unwrap().push((term, command.to_string()));
        self.mark(format!("cut t{term}"));
        Some(term)
    }
    fn send(&self, term: u32, text: &str) -> bool {
        self.typed.lock().unwrap().push((term, text.to_string()));
        if self.timeline.lock().unwrap().is_some() {
            self.mark(format!("typed t{term} seated={:?}", worker_on(term)));
        }
        true
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some(String::new())
    }
    fn focus(&self, _term: u32) -> bool {
        true
    }
    fn close(&self, _term: u32) {}
    fn actor_for(&self, term: u32) -> Option<String> {
        self.returned_actor
            .as_ref()
            .filter(|(returned, _)| *returned == term)
            .map(|(_, actor)| actor.clone())
            .or_else(|| Some(test_actor(term)))
    }
}

/// A verb from the coordinator's leader pane in `team`, which must land.
pub(super) fn say(host: &dyn Host, team: &str, line: &str) -> String {
    let answered = run(
        host,
        Vec::new(),
        team,
        zerocode_core::agent_teams::LEADER_PANE,
        TEST_CAPABILITY,
        &words(line),
        clock(),
    );
    assert_eq!(answered.exit_code, 0, "{line}: {}", answered.stderr);
    answered.stdout
}

/// The old window's coordinator and its run.
pub(super) fn a_run(host: &dyn Host, leader: u32, name: &str) -> (String, String) {
    let team = format!("team-t7812-{name}-{leader}");
    seat_a_team(&team, leader);
    say(
        host,
        &team,
        &format!("run-create --name t7812-{name}-{leader}"),
    );
    let run = super::super::bound_run(
        &team,
        zerocode_core::agent_teams::LEADER_PANE,
        Some(&test_actor(leader)),
    )
    .expect("the coordinator's run");
    (team, run)
}

/// One worker summoned into that run with `launch` (agent, model, effort),
/// and — unless `session` is `None` — the conversation it reported. Answers
/// the task, the worker and the terminal its pane holds.
pub(super) fn a_worker(
    host: &Restoring,
    team: &str,
    launch: &str,
    session: Option<&str>,
) -> (String, String, u32) {
    let task: serde_json::Value =
        serde_json::from_str(&say(host, team, "task-create --spec keep-going")).expect("a task");
    let task = task["taskId"].as_str().expect("a task id").to_string();
    let started: serde_json::Value = serde_json::from_str(&say(
        host,
        team,
        &format!("worker-start {launch} --task {task}"),
    ))
    .expect("a worker");
    let worker = started["workerId"]
        .as_str()
        .expect("a worker id")
        .to_string();
    let term = host.cuts().last().expect("the worker's pane").0;
    if let Some(id) = session {
        let held = super::super::runtime().expect("the private runtime");
        assert!(
            held.actor
                .worker_session_reported(
                    term,
                    zerocode_core::ProviderSession {
                        key: zerocode_core::SessionKey::SessionId,
                        id: id.to_string(),
                        transcript_path: None,
                    },
                    clock(),
                )
                .expect("the session lands")
                .0,
            "{worker}'s session was not written"
        );
    }
    (task, worker, term)
}

/// The window goes the way the ⌘Q road now goes (t-7812 D1): the goodbye
/// with the census read as it goes, and only then the panes.
pub(super) fn the_window_goes(census: &dyn Fn() -> restart_census::RestartCensus, panes: &[u32]) {
    super::super::window_exiting(clock(), crate::exit_runtime::ExitRoad::Terminate, census);
    for term in panes {
        super::super::terminal_gone(*term, clock());
        crate::agent_teams::forget_term(*term);
    }
}

/// The census as the goodbye reads it, off this window's own tables, with
/// no process table to read commands from.
pub(super) fn census_without_commands() -> restart_census::RestartCensus {
    restart_census::take(&|_| None, &|| Err("no process table here".to_string()))
}

pub(super) fn row(worker: &str) -> zerocode_core::orchestration::WorkerRow {
    the_rows()
        .workers
        .into_iter()
        .find(|held| held.id == worker)
        .expect("the worker row")
}

pub(super) fn deaths_of(worker: &str) -> Vec<serde_json::Value> {
    worker_died_bodies(&the_rows())
        .into_iter()
        .filter(|body| body["workerId"] == worker)
        .collect()
}

/// The next window: its boot sweep (`window_restarted` — every seat vacated,
/// anything still live asleep), then its coordinator back — a new leader
/// pane whose conversation is the old leader's — asking its run's sleepers
/// back.
pub(super) fn the_coordinator_returns(
    host: &dyn Host,
    name: &str,
    old_leader: u32,
    new_leader: u32,
) -> usize {
    crate::agent_teams::forget_term(old_leader);
    super::super::runtime()
        .expect("the private runtime")
        .actor
        .window_restarted(clock())
        .expect("the next boot's sweep");
    seat_a_team(&format!("team-t7812-{name}-back-{new_leader}"), new_leader);
    super::super::reseat_sleeping(host, Vec::new(), new_leader, Some(&test_actor(old_leader)))
}

/// t-7812 B / observation 4: a worker whose pane the person had touched
/// comes back — through the ledger's own reseat, because the window never
/// keeps a ledger-seated tab to reopen — as the same worker with the same
/// dispatch, launched as it was summoned (model, effort, peer name), and
/// its mail reaches it where it stands now. On 2026-09-25 the grace ended
/// two such workers (w-7570, w-7631, both `takenOver`) at 01:13.
#[test]
fn a_taken_over_sleeper_comes_back_through_the_ledgers_reseat_as_the_same_worker() {
    const OLD_LEADER: u32 = 278_120;
    const WORKER: u32 = 278_121;
    const NEW_LEADER: u32 = 278_130;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "taken");
    let session = "7812a0c1-0000-4000-8000-000000000001";
    let (task, worker, term) = a_worker(
        &host,
        &team,
        "--agent claude --model claude-fable-5-1 --effort xhigh",
        Some(session),
    );
    super::super::pane_taken_over(term, clock());
    assert!(
        row(&worker).taken_over,
        "the person's key never reached the row"
    );
    let before = row(&worker);
    the_window_goes(&census_without_commands, &[term]);
    assert_eq!(row(&worker).state, WorkerState::Sleeping);
    host.cut.lock().unwrap().clear();

    let restored = the_coordinator_returns(&host, "taken", OLD_LEADER, NEW_LEADER);
    assert_eq!(
        restored, 1,
        "the ledger left a taken-over sleeper to a tab the window never keeps"
    );
    let back = row(&worker);
    assert_eq!(back.state, WorkerState::Active);
    assert_eq!(
        back.dispatch, before.dispatch,
        "a second attempt was minted"
    );
    assert!(
        back.taken_over,
        "the restart forgot whose hand was on the pane"
    );
    let rows = the_rows();
    let carried = rows
        .tasks
        .iter()
        .find(|held| held.id == task)
        .expect("the task");
    assert_eq!(
        carried.status,
        zerocode_core::orchestration::TaskStatus::Dispatched
    );
    assert_eq!(carried.failures, 0, "the restart spent the attempt");
    assert!(
        deaths_of(&worker).is_empty(),
        "the restart was announced as a death"
    );
    let cuts = host.cuts();
    assert_eq!(cuts.len(), 1, "one conversation, one pane: {cuts:?}");
    for said in [
        "--resume",
        session,
        "--model claude-fable-5-1",
        "--effort xhigh",
        "--name",
    ] {
        assert!(
            cuts[0].1.contains(said),
            "the pane came back without `{said}`: {}",
            cuts[0].1
        );
    }
    // Observation 8: its mail reaches it where it stands now.
    say(
        &host,
        &format!("team-t7812-taken-back-{NEW_LEADER}"),
        &format!(
            "send --to @worktree:{} --type status --body back-again",
            checkout.path().display()
        ),
    );
    crate::agent_teams::forget_term(cuts[0].0);
    crate::agent_teams::forget_term(NEW_LEADER);
}

/// t-7812 A/B, astra's rendezvous: one conversation comes back once when a
/// door (a sidebar row, a restored tab — `resume_session`'s own road,
/// `wake_conversation`) and the ledger's reseat reach it together, in either
/// order, each arriving inside the other's run. Door first: the coordinator
/// comes back while the door's wake is building its pane — after its claim,
/// before its seat — and its reseat cuts nothing; the door's pane is the
/// worker, seated before its process starts. Ledger first: a door arrives
/// while the reseat is cutting the pane, is told the conversation is
/// already coming back, and starts nothing. Before, the reseat never asked,
/// and whichever came second put a second process on the transcript.
#[test]
fn a_door_and_the_ledgers_reseat_bring_one_conversation_back_once_in_either_order() {
    const OLD_LEADER: u32 = 278_140;
    const WORKER: u32 = 278_141;
    const DOOR: u32 = 278_148;
    const NEW_LEADER: u32 = 278_150;
    for door_first in [true, false] {
        let (_window, _store) = PrivateWindow::boot();
        let checkout = tempfile::tempdir().expect("the worker's checkout");
        let host = Arc::new(Restoring::new(
            checkout.path(),
            WORKER,
            (NEW_LEADER, test_actor(OLD_LEADER)),
        ));
        let name = if door_first { "door" } else { "ledger" };
        let (team, _run) = a_run(&*host, OLD_LEADER, name);
        let session = if door_first {
            "session-t7812-door-first"
        } else {
            "session-t7812-ledger-first"
        };
        let (_task, worker, term) = a_worker(&host, &team, "--agent codex", Some(session));
        let before = row(&worker);
        the_window_goes(&census_without_commands, &[term]);
        the_next_boot(OLD_LEADER);
        host.cut.lock().unwrap().clear();
        let door = Arc::new(Door::new(checkout.path()));

        if door_first {
            let reseating = Arc::clone(&host);
            let reseated: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
            let counted = Arc::clone(&reseated);
            *door.during_prepare.lock().unwrap() = Some(Box::new(move || {
                *counted.lock().unwrap() =
                    Some(coordinator_back(&*reseating, name, OLD_LEADER, NEW_LEADER));
            }));
            assert_eq!(
                door.wake(DOOR, "codex", session),
                Ok(ConversationWake::opened(DOOR))
            );
            assert_eq!(
                *reseated.lock().unwrap(),
                Some(0),
                "the ledger cut a pane for a conversation a door was opening"
            );
            assert!(host.cuts().is_empty(), "{:?}", host.cuts());
            assert_eq!(
                door.seated_at_start(DOOR).as_deref(),
                Some(worker.as_str()),
                "the door's process started before its seat"
            );
        } else {
            let asking = Arc::clone(&door);
            let asked: Arc<Mutex<Option<Result<ConversationWake, String>>>> =
                Arc::new(Mutex::new(None));
            let answered = Arc::clone(&asked);
            *host.during_split.lock().unwrap() = Some(Box::new(move || {
                *answered.lock().unwrap() = Some(asking.wake(DOOR, "codex", session));
            }));
            assert_eq!(coordinator_back(&*host, name, OLD_LEADER, NEW_LEADER), 1);
            assert_eq!(
                *asked.lock().unwrap(),
                Some(Ok(ConversationWake::standing(0))),
                "a door asking while the ledger cut the pane was not told it was coming back"
            );
            assert!(door.started().is_empty(), "{:?}", door.started());
        }
        let back = row(&worker);
        assert_eq!(back.state, WorkerState::Active, "door first: {door_first}");
        assert_eq!(back.dispatch, before.dispatch);
        assert_eq!(
            host.cuts().len() + door.started().len(),
            1,
            "door first: {door_first}: one conversation, one process: {:?} {:?}",
            host.cuts(),
            door.started()
        );
        assert!(deaths_of(&worker).is_empty());
        for (term, _) in host.cuts() {
            crate::agent_teams::forget_term(term);
        }
        crate::agent_teams::forget_term(DOOR);
        crate::agent_teams::forget_term(NEW_LEADER);
    }
}

/// t-7812 E: a reseated worker is told to go on only when the goodbye read
/// its turn as under way. One at rest, and one whose window crashed — no
/// goodbye to read at all — come back to an untouched composer: nothing
/// typed, not even an empty line. Before, the reseat typed the continue
/// sentence at every worker it restored.
#[test]
fn a_reseated_worker_is_told_to_go_on_only_when_the_goodbye_cut_its_turn() {
    const OLD_LEADER: u32 = 278_160;
    const WORKER: u32 = 278_161;
    const NEW_LEADER: u32 = 278_170;
    for shape in ["mid-turn", "at-rest", "crash"] {
        let (_window, _store) = PrivateWindow::boot();
        let checkout = tempfile::tempdir().expect("the worker's checkout");
        let host = Restoring::new(
            checkout.path(),
            WORKER,
            (NEW_LEADER, test_actor(OLD_LEADER)),
        );
        let (team, _run) = a_run(&host, OLD_LEADER, shape);
        let (_task, worker, term) = a_worker(
            &host,
            &team,
            "--agent codex",
            Some(&format!("session-t7812-{shape}")),
        );
        match shape {
            "mid-turn" => {
                super::super::pane_turn_began(term, clock());
                the_window_goes(&census_without_commands, &[term]);
            }
            "at-rest" => {
                super::super::pane_turn_began(term, clock());
                super::super::pane_turn_ended(term, clock(), false, clock());
                the_window_goes(&census_without_commands, &[term]);
            }
            _ => {
                // No goodbye: the boot's own sweep finds the seat. Whatever
                // an earlier scenario's goodbye left in this shared data root
                // belonged to an earlier boot, whose first wake took it.
                restart_census::leave_cut(
                    super::super::BLACKBOX
                        .get()
                        .expect("the window's data root"),
                    &restart_census::RestartCensus::default(),
                    &|_| false,
                )
                .expect("no note left");
                crate::agent_teams::forget_term(term);
                assert_eq!(
                    super::super::runtime()
                        .expect("the private runtime")
                        .actor
                        .window_restarted(clock())
                        .expect("the boot sweep")
                        .0
                        .sleeping,
                    1
                );
            }
        }
        host.cut.lock().unwrap().clear();
        host.typed.lock().unwrap().clear();
        assert_eq!(
            the_coordinator_returns(&host, shape, OLD_LEADER, NEW_LEADER),
            1,
            "{shape}"
        );
        let reseated = host.cuts()[0].0;
        let typed = host.typed_at(reseated);
        if shape == "mid-turn" {
            assert_eq!(typed.len(), 1, "{shape}: {typed:?}");
            assert!(
                typed[0].starts_with(crate::RESTART_NUDGE),
                "{shape}: the continuation is not the restart sentence: {typed:?}"
            );
            assert!(
                typed[0].contains("Your ledger seat was restored"),
                "{shape}: the seat sentence is missing: {typed:?}"
            );
        } else {
            assert!(
                typed.is_empty(),
                "{shape}: a worker whose turn had ended was typed at: {typed:?}"
            );
        }
        assert_eq!(row(&worker).state, WorkerState::Active, "{shape}");
        crate::agent_teams::forget_term(reseated);
        crate::agent_teams::forget_term(NEW_LEADER);
    }
}

/// t-7812 C: a sleeper that never recorded its conversation is not
/// started fresh in its place — an empty CLI where the work was — and the
/// run hears it once, now, with the dispatch id a `--retry-of` needs.
#[test]
fn a_sleeper_that_recorded_no_conversation_is_told_once_and_nothing_starts_in_its_place() {
    const OLD_LEADER: u32 = 278_180;
    const WORKER: u32 = 278_181;
    const NEW_LEADER: u32 = 278_190;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "unrecorded");
    let (_task, worker, term) = a_worker(&host, &team, "--agent codex", None);
    let dispatch = row(&worker).dispatch.expect("the dispatch");
    the_window_goes(&census_without_commands, &[term]);
    host.cut.lock().unwrap().clear();

    assert_eq!(
        the_coordinator_returns(&host, "unrecorded", OLD_LEADER, NEW_LEADER),
        0
    );
    assert!(
        host.cuts().is_empty(),
        "a CLI was started where the worker's conversation was: {:?}",
        host.cuts()
    );
    let told = deaths_of(&worker);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["dispatchId"], dispatch.as_str());
    assert!(
        told[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("was never recorded")),
        "{}",
        told[0]
    );
    // The grace, later, has nothing left to end or to say.
    let _old = BootedHere::at(clock() - zerocode_core::orchestration::RESEAT_GRACE_MS - 1);
    super::super::tick(&host, &[], clock());
    assert_eq!(deaths_of(&worker).len(), 1, "the loss was told twice");
    crate::agent_teams::forget_term(NEW_LEADER);
}

/// The road a manifest row comes back by (t-7812 F).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
enum Road {
    /// The ledger's reseat cuts it a pane.
    Ledger,
    /// A door — a restored tab, a sidebar row — wakes it.
    Door,
    /// Nothing can: its run is told why, once.
    Told,
}

/// One row of the restore manifest (t-7812 F).
struct Leaf {
    agent: &'static str,
    session: Option<&'static str>,
    /// `None` for a person's own tab.
    worker: Option<String>,
    road: Road,
}

/// t-7812 F: eight agent tabs before the restart — Claude 4, Codex 3, one
/// worker whose conversation was never recorded; five of them workers —
/// and one finished worker whose conversation a person's tab still holds.
/// Every tab comes back by a real road: the ledger's reseat for the panes
/// it seated (`reseat_sleeping`, cutting through [`Restoring`]), and a door
/// for the tabs the window kept (`wake_conversation`, the whole of
/// `resume_session` but the terminal, through `restore_door`'s fake
/// launcher) — and the two race, the coordinator returning while W4's door
/// is building its pane. After it: seven conversations resumed, every one
/// started by a real entry point and none made by hand, no empty CLI and no
/// pane outside the layout, the four workers with a conversation bound as
/// themselves and each seated before its words, the fifth told once, the
/// finished dispatch not revived, and the continuation typed only where the
/// goodbye cut something. The tally and the order of events are printed for
/// the report before anything is asserted, so a red run prints its own.
#[test]
fn eight_tabs_come_back_as_seven_conversations_and_one_notice() {
    const OLD_LEADER: u32 = 278_200;
    const FIRST_WORKER: u32 = 278_201;
    const NEW_LEADER: u32 = 278_220;
    const W4_DOOR: u32 = 278_233;
    const P1_DOOR: u32 = 278_234;
    const P2_DOOR: u32 = 278_235;
    const W6_DOOR: u32 = 278_236;
    const ROOT: u32 = 78_120;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let timeline = Arc::new(Timeline::default());
    let host = Arc::new(Restoring::new(
        checkout.path(),
        FIRST_WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    ));
    let (team, _run) = a_run(&*host, OLD_LEADER, "manifest");
    let claude = "--agent claude --model claude-fable-5-1 --effort xhigh";
    let codex = "--agent codex --model gpt-6-sol --effort high";
    let summon =
        |launch: &str, session: Option<&'static str>| a_worker(&host, &team, launch, session);
    // W1 claude, taken over; W2 claude, mid-turn; W3 codex, a gate under it;
    // W4 codex, taken over, reopened by a person's door; W5 codex, nothing
    // recorded; W6 finished before the restart, its conversation in a
    // person's tab.
    let w1 = summon(claude, Some("7812a0c1-0000-4000-8000-0000000000a1"));
    let w2 = summon(claude, Some("7812a0c1-0000-4000-8000-0000000000a2"));
    let w3 = summon(codex, Some("session-t7812-manifest-w3"));
    let w4 = summon(codex, Some("session-t7812-manifest-w4"));
    let w5 = summon(codex, None);
    let w6 = summon(codex, Some("session-t7812-manifest-w6"));
    super::super::pane_taken_over(w1.2, clock());
    super::super::pane_taken_over(w4.2, clock());
    say(
        &*host,
        &team,
        &format!("worker-stop --worker {} --reason finished", w6.1),
    );
    let w6_dispatch = row(&w6.1).dispatch;
    let manifest = [
        Leaf {
            agent: "claude",
            session: Some("7812a0c1-0000-4000-8000-0000000000a1"),
            worker: Some(w1.1.clone()),
            road: Road::Ledger,
        },
        Leaf {
            agent: "claude",
            session: Some("7812a0c1-0000-4000-8000-0000000000a2"),
            worker: Some(w2.1.clone()),
            road: Road::Ledger,
        },
        Leaf {
            agent: "codex",
            session: Some("session-t7812-manifest-w3"),
            worker: Some(w3.1.clone()),
            road: Road::Ledger,
        },
        Leaf {
            agent: "codex",
            session: Some("session-t7812-manifest-w4"),
            worker: Some(w4.1.clone()),
            road: Road::Door,
        },
        Leaf {
            agent: "codex",
            session: None,
            worker: Some(w5.1.clone()),
            road: Road::Told,
        },
        Leaf {
            agent: "claude",
            session: Some("7812a0c1-0000-4000-8000-0000000000p1"),
            worker: None,
            road: Road::Door,
        },
        Leaf {
            agent: "claude",
            session: Some("7812a0c1-0000-4000-8000-0000000000p2"),
            worker: None,
            road: Road::Door,
        },
        Leaf {
            agent: "codex",
            session: Some("session-t7812-manifest-w6"),
            worker: None,
            road: Road::Door,
        },
    ];
    let dispatches: Vec<Option<String>> = [&w1, &w2, &w3, &w4]
        .iter()
        .map(|(_, worker, _)| row(worker).dispatch)
        .collect();

    // The goodbye reads W2 mid-turn and a gate running under W3's pane.
    super::super::pane_turn_began(w2.2, clock());
    let listing = format!(
        "501 {ROOT} 1 {ROOT} 0 1 Thu Sep 24 01:00:00 2026 node /opt/homebrew/bin/codex resume s\n\
         501 78121 {ROOT} {ROOT} 0 1 Thu Sep 24 01:00:00 2026 /opt/codex/vendor/bin/codex resume s\n\
         501 78122 78121 78122 0 1 Thu Sep 24 01:00:00 2026 /bin/zsh -lc just shell-test\n"
    );
    let w3_term = w3.2;
    let census = || {
        restart_census::take(&|term| (term == w3_term).then_some(ROOT), &|| {
            Ok(crate::resource_usage::ProcessSample::from_ps_listing(
                &listing,
            ))
        })
    };
    timeline.mark("goodbye".to_string());
    the_window_goes(&census, &[w1.2, w2.2, w3.2, w4.2, w5.2, w6.2]);
    the_next_boot(OLD_LEADER);
    timeline.mark("boot".to_string());
    host.cut.lock().unwrap().clear();
    host.typed.lock().unwrap().clear();
    *host.timeline.lock().unwrap() = Some(Arc::clone(&timeline));
    let door = Door::new(checkout.path());
    *door.timeline.lock().unwrap() = Some(Arc::clone(&timeline));

    // The window's doors for the four tabs it kept, and the coordinator
    // coming back while W4's is building its pane: the two roads race in
    // one run.
    let reseating = Arc::clone(&host);
    let noting = Arc::clone(&timeline);
    let reseated: Arc<Mutex<Option<usize>>> = Arc::new(Mutex::new(None));
    let counted = Arc::clone(&reseated);
    let w5_worker = w5.1.clone();
    *door.during_prepare.lock().unwrap() = Some(Box::new(move || {
        noting.mark("coordinator back".to_string());
        *counted.lock().unwrap() = Some(coordinator_back(
            &*reseating,
            "manifest",
            OLD_LEADER,
            NEW_LEADER,
        ));
        for told in deaths_of(&w5_worker) {
            noting.mark(format!("told {w5_worker}: {}", told["reason"]));
        }
    }));
    let mut woke = Vec::new();
    for (at, (agent, session)) in [
        (W4_DOOR, ("codex", "session-t7812-manifest-w4")),
        (P1_DOOR, ("claude", "7812a0c1-0000-4000-8000-0000000000p1")),
        (P2_DOOR, ("claude", "7812a0c1-0000-4000-8000-0000000000p2")),
        (W6_DOOR, ("codex", "session-t7812-manifest-w6")),
    ] {
        woke.push((at, door.wake(at, agent, session)));
    }
    door.settle();
    // The grace comes and finds nobody left asleep.
    let _boot = BootedHere::at(clock() - zerocode_core::orchestration::RESEAT_GRACE_MS);
    super::super::tick(&*host, &[], clock());
    timeline.mark("grace".to_string());

    let rows = the_rows();
    let cuts = host.cuts();
    let started: Vec<(u32, String, Road)> = cuts
        .iter()
        .map(|(term, command)| (*term, command.clone(), Road::Ledger))
        .chain(
            door.started()
                .into_iter()
                .map(|(term, command)| (term, command, Road::Door)),
        )
        .collect();
    let resumed = |command: &str| command.contains("--resume") || command.contains(" resume ");
    let blank = started
        .iter()
        .filter(|(_, command, _)| !resumed(command))
        .count();
    let of_row = |command: &str| {
        manifest.iter().position(|leaf| {
            leaf.session
                .is_some_and(|session| command.contains(session))
        })
    };
    let outside = started
        .iter()
        .filter(|(_, command, _)| of_row(command).is_none())
        .count();
    let sessions: Vec<usize> = started
        .iter()
        .filter_map(|(_, command, _)| of_row(command))
        .collect();
    let distinct: std::collections::HashSet<usize> = sessions.iter().copied().collect();
    let bound: Vec<bool> = [&w1, &w2, &w3, &w4]
        .iter()
        .zip(&dispatches)
        .map(|((_, worker, _), dispatch)| {
            let now = row(worker);
            now.state == WorkerState::Active && &now.dispatch == dispatch
        })
        .collect();
    let term_of = |at: usize| {
        started
            .iter()
            .find(|(_, command, _)| of_row(command) == Some(at))
            .map(|(term, _, road)| (*term, *road))
    };
    // Words typed at each row's pane that reached it, by whichever road.
    let told_at = |term: u32| host.typed_at(term).len() + door.typed(term).1.len();
    let nudged: Vec<Option<usize>> = (0..manifest.len())
        .map(|at| term_of(at).map(|(term, _)| told_at(term)))
        .collect();
    // The door's worker: seated before its process started. The ledger's:
    // typed at only once the pane was already its seat.
    let seated_first: Vec<bool> = std::iter::once(
        term_of(3).is_some_and(|(term, _)| door.seated_at_start(term) == manifest[3].worker),
    )
    .chain([1usize, 2].iter().map(|&at| {
        term_of(at).is_some_and(|(term, _)| {
            let seated = format!(
                "seated={:?}",
                manifest[at].worker.as_deref().map(str::to_string)
            );
            timeline
                .lines()
                .iter()
                .filter(|line| line.starts_with(&format!("typed t{term} ")))
                .all(|line| line.ends_with(&seated))
        })
    }))
    .collect();
    let died_now: usize = [&w1, &w2, &w3, &w4, &w5]
        .iter()
        .map(|(_, worker, _)| deaths_of(worker).len())
        .sum();
    let finished = row(&w6.1);
    let finished_revived = finished.state.is_live() || finished.dispatch != w6_dispatch;
    let order: std::collections::BTreeMap<String, Vec<String>> = manifest
        .iter()
        .enumerate()
        .map(|(at, leaf)| {
            let label = leaf
                .worker
                .clone()
                .unwrap_or_else(|| format!("person-{at}"));
            let mine: Vec<String> = match term_of(at) {
                Some((term, _)) => timeline
                    .lines()
                    .into_iter()
                    .filter(|line| {
                        line.contains(&format!("t{term} ")) || line.ends_with(&format!("t{term}"))
                    })
                    .collect(),
                None => timeline
                    .lines()
                    .into_iter()
                    .filter(|line| {
                        leaf.worker
                            .as_ref()
                            .is_some_and(|worker| line.contains(worker.as_str()))
                    })
                    .collect(),
            };
            (label, mine)
        })
        .collect();
    let tally = serde_json::json!({
        "tabsBefore": manifest.len(),
        "workersBefore": manifest.iter().filter(|leaf| leaf.worker.is_some()).count(),
        "claude": manifest.iter().filter(|leaf| leaf.agent == "claude" && leaf.session.is_some()).count(),
        "codex": manifest.iter().filter(|leaf| leaf.agent == "codex" && leaf.session.is_some()).count(),
        "unrecorded": manifest.iter().filter(|leaf| leaf.session.is_none()).count(),
        "roads": manifest.iter().map(|leaf| leaf.road).collect::<Vec<_>>(),
        "ledgerReseated": *reseated.lock().unwrap(),
        "ledgerCuts": cuts.len(),
        "doorSpawns": door.started().len(),
        "doorAnswers": woke.iter().map(|(at, answer)| format!("t{at}: {answer:?}")).collect::<Vec<_>>(),
        "synthetic": 0,
        "resumedCli": started.len() - blank,
        "blankCli": blank,
        "outsideLayout": outside,
        "agentsStarted": started.len(),
        "bound": bound.iter().filter(|kept| **kept).count(),
        "seatedBeforeWords": seated_first,
        "diedNow": died_now,
        "unrecordedTold": deaths_of(&w5.1).len(),
        "duplicateSessions": sessions.len() - distinct.len(),
        "finishedRevived": usize::from(finished_revived),
        "nudgedPerRow": nudged,
        "gates": rows.gates.len(),
        "order": order,
        "timeline": timeline.lines(),
    });
    eprintln!("t-7812 F tally: {tally}");

    // Reports, asked last — a report ends a task: every worker that came
    // back reports done from its own pane; a person's tab reports for
    // nobody.
    let report_from = |term: u32| {
        let seat = crate::agent_teams::teams().iter().find_map(|(id, team)| {
            team.panes()
                .find(|pane| pane.term == term)
                .map(|pane| (id.clone(), pane.id.clone()))
        });
        seat.map_or(-1, |(team, pane)| {
            let capability = crate::agent_teams::current_pane_capability(&team, &pane)
                .unwrap_or_else(|| TEST_CAPABILITY.to_string());
            super::restore_door::done_from(&*host, &team, &pane, &capability)
        })
    };
    let accepted: Vec<i32> = [0usize, 1, 2, 3]
        .iter()
        .map(|&at| term_of(at).map_or(-1, |(term, _)| report_from(term)))
        .collect();
    let refused: Vec<i32> = [5usize, 6, 7]
        .iter()
        .map(|&at| term_of(at).map_or(0, |(term, _)| report_from(term)))
        .collect();
    eprintln!("t-7812 F reports: accepted {accepted:?} refused {refused:?}");

    assert_eq!(tally["resumedCli"], 7, "{tally}");
    assert_eq!(tally["blankCli"], 0, "{tally}");
    assert_eq!(tally["outsideLayout"], 0, "{tally}");
    assert_eq!(tally["ledgerCuts"], 3, "{tally}");
    assert_eq!(tally["doorSpawns"], 4, "{tally}");
    assert_eq!(tally["bound"], 4, "{tally}");
    assert_eq!(
        tally["seatedBeforeWords"],
        serde_json::json!([true, true, true]),
        "a pane ran or was typed at before its seat: {tally}"
    );
    assert_eq!(tally["unrecordedTold"], 1, "{tally}");
    assert_eq!(tally["diedNow"], 1, "{tally}");
    assert_eq!(tally["duplicateSessions"], 0, "{tally}");
    assert_eq!(tally["finishedRevived"], 0, "{tally}");
    // W1 at rest, W2 mid-turn, W3 with its gate cut, W4 at rest: two
    // continuations; the persons' tabs none.
    assert_eq!(
        tally["nudgedPerRow"],
        serde_json::json!([0, 1, 1, 0, null, 0, 0, 0]),
        "{tally}"
    );
    assert!(
        accepted.iter().all(|code| *code == 0),
        "a worker that came back could not report done: {accepted:?}"
    );
    assert!(
        refused.iter().all(|code| *code != 0),
        "a person's tab reported for a worker: {refused:?}"
    );
    for (term, _, _) in &started {
        crate::agent_teams::forget_term(*term);
    }
    crate::agent_teams::forget_term(NEW_LEADER);
}
