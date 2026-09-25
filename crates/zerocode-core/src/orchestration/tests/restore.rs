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

/// t-9091: a ledger whose window has said its goodbye neither ends a sleeper
/// at the grace nor plans it a pane — both are the next window's, and after
/// that window's boot the same sleeper is planned back as itself.
///
/// On 2026-09-25 the closing window's own beat kept running for the second
/// or two its way out took (19:04:36 → 19:04:37.6, 19:15:10 → 19:15:12.5),
/// measured the grace from its OWN boot (72.2 and 10.4 minutes back) and
/// ended every worker its goodbye had just put to sleep: five `worker_died`
/// with `NOT_RESUMED`, 0.36 to 1.19 s after the goodbye, and the next
/// windows had nobody to bring back. The one worker that lived (w-9055)
/// was last in that loop when the process ended.
#[test]
fn a_ledger_that_said_its_goodbye_leaves_its_sleepers_to_the_next_window() {
    let mut bench = Bench::new();
    bench.actor = Some(bench_actor("team-1", "%1"));
    let run = bench.json("run-create --name goodbye")["runId"]
        .as_str()
        .expect("a run")
        .to_string();
    let task = bench.json("task-create --spec keep-going")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    let (worker, pane) = bench.seat(&format!(
        "worker-start --agent claude --model opus --effort xhigh --task {task}"
    ));
    assert!(bench.ledger.worker_seated(("team-1", &pane), "/wt/goodbye"));
    let session = "9091a0c1-0000-4000-8000-000000000001";
    assert!(bench.ledger.worker_session_reported(
        ("team-1", &pane),
        ProviderSession {
            key: SessionKey::SessionId,
            id: session.to_string(),
            transcript_path: None,
        },
    ));
    let dispatch = bench.ledger.runs()[0]
        .worker(&worker)
        .and_then(|held| held.dispatch.clone())
        .expect("the dispatch");
    assert_eq!(bench.ledger.window_exiting(2_000).sleeping, 1);

    // The closing window's beat, its own boot long past the grace.
    assert!(
        bench
            .ledger
            .sleeper_expired(&worker, 2_001 + RESEAT_GRACE_MS)
            .is_err(),
        "the window that said goodbye ended the sleeper its goodbye made"
    );
    assert!(
        bench
            .ledger
            .prepare_worker_reseat(
                &run,
                &worker,
                &mut bench.team,
                agent_teams::LEADER_PANE,
                &bench.launcher,
                "continue",
            )
            .is_err(),
        "a pane was planned in a window on its way out"
    );
    let kept = bench.ledger.runs()[0].clone();
    assert_eq!(
        kept.worker(&worker).expect("the worker").state,
        WorkerState::Sleeping
    );
    assert!(kept.dispatch(&dispatch).is_some_and(Dispatch::is_open));
    let held = kept.task(&task).expect("the task");
    assert_eq!(held.status, TaskStatus::Dispatched);
    assert_eq!(held.failures, 0, "the goodbye spent the attempt");
    assert!(
        !kept
            .messages
            .iter()
            .any(|message| message.kind == MessageKind::WorkerDied),
        "the goodbye's sleeper was announced dead"
    );

    // The next window's boot: its coordinator back, the sleeper planned back
    // as the same worker, into the same conversation.
    bench.ledger.window_restarted(3_000);
    assert!(
        bench
            .ledger
            .coordinator_returned(&run, "team-2/%1", None, 3_001)
    );
    let mut back = Team::new("team-2", "token", 90);
    let planned = bench
        .ledger
        .prepare_worker_reseat(
            &run,
            &worker,
            &mut back,
            agent_teams::LEADER_PANE,
            &bench.launcher,
            "continue",
        )
        .expect("the next window plans it back");
    let Effect::Split { command, .. } = &planned.effect else {
        panic!("the reseat planned no split: {:?}", planned.effect);
    };
    assert!(
        command.contains(&format!("--resume {session}")),
        "not the same conversation: {command}"
    );
    let prepared = planned
        .prepared_worker_reseat
        .as_ref()
        .expect("the typed reseat");
    assert_eq!(prepared.checkout, "/wt/goodbye");
    assert_eq!(prepared.resumed, WorkerResume::Session);
    assert_eq!(
        bench.ledger.runs()[0]
            .worker(&worker)
            .and_then(|held| held.dispatch.clone()),
        Some(dispatch),
        "a second attempt was minted"
    );
}
