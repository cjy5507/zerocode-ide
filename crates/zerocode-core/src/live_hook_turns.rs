//! Provisional turns built from lifecycle hooks while a CLI has not yet
//! flushed its session document (`vault::Format::settles_after_turn`).
//!
//! A CLI that writes its whole session file only after each turn ends (Devin's
//! ATIF document, ~30 ms after `Stop`) leaves the conversation view silent for
//! the whole turn. Meanwhile, its hooks (`UserPromptSubmit`, `PreToolUse`,
//! `PostToolUse`, `PostToolUseFailure`, `Stop`) arrive in real time carrying
//! the user's prompt, each tool call and its result, and the final answer.
//!
//! This module folds those hook payloads into the same [`TranscriptTurn`] shape
//! the transcript readers produce, reusing [`crate::transcript::tool_call_turn`]
//! and [`crate::transcript::tool_result_turn`] so a tool's kind, file and edits
//! are read once. Once the authoritative session document advances past the
//! turn's starting step count, [`LiveTurnBuffer::reconcile_document`] drops the
//! provisional turns so the document always wins with zero duplicate rows.
//!
//! Nothing here branches on an agent name or touches the filesystem.

use crate::hook::{normalized_event, worker_call_id_in};
use crate::payload::HookPayload;
use crate::transcript::{
    Detail, TranscriptTurn, message_text, prompt_in_parsed, tool_call_turn, tool_result_turn,
};

/// Provisional turns accumulated for one pane's in-progress (or just-stopped,
/// not-yet-written) turn.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveTurnBuffer {
    session_id: Option<String>,
    /// Authoritative document step count when the current provisional turn
    /// began. Document steps at indices `< base_steps` belong to earlier turns.
    base_steps: usize,
    /// Document step count observed when `Stop` arrived for this turn.
    stop_steps: Option<usize>,
    /// Trimmed prompt text of the active provisional turn, used to tell whether
    /// an intervening document write belongs to the previous turn or this one.
    prompt_text: Option<String>,
    /// Provisional turns in display order.
    turns: Vec<TranscriptTurn>,
    /// Call IDs already recorded as `tool` turns in this provisional turn.
    seen_calls: Vec<String>,
    /// Call IDs already recorded as `tool_result` turns in this provisional turn.
    seen_results: Vec<String>,
    /// Whether `Stop` has been observed for the active provisional turn.
    stopped: bool,
}

impl LiveTurnBuffer {
    /// Provisional turns currently held for display behind settled document
    /// turns.
    #[must_use]
    pub fn turns(&self) -> &[TranscriptTurn] {
        &self.turns
    }

    /// Whether no provisional turns are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Drops all provisional turns while preserving nothing from the previous
    /// turn.
    pub fn clear(&mut self) {
        self.base_steps = 0;
        self.stop_steps = None;
        self.prompt_text = None;
        self.turns.clear();
        self.seen_calls.clear();
        self.seen_results.clear();
        self.stopped = false;
    }

    /// Records one hook event into the provisional turn buffer. Returns `true`
    /// when the buffered turns changed.
    ///
    /// Handles out-of-order and duplicate hook deliveries:
    /// - duplicate `PreToolUse` / `PostToolUse` for the same `call_id` are
    ///   ignored;
    /// - `PostToolUse` arriving before `PreToolUse` synthesizes the call+result
    ///   pair once and ignores the late `PreToolUse`;
    /// - `PostToolUse` arriving after `Stop` attaches the result immediately
    ///   after its matching `tool` call (before the trailing `assistant` turn).
    pub fn note_hook(
        &mut self,
        event_name: &str,
        payload: &HookPayload<'_>,
        session_id: Option<&str>,
        current_settled: usize,
        at_ms: Option<i64>,
    ) -> bool {
        if let Some(next_session) = session_id
            && self.session_id.as_deref() != Some(next_session)
        {
            self.clear();
            self.session_id = Some(next_session.to_string());
        }
        let word = normalized_event(event_name);
        match word.as_str() {
            "userpromptsubmit" | "beforesubmitprompt" => {
                self.note_prompt(payload, current_settled, at_ms)
            }
            "pretooluse" => self.note_pre_tool(payload, current_settled, at_ms),
            "posttooluse" | "posttoolusefailure" => self.note_post_tool(
                payload,
                word == "posttoolusefailure",
                current_settled,
                at_ms,
            ),
            "stop" => self.note_stop(payload, current_settled, at_ms),
            _ => false,
        }
    }

    /// Reconciles the provisional buffer against the authoritative document's
    /// settled steps. When the document has recorded this turn, the provisional
    /// turns are cleared so the document wins with zero duplicates.
    pub fn reconcile_document(&mut self, settled: usize, steps: &[Vec<TranscriptTurn>]) {
        let slice: Vec<&TranscriptTurn> = if settled > self.base_steps {
            steps[self.base_steps..settled.min(steps.len())]
                .iter()
                .flatten()
                .collect()
        } else {
            Vec::new()
        };
        self.reconcile_turns(settled, &slice);
    }

    /// Reconciles the provisional buffer against the authoritative turns
    /// returned by `document_log` (avoiding a second disk read).
    pub fn reconcile_turns(&mut self, settled: usize, new_turns: &[&TranscriptTurn]) {
        if self.turns.is_empty() {
            return;
        }
        // A document that shrank (compaction/replacement) supersedes older
        // provisional state too.
        if settled < self.base_steps {
            self.clear();
            return;
        }
        if settled == self.base_steps {
            return;
        }
        if let Some(prompt) = self.prompt_text.as_deref() {
            if turns_contain_prompt(new_turns, prompt) {
                self.clear();
                return;
            }
            // If this turn already stopped and the document advanced past where
            // it stood at `Stop`, the CLI wrote this turn (even if it altered
            // the prompt text, e.g. expansion or trimming).
            if self.stopped && self.stop_steps.is_some_and(|at| settled > at) {
                self.clear();
                return;
            }
            // Otherwise the document growth belonged to an earlier turn that
            // finished flushing just as this turn started: advance `base_steps`
            // so only steps after it can absorb the current turn.
            self.base_steps = settled;
            return;
        }
        // No prompt recorded (mid-turn adoption): any new settled steps in the
        // document supersede the provisional calls.
        self.clear();
    }

    fn note_prompt(
        &mut self,
        payload: &HookPayload<'_>,
        current_settled: usize,
        at_ms: Option<i64>,
    ) -> bool {
        let Some(text) = prompt_in_parsed(payload).or_else(|| {
            let tree = payload.tree()?;
            ["prompt", "message", "user_prompt", "userPrompt"]
                .into_iter()
                .find_map(|key| message_text(tree.get(key)))
        }) else {
            return false;
        };
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return false;
        }
        // A fresh prompt always begins a new provisional turn.
        self.clear();
        self.base_steps = current_settled;
        self.prompt_text = Some(trimmed.to_string());
        self.turns.push(TranscriptTurn {
            role: "user".into(),
            text: trimmed.to_string(),
            at_ms,
            tool: None,
            images: Vec::new(),
        });
        true
    }

    fn note_pre_tool(
        &mut self,
        payload: &HookPayload<'_>,
        current_settled: usize,
        at_ms: Option<i64>,
    ) -> bool {
        let Some(tree) = payload.tree() else {
            return false;
        };
        let call_id = worker_call_id_in(tree);
        if let Some(id) = call_id.as_deref()
            && self.seen_calls.iter().any(|seen| seen == id)
        {
            return false;
        }
        let Some(turn) = hook_tool_call(tree, call_id.as_deref(), at_ms) else {
            return false;
        };
        if self.turns.is_empty() {
            self.base_steps = current_settled;
        }
        if let Some(id) = call_id {
            self.seen_calls.push(id);
        }
        let insert_at = self.before_trailing_stop_answer();
        self.turns.insert(insert_at, turn);
        true
    }

    fn note_post_tool(
        &mut self,
        payload: &HookPayload<'_>,
        explicit_failure: bool,
        current_settled: usize,
        at_ms: Option<i64>,
    ) -> bool {
        let Some(tree) = payload.tree() else {
            return false;
        };
        let call_id = worker_call_id_in(tree);
        if let Some(id) = call_id.as_deref()
            && self.seen_results.iter().any(|seen| seen == id)
        {
            return false;
        }
        if self.turns.is_empty() {
            self.base_steps = current_settled;
        }
        // Out-of-order `PostToolUse` before `PreToolUse`: synthesize the call
        // once so the result folds into a real tool row instead of standing
        // orphaned, and mark the call_id seen so a late `PreToolUse` is ignored.
        let matching_call_idx = call_id.as_deref().and_then(|id| {
            self.turns.iter().position(|turn| {
                turn.role == "tool" && turn.tool.as_ref().is_some_and(|tool| tool.call_id == id)
            })
        });
        let insert_at = if let Some(idx) = matching_call_idx {
            idx + 1
        } else {
            let at = self.before_trailing_stop_answer();
            if let Some(call_turn) = hook_tool_call(tree, call_id.as_deref(), at_ms) {
                if let Some(id) = call_id.clone() {
                    self.seen_calls.push(id);
                }
                self.turns.insert(at, call_turn);
                at + 1
            } else {
                at
            }
        };
        let result_turn = hook_tool_result(tree, call_id.as_deref(), explicit_failure, at_ms);
        if let Some(id) = call_id {
            self.seen_results.push(id);
        }
        self.turns.insert(insert_at, result_turn);
        true
    }

    fn note_stop(
        &mut self,
        payload: &HookPayload<'_>,
        current_settled: usize,
        at_ms: Option<i64>,
    ) -> bool {
        if self.turns.is_empty() {
            self.base_steps = current_settled;
        }
        self.stopped = true;
        self.stop_steps = Some(current_settled);
        let answer = payload.tree().and_then(|tree| {
            ["last_assistant_message", "lastAssistantMessage", "message"]
                .into_iter()
                .find_map(|key| message_text(tree.get(key)))
        });
        let Some(text) = answer else {
            return false;
        };
        if let Some(last) = self
            .turns
            .last_mut()
            .filter(|turn| turn.role == "assistant")
        {
            if last.text == text {
                return false;
            }
            last.text = text;
            last.at_ms = at_ms;
            return true;
        }
        self.turns.push(TranscriptTurn {
            role: "assistant".into(),
            text,
            at_ms,
            tool: None,
            images: Vec::new(),
        });
        true
    }

    /// Position immediately before a trailing `assistant` turn left by an
    /// early `Stop`, or the end of `self.turns` otherwise.
    fn before_trailing_stop_answer(&self) -> usize {
        if self.stopped
            && self
                .turns
                .last()
                .is_some_and(|turn| turn.role == "assistant")
        {
            self.turns.len() - 1
        } else {
            self.turns.len()
        }
    }
}

fn hook_tool_name(tree: &serde_json::Value) -> Option<&str> {
    ["tool_name", "toolName", "tool"]
        .into_iter()
        .find_map(|key| tree.get(key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn hook_tool_input(tree: &serde_json::Value) -> Option<&serde_json::Value> {
    ["tool_input", "toolInput", "input", "arguments"]
        .into_iter()
        .find_map(|key| tree.get(key))
}

fn hook_tool_call(
    tree: &serde_json::Value,
    call_id: Option<&str>,
    at_ms: Option<i64>,
) -> Option<TranscriptTurn> {
    let name = hook_tool_name(tree)?;
    let part = serde_json::json!({
        "name": name,
        "arguments": hook_tool_input(tree),
        "call_id": call_id,
    });
    tool_call_turn(&part, at_ms, Detail::Clipped)
}

fn hook_tool_result(
    tree: &serde_json::Value,
    call_id: Option<&str>,
    explicit_failure: bool,
    at_ms: Option<i64>,
) -> TranscriptTurn {
    let response = [
        "tool_response",
        "toolResponse",
        "tool_result",
        "toolResult",
        "output",
        "result",
        "response",
    ]
    .into_iter()
    .find_map(|key| tree.get(key));
    let is_error = explicit_failure
        || response.is_some_and(|val| {
            val.get("success").and_then(serde_json::Value::as_bool) == Some(false)
                || val.get("is_error").and_then(serde_json::Value::as_bool) == Some(true)
                || val
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|err| !err.trim().is_empty())
        });
    let content = response.map(|val| {
        if let Some(obj) = val.as_object() {
            for key in [
                "output", "stdout", "text", "content", "result", "error", "stderr",
            ] {
                if let Some(text) = obj.get(key).and_then(serde_json::Value::as_str)
                    && !text.is_empty()
                {
                    return serde_json::Value::String(text.to_string());
                }
            }
        }
        val.clone()
    });
    let part = serde_json::json!({
        "tool_name": hook_tool_name(tree).unwrap_or_default(),
        "call_id": call_id,
        "content": content,
        "is_error": is_error,
    });
    tool_result_turn(&part, at_ms, Detail::Clipped)
}

/// Whether any turn in `turns` is a `user` turn matching `prompt`.
fn turns_contain_prompt(turns: &[&TranscriptTurn], prompt: &str) -> bool {
    let needle = prompt.trim();
    if needle.is_empty() {
        return false;
    }
    turns.iter().any(|turn| {
        turn.role == "user" && {
            let said = turn.text.trim();
            said == needle || said.starts_with(needle) || needle.starts_with(said)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(
        buf: &mut LiveTurnBuffer,
        event: &str,
        body: &serde_json::Value,
        session: Option<&str>,
        settled: usize,
        at_ms: i64,
    ) -> bool {
        let raw = body.to_string();
        buf.note_hook(event, &HookPayload::of(&raw), session, settled, Some(at_ms))
    }

    /// A full mid-turn lifecycle (`UserPromptSubmit` → `PreToolUse` →
    /// `PostToolUse` → `Stop`) builds user, tool, tool_result and assistant
    /// turns in order, and drops cleanly once the authoritative document lands.
    #[test]
    fn mid_turn_hooks_build_provisional_turns_and_yield_to_the_document() {
        let mut buf = LiveTurnBuffer::default();
        assert!(hook(
            &mut buf,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "list files", "session_id": "s-1"}),
            Some("s-1"),
            3,
            1000
        ));
        assert!(hook(
            &mut buf,
            "PreToolUse",
            &serde_json::json!({"tool_name": "exec", "tool_input": {"command": "ls"}, "tool_use_id": "c-1"}),
            Some("s-1"),
            3,
            2000
        ));
        assert!(hook(
            &mut buf,
            "PostToolUse",
            &serde_json::json!({
                "tool_name": "exec",
                "tool_input": {"command": "ls"},
                "tool_use_id": "c-1",
                "tool_response": {"success": true, "output": "a.rs\nb.rs", "error": null}
            }),
            Some("s-1"),
            3,
            2100
        ));
        assert!(hook(
            &mut buf,
            "Stop",
            &serde_json::json!({"last_assistant_message": "Found a.rs and b.rs."}),
            Some("s-1"),
            3,
            3000
        ));
        let roles: Vec<&str> = buf.turns().iter().map(|t| t.role.as_str()).collect();
        assert_eq!(roles, ["user", "tool", "tool_result", "assistant"]);
        assert_eq!(buf.turns()[0].text, "list files");
        assert_eq!(buf.turns()[1].tool.as_ref().unwrap().call_id, "c-1");
        assert_eq!(buf.turns()[1].tool.as_ref().unwrap().kind, "bash");
        assert_eq!(buf.turns()[2].text, "a.rs\nb.rs");
        assert!(!buf.turns()[2].tool.as_ref().unwrap().is_error);
        assert_eq!(buf.turns()[3].text, "Found a.rs and b.rs.");

        // Unchanged document keeps provisional turns visible.
        let unchanged: Vec<Vec<TranscriptTurn>> = vec![vec![], vec![], vec![]];
        buf.reconcile_document(3, &unchanged);
        assert_eq!(buf.turns().len(), 4);

        // Once the authoritative document holds the new turn, provisional turns
        // clear completely so nothing appears twice.
        let mut with_turn = unchanged;
        with_turn.push(vec![TranscriptTurn {
            role: "user".into(),
            text: "list files".into(),
            at_ms: Some(1000),
            tool: None,
            images: Vec::new(),
        }]);
        with_turn.push(vec![TranscriptTurn {
            role: "assistant".into(),
            text: "Found a.rs and b.rs.".into(),
            at_ms: Some(3000),
            tool: None,
            images: Vec::new(),
        }]);
        buf.reconcile_document(5, &with_turn);
        assert!(buf.is_empty(), "document must supersede provisional turns");
    }

    /// Out-of-order and duplicate hooks (`Stop` arriving before a late
    /// `PostToolUse`, `PostToolUse` arriving before `PreToolUse`, duplicate
    /// deliveries) never duplicate a step or misplace a result after the final
    /// answer.
    #[test]
    fn out_of_order_and_duplicate_hooks_produce_zero_duplicates() {
        let mut buf = LiveTurnBuffer::default();
        hook(
            &mut buf,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "check status"}),
            Some("s-2"),
            0,
            1000,
        );
        // `PreToolUse` followed by duplicate `PreToolUse`.
        assert!(hook(
            &mut buf,
            "PreToolUse",
            &serde_json::json!({"tool_name": "exec", "tool_input": {"command": "git status"}, "tool_use_id": "c-1"}),
            Some("s-2"),
            0,
            1100,
        ));
        assert!(!hook(
            &mut buf,
            "PreToolUse",
            &serde_json::json!({"tool_name": "exec", "tool_input": {"command": "git status"}, "tool_use_id": "c-1"}),
            Some("s-2"),
            0,
            1105,
        ));
        // `Stop` arrives BEFORE the late `PostToolUse` for `c-1`.
        assert!(hook(
            &mut buf,
            "Stop",
            &serde_json::json!({"last_assistant_message": "Working tree clean."}),
            Some("s-2"),
            0,
            1200,
        ));
        assert!(hook(
            &mut buf,
            "PostToolUse",
            &serde_json::json!({
                "tool_name": "exec",
                "tool_input": {"command": "git status"},
                "tool_use_id": "c-1",
                "tool_response": {"success": false, "output": "", "error": "fatal: not a repo"}
            }),
            Some("s-2"),
            0,
            1250,
        ));
        // Duplicate `PostToolUse` is ignored.
        assert!(!hook(
            &mut buf,
            "PostToolUse",
            &serde_json::json!({
                "tool_name": "exec",
                "tool_input": {"command": "git status"},
                "tool_use_id": "c-1",
                "tool_response": {"success": false, "output": "", "error": "fatal: not a repo"}
            }),
            Some("s-2"),
            0,
            1260,
        ));
        let roles: Vec<&str> = buf.turns().iter().map(|t| t.role.as_str()).collect();
        assert_eq!(roles, ["user", "tool", "tool_result", "assistant"]);
        assert!(buf.turns()[2].tool.as_ref().unwrap().is_error);
        assert_eq!(buf.turns()[2].text, "fatal: not a repo");

        // Second scenario: `PostToolUse` arrives before `PreToolUse`.
        let mut buf2 = LiveTurnBuffer::default();
        assert!(hook(
            &mut buf2,
            "PostToolUse",
            &serde_json::json!({
                "tool_name": "read",
                "tool_input": {"file_path": "/repo/a.rs"},
                "tool_use_id": "c-9",
                "tool_response": {"success": true, "output": "fn main() {}", "error": null}
            }),
            Some("s-3"),
            0,
            2000,
        ));
        assert!(!hook(
            &mut buf2,
            "PreToolUse",
            &serde_json::json!({
                "tool_name": "read",
                "tool_input": {"file_path": "/repo/a.rs"},
                "tool_use_id": "c-9"
            }),
            Some("s-3"),
            0,
            2010,
        ));
        let roles2: Vec<&str> = buf2.turns().iter().map(|t| t.role.as_str()).collect();
        assert_eq!(roles2, ["tool", "tool_result"]);
    }

    /// A queued second turn starting before the first turn finished flushing to
    /// disk keeps its own provisional turns when the first turn's document
    /// lands, and clears only once the second turn itself lands.
    #[test]
    fn late_previous_turn_document_advances_base_without_dropping_active_turn() {
        let mut buf = LiveTurnBuffer::default();
        hook(
            &mut buf,
            "UserPromptSubmit",
            &serde_json::json!({"prompt": "second question"}),
            Some("s-1"),
            0,
            5000,
        );
        // Turn 1's document lands with a different user prompt ("first question").
        let turn1_doc = vec![
            vec![],
            vec![TranscriptTurn {
                role: "user".into(),
                text: "first question".into(),
                at_ms: Some(1000),
                tool: None,
                images: Vec::new(),
            }],
            vec![TranscriptTurn {
                role: "assistant".into(),
                text: "first answer".into(),
                at_ms: Some(2000),
                tool: None,
                images: Vec::new(),
            }],
        ];
        buf.reconcile_document(3, &turn1_doc);
        assert_eq!(
            buf.turns().len(),
            1,
            "active turn 2 must stay visible when turn 1 finishes flushing"
        );
        assert_eq!(buf.base_steps, 3);

        // Turn 2's document lands.
        let mut turn2_doc = turn1_doc;
        turn2_doc.push(vec![TranscriptTurn {
            role: "user".into(),
            text: "second question".into(),
            at_ms: Some(5000),
            tool: None,
            images: Vec::new(),
        }]);
        turn2_doc.push(vec![TranscriptTurn {
            role: "assistant".into(),
            text: "second answer".into(),
            at_ms: Some(6000),
            tool: None,
            images: Vec::new(),
        }]);
        buf.reconcile_document(5, &turn2_doc);
        assert!(buf.is_empty());
    }
}
