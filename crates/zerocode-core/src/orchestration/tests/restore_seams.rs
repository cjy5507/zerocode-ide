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
/// model, effort, and the peer name its run gave it — on either road back;
/// and a value the row never held is not filled in.
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
