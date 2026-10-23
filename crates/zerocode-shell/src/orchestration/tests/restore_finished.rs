//! t-57717: the window does not bring a finished worker's conversation back
//! on its own.
//!
//! On 2026-10-15 and 16 three finished workers' conversations came back as
//! agent panes nobody had called — `term 33 resumed claude <id> in
//! …/t-54563`, 16:27:46; `term 21 … in …/t-55838-url`, 19:15:22; `term 15 …
//! in …/t-56480-zo`, 00:03:24 — each on the first visit to its checkout after
//! a restart. The window read the conversation off its stored layout and
//! asked the ledger only whether it was a sleeper; a finished worker is not,
//! so it was opened as a person's tab.
//!
//! Driven through a private window's production doors, as `restore_door.rs`
//! is: the door's wake on each road (`wake_conversation` behind the fake
//! launcher), the stored set a visit reads and the one a save writes
//! (`cmd::settings`), and the ledger's own reseat beside them. A CLI counted
//! here is one a real entry point started.

use super::restore::{
    Restoring, a_run, a_worker, census_without_commands, row, say, the_window_goes,
};
use super::restore_door::{Did, Door, coordinator_back, door_team, the_next_boot};
use super::*;
use crate::conversation_wake::{ConversationWake, WakeRoad};

/// The four roads that read a conversation off the window's stored record.
const STORED_RECORD_ROADS: [WakeRoad; 4] = [
    WakeRoad::RestoreFront,
    WakeRoad::RestoreBehind,
    WakeRoad::Visit,
    WakeRoad::Stage,
];

/// One stored tab whose one leaf holds `agent`'s conversation `id`, as the
/// window saved it: the eye on it, and standing when the window went.
fn record(agent: &str, id: &str) -> crate::pane_layout::TabLayout {
    serde_json::from_value(serde_json::json!({
        "root": { "type": "leaf" },
        "agents": { "0": { "agent": agent, "key": "session_id", "id": id } },
        "running": { "0": agent },
        "focused": true,
        "awake": true,
    }))
    .expect("a stored tab")
}

/// A stored plain shell.
fn shell() -> crate::pane_layout::TabLayout {
    serde_json::from_value(serde_json::json!({ "root": { "type": "leaf" } }))
        .expect("a stored shell")
}

/// The line a door's wake files for the folder it opened in — the session,
/// whole, then ` in ` — and nothing else.
fn placed(door: &Door, session: &str) -> Vec<String> {
    door.noted(&format!("{session} in "))
}

/// A line a test has just held to its shape, printed for the report's
/// examples (run with `--nocapture`).
fn shown(line: &str) {
    println!("LINE t-57717 {line}");
}

/// The processes a door started that re-enter a conversation: the
/// `--resume` launches the three 2026-10-15/16 lines were.
fn resumes(door: &Door) -> usize {
    door.started()
        .iter()
        .filter(|(_, command)| command.contains("--resume"))
        .count()
}

/// t-57717 red ①: a finished worker's conversation, asked for on each road
/// that read it off a stored record — the boot's front and behind, a first
/// visit, a sleeping tab on the stage — starts nothing: no launch built, no
/// process, the answer names the worker, and one line per road says why,
/// with the road and the ledger's word. A person's pick by name (`reopen`)
/// still opens it, as their own tab: no worker seated, the row released.
/// Before, every road started it — `Did::Started` once each — and the line
/// said neither road nor ledger.
#[test]
fn a_finished_workers_conversation_off_a_stored_record_starts_nothing() {
    const LEADER: u32 = 577_100;
    const WORKER: u32 = 577_101;
    const DOOR: u32 = 577_110;
    const REOPENED: u32 = 577_119;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "finished");
    let session = "57717b0c-0000-4000-8000-000000000001";
    let (_task, worker, _term) = a_worker(&host, &team, "--agent claude", Some(session));
    say(
        &host,
        &team,
        &format!("worker-stop --worker {worker} --reason handed-in"),
    );
    assert_eq!(row(&worker).state, WorkerState::Released);

    let door = Door::new(checkout.path());
    let mut wrong = Vec::new();
    for (road, term) in STORED_RECORD_ROADS.into_iter().zip(DOOR..) {
        door.on_road(road);
        let woke = door.wake(term, "claude", session);
        if woke != Ok(ConversationWake::finished(term, worker.clone())) {
            wrong.push(format!("{}: answered {woke:?}", road.word()));
        }
        if door.did.lock().unwrap().contains(&Did::Prepared(term)) {
            wrong.push(format!("{}: a launch was built in t{term}", road.word()));
        }
        let said = door.noted(&format!("road={} ledger=released:{worker}", road.word()));
        if said.len() != 1
            || !said[0].starts_with(&format!("term {term} did not resume claude 57717b0c"))
        {
            wrong.push(format!("{}: its line {said:?}", road.word()));
        }
    }
    assert!(
        wrong.is_empty(),
        "{wrong:#?}\n{:#?}",
        door.did.lock().unwrap()
    );
    assert!(
        door.started().is_empty(),
        "a finished worker's conversation was started off a stored record: {:?}",
        door.started()
    );
    assert_eq!(row(&worker).state, WorkerState::Released);
    for line in door.noted(" did not resume ") {
        shown(&line);
    }

    // A person's pick by name still opens it — the person's own tab.
    door.on_road(WakeRoad::Reopen);
    let opened = door
        .wake(REOPENED, "claude", session)
        .expect("the person's wake");
    assert_eq!(opened, ConversationWake::opened(REOPENED));
    assert_eq!(door.started().len(), 1, "{:?}", door.started());
    assert_eq!(
        door.seated_at_start(REOPENED),
        None,
        "the reopened conversation was seated as the finished worker"
    );
    let line = placed(&door, session);
    assert_eq!(line.len(), 1, "{line:?}");
    assert!(
        line[0].ends_with(&format!("road=reopen ledger=released:{worker}")),
        "{line:?}"
    );
    shown(&line[0]);
    assert_eq!(row(&worker).state, WorkerState::Released);
    for pane in [REOPENED, WORKER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-57717 red ①, the control: a sleeper's conversation asked for on a
/// stored-record road comes back as the same worker, seated before its
/// process starts — the road the decline must not close — and its line says
/// the road and `sleeping`.
#[test]
fn a_sleepers_conversation_off_a_stored_record_is_seated_as_the_same_worker() {
    const LEADER: u32 = 577_120;
    const WORKER: u32 = 577_121;
    const DOOR: u32 = 577_128;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "asleep");
    let session = "57717d0c-0000-4000-8000-000000000001";
    let (_task, worker, term) = a_worker(&host, &team, "--agent claude", Some(session));
    the_window_goes(&census_without_commands, &[term]);
    the_next_boot(LEADER);
    assert_eq!(row(&worker).state, WorkerState::Sleeping);

    let door = Door::new(checkout.path());
    door.on_road(WakeRoad::Visit);
    door.wake(DOOR, "claude", session)
        .expect("the sleeper's wake");
    assert_eq!(door.started().len(), 1, "{:?}", door.started());
    assert_eq!(
        door.seated_at_start(DOOR).as_deref(),
        Some(worker.as_str()),
        "the sleeper's process started before its seat"
    );
    let now = row(&worker);
    assert_eq!(now.state, WorkerState::Active);
    assert_eq!(now.team, door_team(DOOR));
    let line = placed(&door, session);
    assert_eq!(line.len(), 1, "{line:?}");
    assert!(
        line[0].ends_with(&format!("road=visit ledger=sleeping:{worker}")),
        "{line:?}"
    );
    shown(&line[0]);
    for pane in [DOOR, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-57717 red ①, the other control: a conversation no worker ever reported
/// — a person's own — comes back off a stored record as it always has, the
/// person's tab, handed to the ledger for a run's seat (t-20088), and its
/// line says the road and `none`.
#[test]
fn a_conversation_no_worker_reported_comes_back_off_a_stored_record() {
    const DOOR: u32 = 577_130;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the person's checkout");
    let session = "57717e0c-0000-4000-8000-000000000001";
    let door = Door::new(checkout.path());
    door.on_road(WakeRoad::Visit);

    let woke = door.wake(DOOR, "claude", session).expect("the wake");

    assert_eq!(woke, ConversationWake::opened(DOOR));
    assert_eq!(door.started().len(), 1, "{:?}", door.started());
    assert_eq!(door.seated_at_start(DOOR), None);
    assert!(door.did.lock().unwrap().contains(&Did::Rebound(DOOR)));
    let line = placed(&door, session);
    assert_eq!(line.len(), 1, "{line:?}");
    assert!(line[0].ends_with("road=visit ledger=none"), "{line:?}");
    shown(&line[0]);
    crate::agent_teams::forget_term(DOOR);
}

/// t-57717 red ②: what a first visit reads off the file holds no finished
/// worker's conversation — the finished worker's checkout, whose set held
/// nothing else, reads as one that stored nothing (so it opens as the person
/// chose for a new workspace, t-19779) — while the person's checkout reads
/// its own conversation as stored; the boot puts back behind the front only
/// the person's; and the read takes nothing out of the file itself.
#[test]
fn a_first_visit_reads_no_finished_workers_conversation_off_the_file() {
    const LEADER: u32 = 577_140;
    const WORKER: u32 = 577_141;
    let (_window, _store) = PrivateWindow::boot();
    let root = tempfile::tempdir().expect("the checkouts");
    let worker_checkout = root.path().join("t-54563");
    let persons_checkout = root.path().join("main");
    for folder in [&worker_checkout, &persons_checkout] {
        std::fs::create_dir_all(folder).expect("a checkout");
    }
    let at = |folder: &std::path::Path| folder.to_string_lossy().into_owned();
    let host = Restoring::new(&worker_checkout, WORKER, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "visited");
    let session = "57717f0c-0000-4000-8000-000000000001";
    let persons = "57717f0c-0000-4000-8000-0000000000aa";
    let (_task, worker, _term) = a_worker(&host, &team, "--agent claude", Some(session));
    say(
        &host,
        &team,
        &format!("worker-stop --worker {worker} --reason handed-in"),
    );
    let file = root.path().join("pane-layouts.json");
    crate::pane_layout::rewrite(&file, |held| {
        crate::pane_layout::store(held, at(&worker_checkout), vec![record("claude", session)]);
        crate::pane_layout::store(held, at(&persons_checkout), vec![record("claude", persons)]);
        true
    })
    .expect("the stored sets");

    let visit = crate::cmd::settings::opened_set(&file, &at(&worker_checkout));
    assert_eq!(visit.struck, vec![worker.clone()]);
    assert!(
        visit.clone().kept().is_empty(),
        "the finished worker's checkout still opens with its conversation: {visit:?}"
    );
    let own = crate::cmd::settings::opened_set(&file, &at(&persons_checkout));
    assert!(own.struck.is_empty(), "{own:?}");
    let own = own.kept();
    assert_eq!(own.len(), 1);
    assert_eq!(own[0].agents[&0].id, persons);
    assert_eq!(
        crate::cmd::settings::standing_set(&file),
        vec![at(&persons_checkout)],
        "the boot would put back the finished worker's checkout behind the front"
    );
    assert_eq!(
        crate::pane_layout::read(&file)[&at(&worker_checkout)].len(),
        1,
        "the read wrote the file"
    );
    for pane in [WORKER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-57717 red ③: the window's own save writes no worker's conversation —
/// one at work, one finished — whatever its pane was seated as, so a worker
/// pane a tab held without the ledger's mark (a detached worker row pressed,
/// a worker listed and opened) cannot come back off the file after its
/// worker finished. The person's conversation and a plain shell are saved
/// as they stand, and each tab keeps its place in the answer, for the names
/// the file hands back.
#[test]
fn a_save_writes_no_workers_conversation_and_keeps_the_persons() {
    const LEADER: u32 = 577_150;
    const FIRST: u32 = 577_151;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the checkout");
    let host = Restoring::new(checkout.path(), FIRST, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "saved");
    let working = "57717a1c-0000-4000-8000-000000000001";
    let finished = "57717a1c-0000-4000-8000-000000000002";
    let persons = "57717a1c-0000-4000-8000-0000000000aa";
    let (_task, at_work, _) = a_worker(&host, &team, "--agent claude", Some(working));
    let (_task, done, _) = a_worker(&host, &team, "--agent codex", Some(finished));
    say(
        &host,
        &team,
        &format!("worker-stop --worker {done} --reason handed-in"),
    );

    let saved = crate::cmd::settings::saved_set(vec![
        record("claude", working),
        record("codex", finished),
        record("claude", persons),
        shell(),
    ]);

    assert_eq!(saved.struck, vec![at_work, done]);
    assert_eq!(saved.tabs.len(), 4, "a tab's place in the answer moved");
    assert_eq!(saved.tabs[0], None);
    assert_eq!(saved.tabs[1], None);
    assert_eq!(saved.tabs[2], Some(record("claude", persons)));
    assert_eq!(saved.tabs[3], Some(shell()));
    for pane in [FIRST, FIRST + 1, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-57717 ③, the seat: a conversation that holds a run's coordinator seat
/// is the person's, whatever worker row once carried it — a save keeps it,
/// while a finished worker's conversation saved beside it is struck (so the
/// ledger was read, and the seat is what kept the first).
#[test]
fn a_save_keeps_a_conversation_that_holds_a_runs_seat() {
    const LEADER: u32 = 577_190;
    const FIRST: u32 = 577_191;
    const SEATED: u32 = 577_195;
    let (_window, _store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the checkout");
    let host = Restoring::new(checkout.path(), FIRST, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "seat-kept");
    // The conversation the second run's coordinator is (`test_actor`),
    // carried first by a worker of the first run, which finished.
    let seat = format!("test-session-of-term-{SEATED}");
    let finished = "57717a2c-0000-4000-8000-000000000001";
    let (_task, carried, _) = a_worker(&host, &team, "--agent claude", Some(&seat));
    let (_task, done, _) = a_worker(&host, &team, "--agent claude", Some(finished));
    for worker in [&carried, &done] {
        say(
            &host,
            &team,
            &format!("worker-stop --worker {worker} --reason handed-in"),
        );
    }
    let (seated_team, _seated_run) = a_run(&host, SEATED, "seat-held");
    let current: serde_json::Value =
        serde_json::from_str(&say(&host, &seated_team, "run-current")).expect("run-current");
    assert_eq!(current["seated"], true, "{current}");

    let saved =
        crate::cmd::settings::saved_set(vec![record("claude", &seat), record("claude", finished)]);

    assert_eq!(saved.struck, vec![done], "{saved:?}");
    assert_eq!(saved.tabs[0], Some(record("claude", &seat)));
    assert_eq!(saved.tabs[1], None);
    for pane in [FIRST, FIRST + 1, LEADER, SEATED] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-57717 ②, a ledger that does not answer: the decline is the ledger's
/// word, so without one nothing new stops a wake — the unseen-exit hold that
/// already refuses every door while the ledger cannot be read (t-7538, astra
/// R3-1) answers as before — and the line names it, `ledger=unread`. Once
/// the ledger reads again, the same door declines the finished worker's
/// conversation.
#[test]
fn a_ledger_that_cannot_be_read_is_named_unread_and_the_decline_waits_for_it() {
    const LEADER: u32 = 577_200;
    const WORKER: u32 = 577_201;
    const DOOR: u32 = 577_208;
    let (_window, store) = PrivateWindow::boot();
    let checkout = tempfile::tempdir().expect("the worker's checkout");
    let host = Restoring::new(checkout.path(), WORKER, (0, test_actor(LEADER)));
    let (team, _run) = a_run(&host, LEADER, "unread");
    let session = "57717a3c-0000-4000-8000-000000000001";
    let (_task, worker, _term) = a_worker(&host, &team, "--agent claude", Some(session));
    say(
        &host,
        &team,
        &format!("worker-stop --worker {worker} --reason handed-in"),
    );
    let door = Door::new(checkout.path());
    door.on_road(WakeRoad::Visit);

    let writer = store.fault_connection_for_tests().expect("the store");
    writer
        .execute_batch("BEGIN IMMEDIATE")
        .expect("the store's write lock, held");
    let unread = door.wake(DOOR, "claude", session);
    writer
        .execute_batch("ROLLBACK")
        .expect("the store's write lock let go");
    drop(writer);

    let refused = unread.expect_err("a wake opened on a ledger that could not be read");
    assert!(refused.contains("원장을 읽지 못해"), "{refused}");
    assert!(door.started().is_empty(), "{:?}", door.started());
    let said = door.noted("road=visit ledger=unread");
    assert_eq!(said.len(), 1, "{:#?}", door.did.lock().unwrap());
    shown(&said[0]);

    let read = door.wake(DOOR + 1, "claude", session);
    assert_eq!(read, Ok(ConversationWake::finished(DOOR + 1, worker)));
    assert!(door.started().is_empty(), "{:?}", door.started());
    for pane in [DOOR, DOOR + 1, WORKER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}

/// t-57717 ⑥, the numbers: the three 2026-10-15/16 scenes replayed on the
/// road they took — a first visit to a finished worker's checkout after a
/// restart (t-54563 16:27:46, t-55838-url 19:15:22, t-56480-zo 00:03:24) —
/// and the 19:13:26 scene beside them, the ledger's own reseat of the third
/// worker while it slept, before it finished. A visit reads the checkout's
/// stored set off the pane file (`pane_layouts`) and wakes every
/// conversation left in it through the door on its road (`spawnStoredLeaf`);
/// the door is also handed each scene's record as it stood, as a tab read
/// before its worker finished would hand it. Counted: the visits whose read
/// still held the finished worker's conversation, the `--resume` launches
/// off those reads, and the door's own — 3, 3 and 3 before, 0, 0 and 0 now;
/// the sleeper's reseat, 1 before and 1 now.
#[test]
fn the_three_unsummoned_resumes_are_none_and_the_sleepers_reseat_stays_one() {
    const LEADER: u32 = 577_160;
    const NEW_LEADER: u32 = 577_169;
    const DOORS: u32 = 577_180;
    let (_window, _store) = PrivateWindow::boot();
    let root = tempfile::tempdir().expect("the checkouts");
    let shared = root.path().join("repository").join(".git");
    std::fs::create_dir_all(&shared).expect("the shared repository");
    let scenes = [
        ("t-54563", "57717c01-0000-4000-8000-000000000001", 577_161),
        (
            "t-55838-url",
            "57717c02-0000-4000-8000-000000000002",
            577_163,
        ),
        (
            "t-56480-zo",
            "57717c03-0000-4000-8000-000000000003",
            577_165,
        ),
    ];
    let checkout = |name: &str| {
        let folder = root.path().join(name);
        std::fs::create_dir_all(&folder).expect("a checkout");
        // These scenes visit three worktrees of one project. Their shared
        // Git identity must be present just as it is for the real checkouts.
        let own = shared.join("worktrees").join(name);
        std::fs::create_dir_all(&own).expect("the linked worktree metadata");
        std::fs::write(own.join("commondir"), "../..\n").expect("the shared directory");
        std::fs::write(folder.join(".git"), format!("gitdir: {}\n", own.display()))
            .expect("the worktree pointer");
        folder
    };
    let at = |name: &str| root.path().join(name).to_string_lossy().into_owned();
    let hosts: Vec<Restoring> = scenes
        .iter()
        .map(|&(name, _, first)| {
            Restoring::new(&checkout(name), first, (NEW_LEADER, test_actor(LEADER)))
        })
        .collect();
    let (team, _run) = a_run(&hosts[0], LEADER, "replay");
    let workers: Vec<(String, u32)> = scenes
        .iter()
        .zip(&hosts)
        .map(|(&(_, session, _), host)| {
            let (_task, worker, term) = a_worker(host, &team, "--agent claude", Some(session));
            (worker, term)
        })
        .collect();
    // The first two finished before the restart that brought them back.
    for (worker, _) in &workers[..2] {
        say(
            &hosts[0],
            &team,
            &format!("worker-stop --worker {worker} --reason handed-in"),
        );
    }
    // 19:13: the restart — the third worker asleep, its coordinator back,
    // and the ledger's own reseat bringing it back as itself.
    the_window_goes(&census_without_commands, &[workers[2].1]);
    the_next_boot(LEADER);
    let reseated = coordinator_back(&hosts[2], "replay", LEADER, NEW_LEADER);
    assert_eq!(row(&workers[2].0).state, WorkerState::Active);
    // 23:52: it finishes too, from its coordinator's new seat.
    say(
        &hosts[2],
        &format!("team-t7812-replay-back-{NEW_LEADER}"),
        &format!("worker-stop --worker {} --reason handed-in", workers[2].0),
    );
    for (worker, _) in &workers {
        assert_eq!(row(worker).state, WorkerState::Released, "{worker}");
    }

    // The pane file as the three visits found it: each checkout's one tab,
    // its one leaf the finished worker's conversation.
    let file = root.path().join("pane-layouts.json");
    crate::pane_layout::rewrite(&file, |held| {
        for &(name, session, _) in &scenes {
            crate::pane_layout::store(held, at(name), vec![record("claude", session)]);
        }
        true
    })
    .expect("the stored sets");

    // Each first visit after a restart: the read, then the door the window
    // takes for every conversation the read left — and the door alone,
    // handed the record as it stood.
    let (mut read, mut off_read, mut alone) = (0, 0, 0);
    for (scene, ((&(name, session, _), visit), handed)) in scenes
        .iter()
        .zip((DOORS..).step_by(2))
        .zip((DOORS + 1..).step_by(2))
        .enumerate()
    {
        let set = crate::cmd::settings::opened_set(&file, &at(name)).kept();
        let held: Vec<_> = set.iter().flat_map(|tab| tab.agents.values()).collect();
        read += usize::from(held.iter().any(|wake| wake.id == session));
        let door = Door::new(&root.path().join(name));
        door.on_road(WakeRoad::Visit);
        for wake in held {
            let woke = door.wake(visit, &wake.agent, &wake.id);
            assert!(woke.is_ok(), "scene {scene} ({name}), the visit: {woke:?}");
        }
        off_read += resumes(&door);
        let door = Door::new(&root.path().join(name));
        door.on_road(WakeRoad::Visit);
        let woke = door.wake(handed, "claude", session);
        assert!(woke.is_ok(), "scene {scene} ({name}), the door: {woke:?}");
        alone += resumes(&door);
        for term in [visit, handed] {
            crate::agent_teams::forget_term(term);
        }
    }
    println!(
        "MEASURE t-57717 first visits to a finished worker's checkout, three scenes: reads that \
         held its conversation {read}/3; --resume launched off those reads {off_read}/3; \
         --resume launched by the door handed the stored record {alone}/3; the sleeper's reseat \
         by the ledger {reseated}/1"
    );
    assert_eq!(
        (read, off_read, alone),
        (0, 0, 0),
        "unsummoned resumes on a first visit"
    );
    assert_eq!(reseated, 1, "the sleeper's reseat");
    for host in &hosts {
        for (term, _) in host.cuts() {
            crate::agent_teams::forget_term(term);
        }
    }
    for pane in [NEW_LEADER, LEADER] {
        crate::agent_teams::forget_term(pane);
    }
}
