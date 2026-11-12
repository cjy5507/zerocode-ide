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

use zerocode_core::orchestration::Ledger;
use zerocode_core::orchestration::room::PeerPane;

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
            let folder = host.worktree_of(term);
            Some((
                project_scope::pane_root(host, term),
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
}

/// Why the pointer to `newest` — the newest letter waiting at `address` in
/// `run` — is not handed to `term` this beat, when it was written in a room:
/// it is a room reply, or the pane has been woken for [`WAKES_PER_HAND`]
/// other room letters since the person last typed there. `None` for every
/// other letter, and for a room letter the budget still covers — which is
/// counted here, once per letter, so a pointer said again for the same
/// letter costs nothing more.
pub(super) fn held_back(
    ledger: &Ledger,
    run: &str,
    newest: &str,
    term: u32,
) -> Option<&'static str> {
    /* Asked of every pane the mail pass points at, every beat: nothing to
     * read while no room is open, and the newest letter is looked for from
     * the back, where it stands. */
    if !ledger.runs().iter().any(|held| held.is_room()) {
        return None;
    }
    let message = ledger
        .run(run)?
        .messages()
        .iter()
        .rev()
        .find(|held| held.id == newest)?;
    let written_in = message.from_run.as_deref().unwrap_or(run);
    if !ledger.run(written_in).is_some_and(|held| held.is_room()) {
        return None;
    }
    if message.thread.is_some() {
        return Some("a room reply wakes nobody");
    }
    let mut woken = woken().lock().unwrap_or_else(|held| held.into_inner());
    let spent = woken.entry(term).or_default();
    if spent.contains(newest) {
        return None;
    }
    if spent.len() >= WAKES_PER_HAND {
        return Some(
            "room letters already woke this pane as often as one input of the person allows",
        );
    }
    spent.insert(newest.to_string());
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::agent_teams::Team;
    use zerocode_core::orchestration::evidence::ReceiptFiles;
    use zerocode_core::orchestration::{Launcher, plan_in_project};

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

    /// Three different room letters wake a pane; the fourth waits until the
    /// person types there, and a letter already counted is said again for
    /// free (the person's decision: 「기본 켬 · 사람 입력 뒤 3번」).
    #[test]
    fn a_room_letter_wakes_a_pane_three_times_until_the_person_types_there() {
        const TERM: u32 = 71_001;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-a", "token", 7);
        let to = [
            "send",
            "--to",
            "pane:team-room-a/%8",
            "--type",
            "status",
            "--body",
        ];
        let letters: Vec<String> = (0..4)
            .map(|at| {
                let line: Vec<&str> = to.iter().copied().chain(["news"]).collect();
                id(&typed(&mut ledger, &mut team, "%9", &line, 1_000 + at))
            })
            .collect();
        let room = ledger.room_of(A).expect("a room").to_string();
        for letter in &letters[..WAKES_PER_HAND] {
            assert_eq!(held_back(&ledger, &room, letter, TERM), None);
        }
        assert!(held_back(&ledger, &room, &letters[3], TERM).is_some());
        assert_eq!(
            held_back(&ledger, &room, &letters[0], TERM),
            None,
            "said again, not spent again"
        );
        person_at(TERM);
        assert_eq!(held_back(&ledger, &room, &letters[3], TERM), None);
    }

    /// A room reply wakes nobody — the asker has it as its tool's answer —
    /// so two panes answering each other cannot keep each other turning.
    #[test]
    fn a_room_reply_wakes_nobody() {
        const TERM: u32 = 71_002;
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
        let answer = typed(
            &mut ledger,
            &mut team,
            "%8",
            &["reply", "--to-message", &id(&asked), "--body", "this one"],
            1_001,
        );
        let room = ledger.room_of(A).expect("a room").to_string();
        assert_eq!(
            held_back(&ledger, &room, &id(&answer), TERM),
            Some("a room reply wakes nobody")
        );
        assert_eq!(
            held_back(&ledger, &room, &id(&asked), TERM),
            None,
            "the question wakes"
        );
    }

    /// A letter written in a coordinator's run is none of this budget's.
    #[test]
    fn a_letter_written_in_no_room_is_never_held() {
        const TERM: u32 = 71_003;
        let mut ledger = Ledger::new();
        let mut team = Team::new("team-room-c", "token", 7);
        let run = typed(
            &mut ledger,
            &mut team,
            "%1",
            &["run-create", "--name", "work"],
            1_000,
        );
        let run = run["runId"].as_str().expect("a run").to_string();
        for at in 0..5 {
            let said = typed(
                &mut ledger,
                &mut team,
                "%1",
                &[
                    "send",
                    "--to",
                    &format!("run:{run}"),
                    "--type",
                    "status",
                    "--body",
                    "x",
                ],
                1_001 + at,
            );
            assert_eq!(held_back(&ledger, &run, &id(&said), TERM), None);
        }
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
