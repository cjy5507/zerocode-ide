//! t-69010: a worker's turn that failed is told as a failure, once, in place
//! of its silence.
//!
//! On 2026-10-21 a zo worker's turn died on a 240 s provider silence (t-68710)
//! and its coordinator was told `went_quiet` and nothing else: zo had said the
//! turn failed and why, and the window threw both away. These scenes are the
//! ledger's half: the notice, its words, and the silence it replaces.

use super::*;

/// A run with one claude worker on a task — answers the worker and its pane.
fn a_worker(bench: &mut Bench) -> (String, String) {
    let task = bench.json("task-create --spec turn-failed")["taskId"]
        .as_str()
        .expect("a task")
        .to_string();
    bench.seat(&format!("worker-start --agent claude --task {task}"))
}

fn letters(bench: &Bench, kind: MessageKind) -> Vec<Message> {
    bench.ledger.runs()[0]
        .messages()
        .iter()
        .filter(|one| one.kind == kind)
        .cloned()
        .collect()
}

#[test]
fn a_failed_turn_is_one_notice_to_the_coordinator_and_its_silence_is_not_told() {
    let mut bench = Bench::new();
    bench.json("run-create --name failed-turns");
    let (worker, pane) = a_worker(&mut bench);
    let error = "api failed after 3 attempts: stream_startup_no_progress";

    let told = bench
        .ledger
        .worker_turn_failed(("team-1", &pane), 9_000, error, 9_100)
        .expect("the failed turn is told");
    let failed = letters(&bench, MessageKind::TurnFailed);
    assert_eq!(failed.len(), 1, "one failed turn, one notice");
    assert_eq!(failed[0].id, told);
    assert_eq!(failed[0].from, LEDGER_ITSELF);
    let body: serde_json::Value = serde_json::from_str(failed[0].body.as_str()).expect("json");
    assert_eq!(body["workerId"], worker.as_str());
    assert_eq!(body["error"], error);
    assert_eq!(body["turnEndedMs"], 9_000);
    let run = &bench.ledger.runs()[0];
    let address = run.address();
    assert!(
        run.inboxes
            .iter()
            .any(|(at, inbox)| *at == address && inbox.pending.contains(&told)),
        "the coordinator's inbox was not handed the failure"
    );

    // The same turn's end, heard again as a silence or as a failure, tells
    // nothing more.
    assert!(
        bench
            .ledger
            .worker_fell_silent(("team-1", &pane), 9_000, false, 9_200)
            .is_none(),
        "the failed turn was told again as a silence"
    );
    assert!(
        bench
            .ledger
            .worker_turn_failed(("team-1", &pane), 9_000, error, 9_300)
            .is_none()
    );
    assert!(letters(&bench, MessageKind::WentQuiet).is_empty());
    assert_eq!(letters(&bench, MessageKind::TurnFailed).len(), 1);

    // A provider's whole answer is not a notice: the words are cut.
    let long = "x".repeat(TURN_FAILED_WORDS * 2);
    assert!(
        bench
            .ledger
            .worker_turn_failed(("team-1", &pane), 9_500, &long, 9_600)
            .is_some()
    );
    let newest = letters(&bench, MessageKind::TurnFailed)
        .pop()
        .expect("the second");
    let body: serde_json::Value = serde_json::from_str(newest.body.as_str()).expect("json");
    assert_eq!(
        body["error"].as_str().map(|said| said.chars().count()),
        Some(TURN_FAILED_WORDS)
    );

    // An attempt that is over has no turn to fail.
    bench.json(&format!("worker-stop --worker {worker} --reason done"));
    assert!(
        bench
            .ledger
            .worker_turn_failed(("team-1", &pane), 10_000, error, 10_100)
            .is_none(),
        "a stopped worker's turn was told as failed"
    );
    assert!(MessageKind::TurnFailed.is_the_ledgers_own());
    assert_eq!(
        "turn_failed".parse::<MessageKind>().ok(),
        Some(MessageKind::TurnFailed)
    );
}

/// t-69010: 「다시 띄우기」 — a person's press at a worker whose turn failed.
/// It needs no declared order, is about the failure the attempt stands at
/// now, and types one line per failure: once it was typed the same failure is
/// refused, and once the worker spoke again the press is about a failure that
/// no longer stands.
#[test]
fn a_press_at_a_failed_turn_is_one_continuation_for_the_failure_that_stands() {
    let mut bench = Bench::new();
    bench.json("run-create --name pressed");
    let (worker, pane) = a_worker(&mut bench);
    assert_eq!(person_turn_marker(&bench.ledger.runs()[0], &worker), None);
    let letter = bench
        .ledger
        .worker_turn_failed(
            ("team-1", &pane),
            9_000,
            "api failed after 3 attempts",
            9_100,
        )
        .expect("the failure");
    let marker = person_turn_marker(&bench.ledger.runs()[0], &worker).expect("a press");
    assert_eq!(marker.source, PERSON_TURN_MARKER_SOURCE);
    assert!(marker.key.ends_with(&letter), "{}", marker.key);

    // No `--on-transient-error resume` was declared: the press is the order.
    let plan = resume_plan(&bench.ledger.runs()[0], &worker, &marker, 9_200).expect("a plan");
    let run_id = plan.run.clone();
    let typed = bench.ledger.resume_begin(&plan, 9_200).expect("reserved");
    bench
        .ledger
        .resume_settled(
            &run_id,
            &typed,
            &ResumeOutcome {
                submitted: true,
                typed: true,
                detail: Text::from("typed and entered"),
            },
            9_201,
        )
        .expect("settled");
    // The receipt is the press's own answer, not the attempt going on: the
    // failure still stands, and the same failure is not typed at twice.
    let again = person_turn_marker(&bench.ledger.runs()[0], &worker).expect("still standing");
    assert_eq!(again.key, marker.key);
    assert!(
        matches!(
            resume_plan(&bench.ledger.runs()[0], &worker, &again, 99_000),
            Err(NotResumed::Refused(WallRefusal::AlreadyTyped { .. }))
        ),
        "one failure was typed at twice"
    );

    // The worker spoke: the failure is history, and a press made on it is
    // about a failure that no longer stands.
    bench.json_at(&pane, "send --type status --body going-on");
    assert_eq!(person_turn_marker(&bench.ledger.runs()[0], &worker), None);
    assert!(
        matches!(
            resume_plan(&bench.ledger.runs()[0], &worker, &marker, 99_000),
            Err(NotResumed::Refused(WallRefusal::TurnNotNow { .. }))
        ),
        "a press on a failure that no longer stands was taken"
    );
}

/// The review R2, P2-10: the confirm shows failure A, and while it stands open
/// the attempt fails again (B). The press carries what the person was shown,
/// so its approval is refused as a failure that no longer stands, and nothing
/// is planned to be typed at B; a press shown B is planned.
#[test]
fn a_confirm_shown_one_failure_is_refused_once_another_failure_stands() {
    let mut bench = Bench::new();
    bench.json("run-create --name shown");
    let (worker, pane) = a_worker(&mut bench);
    let shown = bench
        .ledger
        .worker_turn_failed(
            ("team-1", &pane),
            9_000,
            "api failed after 3 attempts",
            9_100,
        )
        .expect("failure A");
    let dispatch = bench.ledger.runs()[0]
        .worker(&worker)
        .and_then(|one| one.dispatch.clone())
        .expect("the attempt");
    let newer = bench
        .ledger
        .worker_turn_failed(
            ("team-1", &pane),
            19_000,
            "api failed after 3 attempts",
            19_100,
        )
        .expect("failure B");
    assert_ne!(shown, newer);
    let run = &bench.ledger.runs()[0];
    assert!(
        matches!(
            resume_plan(
                run,
                &worker,
                &person_turn_marker_at(&dispatch, &shown),
                19_200
            ),
            Err(NotResumed::Refused(WallRefusal::TurnNotNow { .. }))
        ),
        "an approval of failure A was planned against failure B"
    );
    let marker = person_turn_marker_at(&dispatch, &newer);
    assert_eq!(person_turn_marker(run, &worker), Some(marker.clone()));
    assert!(resume_plan(run, &worker, &marker, 19_200).is_ok());
}
