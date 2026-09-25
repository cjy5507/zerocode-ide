//! t-7812: the ledger's half of a window restart, on the core bench — the
//! transitions only, as they stood before the fix and after it.

use super::*;

/// t-7812 B / observation 4: a worker whose pane a person's hand had touched
/// takes the ledger's new seat as the same worker, and the hand stays the
/// person's. On 2026-09-25 01:13 the grace ended two such workers (w-7570,
/// w-7631, both `takenOver`): the window never keeps a ledger-seated tab in
/// its layout to reopen, and the ledger would not cut one either. Once — the
/// worker is live afterwards and a second reseat is refused — and a late exit
/// of the old pane settles nothing.
#[test]
fn a_taken_over_sleeper_takes_the_ledgers_new_seat_and_stays_the_persons() {
    let mut bench = Bench::new();
    bench.actor = Some(bench_actor("team-1", "%1"));
    bench.json("run-create --name taken");
    let task = bench.json("task-create --spec keep-going")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(bench.ledger.worker_seated(("team-1", &pane), "/wt/taken"));
    assert!(bench.ledger.worker_session_reported(
        ("team-1", &pane),
        ProviderSession {
            key: SessionKey::SessionId,
            id: "7812a0c1-0000-4000-8000-000000000c01".to_string(),
            transcript_path: None,
        },
    ));
    assert!(bench.ledger.worker_taken_over(("team-1", &pane)));
    let dispatch = bench.ledger.runs()[0]
        .worker(&worker)
        .and_then(|held| held.dispatch.clone())
        .expect("the dispatch");
    assert_eq!(bench.ledger.window_exiting(2_000).sleeping, 1);

    bench
        .ledger
        .worker_reseated(&worker, ("team-2", "%2"), 3_000)
        .expect("a sleeper a person had touched is reseated too");

    let run = &bench.ledger.runs()[0];
    let back = run.worker(&worker).expect("the worker");
    assert_eq!(back.state, WorkerState::Active);
    assert!(
        back.taken_over,
        "the restart forgot whose hand was on the pane"
    );
    assert_eq!(back.dispatch.as_deref(), Some(dispatch.as_str()));
    assert_eq!((back.team.as_str(), back.pane.as_str()), ("team-2", "%2"));
    assert!(run.dispatch(&dispatch).is_some_and(Dispatch::is_open));
    assert_eq!(
        run.task(&task).expect("the task").status,
        TaskStatus::Dispatched
    );
    assert!(
        bench
            .ledger
            .worker_reseated(&worker, ("team-3", "%3"), 3_001)
            .is_err(),
        "one conversation was given a second seat"
    );
    // The old pane's exit, arriving late, is nobody's any more.
    assert_eq!(bench.ledger.terminal_gone("team-1", &pane, 3_002), None);
    let run = &bench.ledger.runs()[0];
    assert_eq!(
        run.worker(&worker).expect("the worker").state,
        WorkerState::Active
    );
    assert!(
        !run.messages
            .iter()
            .any(|message| message.kind == MessageKind::WorkerDied),
        "the restart was announced as a death"
    );
}
