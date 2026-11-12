//! The window's half of a project's room
//! ([`zerocode_core::orchestration::room`]): the person's switch, the agent
//! panes `peers` lists, and how often a room letter may wake a pane.
//!
//! The ledger cannot look at the window, so it asks the launcher, under the
//! ledger, as a lookup: whether 「에이전트 판끼리 편지」 is on
//! ([`peer_mail`]) and which agent panes stand in the caller's project
//! ([`live_panes`]). Both answer from a cell this side writes — the setting
//! when the person saves it, the panes on every beat of the mail pass
//! ([`note_panes`]) — so the actor never waits on a pane.
//!
//! **The wake budget** (사람 결정 2026-11-12, 「기본 켬 · 사람 입력 뒤 3번」): a
//! letter written in a room may wake a pane for at most [`WAKES_PER_HAND`]
//! different letters after the person last typed in that pane; after that
//! the letters wait, read at the pane's next `check` or by the person, until
//! the person types there again ([`person_at`]). And a room reply wakes
//! nobody: the asker waiting on it has it as its tool's answer, and one not
//! waiting reads it at its next `check` — so two panes answering each other
//! cannot keep each other turning with nobody watching ([`held_back`]).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use zerocode_core::orchestration::Run;
use zerocode_core::orchestration::room::{self, PeerPane};

use super::{Host, PaneTurn, project_scope};

/// Different room letters that may wake one pane between two of the person's
/// inputs there.
pub(crate) const WAKES_PER_HAND: usize = 3;

/// The person's 「에이전트 판끼리 편지」 switch, as last saved. On until the
/// boot reads the settings: on is what a person who never opened them has.
static MAIL_ON: AtomicBool = AtomicBool::new(true);

/// What the person saved for the switch.
pub(crate) fn set_peer_mail(on: bool) {
    MAIL_ON.store(on, Ordering::SeqCst);
}

/// Whether the switch is on — a lookup the actor makes under the ledger.
pub(super) fn peer_mail() -> bool {
    MAIL_ON.load(Ordering::SeqCst)
}

/// Each agent pane with the project root it works in (none for a pane in no
/// known project).
type PaneRows = Vec<(Option<String>, PeerPane)>;

/// The agent panes as the last beat read them.
fn panes() -> &'static Mutex<PaneRows> {
    static PANES: OnceLock<Mutex<PaneRows>> = OnceLock::new();
    PANES.get_or_init(|| Mutex::new(Vec::new()))
}

/// Write down the agent panes this window holds, for `peers`: every pane an
/// agent runs in, its project, folder and whether it is working, idle or
/// waiting on the person. The pane table is let go before the host is asked
/// anything — the host's reads take it themselves.
pub(super) fn note_panes(
    host: &dyn Host,
    turns: &HashMap<u32, PaneTurn>,
    attention: &HashMap<u32, Option<i64>>,
) {
    let seats: Vec<(String, u32)> = crate::agent_teams::teams()
        .iter()
        .flat_map(|(team, held)| {
            held.panes()
                .map(|pane| (format!("{team}/{}", pane.id), pane.term))
                .collect::<Vec<_>>()
        })
        .collect();
    let rows: PaneRows = seats
        .into_iter()
        .filter_map(|(seat, term)| {
            let agent = host.agent_of(term)?;
            let state = if attention.get(&term).copied().flatten().is_some() {
                "waiting"
            } else if matches!(turns.get(&term), Some(PaneTurn::Running { .. })) {
                "working"
            } else {
                "idle"
            };
            // The folder read once, and its project from it.
            let folder = host.worktree_of(term);
            let root = folder.as_deref().and_then(project_scope::root_of);
            Some((
                root,
                PeerPane {
                    seat,
                    agent: Some(agent),
                    title: None,
                    folder: folder.map(|path| path.display().to_string()),
                    state,
                },
            ))
        })
        .collect();
    *panes().lock().unwrap_or_else(|held| held.into_inner()) = rows;
}

/// The agent panes of the project rooted at `project`, for `peers`. A pane
/// in no known project — a remote workspace's, one whose folder the window
/// has no record of — is in nobody's list, and a caller in none is shown
/// nobody.
pub(super) fn live_panes(project: Option<&str>) -> Option<Vec<PeerPane>> {
    Some(of_project(
        &panes().lock().unwrap_or_else(|held| held.into_inner()),
        project,
    ))
}

/// The rows of `rows` in the project rooted at `project` — none for a caller
/// in no known project.
fn of_project(rows: &[(Option<String>, PeerPane)], project: Option<&str>) -> Vec<PeerPane> {
    let Some(project) = project else {
        return Vec::new();
    };
    rows.iter()
        .filter(|(root, _)| root.as_deref() == Some(project))
        .map(|(_, pane)| pane.clone())
        .collect()
}

/// The room letters each pane was woken for since the person last typed
/// there.
fn woken() -> &'static Mutex<HashMap<u32, HashSet<String>>> {
    static WOKEN: OnceLock<Mutex<HashMap<u32, HashSet<String>>>> = OnceLock::new();
    WOKEN.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The person typed or sent something in `term`: the room may wake it again.
pub(crate) fn person_at(term: u32) {
    woken()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .remove(&term);
    noted()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .retain(|(noted_term, _)| *noted_term != term);
}

/// A room letter's recipient, held to a pane an agent runs in: a pane where
/// only a shell stands would take the letter, the sender would hear it was
/// filed, and nobody would ever read it (review t-105562). `term` is the
/// recipient's pane in this window; a pane the window does not hold is the
/// project door's to refuse ([`project_scope`]'s `recipient_gate`).
pub(super) fn room_recipient(
    host: &dyn Host,
    verb: &str,
    recipient: &str,
    term: Option<u32>,
) -> Result<(), String> {
    match term {
        Some(term) if host.agent_of(term).is_none() => Err(format!(
            "pane:{recipient} is a pane no agent runs in — `{verb}` was not sent; a room \
             letter goes to an agent pane (`zerocode-orc peers` lists them)"
        )),
        _ => Ok(()),
    }
}

/// The holds already written down in the window's log, by pane and newest
/// letter — one line each, until the person types in that pane again.
fn noted() -> &'static Mutex<HashSet<(u32, String)>> {
    static NOTED: OnceLock<Mutex<HashSet<(u32, String)>>> = OnceLock::new();
    NOTED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// The reason a room reply is not pointed at.
pub(super) const ROOM_REPLY: &str = "a room reply wakes nobody";

/// The reason a room letter past the budget is not pointed at.
pub(super) const BUDGET_SPENT: &str =
    "room letters already woke this pane as often as one input of the person allows";

/// Why the letters waiting at `address` in `run` are not pointed at `term`
/// this beat — every one of them a room's ([`room::of_a_room`]: written in a
/// room, or an answer in a thread a room letter began, wherever filed):
/// they are all room replies, or the pane has been woken for
/// [`WAKES_PER_HAND`] other room letters since the person last typed there.
///
/// `None` the moment one waiting letter is not a room's: a `worker_done` or a
/// coordinator's word behind a held room copy is pointed at as it always was.
/// `None` too for a room letter the budget still covers — counted here, once
/// per letter (the newest of them), so a pointer said again for the same
/// letter costs nothing more. `rooms_open` is asked once a pass: with no room
/// in the ledger nothing here is read at all.
pub(super) fn held_back(
    held: &Run,
    address: &str,
    term: u32,
    rooms_open: bool,
) -> Option<&'static str> {
    if !rooms_open || held.newest_unread(address).is_none() {
        return None;
    }
    let waiting = held.unread(address);
    // The newest room letter that is no reply, and whether only replies came
    // after it.
    let mut newest_letter: Option<&str> = None;
    let mut replies_after = false;
    for id in &waiting {
        // Looked for from the back, where waiting mail stands.
        let message = held.messages().iter().rev().find(|one| one.id == *id)?;
        if !room::of_a_room(held, message) {
            return None;
        }
        match message.thread {
            None => (newest_letter, replies_after) = (Some(*id), false),
            Some(_) => replies_after = true,
        }
    }
    let why = match newest_letter {
        None => ROOM_REPLY,
        Some(letter) => {
            let mut woken = woken().lock().unwrap_or_else(|held| held.into_inner());
            let spent = woken.entry(term).or_default();
            if spent.contains(letter) {
                /* Already counted: said again for the same letter it costs
                 * nothing — but a reply that came in behind it, with the
                 * letter not yet acknowledged, is a reply, and wakes nobody
                 * (review t-105587, 2). */
                if !replies_after {
                    return None;
                }
                ROOM_REPLY
            } else if spent.len() < WAKES_PER_HAND {
                spent.insert(letter.to_string());
                return None;
            } else {
                BUDGET_SPENT
            }
        }
    };
    // Said once per pane and newest letter, in the window's own log.
    let newest = waiting.last().copied().unwrap_or_default();
    if noted()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .insert((term, newest.to_string()))
    {
        super::note_pointer_silent(&held.id, address, term, &format!("({why})"));
    }
    Some(why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::agent_teams::Team;
    use zerocode_core::orchestration::evidence::ReceiptFiles;
    use zerocode_core::orchestration::{Launcher, Ledger, plan_in_project};

    /// The wake budget asked of the run `run` of `ledger`, a room open.
    fn held(ledger: &Ledger, run: &str, address: &str, term: u32) -> Option<&'static str> {
        held_back(ledger.run(run).expect("the run"), address, term, true)
    }

    const A: &str = "/Users/dev/a/.git";
    const B: &str = "/Users/dev/b/.git";

    /// The window with the switch on.
    struct On;

    impl Launcher for On {
        fn command_for(&self, agent: &str, _: &str, _: &[String]) -> Result<String, String> {
            Ok(agent.to_string())
        }

        fn peer_mail_on(&self) -> bool {
            true
        }
    }

    /// One verb from `pane` of `team` in project A, answered as JSON.
    fn typed(
        ledger: &mut Ledger,
        team: &mut Team,
        pane: &str,
        line: &[&str],
        at: i64,
    ) -> serde_json::Value {
        let argv: Vec<String> = line
            .iter()
            .map(|word| (*word).to_string())
            .chain(["--retry-request".to_string(), format!("room-{at}")])
            .collect();
        let actor = zerocode_core::orchestration::receipt_actor(
            "codex",
            zerocode_core::provider_session::SessionKey::SessionId,
            &format!("room-test-{pane}"),
        );
        let decided = plan_in_project(
            ledger,
            team,
            &On,
            &argv,
            pane,
            at,
            Some(&actor),
            &ReceiptFiles::default(),
            None,
            Some(A),
        );
        assert_eq!(
            decided.reply.exit_code, 0,
            "{line:?}: {}",
            decided.reply.stderr
        );
        serde_json::from_str(&decided.reply.stdout).expect("JSON")
    }

    fn id(said: &serde_json::Value) -> String {
        said["messageId"]
            .as_str()
            .expect("a message id")
            .to_string()
    }

    /// `pane:<team>/<pane>`.
    fn pane(team: &str, pane: &str) -> String {
        format!("pane:{team}/{pane}")
    }

    /// A status letter to `to`.
    fn letter(to: &str, body: &str) -> Vec<String> {
        ["send", "--to", to, "--type", "status", "--body", body]
            .iter()
            .map(|word| (*word).to_string())
            .collect()
    }

    fn words(line: &[String]) -> Vec<&str> {
        line.iter().map(String::as_str).collect()
    }

    /// A run of project A coordinated from `%1` of `team`.
    fn chair_run(ledger: &mut Ledger, team: &mut Team) -> String {
        typed(ledger, team, "%1", &["run-create", "--name", "work"], 900)["runId"]
            .as_str()
            .expect("a run")
            .to_string()
    }

    /// Room letters wake a pane for three different letters; the fourth waits
    /// until the person types there, and a pointer said again for a letter
    /// already counted costs nothing (사람 결정 「기본 켬 · 사람 입력 뒤 3번」).
    /// With no room open nothing is read at all.
    #[test]
    fn a_room_letter_wakes_a_pane_three_times_until_the_person_types_there() {
        const TERM: u32 = 71_001;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-a", "token", 7);
        let to = pane("team-room-a", "%8");
        for at in 0..WAKES_PER_HAND {
            typed(
                &mut ledger,
                &mut team,
                "%9",
                &words(&letter(&to, "news")),
                1_000 + at as i64,
            );
            let room = ledger.room_of(A).expect("a room").to_string();
            assert_eq!(held(&ledger, &room, &to, TERM), None, "letter {at}");
            assert_eq!(held(&ledger, &room, &to, TERM), None, "said again");
        }
        typed(
            &mut ledger,
            &mut team,
            "%9",
            &words(&letter(&to, "news")),
            1_010,
        );
        let room = ledger.room_of(A).expect("a room").to_string();
        assert_eq!(held(&ledger, &room, &to, TERM), Some(BUDGET_SPENT));
        assert_eq!(
            held_back(ledger.run(&room).expect("the run"), &to, TERM, false),
            None,
            "no room open: nothing read"
        );
        person_at(TERM);
        assert_eq!(held(&ledger, &room, &to, TERM), None);
    }

    /// A room reply wakes nobody — the asker has it as its tool's answer —
    /// so two panes answering each other cannot keep each other turning.
    #[test]
    fn a_room_reply_wakes_nobody() {
        const ASKER: u32 = 71_002;
        const ASKED: u32 = 71_012;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-b", "token", 7);
        let asked = typed(
            &mut ledger,
            &mut team,
            "%9",
            &[
                "send",
                "--to",
                "pane:team-room-b/%8",
                "--type",
                "question",
                "--body",
                "which?",
            ],
            1_000,
        );
        let room = ledger.room_of(A).expect("a room").to_string();
        let to_asked = pane("team-room-b", "%8");
        assert_eq!(
            held(&ledger, &room, &to_asked, ASKED),
            None,
            "the question wakes"
        );
        typed(
            &mut ledger,
            &mut team,
            "%8",
            &["reply", "--to-message", &id(&asked), "--body", "this one"],
            1_001,
        );
        let to_asker = pane("team-room-b", "%9");
        assert_eq!(held(&ledger, &room, &to_asker, ASKER), Some(ROOM_REPLY));
    }

    /// A room letter to a pane that coordinates a run is filed as a copy in
    /// that run's `run:` inbox: the copy counts against the coordinator pane's
    /// budget like any room letter, and the coordinator's answer to it,
    /// written in its own run, is a room reply that wakes nobody (review
    /// t-105562, 1).
    #[test]
    fn a_room_copy_in_a_coordinators_run_counts_and_the_answer_to_it_wakes_nobody() {
        const CHAIR: u32 = 71_004;
        const LONE: u32 = 71_005;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-e", "token", 7);
        let run = chair_run(&mut ledger, &mut team);
        let chair = format!("run:{run}");
        let to_chair = pane("team-room-e", "%1");
        let mut last_copy = String::new();
        for at in 0..=WAKES_PER_HAND {
            let sent = typed(
                &mut ledger,
                &mut team,
                "%9",
                &words(&letter(&to_chair, "x")),
                1_001 + at as i64,
            );
            last_copy = sent["deliveredTo"]["messageId"]
                .as_str()
                .expect("a copy")
                .to_string();
            let held = held(&ledger, &run, &chair, CHAIR);
            match at < WAKES_PER_HAND {
                true => assert_eq!(held, None, "copy {at}"),
                false => assert_eq!(held, Some(BUDGET_SPENT), "copy {at}"),
            }
        }
        typed(
            &mut ledger,
            &mut team,
            "%1",
            &["reply", "--to-message", &last_copy, "--body", "noted"],
            1_020,
        );
        let to_lone = pane("team-room-e", "%9");
        assert_eq!(held(&ledger, &run, &to_lone, LONE), Some(ROOM_REPLY));
    }

    /// A live coordinator writing into the room with `--run` writes a room
    /// letter, counted against the pane it wakes (review t-105562, 1).
    #[test]
    fn a_coordinator_writing_into_the_room_is_counted() {
        const LONE: u32 = 71_006;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-f", "token", 7);
        chair_run(&mut ledger, &mut team);
        let to_lone = pane("team-room-f", "%9");
        typed(
            &mut ledger,
            &mut team,
            "%9",
            &words(&letter(&pane("team-room-f", "%8"), "open")),
            1_000,
        );
        let room = ledger.room_of(A).expect("a room").to_string();
        for at in 0..=WAKES_PER_HAND {
            let line = [
                "send", "--run", &room, "--to", &to_lone, "--type", "status", "--body", "x",
            ];
            typed(&mut ledger, &mut team, "%1", &line, 1_001 + at as i64);
            let held = held(&ledger, &room, &to_lone, LONE);
            match at < WAKES_PER_HAND {
                true => assert_eq!(held, None, "letter {at}"),
                false => assert_eq!(held, Some(BUDGET_SPENT), "letter {at}"),
            }
        }
    }

    /// An ordinary letter waiting in the same inbox as held room letters is
    /// pointed at as it always was: a held room copy never holds back the
    /// word a worker or a pane sent the coordinator (review t-105562, 1③).
    /// The budget is spent first, copy by copy, so the hold is real before
    /// the ordinary letter arrives (review t-105587).
    #[test]
    fn an_ordinary_letter_behind_held_room_letters_is_still_pointed_at() {
        const CHAIR: u32 = 71_007;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-g", "token", 7);
        let run = chair_run(&mut ledger, &mut team);
        let chair = format!("run:{run}");
        let to_chair = pane("team-room-g", "%1");
        for at in 0..=WAKES_PER_HAND {
            typed(
                &mut ledger,
                &mut team,
                "%9",
                &words(&letter(&to_chair, "x")),
                1_001 + at as i64,
            );
            let expected = (at == WAKES_PER_HAND).then_some(BUDGET_SPENT);
            assert_eq!(held(&ledger, &run, &chair, CHAIR), expected, "copy {at}");
        }
        let ordinary = [
            "send",
            "--run",
            run.as_str(),
            "--to",
            chair.as_str(),
            "--type",
            "status",
            "--body",
            "done",
        ];
        typed(&mut ledger, &mut team, "%7", &ordinary, 1_020);
        assert_eq!(
            held(&ledger, &run, &chair, CHAIR),
            None,
            "the ordinary letter now waiting keeps the pointer"
        );
    }

    /// A room letter already counted and not yet acknowledged does not let a
    /// reply that came in behind it wake the pane: the reply is a reply
    /// (review t-105587, 2).
    #[test]
    fn a_reply_behind_a_counted_letter_still_wakes_nobody() {
        const LONE: u32 = 71_008;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-h", "token", 7);
        let to_lone = pane("team-room-h", "%9");
        let to_other = pane("team-room-h", "%8");
        // %9 asks %8; %8 writes %9 a letter, counted when it wakes %9; then
        // %8 answers %9's question while the letter waits unacknowledged.
        let asked = typed(
            &mut ledger,
            &mut team,
            "%9",
            &[
                "send",
                "--to",
                to_other.as_str(),
                "--type",
                "question",
                "--body",
                "which?",
            ],
            1_000,
        );
        typed(
            &mut ledger,
            &mut team,
            "%8",
            &words(&letter(&to_lone, "a letter")),
            1_001,
        );
        let room = ledger.room_of(A).expect("a room").to_string();
        assert_eq!(
            held(&ledger, &room, &to_lone, LONE),
            None,
            "the letter is counted and wakes"
        );
        typed(
            &mut ledger,
            &mut team,
            "%8",
            &["reply", "--to-message", &id(&asked), "--body", "the first"],
            1_002,
        );
        assert_eq!(held(&ledger, &room, &to_lone, LONE), Some(ROOM_REPLY));
    }

    /// The board's agent graph draws a room letter as a line between the two
    /// pane cards, counted and unread — the person sees what agents said to
    /// each other (design decision 4), without the room standing on the task
    /// board as an empty run.
    #[test]
    fn the_agent_graph_draws_a_room_letter_between_the_two_panes() {
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-d", "token", 7);
        typed(
            &mut ledger,
            &mut team,
            "%9",
            &[
                "send",
                "--to",
                "pane:team-room-d/%8",
                "--type",
                "status",
                "--body",
                "found",
            ],
            1_000,
        );
        let seats: super::super::TeamSeatIndex = [(
            "team-room-d".to_string(),
            [("%9".to_string(), 91), ("%8".to_string(), 81)]
                .into_iter()
                .collect(),
        )]
        .into_iter()
        .collect();
        let overlay = super::super::graph_overlay_snapshot_for_seats(&ledger, &seats);
        let edge = overlay
            .mail
            .iter()
            .find(|edge| edge.from == "term:91" && edge.to == "term:81")
            .expect("a line between the two panes");
        assert_eq!((edge.count, edge.unread), (1, 1));
        let room = ledger.room_of(A).expect("a room");
        assert!(
            !super::super::desk::in_play(ledger.run(room).expect("the room")),
            "the room stands on the task board as a run"
        );
    }

    /// A host whose one agent pane is `agent`; every other pane is a shell.
    struct Panes {
        agent: u32,
    }

    impl Host for Panes {
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
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn agent_of(&self, term: u32) -> Option<String> {
            (term == self.agent).then(|| "codex".to_string())
        }
    }

    /// A room letter to a pane no agent runs in — a shell — is refused, so
    /// the sender never hears 「filed」 for a letter nobody will read; an
    /// agent pane takes it, and a pane this window does not hold is left to
    /// the project door (review t-105562).
    #[test]
    fn a_room_letter_to_a_pane_no_agent_runs_in_is_refused() {
        let host = Panes { agent: 1 };
        assert!(room_recipient(&host, "send", "t/%1", Some(1)).is_ok());
        let why = room_recipient(&host, "send", "t/%2", Some(2)).expect_err("a shell pane");
        assert!(why.contains("no agent runs in"), "{why}");
        assert!(room_recipient(&host, "send", "t/%3", None).is_ok());
    }

    /// `peers` shows the caller's project only, and nobody to a caller in
    /// none — a pane of another project, or one in no known project (a remote
    /// workspace's), is in no other project's list.
    #[test]
    fn peers_shows_the_panes_of_the_callers_project_only() {
        let pane = |seat: &str| PeerPane {
            seat: seat.to_string(),
            agent: Some("codex".to_string()),
            title: None,
            folder: None,
            state: "idle",
        };
        let rows = vec![
            (Some(A.to_string()), pane("t/%1")),
            (Some(B.to_string()), pane("t/%2")),
            (None, pane("t/%3")),
        ];
        let seats = |project| {
            of_project(&rows, project)
                .into_iter()
                .map(|pane| pane.seat)
                .collect::<Vec<_>>()
        };
        assert_eq!(seats(Some(A)), ["t/%1"]);
        assert_eq!(seats(Some(B)), ["t/%2"]);
        assert!(seats(None).is_empty());
    }
}
