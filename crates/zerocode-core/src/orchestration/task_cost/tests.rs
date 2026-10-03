use serde_json::{Value, json};

use super::*;
use crate::usage_ledger::ModelBreakdown;

mod boundaries;

const MINUTE: i64 = 60_000;
const OPUS: &str = "claude-opus-5-5";
const SOL: &str = "gpt-5.6-sol";

fn task_cost(run: &Run, task_id: &str, sessions: &SessionBook, jev: JevTally) -> TaskCost {
    super::task_cost(run, task_id, sessions, jev, &SessionAttribution::new([run]))
}

fn attempt_generation(run: &Run, attempt: &Dispatch, sessions: &SessionBook) -> GenerationCost {
    super::attempt_generation(run, attempt, sessions, &SessionAttribution::new([run]))
}

/// A run holding the attempts, workers and mail of one test, with two tasks
/// written down — the one being costed, and another a shared worker carried.
fn run(mut dispatches: Value, workers: Value, messages: Value) -> Run {
    for dispatch in dispatches.as_array_mut().expect("attempts") {
        if dispatch.get("session_history").is_none() {
            let session = workers
                .as_array()
                .expect("workers")
                .iter()
                .find(|worker| worker["id"] == dispatch["worker"])
                .and_then(|worker| worker.get("session"))
                .filter(|session| !session.is_null());
            dispatch["session_history"] = json!({"complete": true, "sessions": session.into_iter().cloned().collect::<Vec<_>>()});
        }
    }
    let task = |id: &str| {
        json!({
            "id": id, "spec": "", "title": "", "deps": [], "parent": null,
            "status": "completed", "result": "", "failures": 0, "created_ms": 0,
        })
    };
    serde_json::from_value(json!({
        "id": "run-cost", "name": "costs", "created_ms": 0,
        "tasks": [task("t-1"), task("t-2")],
        "dispatches": dispatches, "workers": workers, "messages": messages, "inboxes": [],
    }))
    .expect("a run")
}

fn attempt(id: &str, task: &str, worker: &str, started: i64, ended: Option<i64>) -> Value {
    json!({
        "id": id, "task": task, "worker": worker, "started_ms": started,
        "ended_ms": ended, "succeeded": ended.map(|_| true),
    })
}

/// A worker with the conversation its pane last reported, written down the
/// way the ledger keeps one: the id and where the agent writes it.
fn worker(id: &str, agent: &str, session: Option<&str>) -> Value {
    json!({
        "id": id, "team": "team-cost", "agent": agent, "pane": format!("%{id}"),
        "state": "released", "started_ms": 0, "dispatch": null, "archive": null,
        "session": session.map(|conversation| json!({
            "key": "session_id",
            "id": conversation,
            "transcript_path": format!("/Users/dev/.claude/projects/-Users-dev-repo/{conversation}.jsonl"),
        })),
    })
}

/// The ledger's receipt that a worker's CLI left the model its summons bound
/// mid-conversation (t-6747), written against one attempt.
fn deviated(attempt: &str) -> Value {
    json!({
        "id": "m-deviated", "from": "ledger", "to": "run:run-cost", "kind": "model_deviated",
        "body": "{}", "thread": null, "task": "t-1", "dispatch": attempt, "created_ms": 0,
    })
}

fn claude_session(id: &str, model: &str, tokens: [i64; 4]) -> usage_stats::Session {
    let [input, output, cache_read, cache_write] = tokens;
    usage_stats::Session {
        session_id: id.to_string(),
        first_timestamp: "2026-09-26T00:00:00Z".to_string(),
        last_timestamp: "2026-09-26T01:00:00Z".to_string(),
        model: Some(model.to_string()),
        last_cwd: Some("/Users/dev/repo".to_string()),
        last_git_branch: Some("wt/t-1".to_string()),
        turn_count: 3,
        total_input_tokens: input,
        total_output_tokens: output,
        total_cache_read_tokens: cache_read,
        total_cache_write_tokens: cache_write,
        location_breakdown: Vec::new(),
    }
}

fn claude(sessions: Vec<usage_stats::Session>) -> usage_stats::Ledger {
    usage_stats::Ledger {
        sessions,
        daily_aggregates: Vec::new(),
    }
}

/// A Codex conversation: `input` includes `cached`, `output` includes the
/// reasoning — the vendor's own counters, as its rollout reports them.
fn codex_session(
    id: &str,
    models: &[&str],
    input: i64,
    cached: i64,
    output: i64,
) -> usage_ledger::Session {
    usage_ledger::Session {
        session_id: id.to_string(),
        first_timestamp: "2026-09-26T00:00:00Z".to_string(),
        last_timestamp: "2026-09-26T01:00:00Z".to_string(),
        model: models.last().map(|model| (*model).to_string()),
        last_cwd: Some("/Users/dev/repo".to_string()),
        event_count: 2,
        input_tokens: input,
        cached_input_tokens: cached,
        output_tokens: output,
        reasoning_output_tokens: output / 4,
        total_tokens: input + output,
        location_breakdown: Vec::new(),
        model_breakdown: models
            .iter()
            .map(|model| ModelBreakdown {
                model_key: (*model).to_string(),
                event_count: 1,
                total_tokens: input + output,
            })
            .collect(),
        estimated_cost_usd: None,
    }
}

fn vendor(sessions: Vec<usage_ledger::Session>) -> usage_ledger::Ledger {
    usage_ledger::Ledger {
        sessions,
        daily_aggregates: Vec::new(),
    }
}

/// A scan read an hour after everything in these tests ended.
const READ_AT: i64 = 600 * MINUTE;

fn book_of(claude_sessions: Vec<usage_stats::Session>) -> SessionBook {
    let mut book = SessionBook::default();
    book.read_claude(&claude(claude_sessions), READ_AT);
    book
}

fn close(left: Option<f64>, right: f64) -> bool {
    left.is_some_and(|left| (left - right).abs() < 1e-9)
}

fn with_history(mut attempt: Value, sessions: &[&str]) -> Value {
    attempt["session_history"] = json!({
        "complete": true,
        "sessions": sessions.iter().map(|id| json!({"key": "session_id", "id": id})).collect::<Vec<_>>()
    });
    attempt
}

#[test]
fn every_session_of_an_attempt_contributes_to_its_total() {
    let held = run(
        json!([with_history(
            attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE)),
            &["conv-first", "conv-second"]
        )]),
        json!([worker("w-1", "claude", Some("conv-second"))]),
        json!([]),
    );
    let book = book_of(vec![
        claude_session("conv-first", OPUS, [100, 10, 0, 0]),
        claude_session("conv-second", OPUS, [200, 20, 0, 0]),
    ]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(
        (
            cost.generation.sessions_known,
            cost.generation.sessions_linked
        ),
        (2, 2)
    );
    assert_eq!(
        (cost.generation.input_tokens, cost.generation.output_tokens),
        (300, 30)
    );
    assert!(cost.generation.usd.is_some());
}

#[test]
fn a_reused_worker_keeps_distinct_task_sessions_separate() {
    let held = run(
        json!([
            with_history(
                attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE)),
                &["conv-first"]
            ),
            with_history(
                attempt("dp-2", "t-2", "w-1", 2 * MINUTE, Some(3 * MINUTE)),
                &["conv-second"]
            ),
        ]),
        json!([worker("w-1", "claude", Some("conv-second"))]),
        json!([]),
    );
    let book = book_of(vec![
        claude_session("conv-first", OPUS, [100, 10, 0, 0]),
        claude_session("conv-second", OPUS, [900, 90, 0, 0]),
    ]);
    let first = task_cost(&held, "t-1", &book, JevTally::default());
    let second = task_cost(&held, "t-2", &book, JevTally::default());
    assert_eq!(
        (
            first.generation.input_tokens,
            second.generation.input_tokens
        ),
        (100, 900)
    );
    assert!(first.generation.usd.is_some() && second.generation.usd.is_some());
}

#[test]
fn a_session_shared_by_distinct_workers_on_different_tasks_is_unlinked() {
    let held = run(
        json!([
            with_history(
                attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE)),
                &["conv-shared"]
            ),
            with_history(
                attempt("dp-2", "t-2", "w-2", 0, Some(MINUTE)),
                &["conv-shared"]
            ),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-shared")),
            worker("w-2", "claude", Some("conv-shared"))
        ]),
        json!([]),
    );
    let book = book_of(vec![claude_session("conv-shared", OPUS, [100, 10, 0, 0])]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd_reason, Some(UsdReason::Unlinked));
    assert_eq!(cost.generation.usd, None);
}

#[test]
fn equal_session_ids_on_different_providers_are_not_the_same_usage() {
    let held = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE)),
            attempt("dp-2", "t-1", "w-2", 0, Some(MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-shared-name")),
            worker("w-2", "codex", Some("conv-shared-name"))
        ]),
        json!([]),
    );
    let mut book = book_of(vec![claude_session(
        "conv-shared-name",
        OPUS,
        [100, 10, 0, 0],
    )]);
    book.read_codex(
        &vendor(vec![codex_session(
            "conv-shared-name",
            &[SOL],
            400,
            100,
            20,
        )]),
        READ_AT,
    );
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(
        (
            cost.generation.sessions_linked,
            cost.generation.input_tokens
        ),
        (2, 400)
    );
}

#[test]
fn distinct_attempt_sessions_on_one_worker_can_be_costed_independently() {
    let held = run(
        json!([
            with_history(
                attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE)),
                &["conv-first"]
            ),
            with_history(
                attempt("dp-2", "t-1", "w-1", 2 * MINUTE, Some(3 * MINUTE)),
                &["conv-second"]
            ),
        ]),
        json!([worker("w-1", "claude", Some("conv-second"))]),
        json!([]),
    );
    let book = book_of(vec![
        claude_session("conv-first", OPUS, [100, 10, 0, 0]),
        claude_session("conv-second", OPUS, [900, 90, 0, 0]),
    ]);
    let first = attempt_generation(&held, &held.dispatches[0], &book);
    let second = attempt_generation(&held, &held.dispatches[1], &book);
    assert_eq!((first.input_tokens, second.input_tokens), (100, 900));
    assert!(first.usd.is_some() && second.usd.is_some());
}

#[test]
fn legacy_session_history_is_not_reported_as_a_complete_bill() {
    let mut old = attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE));
    old["session_history"] = Value::Null;
    let held = run(
        json!([old]),
        json!([worker("w-1", "claude", Some("conv-last"))]),
        json!([]),
    );
    let book = book_of(vec![claude_session("conv-last", OPUS, [100, 10, 0, 0])]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    let value = serde_json::to_value(&cost).expect("public cost");
    assert_eq!(value["generation"]["usd"], Value::Null);
    assert_eq!(value["generation"]["historyComplete"], false);
    assert_eq!(
        value["generation"]["usdReason"],
        "incomplete_session_history"
    );
}

/// A task is its attempts — every dispatch of it, the rows `newest_attempt`
/// picks the newest of — and its wall clock runs from the first start to the
/// last end, the wait between the two attempts included. An attempt still
/// open leaves the clock unread.
#[test]
fn a_task_is_its_attempts_and_its_clock_runs_from_the_first_start_to_the_last_end() {
    let held = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 10 * MINUTE, Some(40 * MINUTE)),
            attempt("dp-9", "t-2", "w-9", 0, Some(5 * MINUTE)),
            attempt("dp-2", "t-1", "w-2", 70 * MINUTE, Some(100 * MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-a")),
            worker("w-2", "claude", Some("conv-b")),
            worker("w-9", "claude", Some("conv-z")),
        ]),
        json!([]),
    );
    let cost = task_cost(&held, "t-1", &SessionBook::default(), JevTally::default());
    assert_eq!(cost.attempts, 2, "{cost:?}");
    assert_eq!(cost.wall_ms, Some(90 * MINUTE), "{cost:?}");
    assert_eq!(
        held.newest_attempt("t-1").map(|one| one.id.as_str()),
        Some("dp-2"),
        "the attempts counted are not the ledger's"
    );

    let open = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 10 * MINUTE, Some(40 * MINUTE)),
            attempt("dp-2", "t-1", "w-2", 70 * MINUTE, None),
        ]),
        json!([worker("w-1", "claude", None), worker("w-2", "claude", None)]),
        json!([]),
    );
    let cost = task_cost(&open, "t-1", &SessionBook::default(), JevTally::default());
    assert_eq!((cost.attempts, cost.wall_ms), (2, None), "{cost:?}");
}

/// Two attempts on two vendors: each conversation is found by the provider's
/// own id and priced at its own model the vendor's way — Claude's four
/// counters as they are, Codex's cached input taken out of its input — and
/// the task's dollars are their sum.
#[test]
fn each_conversation_is_priced_at_its_own_model_and_the_task_sums_them() {
    let held = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-2", "t-1", "w-2", 40 * MINUTE, Some(90 * MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-claude")),
            worker("w-2", "codex", Some("conv-codex")),
        ]),
        json!([]),
    );
    let mut book = book_of(vec![claude_session(
        "conv-claude",
        OPUS,
        [1_000, 20_000, 300_000, 40_000],
    )]);
    book.read_codex(
        &vendor(vec![codex_session(
            "conv-codex",
            &[SOL],
            50_000,
            30_000,
            7_000,
        )]),
        READ_AT,
    );
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    let generation = &cost.generation;
    assert_eq!(
        (generation.sessions_known, generation.sessions_linked),
        (2, 2),
        "{cost:?}"
    );
    assert_eq!(
        generation.input_tokens,
        1_000 + (50_000 - 30_000),
        "{cost:?}"
    );
    assert_eq!(generation.output_tokens, 20_000 + 7_000, "{cost:?}");
    assert_eq!(generation.cache_read_tokens, 300_000 + 30_000, "{cost:?}");
    assert_eq!(generation.cache_write_tokens, 40_000, "{cost:?}");
    let claude_usd = usage_stats::estimate_cost_usd(Some(OPUS), 1_000, 20_000, 300_000, 40_000)
        .expect("opus is priced");
    let codex_usd = usage_stats_codex::estimate_cost_usd(Some(SOL), 50_000, 30_000, 7_000)
        .expect("sol is priced");
    assert!(
        close(generation.usd, claude_usd + codex_usd),
        "{cost:?} against {claude_usd} + {codex_usd}"
    );
    assert_eq!(generation.usd_reason, None);
}

/// zo keeps no usage ledger this window reads: its attempt's dollars are
/// unknown, not zero, and so are the task's.
#[test]
fn an_agent_with_no_usage_ledger_leaves_the_dollars_unknown_not_zero() {
    let held = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-2", "t-1", "w-2", 40 * MINUTE, Some(90 * MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-claude")),
            worker("w-2", "zo", Some("conv-zo")),
        ]),
        json!([]),
    );
    let book = book_of(vec![claude_session("conv-claude", OPUS, [10, 20, 30, 40])]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd, None, "{cost:?}");
    assert_eq!(
        cost.generation.usd_reason,
        Some(UsdReason::UnsupportedAgent),
        "{cost:?}"
    );
    assert_eq!(
        (
            cost.generation.sessions_known,
            cost.generation.sessions_linked
        ),
        (2, 1),
        "{cost:?}"
    );
    assert_eq!(cost.generation.input_tokens, 10, "{cost:?}");
}

/// A switch of model the ledger wrote against an attempt (t-6747) leaves the
/// conversation priced at no single model: the dollars are unknown, the
/// tokens still counted.
#[test]
fn a_switch_of_model_written_against_an_attempt_leaves_its_dollars_unknown() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE))]),
        json!([worker("w-1", "claude", Some("conv-claude"))]),
        json!([deviated("dp-1")]),
    );
    let book = book_of(vec![claude_session("conv-claude", OPUS, [10, 20, 30, 40])]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd, None, "{cost:?}");
    assert_eq!(cost.generation.usd_reason, Some(UsdReason::MixedModels));
    assert_eq!(cost.generation.sessions_linked, 1, "{cost:?}");
    assert_eq!(cost.generation.output_tokens, 20, "{cost:?}");

    // The same switch written against ANOTHER attempt does not touch this one.
    let elsewhere = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-9", "t-2", "w-9", 0, Some(30 * MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-claude")),
            worker("w-9", "claude", Some("conv-other")),
        ]),
        json!([deviated("dp-9")]),
    );
    let cost = task_cost(&elsewhere, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd_reason, None, "{cost:?}");
}

/// A Codex conversation whose own ledger saw two models is priced at no one
/// of them.
#[test]
fn a_conversation_its_vendor_saw_use_two_models_is_mixed() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE))]),
        json!([worker("w-1", "codex", Some("conv-codex"))]),
        json!([]),
    );
    let mut book = SessionBook::default();
    book.read_codex(
        &vendor(vec![codex_session(
            "conv-codex",
            &["gpt-5.6-luna", SOL],
            50_000,
            30_000,
            7_000,
        )]),
        READ_AT,
    );
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd, None, "{cost:?}");
    assert_eq!(cost.generation.usd_reason, Some(UsdReason::MixedModels));
    assert_eq!(cost.generation.input_tokens, 20_000, "{cost:?}");
}

/// A model no price table names is unpriced, never billed at a neighbour's
/// rate.
#[test]
fn a_model_with_no_price_leaves_the_dollars_unknown() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE))]),
        json!([worker("w-1", "claude", Some("conv-claude"))]),
        json!([]),
    );
    let book = book_of(vec![claude_session(
        "conv-claude",
        "a-model-nobody-priced",
        [10, 20, 30, 40],
    )]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd, None, "{cost:?}");
    assert_eq!(cost.generation.usd_reason, Some(UsdReason::UnpricedModel));
    assert_eq!(cost.generation.sessions_linked, 1, "{cost:?}");
}

/// A scan that is not held, or was read before the attempt ended, is not the
/// conversation's total — and a conversation the scan does not hold is known
/// to the ledger and linked to nothing.
#[test]
fn a_scan_read_before_the_work_ended_or_missing_the_conversation_is_not_its_total() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE))]),
        json!([worker("w-1", "claude", Some("conv-claude"))]),
        json!([]),
    );
    let never = task_cost(&held, "t-1", &SessionBook::default(), JevTally::default());
    assert_eq!(
        never.generation.usd_reason,
        Some(UsdReason::Unscanned),
        "{never:?}"
    );
    assert_eq!(never.generation.usd, None);

    let mut early = SessionBook::default();
    early.read_claude(
        &claude(vec![claude_session("conv-claude", OPUS, [10, 20, 30, 40])]),
        29 * MINUTE,
    );
    let stale = task_cost(&held, "t-1", &early, JevTally::default());
    assert_eq!(
        stale.generation.usd_reason,
        Some(UsdReason::Unscanned),
        "{stale:?}"
    );
    assert_eq!(
        (
            stale.generation.sessions_known,
            stale.generation.sessions_linked
        ),
        (1, 0),
        "{stale:?}"
    );
    assert_eq!(stale.generation.input_tokens, 0, "{stale:?}");

    let missing = book_of(vec![claude_session("conv-another", OPUS, [10, 20, 30, 40])]);
    let unheld = task_cost(&held, "t-1", &missing, JevTally::default());
    assert_eq!(
        unheld.generation.usd_reason,
        Some(UsdReason::Unlinked),
        "{unheld:?}"
    );
    assert_eq!(
        (
            unheld.generation.sessions_known,
            unheld.generation.sessions_linked
        ),
        (1, 0),
        "{unheld:?}"
    );
}

/// The worker row keeps its last conversation only, so the ledger knows
/// conversations, not attempts: an attempt that reported none is a gap the
/// dollars refuse to hide, and two attempts in one conversation are one.
#[test]
fn the_ledger_knows_conversations_not_attempts() {
    let silent = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-2", "t-1", "w-2", 40 * MINUTE, Some(90 * MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", None),
            worker("w-2", "claude", Some("conv-claude")),
        ]),
        json!([]),
    );
    let book = book_of(vec![claude_session("conv-claude", OPUS, [10, 20, 30, 40])]);
    let cost = task_cost(&silent, "t-1", &book, JevTally::default());
    assert_eq!(cost.attempts, 2, "{cost:?}");
    assert_eq!(
        (
            cost.generation.sessions_known,
            cost.generation.sessions_linked
        ),
        (1, 1),
        "{cost:?}"
    );
    assert_eq!(
        cost.generation.usd_reason,
        Some(UsdReason::Unlinked),
        "{cost:?}"
    );

    let again = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-2", "t-1", "w-1", 40 * MINUTE, Some(90 * MINUTE)),
        ]),
        json!([worker("w-1", "claude", Some("conv-claude"))]),
        json!([]),
    );
    let cost = task_cost(&again, "t-1", &book, JevTally::default());
    assert_eq!(cost.attempts, 2, "{cost:?}");
    assert_eq!(
        (
            cost.generation.sessions_known,
            cost.generation.sessions_linked
        ),
        (1, 1),
        "{cost:?}"
    );
    assert_eq!(cost.generation.input_tokens, 10, "counted twice: {cost:?}");
    assert!(cost.generation.usd.is_some(), "{cost:?}");
}

/// A conversation whose worker carried another task's attempt too holds both
/// tasks' work, and no rule splits it: it is known, and tied to neither.
#[test]
fn a_conversation_another_task_shares_is_tied_to_neither() {
    let held = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-2", "t-2", "w-1", 40 * MINUTE, Some(90 * MINUTE)),
        ]),
        json!([worker("w-1", "claude", Some("conv-claude"))]),
        json!([]),
    );
    let book = book_of(vec![claude_session("conv-claude", OPUS, [10, 20, 30, 40])]);
    for task in ["t-1", "t-2"] {
        let cost = task_cost(&held, task, &book, JevTally::default());
        assert_eq!(
            (
                cost.generation.sessions_known,
                cost.generation.sessions_linked
            ),
            (1, 0),
            "{task}: {cost:?}"
        );
        assert_eq!(
            cost.generation.usd_reason,
            Some(UsdReason::Unlinked),
            "{task}"
        );
        assert_eq!(cost.generation.input_tokens, 0, "{task}: {cost:?}");
    }
}

/// OpenCode reports its own dollars per turn, and a conversation's are their
/// sum — carried, not priced again.
#[test]
fn opencode_carries_the_dollars_it_reported() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE))]),
        json!([worker("w-1", "opencode", Some("conv-open"))]),
        json!([]),
    );
    let mut reported = codex_session("conv-open", &["anthropic/claude-sonnet-5"], 400, 100, 60);
    reported.reasoning_output_tokens = 15;
    reported.estimated_cost_usd = Some(0.25);
    let mut book = SessionBook::default();
    book.read_opencode(&vendor(vec![reported]), READ_AT);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert!(close(cost.generation.usd, 0.25), "{cost:?}");
    assert_eq!(
        cost.generation.input_tokens, 400,
        "separate buckets: {cost:?}"
    );
    assert_eq!(cost.generation.cache_read_tokens, 100, "{cost:?}");
    assert_eq!(cost.generation.output_tokens, 60 + 15, "{cost:?}");
}

/// Only the rows stamped with the task count, and only what they asked: a
/// label, the judge's own note and another task's row add nothing. The seats
/// read are the stamping ones, and the rest are said as a number.
#[test]
fn cached_shared_and_refused_jev_rows_bill_no_new_tokens() {
    let mut book = JevBook::default();
    for row in [
        json!({"task": "t-1", "outcome": "answered", "requests": 1, "inputTokens": 90, "outputTokens": 10}),
        json!({"task": "t-1", "outcome": "answered", "requests": 1, "cached": true, "inputTokens": 900}),
        json!({"task": "t-1", "outcome": "answered", "requests": 0, "inputTokens": 800}),
        json!({"task": "t-1", "outcome": "not_consented"}),
    ] {
        book.read(&row);
    }
    let cost = JevCost::of(book.tally("t-1"));
    assert_eq!(cost.requests, 1);
    assert_eq!(cost.input_tokens, Some(90));
}

#[test]
fn invalid_session_counters_do_not_become_negative_bills_or_learning_evidence() {
    let held = run(
        json!([attempt("dp-1", "t-1", "w-1", 0, Some(MINUTE))]),
        json!([worker("w-1", "claude", Some("conv-invalid"))]),
        json!([]),
    );
    let book = book_of(vec![claude_session("conv-invalid", OPUS, [-1, 4, 0, 0])]);
    let cost = task_cost(&held, "t-1", &book, JevTally::default());
    assert_eq!(cost.generation.usd, None);
    assert_eq!(cost.generation.input_tokens, 0);
    assert_eq!(cost.generation.measured_tokens(), None);
}

#[test]
fn jev_counts_the_requests_of_the_rows_stamped_with_the_task() {
    let mut jev = JevBook::default();
    for row in [
        json!({"at": 1, "task": "t-1", "outcome": "answered", "requests": 1, "mode": "auto"}),
        json!({"at": 2, "task": "t-1", "outcome": "answered", "requests": 2, "mode": "on"}),
        json!({"at": 3, "task": "t-1", "outcome": "not_consented", "requests": 0}),
        json!({"at": 4, "task": "t-1", "label": "w-1", "agreed": true}),
        json!({"at": 5, "task": "t-1", "transition": "rose", "outcome": "answered", "requests": 9}),
        json!({"at": 6, "task": "t-2", "outcome": "answered", "requests": 5}),
        json!({"at": 7, "outcome": "answered", "requests": 7}),
    ] {
        jev.read(&row);
    }
    let held = run(json!([]), json!([]), json!([]));
    let cost = task_cost(&held, "t-1", &SessionBook::default(), jev.tally("t-1"));
    assert_eq!(cost.jev.requests, 3, "{cost:?}");
    assert_eq!(cost.jev.stamped_seats, TASK_STAMPED.len());
    assert_eq!(
        cost.jev.stamped_seats + cost.jev.unstamped_seats,
        crate::jev::JEV_USES.len()
    );
    assert_eq!(
        cost.jev.input_tokens, None,
        "no counted row recorded tokens"
    );
    assert_eq!(jev.tally("t-2").requests, 5);

    let mut billed = JevBook::default();
    billed.read(&json!({"task": "t-1", "outcome": "answered", "requests": 1, "inputTokens": 900}));
    billed.read(&json!({"task": "t-1", "outcome": "answered", "requests": 1, "input_tokens": 100}));
    let cost = task_cost(&held, "t-1", &SessionBook::default(), billed.tally("t-1"));
    assert_eq!(cost.jev.input_tokens, Some(1_000), "{cost:?}");

    // Two ledgers' rows of one task, together — and a ledger that recorded
    // no tokens leaves the sum unrecorded.
    let both = task_cost(
        &held,
        "t-1",
        &SessionBook::default(),
        jev.tally("t-1") + billed.tally("t-1"),
    );
    assert_eq!(both.jev.requests, 5, "{both:?}");
    assert_eq!(both.jev.input_tokens, None, "{both:?}");
}

/// The seats named as stamping a task are the window's own, each writing
/// its rows to its own ledger.
#[test]
fn the_stamping_seats_are_distinct_ledgers() {
    let ledgers: HashSet<&str> = TASK_STAMPED.iter().map(|seat| seat.ledger).collect();
    assert_eq!(ledgers.len(), TASK_STAMPED.len());
    for seat in TASK_STAMPED {
        assert!(
            crate::jev::JEV_USES.iter().any(|one| one.id == seat.id),
            "{} is not a seat",
            seat.id
        );
    }
}

/// The cost leaves the backend with numbers and words only: no conversation
/// id, no transcript path, no working directory — the three things the
/// ledger and the scan hold beside it. Model names may go; none does.
#[test]
fn the_cost_carries_no_conversation_id_path_or_working_directory() {
    let held = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-2", "t-1", "w-2", 40 * MINUTE, Some(90 * MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-secret-claude")),
            worker("w-2", "codex", Some("conv-secret-codex")),
        ]),
        json!([]),
    );
    let mut book = book_of(vec![claude_session(
        "conv-secret-claude",
        OPUS,
        [10, 20, 30, 40],
    )]);
    book.read_codex(
        &vendor(vec![codex_session("conv-secret-codex", &[SOL], 50, 30, 7)]),
        READ_AT,
    );
    let mut jev = JevBook::default();
    jev.read(&json!({"task": "t-1", "outcome": "answered", "requests": 1}));
    let cost = task_cost(&held, "t-1", &book, jev.tally("t-1"));
    assert_eq!(cost.generation.sessions_linked, 2, "{cost:?}");
    let said = serde_json::to_string(&cost).expect("the cost serializes");
    for private in [
        "conv-secret",
        "/Users",
        ".jsonl",
        "wt/t-1",
        "\"session\":",
        "\"sessionId\":",
        "\"transcriptPath\":",
        "\"path\":",
        "\"cwd\":",
    ] {
        assert!(!said.contains(private), "`{private}` left in {said}");
    }
    let value: Value = serde_json::from_str(&said).expect("json");
    for key in ["attempts", "wallMs", "generation", "jev"] {
        assert!(value.get(key).is_some(), "`{key}` missing from {said}");
    }
    assert_eq!(value["generation"]["usdReason"], Value::Null);
    assert!(value["generation"]["sessionsKnown"].is_u64());
}

/// Why the dollars are unknown is said once, the most fundamental reason
/// first — an agent nobody can read before a scan nobody read.
#[test]
fn the_reason_said_is_the_most_fundamental() {
    let held = run(
        json!([
            attempt("dp-1", "t-1", "w-1", 0, Some(30 * MINUTE)),
            attempt("dp-2", "t-1", "w-2", 40 * MINUTE, Some(90 * MINUTE)),
        ]),
        json!([
            worker("w-1", "claude", Some("conv-claude")),
            worker("w-2", "zo", Some("conv-zo")),
        ]),
        json!([deviated("dp-1")]),
    );
    let cost = task_cost(&held, "t-1", &SessionBook::default(), JevTally::default());
    assert_eq!(
        cost.generation.usd_reason,
        Some(UsdReason::UnsupportedAgent),
        "{cost:?}"
    );
    let serialized = serde_json::to_value(&cost).expect("json");
    assert_eq!(serialized["generation"]["usdReason"], "unsupported_agent");
}
