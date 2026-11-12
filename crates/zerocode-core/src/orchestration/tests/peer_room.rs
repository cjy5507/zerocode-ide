//! A project's room (2026-11-12): agent panes the person opened write to each
//! other through the ledger, whatever CLI each one runs ([`room`]).

use super::*;
use crate::orchestration::room::{self, MAIL_OFF, PeerPane, ROOM_NAME};

const A: &str = "/Users/dev/a/.git";
const B: &str = "/Users/dev/b/.git";

/// A pane no run holds, in the bench's team.
const LONE: &str = "%9";
/// Another such pane.
const OTHER: &str = "%8";

/// The window with the person's 「에이전트 판끼리 편지」 on, and the panes it
/// has open.
struct RoomOn {
    panes: Vec<PeerPane>,
}

impl Launcher for RoomOn {
    fn command_for(
        &self,
        agent: &str,
        _prompt: &str,
        _tuning: &[String],
    ) -> Result<String, String> {
        Ok(agent.to_string())
    }

    fn peer_mail_on(&self) -> bool {
        true
    }

    fn live_panes(&self, _project: Option<&str>) -> Option<Vec<PeerPane>> {
        Some(self.panes.clone())
    }
}

fn on() -> RoomOn {
    RoomOn {
        panes: vec![
            PeerPane {
                seat: format!("team-1/{LONE}"),
                agent: Some("codex".to_string()),
                title: Some("codex".to_string()),
                folder: Some("/Users/dev/a".to_string()),
                state: "idle",
            },
            PeerPane {
                seat: format!("team-1/{OTHER}"),
                agent: Some("agy".to_string()),
                title: None,
                folder: None,
                state: "working",
            },
        ],
    }
}

/// One verb from `pane` in `project`, through `launcher`, with the receipt
/// filed the way the window files it.
fn typed(
    bench: &mut Bench,
    launcher: &dyn Launcher,
    pane: &str,
    project: Option<&str>,
    line: &[&str],
) -> Decided {
    bench.clock += 1;
    let argv = named_if_it_has_to_be(
        line.iter().map(|word| (*word).to_string()).collect(),
        bench.clock,
    );
    let actor = bench_actor(&bench.team.id, pane);
    let planned = plan_in_project(
        &mut bench.ledger,
        &mut bench.team,
        launcher,
        &argv,
        pane,
        bench.clock,
        Some(&actor),
        &crate::orchestration::evidence::ReceiptFiles::default(),
        None,
        project,
    );
    bench.ledger.file_receipt(&planned, bench.clock);
    planned
}

fn answered(planned: &Decided) -> serde_json::Value {
    assert_eq!(
        planned.reply.exit_code, 0,
        "refused: {}",
        planned.reply.stderr
    );
    serde_json::from_str(&planned.reply.stdout).expect("JSON")
}

fn refused(planned: &Decided) -> String {
    assert_ne!(
        planned.reply.exit_code, 0,
        "answered: {}",
        planned.reply.stdout
    );
    planned.reply.stderr.clone()
}

fn letter(to: &str, body: &str) -> Vec<String> {
    ["send", "--to", to, "--type", "status", "--body", body]
        .iter()
        .map(|word| (*word).to_string())
        .collect()
}

fn send(bench: &mut Bench, launcher: &dyn Launcher, pane: &str, to: &str, body: &str) -> Decided {
    let line = letter(to, body);
    let line: Vec<&str> = line.iter().map(String::as_str).collect();
    typed(bench, launcher, pane, Some(A), &line)
}

fn the_room(bench: &Bench) -> &Run {
    let id = bench.ledger.room_of(A).expect("a room for A");
    bench.ledger.run(id).expect("the room run")
}

/// The first letter a pane no run holds sends opens its project's room —
/// named for what it is, of that project, marked — and is signed with the
/// pane's own address. The answer it gets is the room it went to.
#[test]
fn a_pane_no_run_holds_opens_its_projects_room_with_its_first_letter() {
    let mut bench = Bench::new();
    let launcher = on();
    let to = format!("pane:team-1/{OTHER}");
    let sent = answered(&send(&mut bench, &launcher, LONE, &to, "found-it"));
    assert!(sent["messageId"].is_string(), "{sent}");
    let room = the_room(&bench);
    assert!(room.is_room());
    assert_eq!(room.name, ROOM_NAME);
    assert_eq!(room.project.as_ref().map(|p| p.root.as_str()), Some(A));
    assert!(room.coordinator.is_none(), "a room has no chair");
    let filed = room.messages().last().expect("the letter");
    assert_eq!(filed.from, format!("pane:team-1/{LONE}"));
    assert_eq!(filed.to, to);
    // The second letter goes to the same room.
    answered(&send(
        &mut bench,
        &launcher,
        OTHER,
        &format!("pane:team-1/{LONE}"),
        "thanks",
    ));
    assert_eq!(
        bench
            .ledger
            .runs()
            .iter()
            .filter(|run| run.is_room())
            .count(),
        1,
        "one room per project"
    );
}

/// The reader's side: a plain `check` from a pane no run holds reads its
/// pane address in the room.
#[test]
fn a_pane_no_run_holds_reads_its_room_letters_with_a_plain_check() {
    let mut bench = Bench::new();
    let launcher = on();
    answered(&send(
        &mut bench,
        &launcher,
        LONE,
        &format!("pane:team-1/{OTHER}"),
        "hello-other",
    ));
    let read = answered(&typed(&mut bench, &launcher, OTHER, Some(A), &["check"]));
    let bodies: Vec<&str> = read["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|message| message["body"].as_str())
        .collect();
    assert_eq!(bodies, ["hello-other"], "{read}");
}

/// Off unless the window says the person's switch is on: the letter is
/// refused with the reason, and no room is opened for it.
#[test]
fn a_room_letter_is_refused_while_the_persons_switch_is_off() {
    let mut bench = Bench::new();
    let off = Catalog(&["claude"]);
    let why = refused(&send(&mut bench, &off, LONE, "pane:team-1/%8", "hello"));
    assert!(why.contains(MAIL_OFF), "{why}");
    assert!(
        bench.ledger.room_of(A).is_none(),
        "a refused letter opened a room"
    );
}

/// Nobody speaks as the room. The leader-shaped pane — the one a run with no
/// coordinator record signs as `run:` — signs as its pane in a room, and a
/// letter to the room's own `run:` address is refused.
#[test]
fn in_a_room_a_leader_shaped_pane_signs_as_its_pane_never_as_the_run() {
    let mut bench = Bench::new();
    let launcher = on();
    let leader = agent_teams::LEADER_PANE;
    answered(&send(
        &mut bench,
        &launcher,
        leader,
        "pane:team-1/%8",
        "from-the-leader",
    ));
    let room = the_room(&bench);
    let filed = room.messages().last().expect("the letter");
    assert_eq!(filed.from, format!("pane:team-1/{leader}"));
    assert!(!filed.from.starts_with(RUN_ADDRESS_PREFIX));
    let address = room.address();
    let why = refused(&send(
        &mut bench,
        &launcher,
        leader,
        &address,
        "to-the-room",
    ));
    assert!(why.contains("pane: address"), "{why}");
}

/// A room answers mail and nothing else: a task, a worker, a gate, a seat
/// and every kind that carries work are refused, by `--run` or by the run id
/// a verb takes first, and so is a group.
#[test]
fn a_room_answers_mail_verbs_only() {
    let mut bench = Bench::new();
    let launcher = on();
    answered(&send(
        &mut bench,
        &launcher,
        LONE,
        "pane:team-1/%8",
        "open-it",
    ));
    let room = the_room(&bench).id.clone();
    for line in [
        vec!["task-create", "--run", &room, "--spec", "work"],
        vec![
            "worker-start",
            "--run",
            &room,
            "--agent",
            "claude",
            "--prompt",
            "go",
        ],
        vec!["gate-list", "--run", &room],
        vec!["run-use", &room],
        vec!["run-show", &room],
        vec!["handover-policy", "--run", &room],
    ] {
        let why = refused(&typed(&mut bench, &launcher, LONE, Some(A), &line));
        assert!(why.contains("this project's room"), "{line:?}: {why}");
    }
    for kind in [
        "worker_done",
        "dispatch",
        "handoff",
        "decision_gate",
        "escalation",
    ] {
        let line = [
            "send",
            "--to",
            "pane:team-1/%8",
            "--type",
            kind,
            "--body",
            "x",
        ];
        let why = refused(&typed(&mut bench, &launcher, LONE, Some(A), &line));
        assert!(
            why.contains("a room letter is a status or a question") || why.contains("worker_done"),
            "{kind}: {why}"
        );
    }
    let why = refused(&send(&mut bench, &launcher, LONE, "@all", "everyone"));
    assert!(why.contains("pane: address"), "{why}");
}

/// A room letter to a pane working as a worker goes to that run's
/// coordinator, its subject naming the worker, and nothing waits in the room
/// for a reader who never reads it there.
#[test]
fn a_room_letter_for_a_worker_goes_to_its_coordinator_saying_whom_it_was_for() {
    let mut bench = Bench::new();
    let run = bench.json("run-create --name work")["runId"]
        .as_str()
        .expect("a run")
        .to_string();
    let task = bench.json("task-create --spec x")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let launcher = on();
    let to = format!("pane:team-1/{pane}");
    let sent = answered(&send(&mut bench, &launcher, LONE, &to, "for-the-worker"));
    assert_eq!(sent["deliveredTo"]["runId"], run.as_str(), "{sent}");
    let held = bench.ledger.run(&run).expect("the run");
    let filed = held.messages().last().expect("the letter");
    assert_eq!(filed.to, held.address(), "filed for the coordinator");
    assert_eq!(
        filed.from,
        format!("pane:team-1/{LONE}"),
        "in the room's voice"
    );
    assert!(
        filed.subject.as_str().contains(&format!("worker:{worker}")),
        "the subject names the worker: {}",
        filed.subject.as_str()
    );
    let worker_address = format!("worker:{worker}");
    assert!(
        held.unread(&worker_address).is_empty(),
        "nothing reached the worker's own inbox"
    );
    assert!(
        bench
            .ledger
            .room_of(A)
            .and_then(|room| bench.ledger.run(room))
            .is_none_or(|room| room.messages().iter().all(|message| message.to != to)),
        "the room kept a copy nobody reads"
    );
}

/// And when that run has no coordinator sitting, the letter is refused with
/// the reason rather than dropped on the worker.
#[test]
fn a_room_letter_for_a_worker_of_a_run_nobody_coordinates_is_refused() {
    let mut bench = Bench::new();
    bench.json("run-create --name work");
    let task = bench.json("task-create --spec x")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (_, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    // The coordinator's pane is gone: its chair is written down as left.
    bench.ledger.vacate_seats_of_team("team-1", bench.clock);
    let launcher = on();
    let why = refused(&send(
        &mut bench,
        &launcher,
        LONE,
        &format!("pane:team-1/{pane}"),
        "x",
    ));
    assert!(why.contains("no coordinator sitting"), "{why}");
}

/// A room letter to a pane that coordinates a run reaches the `check` of that
/// run — and keeps the pane signature it was written with.
#[test]
fn a_room_letter_to_a_coordinator_pane_reaches_its_runs_check_in_the_rooms_voice() {
    let mut bench = Bench::new();
    let run = bench.json("run-create --name coordinated")["runId"]
        .as_str()
        .expect("a run")
        .to_string();
    let launcher = on();
    let to = format!("pane:team-1/{}", agent_teams::LEADER_PANE);
    let sent = answered(&send(&mut bench, &launcher, LONE, &to, "for-the-chair"));
    assert_eq!(sent["deliveredTo"]["runId"], run.as_str(), "{sent}");
    let held = bench.ledger.run(&run).expect("the run");
    let copy = held.messages().last().expect("the copy");
    assert_eq!(copy.to, held.address());
    assert_eq!(copy.from, format!("pane:team-1/{LONE}"));
}

/// A room question to a pane seated in a run would wait on nobody — it reads
/// there — so it is refused, saying to send instead.
#[test]
fn a_room_question_to_a_pane_seated_elsewhere_is_refused() {
    let mut bench = Bench::new();
    bench.json("run-create --name coordinated");
    let launcher = on();
    let to = format!("pane:team-1/{}", agent_teams::LEADER_PANE);
    let line = [
        "ask",
        "--to",
        to.as_str(),
        "--body",
        "which?",
        "--timeout-ms",
        "1000",
    ];
    let why = refused(&typed(&mut bench, &launcher, LONE, Some(A), &line));
    assert!(why.contains("send it a letter"), "{why}");
}

/// `peers` answers the window's panes, where the ledger knows each sits,
/// which one is the caller, the room and the switch.
#[test]
fn peers_lists_the_windows_panes_and_where_each_one_sits() {
    let mut bench = Bench::new();
    let launcher = on();
    let said = answered(&typed(
        &mut bench,
        &launcher,
        LONE,
        Some(A),
        &[room::PEERS_VERB],
    ));
    assert_eq!(said["mailOn"], true);
    assert!(
        said["room"].is_null(),
        "no room before the first letter: {said}"
    );
    let rows = said["peers"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["address"], format!("pane:team-1/{LONE}"));
    assert_eq!(rows[0]["you"], true);
    assert_eq!(rows[1]["agent"], "agy");
    assert_eq!(rows[1]["state"], "working");
    let refused_by = typed(
        &mut bench,
        &Catalog(&["claude"]),
        LONE,
        Some(A),
        &[room::PEERS_VERB],
    );
    assert!(refused(&refused_by).contains("answered by the window"));
}

/// A pane of another project cannot reach this project's room by naming it:
/// the room is a run of project A, and a pane of B is held out of it.
#[test]
fn a_pane_of_another_project_is_refused_this_projects_room() {
    let mut bench = Bench::new();
    let launcher = on();
    answered(&send(&mut bench, &launcher, LONE, "pane:team-1/%8", "in-a"));
    let room = the_room(&bench).id.clone();
    let line = [
        "send",
        "--run",
        &room,
        "--to",
        "pane:team-1/%9",
        "--type",
        "status",
        "--body",
        "from-b",
    ];
    let why = refused(&typed(&mut bench, &launcher, OTHER, Some(B), &line));
    assert!(!why.is_empty());
    assert!(
        the_room(&bench)
            .messages()
            .iter()
            .all(|message| message.body.as_str() != "from-b"),
        "a pane of B wrote into A's room"
    );
    // And B's own first letter opens B's room, not A's.
    let mine = [
        "send",
        "--to",
        "pane:team-1/%9",
        "--type",
        "status",
        "--body",
        "b-room",
    ];
    answered(&typed(&mut bench, &launcher, OTHER, Some(B), &mine));
    assert_ne!(bench.ledger.room_of(A), bench.ledger.room_of(B));
}

/// A retried room letter is the letter it retries — one letter, one room.
#[test]
fn a_retried_room_letter_is_the_same_letter() {
    let mut bench = Bench::new();
    let launcher = on();
    let line = [
        "send",
        "--to",
        "pane:team-1/%8",
        "--type",
        "status",
        "--body",
        "once",
        RETRY_REQUEST,
        "room-once",
    ];
    let first = answered(&typed(&mut bench, &launcher, LONE, Some(A), &line));
    let again = answered(&typed(&mut bench, &launcher, LONE, Some(A), &line));
    assert_eq!(first["messageId"], again["messageId"]);
    assert_eq!(
        the_room(&bench)
            .messages()
            .iter()
            .filter(|message| message.body.as_str() == "once")
            .count(),
        1
    );
}

/// The window's project door reads a room letter's recipient both before the
/// room is open and after: the first letter names its pane with no run to
/// file it in yet, and every later one is judged in the room — so a room
/// letter is never sent across projects unexamined.
#[test]
fn the_windows_project_door_sees_a_room_letters_pane_before_and_after_the_room_opens() {
    let mut bench = Bench::new();
    let line: Vec<String> = letter("pane:team-1/%8", "door");
    let actor = bench_actor(&bench.team.id, LONE);
    let first = crate::orchestration::project_scope::reach(
        &bench.ledger,
        &line,
        &bench.team.id,
        LONE,
        Some(&actor),
        A,
    );
    assert!(first.run.is_none(), "no room is open yet");
    assert_eq!(first.recipient_seats, ["team-1/%8"]);
    let launcher = on();
    answered(&send(
        &mut bench,
        &launcher,
        LONE,
        "pane:team-1/%8",
        "open-it",
    ));
    let room = the_room(&bench).id.clone();
    let later = crate::orchestration::project_scope::reach(
        &bench.ledger,
        &line,
        &bench.team.id,
        LONE,
        Some(&actor),
        A,
    );
    assert_eq!(
        later.run.as_ref().map(|(run, _)| run.as_str()),
        Some(room.as_str())
    );
    assert_eq!(later.recipient_seats, ["team-1/%8"]);
    // A pane of project B is not handed A's room as its acting run.
    let other = crate::orchestration::project_scope::reach(
        &bench.ledger,
        &line,
        &bench.team.id,
        OTHER,
        Some(&bench_actor(&bench.team.id, OTHER)),
        B,
    );
    assert!(other.run.is_none(), "B was handed A's room");
}

/// Every room verb that reaches a pane hands the window's project door that
/// pane — `send`, `ask`, and a `reply` to a room letter — so the door that
/// holds every letter to its project (`recipient_gate`, the window's half)
/// judges each one; no room verb carries its own boundary.
#[test]
fn every_room_verb_that_reaches_a_pane_hands_the_project_door_that_pane() {
    let mut bench = Bench::new();
    let launcher = on();
    let asked = answered(&typed(
        &mut bench,
        &launcher,
        OTHER,
        Some(A),
        &[
            "send",
            "--to",
            &format!("pane:team-1/{LONE}"),
            "--type",
            "question",
            "--body",
            "which?",
        ],
    ));
    let question = asked["messageId"].as_str().expect("a question").to_string();
    let other = format!("team-1/{OTHER}");
    let to_other = format!("pane:{other}");
    for line in [
        vec!["send", "--to", &to_other, "--type", "status", "--body", "x"],
        vec!["ask", "--to", &to_other, "--body", "x"],
        vec!["reply", "--to-message", &question, "--body", "this one"],
    ] {
        let argv: Vec<String> = line.iter().map(|word| (*word).to_string()).collect();
        let door = crate::orchestration::project_scope::reach(
            &bench.ledger,
            &argv,
            &bench.team.id,
            LONE,
            Some(&bench_actor(&bench.team.id, LONE)),
            A,
        );
        assert_eq!(door.recipient_seats, [other.as_str()], "{line:?}");
        assert!(door.run.is_some(), "{line:?}: judged in the room");
    }
}

/// `run-list` says which run is the room, so a coordinator reading it learns
/// the room answers mail only before it tries to sit in it; every other row
/// reads as it always did.
#[test]
fn run_list_says_which_run_is_the_room() {
    let mut bench = Bench::new();
    let launcher = on();
    // A run of this project, so the pane listing below sees it.
    let made = typed(
        &mut bench,
        &launcher,
        agent_teams::LEADER_PANE,
        Some(A),
        &["run-create", "--name", "work"],
    );
    let work = answered(&made)["runId"]
        .as_str()
        .expect("a run")
        .to_string();
    answered(&send(
        &mut bench,
        &launcher,
        LONE,
        "pane:team-1/%8",
        "open-it",
    ));
    let room = the_room(&bench).id.clone();
    let listed = answered(&typed(&mut bench, &launcher, LONE, Some(A), &["run-list"]));
    assert_eq!(
        row_id(&listed, &work),
        Some(work.as_str()),
        "the work run is listed: {listed}"
    );
    let row = |id: &str| {
        listed["runs"]
            .as_array()
            .expect("rows")
            .iter()
            .find(|row| row["runId"] == id)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        row(&room)["room"]["answers"][0],
        room::PEERS_VERB,
        "{listed}"
    );
    assert!(row(&work).get("room").is_none(), "{listed}");
}

/// The id of the row of `listed` that is `id`, when the list holds it.
fn row_id<'a>(listed: &'a serde_json::Value, id: &str) -> Option<&'a str> {
    listed["runs"]
        .as_array()?
        .iter()
        .find_map(|row| row["runId"].as_str().filter(|held| *held == id))
}

fn rooms(bench: &Bench) -> usize {
    bench
        .ledger
        .runs()
        .iter()
        .filter(|run| run.is_room())
        .count()
}

/// A pane whose folder the window does not know is sent with an empty
/// project: its letter is refused with the reason, and no room is opened —
/// not one per letter, not one nobody can read (review t-105450, 1).
#[test]
fn a_pane_whose_project_is_unknown_opens_no_room() {
    let mut bench = Bench::new();
    let launcher = on();
    for _ in 0..2 {
        let line = [
            "send",
            "--to",
            "pane:team-1/%8",
            "--type",
            "status",
            "--body",
            "x",
        ];
        let why = refused(&typed(&mut bench, &launcher, LONE, Some(""), &line));
        assert!(why.contains(room::PROJECT_UNKNOWN), "{why}");
    }
    assert_eq!(rooms(&bench), 0);
}

/// A first letter the room refuses leaves no room behind: a refused verb
/// changes nothing (review t-105450, 2).
#[test]
fn a_refused_first_letter_leaves_no_room() {
    let mut bench = Bench::new();
    let launcher = on();
    for line in [
        vec!["send", "--type", "status", "--body", "no-to"],
        vec![
            "send", "--to", "@all", "--type", "status", "--body", "crowd",
        ],
        vec![
            "send",
            "--to",
            "pane:team-1/%8",
            "--type",
            "worker_done",
            "--body",
            "{}",
        ],
        vec!["ask", "--body", "nobody named"],
    ] {
        refused(&typed(&mut bench, &launcher, LONE, Some(A), &line));
        assert_eq!(rooms(&bench), 0, "{line:?} left a room behind");
    }
    // And a room that holds letters is never taken back by a later refusal.
    answered(&send(&mut bench, &launcher, LONE, "pane:team-1/%8", "kept"));
    refused(&typed(
        &mut bench,
        &launcher,
        LONE,
        Some(A),
        &["send", "--type", "status", "--body", "x"],
    ));
    assert_eq!(rooms(&bench), 1);
}

/// The person's switch holds a letter that names the room by `--run` as much
/// as one the room was found for (review t-105450, 3).
#[test]
fn the_switch_off_holds_a_letter_that_names_the_room() {
    let mut bench = Bench::new();
    answered(&send(&mut bench, &on(), LONE, "pane:team-1/%8", "while-on"));
    let room = the_room(&bench).id.clone();
    let off = Catalog(&["claude"]);
    for line in [
        vec![
            "send",
            "--run",
            &room,
            "--to",
            "pane:team-1/%8",
            "--type",
            "status",
            "--body",
            "x",
        ],
        vec![
            "ask",
            "--run",
            &room,
            "--to",
            "pane:team-1/%8",
            "--body",
            "x",
        ],
    ] {
        let why = refused(&typed(&mut bench, &off, OTHER, Some(A), &line));
        assert!(why.contains(MAIL_OFF), "{line:?}: {why}");
    }
    // What the room holds can still be read.
    let read = answered(&typed(
        &mut bench,
        &off,
        "%8",
        Some(A),
        &["check", "--run", &room],
    ));
    assert!(
        read["messages"]
            .as_array()
            .is_some_and(|held| !held.is_empty()),
        "{read}"
    );
}

/// Every verb of the table that is not mail is refused in a room, by the
/// table rather than a hand-written list — a verb added later is held too.
/// The gate-reuse verbs that need a server's job context are refused before
/// any run is looked at, which holds them out of the room as well.
#[test]
fn every_verb_of_the_table_outside_the_rooms_is_refused_in_a_room() {
    let mut bench = Bench::new();
    let launcher = on();
    answered(&send(
        &mut bench,
        &launcher,
        LONE,
        "pane:team-1/%8",
        "open-it",
    ));
    let room = the_room(&bench).id.clone();
    let mut let_through = Vec::new();
    for (verb, _, _) in crate::orchestration::VERBS {
        if room::ROOM_VERBS.contains(verb) {
            continue;
        }
        for line in [
            vec![*verb, "--run", room.as_str()],
            vec![*verb, room.as_str()],
        ] {
            let planned = typed(&mut bench, &launcher, LONE, Some(A), &line);
            let why = &planned.reply.stderr;
            if planned.reply.exit_code == 0
                || !(why.contains("this project's room") || why.contains("out_of_scope"))
            {
                let_through.push(format!(
                    "{line:?}: {}",
                    planned.reply.stdout.chars().take(80).collect::<String>()
                ));
            }
        }
    }
    assert!(let_through.is_empty(), "{let_through:#?}");
    assert!(the_room(&bench).tasks.is_empty() && the_room(&bench).workers.is_empty());
}

/// Every letter a room's verbs write carries the room, wherever it is filed:
/// the letter in the room, the copy a coordinator's pane reads in its run,
/// the letter redirected to a worker's coordinator, and a live
/// coordinator's own letter written into the room by `--run`. A letter of
/// an ordinary run carries none (review t-105562, 1).
#[test]
fn every_letter_a_rooms_verbs_write_carries_the_room() {
    let mut bench = Bench::new();
    let run = bench.json("run-create --name work")["runId"]
        .as_str()
        .expect("a run")
        .to_string();
    let task = bench.json("task-create --spec x")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (_, worker_pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let ordinary = bench.json(&format!("send --to run:{run} --type status --body plain"));
    let launcher = on();
    let to_chair = format!("pane:team-1/{}", agent_teams::LEADER_PANE);
    let copied = answered(&send(
        &mut bench,
        &launcher,
        LONE,
        &to_chair,
        "for-the-chair",
    ));
    let redirected = answered(&send(
        &mut bench,
        &launcher,
        LONE,
        &format!("pane:team-1/{worker_pane}"),
        "for-the-worker",
    ));
    let room = the_room(&bench).id.clone();
    let line = [
        "send",
        "--run",
        room.as_str(),
        "--to",
        &format!("pane:team-1/{OTHER}"),
        "--type",
        "status",
        "--body",
        "from-the-chair",
    ];
    let chairs_own = answered(&typed(
        &mut bench,
        &launcher,
        agent_teams::LEADER_PANE,
        Some(A),
        &line,
    ));
    let marked = |run_id: &str, id: &str| {
        bench
            .ledger
            .run(run_id)
            .and_then(|held| held.message(id))
            .map(|message| message.room.clone())
    };
    let room_mark = Some(Some(room.clone()));
    assert_eq!(
        marked(&room, copied["messageId"].as_str().expect("id")),
        room_mark
    );
    assert_eq!(
        marked(
            &run,
            copied["deliveredTo"]["messageId"].as_str().expect("a copy")
        ),
        room_mark,
        "the copy in the coordinator's run"
    );
    assert_eq!(
        marked(&run, redirected["messageId"].as_str().expect("id")),
        room_mark,
        "the letter redirected to the worker's coordinator"
    );
    assert_eq!(
        marked(&room, chairs_own["messageId"].as_str().expect("id")),
        room_mark,
        "a live coordinator's letter written into the room"
    );
    assert_eq!(
        marked(&run, ordinary["messageId"].as_str().expect("id")),
        Some(None),
        "an ordinary run's letter"
    );
}

/// A room reply is held by the person's switch like any room letter: off,
/// two panes do not go on answering each other in a room (review t-105562).
#[test]
fn a_room_reply_is_refused_while_the_switch_is_off() {
    let mut bench = Bench::new();
    let sent = answered(&typed(
        &mut bench,
        &on(),
        LONE,
        Some(A),
        &[
            "send",
            "--to",
            &format!("pane:team-1/{OTHER}"),
            "--type",
            "question",
            "--body",
            "which?",
        ],
    ));
    let question = sent["messageId"].as_str().expect("a question").to_string();
    let off = Catalog(&["claude"]);
    let why = refused(&typed(
        &mut bench,
        &off,
        OTHER,
        Some(A),
        &["reply", "--to-message", &question, "--body", "this one"],
    ));
    assert!(why.contains(MAIL_OFF), "{why}");
}

/// A reply in the room to a letter a live coordinator wrote there leaves a
/// copy in the coordinator's run — and the copy carries the room too
/// (review t-105587).
#[test]
fn a_room_replys_copy_in_the_writers_run_carries_the_room() {
    let mut bench = Bench::new();
    let run = bench.json("run-create --name work")["runId"]
        .as_str()
        .expect("a run")
        .to_string();
    let launcher = on();
    answered(&send(
        &mut bench,
        &launcher,
        LONE,
        "pane:team-1/%8",
        "open-it",
    ));
    let room = the_room(&bench).id.clone();
    let to_other = format!("pane:team-1/{OTHER}");
    let line = [
        "send",
        "--run",
        room.as_str(),
        "--to",
        &to_other,
        "--type",
        "status",
        "--body",
        "from-the-chair",
    ];
    let written = answered(&typed(
        &mut bench,
        &launcher,
        agent_teams::LEADER_PANE,
        Some(A),
        &line,
    ));
    let letter = written["messageId"].as_str().expect("a letter").to_string();
    let replied = answered(&typed(
        &mut bench,
        &launcher,
        OTHER,
        Some(A),
        &["reply", "--to-message", &letter, "--body", "noted"],
    ));
    assert_eq!(replied["deliveredTo"]["runId"], run.as_str(), "{replied}");
    let copy = replied["deliveredTo"]["messageId"]
        .as_str()
        .expect("a copy");
    let held = bench
        .ledger
        .run(&run)
        .and_then(|held| held.message(copy))
        .expect("the copy");
    assert_eq!(held.room.as_deref(), Some(room.as_str()));
    assert_eq!(held.from, to_other, "the copy keeps the room's signature");
}
