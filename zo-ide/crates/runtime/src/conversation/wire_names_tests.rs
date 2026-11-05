//! A core tool's call between the model and zo (t-96186): the model calls the
//! general names (`Read` with `file_path`), the session stores the handler's
//! (`read_file` with `path`), and every road out — the next request, a hook, a
//! permission rule — reads one tool whichever spelling it holds.

use serde_json::json;

use super::helpers::{build_assistant_message, AssistantTurn};
use crate::hook_test_shell::shell_snippet;
use crate::session::{ContentBlock, ConversationMessage};
use crate::{
    convert_messages, AssistantEvent, HookMatcher, HookRunner, PermissionMode, PermissionOutcome,
    PermissionPolicy, RuntimeHookConfig, RuntimePermissionRuleConfig,
};

fn call(id: &str, name: &str, input: &str) -> AssistantEvent {
    AssistantEvent::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input: input.to_string(),
    }
}

/// The session keeps one spelling, whichever one the model used.
#[test]
fn a_call_in_any_spelling_is_stored_under_its_handler() {
    let AssistantTurn::Content { message, .. } = build_assistant_message(vec![
        call("t1", "Read", r#"{"file_path":"src/a.rs"}"#),
        call("t2", "Bash", r#"{"cmd":"ls"}"#),
        call("t3", "Agent", r#"{"description":"d","prompt":"p"}"#),
        AssistantEvent::MessageStop,
    ]) else {
        panic!("expected an assistant message");
    };
    let stored: Vec<(String, serde_json::Value)> = message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { name, input, .. } => Some((
                name.clone(),
                serde_json::from_str(input).expect("stored input is JSON"),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        stored,
        vec![
            ("read_file".to_string(), json!({"path": "src/a.rs"})),
            ("bash".to_string(), json!({"command": "ls"})),
            ("Agent".to_string(), json!({"description": "d", "prompt": "p"})),
        ]
    );
}

/// The model is shown its own earlier call the way it would make it today.
#[test]
fn a_stored_call_reaches_the_model_under_the_names_it_is_shown() {
    let messages = vec![ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: "t1".to_string(),
        name: "read_file".to_string(),
        input: r#"{"path":"src/a.rs","offset":3}"#.to_string(),
    }])];
    let wire = convert_messages(&messages);
    let Some(api::InputContentBlock::ToolUse { name, input, .. }) = wire[0].content.first() else {
        panic!("expected a tool_use on the wire: {wire:?}");
    };
    assert_eq!(name, "Read");
    assert_eq!(input, &json!({"file_path": "src/a.rs", "offset": 3}));
}

/// A hook script written for Claude Code reads `tool_name` and
/// `tool_input.file_path`; zo hands it exactly that.
#[test]
fn a_hook_is_shown_the_call_under_the_general_names() {
    let runner = HookRunner::new(RuntimeHookConfig::new(
        vec![shell_snippet("cat")],
        Vec::new(),
        Vec::new(),
    ));
    let echoed = runner
        .run_pre_tool_use("read_file", r#"{"path":"src/a.rs"}"#)
        .messages()
        .join("");
    assert!(echoed.contains(r#""tool_name":"Read""#), "{echoed}");
    assert!(echoed.contains(r#""file_path":"src/a.rs""#), "{echoed}");
}

/// A matcher written for Claude Code and one written for an older zo both meet
/// the call.
#[test]
fn a_hook_matcher_meets_a_tool_under_any_name_it_answers_to() {
    assert!(HookMatcher::parse("Edit|Write").matches("edit_file"));
    assert!(HookMatcher::parse("edit_file").matches("Edit"));
    assert!(HookMatcher::parse("Bash").matches("bash"));
    assert!(HookMatcher::parse("^Read$").matches("read_file"));
    assert!(!HookMatcher::parse("Edit").matches("write_file"));
}

/// `Edit(src/**)` in a settings file written for Claude Code guards zo's edit
/// tool as well.
#[test]
fn a_rule_under_claude_codes_name_meets_zos_tool() {
    let rules = RuntimePermissionRuleConfig::new(
        Vec::new(),
        vec!["Edit(src/**)".to_string()],
        Vec::new(),
    );
    let policy = PermissionPolicy::new(PermissionMode::DangerFullAccess)
        .with_tool_requirement("edit_file", PermissionMode::WorkspaceWrite)
        .with_permission_rules(&rules);
    assert!(matches!(
        policy.authorize("edit_file", r#"{"path":"src/a.rs"}"#, None),
        PermissionOutcome::Deny { .. }
    ));
}
