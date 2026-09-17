use super::{
    ClipboardSink, ClipboardWrite, ClipboardWriteState, GOAL_TODO_PREFIX, base64_encode,
    clear_goal_todo, custom_provider_catalog_supports_model, fast_variant_pair, gist_create_args,
    last_copy_payload, osc52_sequence, render_repl_help_with_prompt_commands,
    share_gist_warning, sync_goal_todo, write_share_artifact_in,
};
use std::ffi::OsString;
use std::sync::MutexGuard;

struct EnvValueGuard {
    key: &'static str,
    previous: Option<OsString>,
    _lock: MutexGuard<'static, ()>,
}

impl EnvValueGuard {
    fn set(key: &'static str, value: &std::path::Path) -> Self {
        // Route through the single crate-wide env lock so this guard serializes
        // against env-mutating tests in OTHER modules too (a per-file `ENV_LOCK`
        // only serialized within this file, letting `ZO_TODO_STORE` writers
        // elsewhere race and stomp each other).
        let lock = crate::test_env_lock();
        let previous = std::env::var_os(key);
        std::env::set_var(key, value);
        Self {
            key,
            previous,
            _lock: lock,
        }
    }
}

impl Drop for EnvValueGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            std::env::set_var(self.key, previous);
        } else {
            std::env::remove_var(self.key);
        }
    }
}

fn unique_temp_path(label: &str) -> std::path::PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!("zo-{label}-{unique}.json"))
}

#[test]
fn custom_provider_catalog_model_is_supported_case_insensitively() {
    let catalog = [("acme-litellm", vec!["ornith-1.0-9b".to_string()])];

    assert!(custom_provider_catalog_supports_model(
        "ornith-1.0-9b",
        &catalog
    ));
    assert!(custom_provider_catalog_supports_model(
        "Ornith-1.0-9B",
        &catalog
    ));
    assert!(custom_provider_catalog_supports_model(
        " ornith-1.0-9b ",
        &catalog
    ));
    assert!(!custom_provider_catalog_supports_model(
        "qwen3.6-35b-a3b",
        &catalog
    ));
}

#[test]
fn repl_help_includes_project_prompt_commands() {
    let help = render_repl_help_with_prompt_commands(&[commands::PromptCommandDef {
        name: "review-local".to_string(),
        description: Some("Review local diff".to_string()),
        argument_hint: Some("<scope>".to_string()),
        model: None,
        effort: None,
        body: "Review $ARGUMENTS".to_string(),
        allowed_tools: Vec::new(),
        path: std::path::PathBuf::from(".zo/commands/review-local.md"),
    }]);

    assert!(help.contains("Project prompt commands"));
    assert!(help.contains("/review-local <scope>"));
    assert!(help.contains("Review local diff"));
}

#[test]
fn sync_goal_todo_creates_top_goal_item() {
    let path = unique_temp_path("goal-todo-create");
    let _env = EnvValueGuard::set("ZO_TODO_STORE", &path);

    sync_goal_todo("fix rendering").expect("goal todo sync");

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read todo store"))
            .expect("json");
    let items = value.as_array().expect("todo array");
    let expected = format!("{GOAL_TODO_PREFIX}fix rendering");
    assert_eq!(items[0]["content"].as_str(), Some(expected.as_str()));
    assert_eq!(items[0]["status"].as_str(), Some("in_progress"));
    std::fs::remove_file(path).ok();
}

#[test]
fn sync_goal_todo_replaces_prior_goal_and_preserves_other_items() {
    let path = unique_temp_path("goal-todo-replace");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!([
            {
                "content": "Goal: old goal",
                "activeForm": "Working on: old goal",
                "status": "in_progress"
            },
            {
                "content": "keep this task",
                "activeForm": "Keeping this task",
                "status": "pending"
            }
        ]))
        .expect("serialize fixture"),
    )
    .expect("write fixture");
    let _env = EnvValueGuard::set("ZO_TODO_STORE", &path);

    sync_goal_todo("new goal").expect("goal todo sync");

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read todo store"))
            .expect("json");
    let items = value.as_array().expect("todo array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["content"].as_str(), Some("Goal: new goal"));
    assert_eq!(items[1]["content"].as_str(), Some("keep this task"));
    std::fs::remove_file(path).ok();
}

#[test]
fn clear_goal_todo_removes_goal_and_preserves_other_items() {
    let path = unique_temp_path("goal-todo-clear");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!([
            {
                "content": "Goal: old goal",
                "activeForm": "Working on: old goal",
                "status": "in_progress"
            },
            {
                "content": "keep this task",
                "activeForm": "Keeping this task",
                "status": "pending"
            }
        ]))
        .expect("serialize fixture"),
    )
    .expect("write fixture");
    let _env = EnvValueGuard::set("ZO_TODO_STORE", &path);

    assert!(clear_goal_todo().expect("clear goal todo"));

    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read todo store"))
            .expect("json");
    let items = value.as_array().expect("todo array");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["content"].as_str(), Some("keep this task"));
    std::fs::remove_file(path).ok();
}

#[test]
fn goal_todo_clear_line_reports_removed_then_absent() {
    // Pins the exact status strings `handle_goal_advance` (interactive TUI)
    // and the headless loop match on to decide whether to surface a cleanup
    // line. A drift here would silently break the TUI `Goal:` todo cleanup.
    let path = unique_temp_path("goal-todo-clear-line");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&serde_json::json!([
            {
                "content": "Goal: ship it",
                "activeForm": "Working on: ship it",
                "status": "in_progress"
            }
        ]))
        .expect("serialize fixture"),
    )
    .expect("write fixture");
    let _env = EnvValueGuard::set("ZO_TODO_STORE", &path);

    // First clear removes the goal item and reports it.
    let first = super::super::live_cli::LiveCli::goal_todo_clear_line();
    assert!(
        first.contains("removed goal item"),
        "expected removal status, got: {first}"
    );
    // Second clear finds nothing to remove.
    let second = super::super::live_cli::LiveCli::goal_todo_clear_line();
    assert!(
        second.contains("no goal item found"),
        "expected absent status, got: {second}"
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn goal_clear_command_accepts_common_clear_forms() {
    assert_eq!(
        commands::SlashCommand::parse("/goal clear"),
        Ok(Some(commands::SlashCommand::Goal {
            command: commands::GoalCommand::Clear
        }))
    );
    assert_eq!(
        commands::SlashCommand::parse("/goal --clear"),
        Ok(Some(commands::SlashCommand::Goal {
            command: commands::GoalCommand::Clear
        }))
    );
    assert_eq!(
        commands::SlashCommand::parse("/goal 해제"),
        Ok(Some(commands::SlashCommand::Goal {
            command: commands::GoalCommand::Clear
        }))
    );
    assert!(matches!(
        commands::SlashCommand::parse("/goal clear the HUD bug"),
        Ok(Some(commands::SlashCommand::Goal {
            command: commands::GoalCommand::Start { .. }
        }))
    ));
}

#[test]
fn gist_create_is_secret_by_default() {
    let args = gist_create_args("sess-42");
    // The security-critical invariant: never publish a public gist.
    assert!(!args.iter().any(|arg| arg == "--public"), "{args:?}");
    // Reads the body from stdin and names the file after the session.
    assert!(args.iter().any(|arg| arg == "-"));
    assert!(args.iter().any(|arg| arg == "sess-42.txt"));
    assert!(args.first().is_some_and(|arg| arg == "gist"));
}

#[test]
fn share_gist_warning_names_the_real_risks() {
    let warning = share_gist_warning(1234);
    assert!(warning.contains("1234"));
    assert!(warning.contains("UNLISTED"));
    assert!(warning.contains("/unshare"));
}

#[test]
fn write_share_artifact_writes_transcript_to_local_share_dir() {
    let base = std::env::temp_dir().join(format!(
        "zo-share-artifact-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&base).expect("base dir");

    let mut session = runtime::Session::new();
    session
        .push_message(runtime::ConversationMessage::user_text("hello share"))
        .expect("push message");

    let artifact =
        write_share_artifact_in(&base, "sess-123", &session).expect("share artifact written");

    let expected = base.join(".zo").join("share").join("sess-123.txt");
    assert_eq!(artifact.path, expected);
    assert!(expected.exists(), "share file should exist on disk");

    let contents = std::fs::read_to_string(&expected).expect("read share file");
    assert!(contents.contains("# Conversation Export"));
    assert!(contents.contains("hello share"));
    // The reported char count must match exactly what was written so the
    // "Characters" result line is trustworthy.
    assert_eq!(artifact.char_count, contents.len());

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn last_copy_payload_prefers_last_text_message_over_later_tool_blocks() {
    let mut session = runtime::Session::new();
    session
        .push_message(runtime::ConversationMessage::assistant(vec![
            runtime::ContentBlock::Text {
                text: "first part".to_string(),
            },
            runtime::ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: "echo hidden".to_string(),
            },
            runtime::ContentBlock::Text {
                text: "second part".to_string(),
            },
        ]))
        .expect("push assistant");
    session
        .push_message(runtime::ConversationMessage::tool_result(
            "tool-1",
            "bash",
            "tool output should not replace the visible answer",
            false,
        ))
        .expect("push tool result");

    assert_eq!(
        last_copy_payload(&session),
        Some("first part\n\nsecond part".to_string())
    );
}

#[test]
fn last_copy_payload_falls_back_to_non_text_when_no_text_exists() {
    let mut session = runtime::Session::new();
    session
        .push_message(runtime::ConversationMessage::assistant(vec![
            runtime::ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "bash".to_string(),
                input: "echo fallback".to_string(),
            },
        ]))
        .expect("push assistant");

    assert_eq!(
        last_copy_payload(&session),
        Some("echo fallback".to_string())
    );
}

#[test]
fn last_copy_payload_ignores_empty_text_blocks() {
    let mut session = runtime::Session::new();
    session
        .push_message(runtime::ConversationMessage::assistant(vec![
            runtime::ContentBlock::Text {
                text: "   ".to_string(),
            },
            runtime::ContentBlock::Text {
                text: "copy me".to_string(),
            },
        ]))
        .expect("push assistant");

    assert_eq!(last_copy_payload(&session), Some("copy me".to_string()));
}

#[test]
fn base64_encode_matches_rfc4648() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
    assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
}

#[test]
fn osc52_plain_wraps_payload_with_bel_terminator() {
    // ESC ] 52 ; c ; <b64> BEL
    let seq = osc52_sequence("aGVsbG8=", false);
    assert_eq!(seq, "\u{1b}]52;c;aGVsbG8=\u{07}");
    assert!(seq.starts_with("\u{1b}]52;c;"));
    assert!(seq.ends_with('\u{07}'));
    // No stray DCS wrapper outside tmux.
    assert!(!seq.contains("\u{1b}P"));
}

#[test]
fn osc52_tmux_passthrough_doubles_inner_esc() {
    // ESC P tmux ; ESC ESC ] 52 ; c ; <b64> BEL ESC \
    let seq = osc52_sequence("aGVsbG8=", true);
    assert_eq!(seq, "\u{1b}Ptmux;\u{1b}\u{1b}]52;c;aGVsbG8=\u{07}\u{1b}\\");
    assert!(seq.starts_with("\u{1b}Ptmux;"));
    // tmux passthrough terminates with ESC backslash (ST).
    assert!(seq.ends_with("\u{1b}\\"));
    // The inner OSC introducer ESC must be doubled for tmux.
    assert!(seq.contains("\u{1b}\u{1b}]52;c;"));
}

#[test]
fn fast_variant_pair_covers_gpt55_legacy_alias_pair() {
    assert_eq!(
        fast_variant_pair("gpt-5.5"),
        Some(("gpt-5.5".to_string(), "gpt-5.5-fast".to_string()))
    );
    assert_eq!(
        fast_variant_pair("gpt-5.5-fast"),
        Some(("gpt-5.5".to_string(), "gpt-5.5-fast".to_string()))
    );
    // A dated gpt-5.5 id still maps to the same bare pair.
    assert_eq!(
        fast_variant_pair("gpt-5.5-2026-04-23"),
        Some(("gpt-5.5".to_string(), "gpt-5.5-fast".to_string()))
    );
}

#[test]
fn fast_variant_pair_covers_gpt56_bracket_service_tier() {
    assert_eq!(
        fast_variant_pair("gpt-5.6-sol"),
        Some(("gpt-5.6-sol".to_string(), "gpt-5.6-sol[fast]".to_string()))
    );
    assert_eq!(
        fast_variant_pair("gpt-5.6-terra"),
        Some(("gpt-5.6-terra".to_string(), "gpt-5.6-terra[fast]".to_string()))
    );
    assert_eq!(
        fast_variant_pair("gpt-5.6-luna"),
        Some(("gpt-5.6-luna".to_string(), "gpt-5.6-luna[fast]".to_string()))
    );
    // Already-toggled id still resolves to the same pair (idempotent status
    // detection — `toggle_fast` compares the current model against `fast_id`).
    assert_eq!(
        fast_variant_pair("gpt-5.6-terra[fast]"),
        Some(("gpt-5.6-terra".to_string(), "gpt-5.6-terra[fast]".to_string()))
    );
}

#[test]
fn fast_variant_pair_is_none_for_families_with_no_fast_variant() {
    assert_eq!(fast_variant_pair("claude-opus-4-8"), None);
    assert_eq!(fast_variant_pair("gemini-3-pro"), None);
    assert_eq!(fast_variant_pair("grok-3"), None);
    assert_eq!(fast_variant_pair("deepseek-chat"), None);
    // GPT-5.3 Codex Spark is a known GPT family but has no fast-variant pair.
    assert_eq!(fast_variant_pair("gpt-5.3-codex-spark"), None);
}

#[tokio::test]
async fn clipboard_write_keeps_result_after_cancelled_select_wait() {
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    let mut write = ClipboardWrite {
        text: "copied text".to_string(),
        state: ClipboardWriteState::Pending(rx),
    };

    tokio::select! {
        () = write.wait_until_ready() => panic!("clipboard worker should still be pending"),
        () = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
    }
    assert!(!write.is_ready());

    tx.send(Ok("test-helper"))
        .await
        .expect("clipboard receiver remains after select cancellation");
    write.wait_until_ready().await;

    assert!(write.is_ready());
    assert!(matches!(
        write.finish(),
        Ok(ClipboardSink::Command("test-helper"))
    ));
}
