//! t-7812 r2: the window's own resume road — `wake_conversation`, the whole
//! of every door's `resume_session` but the terminal itself — driven through
//! a fake launcher on a private window, beside the ledger's own reseat. A CLI
//! these tests count is one a real entry point started: the door's spawn, or
//! the ledger's cut through `Restoring`. Nothing is seated by hand.

use super::restore::{
    Restoring, a_run, a_worker, census_without_commands, deaths_of, row, the_window_goes,
};
use super::*;
use crate::cmd::board::{ResumeCommand, WakeWindow, wake_conversation};
use crate::conversation_wake::ConversationWake;
use crate::restart_nudge_runtime::{PendingNudge, PendingNudges, WakeReceipts};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::Receiver;
use std::time::Duration;
use zerocode_pty::DeliveryOutcome;

/// The receipt window a door's watch runs on here: the product's two windows
/// of `RESUME_NUDGE_RECEIPT_MS`, shortened so a test waits both out at once.
const WATCH: Duration = Duration::from_millis(25);

/// One thing the fake launcher did, in the order it did it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Did {
    /// A launch was built and its seat — its team — opened.
    Prepared(u32),
    /// A prepared launch's seat was taken back: nothing started.
    Abandoned(u32),
    /// A process started with this command line; `seated` is the worker the
    /// ledger held in the pane's seat at that moment, if any.
    Started {
        term: u32,
        command: String,
        seated: Option<String>,
    },
    /// The launcher refused a spawn: nothing started.
    Refused(u32),
    /// Words typed at a composer, and whether they reached it.
    Typed {
        term: u32,
        words: String,
        reached: bool,
    },
    /// A line in the window's log.
    Noted(String),
}

/// The team a door's pane opens, as `open_team` opens one for a resume.
pub(super) fn door_team(term: u32) -> String {
    format!("team-t7812-door-{term}")
}

/// The worker the ledger holds live in `team`'s leader pane — the seat a
/// door's pane is — if any. A window that cannot read its ledger names
/// nobody there: a process a door starts in it is still recorded, so the
/// test's own count says a door started one (t-7538, astra R3-1).
fn seated_in(team: &str) -> Option<String> {
    let held = super::super::runtime()?;
    let image = held.actor.view().ok()?;
    image
        .projection()
        .workers
        .iter()
        .find(|worker| {
            worker.team == team
                && worker.pane == zerocode_core::agent_teams::LEADER_PANE
                && worker.state.is_live()
        })
        .map(|worker| worker.id.clone())
}

/// A fake launcher and composer under the product's wake road: it opens the
/// seat `open_team` would, starts nothing but a line in its book, answers
/// each composer delivery as a test told it to, and keeps the pending
/// receipts in its own table for the product's own watch to walk.
pub(super) struct Door {
    checkout: std::path::PathBuf,
    pub(super) did: Mutex<Vec<Did>>,
    refuse_start: Mutex<HashSet<u32>>,
    seatless: Mutex<HashSet<u32>>,
    answers: Mutex<HashMap<u32, VecDeque<DeliveryOutcome>>>,
    launches: Mutex<HashMap<u32, u64>>,
    live: Mutex<HashMap<zerocode_core::ConversationKey, u32>>,
    agents: Mutex<HashMap<u32, &'static str>>,
    rows: Mutex<PendingNudges>,
    armed: Mutex<Vec<(u32, Option<Receiver<DeliveryOutcome>>)>>,
    /// Run once inside the next launch's build, after its seat opens and
    /// before the witness is asked — a road arriving while a door wakes.
    pub(super) during_prepare: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Where this door's events are written in order, beside the other
    /// roads' (t-7812 F).
    pub(super) timeline: Mutex<Option<std::sync::Arc<super::restore::Timeline>>>,
    /// A program an account switch's close left behind is still there, as
    /// this window's look sees it (t-7538).
    pub(super) lingering: Mutex<bool>,
}

impl Door {
    pub(super) fn new(checkout: &std::path::Path) -> Self {
        Self {
            checkout: checkout.to_path_buf(),
            did: Mutex::new(Vec::new()),
            refuse_start: Mutex::new(HashSet::new()),
            seatless: Mutex::new(HashSet::new()),
            answers: Mutex::new(HashMap::new()),
            launches: Mutex::new(HashMap::new()),
            live: Mutex::new(HashMap::new()),
            agents: Mutex::new(HashMap::new()),
            rows: Mutex::new(PendingNudges::default()),
            armed: Mutex::new(Vec::new()),
            during_prepare: Mutex::new(None),
            timeline: Mutex::new(None),
            lingering: Mutex::new(false),
        }
    }

    /// Write `did` into this door's book, and into the timeline if one is
    /// kept.
    fn saw(&self, did: Did) {
        if let Some(timeline) = self.timeline.lock().unwrap().as_ref() {
            timeline.mark(match &did {
                Did::Prepared(term) => format!("seat t{term}"),
                Did::Abandoned(term) => format!("abandon t{term}"),
                Did::Started { term, seated, .. } => format!("spawn t{term} seated={seated:?}"),
                Did::Refused(term) => format!("refused t{term}"),
                Did::Typed { term, reached, .. } => format!("typed t{term} reached={reached}"),
                Did::Noted(line) => format!("line {line}"),
            });
        }
        self.did.lock().unwrap().push(did);
    }

    /// Wake `agent`'s conversation `session` into `term`, the way
    /// `resume_session` does.
    pub(super) fn wake(
        &self,
        term: u32,
        agent: &str,
        session: &str,
    ) -> Result<ConversationWake, String> {
        wake_conversation(
            self,
            term,
            agent,
            zerocode_core::ProviderSession {
                key: zerocode_core::SessionKey::SessionId,
                id: session.to_string(),
                transcript_path: None,
            },
            24,
            80,
        )
    }

    /// Walk every armed wake through both of its receipt windows, on the
    /// product's own watch — no `working` hook arrives unless a test sent
    /// one first.
    pub(super) fn settle(&self) {
        let armed = std::mem::take(&mut *self.armed.lock().unwrap());
        for (term, delivery) in armed {
            crate::restart_nudge_runtime::watch(self, term, delivery, WATCH);
        }
    }

    /// The launcher refuses the spawn in `term`.
    pub(super) fn refuse_start(&self, term: u32) {
        self.refuse_start.lock().unwrap().insert(term);
    }

    /// `term` opens no team: the window's teammate mode is off, or its
    /// bridge is down.
    pub(super) fn seatless(&self, term: u32) {
        self.seatless.lock().unwrap().insert(term);
    }

    /// What the composer in `term` answers, delivery by delivery; a delivery
    /// nobody told it about is `Delivered`.
    pub(super) fn answers(&self, term: u32, said: &[DeliveryOutcome]) {
        self.answers
            .lock()
            .unwrap()
            .insert(term, said.iter().copied().collect());
    }

    /// The pane in `term` is relaunched: a different program holds it now.
    pub(super) fn relaunch(&self, term: u32) {
        *self.launches.lock().unwrap().entry(term).or_insert(0) += 1;
    }

    /// A live pane already holds `agent`'s conversation `session`.
    pub(super) fn already_live(&self, agent: &str, session: &str, term: u32) {
        let key = zerocode_core::conversation_key(
            agent,
            &zerocode_core::ProviderSession {
                key: zerocode_core::SessionKey::SessionId,
                id: session.to_string(),
                transcript_path: None,
            },
        )
        .expect("a conversation");
        self.live.lock().unwrap().insert(key, term);
    }

    /// Every process this launcher started: its term and command line.
    pub(super) fn started(&self) -> Vec<(u32, String)> {
        self.did
            .lock()
            .unwrap()
            .iter()
            .filter_map(|did| match did {
                Did::Started { term, command, .. } => Some((*term, command.clone())),
                _ => None,
            })
            .collect()
    }

    /// The worker the ledger held in `term`'s seat as its process started.
    pub(super) fn seated_at_start(&self, term: u32) -> Option<String> {
        self.did.lock().unwrap().iter().find_map(|did| match did {
            Did::Started {
                term: at, seated, ..
            } if *at == term => seated.clone(),
            _ => None,
        })
    }

    /// Words typed at `term`'s composer: every attempt, and the ones that
    /// reached it.
    pub(super) fn typed(&self, term: u32) -> (usize, Vec<String>) {
        let did = self.did.lock().unwrap();
        let asked = did
            .iter()
            .filter(|one| matches!(one, Did::Typed { term: at, .. } if *at == term))
            .count();
        let reached = did
            .iter()
            .filter_map(|one| match one {
                Did::Typed {
                    term: at,
                    words,
                    reached: true,
                } if *at == term => Some(words.clone()),
                _ => None,
            })
            .collect();
        (asked, reached)
    }

    /// The window's log lines that contain `needle`.
    pub(super) fn noted(&self, needle: &str) -> Vec<String> {
        self.did
            .lock()
            .unwrap()
            .iter()
            .filter_map(|did| match did {
                Did::Noted(line) if line.contains(needle) => Some(line.clone()),
                _ => None,
            })
            .collect()
    }

    fn answer(&self, term: u32, words: &str) -> Receiver<DeliveryOutcome> {
        let outcome = self
            .answers
            .lock()
            .unwrap()
            .get_mut(&term)
            .and_then(VecDeque::pop_front)
            .unwrap_or(DeliveryOutcome::Delivered);
        self.saw(Did::Typed {
            term,
            words: words.to_string(),
            reached: outcome.pasted(),
        });
        let (said, heard) = std::sync::mpsc::sync_channel(1);
        said.send(outcome).expect("the answer");
        heard
    }
}

impl WakeWindow for Door {
    type Launch = String;
    type Channel = ();

    fn root(&self) -> std::path::PathBuf {
        self.checkout.clone()
    }

    fn data_root(&self) -> std::path::PathBuf {
        super::super::BLACKBOX
            .get()
            .expect("the window's data root")
            .to_path_buf()
    }

    fn standing(&self, wanted: &zerocode_core::ConversationKey) -> Option<u32> {
        self.live.lock().unwrap().get(wanted).copied()
    }

    fn launch_override(
        &self,
        _agent: &str,
    ) -> Result<Option<zerocode_core::LaunchOverride>, String> {
        Ok(None)
    }

    fn prepare(
        &self,
        term: u32,
        command: ResumeCommand,
        _session_id: &str,
    ) -> Result<String, String> {
        if !self.seatless.lock().unwrap().contains(&term) {
            seat_a_team(&door_team(term), term);
        }
        self.agents
            .lock()
            .unwrap()
            .insert(term, command.kind.slug());
        self.saw(Did::Prepared(term));
        let during = self.during_prepare.lock().unwrap().take();
        if let Some(during) = during {
            during();
        }
        Ok(command.argv.join(" "))
    }

    fn abandon(&self, term: u32) {
        crate::agent_teams::forget_term(term);
        self.saw(Did::Abandoned(term));
    }

    fn start(
        &self,
        term: u32,
        launch: String,
        _rows: u16,
        _cols: u16,
    ) -> Result<(Option<String>, ()), String> {
        if self.refuse_start.lock().unwrap().remove(&term) {
            crate::agent_teams::forget_term(term);
            self.saw(Did::Refused(term));
            return Err("the fake launcher refused this spawn".to_string());
        }
        let seated = seated_in(&door_team(term));
        self.saw(Did::Started {
            term,
            command: launch,
            seated,
        });
        Ok((None, ()))
    }

    fn record(&self, term: u32, session: zerocode_core::ProviderSession) {
        let agent = self.agents.lock().unwrap().get(&term).copied();
        if let Some(key) = agent.and_then(|agent| zerocode_core::conversation_key(agent, &session))
        {
            self.live.lock().unwrap().insert(key, term);
        }
    }

    fn attach(&self, _term: u32, _channel: ()) {}

    fn launch_of(&self, term: u32) -> Option<u64> {
        Some(
            (u64::from(term) << 16)
                + self
                    .launches
                    .lock()
                    .unwrap()
                    .get(&term)
                    .copied()
                    .unwrap_or(0),
        )
    }

    fn type_words(
        &self,
        term: u32,
        _agent: &str,
        words: &str,
    ) -> Option<Receiver<DeliveryOutcome>> {
        Some(self.answer(term, words))
    }

    fn arm(&self, term: u32, pending: PendingNudge, delivery: Option<Receiver<DeliveryOutcome>>) {
        self.rows.lock().unwrap().register(term, pending);
        self.armed.lock().unwrap().push((term, delivery));
    }

    fn note(&self, line: &str) {
        self.saw(Did::Noted(line.to_string()));
    }

    fn stir(&self) {}

    fn exit_seen(&self, _witness: &crate::agent_teams::ExitWitness) -> bool {
        !*self.lingering.lock().unwrap()
    }
}

impl WakeReceipts for Door {
    fn rows(&self) -> std::sync::MutexGuard<'_, PendingNudges> {
        self.rows.lock().unwrap()
    }

    fn launch(&self, term: u32) -> Option<u64> {
        WakeWindow::launch_of(self, term)
    }

    fn type_again(&self, term: u32, _agent: &str, text: &str) -> Option<Receiver<DeliveryOutcome>> {
        Some(self.answer(term, text))
    }

    fn note(&self, line: &str) {
        self.saw(Did::Noted(line.to_string()));
    }
}

/// The ledger's store refuses every write while this stands — the fault a
/// real disk gives, injected through the store's own fault door the way the
/// orchestrator's tests inject it — so the actor answers `NotDurable` and
/// rewinds to the disk's word after.
pub(super) struct LedgerRefuses(rusqlite::Connection);

impl LedgerRefuses {
    pub(super) fn on(store: &zerocode_orchestrator::workflow_store::WorkflowStore) -> Self {
        let held = store.fault_connection_for_tests().expect("the store");
        held.execute_batch(
            "CREATE TRIGGER t7812_ledger_refuses
                AFTER UPDATE OF revision ON orchestration_ledger_heads
                BEGIN SELECT RAISE(ABORT, 't-7812: the ledger refuses this write'); END;",
        )
        .expect("the fault");
        Self(held)
    }
}

impl Drop for LedgerRefuses {
    fn drop(&mut self) {
        self.0
            .execute_batch("DROP TRIGGER t7812_ledger_refuses;")
            .expect("the fault lifted");
    }
}

/// What the goodbye still owes `worker`: its note, read and not spent.
pub(super) fn owed(worker: &str) -> restart_census::Cut {
    restart_census::peek_cut(
        super::super::BLACKBOX
            .get()
            .expect("the window's data root"),
        worker,
    )
}

/// A verb from `team`'s leader pane, answered with its exit code.
pub(super) fn done_from(host: &dyn Host, team: &str, pane: &str, capability: &str) -> i32 {
    run(
        host,
        Vec::new(),
        team,
        pane,
        capability,
        &words("send --type worker_done --body {\"ok\":true,\"summary\":\"landed\"}"),
        clock(),
    )
    .exit_code
}

/// The next window's boot: its sweep, every seat vacated, the live asleep.
pub(super) fn the_next_boot(old_leader: u32) {
    crate::agent_teams::forget_term(old_leader);
    super::super::runtime()
        .expect("the private runtime")
        .actor
        .window_restarted(clock())
        .expect("the next boot's sweep");
}

/// The coordinator back in a new leader pane — its conversation the one
/// `actor_leader`'s pane held — asking its run's sleepers back. No sweep:
/// the boot's own is [`the_next_boot`], and a second one would put to sleep
/// what a door already brought back.
pub(super) fn coordinator_back(
    host: &dyn Host,
    name: &str,
    actor_leader: u32,
    new_leader: u32,
) -> usize {
    seat_a_team(&format!("team-t7812-{name}-back-{new_leader}"), new_leader);
    super::super::reseat_sleeping(
        host,
        Vec::new(),
        new_leader,
        Some(&test_actor(actor_leader)),
    )
}

/// A provider session reported for the worker in `term`, as its hook would.
fn report(term: u32, session: zerocode_core::ProviderSession) {
    let held = super::super::runtime().expect("the private runtime");
    assert!(
        held.actor
            .worker_session_reported(term, session, clock())
            .expect("the session lands")
            .0,
        "the session was not written"
    );
}

/// t-7812 R1: a sleeper's conversation is seated before anything of its
/// pane runs, and a seat the ledger does not write starts nothing. Two
/// refusals — a pane with no seat to write (teammate mode off), and a ledger
/// that refuses the write (`NotDurable`) — each leave no CLI, no words, no
/// `opened`, one line saying why, the seat taken back and the sleeper asleep
/// with its words still owed. Before, `resume_session` ran the CLI first
/// (Claude's argv already carrying the continuation), ignored the refused
/// seat, and answered `opened`: a live conversation whose `worker_done` the
/// ledger could not hear. Once the ledger writes again, the same door brings
/// it back — seated before its process starts, told once, reporting done.
#[test]
fn a_sleeper_the_ledger_will_not_seat_starts_nothing_and_comes_back_once_it_can() {
    const OLD_LEADER: u32 = 278_400;
    const WORKER: u32 = 278_401;
    const SEATLESS: u32 = 278_406;
    const REFUSED: u32 = 278_407;
    const DOOR: u32 = 278_408;
    let (_window, store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(OLD_LEADER)));
    let (team, _run) = a_run(&host, OLD_LEADER, "unseated");
    let session = "7812b0c1-0000-4000-8000-000000000001";
    let (_task, worker, term) = a_worker(
        &host,
        &team,
        "--agent claude --model claude-fable-5-1 --effort xhigh",
        Some(session),
    );
    super::super::pane_turn_began(term, clock());
    the_window_goes(&census_without_commands, &[term]);
    the_next_boot(OLD_LEADER);
    assert_eq!(row(&worker).state, WorkerState::Sleeping);
    let door = Door::new(checkout.path());

    door.seatless(SEATLESS);
    let no_seat = door.wake(SEATLESS, "claude", session);
    let refused = {
        let _refusing = LedgerRefuses::on(&store);
        door.wake(REFUSED, "claude", session)
    };
    // Both refusals are judged before anything fails, so a run of the old
    // road says what each of them did.
    let mut wrong = Vec::new();
    for (shape, answer, at) in [
        ("no seat", &no_seat, SEATLESS),
        ("store refused", &refused, REFUSED),
    ] {
        if answer.is_ok() {
            wrong.push(format!(
                "{shape}: a sleeper's pane the ledger never seated was opened: {answer:?}"
            ));
        }
        if door.started().iter().any(|(term, _)| *term == at) {
            wrong.push(format!("{shape}: a CLI was started in t{at}"));
        }
        if !door.did.lock().unwrap().contains(&Did::Abandoned(at)) {
            wrong.push(format!("{shape}: the seat was left standing"));
        }
        if door.typed(at).0 != 0 {
            wrong.push(format!("{shape}: words were typed at the composer"));
        }
        if crate::agent_teams::teams()
            .values()
            .any(|held| held.leader_term == at)
        {
            wrong.push(format!("{shape}: the pane's team outlived it"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
    assert!(
        door.started().is_empty(),
        "a CLI was started for a sleeper the ledger did not seat: {:?}",
        door.started()
    );
    assert_eq!(
        door.noted("could not be seated").len(),
        2,
        "one line for each refusal: {:?}",
        door.did.lock().unwrap()
    );
    assert_eq!(row(&worker).state, WorkerState::Sleeping);
    assert!(owed(&worker).turn, "the refused wake spent the words");
    assert!(deaths_of(&worker).is_empty());
    assert_ne!(
        done_from(
            &host,
            &door_team(REFUSED),
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY
        ),
        0,
        "the refused pane's seat reported for the worker"
    );

    // The ledger writes again: the same road brings it back, once.
    let back = door.wake(DOOR, "claude", session).expect("the wake");
    assert_eq!(back, ConversationWake::opened(DOOR));
    let started = door.started();
    assert_eq!(started.len(), 1, "{started:?}");
    let command = &started[0].1;
    for said in [
        "--resume",
        session,
        "--model claude-fable-5-1",
        "--effort xhigh",
        "--name",
    ] {
        assert!(command.contains(said), "without `{said}`: {command}");
    }
    assert_eq!(
        command.matches(crate::RESTART_NUDGE).count(),
        1,
        "the cut turn is continued once, on the argv: {command}"
    );
    assert_eq!(
        door.seated_at_start(DOOR).as_deref(),
        Some(worker.as_str()),
        "the process started before its seat was durable"
    );
    door.settle();
    assert_eq!(
        door.typed(DOOR).0,
        0,
        "the argv's words were typed at the composer too"
    );
    let now = row(&worker);
    assert_eq!(now.state, WorkerState::Active);
    assert_eq!(now.team, door_team(DOOR));
    assert!(!owed(&worker).any(), "the words were left to be said again");
    assert_eq!(
        done_from(
            &host,
            &door_team(DOOR),
            zerocode_core::agent_teams::LEADER_PANE,
            TEST_CAPABILITY
        ),
        0,
        "the resumed pane cannot report for its worker"
    );
    assert!(deaths_of(&worker).is_empty());
    crate::agent_teams::forget_term(DOOR);
}

/// t-7812 R1 and R2b: a spawn that starts nothing — the host refused it —
/// puts the sleeper its door had seated back to sleep, and leaves its words
/// owed. The next road that asks brings it back and tells it once: here the
/// ledger's own reseat, when the coordinator returns. Before, the census was
/// spent before the spawn, and the retry came back with nothing to say.
#[test]
fn a_door_whose_spawn_starts_nothing_puts_the_sleeper_back_with_its_words_owed() {
    const OLD_LEADER: u32 = 278_420;
    const WORKER: u32 = 278_421;
    const DOOR: u32 = 278_428;
    const NEW_LEADER: u32 = 278_430;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "unstarted");
    let session = "session-t7812-unstarted";
    let (_task, worker, term) = a_worker(&host, &team, "--agent codex", Some(session));
    super::super::pane_turn_began(term, clock());
    the_window_goes(&census_without_commands, &[term]);
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();
    let door = Door::new(checkout.path());
    door.refuse_start(DOOR);

    assert!(door.wake(DOOR, "codex", session).is_err());
    assert!(door.started().is_empty());
    assert!(door.did.lock().unwrap().contains(&Did::Refused(DOOR)));
    assert_eq!(door.typed(DOOR).0, 0);
    assert_eq!(
        row(&worker).state,
        WorkerState::Sleeping,
        "a sleeper whose pane never started was left seated in it"
    );
    assert!(owed(&worker).turn, "the unstarted wake spent the words");
    assert!(deaths_of(&worker).is_empty());

    assert_eq!(
        coordinator_back(&host, "unstarted", OLD_LEADER, NEW_LEADER),
        1
    );
    let cuts = host.cuts();
    assert_eq!(cuts.len(), 1, "{cuts:?}");
    let told = host.typed_at(cuts[0].0);
    assert_eq!(told.len(), 1, "the retry was told {told:?}");
    assert!(told[0].starts_with(crate::RESTART_NUDGE), "{told:?}");
    assert_eq!(row(&worker).state, WorkerState::Active);
    assert!(!owed(&worker).any(), "said once, and still owed");
    assert!(deaths_of(&worker).is_empty());
    crate::agent_teams::forget_term(cuts[0].0);
    crate::agent_teams::forget_term(NEW_LEADER);
}

/// t-7812 R2a: a continuation that reached the pane is not said again when
/// the pane's `working` hook comes late or never — on either road: Claude's
/// words ride its argv and nothing types them, Codex's are typed once at the
/// composer and its delivery says they went. Both windows close on nothing
/// and the line reads `receipt=none`. Before, the first window's silence
/// typed the same words at the composer again: a Claude pane heard its
/// continuation twice, and a Codex pane whose hook was late did too.
#[test]
fn a_continuation_that_reached_the_pane_is_not_said_again_when_its_hook_never_comes() {
    const OLD_LEADER: u32 = 278_440;
    const WORKER: u32 = 278_441;
    const CLAUDE: u32 = 278_446;
    const CODEX: u32 = 278_447;
    const AGAIN: u32 = 278_448;
    const NEW_LEADER: u32 = 278_450;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "unheard");
    let claude_session = "7812b0c1-0000-4000-8000-000000000441";
    let codex_session = "session-t7812-unheard";
    let (_, claude, claude_term) = a_worker(&host, &team, "--agent claude", Some(claude_session));
    let (_, codex, codex_term) = a_worker(&host, &team, "--agent codex", Some(codex_session));
    super::super::pane_turn_began(claude_term, clock());
    super::super::pane_turn_began(codex_term, clock());
    the_window_goes(&census_without_commands, &[claude_term, codex_term]);
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();
    let door = Door::new(checkout.path());

    door.wake(CLAUDE, "claude", claude_session)
        .expect("claude's wake");
    door.wake(CODEX, "codex", codex_session)
        .expect("codex's wake");
    door.settle();

    let started = door.started();
    assert_eq!(started.len(), 2, "{started:?}");
    assert_eq!(
        started[0].1.matches(crate::RESTART_NUDGE).count(),
        1,
        "{started:?}"
    );
    assert_eq!(
        started[1].1.matches(crate::RESTART_NUDGE).count(),
        0,
        "codex's words rode its argv too: {started:?}"
    );
    assert_eq!(
        door.typed(CLAUDE),
        (0, Vec::new()),
        "a silent hook typed claude's argv words at its composer"
    );
    let (asked, reached) = door.typed(CODEX);
    assert_eq!(asked, 1, "a silent hook typed codex's words again");
    assert_eq!(reached.len(), 1);
    assert!(reached[0].starts_with(crate::RESTART_NUDGE), "{reached:?}");
    assert_eq!(
        door.noted("receipt=none").len(),
        2,
        "{:?}",
        door.did.lock().unwrap()
    );
    for worker in [&claude, &codex] {
        assert_eq!(row(worker).state, WorkerState::Active);
        assert!(!owed(worker).any(), "{worker}'s words are still owed");
        assert!(deaths_of(worker).is_empty());
    }
    // One conversation, one process: a second door for it is sent to the
    // pane that holds it, and the ledger has nobody asleep to cut for.
    assert_eq!(
        door.wake(AGAIN, "codex", codex_session),
        Ok(ConversationWake::standing(CODEX))
    );
    assert_eq!(
        coordinator_back(&host, "unheard", OLD_LEADER, NEW_LEADER),
        0
    );
    assert!(host.cuts().is_empty(), "{:?}", host.cuts());
    assert_eq!(door.started().len(), 2);
    for term in [CLAUDE, CODEX, NEW_LEADER] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-7812 R2a, the boundaries of the one fallback: words that never left a
/// composer that was not ready are placed once more, and the goodbye's word
/// is spent when they land; words a person's draft kept off the line, and
/// words whose pane was relaunched before the fallback came due, are not
/// typed again — and, never sent, they stay owed.
#[test]
fn only_words_that_never_left_are_placed_again_and_a_persons_line_is_left_alone() {
    const OLD_LEADER: u32 = 278_460;
    const WORKER: u32 = 278_461;
    const UNREADY: u32 = 278_466;
    const DRAFT: u32 = 278_467;
    const RELAUNCHED: u32 = 278_468;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(OLD_LEADER)));
    let (team, _run) = a_run(&host, OLD_LEADER, "fallback");
    let sessions = [
        "session-t7812-unready",
        "session-t7812-draft",
        "session-t7812-relaunched",
    ];
    let workers: Vec<(String, u32)> = sessions
        .iter()
        .map(|session| {
            let (_, worker, term) = a_worker(&host, &team, "--agent codex", Some(session));
            super::super::pane_turn_began(term, clock());
            (worker, term)
        })
        .collect();
    let terms: Vec<u32> = workers.iter().map(|(_, term)| *term).collect();
    the_window_goes(&census_without_commands, &terms);
    the_next_boot(OLD_LEADER);
    let door = Door::new(checkout.path());
    door.answers(
        UNREADY,
        &[DeliveryOutcome::TimedOut, DeliveryOutcome::Delivered],
    );
    door.answers(
        DRAFT,
        &[DeliveryOutcome::Refused(
            zerocode_pty::ready::Refusal::HoldsADraft,
        )],
    );
    door.answers(RELAUNCHED, &[DeliveryOutcome::TimedOut]);
    for (at, session) in [UNREADY, DRAFT, RELAUNCHED].into_iter().zip(sessions) {
        door.wake(at, "codex", session).expect("the wake");
    }
    door.relaunch(RELAUNCHED);
    door.settle();

    let (asked, reached) = door.typed(UNREADY);
    assert_eq!(
        (asked, reached.len()),
        (2, 1),
        "the unready composer: {asked} asked"
    );
    assert!(!owed(&workers[0].0).any(), "landed, and still owed");
    for (at, (worker, _), shape) in [
        (DRAFT, &workers[1], "a person's draft"),
        (RELAUNCHED, &workers[2], "a relaunched pane"),
    ] {
        assert_eq!(
            door.typed(at),
            (1, Vec::new()),
            "{shape}: the words were typed again"
        );
        assert!(owed(worker).turn, "{shape}: words never sent were spent");
    }
    for term in [UNREADY, DRAFT, RELAUNCHED] {
        crate::agent_teams::forget_term(term);
    }
}

/// t-7812 R2b, across a window: words no road placed — the door's seat
/// refused, the ledger's pane refused — are still owed when the window goes
/// again with that worker asleep, and the next window's reseat says them,
/// once. The second goodbye reads only the worker that came back; what the
/// first one cut of the sleeper rides on in its note, and the next process
/// has only the disk.
///
/// The windows here share one test process, and so one host epoch: the
/// journal lets the ledger cut a worker's pane once per epoch (a replayed
/// cut answers "already carried out"), so the worker that came back by the
/// ledger in the second window comes back by a door in the third.
#[test]
fn a_continuation_no_road_placed_goes_on_to_the_next_window() {
    const OLD_LEADER: u32 = 278_480;
    const WORKER: u32 = 278_481;
    const DOOR: u32 = 278_488;
    const IDLE_DOOR: u32 = 278_489;
    const SECOND_LEADER: u32 = 278_490;
    const THIRD_LEADER: u32 = 278_495;
    let (_window, store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (SECOND_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "owed");
    let owed_session = "session-t7812-owed";
    let idle_session = "session-t7812-idle";
    let (_, cut, cut_term) = a_worker(&host, &team, "--agent codex", Some(owed_session));
    let (_, idle, idle_term) = a_worker(&host, &team, "--agent codex", Some(idle_session));
    super::super::pane_turn_began(cut_term, clock());
    the_window_goes(&census_without_commands, &[cut_term, idle_term]);
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();

    // The second window: a door for the cut worker meets a ledger that will
    // not write, and the coordinator's reseat is refused its pane for it.
    let door = Door::new(checkout.path());
    {
        let _refusing = LedgerRefuses::on(&store);
        assert!(door.wake(DOOR, "codex", owed_session).is_err());
    }
    host.refuse_splits(1);
    assert_eq!(
        coordinator_back(&host, "owed", OLD_LEADER, SECOND_LEADER),
        1,
        "only the idle worker came back"
    );
    assert_eq!(row(&cut).state, WorkerState::Sleeping);
    assert_eq!(row(&idle).state, WorkerState::Active);
    assert!(owed(&cut).turn);
    let second = host.cuts();
    assert_eq!(second.len(), 1, "{second:?}");
    assert!(
        host.typed_at(second[0].0).is_empty(),
        "the idle worker was told to go on"
    );

    // The second goodbye reads the idle worker, at rest; the next process
    // has only the disk.
    the_window_goes(&census_without_commands, &[second[0].0]);
    restart_census::a_new_process(super::super::BLACKBOX.get().expect("the data root"));
    the_next_boot(SECOND_LEADER);
    host.cut.lock().unwrap().clear();
    host.typed.lock().unwrap().clear();
    let third_door = Door::new(checkout.path());
    assert_eq!(
        third_door.wake(IDLE_DOOR, "codex", idle_session),
        Ok(ConversationWake::opened(IDLE_DOOR))
    );
    assert_eq!(
        coordinator_back(&host, "owed-again", OLD_LEADER, THIRD_LEADER),
        1
    );
    let third = host.cuts();
    assert_eq!(third.len(), 1, "{third:?}");
    assert!(third[0].1.contains(owed_session), "{third:?}");
    let said = host.typed_at(third[0].0);
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].starts_with(crate::RESTART_NUDGE), "{said:?}");
    third_door.settle();
    assert_eq!(
        third_door.typed(IDLE_DOOR),
        (0, Vec::new()),
        "the idle worker was told to go on"
    );
    assert_eq!(door.typed(DOOR).0, 0, "the refused door typed");
    for worker in [&cut, &idle] {
        assert_eq!(row(worker).state, WorkerState::Active, "{worker}");
        assert!(deaths_of(worker).is_empty(), "{worker}");
    }
    assert!(!owed(&cut).any());
    crate::agent_teams::forget_term(third[0].0);
    crate::agent_teams::forget_term(IDLE_DOOR);
    crate::agent_teams::forget_term(THIRD_LEADER);
}

/// t-7812 E, the coordinator's negative (brief supplement 5): a worker
/// stopped at a question for the person when the window went is not told to
/// go on, on either road — the question was the person's to answer. A worker
/// whose turn was under way beside it still is. Before, the census wrote a
/// parked question down as a turn under way, and its wake typed the
/// continuation into the person's question.
#[test]
fn a_worker_waiting_on_the_person_is_not_told_to_go_on() {
    const OLD_LEADER: u32 = 278_500;
    const WORKER: u32 = 278_501;
    const DOOR: u32 = 278_508;
    const NEW_LEADER: u32 = 278_510;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "asking");
    let asking_session = "7812b0c1-0000-4000-8000-000000000501";
    let (_, asking_by_door, asking_door_term) =
        a_worker(&host, &team, "--agent claude", Some(asking_session));
    let (_, asking, asking_term) =
        a_worker(&host, &team, "--agent codex", Some("session-t7812-asking"));
    let (_, working, working_term) =
        a_worker(&host, &team, "--agent codex", Some("session-t7812-working"));
    for term in [asking_door_term, asking_term] {
        super::super::pane_turn_began(term, clock());
        super::super::pane_attention_noted(term, Some(clock()), clock());
    }
    super::super::pane_turn_began(working_term, clock());
    the_window_goes(
        &census_without_commands,
        &[asking_door_term, asking_term, working_term],
    );
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();
    host.typed.lock().unwrap().clear();

    let door = Door::new(checkout.path());
    door.wake(DOOR, "claude", asking_session).expect("the wake");
    door.settle();
    let started = door.started();
    assert_eq!(started.len(), 1);
    assert!(
        !started[0].1.contains(crate::RESTART_NUDGE),
        "a worker waiting on the person was told to go on: {}",
        started[0].1
    );
    assert_eq!(door.typed(DOOR).0, 0);
    assert_eq!(row(&asking_by_door).state, WorkerState::Active);

    assert_eq!(coordinator_back(&host, "asking", OLD_LEADER, NEW_LEADER), 2);
    let told = |session: &str| {
        let cut = host
            .cuts()
            .into_iter()
            .find(|(_, command)| command.contains(session))
            .expect("the pane");
        host.typed_at(cut.0)
    };
    assert!(
        told("session-t7812-asking").is_empty(),
        "a worker waiting on the person was typed at"
    );
    let went_on = told("session-t7812-working");
    assert_eq!(went_on.len(), 1, "{went_on:?}");
    assert!(went_on[0].starts_with(crate::RESTART_NUDGE));
    for worker in [&asking_by_door, &asking, &working] {
        assert_eq!(row(worker).state, WorkerState::Active);
        assert!(!owed(worker).any());
    }
    for (term, _) in host.cuts() {
        crate::agent_teams::forget_term(term);
    }
    crate::agent_teams::forget_term(DOOR);
    crate::agent_teams::forget_term(NEW_LEADER);
}

/// t-7812 C's other failures (the report left them unproved): a sleeper
/// whose recorded conversation file is not on disk, and one whose
/// conversation this window has no resume for, are told once with why and
/// nothing is started in their place — through the door as through the
/// ledger. Before, the door resumed a file that was not there (a pane that
/// dies a second later) and the ledger started the agent fresh with the task
/// preamble: a new conversation where the work was.
#[test]
fn a_sleeper_whose_conversation_cannot_come_back_is_told_once_and_nothing_starts() {
    const OLD_LEADER: u32 = 278_520;
    const WORKER: u32 = 278_521;
    const DOOR: u32 = 278_528;
    const NEW_LEADER: u32 = 278_530;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(
        checkout.path(),
        WORKER,
        (NEW_LEADER, test_actor(OLD_LEADER)),
    );
    let (team, _run) = a_run(&host, OLD_LEADER, "gone");
    let gone = |name: &str| {
        checkout
            .path()
            .join(format!("{name}.jsonl"))
            .to_string_lossy()
            .into_owned()
    };
    let door_session = "7812b0c1-0000-4000-8000-000000000521";
    let ledger_session = "7812b0c1-0000-4000-8000-000000000522";
    let (_, by_door, door_term) = a_worker(&host, &team, "--agent claude", None);
    report(
        door_term,
        zerocode_core::ProviderSession {
            key: zerocode_core::SessionKey::SessionId,
            id: door_session.to_string(),
            transcript_path: Some(gone(door_session)),
        },
    );
    let (_, by_ledger, ledger_term) = a_worker(&host, &team, "--agent claude", None);
    report(
        ledger_term,
        zerocode_core::ProviderSession {
            key: zerocode_core::SessionKey::SessionId,
            id: ledger_session.to_string(),
            transcript_path: Some(gone(ledger_session)),
        },
    );
    let (_, unsupported, unsupported_term) = a_worker(&host, &team, "--agent claude", None);
    report(
        unsupported_term,
        zerocode_core::ProviderSession {
            key: zerocode_core::SessionKey::ConversationId,
            id: "7812b0c1-0000-4000-8000-000000000523".to_string(),
            transcript_path: None,
        },
    );
    the_window_goes(
        &census_without_commands,
        &[door_term, ledger_term, unsupported_term],
    );
    the_next_boot(OLD_LEADER);
    host.cut.lock().unwrap().clear();

    let door = Door::new(checkout.path());
    assert!(door.wake(DOOR, "claude", door_session).is_err());
    assert!(door.started().is_empty(), "{:?}", door.started());
    assert!(
        !door.did.lock().unwrap().contains(&Did::Prepared(DOOR)),
        "a launch was built for a conversation that cannot come back"
    );
    assert_eq!(coordinator_back(&host, "gone", OLD_LEADER, NEW_LEADER), 0);
    assert!(
        host.cuts().is_empty(),
        "a CLI was started where the conversation could not come back: {:?}",
        host.cuts()
    );
    for (worker, why) in [
        (
            &by_door,
            zerocode_core::orchestration::CONVERSATION_FILE_GONE,
        ),
        (
            &by_ledger,
            zerocode_core::orchestration::CONVERSATION_FILE_GONE,
        ),
        (
            &unsupported,
            zerocode_core::orchestration::RESUME_UNSUPPORTED,
        ),
    ] {
        let told = deaths_of(worker);
        assert_eq!(told.len(), 1, "{worker}: {told:?}");
        assert_eq!(told[0]["reason"], why, "{worker}");
        assert!(!row(worker).state.is_live(), "{worker}");
    }
    let _old = BootedHere::at(clock() - zerocode_core::orchestration::RESEAT_GRACE_MS - 1);
    super::super::tick(&host, &[], clock());
    for worker in [&by_door, &by_ledger, &unsupported] {
        assert_eq!(deaths_of(worker).len(), 1, "{worker}'s loss was told twice");
    }
    crate::agent_teams::forget_term(NEW_LEADER);
}

/// The report's unproved negatives (brief supplements 2 and 3): one
/// conversation comes back once, as its own worker, whatever else names it.
/// Two tabs naming one person's conversation start one CLI; a conversation a
/// live pane holds starts none; a sleeper's session id reopened as another
/// agent's, or in another checkout, is not that sleeper and seats nobody;
/// two runs' workers in one checkout each come back as themselves; and a
/// late exit of a pane from the last window ends nobody that came back.
#[test]
fn one_conversation_comes_back_once_whatever_else_names_it() {
    const LEADER_A: u32 = 278_540;
    const WORKER_A: u32 = 278_541;
    const LEADER_B: u32 = 278_545;
    const WORKER_B: u32 = 278_546;
    const TAB: u32 = 278_550;
    const TAB_AGAIN: u32 = 278_551;
    const HELD_ELSEWHERE: u32 = 278_552;
    const LIVE: u32 = 278_553;
    const OTHER_AGENT: u32 = 278_554;
    const OTHER_CHECKOUT: u32 = 278_555;
    const DOOR_A: u32 = 278_556;
    const DOOR_B: u32 = 278_557;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the shared checkout");
    let elsewhere = tempfile::tempdir().expect("another checkout");
    let host_a = Restoring::new(checkout.path(), WORKER_A, (0, test_actor(LEADER_A)));
    let host_b = Restoring::new(checkout.path(), WORKER_B, (0, test_actor(LEADER_B)));
    let (team_a, run_a) = a_run(&host_a, LEADER_A, "same-cwd-a");
    let (team_b, run_b) = a_run(&host_b, LEADER_B, "same-cwd-b");
    let (_, worker_a, term_a) = a_worker(
        &host_a,
        &team_a,
        "--agent codex",
        Some("session-t7812-run-a"),
    );
    let (_, worker_b, term_b) = a_worker(
        &host_b,
        &team_b,
        "--agent codex",
        Some("session-t7812-run-b"),
    );
    assert_ne!(run_a, run_b);
    the_window_goes(&census_without_commands, &[term_a, term_b]);
    the_next_boot(LEADER_A);
    crate::agent_teams::forget_term(LEADER_B);
    let door = Door::new(checkout.path());

    // Two tabs, one person's conversation: one CLI.
    let person = "7812b0c1-0000-4000-8000-000000000540";
    assert_eq!(
        door.wake(TAB, "claude", person),
        Ok(ConversationWake::opened(TAB))
    );
    assert_eq!(
        door.wake(TAB_AGAIN, "claude", person),
        Ok(ConversationWake::standing(TAB))
    );
    // A conversation a live pane holds: none.
    door.already_live("claude", "7812b0c1-0000-4000-8000-000000000541", LIVE);
    assert_eq!(
        door.wake(
            HELD_ELSEWHERE,
            "claude",
            "7812b0c1-0000-4000-8000-000000000541"
        ),
        Ok(ConversationWake::standing(LIVE))
    );
    // Run A's sleeper's id, reopened as another agent's, or from another
    // checkout: a person's conversation, and the sleeper is untouched.
    door.wake(OTHER_AGENT, "claude", "session-t7812-run-a")
        .expect("another agent's conversation");
    Door::new(elsewhere.path())
        .wake(OTHER_CHECKOUT, "codex", "session-t7812-run-a")
        .expect("another checkout's conversation");
    assert_eq!(row(&worker_a).state, WorkerState::Sleeping);
    assert_eq!(door.seated_at_start(OTHER_AGENT), None);
    assert!(
        !door
            .started()
            .iter()
            .any(|(_, command)| command.contains(crate::RESTART_NUDGE)),
        "a person's conversation was told to go on"
    );

    // Two runs, one checkout: each door seats its own run's worker.
    door.wake(DOOR_A, "codex", "session-t7812-run-a")
        .expect("run A's worker");
    door.wake(DOOR_B, "codex", "session-t7812-run-b")
        .expect("run B's worker");
    let (a, b) = (row(&worker_a), row(&worker_b));
    assert_eq!(
        (a.state, a.team.as_str()),
        (WorkerState::Active, door_team(DOOR_A).as_str())
    );
    assert_eq!(
        (b.state, b.team.as_str()),
        (WorkerState::Active, door_team(DOOR_B).as_str())
    );
    assert_eq!(
        (a.run.as_str(), b.run.as_str()),
        (run_a.as_str(), run_b.as_str())
    );
    // The last window's pane exits late: nobody who came back ends.
    super::super::terminal_gone(term_a, clock());
    super::super::terminal_gone(term_b, clock());
    assert_eq!(row(&worker_a).state, WorkerState::Active);
    assert_eq!(row(&worker_b).state, WorkerState::Active);
    assert!(deaths_of(&worker_a).is_empty() && deaths_of(&worker_b).is_empty());

    let started = door.started();
    assert_eq!(
        started.iter().map(|(term, _)| *term).collect::<Vec<_>>(),
        vec![TAB, OTHER_AGENT, DOOR_A, DOOR_B],
        "{started:?}"
    );
    for term in [TAB, OTHER_AGENT, OTHER_CHECKOUT, DOOR_A, DOOR_B] {
        crate::agent_teams::forget_term(term);
    }
}

/// The report's unproved grace boundary (brief supplement 4): a sleeper
/// nothing brought back is ended at the grace — not a millisecond before —
/// and told once; one a door brought back inside the grace, however late,
/// is not ended by it.
#[test]
fn a_sleeper_ends_at_the_grace_and_not_before_and_one_that_came_back_does_not() {
    const OLD_LEADER: u32 = 278_560;
    const WORKER: u32 = 278_561;
    const DOOR: u32 = 278_568;
    let (_window, _store) = PrivateWindow::boot();
    let _beat = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the workers' checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(OLD_LEADER)));
    let (team, _run) = a_run(&host, OLD_LEADER, "grace");
    let (_, left, left_term) = a_worker(&host, &team, "--agent codex", Some("session-t7812-left"));
    let (_, late, late_term) = a_worker(&host, &team, "--agent codex", Some("session-t7812-late"));
    the_window_goes(&census_without_commands, &[left_term, late_term]);
    the_next_boot(OLD_LEADER);
    let booted = clock();
    let _boot = BootedHere::at(booted);
    let grace = zerocode_core::orchestration::RESEAT_GRACE_MS;

    super::super::tick(&host, &[], booted + grace - 1);
    assert_eq!(
        row(&left).state,
        WorkerState::Sleeping,
        "ended before the grace"
    );
    assert!(deaths_of(&left).is_empty());
    // A slow door, inside the grace.
    Door::new(checkout.path())
        .wake(DOOR, "codex", "session-t7812-late")
        .expect("the late wake");
    super::super::tick(&host, &[], booted + grace);
    let told = deaths_of(&left);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["reason"], zerocode_core::orchestration::NOT_RESUMED);
    assert_eq!(
        row(&late).state,
        WorkerState::Active,
        "the grace ended a worker that came back"
    );
    assert!(deaths_of(&late).is_empty());
    super::super::tick(&host, &[], booted + grace + 1);
    assert_eq!(deaths_of(&left).len(), 1, "told twice");
    crate::agent_teams::forget_term(DOOR);
}
