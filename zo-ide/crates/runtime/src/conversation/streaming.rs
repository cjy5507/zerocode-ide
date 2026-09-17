//! Streaming-turn helpers — permission prompter adapter and tool summary
//! formatters used by [`ConversationRuntime::run_turn_streaming`].
//!
//! Two responsibilities live here:
//!
//! 1. The [`CapturePrompter`] adapter that converts the synchronous
//!    permission-prompter trait into something the async streaming path
//!    can `.await` against (per `.zo/code-rules.md` R3 — no `block_on`
//!    inside async context).
//! 2. The pure functions that build [`ToolPreview`] / single-line tool
//!    summary strings from the tool name + raw JSON input. These never
//!    touch ratatui types — they live in the runtime layer so both the
//!    streaming path and any non-TUI consumer get the same canonical
//!    summary.

use std::cell::RefCell;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::message_stream::anthropic::tools::preview_tool_input;
use crate::message_stream::types::ToolPreview;
use crate::permission::{
    PermissionChoice as AsyncPermissionChoice, PermissionDecision as AsyncPermissionDecision,
    PermissionRequest as AsyncPermissionRequest, RiskLevel as AsyncRiskLevel,
};
use crate::permissions::{
    PermissionPromptDecision, PermissionPrompter, PermissionRequest as SyncPermissionRequest,
};

/// Sync `PermissionPrompter` adapter used internally by the streaming
/// path. Records the decision-request that the policy engine would
/// have asked a human about and answers with a tentative fixed reply
/// so the synchronous `authorize_with_context` call can complete
/// without blocking. After the call returns, the streaming loop
/// inspects the captured request and, if present, awaits the real
/// async prompter — this is how we satisfy R3 (no `block_on` inside
/// async context).
pub(crate) struct CapturePrompter {
    captured: RefCell<Option<SyncPermissionRequest>>,
    fake_answer: PermissionPromptDecision,
}

impl CapturePrompter {
    pub(crate) fn new(fake_answer: PermissionPromptDecision) -> Self {
        Self {
            captured: RefCell::new(None),
            fake_answer,
        }
    }

    pub(crate) fn take(&self) -> Option<SyncPermissionRequest> {
        self.captured.borrow_mut().take()
    }
}

impl PermissionPrompter for CapturePrompter {
    fn decide(&mut self, request: &SyncPermissionRequest) -> PermissionPromptDecision {
        *self.captured.borrow_mut() = Some(request.clone());
        self.fake_answer.clone()
    }
}

pub(crate) fn default_permission_choices() -> Vec<AsyncPermissionChoice> {
    vec![
        AsyncPermissionChoice {
            key: 'y',
            label: "Allow".to_string(),
            decision: AsyncPermissionDecision::Allow,
        },
        AsyncPermissionChoice {
            key: 'o',
            label: "Allow once".to_string(),
            decision: AsyncPermissionDecision::AllowOnce,
        },
        AsyncPermissionChoice {
            key: 'n',
            label: "Deny".to_string(),
            decision: AsyncPermissionDecision::Deny,
        },
    ]
}

pub(crate) fn risk_from_tool_name(tool_name: &str) -> AsyncRiskLevel {
    let lower = tool_name.to_ascii_lowercase();
    if lower.contains("write")
        || lower.contains("edit")
        || lower.contains("bash")
        || lower.contains("shell")
        || lower.contains("exec")
    {
        AsyncRiskLevel::High
    } else if lower.contains("read") || lower.contains("grep") || lower.contains("glob") {
        AsyncRiskLevel::Low
    } else {
        AsyncRiskLevel::Medium
    }
}

/// Build the typed [`ToolPreview`] for a tool dispatch.
///
/// Parses the raw JSON input and routes it through the canonical
/// [`preview_tool_input`] builder so the dispatch path emits the same
/// typed previews (`Bash`/`Read`/`Grep`/…) as the streaming parser —
/// the TUI then renders `Ran rg -n …` / `Explored read keys.rs` instead
/// of leaking a truncated raw-JSON `Generic` summary. Unparseable input
/// falls back to the legacy `Generic` + truncated-text shape.
pub(crate) fn tool_preview_from(tool_name: &str, input: &str) -> ToolPreview {
    if let Ok(parsed) = serde_json::from_str::<Value>(input) {
        return preview_tool_input(tool_name, &parsed);
    }
    let summary = if input.chars().count() > 120 {
        format!("{}…", truncate_chars(input, 120))
    } else {
        input.to_string()
    };
    ToolPreview::Generic {
        name: tool_name.to_string(),
        input_summary: summary,
    }
}

pub(crate) fn tool_summary_line(tool_name: &str, input: &str) -> String {
    if let Some(summary) = extract_tool_summary(tool_name, input) {
        return summary;
    }
    let trimmed = input.trim();
    // `{}` is not an argument. Rendering it verbatim gave every argument-less
    // tool a `Name({})` row (and a bare `{}` wherever only the summary is
    // shown); the tool's own name is the whole story for such a call.
    let argumentless = serde_json::from_str::<Value>(trimmed)
        .is_ok_and(|value| crate::message_stream::anthropic::tools::json_is_argumentless(&value));
    if trimmed.is_empty() || argumentless {
        tool_name.to_string()
    } else if trimmed.chars().count() > 80 {
        // Never ship a mid-JSON cut: the TUI re-parses this summary
        // (`extract_call_json`) and a truncated `{"command": "rg -n \"…`
        // fails that parse, leaking raw JSON into the transcript. An
        // empty summary lets the typed preview drive the row instead.
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            String::new()
        } else {
            format!("{tool_name}({}…)", truncate_chars(trimmed, 80))
        }
    } else {
        format!("{tool_name}({trimmed})")
    }
}

/// Build the permission UI's deliberately narrow input summary. Known tools
/// expose only their canonical command/path/query preview; unknown payloads are
/// withheld rather than leaking short JSON values verbatim.
fn permission_input_summary(tool_name: &str, input: &str) -> String {
    extract_tool_summary(tool_name, input).unwrap_or_else(|| "Input details hidden".to_string())
}

fn extract_tool_summary(tool_name: &str, input: &str) -> Option<String> {
    let v: Value = serde_json::from_str(input).ok()?;
    let summary = match tool_name {
        "Read" | "read_file" | "read" | "Edit" | "edit_file" | "edit" | "Write" | "write_file"
        | "write" => {
            let path = v
                .get("file_path")
                .or_else(|| v.get("path"))
                .and_then(Value::as_str)?;
            path.rsplit('/').next().unwrap_or(path).to_string()
        }
        "Bash" | "bash" => {
            let cmd = v.get("command").and_then(Value::as_str)?;
            let short = truncate_chars(cmd.trim(), 60);
            if cmd.trim().chars().count() > 60 {
                format!("{short}…")
            } else {
                short.to_string()
            }
        }
        "Grep" | "grep_search" | "grep" => {
            let pattern = v.get("pattern").and_then(Value::as_str)?;
            let path = v
                .get("path")
                .and_then(Value::as_str)
                .map(|p| p.rsplit('/').next().unwrap_or(p));
            match path {
                Some(p) => format!("/{pattern}/ in {p}"),
                None => format!("/{pattern}/"),
            }
        }
        "Glob" | "glob_search" | "glob" => v.get("pattern").and_then(Value::as_str)?.to_string(),
        "Agent" => v.get("description").and_then(Value::as_str)?.to_string(),
        "TaskCreate" => {
            let desc = v.get("description").and_then(Value::as_str)?;
            let short = truncate_chars(desc, 50);
            short.to_string()
        }
        "TaskUpdate" => {
            let id = v.get("id").and_then(Value::as_str).unwrap_or("?");
            let status = v.get("status").and_then(Value::as_str).unwrap_or("?");
            format!("#{id} → {status}")
        }
        "WebFetch" => {
            let url = v.get("url").and_then(Value::as_str)?;
            let short = truncate_chars(url, 50);
            short.to_string()
        }
        "WebSearch" => {
            let query = v.get("query").and_then(Value::as_str)?;
            format!("\"{query}\"")
        }
        "AskUserQuestion" => {
            let q = v.get("question").and_then(Value::as_str)?;
            let short = truncate_chars(q, 50);
            if q.chars().count() > 50 {
                format!("\"{short}…\"")
            } else {
                format!("\"{short}\"")
            }
        }
        "SpawnMultiAgent" => {
            let agents = v.get("agents").and_then(Value::as_array)?;
            format!("{} agents", agents.len())
        }
        _ => return None,
    };
    Some(summary)
}

fn truncate_chars(input: &str, max_chars: usize) -> &str {
    if input.chars().count() <= max_chars {
        return input;
    }
    let end = input
        .char_indices()
        .nth(max_chars)
        .map_or(input.len(), |(idx, _)| idx);
    &input[..end]
}

pub(crate) fn build_async_permission_request(
    sync_request: &SyncPermissionRequest,
) -> AsyncPermissionRequest {
    AsyncPermissionRequest {
        tool: sync_request.tool_name.clone(),
        input_summary: permission_input_summary(&sync_request.tool_name, &sync_request.input),
        input_hash: format!("{:x}", Sha256::digest(sync_request.input.as_bytes())),
        reasoning: sync_request.reason.clone().unwrap_or_else(|| {
            format!(
                "tool '{}' requires approval to run while mode is {}",
                sync_request.tool_name,
                sync_request.current_mode.as_str()
            )
        }),
        choices: default_permission_choices(),
        risk_level: risk_from_tool_name(&sync_request.tool_name),
    }
}

#[cfg(test)]
mod permission_metadata_tests {
    use super::build_async_permission_request;
    use crate::permissions::{PermissionMode, PermissionRequest};

    #[test]
    fn permission_metadata_exposes_summary_and_hash_not_full_unknown_payload() {
        let full_input = r#"{"token":"do-not-publish","nested":{"value":42}}"#;
        let request = build_async_permission_request(&PermissionRequest {
            tool_name: "CustomNetworkTool".to_string(),
            input: full_input.to_string(),
            current_mode: PermissionMode::Prompt,
            required_mode: PermissionMode::Allow,
            reason: None,
            deny_reason: None,
        });
        assert_eq!(request.input_summary, "Input details hidden");
        assert_eq!(request.input_hash.len(), 64);
        assert!(!request.input_hash.contains("do-not-publish"));

        let bash = build_async_permission_request(&PermissionRequest {
            tool_name: "Bash".to_string(),
            input: r#"{"command":"cargo test -p runtime","description":"ignored"}"#.to_string(),
            current_mode: PermissionMode::Prompt,
            required_mode: PermissionMode::Allow,
            reason: None,
            deny_reason: None,
        });
        assert_eq!(bash.input_summary, "cargo test -p runtime");
    }
}
