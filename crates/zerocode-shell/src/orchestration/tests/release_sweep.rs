//! t-94842: a finished worker is reclaimed without a person's hands.
//!
//! The release sweep closed a finished worker's pane through `worker-release`
//! and threw the answer away: a refusal left the pane standing with no line
//! anywhere. On 11-04 night the sentence "the project of this pane or run-N is
//! not identified" refused the build line's notices, and the same project gate
//! refuses the sweep when the coordinator's pane stands outside the run's
//! project. These drive the sweep through the window's own doors: the verb it
//! sends, the letter its run gets, the reason the board reads, and the line a
//! person typed on.

use super::restore::{Restoring, a_run, a_worker, row};
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// An empty Claude Code composer, as the window's terminal holds it.
const EMPTY_FIELD: &str = "\x1b[?2004h\x1b[?25h\x1b[3;1H────────\r\n❯ \r\n────────\x1b[4;3H";
/// The same composer holding words the person typed.
const FIELD_WITH_WORDS: &str =
    "\x1b[?2004h\x1b[?25h\x1b[3;1H────────\r\n❯ please also fix the docs\r\n────────\x1b[4;27H";

/// The beat's host: the folder each pane works in, whether its panes report a
/// session, every close, and what each pane's live field shows when looked at.
struct Sweeping {
    folder: Option<std::path::PathBuf>,
    identified: AtomicBool,
    closed: Mutex<Vec<u32>>,
    field: Mutex<Option<&'static str>>,
    looks: AtomicU64,
}

impl Sweeping {
    fn in_folder(folder: Option<&std::path::Path>) -> Self {
        Self {
            folder: folder.map(std::path::Path::to_path_buf),
            identified: AtomicBool::new(true),
            closed: Mutex::new(Vec::new()),
            field: Mutex::new(None),
            looks: AtomicU64::new(0),
        }
    }
    fn closed(&self) -> Vec<u32> {
        self.closed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone()
    }
    fn shows(&self, field: &'static str) {
        *self.field.lock().unwrap_or_else(|held| held.into_inner()) = Some(field);
    }
}

impl Host for Sweeping {
    fn worktree_of(&self, _term: u32) -> Option<std::path::PathBuf> {
        self.folder.clone()
    }
    fn split(
        &self,
        _team: &str,
        _leader_term: u32,
        _from_term: u32,
        _pane: &str,
        _direction: zerocode_core::agent_teams::Direction,
        _command: &str,
        _token: &str,
    ) -> Option<u32> {
        None
    }
    fn send(&self, _term: u32, _text: &str) -> bool {
        false
    }
    fn capture(&self, _term: u32) -> Option<String> {
        Some("what the worker printed".to_string())
    }
    fn focus(&self, _term: u32) -> bool {
        false
    }
    fn close(&self, term: u32) {
        self.closed
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .push(term);
    }
    fn actor_for(&self, term: u32) -> Option<String> {
        self.identified
            .load(Ordering::SeqCst)
            .then(|| test_actor(term))
    }
    /// The real host's look: the live field under the terminal lock, after a
    /// redraw that followed the last hand. Each look is the next grace later,
    /// as the beats that ask are.
    fn reread_line(&self, term: u32) {
        let Some(field) = *self.field.lock().unwrap_or_else(|held| held.into_inner()) else {
            return;
        };
        let mut screen = zerocode_pty::Terminal::new(8, 64);
        screen.feed(field.as_bytes());
        let look = self.looks.fetch_add(1, Ordering::SeqCst) + 1;
        let output = std::time::Instant::now();
        let now = output + std::time::Duration::from_secs(16 * look);
        crate::human_input::recheck_grid(term, "claude", screen.grid(), Some(output), now);
    }
}

/// A finished worker of a run made in `checkout`'s project: its report filed
/// at the answer's time, which the sweep's grace counts from.
fn a_finished_worker(
    leader: u32,
    worker_term: u32,
    name: &str,
    checkout: &std::path::Path,
) -> (String, String, i64) {
    let host = Restoring::new(checkout, worker_term, (0, test_actor(leader)));
    let (team, run_id) = a_run(&host, leader, name);
    let (_task, worker, _term) = a_worker(&host, &team, "--agent claude", None);
    let held = row(&worker);
    let capability = crate::agent_teams::current_pane_capability(&held.team, &held.pane).unwrap();
    let ended = clock();
    let done = run(
        &host,
        Vec::new(),
        &held.team,
        &held.pane,
        &capability,
        &words(&format!(
            "send --type worker_done --body {{\"ok\":true}} --retry-request done-{name}"
        )),
        ended,
    );
    assert_eq!(done.exit_code, 0, "{}", done.stderr);
    assert_eq!(row(&worker).state, WorkerState::Reclaimable);
    (run_id, worker, ended)
}

/// The run's letters to its coordinator that hold `needle`.
fn letters_to_the_run(run_id: &str, needle: &str) -> Vec<String> {
    let held = super::super::runtime().expect("the runtime");
    let image = held.actor.view().expect("the ledger");
    image
        .ledger()
        .run(run_id)
        .expect("the run")
        .messages()
        .iter()
        .filter(|one| one.to == format!("run:{run_id}") && one.body.as_str().contains(needle))
        .map(|one| one.body.as_str().to_string())
        .collect()
}

/// t-94842 ②: the coordinator's pane stands in a folder outside the run's
/// project (a checkout of another repository, a folder it `cd`'d to). The sweep
/// is the window's own verb for that run, so it answers for the run's recorded
/// project and the finished pane is closed. Before, the project gate refused it
/// as a crossing into another project and the pane stood for good.
#[test]
fn a_finished_pane_is_reclaimed_when_its_coordinators_pane_stands_outside_the_project() {
    const LEADER: u32 = 948_420;
    const WORKER: u32 = 948_421;
    let (_window, _store) = PrivateWindow::boot();
    let _turn = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the run's project");
    let elsewhere = tempfile::tempdir().expect("a folder outside it");
    let (_run, worker, ended) = a_finished_worker(LEADER, WORKER, "outside", checkout.path());

    let sweeping = Sweeping::in_folder(Some(elsewhere.path()));
    super::super::release_finished_panes(&sweeping, &[], ended + super::super::RESEAT_GRACE_MS);
    assert_eq!(
        sweeping.closed(),
        vec![WORKER],
        "held: {:?}",
        super::super::release_held_reason(&worker)
    );
    assert_eq!(row(&worker).state, WorkerState::Released);
    assert_eq!(super::super::release_held_reason(&worker), None);
    for pane in [WORKER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-94842 ②, the pane that answers no folder at all (a coordinator seat
/// restored before its checkout was read): the same — the run's project is the
/// ledger's to say, not the pane's.
#[test]
fn a_finished_pane_is_reclaimed_when_its_coordinators_pane_answers_no_folder() {
    const LEADER: u32 = 948_430;
    const WORKER: u32 = 948_431;
    let (_window, _store) = PrivateWindow::boot();
    let _turn = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the run's project");
    let (_run, worker, ended) = a_finished_worker(LEADER, WORKER, "nofolder", checkout.path());

    let sweeping = Sweeping::in_folder(None);
    super::super::release_finished_panes(&sweeping, &[], ended + super::super::RESEAT_GRACE_MS);
    assert_eq!(
        sweeping.closed(),
        vec![WORKER],
        "held: {:?}",
        super::super::release_held_reason(&worker)
    );
    assert_eq!(row(&worker).state, WorkerState::Released);
    for pane in [WORKER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-94842 ①: a refused release is kept. The coordinator's pane has no session
/// identity yet (right after a restart), so `worker-release` is refused: the
/// reason is what the board reads beside the worker, the run is told once in
/// one letter, the same reason asked again tells nobody again, nothing is asked
/// before the minute is up, and once the refusal's cause is gone the next ask
/// closes the pane and the reason goes with it.
#[test]
fn a_refused_release_is_kept_told_once_and_asked_again_a_minute_later() {
    const LEADER: u32 = 948_440;
    const WORKER: u32 = 948_441;
    let (_window, _store) = PrivateWindow::boot();
    let _turn = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the run's project");
    let (run_id, worker, ended) = a_finished_worker(LEADER, WORKER, "refused", checkout.path());
    let first = ended + super::super::RESEAT_GRACE_MS;
    let retry = super::super::RELEASE_RETRY_MS;

    let sweeping = Sweeping::in_folder(Some(checkout.path()));
    sweeping.identified.store(false, Ordering::SeqCst);
    super::super::release_finished_panes(&sweeping, &[], first);
    assert!(sweeping.closed().is_empty());
    let reason = super::super::release_held_reason(&worker).expect("the refusal was kept");
    assert!(reason.contains("no session identity"), "{reason}");
    let told = letters_to_the_run(&run_id, "was not reclaimed");
    assert_eq!(told.len(), 1, "{told:#?}");
    assert!(
        told[0].contains(&worker) && told[0].contains(&reason),
        "{told:#?}"
    );

    // The same reason, asked again a minute later: no second letter.
    super::super::release_finished_panes(&sweeping, &[], first + retry);
    assert_eq!(letters_to_the_run(&run_id, "was not reclaimed").len(), 1);
    assert_eq!(row(&worker).state, WorkerState::Reclaimable);

    // The cause is gone; inside the minute nothing is asked, after it the
    // pane is closed.
    sweeping.identified.store(true, Ordering::SeqCst);
    super::super::release_finished_panes(&sweeping, &[], first + 2 * retry - 1);
    assert!(
        sweeping.closed().is_empty(),
        "asked again inside the minute"
    );
    super::super::release_finished_panes(&sweeping, &[], first + 2 * retry);
    assert_eq!(sweeping.closed(), vec![WORKER]);
    assert_eq!(row(&worker).state, WorkerState::Released);
    assert_eq!(super::super::release_held_reason(&worker), None);
    assert_eq!(letters_to_the_run(&run_id, "was not reclaimed").len(), 1);
    for pane in [WORKER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-94842: words the person typed on a finished worker's line are theirs, and
/// the pane stays with the reason said once; when the field is seen empty — they
/// erased the words, or a composition was let go — the pane is closed with its
/// last screen archived. Before, any mark on the line held the pane, and the
/// sweep never looked at the field again, so an erased line held it for good.
#[test]
fn a_finished_pane_stays_while_the_persons_words_stand_and_goes_once_the_field_is_empty() {
    const LEADER: u32 = 948_450;
    const WORKER: u32 = 948_451;
    let (_window, _store) = PrivateWindow::boot();
    let _turn = one_beat_at_a_time();
    let checkout = tempfile::tempdir().expect("the run's project");
    let (run_id, worker, ended) = a_finished_worker(LEADER, WORKER, "words", checkout.path());
    crate::human_input::forget_term(WORKER);
    crate::human_input::typed(WORKER);
    // A Backspace: a key that carries their words on.
    crate::human_input::keyed(WORKER);
    crate::human_input::provider_said(WORKER, true, true);
    let first = ended + super::super::RESEAT_GRACE_MS;

    let sweeping = Sweeping::in_folder(Some(checkout.path()));
    sweeping.shows(FIELD_WITH_WORDS);
    super::super::release_finished_panes(&sweeping, &[], first);
    assert!(
        sweeping.closed().is_empty(),
        "the person's words were closed"
    );
    assert_eq!(
        super::super::release_held_reason(&worker).as_deref(),
        Some(super::super::RELEASE_HELD_OWN_WORDS)
    );
    assert_eq!(letters_to_the_run(&run_id, "was not reclaimed").len(), 1);

    sweeping.shows(EMPTY_FIELD);
    super::super::release_finished_panes(&sweeping, &[], first + super::super::RELEASE_RETRY_MS);
    assert_eq!(sweeping.closed(), vec![WORKER]);
    assert_eq!(row(&worker).state, WorkerState::Released);
    crate::human_input::forget_term(WORKER);
    for pane in [WORKER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-94842, the case coordinator 87472 met (m-94851): a pane the person took
/// over, whose line they wrote on and then emptied, looks empty on screen and
/// `worker-return` still said it held their unsent words — the refusal read a
/// mark nothing looked at again. The window looks at the live field first: an
/// empty field hands the pane back, a field still holding words is refused.
#[test]
fn worker_return_looks_at_the_live_field_before_refusing_over_a_draft() {
    const LEADER: u32 = 948_460;
    const WORKER: u32 = 948_461;
    let _window = the_window();
    let team = format!("team-return-field-{LEADER}");
    let (run_id, worker, _pane) = a_worker_carrying_work(&team, LEADER, WORKER);
    let leader = zerocode_core::agent_teams::LEADER_PANE;
    super::super::pane_taken_over(WORKER, "words", clock());
    crate::human_input::forget_term(WORKER);
    crate::human_input::typed(WORKER);
    crate::human_input::keyed(WORKER);
    crate::human_input::provider_said(WORKER, true, true);
    let host = Sweeping::in_folder(None);
    let give_back = || {
        run(
            &host,
            Vec::new(),
            &team,
            leader,
            TEST_CAPABILITY,
            &words(&format!("worker-return --worker {worker}")),
            clock(),
        )
    };

    host.shows(FIELD_WITH_WORDS);
    let refused = give_back();
    assert_ne!(refused.exit_code, 0, "a pane holding words was handed back");
    assert!(
        refused.stderr.contains("has not sent"),
        "{}",
        refused.stderr
    );

    host.shows(EMPTY_FIELD);
    let back = give_back();
    assert_eq!(back.exit_code, 0, "{}", back.stderr);
    assert!(
        !the_rows()
            .workers
            .iter()
            .find(|row| row.run == run_id && row.id == worker)
            .expect("the worker row")
            .taken_over
    );
    crate::human_input::forget_term(WORKER);
    crate::agent_teams::forget_term(WORKER);
    crate::agent_teams::forget_term(LEADER);
}
