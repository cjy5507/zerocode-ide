//! A registered graph runs as host work inside the normal turn event pump.
//! The registry carries the current permission policy; no parent model is
//! asked to decide whether to invoke the registered workflow.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel};
use runtime::{BudgetExhausted, ContentBlock, ConversationMessage, HookAbortSignal, TokenUsage, TurnSummary};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc::Sender;

const STOP_POLL: Duration = Duration::from_millis(50);
const MAX_LABEL_CHARS: usize = 128;
const MAX_NOTE_CHARS: usize = 240;

pub(super) struct ScheduledResult {
    pub summary: TurnSummary,
    pub status: String,
}

pub(super) async fn run(
    registry: tools::GlobalToolRegistry,
    input: Value,
    blocks: Sender<RenderBlock>,
    stop: HookAbortSignal,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<ScheduledResult, String> {
    let expected_run = input.get("runId").and_then(Value::as_str).map(str::to_string);
    run_with(
        move || execute_registered(&registry, &input),
        tools::request_foreground_workflow_cancel,
        expected_run.as_deref(),
        blocks,
        stop,
        cancelled,
        timeout,
    )
    .await
}

fn execute_registered(registry: &tools::GlobalToolRegistry, input: &Value) -> Result<String, String> {
    registry.execute_workflow_for_host(input).or_else(|error| {
        known_refusal_receipt(&error, input).ok_or(error)
    }).map_err(|error| error.to_string())
}

/// These failures occur before Workflow starts its engine. Once execution
/// starts, ordinary node failures are returned inside the structured report.
fn known_refusal_receipt(error: &tools::ToolError, input: &Value) -> Option<String> {
    matches!(error, tools::ToolError::PermissionDenied { .. }
        | tools::ToolError::InvalidInput(_) | tools::ToolError::NotFound(_)).then(|| {
        json!({
            "status":"blocked", "output_tokens":0,
            "graph":{"run_id":input.get("runId").and_then(Value::as_str), "accounting_complete":true},
            "notes":[bounded_line(&error.to_string(), MAX_NOTE_CHARS)],
        }).to_string()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopReason {
    Requested,
    Deadline,
    OutputClosed,
}

// The injected work/cancel pair exercises the same wait and accounting path
// without starting agents, shells, or providers in a test.
#[allow(clippy::too_many_arguments)]
async fn run_with<F, C>(
    work: F,
    request_cancel: C,
    expected_run: Option<&str>,
    blocks: Sender<RenderBlock>,
    stop: HookAbortSignal,
    cancelled: Arc<AtomicBool>,
    timeout: Duration,
) -> Result<ScheduledResult, String>
where
    F: FnOnce() -> Result<String, String> + Send + 'static,
    C: Fn(),
{
    if stop.is_aborted() || cancelled.load(Ordering::SeqCst) || timeout.is_zero() {
        return Err("scheduled graph stopped before execution".to_string());
    }
    let ids = BlockIdGen::default();
    blocks.send(RenderBlock::System {
        id: ids.next(),
        level: SystemLevel::Info,
        text: "scheduled graph: running the registered workflow".to_string(),
    }).await.map_err(|_| "scheduled graph output closed before execution".to_string())?;

    let worker_stop = stop.clone();
    let worker_cancelled = Arc::clone(&cancelled);
    let mut worker = tokio::task::spawn_blocking(move || {
        if worker_stop.is_aborted() || worker_cancelled.load(Ordering::SeqCst) {
            Err("scheduled graph stopped before execution".to_string())
        } else {
            work()
        }
    });
    let deadline = tokio::time::Instant::now() + timeout;
    let mut stop_reason = None;
    let result = loop {
        tokio::select! {
            biased;
            result = &mut worker => break result.map_err(|error| format!("scheduled graph worker failed: {error}"))?,
            () = tokio::time::sleep(STOP_POLL) => {
                if stop_reason.is_none() {
                    stop_reason = if stop.is_aborted() || cancelled.load(Ordering::SeqCst) {
                        Some(StopReason::Requested)
                    } else if blocks.is_closed() {
                        Some(StopReason::OutputClosed)
                    } else if tokio::time::Instant::now() >= deadline {
                        Some(StopReason::Deadline)
                    } else { None };
                }
                if stop_reason.is_some() {
                    // The engine clears its foreground signal when it starts.
                    // Repeat the stop until this exact blocking worker exits,
                    // including a stop racing with that startup clear.
                    request_cancel();
                }
            }
        }
    };
    let raw = result?;
    let (summary, notice) = summarize(&raw, expected_run, stop_reason)?;
    let _ = blocks.send(RenderBlock::TextDelta {
        id: ids.next(), text: notice, done: true,
    }).await;
    Ok(summary)
}

#[derive(Debug, Deserialize)]
struct GraphReport {
    status: GraphStatus,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    graph: Option<GraphIdentity>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GraphIdentity {
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    accounting_complete: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum GraphStatus {
    Completed,
    Failed,
    Blocked,
    Cancelled,
    BudgetExhausted,
}

impl GraphStatus {
    const fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
            Self::Cancelled => "cancelled",
            Self::BudgetExhausted => "budget_exhausted",
        }
    }
}

fn summarize(raw: &str, expected_run: Option<&str>, stop: Option<StopReason>) -> Result<(ScheduledResult, String), String> {
    let report: GraphReport = serde_json::from_str(raw)
        .map_err(|_| "scheduled graph returned an invalid status or accounting report".to_string())?;
    if !report.graph.as_ref().is_some_and(|graph| graph.accounting_complete) {
        return Err("scheduled graph did not report complete child usage; further automatic work must stop".to_string());
    }
    let reported_run = report.graph.as_ref().and_then(|graph| graph.run_id.as_deref());
    if reported_run.is_some() && expected_run.is_some() && reported_run != expected_run {
        return Err("scheduled graph returned another run's report".to_string());
    }
    if report.status == GraphStatus::Completed && reported_run.is_none() {
        return Err("scheduled graph reported completion without its run identity".to_string());
    }
    let run = bounded_line(reported_run.or(expected_run).unwrap_or("unknown"), MAX_LABEL_CHARS);
    let status = report.status.label();
    let state = match stop {
        None => status.to_string(),
        Some(StopReason::Requested) => format!("stop requested; graph reported {status}"),
        Some(StopReason::Deadline) => format!("deadline reached; graph reported {status}"),
        Some(StopReason::OutputClosed) => format!("output closed; graph reported {status}"),
    };
    let mut notice = format!("scheduled graph {run}: {state} · output {} tokens", report.output_tokens);
    if report.status != GraphStatus::Completed {
        for note in report.notes.iter().take(2) {
            let note = bounded_line(note, MAX_NOTE_CHARS);
            if !note.is_empty() { notice.push('\n'); notice.push_str(&note); }
        }
    }
    let output_tokens = u32::try_from(report.output_tokens).unwrap_or(u32::MAX);
    let budget_exhausted = if stop == Some(StopReason::Deadline) {
        Some(BudgetExhausted::Deadline)
    } else if report.output_tokens > u64::from(u32::MAX) {
        notice.push_str("\nThe session token counter saturated; the full charge is recorded in the graph report.");
        Some(BudgetExhausted::OutputTokens)
    } else { None };
    let summary = TurnSummary {
        assistant_messages: vec![ConversationMessage::assistant(vec![ContentBlock::Text { text: notice.clone() }])],
        tool_results: Vec::new(),
        prompt_cache_events: Vec::new(),
        iterations: 0,
        // There was no parent model request. Child spend is charged by the
        // explicit turn delta rather than attributed to the parent's model.
        usage: TokenUsage::default(),
        turn_output_tokens: output_tokens,
        auto_compaction: None,
        microcompact: None,
        deep_verification: None,
        verification_issues: if report.status == GraphStatus::Completed && stop.is_none() { Vec::new() } else { vec![notice.clone()] },
        deep_verifier_parse: None,
        deep_verifier_model: None,
        budget_exhausted,
    };
    Ok((ScheduledResult { summary, status: status.to_string() }, notice))
}

fn bounded_line(text: &str, max_chars: usize) -> String {
    text.chars().filter(|character| !character.is_control()).take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn report(status: &str, tokens: u64) -> String {
        json!({"status":status,"output_tokens":tokens,"graph":{"run_id":"cron-run-1", "accounting_complete":true}}).to_string()
    }

    fn notice(summary: &TurnSummary) -> &str {
        let ContentBlock::Text { text } = &summary.assistant_messages[0].blocks[0] else {
            panic!("one plain summary message");
        };
        text
    }

    #[test]
    fn graph_failures_keep_their_token_charge_and_never_claim_verified_success() {
        for status in ["completed", "failed", "blocked", "budget_exhausted", "cancelled"] {
            let (result, text) = summarize(&report(status, 73), Some("cron-run-1"), None).unwrap();
            let summary = result.summary;
            assert_eq!(result.status, status);
            assert_eq!(summary.turn_output_tokens, 73);
            assert_eq!(summary.usage, TokenUsage::default(), "no parent model request");
            assert_eq!(summary.iterations, 0);
            assert!(text.contains(status), "{text}");
            assert_eq!(summary.deep_verification, None);
            assert_eq!(summary.verification_issues.is_empty(), status == "completed");
        }
    }

    #[test]
    fn malformed_status_usage_or_identity_cannot_become_success() {
        for raw in [
            "{}", "not json", r#"{"status":"done"}"#,
            r#"{"status":"completed","output_tokens":-1}"#,
            r#"{"status":"completed","output_tokens":"7"}"#,
            r#"{"status":"completed","output_tokens":7}"#,
            r#"{"status":"completed","graph":{"run_id":"another-run","accounting_complete":true}}"#,
        ] {
            assert!(summarize(raw, Some("cron-run-1"), None).is_err(), "{raw}");
        }
    }

    #[test]
    fn a_known_pre_execution_refusal_has_a_verified_zero_cost_receipt() {
        let input = json!({"runId":"cron-run-1"});
        for error in [
            tools::ToolError::PermissionDenied { tool:"Workflow".into(), reason:"ask rule".into() },
            tools::ToolError::InvalidInput("bad graph".into()),
            tools::ToolError::NotFound("Workflow".into()),
        ] {
            let raw = known_refusal_receipt(&error, &input).expect("pre-execution refusal");
            let (result, text) = summarize(&raw, Some("cron-run-1"), None).unwrap();
            assert_eq!(result.status, "blocked");
            assert_eq!(result.summary.turn_output_tokens, 0);
            assert!(text.contains("blocked"));
        }
        assert!(known_refusal_receipt(&tools::ToolError::Execution("worker result lost".into()), &input).is_none());
        assert!(known_refusal_receipt(&tools::ToolError::Io(std::io::Error::other("write failed")), &input).is_none());
    }

    #[test]
    fn missing_or_incomplete_child_accounting_refuses_even_a_completed_graph() {
        for graph in [
            json!({"run_id":"cron-run-1", "accounting_complete":false}),
            json!({"run_id":"cron-run-1"}),
            Value::Null,
        ] {
            let raw = json!({"status":"completed", "output_tokens":7, "graph":graph}).to_string();
            let error = summarize(&raw, Some("cron-run-1"), None).err().expect("unknown cost must stop");
            assert!(error.contains("complete child usage"), "{error}");
        }
    }

    #[test]
    fn a_deadline_keeps_late_completion_truthful_and_charged() {
        let (result, text) = summarize(&report("completed", 11), Some("cron-run-1"), Some(StopReason::Deadline)).unwrap();
        let summary = result.summary;
        assert_eq!(summary.budget_exhausted, Some(BudgetExhausted::Deadline));
        assert_eq!(summary.turn_output_tokens, 11);
        assert!(text.contains("deadline reached; graph reported completed"));
        assert!(!summary.verification_issues.is_empty());
    }

    #[test]
    fn display_labels_and_failure_notes_are_bounded() {
        let raw = json!({"status":"blocked","graph":{"accounting_complete":true},"notes":["x".repeat(1000), "y".repeat(1000), "ignored"]}).to_string();
        let (_, text) = summarize(&raw, Some(&format!("{}\u{1b}", "a".repeat(1000))), None).unwrap();
        assert!(!text.contains('\u{1b}'));
        assert!(!text.contains("ignored"));
        assert!(text.chars().count() < MAX_LABEL_CHARS + 2 * MAX_NOTE_CHARS + 100);
    }

    #[tokio::test]
    async fn scheduled_work_executes_once_and_emits_a_final_notice() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let (sender, mut receiver) = tokio::sync::mpsc::channel(8);
        let result = run_with(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(report("completed", 5))
        }, || {}, Some("cron-run-1"), sender, HookAbortSignal::new(), Arc::new(AtomicBool::new(false)), Duration::from_secs(1)).await.unwrap();
        let summary = result.summary;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(summary.turn_output_tokens, 5);
        assert!(matches!(receiver.recv().await, Some(RenderBlock::System { .. })));
        assert!(matches!(receiver.recv().await, Some(RenderBlock::TextDelta { done:true, .. })));
    }

    #[tokio::test]
    async fn an_already_cancelled_job_never_starts() {
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = Arc::clone(&calls);
        let (sender, _receiver) = tokio::sync::mpsc::channel(8);
        let result = run_with(move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(report("completed", 0))
        }, || {}, Some("cron-run-1"), sender, HookAbortSignal::new(), Arc::new(AtomicBool::new(true)), Duration::from_secs(1)).await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_user_stop_waits_for_the_worker_and_preserves_its_spend() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let (sender, _receiver) = tokio::sync::mpsc::channel(8);
        let cancelled = Arc::new(AtomicBool::new(false));
        let turn_cancelled = Arc::clone(&cancelled);
        let task = tokio::spawn(async move {
            run_with(move || {
                let _ = started_tx.send(());
                stop_rx.recv_timeout(Duration::from_secs(10)).expect("user stop reaches worker");
                Ok(report("cancelled", 17))
            }, move || { let _ = stop_tx.send(()); }, Some("cron-run-1"), sender,
                HookAbortSignal::new(), turn_cancelled, Duration::from_secs(30)).await
        });
        started_rx.await.unwrap();
        cancelled.store(true, Ordering::SeqCst);
        let result = task.await.unwrap().unwrap();
        assert_eq!(result.status, "cancelled");
        assert_eq!(result.summary.turn_output_tokens, 17);
        assert!(notice(&result.summary).contains("stop requested; graph reported cancelled"));
    }

    #[tokio::test]
    async fn cancellation_is_repeated_until_the_blocking_worker_has_exited() {
        let cancellation_calls = Arc::new(AtomicUsize::new(0));
        let cancel_calls = Arc::clone(&cancellation_calls);
        let (stop_tx, stop_rx) = std::sync::mpsc::channel();
        let exited = Arc::new(AtomicBool::new(false));
        let worker_exited = Arc::clone(&exited);
        let (sender, _receiver) = tokio::sync::mpsc::channel(8);
        let result = run_with(move || {
            stop_rx.recv_timeout(Duration::from_secs(10)).expect("first stop");
            stop_rx.recv_timeout(Duration::from_secs(10)).expect("stop repeated until exit");
            worker_exited.store(true, Ordering::SeqCst);
            Ok(report("cancelled", 13))
        }, move || { cancel_calls.fetch_add(1, Ordering::SeqCst); let _ = stop_tx.send(()); }, Some("cron-run-1"), sender, HookAbortSignal::new(), Arc::new(AtomicBool::new(false)), Duration::from_millis(1)).await.unwrap();
        let summary = result.summary;
        assert!(exited.load(Ordering::SeqCst), "returning must not detach the old worker");
        assert!(cancellation_calls.load(Ordering::SeqCst) >= 2);
        assert_eq!(summary.turn_output_tokens, 13);
        assert!(notice(&summary).contains("deadline reached; graph reported cancelled"));
    }
}
