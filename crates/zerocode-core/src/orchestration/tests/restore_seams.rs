//! t-7812: the transitions the restore roads added to the ledger — a
//! sleeper that cannot come back told now, and the launch words a sleeper's
//! conversation is resumed with.

use super::*;

fn a_sleeper(bench: &mut Bench, line: &str, session: Option<&str>) -> (String, String, String) {
    bench.actor = Some(bench_actor("team-1", "%1"));
    bench.json("run-create --name sleepers");
    let task = bench.json("task-create --spec keep-going")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (worker, pane) = bench.seat(&format!("{line} --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), "/wt/sleeper"));
    if let Some(id) = session {
        assert!(bench.ledger.worker_session_reported(
            ("team-1", &pane),
            ProviderSession {
                key: SessionKey::SessionId,
                id: id.to_string(),
                transcript_path: None,
            },
        ));
    }
    let dispatch = bench.ledger.runs()[0]
        .worker(&worker)
        .and_then(|held| held.dispatch.clone())
        .expect("the dispatch");
    assert_eq!(bench.ledger.window_exiting(2_000).sleeping, 1);
    (task, worker, dispatch)
}

/// A sleeper whose conversation was never recorded ends now — not at the
/// grace — and the run hears it once, with the dispatch id a `--retry-of`
/// needs and the reason in the same words as the attempt's record.
#[test]
fn a_sleeper_that_cannot_come_back_is_ended_now_and_told_once_with_why() {
    let mut bench = Bench::new();
    let (_task, worker, dispatch) = a_sleeper(&mut bench, "worker-start --agent codex", None);
    bench
        .ledger
        .sleeper_unrecoverable(&worker, NO_SESSION_RECORDED, 3_000)
        .expect("a sleeper can be given up on");
    let run = &bench.ledger.runs()[0];
    assert!(!run.worker(&worker).expect("the worker").state.is_live());
    let told: Vec<serde_json::Value> = run
        .messages
        .iter()
        .filter(|message| message.kind == MessageKind::WorkerDied)
        .map(|message| serde_json::from_str(message.body.as_str()).expect("a notice"))
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0]["dispatchId"], dispatch.as_str());
    assert_eq!(told[0]["reason"], NO_SESSION_RECORDED);
    // Not twice, and not again at the grace.
    assert!(
        bench
            .ledger
            .sleeper_unrecoverable(&worker, NO_SESSION_RECORDED, 3_001)
            .is_err()
    );
    assert!(bench.ledger.sleeper_expired(&worker, 3_002).is_err());
    assert_eq!(
        bench.ledger.runs()[0]
            .messages
            .iter()
            .filter(|message| message.kind == MessageKind::WorkerDied)
            .count(),
        1
    );
}

/// A sleeper's conversation is resumed as the launch it was summoned with —
/// model, effort, the peer name its run gave it, and what its CLI is told
/// about a classifier's decline (t-7153: the pinned column for a pinned
/// model) — on either road back; and a value the row never held is not
/// filled in.
#[test]
fn a_sleepers_resume_words_are_the_launch_it_was_summoned_with() {
    let mut bench = Bench::new();
    let (_task, worker, _dispatch) = a_sleeper(
        &mut bench,
        "worker-start --agent claude --model claude-fable-5-1 --effort xhigh",
        Some("7812a0c1-0000-4000-8000-000000000c02"),
    );
    let run = bench.ledger.runs()[0].id.clone();
    let peer = provider_peer("claude", &run, &worker).expect("claude takes a peer name");
    let mut expected = vec![
        "--model".to_string(),
        "claude-fable-5-1".to_string(),
        "--effort".to_string(),
        "xhigh".to_string(),
    ];
    peer.append_launch_tuning(&mut expected);
    continue_past_classifier_declines("claude", true, &mut expected);
    assert_eq!(
        bench.ledger.sleeper_resume_tuning(&worker),
        Ok(expected),
        "the window's resume would launch a CLI nobody summoned"
    );

    let mut plain = Bench::new();
    let (_task, codex, _dispatch) = a_sleeper(
        &mut plain,
        "worker-start --agent codex",
        Some("session-t7812-plain"),
    );
    assert_eq!(
        plain.ledger.sleeper_resume_tuning(&codex),
        Ok(Vec::new()),
        "a launch summoned without tuning came back with some"
    );
    assert!(plain.ledger.sleeper_resume_tuning("w-nobody").is_err());
}

/// The witness road's undo (t-7812 R1): a sleeper a door seated before its
/// pane started goes back to sleep when the pane never did — its dispatch
/// open, its task dispatched, its attempt unspent, and the seat's address no
/// longer the run's — so the next road can seat it again. Nothing is told:
/// nothing was lost. A worker that is not active is not this road's.
#[test]
fn a_sleeper_whose_resumed_pane_never_started_sleeps_again() {
    let mut bench = Bench::new();
    let session = "session-t7812-undo";
    let (task, worker, dispatch) =
        a_sleeper(&mut bench, "worker-start --agent codex", Some(session));
    let run = bench.ledger.runs()[0].id.clone();
    assert_eq!(
        bench
            .ledger
            .worker_pane_resumed(("team-9", "%1"), "/wt/sleeper", "codex", session, 3_000),
        Some(worker.clone())
    );
    assert_eq!(bench.ledger.bound_run("team-9/%1"), Some(run.as_str()));

    assert!(bench.ledger.resumed_pane_never_started(&worker));
    let held = bench.ledger.runs()[0].clone();
    let row = held.worker(&worker).expect("the worker");
    assert_eq!(row.state, WorkerState::Sleeping);
    assert_eq!(row.dispatch.as_deref(), Some(dispatch.as_str()));
    let kept = held.task(&task).expect("the task");
    assert_eq!(kept.status, TaskStatus::Dispatched);
    assert_eq!(
        kept.failures, 0,
        "the pane that never started spent the attempt"
    );
    assert_eq!(
        bench.ledger.bound_run("team-9/%1"),
        None,
        "the seat that never started still speaks for the run"
    );
    assert!(
        held.messages
            .iter()
            .all(|message| message.kind != MessageKind::WorkerDied),
        "a pane that never started was told as a death"
    );
    assert!(
        !bench.ledger.resumed_pane_never_started(&worker),
        "a sleeper was put to sleep twice"
    );
    assert!(!bench.ledger.resumed_pane_never_started("w-nobody"));
    // The next road seats it again.
    assert_eq!(
        bench
            .ledger
            .worker_pane_resumed(("team-10", "%1"), "/wt/sleeper", "codex", session, 3_100),
        Some(worker)
    );
}

/// t-6740 with t-7812: an asker waiting on a worker the restart could not
/// bring back hears that it EXITED — final, and without the advice a
/// cancelled receiver carries — whichever way the loss was known: the grace
/// ran out, or its conversation was never recorded, is not on disk, or is
/// one this window cannot resume. None of them is its coordinator
/// cancelling it, and a question answered "cancelled" would tell the asker
/// somebody chose to.
#[test]
fn an_asker_hears_a_sleeper_the_restart_lost_as_exited_not_cancelled() {
    for reason in [
        NOT_RESUMED,
        NO_SESSION_RECORDED,
        CONVERSATION_FILE_GONE,
        RESUME_UNSUPPORTED,
    ] {
        let mut bench = Bench::new();
        bench.actor = Some(bench_actor("team-1", "%1"));
        bench.json("run-create --name lost");
        let task = bench.json("task-create --spec keep-going")["taskId"]
            .as_str()
            .expect("a task")
            .to_string();
        let (worker, pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
        assert!(bench.ledger.worker_seated(("team-1", &pane), "/wt/lost"));
        let (question, _) = asked_of(&mut bench, "%1", &format!("worker:{worker}"));
        assert_eq!(bench.ledger.window_exiting(2_000).sleeping, 1);
        let given_up = if reason == NOT_RESUMED {
            bench.ledger.sleeper_expired(&worker, 3_000)
        } else {
            bench.ledger.sleeper_unrecoverable(&worker, reason, 3_000)
        };
        given_up.expect("a sleeper can be given up on");
        let mail = bench.json_at("%1", "check --peek");
        let told: Vec<serde_json::Value> = mail["messages"]
            .as_array()
            .expect("the inbox")
            .iter()
            .filter(|row| row["thread"] == question.as_str())
            .map(notice_body)
            .collect();
        assert_eq!(told.len(), 1, "{reason}: {mail}");
        assert_eq!(told[0]["reason"], ReceiverNews::Exited.word(), "{reason}");
        assert_eq!(told[0]["final"], true, "{reason}");
        assert!(told[0]["advice"].is_null(), "{reason}: {}", told[0]);
    }
}
