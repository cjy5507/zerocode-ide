//! Common receipts produced from one unattended runtime turn.

use std::path::Path;

use core_types::session::ContentBlock;

use crate::goal::{observe_gate, workspace_fingerprint, GateObservation, GoalTurnReport};
use super::driver::TurnOutcome;
use super::wakeup::{self, LoopScheduleRequest};

pub struct AutonomyRunner;

#[must_use]
pub fn elapsed_millis(started: std::time::Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockedReason {
    Permission,
    Question,
    Interrupted,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnReceipt {
    pub workspace_before: String,
    pub workspace_after: String,
    pub gate: Option<GateObservation>,
    pub output_tokens: u64,
    pub active_millis: u64,
    pub blocked: Option<BlockedReason>,
    pub todo_completed_delta: u32,
    pub loop_schedule: Option<LoopScheduleRequest>,
}

impl TurnReceipt {
    #[must_use]
    pub fn capture_workspace(cwd: &Path) -> String {
        workspace_fingerprint(cwd)
            .unwrap_or_else(|failure| format!("fingerprint-unavailable:{failure}"))
    }

    #[must_use]
    pub fn from_turn_outcome(
        cwd: &Path,
        workspace_before: String,
        expected_gate: Option<&str>,
        active_millis: u64,
        outcome: TurnOutcome,
        attempt: &str,
    ) -> Self {
        let workspace_after = tokio::task::block_in_place(|| Self::capture_workspace(cwd));
        let summary = outcome.summary.as_ref();
        let gate = expected_gate.and_then(|expected| {
            summary.map(|summary| observe_gate(summary, expected))
        });
        // A gate that ran settled this attempt on evidence, not on an opinion.
        if let (Some(summary), Some(observation)) = (summary, gate.as_ref()) {
            crate::goal::record_gate_verdict(cwd, summary, attempt, observation);
        }
        let blocked = if outcome.permission_blocked {
            Some(BlockedReason::Permission)
        } else if outcome.question_blocked {
            Some(BlockedReason::Question)
        } else if outcome.cancelled {
            Some(BlockedReason::Interrupted)
        } else {
            outcome.error.map(BlockedReason::Failed)
        };
        Self {
            workspace_before,
            workspace_after,
            gate,
            output_tokens: summary.map_or(0, |summary| u64::from(summary.turn_output_tokens)),
            active_millis,
            blocked,
            todo_completed_delta: summary.map_or(0, completed_todos),
            loop_schedule: wakeup::take_request(),
        }
    }

    #[must_use]
    pub fn into_goal_report(self) -> GoalTurnReport {
        let (permission_blocked, question_blocked, cancelled, error) = match self.blocked {
            Some(BlockedReason::Permission) => (true, false, false, None),
            Some(BlockedReason::Question) => (false, true, false, None),
            Some(BlockedReason::Interrupted) => (false, false, true, None),
            Some(BlockedReason::Failed(error)) => (false, false, false, Some(error)),
            None => (false, false, false, None),
        };
        GoalTurnReport {
            output_tokens: self.output_tokens,
            active_millis: self.active_millis,
            workspace_before: self.workspace_before,
            workspace_fingerprint: self.workspace_after,
            gate: self.gate,
            permission_blocked,
            question_blocked,
            cancelled,
            error,
            todo_completed_delta: self.todo_completed_delta,
        }
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let workspace = if self.workspace_before == self.workspace_after {
            "workspace unchanged"
        } else {
            "workspace changed"
        };
        let blocked = self.blocked.as_ref().map_or_else(
            || "completed".to_string(),
            |reason| match reason {
                BlockedReason::Permission => "blocked on approval".to_string(),
                BlockedReason::Question => "blocked on question".to_string(),
                BlockedReason::Interrupted => "interrupted".to_string(),
                BlockedReason::Failed(error) => format!("failed: {error}"),
            },
        );
        format!(
            "{workspace} · output {} · active {}ms · todo +{} · {blocked}",
            self.output_tokens, self.active_millis, self.todo_completed_delta
        )
    }
}

fn completed_todos(summary: &runtime::TurnSummary) -> u32 {
    summary
        .assistant_messages
        .iter()
        .flat_map(|message| &message.blocks)
        .filter_map(|block| {
            let ContentBlock::ToolUse { name, input, .. } = block else {
                return None;
            };
            name.eq_ignore_ascii_case("TodoWrite")
                .then(|| serde_json::from_str::<serde_json::Value>(input).ok())
                .flatten()
        })
        .filter_map(|input| input.get("todos").and_then(serde_json::Value::as_array).cloned())
        .flatten()
        .filter(|todo| {
            todo.get("status").and_then(serde_json::Value::as_str) == Some("completed")
        })
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

#[must_use]
pub fn provider_reset_at(error: &str, now_unix_ms: u64) -> Option<u64> {
    let lower = error.to_ascii_lowercase();
    if !(lower.contains("429") || lower.contains("rate limit")) {
        return None;
    }
    let retry_after = ["retry-after:", "retry_after:"]
        .iter()
        .find_map(|marker| parse_number_after(&lower, marker));
    retry_after.map(|seconds| now_unix_ms.saturating_add(seconds.saturating_mul(1_000)))
}

#[must_use]
pub fn is_transient_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "network",
        "timeout",
        "timed out",
        "connection reset",
        "connection closed",
        "temporarily unavailable",
        "503",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn parse_number_after(text: &str, marker: &str) -> Option<u64> {
    let tail = text.split_once(marker)?.1.trim_start();
    let digits = tail.chars().take_while(char::is_ascii_digit).collect::<String>();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::provider_reset_at;

    #[test]
    fn provider_retry_after_becomes_an_absolute_wakeup() {
        assert_eq!(
            provider_reset_at("429 rate limit; retry-after: 90", 1_000),
            Some(91_000)
        );
    }

    #[test]
    fn ordinary_failures_do_not_claim_a_provider_reset() {
        assert_eq!(provider_reset_at("network disconnected", 1_000), None);
    }
}
