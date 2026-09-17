//! An Anthropic request-body validator for the tests.
//!
//! The mock provider accepts anything, which is the point of a mock and also
//! its blind spot: a body that the real API answers with `400
//! invalid_request_error` sails through it and the test goes green. Every
//! measurement that asks "would this session survive its next request?" has to
//! read the body itself, so the rules live here once and both the e2e cases and
//! the measurement runs call the same checker.
//!
//! The rules encoded below are the ones a killed-and-resumed session can break:
//!
//! * **Paired tool use.** Every `tool_use` block must be answered by a
//!   `tool_result` with the same id in the message that *immediately follows*
//!   the assistant message that issued it. Anthropic's message is `tool_use
//!   ids were found without tool_result blocks`; the session that gets it is
//!   bricked, because the offending pair sits in history and every later
//!   request carries it again.
//! * **Results lead.** Within that user message the `tool_result` blocks must
//!   come first: the validator only credits results at the head, so a text
//!   block ahead of them hides every result behind it.
//! * **Alternating roles.** Two messages with the same role in a row are a 400
//!   (`messages: roles must alternate between "user" and "assistant"`), and the
//!   first message must be `user`.
//!
//! Kept separate per rule so a test can pin the one it fixed while still
//! *reporting* the others — a checker that only says "invalid" cannot tell a
//! measurement which wall it hit.

use serde_json::Value;

/// Which contract rule a body broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// A `tool_use` with no `tool_result` immediately answering it.
    OrphanToolUse,
    /// A `tool_result` sitting behind a non-result block in its message.
    BuriedToolResult,
    /// Two same-role messages in a row, or an opening `assistant`.
    RoleAlternation,
}

/// One broken rule, with enough context to name it in a failure message.
#[derive(Debug, Clone)]
pub struct Violation {
    pub rule: Rule,
    pub message_index: usize,
    pub detail: String,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "[{:?} @ messages[{}]] {}",
            self.rule, self.message_index, self.detail
        )
    }
}

/// Every way `body` breaks the Anthropic message contract, in message order.
///
/// A body that does not parse, or that carries no `messages` array, is itself
/// reported rather than silently passing — a checker that returns "clean" for
/// input it could not read is worse than no checker.
#[must_use]
pub fn violations(body: &str) -> Vec<Violation> {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return vec![Violation {
            rule: Rule::OrphanToolUse,
            message_index: 0,
            detail: "request body is not JSON".to_string(),
        }];
    };
    let Some(messages) = parsed.get("messages").and_then(Value::as_array) else {
        return vec![Violation {
            rule: Rule::OrphanToolUse,
            message_index: 0,
            detail: "request body has no `messages` array".to_string(),
        }];
    };

    let mut found = Vec::new();
    found.extend(role_violations(messages));
    found.extend(tool_pairing_violations(messages));
    found.sort_by_key(|violation| violation.message_index);
    found
}

/// Only the violations that brick a session outright — the pairing rules.
///
/// Role alternation is checked and reported but deliberately not folded in
/// here: it is a separate wall, and a pin that conflates the two cannot say
/// which one a regression knocked over.
#[must_use]
pub fn tool_pairing_violations(messages: &[Value]) -> Vec<Violation> {
    let mut found = Vec::new();
    for (index, message) in messages.iter().enumerate() {
        if role_of(message) != "assistant" {
            continue;
        }
        let uses: Vec<String> = blocks_of(message)
            .iter()
            .filter(|block| type_of(block) == "tool_use")
            .filter_map(|block| block.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        if uses.is_empty() {
            continue;
        }
        let next = messages.get(index + 1);
        let answered: Vec<String> = next
            .map(|message| {
                blocks_of(message)
                    .iter()
                    .filter(|block| type_of(block) == "tool_result")
                    .filter_map(|block| block.get("tool_use_id").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let next_is_user = next.is_some_and(|message| role_of(message) == "user");
        for id in &uses {
            if !next_is_user || !answered.contains(id) {
                found.push(Violation {
                    rule: Rule::OrphanToolUse,
                    message_index: index,
                    detail: format!(
                        "tool_use {id} is not answered by a tool_result in messages[{}] (role {:?})",
                        index + 1,
                        next.map_or("<end of messages>", role_of),
                    ),
                });
            }
        }
        if let Some(next) = next {
            found.extend(buried_result_violations(next, index + 1));
        }
    }
    found
}

/// `tool_result` blocks must lead their message; report the first that does not.
fn buried_result_violations(message: &Value, index: usize) -> Vec<Violation> {
    let mut seen_other = false;
    let mut found = Vec::new();
    for block in blocks_of(message) {
        if type_of(block) == "tool_result" {
            if seen_other {
                found.push(Violation {
                    rule: Rule::BuriedToolResult,
                    message_index: index,
                    detail: format!(
                        "tool_result {} sits behind a non-result block",
                        block
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .unwrap_or("<no id>"),
                    ),
                });
                break;
            }
        } else {
            seen_other = true;
        }
    }
    found
}

fn role_violations(messages: &[Value]) -> Vec<Violation> {
    let mut found = Vec::new();
    let mut previous: Option<&str> = None;
    for (index, message) in messages.iter().enumerate() {
        let role = role_of(message);
        if index == 0 && role != "user" {
            found.push(Violation {
                rule: Rule::RoleAlternation,
                message_index: 0,
                detail: format!("first message has role {role:?}, must be \"user\""),
            });
        }
        if previous == Some(role) {
            found.push(Violation {
                rule: Rule::RoleAlternation,
                message_index: index,
                detail: format!("role {role:?} repeats messages[{}]", index - 1),
            });
        }
        previous = Some(role);
    }
    found
}

fn role_of(message: &Value) -> &str {
    message.get("role").and_then(Value::as_str).unwrap_or("")
}

fn blocks_of(message: &Value) -> &[Value] {
    message
        .get("content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn type_of(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

/// A compact `role: [block types]` sketch of a body, for failure messages and
/// measurement notes. The whole body is tens of kilobytes of system prompt; the
/// shape is the part a person reads.
#[must_use]
pub fn message_shape(body: &str) -> String {
    let Ok(parsed) = serde_json::from_str::<Value>(body) else {
        return "<unparseable body>".to_string();
    };
    let Some(messages) = parsed.get("messages").and_then(Value::as_array) else {
        return "<no messages>".to_string();
    };
    messages
        .iter()
        .enumerate()
        .map(|(index, message)| {
            let kinds = blocks_of(message)
                .iter()
                .map(|block| {
                    let kind = type_of(block);
                    match kind {
                        "tool_use" => format!(
                            "tool_use:{}",
                            block.get("id").and_then(Value::as_str).unwrap_or("?")
                        ),
                        "tool_result" => format!(
                            "tool_result:{}",
                            block
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or("?")
                        ),
                        other => other.to_string(),
                    }
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("  [{index}] {}: [{kinds}]", role_of(message))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{violations, Rule};

    fn body(messages: &str) -> String {
        format!(r#"{{"model":"m","messages":{messages}}}"#)
    }

    #[test]
    fn a_paired_tool_turn_is_clean() {
        let found = violations(&body(
            r#"[
                {"role":"user","content":[{"type":"text","text":"go"}]},
                {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"bash","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]},
                {"role":"assistant","content":[{"type":"text","text":"done"}]}
            ]"#,
        ));
        assert!(found.is_empty(), "unexpected: {found:#?}");
    }

    #[test]
    fn a_trailing_tool_use_is_an_orphan() {
        let found = violations(&body(
            r#"[
                {"role":"user","content":[{"type":"text","text":"go"}]},
                {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"bash","input":{}}]}
            ]"#,
        ));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].rule, Rule::OrphanToolUse);
    }

    #[test]
    fn a_result_that_arrives_a_message_late_is_still_an_orphan() {
        let found = violations(&body(
            r#"[
                {"role":"user","content":[{"type":"text","text":"go"}]},
                {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"bash","input":{}}]},
                {"role":"user","content":[{"type":"text","text":"never mind"}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"late"}]}
            ]"#,
        ));
        assert!(found.iter().any(|found| found.rule == Rule::OrphanToolUse), "{found:#?}");
    }

    #[test]
    fn a_result_behind_text_is_buried() {
        let found = violations(&body(
            r#"[
                {"role":"user","content":[{"type":"text","text":"go"}]},
                {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"bash","input":{}}]},
                {"role":"user","content":[{"type":"text","text":"aside"},{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}
            ]"#,
        ));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].rule, Rule::BuriedToolResult);
    }

    #[test]
    fn two_user_messages_in_a_row_break_alternation() {
        let found = violations(&body(
            r#"[
                {"role":"user","content":[{"type":"text","text":"one"}]},
                {"role":"user","content":[{"type":"text","text":"two"}]}
            ]"#,
        ));
        assert_eq!(found.len(), 1, "{found:#?}");
        assert_eq!(found[0].rule, Rule::RoleAlternation);
        assert_eq!(found[0].message_index, 1);
    }
}
