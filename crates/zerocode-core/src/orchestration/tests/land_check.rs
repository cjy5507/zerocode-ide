//! t-34501 stage 3 (t-42447): `land-check` is the coordinator's verb. The ledger names the task, the
//! head and the repository and hands the window its effect; the window's evidence comes back once per
//! check id as the ledger's own letter to the coordinator, with the task named.

use super::*;
use crate::mail_triage::{Triage, kind_rule};
use crate::orchestration::land_check::{
    CHECK_PREFIX, LAND_CHECK_VERB, LandCheckAsk, LandCheckReceipt, is_check_id,
};

const CHECKOUT: &str = "/r/checkout";
const HEAD: &str = "abc1234def5678abc1234def5678abc1234def56";

/// An absolute log path on either platform: `/abs/…` is no absolute path on Windows.
fn abs_log(rest: &str) -> String {
    if cfg!(windows) {
        format!("C:/abs/{rest}")
    } else {
        format!("/abs/{rest}")
    }
}

/// A run with one task that a seated worker handed in from [`CHECKOUT`] at `head`. Answers the task
/// id and the worker's pane.
fn handed_in(bench: &mut Bench, head: &str) -> (String, String) {
    if bench.ledger.runs().is_empty() {
        bench.json("run-create --name check");
    }
    let task = bench.json("task-create --spec check-task")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let (worker, pane) = bench.seat(&format!("worker-start --agent claude --task {task}"));
    assert!(
        bench.ledger.worker_seated(("team-1", &pane), CHECKOUT),
        "the worker was seated in its checkout"
    );
    assert!(!worker.is_empty());
    bench.json_at(
        &pane,
        &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{head}\"}}"),
    );
    (task, pane)
}

/// The window's answer for one check, as the receipt the ledger takes from it.
fn receipt(run: &str, task: &str, check: &str, evidence: &str) -> LandCheckReceipt {
    LandCheckReceipt {
        run: run.to_string(),
        task: task.to_string(),
        check: check.to_string(),
        evidence: evidence.to_string(),
    }
}

/// The ask the coordinator's verb planned, or a panic that says what it planned instead.
fn asked(planned: &Decided) -> LandCheckAsk {
    match planned.effect.clone() {
        Effect::LandCheck(ask) => ask,
        other => panic!("land-check planned no land check: {other:?}"),
    }
}

/// The coordinator asks for its task's handed-in head to be checked. The ledger names the task, the
/// head its worker handed in, and the checkout that names the repository.
#[test]
fn a_check_names_the_task_its_handed_in_head_and_its_checkout() {
    let mut bench = Bench::new();
    let (task, _pane) = handed_in(&mut bench, HEAD);
    let run = bench.ledger.runs()[0].id.clone();
    let planned = bench.run(&format!("land-check --task {task}"));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    assert_eq!(
        asked(&planned),
        LandCheckAsk::Merge {
            run,
            task,
            head: HEAD.to_string(),
            checkout: CHECKOUT.to_string(),
            prepare: false,
        }
    );
}

/// A head named on the line is the one checked, not the one the worker handed in.
#[test]
fn a_head_named_on_the_line_wins_over_the_handed_in_one() {
    let mut bench = Bench::new();
    let (task, _pane) = handed_in(&mut bench, HEAD);
    let planned = bench.run(&format!("land-check --task {task} --head 1234567"));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let LandCheckAsk::Merge { head, .. } = asked(&planned) else {
        panic!("a merge was asked for")
    };
    assert_eq!(head, "1234567");
}

/// The verb is about one task, so a line without `--task` is refused rather than guessed at.
#[test]
fn a_check_without_a_task_is_refused() {
    let mut bench = Bench::new();
    handed_in(&mut bench, HEAD);
    let planned = bench.run("land-check");
    assert_ne!(planned.reply.exit_code, 0);
    assert!(
        planned.reply.stderr.contains("--task"),
        "{}",
        planned.reply.stderr
    );
    assert!(
        matches!(planned.effect, Effect::None),
        "{:?}",
        planned.effect
    );
}

/// A task no worker handed a commit in for has no head to check, and the ledger says so instead of
/// merging a guess.
#[test]
fn a_check_of_a_task_nobody_handed_in_a_commit_for_is_refused() {
    let mut bench = Bench::new();
    bench.json("run-create --name check");
    let task = bench.json("task-create --spec nothing-handed-in")["taskId"]
        .as_str()
        .expect("a task id")
        .to_string();
    let planned = bench.run(&format!("land-check --task {task}"));
    assert_ne!(planned.reply.exit_code, 0);
    assert!(
        planned.reply.stderr.contains("handed-in"),
        "{}",
        planned.reply.stderr
    );
}

/// `--prepare` stops at the merge and keeps the folder, so the ask says so.
#[test]
fn prepare_is_a_flag_of_the_check_not_a_second_verb() {
    let mut bench = Bench::new();
    let (task, _pane) = handed_in(&mut bench, HEAD);
    let planned = bench.run(&format!("land-check --task {task} --prepare"));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let LandCheckAsk::Merge { prepare, .. } = asked(&planned) else {
        panic!("a merge was asked for")
    };
    assert!(prepare);
}

/// A worker cannot run a check on its own work: the verb is the coordinator's, and the worker is told
/// to say which head it wants checked.
#[test]
fn a_worker_cannot_run_the_coordinators_check() {
    let mut bench = Bench::new();
    let (task, pane) = handed_in(&mut bench, HEAD);
    let planned = bench.at(&pane, &format!("land-check --task {task}"));
    assert_ne!(planned.reply.exit_code, 0);
    assert!(
        planned.reply.stderr.contains("coordinator's verb"),
        "{}",
        planned.reply.stderr
    );
    assert!(
        matches!(planned.effect, Effect::None),
        "{:?}",
        planned.effect
    );
}

/// `--record` ends a prepared check: the check id, the head said back, the exit code and the log
/// the prepared check named, and the time it took.
#[test]
fn a_record_names_its_check_head_exit_code_log_and_time() {
    let mut bench = Bench::new();
    let (task, _pane) = handed_in(&mut bench, HEAD);
    let run = bench.ledger.runs()[0].id.clone();
    let check = format!("{CHECK_PREFIX}1700000000000-1");
    let log = abs_log(&format!("land-check/logs/{check}.log"));
    let planned = bench.run(&format!(
        "land-check --task {task} --head {HEAD} --record {check} --rc 0 --log {log} --took-ms 1200"
    ));
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    assert_eq!(
        asked(&planned),
        LandCheckAsk::Record {
            run,
            task,
            head: HEAD.to_string(),
            check: check.clone(),
            rc: 0,
            log,
            took_ms: Some(1200),
        }
    );
}

/// A record without what it must say is refused: no head to hold it to, no exit code, a log that is
/// not an absolute path, or a check id the window could not have minted.
#[test]
fn a_record_without_its_head_exit_code_or_absolute_log_is_refused() {
    let mut bench = Bench::new();
    let (task, _pane) = handed_in(&mut bench, HEAD);
    let check = format!("{CHECK_PREFIX}1700000000000-1");
    let x_log = abs_log("x.log");
    // Each refusal names what it refused, so a refusal for some other reason does not pass.
    for (line, words) in [
        (
            format!("land-check --task {task} --record {check} --rc 0 --log {x_log}"),
            "--head",
        ),
        (
            format!("land-check --task {task} --head {HEAD} --record {check} --log {x_log}"),
            "--rc",
        ),
        (
            format!(
                "land-check --task {task} --head {HEAD} --record {check} --rc 0 --log relative.log"
            ),
            "absolute",
        ),
        (
            format!(
                "land-check --task {task} --head {HEAD} --record ../{check} --rc 0 --log {x_log}"
            ),
            "--record",
        ),
        (
            format!(
                "land-check --task {task} --head {HEAD} --record {check} --rc zero --log {x_log}"
            ),
            "--rc",
        ),
    ] {
        let planned = bench.run(&line);
        assert_ne!(planned.reply.exit_code, 0, "`{line}` was not refused");
        assert!(
            planned.reply.stderr.contains(words),
            "`{line}` was refused, but not for {words}: {}",
            planned.reply.stderr
        );
        assert!(
            matches!(planned.effect, Effect::None),
            "`{line}`: {:?}",
            planned.effect
        );
    }
}

/// A check id is `lc-<digits>-<digits>` and nothing else: the window builds paths from it.
#[test]
fn a_check_id_is_the_prefix_and_two_numbers_and_nothing_else() {
    assert!(is_check_id("lc-1700000000000-1"));
    assert!(is_check_id("lc-0-0"));
    for bad in [
        "",
        "lc-",
        "lc-1",
        "lc-1-2-3",
        "lc-a-1",
        "lc--1",
        "LC-1-2",
        "lc-1-2/",
        "../lc-1-2",
        "lc-1-2 ",
        "xx-1-2",
    ] {
        assert!(!is_check_id(bad), "{bad:?} is not a check id");
    }
}

/// The window's evidence is written once per check id, as the ledger's own letter to the run's
/// coordinator, with the task named and the evidence kept whole in the payload.
#[test]
fn a_check_letter_reaches_the_coordinator_once_with_its_task_and_evidence() {
    let mut bench = Bench::new();
    let (task, _pane) = handed_in(&mut bench, HEAD);
    let run = bench.ledger.runs()[0].id.clone();
    let check = format!("{CHECK_PREFIX}1700000000000-7");
    let evidence = serde_json::json!({
        "state": "passed",
        "head": HEAD,
        "tree": "0123456789abcdef0123456789abcdef01234567",
        "rc": 0,
        "tookMs": 1200,
        "log": abs_log(&format!("land-check/logs/{check}.log")),
    })
    .to_string();
    let first = bench
        .ledger
        .land_checked(&receipt(&run, &task, &check, &evidence), 5_000)
        .expect("the letter is written");
    assert!(first.is_some(), "the first evidence is written");
    let again = bench
        .ledger
        .land_checked(&receipt(&run, &task, &check, &evidence), 5_001)
        .expect("the same check again is not an error");
    assert_eq!(again, None, "the same check id writes no second letter");

    let held = bench.ledger.run(&run).expect("the run");
    let letters: Vec<&Message> = held
        .messages
        .iter()
        .filter(|row| row.kind == MessageKind::LandCheck)
        .collect();
    assert_eq!(letters.len(), 1, "exactly one letter");
    let letter = letters[0];
    assert_eq!(letter.from, LEDGER_ITSELF);
    assert_eq!(letter.to, held.address());
    assert_eq!(letter.task.as_deref(), Some(task.as_str()));
    assert!(
        letter.body.as_str().starts_with("land-check "),
        "{}",
        letter.body.as_str()
    );
    let kept: serde_json::Value =
        serde_json::from_str(letter.payload.as_str()).expect("the payload is the evidence");
    assert_eq!(kept["check"], check.as_str());
    assert_eq!(kept["task"], task.as_str());
    assert_eq!(kept["state"], "passed");
    assert_eq!(kept["rc"], 0);
}

/// A letter with no state, no JSON object, or a check id the window could not have minted is not
/// written at all.
#[test]
fn a_check_letter_says_its_state_and_names_a_check_id() {
    let mut bench = Bench::new();
    let (task, _pane) = handed_in(&mut bench, HEAD);
    let run = bench.ledger.runs()[0].id.clone();
    let good = format!("{CHECK_PREFIX}1-2");
    for (check, evidence) in [
        (good.as_str(), "{}"),
        (good.as_str(), "[1,2]"),
        (good.as_str(), "not json"),
        ("lc-x-y", "{\"state\":\"passed\"}"),
    ] {
        assert!(
            bench
                .ledger
                .land_checked(&receipt(&run, &task, check, evidence), 9_000)
                .is_err(),
            "{check} {evidence} was written"
        );
    }
}

/// The letter is the ledger's own voice: a worker or a coordinator cannot type one, and the triage
/// reads it as news the coordinator can wait for.
#[test]
fn a_check_letter_is_the_ledgers_own_kind_and_news_to_wait_for() {
    assert_eq!(
        "land_check".parse::<MessageKind>(),
        Ok(MessageKind::LandCheck)
    );
    assert_eq!(MessageKind::LandCheck.as_str(), "land_check");
    assert!(MessageKind::LandCheck.is_the_ledgers_own());
    assert_eq!(kind_rule(MessageKind::LandCheck), Triage::CanWait);
}

/// The verb is in the table as a mutation: it files a receipt and needs `--retry-request`.
#[test]
fn the_check_verb_is_in_the_table_as_a_mutation() {
    let row = VERBS
        .iter()
        .find(|(verb, _, _)| *verb == LAND_CHECK_VERB)
        .expect("land-check is in the verb table");
    assert_eq!(row.2, Doing::Mutation);
}
