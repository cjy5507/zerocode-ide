//! Completion claims beside the tool results of the same turn. The existing
//! r43 gate owns the turn-end seam; this reader only prepares a bounded,
//! provenance-preserving question for the Jev seat.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use serde_json::{json, Value};
use zerocode_core::jev::{CLAIM_CRITERIA, CLAIM_EVIDENCE_BYTE_CAP, CLAIM_LIMIT, CLAIM_TEXT_CHAR_CAP};

use super::deep_gate::{bash_result_exited_nonzero, command_is_check_shaped};
use super::verified_state::tool_verified_state_events;
use super::{ContentBlock, ConversationMessage};
use crate::verified_state::VerifiedStateEvent;

pub const CLAIM_RUBRIC_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeVerdict {
    NeedsReading,
    Unsupported,
    Contradicted,
}

impl CodeVerdict {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::NeedsReading => "needs_reading",
            Self::Unsupported => "unsupported",
            Self::Contradicted => CLAIM_CRITERIA[1].0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimCandidate {
    pub id: String,
    pub text: String,
    pub evidence: String,
    pub code: CodeVerdict,
}

fn claim_pattern() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(?:done|complete(?:d)?|finished|implemented|fixed|green|pass(?:ed|es|ing)?)\b|완료|끝났|마쳤|고침|고쳤|수정했|통과|초록|검증했").ok()
    }).as_ref()
}

fn quoted_command() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"`((?:cargo|just|npm|pnpm|pytest|python|go|make|git) [^`\n]+)`").ok()).as_ref()
}

fn named_path() -> Option<&'static Regex> {
    static PATTERN: OnceLock<Option<Regex>> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"(?:^|[\s`(])([\w./-]+\.[A-Za-z0-9]+)(?::\d+)?").ok()).as_ref()
}

#[derive(Debug)]
struct Evidence<'a> {
    command: Option<String>,
    output: &'a str,
    is_error: bool,
    nonzero: bool,
}

/// Candidates in the answer's final paragraph. A named command has to have
/// run in this turn; a passing claim over its failed result is settled by
/// code. Only claims with matching evidence reach a Jev question.
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
    let paragraph = final_text.trim_end().rsplit("\n\n").next().unwrap_or("");
    paragraph.lines()
        .flat_map(|line| line.split('。'))
        .flat_map(|part| part.split(". "))
        .map(str::trim)
        .filter(|part| claim_pattern().is_some_and(|pattern| pattern.is_match(part)))
        .take(CLAIM_LIMIT)
        .enumerate()
        .map(|(index, text)| {
            let command = quoted_command().and_then(|pattern| pattern.captures(text)).map(|found| found[1].to_string());
            let path = named_path().and_then(|pattern| pattern.captures(text)).map(|found| found[1].to_string());
            let found = evidence.iter().rev().find(|item| {
                command.as_ref().is_none_or(|command| item.command.as_ref() == Some(command))
                    && path.as_ref().is_none_or(|path| item.command.as_deref().is_some_and(|cmd| cmd.contains(path)) || item.output.contains(path))
            });
            let mut code = match found {
                None => CodeVerdict::Unsupported,
                Some(item) if (text.to_lowercase().contains("pass") || text.contains("통과") || text.to_lowercase().contains("green") || text.contains("초록"))
                    && item.nonzero && (command.is_some() || item.command.as_deref().is_some_and(command_is_check_shaped)) => CodeVerdict::Contradicted,
                Some(item) if item.is_error => CodeVerdict::Unsupported,
                Some(_) => CodeVerdict::NeedsReading,
            };
            // The executor's JSON envelope carries exit information; the
            // question gets only the textual output lines.
            let selected = found.map_or(String::new(), |item| {
                serde_json::from_str::<Value>(item.output).ok().map_or_else(
                    || item.output.to_string(),
                    |value| ["stdout", "stderr"].iter()
                        .filter_map(|key| value.get(*key).and_then(Value::as_str))
                        .collect::<Vec<_>>().join("\n"),
                )
            });
            let lines = selected.lines().filter(|line| !line.trim().is_empty()).rev().take(12).collect::<Vec<_>>();
            let mut excerpt = lines.into_iter().rev().collect::<Vec<_>>().join("\n");
            if excerpt.len() > CLAIM_EVIDENCE_BYTE_CAP {
                let mut start = excerpt.len() - CLAIM_EVIDENCE_BYTE_CAP;
                while !excerpt.is_char_boundary(start) {
                    start += 1;
                }
                excerpt = excerpt[start..].to_string();
            }
            if excerpt.is_empty() && code == CodeVerdict::NeedsReading {
                code = CodeVerdict::Unsupported;
            }
            ClaimCandidate {
                id: format!("C{}", index + 1),
                text: text.chars().take(CLAIM_TEXT_CHAR_CAP).collect(),
                evidence: excerpt,
                code,
            }
        })
        .collect()
}

/// All unresolved claims share one request and each has its own Choice.
#[must_use]
pub fn state(claims: &[ClaimCandidate]) -> Value {
    json!({
        "claims": claims.iter().filter(|claim| claim.code == CodeVerdict::NeedsReading)
            .map(|claim| json!({"id": claim.id, "text": claim.text})).collect::<Vec<_>>(),
        "evidence": claims.iter().filter(|claim| claim.code == CodeVerdict::NeedsReading)
            .map(|claim| (claim.id.clone(), claim.evidence.clone())).collect::<HashMap<_, _>>(),
    })
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
