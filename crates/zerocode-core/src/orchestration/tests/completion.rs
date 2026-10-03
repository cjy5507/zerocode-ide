use super::*;
use task_cost::{JevTally, SessionAttribution, SessionBook, TaskCost};

fn reported() -> (Bench, String, String) {
    let mut bench = Bench::new();
    bench.json("run-create --name completion");
    let task = bench.json("task-create --spec work")["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let dispatch = bench.ledger.runs()[0].dispatches[0].id.clone();
    bench.clock = 2_000;
    bench.json_at(&pane,
        "send --type worker_done --body {\"ok\":true,\"head\":\"abc1234\",\"verified\":true,\"merged\":true,\"completed_ms\":1}");
    (bench, task, dispatch)
}

fn accept(bench: &mut Bench, task: &str, dispatch: &str, source: &str) {
    bench.json(&format!(
        "task-update --task {task} --result {{\"verified\":true,\"merged\":true}} --attempt {dispatch} --source {source}"
    ));
}

fn cost(ledger: &Ledger, task: &str) -> TaskCost {
    task_cost::task_cost(
        &ledger.runs()[0],
        task,
        &SessionBook::default(),
        JevTally::default(),
        &SessionAttribution::new(ledger.runs()),
    )
}

#[test]
fn completion_waits_for_independent_acceptance_and_survives_rebuild() {
    let (mut bench, task, dispatch) = reported();
    let started = bench.ledger.runs()[0].dispatches[0].started_ms;
    assert_eq!(cost(&bench.ledger, &task).wall_ms, Some(2_001 - started));
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 3_000;
    bench.json(&format!(
        "task-update --task {task} --result {{\"verified\":true}} --attempt {dispatch} --source abc1234"
    ));
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 9_000;
    accept(&mut bench, &task, &dispatch, "abc1234");
    let rebuilt = Ledger::rebuild(bench.ledger.export()).unwrap();
    for ledger in [&bench.ledger, &rebuilt] {
        let total = cost(ledger, &task);
        assert_eq!(total.wall_ms, Some(2_001 - started));
        assert_eq!(total.completion_wall_ms, Some(9_001 - started));
        let run = &ledger.runs()[0];
        let outcome = crate::summon_difficulty::outcomes::observe(
            run,
            &run.dispatches[0],
            &total.generation,
            &total,
        )
        .unwrap();
        assert_eq!(outcome.task_wall_ms, total.completion_wall_ms);
        assert_eq!(outcome.first_attempt_success, Some(true));
    }
}

#[test]
fn later_deployment_keeps_completion_time_but_a_retracted_review_revokes_it() {
    let (mut bench, task, dispatch) = reported();
    bench.clock = 9_000;
    accept(&mut bench, &task, &dispatch, "abc1234");
    let accepted = cost(&bench.ledger, &task).completion_wall_ms;
    bench.clock = 20_000;
    bench.json(&format!(
        "task-update --task {task} --result {{\"verified\":true,\"merged\":true,\"deployed\":true}} --attempt {dispatch} --source abc1234"
    ));
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, accepted);
    bench.json(&format!(
        "task-update --task {task} --result {{\"verified\":false,\"merged\":true}} --attempt {dispatch} --source abc1234"
    ));
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 30_000;
    accept(&mut bench, &task, &dispatch, "abc1234");
    let started = bench.ledger.runs()[0].dispatches[0].started_ms;
    assert_eq!(
        cost(&bench.ledger, &task).completion_wall_ms,
        Some(30_001 - started)
    );
}

#[test]
fn another_attempt_inherits_no_completion_and_its_waits_count_in_the_new_one() {
    let (mut bench, task, dispatch) = reported();
    bench.clock = 9_000;
    accept(&mut bench, &task, &dispatch, "abc1234");
    bench.json(&format!("task-update --task {task} --status ready"));
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 20_000;
    let (_, pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
    let next = bench.ledger.runs()[0].dispatches[1].id.clone();
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 25_000;
    bench.json_at(
        &pane,
        "send --type worker_done --body {\"ok\":true,\"head\":\"def5678\"}",
    );
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 30_000;
    accept(&mut bench, &task, &next, "def5678");
    let started = bench.ledger.runs()[0].dispatches[0].started_ms;
    assert_eq!(
        cost(&bench.ledger, &task).completion_wall_ms,
        Some(30_001 - started)
    );
}

#[test]
fn a_changed_hand_in_revokes_the_clock_even_on_the_same_attempt() {
    let (mut bench, task, dispatch) = reported();
    bench.clock = 9_000;
    accept(&mut bench, &task, &dispatch, "abc1234");
    let run_id = bench.ledger.runs()[0].id.clone();
    bench.ledger.run_mut(&run_id).unwrap().dispatches[0].source = Some("def5678".into());
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 20_000;
    accept(&mut bench, &task, &dispatch, "def5678");
    let started = bench.ledger.runs()[0].dispatches[0].started_ms;
    assert_eq!(
        cost(&bench.ledger, &task).completion_wall_ms,
        Some(20_001 - started)
    );
}

#[test]
fn old_reviews_and_a_clock_before_the_work_ended_never_invent_completion_time() {
    let (mut bench, task, dispatch) = reported();
    bench.clock = 100;
    accept(&mut bench, &task, &dispatch, "abc1234");
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    bench.clock = 9_000;
    accept(&mut bench, &task, &dispatch, "abc1234");
    let mut saved = serde_json::to_value(bench.ledger.export()).unwrap();
    saved["tasks"][0]["result_author"]
        .as_object_mut()
        .unwrap()
        .remove("completed_ms");
    let old = Ledger::rebuild(serde_json::from_value(saved).unwrap()).unwrap();
    let run = &old.runs()[0];
    assert!(run.review_of(run.task(&task).unwrap()).verified);
    let total = cost(&old, &task);
    assert_eq!(total.completion_wall_ms, None);
    assert_eq!(
        crate::summon_difficulty::outcomes::observe(
            run,
            &run.dispatches[0],
            &total.generation,
            &total
        )
        .unwrap()
        .task_wall_ms,
        None
    );
    bench.ledger = old;
    bench.clock = 20_000;
    bench.json(&format!("task-update --task {task} --status completed"));
    assert_eq!(cost(&bench.ledger, &task).completion_wall_ms, None);
    accept(&mut bench, &task, &dispatch, "abc1234");
    assert!(cost(&bench.ledger, &task).completion_wall_ms.is_some());
}
