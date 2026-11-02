//! t-68069: one conversation is one worker's.
//!
//! On 2026-10-21 two workers shared one checkout. After a tab closed, the
//! pane of one of them was continued and ran its neighbour's conversation, so
//! two processes wrote one transcript. These scenes are the ledger's half:
//! whatever seat a report comes by, a conversation another row still carries
//! on never becomes this worker's, and each sleeper comes back on its own.

use super::*;

const MINE: &str = "68069a17-0000-4000-8000-00000000000a";
const THEIRS: &str = "68069b04-0000-4000-8000-00000000000b";
const FRESH: &str = "68069c55-0000-4000-8000-00000000000c";
const CHECKOUT: &str = "/wt/main";

fn session(id: &str) -> ProviderSession {
    ProviderSession {
        key: SessionKey::SessionId,
        id: id.to_string(),
        transcript_path: None,
    }
}

/// A claude worker seated in the shared checkout, its pane having reported
/// `id` — answers the worker and its pane.
fn a_worker_on(bench: &mut Bench, id: &str) -> (String, String) {
    a_worker_in(bench, id, CHECKOUT)
}

/// [`a_worker_on`], seated in a folder of the test's own.
fn a_worker_in(bench: &mut Bench, id: &str, checkout: &str) -> (String, String) {
    let task = bench.json("task-create --spec shared-folder")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), checkout));
    assert!(
        bench
            .ledger
            .worker_session_reported(("team-1", &pane), session(id))
    );
    (worker, pane)
}

fn conversation_of(bench: &Bench, worker: &str) -> Option<String> {
    let at = bench.ledger.locate(worker).expect("the worker");
    bench.ledger.runs[at.0].workers[at.1]
        .session
        .as_ref()
        .map(|held| held.id.clone())
}

fn set_state(bench: &mut Bench, worker: &str, state: WorkerState) {
    let at = bench.ledger.locate(worker).expect("the worker");
    bench.ledger.runs[at.0].workers[at.1].state = state;
}

/// The 2026-10-21 scene: two workers in one folder, each with its own
/// conversation. A report at the first one's seat naming the second one's
/// conversation is refused and changes nothing; across a restart each
/// sleeper is awaited on its own conversation, and the neighbour's names
/// the neighbour.
#[test]
fn two_workers_in_one_folder_each_come_back_on_their_own_conversation() {
    let mut bench = Bench::new();
    bench.json("run-create --name shared-folder");
    let (mine, my_pane) = a_worker_on(&mut bench, MINE);
    let (theirs, _) = a_worker_on(&mut bench, THEIRS);

    assert!(
        bench
            .ledger
            .worker_session_reported(("team-1", &my_pane), session(THEIRS)),
        "the refusal wrote no letter"
    );
    assert_eq!(conversation_of(&bench, &mine).as_deref(), Some(MINE));
    assert_eq!(conversation_of(&bench, &theirs).as_deref(), Some(THEIRS));
    // The refusal is told to the coordinator once — the pane may still be
    // writing the neighbour's transcript — naming both workers and never the
    // conversation.
    let told: Vec<Message> = bench.ledger.runs()[0]
        .messages()
        .iter()
        .filter(|one| one.from == LEDGER_ITSELF && one.kind == MessageKind::Status)
        .cloned()
        .collect();
    assert_eq!(told.len(), 1, "the refusal was told {} times", told.len());
    let said: serde_json::Value = serde_json::from_str(told[0].body.as_str()).expect("json");
    assert_eq!(said["reason"], CONVERSATION_HELD_ELSEWHERE);
    assert_eq!(
        (said["workerId"].as_str(), said["holderId"].as_str()),
        (Some(mine.as_str()), Some(theirs.as_str()))
    );
    assert!(!told[0].body.as_str().contains(THEIRS) && !told[0].subject.as_str().contains(THEIRS));
    assert!(
        !bench
            .ledger
            .worker_session_reported(("team-1", &my_pane), session(THEIRS)),
        "the same refusal moved the ledger again"
    );
    assert_eq!(
        bench.ledger.runs()[0]
            .messages()
            .iter()
            .filter(|one| one.from == LEDGER_ITSELF && one.kind == MessageKind::Status)
            .count(),
        1,
        "the same refusal was told twice"
    );
    assert_eq!(
        bench
            .ledger
            .worker_of_conversation("claude", THEIRS)
            .map(|worker| worker.id.clone()),
        Some(theirs.clone())
    );

    assert_eq!(bench.ledger.window_exiting(2_000).sleeping, 2);
    assert_eq!(
        bench
            .ledger
            .sleeper_awaiting(CHECKOUT, "claude", MINE)
            .map(|worker| worker.id.clone()),
        Some(mine.clone()),
        "this worker's own conversation does not bring it back"
    );
    assert_eq!(
        bench
            .ledger
            .sleeper_awaiting(CHECKOUT, "claude", THEIRS)
            .map(|worker| worker.id.clone()),
        Some(theirs),
        "the neighbour's conversation brings back somebody else"
    );
}

/// The refusal is about a conversation another row CARRIES ON. A pane whose
/// agent was restarted on a conversation of its own still moves to it, and
/// a conversation whose worker has ended may be continued in another pane:
/// nobody else is writing it.
#[test]
fn a_new_conversation_or_an_ended_workers_one_is_still_taken() {
    let mut bench = Bench::new();
    bench.json("run-create --name taken-over");
    let (mine, my_pane) = a_worker_on(&mut bench, MINE);
    let (theirs, _) = a_worker_on(&mut bench, THEIRS);

    assert!(
        bench
            .ledger
            .worker_session_reported(("team-1", &my_pane), session(FRESH)),
        "a restart on a fresh conversation was refused"
    );
    assert_eq!(conversation_of(&bench, &mine).as_deref(), Some(FRESH));

    for (state, carries_on) in [
        (WorkerState::Orphaned, true),
        (WorkerState::Sleeping, true),
        (WorkerState::Reclaimable, false),
        (WorkerState::Released, false),
    ] {
        set_state(&mut bench, &theirs, state);
        // The answer is whether the ledger moved — a refusal moves it too,
        // the first time, by its letter — so the conversation written down
        // is what says whether the report was taken.
        bench
            .ledger
            .worker_session_reported(("team-1", &my_pane), session(THEIRS));
        let taken = conversation_of(&bench, &mine).as_deref() == Some(THEIRS);
        assert_eq!(
            taken,
            !carries_on,
            "a neighbour {} — its conversation taken: {taken}",
            state.as_str()
        );
        if carries_on {
            assert_eq!(conversation_of(&bench, &mine).as_deref(), Some(FRESH));
        } else {
            assert_eq!(conversation_of(&bench, &mine).as_deref(), Some(THEIRS));
            assert!(
                bench
                    .ledger
                    .worker_session_reported(("team-1", &my_pane), session(FRESH))
            );
        }
    }
}

/// t-68069 (2): a pane seated in a checkout another worker still holds a
/// pane in is told who it shares it with — every run's rows, never itself,
/// never a worker released or asleep — so the summons that made two
/// workers in one folder says so. A checkout of its own shares nothing.
#[test]
fn a_worker_seated_in_a_neighbours_checkout_is_told_who_it_shares_it_with() {
    let mut bench = Bench::new();
    let run = bench.json("run-create --name shared-checkout")["runId"]
        .as_str()
        .expect("a run")
        .to_string();
    let (first, _) = a_worker_on(&mut bench, MINE);
    assert!(
        bench
            .ledger
            .workers_sharing_checkout(CHECKOUT, &first)
            .is_empty(),
        "a worker alone in its checkout was told of a neighbour"
    );
    let (second, _) = a_worker_on(&mut bench, THEIRS);
    assert_eq!(
        bench.ledger.workers_sharing_checkout(CHECKOUT, &second),
        vec![(run.clone(), first.clone(), "claude".to_string())]
    );
    assert_eq!(
        bench.ledger.workers_sharing_checkout(CHECKOUT, &first),
        vec![(run.clone(), second.clone(), "claude".to_string())]
    );
    assert!(
        bench
            .ledger
            .workers_sharing_checkout("/wt/its-own", &second)
            .is_empty()
    );
    for gone in [WorkerState::Released, WorkerState::Sleeping] {
        set_state(&mut bench, &first, gone);
        assert!(
            bench
                .ledger
                .workers_sharing_checkout(CHECKOUT, &second)
                .is_empty(),
            "a {gone:?} worker holds no pane to share"
        );
    }
}

/// One folder spelled two ways is still one folder (t-85366): a trailing
/// slash, whether or not the folder is there to ask, and a link to it.
#[test]
fn a_neighbour_in_one_folder_is_found_however_the_folder_is_spelled() {
    let mut bench = Bench::new();
    bench.json("run-create --name one-folder");
    let (first, _) = a_worker_on(&mut bench, MINE);
    let (second, _) = a_worker_in(&mut bench, THEIRS, &format!("{CHECKOUT}/"));
    assert_eq!(
        bench
            .ledger
            .workers_sharing_checkout(&format!("{CHECKOUT}/"), &second)
            .first()
            .map(|(_, worker, _)| worker.clone()),
        Some(first.clone()),
        "a slash on the end of what was asked made two folders"
    );
    assert_eq!(
        bench
            .ledger
            .workers_sharing_checkout(CHECKOUT, &first)
            .first()
            .map(|(_, worker, _)| worker.clone()),
        Some(second),
        "a slash on the end of a row's folder that is not there made two folders"
    );
    assert!(
        bench
            .ledger
            .workers_sharing_checkout("/wt/main-too", &first)
            .is_empty(),
        "a folder whose name starts the same was called the same folder"
    );
}

#[cfg(unix)]
#[test]
fn a_neighbour_is_found_through_a_link_to_its_folder() {
    let root = tempfile::tempdir().expect("a scratch folder");
    let real = root.path().join("real");
    std::fs::create_dir(&real).expect("the folder");
    let link = root.path().join("link");
    std::os::unix::fs::symlink(&real, &link).expect("the link");
    let (real, link) = (
        real.to_str().expect("a path"),
        link.to_str().expect("a path"),
    );
    let slashed = format!("{link}/");
    let mut bench = Bench::new();
    bench.json("run-create --name one-folder");
    let (first, _) = a_worker_in(&mut bench, MINE, real);
    let (second, _) = a_worker_in(&mut bench, THEIRS, link);
    for (asked, worker, neighbour) in [
        (real, &second, &first),
        (link, &first, &second),
        (slashed.as_str(), &first, &second),
    ] {
        assert_eq!(
            bench
                .ledger
                .workers_sharing_checkout(asked, worker)
                .first()
                .map(|(_, one, _)| one),
            Some(neighbour),
            "asked of {asked}"
        );
    }
    // Another folder is another folder.
    let other = root.path().join("other");
    std::fs::create_dir(&other).expect("the other folder");
    assert!(
        bench
            .ledger
            .workers_sharing_checkout(other.to_str().expect("a path"), "nobody")
            .is_empty()
    );
}

/// t-68069 ③: a pane that re-entered a conversation nobody named has its
/// coordinator told which, once per conversation, by its first letters and
/// never the whole id. A seat with no worker has nobody to tell.
#[test]
fn a_conversation_nobody_named_is_told_once_by_its_first_letters() {
    let mut bench = Bench::new();
    bench.json("run-create --name picked");
    let (mine, pane) = a_worker_on(&mut bench, MINE);
    let told = |bench: &Bench| -> Vec<Message> {
        bench.ledger.runs()[0]
            .messages()
            .iter()
            .filter(|one| {
                one.from == LEDGER_ITSELF
                    && one.kind == MessageKind::Status
                    && one.body.as_str().contains(CONVERSATION_PICKED_UNASKED)
            })
            .cloned()
            .collect()
    };

    assert!(
        bench
            .ledger
            .worker_conversation_picked(("team-1", &pane), &session(FRESH), 1_000)
    );
    let letters = told(&bench);
    assert_eq!(letters.len(), 1);
    let said: serde_json::Value = serde_json::from_str(letters[0].body.as_str()).expect("json");
    assert_eq!(said["workerId"].as_str(), Some(mine.as_str()));
    assert_eq!(said["conversation"].as_str(), Some(&FRESH[..8]));
    assert!(
        !letters[0].body.as_str().contains(FRESH) && !letters[0].subject.as_str().contains(FRESH),
        "the whole id reached the letter"
    );

    assert!(
        !bench
            .ledger
            .worker_conversation_picked(("team-1", &pane), &session(FRESH), 2_000),
        "the same conversation was told twice"
    );
    assert!(
        bench
            .ledger
            .worker_conversation_picked(("team-1", &pane), &session(THEIRS), 3_000),
        "another conversation was not told"
    );
    assert!(
        !bench.ledger.worker_conversation_picked(
            ("team-1", "%no-such-pane"),
            &session(FRESH),
            4_000
        ),
        "a seat with no worker told somebody"
    );
    assert_eq!(told(&bench).len(), 2);
}

/// t-68069 P3-3: eight letters of a zo id are always the word every zo id opens
/// with. The letter names the conversation by what follows that word, and two
/// conversations of one worker are told apart by the whole id. Two ids a line
/// would spell alike are still two conversations.
#[test]
fn two_zo_conversations_of_one_worker_are_two_letters_that_read_apart() {
    const FIRST: &str = "session-1792758210243-0";
    const SECOND: &str = "session-1792758210243-1";
    let mut bench = Bench::new();
    bench.json("run-create --name picked-zo");
    let (_, pane) = a_worker_on(&mut bench, MINE);
    let said = |bench: &Bench| -> Vec<String> {
        bench.ledger.runs()[0]
            .messages()
            .iter()
            .filter(|one| one.body.as_str().contains(CONVERSATION_PICKED_UNASKED))
            .map(|one| {
                serde_json::from_str::<serde_json::Value>(one.body.as_str()).expect("json")
                    ["conversation"]
                    .as_str()
                    .expect("its words")
                    .to_string()
            })
            .collect()
    };
    for (at, id) in [FIRST, SECOND].into_iter().enumerate() {
        assert!(
            bench.ledger.worker_conversation_picked(
                ("team-1", &pane),
                &session(id),
                1_000 + i64::try_from(at).expect("two")
            ),
            "{id} was taken for a conversation already told"
        );
    }
    assert_eq!(said(&bench), ["1792758210243-0", "1792758210243-1"]);

    // Past what a line carries of a zo id, two ids still differ.
    let long = |tail: &str| format!("session-{}-{tail}", "1".repeat(40));
    assert!(bench.ledger.worker_conversation_picked(
        ("team-1", &pane),
        &session(&long("a")),
        2_000
    ));
    assert!(
        bench
            .ledger
            .worker_conversation_picked(("team-1", &pane), &session(&long("b")), 2_001),
        "two conversations a line spells alike were counted as one"
    );
}
