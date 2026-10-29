//! t-82501: the board's 「통과」 is written over the result it found, not in
//! its place.
//!
//! 2026-10-29 10:27:29 (run-4275's observation): t-81714's worker handed in
//! its summary, its tests and its report's path, and 33 seconds later a press
//! of 「통과」 replaced the whole result with `{"verified":true}` — `task-update`
//! replaces a result whole, and the board sent one key. The board now sends
//! the result as it stood with `verified` and its own mark added
//! ([`board_pass_result`]). What it carries over must not dress the press as
//! a new review: a coordinator's withdrawal of that attempt and source still
//! refuses it (t-81913), whatever keys the withdrawn result held.

use super::*;

/// A worker in its own pane carrying a new task of a new run, its report
/// sent with `body`: the task and the attempt that report ended.
fn handed_in(bench: &mut Bench, body: &serde_json::Value) -> (String, String) {
    bench.json("run-create --name board-pass");
    let task = bench.json("task-create --spec board-pass")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (_, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    let attempt = bench.ledger.runs()[0].dispatches[0].id.clone();
    let sent = bench.at_argv(
        &pane,
        ["send", "--type", "worker_done", "--body", &body.to_string()]
            .map(str::to_string)
            .to_vec(),
    );
    assert_eq!(sent.reply.exit_code, 0, "{:?}", sent.reply);
    (task, attempt)
}

/// The board's press as the window sends it: the result the task holds now,
/// passed over, against the attempt and the source the board drew.
fn press(bench: &mut Bench, task: &str, attempt: &str, source: &str, at: i64) -> Decided {
    let result = {
        let run = &bench.ledger.runs()[0];
        board_pass_result(run.task(task).expect("the task").result.as_str(), at)
    };
    bench.at_argv(
        agent_teams::LEADER_PANE,
        [
            "task-update",
            "--task",
            task,
            "--result",
            &result,
            "--attempt",
            attempt,
            "--source",
            source,
        ]
        .map(str::to_string)
        .to_vec(),
    )
}

fn stored(bench: &Bench, task: &str) -> serde_json::Value {
    let run = &bench.ledger.runs()[0];
    serde_json::from_str(run.task(task).expect("the task").result.as_str())
        .expect("the result is JSON")
}

/// A pass keeps what the worker wrote — its summary, its tests and its
/// report — and adds the verification and when the board was pressed. A
/// worker's own word about landing stays its claim: the pass does not write
/// it over as the coordinator's fact.
#[test]
fn the_boards_pass_keeps_the_workers_result_and_adds_its_verification() {
    let mut bench = Bench::new();
    let worker_said = serde_json::json!({
        "ok": true,
        "head": "abc1234",
        "summary": "가격 가드가 빈 장바구니에서도 0원을 막는다",
        "tests": "price_guard 12 passed",
        "report": "/Users/dev/acme-web/reviews/t-2041/REPORT.md",
        "merged": true,
    });
    let (task, attempt) = handed_in(&mut bench, &worker_said);
    let pressed = press(&mut bench, &task, &attempt, "abc1234", 1_234);
    assert_eq!(pressed.reply.exit_code, 0, "{:?}", pressed.reply);

    let after = stored(&bench, &task);
    for key in ["ok", "head", "summary", "tests", "report"] {
        assert_eq!(
            after[key], worker_said[key],
            "the pass lost the worker's `{key}`: {after}"
        );
    }
    assert_eq!(after["verified"], true, "{after}");
    assert_eq!(after[BOARD_PASS_KEY], 1_234, "{after}");
    assert!(
        after.get("merged").is_none(),
        "the worker's claim was written over as the coordinator's: {after}"
    );
    let run = &bench.ledger.runs()[0];
    let facts = run.review_of(run.task(&task).expect("the task"));
    assert!(
        facts.verified && !facts.merged && facts.author == ReviewAuthor::Coordinator,
        "{facts:?}"
    );
}

/// The withdrawal the board's pass carries over names a review — its report,
/// the head it was tested at — and none of that is new: the press is still a
/// bare verification, and the withdrawal still refuses it in its own words.
#[test]
fn a_withdrawn_verification_still_refuses_the_boards_pass_whatever_it_carries_over() {
    let mut bench = Bench::new();
    let (task, attempt) = handed_in(
        &mut bench,
        &serde_json::json!({ "ok": true, "head": "abc1234", "summary": "빈 상태 그림" }),
    );
    let why = "the independent review found the dark-mode picture unreadable";
    let withdrawal = serde_json::json!({
        "verified": false,
        "verificationWithdrawn": why,
        "report": "/Users/dev/acme-web/reviews/t-2025/REPORT.md",
        "testedHead": "abc1234",
    });
    let wrote = bench.at_argv(
        agent_teams::LEADER_PANE,
        [
            "task-update",
            "--task",
            &task,
            "--result",
            &withdrawal.to_string(),
            "--attempt",
            &attempt,
            "--source",
            "abc1234",
        ]
        .map(str::to_string)
        .to_vec(),
    );
    assert_eq!(wrote.reply.exit_code, 0, "{:?}", wrote.reply);

    let before = bench.ledger.export();
    let pressed = press(&mut bench, &task, &attempt, "abc1234", 2_345);
    assert_ne!(
        pressed.reply.exit_code, 0,
        "the board's pass covered the withdrawal: {:?}",
        pressed.reply
    );
    let said = pressed.reply.stderr.trim();
    assert!(
        said.contains("withdrew") && said.contains(why) && !said.contains('\n'),
        "the refusal says, in one line, that the verification was withdrawn and why: {said}"
    );
    assert_eq!(bench.ledger.export().tasks, before.tasks);
}

/// The board's mark rides along when a coordinator writes the result again,
/// as a coordinator who keeps the whole result does (`task-update` replaces it
/// whole). Over a withdrawal, that write names a review of its own — who
/// reviewed and the head it tested, keys the press never leaves — so it is a
/// new review and stands; the mark beside it does not make it a bare press
/// (run-4275's review of t-82501, P3).
#[test]
fn a_new_review_written_over_the_boards_mark_still_lifts_a_withdrawal() {
    let mut bench = Bench::new();
    let (task, attempt) = handed_in(
        &mut bench,
        &serde_json::json!({ "ok": true, "head": "abc1234", "summary": "빈 상태 그림" }),
    );
    let pressed = press(&mut bench, &task, &attempt, "abc1234", 2_345);
    assert_eq!(pressed.reply.exit_code, 0, "{:?}", pressed.reply);
    let mut withdrawal = stored(&bench, &task);
    let map = withdrawal.as_object_mut().expect("a result object");
    map.insert("verified".into(), false.into());
    map.insert(
        "verificationWithdrawn".into(),
        "the independent review found the dark-mode picture unreadable".into(),
    );
    let update = |bench: &mut Bench, result: &serde_json::Value| {
        bench.at_argv(
            agent_teams::LEADER_PANE,
            [
                "task-update",
                "--task",
                &task,
                "--result",
                &result.to_string(),
                "--attempt",
                &attempt,
                "--source",
                "abc1234",
            ]
            .map(str::to_string)
            .to_vec(),
        )
    };
    let withdrew = update(&mut bench, &withdrawal);
    assert_eq!(withdrew.reply.exit_code, 0, "{:?}", withdrew.reply);
    assert!(
        stored(&bench, &task).get("passedOnBoard").is_some(),
        "the mark rode along"
    );

    let mut again = stored(&bench, &task);
    let map = again.as_object_mut().expect("a result object");
    map.remove("verificationWithdrawn");
    map.insert("verified".into(), true.into());
    map.insert("reviewedBy".into(), "run-2025 coordinator".into());
    map.insert("testedHead".into(), "abc1234".into());
    let reviewed = update(&mut bench, &again);
    assert_eq!(
        reviewed.reply.exit_code, 0,
        "a review that names who reviewed and the head it tested lifts the withdrawal \
         though the board's mark rode along: {:?}",
        reviewed.reply
    );
    assert_eq!(stored(&bench, &task)["reviewedBy"], "run-2025 coordinator");
}
