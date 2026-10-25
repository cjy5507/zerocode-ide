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
