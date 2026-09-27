//! Completion claims beside the tool results of the same turn. The existing
//! r43 gate owns the turn-end seam; this reader only prepares a bounded,
//! provenance-preserving question for the Jev seat.
//!
//! What a claim is, what code settles before a question and what the one
//! request carries live in the core since t-11349 (`zerocode_core::jev::claim`),
//! where the window reads its panes' turns by the same rules; this file reads
//! zo's own turn into them.

use std::collections::HashMap;

use serde_json::Value;
#[cfg(test)]
use serde_json::json;
use zerocode_core::jev::claim::Evidence;
pub use zerocode_core::jev::claim::{
    instructions, rubric_words, state, ClaimCandidate, CodeVerdict, CLAIM_RUBRIC_VERSION,
};
#[cfg(test)]
use zerocode_core::jev::claim::{claim_id, CLAIM_KEYS, CLAIM_STATE_KEYS};
#[cfg(test)]
use zerocode_core::jev::CLAIM_EVIDENCE_BYTE_CAP;

use super::deep_gate::bash_result_exited_nonzero;
use super::verified_state::tool_verified_state_events;
use super::{ContentBlock, ConversationMessage};
use crate::verified_state::VerifiedStateEvent;

/// Candidates in the answer's final paragraph, over this turn's tool results
/// ([`zerocode_core::jev::claim::scan`]): each result with the bash command
/// that made it, and whether its exit was non-zero as zo's bash tool records
/// it.
#[must_use]
pub fn scan(turn: &[ConversationMessage], final_text: &str) -> Vec<ClaimCandidate> {
    let mut commands: HashMap<&str, String> = HashMap::new();
    let mut evidence = Vec::new();
    for message in turn {
        for block in &message.blocks {
            if let ContentBlock::ToolUse { id, name, input } = block {
                if name == "bash" {
                    if let Some(command) = serde_json::from_str::<Value>(input).ok()
                        .and_then(|value| value.get("command").and_then(Value::as_str).map(str::to_owned)) {
                        commands.insert(id, command);
                    }
                }
            }
        }
        for block in &message.blocks {
            if let ContentBlock::ToolResult { tool_use_id, tool_name, output, is_error, .. } = block {
                let command = commands.get(tool_use_id.as_str()).cloned();
                let nonzero = tool_name == "bash" && bash_result_exited_nonzero(output);
                evidence.push(Evidence {
                    command,
                    output,
                    is_error: *is_error,
                    nonzero,
                });
            }
        }
    }
    zerocode_core::jev::claim::scan(&evidence, final_text)
}

/// r43's existing completion screen over this turn's exact observed edits
/// and green checks, for a replay to count its false asks on the same turns.
#[must_use]
pub fn r43_would_reprompt(turn: &[ConversationMessage], final_text: &str) -> bool {
    let mut inputs: HashMap<&str, &str> = HashMap::new();
    let mut unverified = Vec::new();
    for message in turn {
        for block in &message.blocks {
            if let ContentBlock::ToolUse { id, input, .. } = block {
                inputs.insert(id, input);
            }
        }
        let Some(called) = message.blocks.iter().find_map(|block| match block {
            ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
            _ => None,
        }) else { continue; };
        for event in tool_verified_state_events(message, inputs.get(called).copied().unwrap_or_default()) {
            match event {
                VerifiedStateEvent::Edit(path) => unverified.push(path),
                VerifiedStateEvent::GreenCheck(_) => unverified.clear(),
            }
        }
    }
    let edits = unverified.iter().map(String::as_str).collect::<Vec<_>>();
    super::turn_end_gate::reports_unverified_completion(final_text, &edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(command: &str, output: &str) -> Vec<ConversationMessage> {
        vec![
            ConversationMessage::assistant(vec![ContentBlock::ToolUse {
                id: "id".to_string(), name: "bash".to_string(),
                input: json!({"command": command}).to_string(),
            }]),
            ConversationMessage::tool_result("id", "bash", output, false),
        ]
    }

    /// The claim seat's words are pinned to its version (t-9469): the question
    /// a claim is asked under, its three options and what each means, and the
    /// keys the state carries. A word changed without a version is red here,
    /// and the question names every key the state carries.
    #[test]
    fn the_version_is_pinned_to_the_words() {
        assert_eq!(CLAIM_RUBRIC_VERSION, 1);
        assert_eq!(zerocode_core::jev::rubric_fingerprint(rubric_words), "864bb8ebe6e00fe8");
        let asked = instructions(&claim_id(0));
        for key in CLAIM_STATE_KEYS {
            assert!(asked.contains(&format!("`{key}")), "{key}: {asked}");
        }
        let claims = scan(&tool("cargo test", r#"{"stdout":"test result: ok"}"#), "`cargo test` passed.");
        let sent = state(&claims);
        let mut keys: Vec<&str> = sent.as_object().expect("an object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, CLAIM_STATE_KEYS);
        let mut claim: Vec<&str> = sent["claims"][0].as_object().expect("a claim").keys().map(String::as_str).collect();
        claim.sort_unstable();
        assert_eq!(claim, CLAIM_KEYS);
    }

    #[test]
    fn a_claim_naming_a_command_that_never_ran_is_unsupported_without_a_request() {
        let claims = scan(&tool("cargo check", r#"{"stdout":"ok"}"#), "`cargo test` passed.");
        assert_eq!(claims[0].code, CodeVerdict::Unsupported);
    }

    #[test]
    fn a_pass_claim_over_a_nonzero_exit_is_contradicted_by_code_alone() {
        let claims = scan(&tool("cargo test", r#"{"returnCodeInterpretation":"exit_code:1","stdout":"failed"}"#), "`cargo test` passed.");
        assert_eq!(claims[0].code, CodeVerdict::Contradicted);
    }

    #[test]
    fn a_pass_claim_over_unrelated_output_is_not_a_failed_exit() {
        let turn = [ConversationMessage::tool_result("id", "read_file", "The check was described here.", false)];
        let claims = scan(&turn, "The tests passed.");
        assert_eq!(claims[0].code, CodeVerdict::NeedsReading);
        let background = tool("cargo test", r#"{"stdout":"started","backgroundTaskId":"task-1","returnCodeInterpretation":"exit_code:1"}"#);
        assert_eq!(scan(&background, "`cargo test` passed.")[0].code, CodeVerdict::NeedsReading);
    }

    #[test]
    fn the_state_carries_selected_lines_never_a_whole_output() {
        let output = json!({"stdout": "line\n".repeat(2_000)}).to_string();
        let claims = scan(&tool("cargo test", &output), "`cargo test` passed.");
        assert_eq!(claims[0].code, CodeVerdict::NeedsReading);
        assert!(claims[0].evidence.len() <= CLAIM_EVIDENCE_BYTE_CAP);
        assert!(!claims[0].evidence.contains("stdout"), "only the selected output text crosses the door");
        assert_eq!(state(&claims)["claims"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn two_sentences_from_one_final_line_become_two_atomic_claims() {
        let claims = scan(&tool("cargo test", r#"{"stdout":"test ok"}"#),
            "`cargo test` passed. `cargo test` is green.");
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[0].id, "C1");
        assert_eq!(claims[1].id, "C2");
    }
}
