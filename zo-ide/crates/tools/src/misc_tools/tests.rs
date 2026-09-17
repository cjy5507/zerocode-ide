use core_types::helper_run::HelperRun;
use super::{resolve_option_choice, run_ask_user_question, AskUserQuestionInput};
use crate::{ToolContext, ToolError, UserQuestionChannel};
use runtime::message_stream::QuestionOption;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::MutexGuard;
use std::time::{SystemTime, UNIX_EPOCH};

/// The one header every host leads a completion with must carry what the run
/// cost, so the model can weigh a helper's answer against its price without
/// asking for a second report.
#[test]
fn the_completion_header_carries_what_the_run_cost() {
    let header = super::background_agent_notification_header(
        "agent-7",
        "runtime-scout",
        "completed",
        None,
        HelperRun {
            tool_calls: 12,
            output_tokens: 45_300,
            elapsed: Some(std::time::Duration::from_secs(80)),
        },
    );
    assert!(
        header.contains("finished (12 tool uses · 45.3k tokens · 1m 20s)."),
        "{header}"
    );
    assert!(header.contains("use SendMessage with that id/name as `to`"), "{header}");
}

/// A backend that measured nothing must leave the sentence exactly as it read
/// before the cost existed — an empty `()` would be noise, and the resume
/// parser reads this text back.
#[test]
fn a_completion_nothing_was_measured_for_keeps_the_bare_header() {
    let header = super::background_agent_notification_header(
        "agent-7",
        "runtime-scout",
        "completed",
        None,
        HelperRun::default(),
    );
    assert!(header.contains("(id: agent-7) finished. To follow up"), "{header}");
}

/// Delegate to the crate-wide env lock: several sibling test modules mutate
/// the same process-global variables (`ZO_CONFIG_HOME`, `HOME`, …), and a
/// module-local mutex provides zero mutual exclusion against them — the
/// wandering `memory_write_local_targets` flake was exactly that race.
fn env_lock() -> MutexGuard<'static, ()> {
    crate::tests::env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn with_config_home<T>(home: &std::path::Path, f: impl FnOnce() -> T) -> T {
    let previous = std::env::var_os("ZO_CONFIG_HOME");
    std::env::set_var("ZO_CONFIG_HOME", home);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match previous {
        Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
        None => std::env::remove_var("ZO_CONFIG_HOME"),
    }
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn temp_dir() -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "tools-memory-{}-{nanos}-{counter}",
        std::process::id()
    ))
}

struct FixedChannel(String);

impl UserQuestionChannel for FixedChannel {
    fn ask(
        &self,
        _question: &str,
        _header: Option<&str>,
        _options: &[QuestionOption],
        _multi_select: bool,
    ) -> Result<Vec<String>, ToolError> {
        Ok(vec![self.0.clone()])
    }
}

/// A channel that echoes a fixed list of picks — the multi-select analogue of
/// [`FixedChannel`]. It records the `multi_select` flag it was handed so a test
/// can assert the flag threaded through from input to channel.
struct MultiChannel {
    picks: Vec<String>,
    saw_multi: std::sync::atomic::AtomicBool,
}

impl MultiChannel {
    fn new(picks: &[&str]) -> Self {
        Self {
            picks: picks.iter().map(|p| (*p).to_string()).collect(),
            saw_multi: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl UserQuestionChannel for MultiChannel {
    fn ask(
        &self,
        _question: &str,
        _header: Option<&str>,
        _options: &[QuestionOption],
        multi_select: bool,
    ) -> Result<Vec<String>, ToolError> {
        self.saw_multi
            .store(multi_select, std::sync::atomic::Ordering::Relaxed);
        Ok(self.picks.clone())
    }
}

#[test]
fn channel_delivers_answer() {
    let ch = FixedChannel("yes".to_string());
    let input = AskUserQuestionInput {
        question: "Continue?".to_string(),
        header: None,
        options: None,
        multi_select: false,
    };
    let result = run_ask_user_question(input, Some(&ch)).expect("should succeed");
    assert!(result.contains("\"answer\": \"yes\""));
    assert!(result.contains("\"status\": \"answered\""));
}

#[test]
fn channel_resolves_numeric_option() {
    let ch = FixedChannel("2".to_string());
    let input = AskUserQuestionInput {
        question: "Pick one".to_string(),
        header: None,
        options: Some(vec![
            QuestionOption::plain("alpha"),
            QuestionOption::plain("beta"),
        ]),
        multi_select: false,
    };
    let result = run_ask_user_question(input, Some(&ch)).expect("should succeed");
    assert!(result.contains("\"answer\": \"beta\""));
}

#[test]
fn multi_select_returns_answer_array() {
    // A multi-select prompt returns every checked label as a JSON array, and
    // the flag threads from input all the way to the channel.
    let ch = MultiChannel::new(&["alpha", "gamma"]);
    let input = AskUserQuestionInput {
        question: "Pick any".to_string(),
        header: None,
        options: Some(vec![
            QuestionOption::plain("alpha"),
            QuestionOption::plain("beta"),
            QuestionOption::plain("gamma"),
        ]),
        multi_select: true,
    };
    let result = run_ask_user_question(input, Some(&ch)).expect("should succeed");
    let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert_eq!(
        parsed["answer"],
        serde_json::json!(["alpha", "gamma"]),
        "multi-select answer must be the selected-label array: {result}"
    );
    assert_eq!(parsed["status"], "answered");
    assert!(
        ch.saw_multi.load(std::sync::atomic::Ordering::Relaxed),
        "the multi_select flag must reach the channel"
    );
}

#[test]
fn multi_select_maps_numeric_picks_to_labels() {
    // Numeric picks (the stdio "type 2,3" form) map to labels per element.
    let ch = MultiChannel::new(&["1", "3"]);
    let input = AskUserQuestionInput {
        question: "Pick any".to_string(),
        header: None,
        options: Some(vec![
            QuestionOption::plain("alpha"),
            QuestionOption::plain("beta"),
            QuestionOption::plain("gamma"),
        ]),
        multi_select: true,
    };
    let result = run_ask_user_question(input, Some(&ch)).expect("should succeed");
    let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
    assert_eq!(parsed["answer"], serde_json::json!(["alpha", "gamma"]));
}

#[test]
fn ask_input_parses_multi_select_flag() {
    // The schema field `multiSelect` (and its snake_case alias) flips the flag;
    // absence keeps the single-select default.
    let camel: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "question": "Pick any",
        "options": ["a", "b"],
        "multiSelect": true,
    }))
    .expect("camelCase multiSelect deserializes");
    assert!(camel.multi_select);

    let snake: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "question": "Pick any",
        "options": ["a", "b"],
        "multi_select": true,
    }))
    .expect("snake_case multi_select deserializes");
    assert!(snake.multi_select);

    let default: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "question": "Pick one",
        "options": ["a", "b"],
    }))
    .expect("plain deserializes");
    assert!(!default.multi_select, "absence defaults to single-select");
}

#[test]
fn ask_input_accepts_rich_and_plain_options() {
    // The model may mix bare strings with {label, description} objects; both
    // deserialize into QuestionOption and the header rides along.
    let input: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "question": "Which auth?",
        "header": "Auth method",
        "options": [
            {"label": "OAuth", "description": "browser login, auto refresh"},
            "API key"
        ]
    }))
    .expect("rich input deserializes");
    assert_eq!(input.header.as_deref(), Some("Auth method"));
    let options = input.options.expect("options present");
    assert_eq!(options[0].label, "OAuth");
    assert_eq!(
        options[0].description.as_deref(),
        Some("browser login, auto refresh")
    );
    assert_eq!(options[1], QuestionOption::plain("API key"));
}

#[test]
fn ask_input_unwraps_nested_json_in_question() {
    // gpt-5.5-fast sometimes JSON-encodes the whole payload into `question`,
    // which previously rendered as raw JSON inside the popup. Recover it.
    let input: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "header": "Auth method",
        "question": "{\"question\": \"Which auth?\", \"options\": [{\"label\": \"OAuth\", \"description\": \"browser login\"}, \"API key\"]}"
    }))
    .expect("nested json deserializes");
    assert_eq!(input.question, "Which auth?");
    assert_eq!(input.header.as_deref(), Some("Auth method"));
    let options = input.options.expect("options recovered");
    assert_eq!(options[0].label, "OAuth");
    assert_eq!(options[1], QuestionOption::plain("API key"));
}

#[test]
fn ask_input_unwraps_questions_envelope() {
    // Some models mirror the harness `{ questions: [ … ] }` shape.
    let input: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "questions": [{
            "question": "Pick one",
            "header": "Topic",
            "options": ["alpha", "beta"],
        }]
    }))
    .expect("envelope deserializes");
    assert_eq!(input.question, "Pick one");
    assert_eq!(input.header.as_deref(), Some("Topic"));
    assert_eq!(input.options.expect("options").len(), 2);
}

#[test]
fn ask_input_splits_trailing_option_array_from_question() {
    let input: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "question": "Which one?\n[{\"label\": \"a\"}, {\"label\": \"b\"}]"
    }))
    .expect("trailing array deserializes");
    assert_eq!(input.question, "Which one?");
    assert_eq!(input.options.expect("options").len(), 2);
}

#[test]
fn ask_input_keeps_plain_question_untouched() {
    let input: AskUserQuestionInput = serde_json::from_value(serde_json::json!({
        "question": "Continue?"
    }))
    .expect("plain deserializes");
    assert_eq!(input.question, "Continue?");
    assert!(input.options.is_none());
}

#[test]
fn no_channel_non_interactive_returns_unanswered_payload() {
    let input = AskUserQuestionInput {
        question: "Need input?".to_string(),
        header: None,
        options: Some(vec![
            QuestionOption::plain("yes"),
            QuestionOption::plain("no"),
        ]),
        multi_select: false,
    };
    let result = super::run_ask_user_question_with_terminal_state(input, None, false)
        .expect("non-interactive fallback should return JSON");
    assert!(result.contains("\"status\": \"unanswered\""));
    assert!(result.contains("\"reason\": \"non-interactive\""));
    assert!(result.contains("\"question\": \"Need input?\""));
    assert!(!result.contains("\"answer\""));
}

#[test]
fn memory_write_writes_entry_and_upserts_index_once() {
    let _guard = env_lock();
    let root = temp_dir();
    std::fs::create_dir_all(&root).expect("temp root");
    let config_home = root.join("home").join(".zo");
    std::fs::create_dir_all(&config_home).expect("config home");
    let config_home = std::fs::canonicalize(config_home).expect("canonical config home");
    let ctx = ToolContext::new().with_cwd(root.clone());

    let first = with_config_home(&config_home, || {
        super::run_memory_write(
            &super::MemoryWriteInput {
                slug: "Runtime Notes.md".to_string(),
                summary: "first summary".to_string(),
                body: "# Runtime Notes\n\nFirst body.".to_string(),
                local: false,
            },
            &ctx,
        )
    })
    .expect("first write");
    assert!(first.contains("\"slug\": \"runtime-notes\""));

    with_config_home(&config_home, || {
        super::run_memory_write(
            &super::MemoryWriteInput {
                slug: "runtime-notes".to_string(),
                summary: "updated summary".to_string(),
                body: "# Runtime Notes\n\nUpdated body.".to_string(),
                local: false,
            },
            &ctx,
        )
    })
    .expect("second write");

    let memory_dir = with_config_home(&config_home, || {
        runtime::memory::paths::memory_write_dir(&root, false)
    });
    assert!(memory_dir.starts_with(&config_home));
    let entry = std::fs::read_to_string(memory_dir.join("runtime-notes.md")).expect("entry file");
    assert!(entry.contains("Updated body."));
    assert!(entry.contains("- memory_metadata: v=1;source=hand_written;"));
    assert!(entry.contains("protected=true"));
    let classification = runtime::memory::classify_memory_body(&entry);
    assert_eq!(classification.source, runtime::memory::MemorySource::HandWritten);
    assert_eq!(classification.kind, runtime::memory::MemoryKind::Unknown);
    assert!(classification.protected);
    let index = std::fs::read_to_string(memory_dir.join("MEMORY.md")).expect("memory index");
    assert_eq!(index.matches("](runtime-notes.md)").count(), 1);
    assert!(index.contains("updated summary"));

    std::fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn memory_write_records_the_model_that_is_writing_it() {
    // The tag has to come from the live session, not from the tool input: a
    // `/model` switch refreshes `active_model` on every registry clone, and
    // that cell is the only thing that knows which model is speaking.
    let _guard = env_lock();
    let root = temp_dir();
    std::fs::create_dir_all(&root).expect("temp root");
    let config_home = root.join("home").join(".zo");
    std::fs::create_dir_all(&config_home).expect("config home");
    let config_home = std::fs::canonicalize(config_home).expect("canonical config home");
    let ctx = ToolContext::new().with_cwd(root.clone());
    ctx.set_active_model("Claude-Opus-5");

    let write = |slug: &str| {
        with_config_home(&config_home, || {
            super::run_memory_write(
                &super::MemoryWriteInput {
                    slug: slug.to_string(),
                    summary: "a lesson".to_string(),
                    body: "the lesson body".to_string(),
                    local: false,
                },
                &ctx,
            )
        })
        .expect("memory write");
    };
    write("opus-lesson");

    let memory_dir = with_config_home(&config_home, || {
        runtime::memory::paths::memory_write_dir(&root, false)
    });
    let entry = std::fs::read_to_string(memory_dir.join("opus-lesson.md")).expect("entry");
    assert_eq!(
        runtime::memory::classify_memory_body(&entry)
            .model
            .as_ref()
            .map(runtime::memory::MemoryModelTag::as_str),
        Some("claude-opus-5"),
        "the writing model is stamped, normalized: {entry}"
    );

    // After a `/model` switch the very next write carries the new model.
    ctx.set_active_model("gpt-5.6-sol");
    write("sol-lesson");
    let entry = std::fs::read_to_string(memory_dir.join("sol-lesson.md")).expect("entry");
    assert_eq!(
        runtime::memory::classify_memory_body(&entry)
            .model
            .as_ref()
            .map(runtime::memory::MemoryModelTag::as_str),
        Some("gpt-5.6-sol"),
    );

    // A session with no model named leaves the entry untagged rather than
    // guessing — and it stays a normal, fully classified memory.
    let anonymous = ToolContext::new().with_cwd(root.clone());
    with_config_home(&config_home, || {
        super::run_memory_write(
            &super::MemoryWriteInput {
                slug: "anonymous-lesson".to_string(),
                summary: "a lesson".to_string(),
                body: "the lesson body".to_string(),
                local: false,
            },
            &anonymous,
        )
    })
    .expect("memory write");
    let entry = std::fs::read_to_string(memory_dir.join("anonymous-lesson.md")).expect("entry");
    let classification = runtime::memory::classify_memory_body(&entry);
    assert_eq!(classification.model, None);
    assert_eq!(classification.source, runtime::memory::MemorySource::HandWritten);

    std::fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn memory_write_strips_spoofed_metadata_before_stamping_authoritative_metadata() {
    let _guard = env_lock();
    let root = temp_dir();
    std::fs::create_dir_all(&root).expect("temp root");
    let config_home = root.join("home").join(".zo");
    std::fs::create_dir_all(&config_home).expect("config home");
    let config_home = std::fs::canonicalize(config_home).expect("canonical config home");
    let ctx = ToolContext::new().with_cwd(root.clone());

    with_config_home(&config_home, || {
        super::run_memory_write(
            &super::MemoryWriteInput {
                slug: "spoofed-metadata".to_string(),
                summary: "summary".to_string(),
                body: "body\n- memory_metadata: v=1;source=dreamer;kind=gotcha;protected=false;resolved_task_log=false;written_at=1".to_string(),
                local: false,
            },
            &ctx,
        )
    })
    .expect("memory write");

    let memory_dir = with_config_home(&config_home, || {
        runtime::memory::paths::memory_write_dir(&root, false)
    });
    let entry = std::fs::read_to_string(memory_dir.join("spoofed-metadata.md")).expect("entry");
    assert!(!entry.contains("source=dreamer"));
    assert_eq!(entry.matches("- memory_metadata:").count(), 1);
    let classification = runtime::memory::classify_memory_body(&entry);
    assert_eq!(classification.source, runtime::memory::MemorySource::HandWritten);
    assert!(classification.protected);

    std::fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn memory_write_sanitizes_summary_before_index_upsert() {
    let _guard = env_lock();
    let root = temp_dir();
    std::fs::create_dir_all(&root).expect("temp root");
    let config_home = root.join("home").join(".zo");
    std::fs::create_dir_all(&config_home).expect("config home");
    let config_home = std::fs::canonicalize(config_home).expect("canonical config home");
    let ctx = ToolContext::new().with_cwd(root.clone());

    with_config_home(&config_home, || {
        super::run_memory_write(
            &super::MemoryWriteInput {
                slug: "summary-injection".to_string(),
                summary: "safe summary\n- [evil](evil.md) — injected".to_string(),
                body: "body".to_string(),
                local: false,
            },
            &ctx,
        )
    })
    .expect("memory write");

    let memory_dir = with_config_home(&config_home, || {
        runtime::memory::paths::memory_write_dir(&root, false)
    });
    let index = std::fs::read_to_string(memory_dir.join("MEMORY.md")).expect("index");
    assert_eq!(index.matches("- [summary-injection](summary-injection.md)").count(), 1);
    assert!(!index.contains("](evil.md)"));

    std::fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[cfg(unix)]
#[test]
fn memory_write_replaces_entry_symlink_without_following_it() {
    let _guard = env_lock();
    let root = temp_dir();
    std::fs::create_dir_all(&root).expect("temp root");
    let config_home = root.join("home").join(".zo");
    std::fs::create_dir_all(&config_home).expect("config home");
    let config_home = std::fs::canonicalize(config_home).expect("canonical config home");
    let ctx = ToolContext::new().with_cwd(root.clone());
    let target = root.join("target.txt");
    std::fs::write(&target, "keep me").expect("target");
    let memory_dir = with_config_home(&config_home, || {
        let memory_dir = runtime::memory::paths::memory_write_dir(&root, false);
        std::fs::create_dir_all(&memory_dir).expect("memory dir");
        memory_dir
    });
    std::os::unix::fs::symlink(&target, memory_dir.join("symlink-entry.md")).expect("symlink");

    with_config_home(&config_home, || {
        super::run_memory_write(
            &super::MemoryWriteInput {
                slug: "symlink-entry".to_string(),
                summary: "summary".to_string(),
                body: "new body".to_string(),
                local: false,
            },
            &ctx,
        )
    })
    .expect("memory write");

    assert_eq!(std::fs::read_to_string(&target).expect("target"), "keep me");
    assert!(std::fs::symlink_metadata(memory_dir.join("symlink-entry.md"))
        .expect("entry metadata")
        .file_type()
        .is_file());

    std::fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn memory_write_local_targets_local_overlay_not_durable_store() {
    let _guard = env_lock();
    let root = temp_dir();
    std::fs::create_dir_all(&root).expect("temp root");
    let config_home = root.join("home").join(".zo");
    std::fs::create_dir_all(&config_home).expect("config home");
    let config_home = std::fs::canonicalize(config_home).expect("canonical config home");
    let ctx = ToolContext::new().with_cwd(root.clone());

    let report = with_config_home(&config_home, || {
        super::run_memory_write(
            &super::MemoryWriteInput {
                slug: "local-token".to_string(),
                summary: "machine-local deploy token".to_string(),
                body: "Local-only note.".to_string(),
                local: true,
            },
            &ctx,
        )
    })
    .expect("local write");
    assert!(report.contains("\"local\": true"));

    // Entry + index land in the machine-local overlay, not the durable store.
    let (local_dir, durable_dir) = with_config_home(&config_home, || {
        (
            runtime::memory::paths::memory_write_dir(&root, true),
            runtime::memory::paths::memory_write_dir(&root, false),
        )
    });
    assert!(local_dir.starts_with(&config_home));
    assert!(
        local_dir.join("local-token.md").exists(),
        "entry missing at recomputed dir {}; write-time report was: {report}",
        local_dir.display()
    );
    let index = std::fs::read_to_string(local_dir.join("MEMORY.md")).expect("local memory index");
    assert!(index.contains("](local-token.md)"));
    assert!(
        !durable_dir.exists(),
        "a local write must not touch the durable store"
    );

    std::fs::remove_dir_all(root).expect("cleanup temp dir");
}

#[test]
fn resolve_option_choice_returns_text_for_non_numeric() {
    let opts = vec![QuestionOption::plain("a"), QuestionOption::plain("b")];
    assert_eq!(resolve_option_choice("hello", &opts), "hello");
}

#[test]
fn resolve_option_choice_returns_option_for_valid_index() {
    let opts = vec![QuestionOption::plain("a"), QuestionOption::plain("b")];
    assert_eq!(resolve_option_choice("1", &opts), "a");
    assert_eq!(resolve_option_choice("2", &opts), "b");
}

#[test]
fn resolve_option_choice_returns_raw_for_out_of_range() {
    let opts = vec![QuestionOption::plain("a")];
    assert_eq!(resolve_option_choice("5", &opts), "5");
    assert_eq!(resolve_option_choice("0", &opts), "0");
}

#[test]
fn coerce_agents_accepts_real_array() {
    let out = super::coerce_agents(serde_json::json!([{"prompt": "a"}, {"prompt": "b"}]))
        .expect("array is the schema-correct form");
    assert_eq!(out.len(), 2);
}

#[test]
fn coerce_agents_wraps_single_object() {
    let out = super::coerce_agents(serde_json::json!({"prompt": "solo"}))
        .expect("single object is a one-agent fan-out");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["prompt"], "solo");
}

#[test]
fn coerce_agents_reparses_stringified_array() {
    // The exact wild failure: the model serialized the argument to a string.
    let out = super::coerce_agents(serde_json::Value::String(
        r#"[{"prompt": "x"}]"#.to_string(),
    ))
    .expect("stringified array is reparsed");
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["prompt"], "x");
}

#[test]
fn coerce_agents_reparses_stringified_object() {
    let out = super::coerce_agents(serde_json::Value::String(r#"{"prompt": "y"}"#.to_string()))
        .expect("stringified object is reparsed and wrapped");
    assert_eq!(out.len(), 1);
}

#[test]
fn coerce_agents_rejects_garbage_string_and_scalars() {
    assert!(super::coerce_agents(serde_json::Value::String("not json".to_string())).is_err());
    assert!(super::coerce_agents(serde_json::Value::Null).is_err());
    assert!(super::coerce_agents(serde_json::json!(42)).is_err());
}

#[test]
fn coerce_optional_usize_accepts_wild_model_forms() {
    use serde_json::json;
    assert_eq!(super::coerce_optional_usize(&json!(3)), Ok(Some(3)));
    assert_eq!(super::coerce_optional_usize(&json!("3")), Ok(Some(3)));
    assert_eq!(super::coerce_optional_usize(&json!(" 4 ")), Ok(Some(4)));
    assert_eq!(super::coerce_optional_usize(&json!(3.0)), Ok(Some(3)));
    assert_eq!(super::coerce_optional_usize(&json!("3.0")), Ok(Some(3)));
    assert_eq!(super::coerce_optional_usize(&json!(null)), Ok(None));
    assert_eq!(super::coerce_optional_usize(&json!("")), Ok(None));
    assert_eq!(super::coerce_optional_usize(&json!("  ")), Ok(None));
    assert!(super::coerce_optional_usize(&json!(-1)).is_err());
    assert!(super::coerce_optional_usize(&json!(3.5)).is_err());
    assert!(super::coerce_optional_usize(&json!("3.5")).is_err());
    assert!(super::coerce_optional_usize(&json!("abc")).is_err());
    assert!(super::coerce_optional_usize(&json!(true)).is_err());
    assert!(super::coerce_optional_usize(&json!([3])).is_err());
}

#[test]
fn spawn_multi_agent_input_accepts_stringified_concurrency() {
    // The exact wild failure: `concurrency: "3"` rejected the whole fan-out
    // call with `invalid input: invalid type: string "3", expected usize`.
    let input: super::SpawnMultiAgentInput = serde_json::from_value(serde_json::json!({
        "agents": [{"prompt": "a"}, {"prompt": "b"}],
        "concurrency": "3"
    }))
    .expect("lenient concurrency accepts the stringified form");
    assert_eq!(input.concurrency, Some(3));
    assert_eq!(input.agents.len(), 2);

    // Absent and null still mean unset.
    let unset: super::SpawnMultiAgentInput =
        serde_json::from_value(serde_json::json!({ "agents": [{"prompt": "a"}] }))
            .expect("absent concurrency stays None");
    assert_eq!(unset.concurrency, None);
    let null: super::SpawnMultiAgentInput = serde_json::from_value(serde_json::json!({
        "agents": [{"prompt": "a"}],
        "concurrency": null
    }))
    .expect("null concurrency stays None");
    assert_eq!(null.concurrency, None);
}

#[test]
fn spawn_multi_agent_input_accepts_stringified_agents() {
    let input: super::SpawnMultiAgentInput = serde_json::from_value(serde_json::json!({
        "agents": "[{\"prompt\": \"x\"}]"
    }))
    .expect("lenient deserialize accepts the stringified form");
    assert_eq!(input.agents.len(), 1);
}

#[test]
fn spawn_multi_agent_wait_window_label_is_readable() {
    assert_eq!(
        super::wait_window_label(std::time::Duration::from_secs(20)),
        "20s"
    );
    assert_eq!(
        super::wait_window_label(std::time::Duration::from_secs(300)),
        "5m"
    );
    assert_eq!(
        super::wait_window_label(std::time::Duration::from_secs(20 * 60)),
        "20m"
    );
}

#[test]
fn spawn_multi_agent_collection_window_returns_still_running() {
    let missing_id = format!(
        "missing-agent-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos()
    );
    let start = std::time::Instant::now();
    let result = super::wait_for_spawned_agent_completions(
        std::slice::from_ref(&missing_id),
        std::time::Duration::from_millis(30),
    );
    let elapsed = start.elapsed();

    assert_eq!(result.len(), 1);
    assert_eq!(result[0].agent_id, missing_id);
    assert_eq!(result[0].status, "still_running");
    assert!(
        elapsed < std::time::Duration::from_millis(300),
        "collection should honor the timeout instead of waiting forever: {elapsed:?}"
    );
}

#[test]
#[allow(clippy::too_many_lines)] // one end-to-end timeout-to-delivery invariant
fn blocking_agent_timeout_detaches_and_delivers_completion_once() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let worker = std::sync::Arc::new(std::sync::Mutex::new(None));
    let worker_slot = std::sync::Arc::clone(&worker);
    let cancel_signal = std::sync::Arc::new(std::sync::Mutex::new(None));
    let cancel_slot = std::sync::Arc::clone(&cancel_signal);
    let mut completion_rx = super::register_agent_completion_channel();
    let mut input: super::AgentInput = serde_json::from_value(serde_json::json!({
        "description": "slow timeout agent",
        "prompt": "wait for the test release",
        "background": false,
    }))
    .expect("agent input");
    input.parent_session_id = Some("timeout-session".to_string());

    let manifest = super::agent_tools::execute_agent_with_spawn(input, move |job| {
        *cancel_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(job.cancel_signal.clone());
        let handle = std::thread::spawn(move || {
            release_rx.recv().expect("release slow agent");
            super::agent_tools::persist_agent_terminal_state(
                &job.manifest,
                "completed",
                Some("late result"),
                None,
            )
            .expect("persist completion");
            let completion = super::AgentCompletion {
                agent_id: job.manifest.agent_id.clone(),
                name: job.manifest.name.clone(),
                status: "completed".to_string(),
                result: Some("late result".to_string()),
                structured: None,
                error: None,
                run: HelperRun { output_tokens: 7, ..HelperRun::default() },
            };
            assert!(super::agent_tools::publish_agent_completion_for_tests(
                completion.clone()
            ));
            assert!(
                !super::agent_tools::publish_agent_completion_for_tests(completion),
                "the completion store must publish only one channel event per agent"
            );
        });
        *worker_slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(handle);
        Ok(())
    })
    .expect("spawn slow agent");
    let completion = super::wait_for_agent_completions(
        std::slice::from_ref(&manifest.agent_id),
        std::time::Duration::from_millis(20),
    )
    .into_iter()
    .find(|completion| completion.agent_id == manifest.agent_id);
    let response = super::finish_blocking_agent_call(&manifest, completion)
        .expect("render timeout response");
    let response: serde_json::Value = serde_json::from_str(&response).expect("response json");

    assert_eq!(response["status"], "running");
    assert_eq!(response["background"], true);
    assert!(response["note"]
        .as_str()
        .is_some_and(|note| note.contains("blocking wait timed out") && note.contains("keeps running")));
    assert!(super::is_background_agent(
        response["agentId"].as_str().expect("agent id")
    ));
    assert!(
        !cancel_signal
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .expect("captured cancel signal")
            .is_aborted(),
        "timeout conversion must not cancel the worker"
    );

    release_tx.send(()).expect("release worker");
    worker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
        .expect("worker handle")
        .join()
        .expect("worker completes");
    let event = completion_rx.try_recv().expect("one channel completion");
    assert_eq!(event.agent_id, manifest.agent_id);
    assert!(
        matches!(
            completion_rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "duplicate publication must not enqueue a second completion"
    );
    let delivered = super::drain_background_completions_for_session(&test_registry(), "timeout-session");
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].result.as_deref(), Some("late result"));
    assert!(super::drain_background_completions_for_session(&test_registry(), "timeout-session").is_empty());

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn resolve_option_choice_no_options_returns_trimmed() {
    assert_eq!(resolve_option_choice("  answer  ", &[]), "answer");
}

// --- serve background-completion sweep (drain + fold) ---

fn write_manifest_for_drain(dir: &std::path::Path, id: &str, session: &str, status: &str) {
    let manifest = serde_json::json!({
        "agentId": id,
        "name": id,
        "description": "drain test agent",
        "subagentType": "Explore",
        "model": null,
        "status": status,
        "outputFile": dir.join(format!("{id}.md")),
        "manifestFile": dir.join(format!("{id}.json")),
        "createdAt": "100",
        "parentSessionId": session,
    });
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string(&manifest).expect("manifest json"),
    )
    .expect("write manifest");
}

/// The serve-side sweep must take EXACTLY this session's terminal background
/// agents — clearing their marks — and leave other sessions' agents and
/// still-running agents marked for their own hosts.
#[test]
fn drain_background_completions_sweeps_only_this_sessions_terminal_agents() {
    let _guard = env_lock();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-drain-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("store dir");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let mine = format!("bg-mine-{unique}");
    let other = format!("bg-other-{unique}");
    let running = format!("bg-running-{unique}");
    write_manifest_for_drain(&dir, &mine, "sess-me", "completed");
    write_manifest_for_drain(&dir, &other, "sess-other", "completed");
    write_manifest_for_drain(&dir, &running, "sess-me", "running");
    for (id, result) in [(&mine, "the answer"), (&other, "not yours"), (&running, "partial")] {
        super::mark_background_agent(id.clone());
        super::agent_tools::inject_completion_for_tests(super::AgentCompletion {
            agent_id: id.clone(),
            name: id.clone(),
            status: "completed".to_string(),
            result: Some(result.to_string()),
            structured: None,
            error: None,
            run: HelperRun::default(),
        });
    }

    let drained = super::drain_background_completions_for_session(&test_registry(), "sess-me");

    assert_eq!(drained.len(), 1, "exactly this session's terminal agent");
    assert_eq!(drained[0].agent_id, mine);
    assert_eq!(drained[0].result.as_deref(), Some("the answer"));
    assert!(
        !super::is_background_agent(&mine),
        "a drained completion clears its mark"
    );
    assert!(
        super::is_background_agent(&other),
        "another session's agent stays marked for its own host"
    );
    assert!(
        super::is_background_agent(&running),
        "a still-running manifest is not swept even with a store entry"
    );

    super::clear_background_agent(&other);
    super::clear_background_agent(&running);
    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Closing or replacing one interactive session retires all of ITS delivery
/// claims, including agents which are still running. Their eventual channel
/// events may reach the next session's process-global subscriber, but without
/// the marker they cannot be re-injected into that unrelated conversation.
#[test]
fn closing_a_session_clears_only_its_background_delivery_marks() {
    let _guard = env_lock();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-close-marks-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("store dir");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let mine_running = format!("bg-close-running-{unique}");
    let mine_done = format!("bg-close-done-{unique}");
    let other = format!("bg-close-other-{unique}");
    write_manifest_for_drain(&dir, &mine_running, "sess-closing", "running");
    write_manifest_for_drain(&dir, &mine_done, "sess-closing", "completed");
    write_manifest_for_drain(&dir, &other, "sess-next", "running");
    for id in [&mine_running, &mine_done, &other] {
        super::mark_background_agent(id.clone());
    }

    let cleared = super::clear_background_completion_marks_for_session(&test_registry(), "sess-closing");

    assert_eq!(cleared, 2, "both terminal and live claims are session-owned");
    assert!(!super::is_background_agent(&mine_running));
    assert!(!super::is_background_agent(&mine_done));
    assert!(
        super::is_background_agent(&other),
        "closing one session must not steal another session's delivery claim"
    );

    super::clear_background_agent(&other);
    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn drain_background_task_completion_waits_for_its_session() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let task_id = format!("task-session-drain-{unique}");
    super::notify_background_task_completion(
        task_id.clone(),
        "completed",
        Some("session-a result".to_string()),
        Some("session-a".to_string()),
    );

    assert!(
        super::drain_background_completions_for_session(&test_registry(), "session-b").is_empty(),
        "another session must not consume the completion"
    );
    assert!(
        super::is_background_agent(&task_id),
        "the matching session may still drain the queued completion"
    );

    let drained = super::drain_background_completions_for_session(&test_registry(), "session-a");
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].agent_id, task_id);
    let (body, notice) = runtime::background_log::split_completion_notice(drained[0].result.as_deref().unwrap());
    assert_eq!(body.trim(), "session-a result");
    assert!(notice.unwrap().contains(&task_id));
    assert!(!super::is_background_agent(&drained[0].agent_id));
}

#[test]
fn fold_background_completions_points_at_send_message_and_keeps_input_last() {
    // Empty sweep: the input passes through untouched.
    assert_eq!(
        super::fold_background_completions_into_input(&[], "just the prompt"),
        "just the prompt"
    );

    let completion = |id: &str, status: &str, result: Option<&str>, error: Option<&str>| {
        super::AgentCompletion {
            agent_id: id.to_string(),
            name: format!("name-{id}"),
            status: status.to_string(),
            result: result.map(str::to_string),
            structured: None,
            error: error.map(str::to_string),
            run: HelperRun::default(),
        }
    };
    let folded = super::fold_background_completions_into_input(
        &[
            completion("a1", "completed", Some("found 3 bugs"), None),
            completion("a2", "failed", None, Some("time budget")),
        ],
        "and my next question",
    );
    // Every section names BOTH addresses (name and id) and carries the one
    // continuation hint shared with the interactive re-injection header.
    assert!(folded.contains("`name-a1` (id: a1) finished"), "{folded}");
    assert!(
        folded.contains("use SendMessage with that id/name as `to` to continue this agent"),
        "{folded}"
    );
    assert!(folded.contains("found 3 bugs"), "{folded}");
    assert!(
        folded.contains("`name-a2` (id: a2) failed: time budget"),
        "{folded}"
    );
    assert!(
        folded.ends_with("and my next question"),
        "the user's input stays last: {folded}"
    );

    // A stop with NOTHING to show still produces a full notification section —
    // silence used to leave the model believing the agent was still running.
    let empty = super::fold_background_completions_into_input(
        &[completion("a3", "stopped", None, Some("cancelled by foreground turn"))],
        "next",
    );
    assert!(
        empty.contains("`name-a3` (id: a3) was stopped by user"),
        "{empty}"
    );
    assert!(empty.contains("cancelled by foreground turn"), "{empty}");

    let silent = super::fold_background_completions_into_input(
        &[completion("a4", "completed", None, None)],
        "next",
    );
    assert!(silent.contains("`name-a4` (id: a4) finished"), "{silent}");
    assert!(
        silent.contains(super::AGENT_NOTIFICATION_EMPTY_RESULT),
        "{silent}"
    );
}

/// CC's summary grammar: `finished` / `failed: {err}` / `was stopped by user`
/// / a bare `was stopped`, with `Unknown error` as the honest failure fallback.
#[test]
fn agent_terminal_summary_mirrors_cc_grammar() {
    assert_eq!(super::agent_terminal_summary("completed", None), "finished");
    assert_eq!(
        super::agent_terminal_summary("failed", Some("time budget")),
        "failed: time budget"
    );
    assert_eq!(
        super::agent_terminal_summary("failed", None),
        "failed: Unknown error"
    );
    assert_eq!(
        super::agent_terminal_summary("stopped", Some("cancelled by foreground turn")),
        "was stopped by user"
    );
    assert_eq!(
        super::agent_terminal_summary("stopped", Some("parent session closed")),
        "was stopped: parent session closed"
    );
    assert_eq!(super::agent_terminal_summary("stopped", None), "was stopped");
    // A multi-KB provider dump never lands in a header.
    let long = super::agent_terminal_summary("failed", Some(&"x".repeat(5_000)));
    assert!(long.chars().count() < 400, "summary must stay short: {}", long.len());
}

// --- Named-agent addressing (`SendMessage` `to` resolution) ---

/// Write one agent manifest into `store` with just the fields the lookup reads.
fn write_lookup_manifest(
    store: &std::path::Path,
    agent_id: &str,
    name: &str,
    session: Option<&str>,
    created_at: u64,
) {
    let mut manifest = serde_json::json!({
        "agentId": agent_id,
        "name": name,
        "description": "fixture",
        "status": "completed",
        "outputFile": store.join(format!("{agent_id}.md")).display().to_string(),
        "manifestFile": store.join(format!("{agent_id}.json")).display().to_string(),
        "createdAt": created_at.to_string(),
    });
    if let Some(session) = session {
        manifest["parentSessionId"] = serde_json::json!(session);
    }
    std::fs::write(
        store.join(format!("{agent_id}.json")),
        serde_json::to_string(&manifest).expect("serialize fixture manifest"),
    )
    .expect("write fixture manifest");
}

/// The registry a test reads through: the cwd fallback, which honours the
/// `ZO_AGENT_STORE` pin `with_agent_store` sets — so it lands on the test's
/// own store, exactly as the pre-registry readers did.
fn test_registry() -> std::sync::Arc<super::agent_tools::AgentRegistry> {
    super::agent_tools::AgentRegistry::unowned_from_cwd()
}

fn with_agent_store<T>(store: &std::path::Path, f: impl FnOnce() -> T) -> T {
    let previous = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", store);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    match previous {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    match result {
        Ok(value) => value,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

/// CC's addressing rules, in one fixture: an exact NAME is session-scoped and
/// beats a newer same-named agent from another session; duplicates inside the
/// session resolve latest-wins; an exact id always resolves; and a partial
/// string still falls back to the legacy prefix/substring match.
#[test]
fn send_message_resolves_names_before_prefixes_and_scopes_to_the_session() {
    let _lock = env_lock();
    let store = temp_dir();
    std::fs::create_dir_all(&store).expect("create store");

    write_lookup_manifest(&store, "agent-10", "scout", Some("session-a"), 100);
    write_lookup_manifest(&store, "agent-20", "scout", Some("session-a"), 300);
    // NEWER, but it belongs to a different session — it must never win an
    // exact-name send issued from session-a.
    write_lookup_manifest(&store, "agent-30", "scout", Some("session-b"), 900);
    write_lookup_manifest(&store, "agent-40", "scoutmaster", Some("session-a"), 200);

    with_agent_store(&store, || {
        // Exact name, session-scoped, latest of the two in-session duplicates.
        let hit = super::lookup_agent_manifest(&test_registry(), "scout", Some("session-a"))
            .expect("exact name resolves");
        assert_eq!(hit.agent_id, "agent-20");

        // The other session addresses its own worker by the same name.
        let hit = super::lookup_agent_manifest(&test_registry(), "scout", Some("session-b"))
            .expect("exact name resolves");
        assert_eq!(hit.agent_id, "agent-30");

        // No addressing session (viewer send / legacy host): plain latest-wins.
        let hit =
            super::lookup_agent_manifest(&test_registry(), "scout", None).expect("exact name resolves");
        assert_eq!(hit.agent_id, "agent-30");

        // An exact id beats every name match, even from another session.
        let hit = super::lookup_agent_manifest(&test_registry(), "agent-10", Some("session-a"))
            .expect("exact id resolves");
        assert_eq!(hit.agent_id, "agent-10");

        // Partial → prefix/substring fallback, still session-first.
        let hit = super::lookup_agent_manifest(&test_registry(), "scoutm", Some("session-a"))
            .expect("prefix resolves");
        assert_eq!(hit.agent_id, "agent-40");

        assert!(super::lookup_agent_manifest(&test_registry(), "nobody", Some("session-a")).is_none());
    });

    let _ = std::fs::remove_dir_all(&store);
}

// --- Flat SpawnMultiAgent live-scheduler (rolling window, no chunk barrier) ---

/// A slow first agent must not delay the *start* of its siblings while a slot is
/// free: with a window of 2 over 4 agents, agents 0 and 1 both spawn before any
/// wait, and once agent 0 finishes agent 2 starts into the freed slot *while
/// agent 1 is still in flight*. The old `chunks(window)` barrier drained the
/// whole window (both 0 and 1) before starting the next chunk, so under it agent
/// 1 would no longer be in flight when agent 2 starts — this test fails against
/// that barrier and passes for the rolling window.
#[test]
fn fanout_schedule_refills_freed_slot_without_head_of_line_block() {
    let mut waits_before_second_slot = 0usize;
    // Snapshot of the live set observed at the moment each agent is spawned.
    let mut in_flight_at_spawn: Vec<Vec<String>> = Vec::new();
    let order = super::drive_fanout_schedule(
        4,
        2,
        |idx, in_flight| {
            in_flight_at_spawn.push(in_flight.to_vec());
            format!("a{idx}")
        },
        |in_flight| {
            waits_before_second_slot += 1;
            // Reclaim exactly one slot (the oldest), like a single completion.
            in_flight.remove(0);
        },
    );

    assert_eq!(order, ["a0", "a1", "a2", "a3"], "spawn order is input order");
    // Agents 0 and 1 start back-to-back with no wait between them.
    assert!(in_flight_at_spawn[0].is_empty());
    assert_eq!(in_flight_at_spawn[1], ["a0"]);
    // Agent 2 starts after a single reclaim, and agent 1 is STILL in flight — the
    // freed slot was refilled without draining the whole window.
    assert_eq!(
        in_flight_at_spawn[2],
        ["a1"],
        "the freed slot is refilled while the slow sibling is still running"
    );
    assert_eq!(in_flight_at_spawn[3], ["a2"]);
    assert_eq!(waits_before_second_slot, 2, "one reclaim per over-window spawn");
}

/// The rolling window never holds more than `window` agents in flight, and it
/// reclaims exactly `agent_count - window` times (one per agent that cannot fit
/// in the initial window).
#[test]
fn fanout_schedule_never_exceeds_window() {
    let window = 3usize;
    let agent_count = 8usize;
    let mut max_in_flight = 0usize;
    let mut reclaims = 0usize;
    super::drive_fanout_schedule(
        agent_count,
        window,
        |idx, in_flight| {
            assert!(
                in_flight.len() < window,
                "a slot must be free before spawning: {} live, window {window}",
                in_flight.len()
            );
            max_in_flight = max_in_flight.max(in_flight.len() + 1);
            format!("a{idx}")
        },
        |in_flight| {
            reclaims += 1;
            in_flight.remove(0);
        },
    );
    assert_eq!(max_in_flight, window, "concurrency is capped at the window");
    assert_eq!(reclaims, agent_count - window);
}

/// `reclaim_spawn_slots` returns as soon as *one* in-flight agent is terminal,
/// draining only that completion and leaving the unfinished sibling in flight —
/// it does not block on the whole set the way a barrier drain would.
#[test]
fn reclaim_spawn_slots_frees_one_and_keeps_the_unfinished() {
    // Registering the process-global completion channel REPLACES the installed
    // sender, so a sibling test holding an older receiver sees `Disconnected`.
    // Every other channel-registering test takes the crate-wide lock for
    // exactly this reason; this one was missing it.
    let _guard = env_lock();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    // Capture the published completion on a local channel so it does not leak
    // into the process-global receiver another test may read.
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let done_id = format!("reclaim-done-{stamp}");
    let slow_id = format!("reclaim-slow-{stamp}");

    // Only the first agent has published a terminal completion.
    assert!(super::agent_tools::publish_agent_completion_for_tests(
        super::AgentCompletion {
            agent_id: done_id.clone(),
            name: "done".to_string(),
            status: "completed".to_string(),
            result: Some("ok".to_string()),
            structured: None,
            error: None,
            run: HelperRun { output_tokens: 1, ..HelperRun::default() },
        }
    ));

    let mut in_flight = vec![done_id.clone(), slow_id.clone()];
    let mut completions = Vec::new();
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut extensions = 0u32;
    let start = std::time::Instant::now();
    super::reclaim_spawn_slots(&test_registry(), &mut in_flight, &mut completions, &mut deadline, &mut extensions);
    let elapsed = start.elapsed();

    assert_eq!(in_flight, [slow_id], "the unfinished sibling stays in flight");
    assert_eq!(completions.len(), 1, "only the terminal agent is drained");
    assert_eq!(completions[0].agent_id, done_id);
    assert!(
        elapsed < std::time::Duration::from_secs(4),
        "reclaim returns on the first completion, not after the whole timeout: {elapsed:?}"
    );
}

/// Read a persisted agent manifest's `status` field back off disk. Used by the
/// scheduler timeout-path tests to prove an agent was actually driven to a
/// terminal state (not merely dropped from the in-flight vector while live).
fn manifest_status_on_disk(dir: &std::path::Path, id: &str) -> String {
    let raw = std::fs::read_to_string(dir.join(format!("{id}.json")))
        .unwrap_or_else(|e| panic!("read manifest for {id}: {e}"));
    let value: serde_json::Value =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse manifest for {id}: {e}"));
    value
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or_default()
        .to_string()
}

/// When the shared deadline elapses with nothing terminal, `reclaim_spawn_slots`
/// must **cancel and salvage the oldest agent to a terminal state before freeing
/// its slot** — not merely drop its id from `in_flight` while the worker is still
/// live. This exercises the real scheduler timeout path (against a real agent
/// store), not the pure `drive_fanout_schedule` happy path: the oldest agent's
/// on-disk manifest is `running` before the call and must be terminal after, so
/// its worktree is safe to collect and live workers never exceed the window.
#[test]
fn reclaim_timeout_cancels_oldest_to_terminal_before_freeing_slot() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    // Capture the completions our salvage emits on a local channel so they do
    // not leak into the process-global receiver another test may read.
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let oldest = format!("reclaim-oldest-{stamp}");
    let younger = format!("reclaim-younger-{stamp}");
    // Both agents are LIVE (no terminal record); neither will finish on its own.
    // The salvage path appends to the agent output file, so it must pre-exist.
    write_manifest_for_drain(&dir, &oldest, "sched-sess", "running");
    write_manifest_for_drain(&dir, &younger, "sched-sess", "running");
    std::fs::write(dir.join(format!("{oldest}.md")), "").expect("oldest output file");
    std::fs::write(dir.join(format!("{younger}.md")), "").expect("younger output file");

    let mut in_flight = vec![oldest.clone(), younger.clone()];
    let mut completions = Vec::new();
    // No manifest heartbeat was ever stamped (`lastActivityAt` absent), so the
    // progress gate treats both agents as stale and never extends the deadline.
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_millis(20);
    let mut extensions = 0u32;
    let start = std::time::Instant::now();
    // Deadline already at/near now, so the timeout branch fires deterministically.
    super::reclaim_spawn_slots(&test_registry(), &mut in_flight, &mut completions, &mut deadline, &mut extensions);
    let elapsed = start.elapsed();

    assert_eq!(
        in_flight,
        std::slice::from_ref(&younger),
        "exactly the oldest slot is reclaimed; the younger agent stays in flight"
    );
    assert_eq!(
        manifest_status_on_disk(&dir, &oldest),
        "stopped",
        "the reclaimed agent is driven to a terminal state before its slot frees"
    );
    assert_eq!(
        manifest_status_on_disk(&dir, &younger),
        "running",
        "the agent that keeps its slot is left untouched"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "reclaim honors the shared deadline instead of blocking: {elapsed:?}"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The final drain must bring **every** still-in-flight agent to a
/// terminal/cancelled state before the caller collects or drops any worktree.
/// With no agent finishing on its own and an already-elapsed deadline,
/// `drain_or_cancel_remaining_agents` cancels+salvages all survivors: `in_flight`
/// is emptied and every manifest is terminal on disk, so the subsequent
/// per-worktree `collect_patch`/`drop` cannot race a live editor.
#[test]
fn final_drain_cancels_all_live_agents_before_worktree_collect() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let first = format!("drain-first-{stamp}");
    let second = format!("drain-second-{stamp}");
    write_manifest_for_drain(&dir, &first, "sched-sess", "running");
    write_manifest_for_drain(&dir, &second, "sched-sess", "running");
    std::fs::write(dir.join(format!("{first}.md")), "").expect("first output file");
    std::fs::write(dir.join(format!("{second}.md")), "").expect("second output file");

    let mut in_flight = vec![first.clone(), second.clone()];
    let mut completions = Vec::new();
    // No heartbeat stamped → stale → the drain cancels instead of extending.
    let mut deadline = std::time::Instant::now();
    let mut extensions = 0u32;
    let start = std::time::Instant::now();
    super::drain_or_cancel_remaining_agents(
        &test_registry(),
        &mut in_flight,
        &mut completions,
        &mut deadline,
        &mut extensions,
    );
    let elapsed = start.elapsed();

    assert!(
        in_flight.is_empty(),
        "no agent stays live once the drain returns: {in_flight:?}"
    );
    assert_eq!(
        manifest_status_on_disk(&dir, &first),
        "stopped",
        "the first agent is terminal before any worktree collect/drop"
    );
    assert_eq!(
        manifest_status_on_disk(&dir, &second),
        "stopped",
        "the second agent is terminal before any worktree collect/drop"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the drain honors the shared deadline rather than blocking: {elapsed:?}"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A single shared deadline must NOT be re-armed per reclaim. Reclaiming a whole
/// window of live agents one slot at a time against one already-elapsed deadline
/// stays within a single collection budget: the total wall time is a small
/// multiple of the poll slice, not `agents * wait_timeout`. This is the
/// regression guard for the cumulative-timeout defect.
#[test]
fn shared_deadline_is_not_rearmed_per_reclaim() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let ids: Vec<String> = (0..5).map(|i| format!("budget-{stamp}-{i}")).collect();
    for id in &ids {
        write_manifest_for_drain(&dir, id, "sched-sess", "running");
        std::fs::write(dir.join(format!("{id}.md")), "").expect("output file");
    }

    let mut in_flight = ids.clone();
    let mut completions = Vec::new();
    // One deadline shared by every reclaim, already elapsed. No heartbeat is
    // stamped on any manifest, so the progress gate never re-arms it either.
    let mut deadline = std::time::Instant::now();
    let mut extensions = 0u32;
    let start = std::time::Instant::now();
    while !in_flight.is_empty() {
        super::reclaim_spawn_slots(&test_registry(), &mut in_flight, &mut completions, &mut deadline, &mut extensions);
    }
    let elapsed = start.elapsed();

    // A per-reclaim re-armed timeout would make this grow with `ids.len()`.
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "the shared deadline caps total collection time regardless of agent count: {elapsed:?}"
    );
    for id in &ids {
        assert_eq!(
            manifest_status_on_disk(&dir, id),
            "stopped",
            "every reclaimed agent is driven terminal"
        );
    }

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// What counts as evidence that an agent is still working: a heartbeat inside
/// the activity window, or being blocked in a harness call it cannot stamp from
/// — a tool call (stamped on entry and exit only, so a multi-minute build reads
/// as idle by heartbeat alone) or an open provider request (a silently thinking
/// turn streams no frame at all). Both are the exact shape that would otherwise
/// kill a healthy agent.
#[test]
fn progress_snapshot_covers_heartbeat_and_in_tool_work() {
    let window_secs = super::SPAWN_RECLAIM_ACTIVITY_WINDOW.as_secs();
    assert!(super::snapshot_shows_progress(Some(0), false));
    assert!(
        super::snapshot_shows_progress(Some(window_secs), false),
        "the window boundary is inclusive"
    );
    assert!(
        !super::snapshot_shows_progress(Some(window_secs + 1), false),
        "a heartbeat past the window with nothing in flight is stale"
    );
    assert!(
        !super::snapshot_shows_progress(None, false),
        "never stamped and nothing in flight is no evidence at all"
    );
    assert!(
        super::snapshot_shows_progress(Some(window_secs * 10), true),
        "an agent blocked in a tool call or a provider request is working even \
         with a stale stamp"
    );
    assert!(
        super::snapshot_shows_progress(None, true),
        "a live harness call alone is sufficient evidence"
    );
}

/// The progress gate that decides between extending the fan-out deadline and
/// cancelling: pure, so every branch is exercised without waiting out a real
/// deadline. A candidate is spared only while it is progressing AND extensions
/// remain, so the total collection budget stays bounded.
#[test]
fn deadline_extension_gate_is_progress_and_cap_bounded() {
    assert!(
        super::reclaim_should_extend_deadline(true, 0),
        "a progressing candidate with extensions remaining earns an extension"
    );
    assert!(
        super::reclaim_should_extend_deadline(true, super::SPAWN_DEADLINE_MAX_EXTENSIONS - 1),
        "the last extension is grantable"
    );
    assert!(
        !super::reclaim_should_extend_deadline(false, 0),
        "a stalled candidate is reclaimed even with extensions to spare"
    );
    assert!(
        !super::reclaim_should_extend_deadline(true, super::SPAWN_DEADLINE_MAX_EXTENSIONS),
        "once the cap is spent even a busy agent is reclaimed — the budget stays bounded"
    );
}

/// Patch a persisted manifest's `lastActivityAt` heartbeat, mirroring what the
/// production `record_agent_*` frame updates do.
fn stamp_manifest_activity(dir: &std::path::Path, id: &str, stamped_at: u64) {
    let path = dir.join(format!("{id}.json"));
    let raw = std::fs::read_to_string(&path).expect("read manifest for stamp");
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).expect("parse manifest for stamp");
    value["lastActivityAt"] = serde_json::json!(stamped_at);
    std::fs::write(&path, serde_json::to_string(&value).expect("manifest json"))
        .expect("write stamped manifest");
}

/// Mark a persisted manifest as blocked inside a tool call, as
/// `record_current_tool` does in production.
fn set_manifest_current_tool(dir: &std::path::Path, id: &str, tool: &str) {
    let path = dir.join(format!("{id}.json"));
    let raw = std::fs::read_to_string(&path).expect("read manifest for tool stamp");
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).expect("parse manifest for tool stamp");
    value["currentTool"] = serde_json::json!(tool);
    std::fs::write(&path, serde_json::to_string(&value).expect("manifest json"))
        .expect("write manifest with current tool");
}

/// Mark a persisted manifest as blocked on an open provider request, as
/// `record_agent_stream_open` does in production.
fn set_manifest_awaiting_provider(dir: &std::path::Path, id: &str, since: u64) {
    let path = dir.join(format!("{id}.json"));
    let raw = std::fs::read_to_string(&path).expect("read manifest for provider stamp");
    let mut value: serde_json::Value =
        serde_json::from_str(&raw).expect("parse manifest for provider stamp");
    value["awaitingProviderSince"] = serde_json::json!(since);
    std::fs::write(&path, serde_json::to_string(&value).expect("manifest json"))
        .expect("write manifest awaiting a provider");
}

/// Epoch seconds for heartbeat fixtures.
fn epoch_secs_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_secs()
}

/// The manifest readers behind the progress gate, against a real store: a
/// fresh stamp reads as progress, a stale one does not, a never-stamped
/// manifest does not — an agent whose manifest says it is inside a tool call
/// does even with an ancient stamp, and the drain's set-level reader answers
/// yes when any single member is working.
#[test]
fn heartbeat_age_readers_reflect_the_persisted_stamp() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let fresh = format!("hb-fresh-{stamp}");
    let stale = format!("hb-stale-{stamp}");
    let never = format!("hb-never-{stamp}");
    let in_tool = format!("hb-in-tool-{stamp}");
    for id in [&fresh, &stale, &never, &in_tool] {
        write_manifest_for_drain(&dir, id, "hb-sess", "running");
    }
    stamp_manifest_activity(&dir, &fresh, epoch_secs_now());
    stamp_manifest_activity(&dir, &stale, epoch_secs_now().saturating_sub(10_000));
    // A long build: stamped when the tool started, silent ever since, worker
    // still registered.
    stamp_manifest_activity(&dir, &in_tool, epoch_secs_now().saturating_sub(10_000));
    set_manifest_current_tool(&dir, &in_tool, "bash");
    super::agent_tools::register_agent_cancel_signal_for_tests(
        &in_tool,
        super::agent_tools::AGENT_INITIAL_RUN_GENERATION,
    );

    let snapshot = super::agent_tools::agent_progress_snapshot(&test_registry(), &fresh)
        .expect("fresh agent has a manifest");
    assert!(
        snapshot.seconds_since_activity.is_some_and(|age| age <= 5),
        "a just-stamped heartbeat reads near zero: {:?}",
        snapshot.seconds_since_activity
    );
    assert!(super::agent_shows_progress(&test_registry(), &fresh));
    assert!(!super::agent_shows_progress(&test_registry(), &stale));
    assert!(!super::agent_shows_progress(&test_registry(), &never));
    assert!(
        super::agent_shows_progress(&test_registry(), &in_tool),
        "an agent blocked in a tool call is working despite the old stamp"
    );
    assert!(
        super::any_agent_shows_progress(&test_registry(), &[stale.clone(), never.clone(), fresh.clone()]),
        "the set-level reader answers yes when any member is working"
    );
    assert!(
        !super::any_agent_shows_progress(&test_registry(), &[stale.clone(), never.clone()]),
        "a set with no working member is reclaimable"
    );

    // The corpse: same manifest, worker gone. `current_tool` is cleared by the
    // worker itself, so a worker that died before its terminal transition
    // leaves the claim standing forever — and taken at face value it wins every
    // extension and holds the whole set's collection for the full budget.
    super::agent_tools::unregister_agent_cancel_signal_for_tests(
        &in_tool,
        super::agent_tools::AGENT_INITIAL_RUN_GENERATION,
    );
    assert!(
        !super::agent_shows_progress(&test_registry(), &in_tool),
        "a dead worker's stale tool claim is not evidence of progress"
    );
    assert!(
        !super::any_agent_shows_progress(&test_registry(), &[in_tool.clone(), stale.clone()]),
        "the set-level reader must not follow a corpse either"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The defect no frame-driven signal could see: a provider that accepts a
/// request and then thinks silently streams nothing for minutes, so no
/// heartbeat is stamped and no tool is in flight — a healthy agent mid-API-call
/// read as hung and was cancelled by the fan-out deadline. The manifest's
/// request bracket is the harness's own record that the call is open, and like
/// every other in-flight claim it is worth nothing once the worker is gone.
#[test]
fn an_agent_awaiting_a_provider_is_progress_only_while_its_worker_lives() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let thinking = format!("hb-thinking-{stamp}");
    write_manifest_for_drain(&dir, &thinking, "hb-sess", "running");
    // The request went out and nothing has come back: the newest stamp of any
    // kind is the request itself, far outside the reclaim window.
    let issued_at = epoch_secs_now().saturating_sub(10_000);
    stamp_manifest_activity(&dir, &thinking, issued_at);
    set_manifest_awaiting_provider(&dir, &thinking, issued_at);
    super::agent_tools::register_agent_cancel_signal_for_tests(
        &thinking,
        super::agent_tools::AGENT_INITIAL_RUN_GENERATION,
    );

    let snapshot = super::agent_tools::agent_progress_snapshot(&test_registry(), &thinking)
        .expect("the waiting agent has a manifest");
    assert!(
        snapshot.awaiting_provider,
        "the open request has to reach the scheduler's snapshot"
    );
    assert!(
        !snapshot.inside_tool_call,
        "a provider request is not a tool call — the two signals stay distinct"
    );
    assert!(
        super::agent_shows_progress(&test_registry(), &thinking),
        "an agent blocked in a provider call is working despite the stale heartbeat"
    );
    assert!(
        super::any_agent_shows_progress(&test_registry(), std::slice::from_ref(&thinking)),
        "the set-level reader sees it too"
    );

    super::agent_tools::unregister_agent_cancel_signal_for_tests(
        &thinking,
        super::agent_tools::AGENT_INITIAL_RUN_GENERATION,
    );
    assert!(
        !super::agent_shows_progress(&test_registry(), &thinking),
        "a worker that died mid-request leaves the claim standing forever; that \
         corpse must not win extensions"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The screenshot defect: the final drain used to cancel a healthy agent that
/// was 20+ minutes into real work the instant the fixed deadline elapsed. Now
/// a fresh heartbeat earns a bounded extension and the agent's own terminal
/// completion is collected instead of a synthetic `stopped`: the deliverable
/// survives, the extension counter records the grant, and the manifest is
/// never driven terminal by the scheduler.
#[test]
fn final_drain_extends_for_a_mid_work_agent_instead_of_cancelling() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let busy = format!("drain-busy-{stamp}");
    write_manifest_for_drain(&dir, &busy, "sched-sess", "running");
    std::fs::write(dir.join(format!("{busy}.md")), "").expect("busy output file");
    stamp_manifest_activity(&dir, &busy, epoch_secs_now());

    // The agent finishes on its own shortly after the (already-elapsed)
    // deadline — inside the extension the gate grants for its fresh heartbeat.
    let publish_id = busy.clone();
    let publisher = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(super::agent_tools::publish_agent_completion_for_tests(
            super::AgentCompletion {
                agent_id: publish_id.clone(),
                name: "busy".to_string(),
                status: "completed".to_string(),
                result: Some("real deliverable".to_string()),
                structured: None,
                error: None,
                run: HelperRun { output_tokens: 1, ..HelperRun::default() },
            }
        ));
    });

    let mut in_flight = vec![busy.clone()];
    let mut completions = Vec::new();
    let mut deadline = std::time::Instant::now();
    let mut extensions = 0u32;
    let start = std::time::Instant::now();
    super::drain_or_cancel_remaining_agents(
        &test_registry(),
        &mut in_flight,
        &mut completions,
        &mut deadline,
        &mut extensions,
    );
    let elapsed = start.elapsed();
    publisher.join().expect("publisher thread");

    assert!(in_flight.is_empty(), "the drain still empties the in-flight set");
    assert_eq!(extensions, 1, "exactly one progress extension was granted");
    assert_eq!(completions.len(), 1);
    assert_eq!(
        completions[0].status, "completed",
        "the agent's own terminal completion is collected, not a synthetic stop"
    );
    assert_eq!(completions[0].result.as_deref(), Some("real deliverable"));
    assert_eq!(
        manifest_status_on_disk(&dir, &busy),
        "running",
        "the scheduler never drove the mid-work agent terminal"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the extension wait returns on the completion, not after the whole step: {elapsed:?}"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Once the extension cap is spent, even a demonstrably busy agent is
/// reclaimed — the collection budget must stay bounded. Same fixture as the
/// rescue test, but with the counter already at the cap: the drain cancels
/// immediately instead of waiting another step.
#[test]
fn final_drain_reclaims_a_busy_agent_once_the_extension_cap_is_spent() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let busy = format!("drain-capped-{stamp}");
    write_manifest_for_drain(&dir, &busy, "sched-sess", "running");
    std::fs::write(dir.join(format!("{busy}.md")), "").expect("busy output file");
    stamp_manifest_activity(&dir, &busy, epoch_secs_now());

    let mut in_flight = vec![busy.clone()];
    let mut completions = Vec::new();
    let mut deadline = std::time::Instant::now();
    let mut extensions = super::SPAWN_DEADLINE_MAX_EXTENSIONS;
    let start = std::time::Instant::now();
    super::drain_or_cancel_remaining_agents(
        &test_registry(),
        &mut in_flight,
        &mut completions,
        &mut deadline,
        &mut extensions,
    );
    let elapsed = start.elapsed();

    assert!(in_flight.is_empty());
    assert_eq!(
        extensions,
        super::SPAWN_DEADLINE_MAX_EXTENSIONS,
        "no further extension is granted past the cap"
    );
    assert_eq!(
        manifest_status_on_disk(&dir, &busy),
        "stopped",
        "past the cap the busy agent is driven terminal as before"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "a capped drain cancels promptly instead of waiting another step: {elapsed:?}"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Mid-loop mirror of the rescue: when the deadline elapses but the OLDEST
/// in-flight agent (the cancellation candidate) is still mid-work, the reclaim
/// extends and keeps waiting — the slot is then freed by a sibling's real
/// completion, and the busy oldest keeps running untouched.
#[test]
fn reclaim_extension_keeps_the_busy_oldest_until_a_sibling_frees_the_slot() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let oldest = format!("reclaim-busy-oldest-{stamp}");
    let sibling = format!("reclaim-sibling-{stamp}");
    write_manifest_for_drain(&dir, &oldest, "sched-sess", "running");
    write_manifest_for_drain(&dir, &sibling, "sched-sess", "running");
    std::fs::write(dir.join(format!("{oldest}.md")), "").expect("oldest output file");
    std::fs::write(dir.join(format!("{sibling}.md")), "").expect("sibling output file");
    stamp_manifest_activity(&dir, &oldest, epoch_secs_now());

    let publish_id = sibling.clone();
    // Publish only once the reclaim has RECORDED its extension. A fixed
    // 150 ms sleep here bet against the scheduler and lost on the loaded
    // release lane (2026-09-14): the main thread was starved past the
    // publish, the completion was already in the store when the first poll
    // ran, and the reclaim returned without ever reaching its deadline —
    // `extensions` read 0. Gating on the seam makes the order a fact. A
    // reclaim that never extends is still caught below, by the assertion,
    // after a bounded wait — never by a hang.
    let granted_before =
        super::RECLAIM_EXTENSIONS_GRANTED.load(std::sync::atomic::Ordering::SeqCst);
    let publisher = std::thread::spawn(move || {
        let patience = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while super::RECLAIM_EXTENSIONS_GRANTED.load(std::sync::atomic::Ordering::SeqCst)
            == granted_before
            && std::time::Instant::now() < patience
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(super::agent_tools::publish_agent_completion_for_tests(
            super::AgentCompletion {
                agent_id: publish_id.clone(),
                name: "sibling".to_string(),
                status: "completed".to_string(),
                result: Some("sibling done".to_string()),
                structured: None,
                error: None,
                run: HelperRun { output_tokens: 1, ..HelperRun::default() },
            }
        ));
    });

    let mut in_flight = vec![oldest.clone(), sibling.clone()];
    let mut completions = Vec::new();
    let mut deadline = std::time::Instant::now() + std::time::Duration::from_millis(20);
    let mut extensions = 0u32;
    let start = std::time::Instant::now();
    super::reclaim_spawn_slots(&test_registry(), &mut in_flight, &mut completions, &mut deadline, &mut extensions);
    let elapsed = start.elapsed();
    publisher.join().expect("publisher thread");

    assert_eq!(
        in_flight,
        std::slice::from_ref(&oldest),
        "the busy oldest keeps its slot; the finished sibling freed one"
    );
    assert_eq!(extensions, 1, "the grant is recorded against the shared cap");
    assert_eq!(completions.len(), 1);
    assert_eq!(completions[0].agent_id, sibling);
    assert_eq!(
        manifest_status_on_disk(&dir, &oldest),
        "running",
        "the mid-work candidate was never cancelled"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "the reclaim returns on the sibling's completion: {elapsed:?}"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A `WorktreeGuard` that records the order in which guards are dropped, so a test
/// can prove teardown happens strictly after the physical worker exits. `Send`
/// (only `PathBuf` + `String` + `Arc<Mutex<_>>`) so it can be moved to the
/// background cleanup owner thread.
struct DropRecordingGuard {
    id: String,
    path: std::path::PathBuf,
    dropped: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl crate::workflow_tools::worktree::WorktreeGuard for DropRecordingGuard {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
    // `collect_patch` defaults to a clean tree (`Ok(None)`), which is what this
    // drop-ordering test wants — no merge-back, just teardown timing.
}

impl Drop for DropRecordingGuard {
    fn drop(&mut self) {
        self.dropped
            .lock()
            .expect("drop-order lock")
            .push(self.id.clone());
    }
}

/// Cooperative cancel writes a terminal `stopped` manifest immediately, but the
/// physical worker keeps running until it observes the abort. The fan-out
/// reclaim path must not mistake that **synthetic** terminal for a joined worker:
/// `cancel_and_salvage_agent_keep_worker_registered` must persist the terminal
/// manifest yet keep the worker's exact generation live until it actually exits
/// (i.e. its cancel signal is unregistered). This is the discriminator the
/// scheduler uses to gate slot reuse and worktree teardown on the real
/// physical-exit ack instead of the manifest terminal.
#[test]
fn keep_registered_salvage_persists_terminal_but_preserves_worker_liveness() {
    let _guard = env_lock();
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).expect("create agent store");
    let prior_store = std::env::var_os("ZO_AGENT_STORE");
    std::env::set_var("ZO_AGENT_STORE", &dir);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let _completion_rx = super::agent_tools::register_agent_completion_channel();
    let id = format!("keepreg-{stamp}");
    let generation = super::agent_tools::AGENT_INITIAL_RUN_GENERATION;
    write_manifest_for_drain(&dir, &id, "sched-sess", "running");
    std::fs::write(dir.join(format!("{id}.md")), "").expect("output file");

    // The worker is physically live: a cancel signal is registered for it.
    super::agent_tools::register_agent_cancel_signal_for_tests(&id, generation);
    assert!(
        super::agent_tools::agent_worker_generation_is_live(&id, generation),
        "worker generation is live once its cancel signal is registered"
    );

    let _ = super::agent_tools::cancel_and_salvage_agent_keep_worker_registered(
        &test_registry(),
        &id,
        "fan-out reclaimed this slot before the agent reached a terminal state",
    );

    // The synthetic terminal is on disk...
    assert_eq!(
        manifest_status_on_disk(&dir, &id),
        "stopped",
        "cooperative cancel persists a terminal manifest immediately"
    );
    // ...but it must NOT be mistaken for a physical join: the worker is still
    // live, so the scheduler will not reuse its slot or collect its worktree yet.
    assert!(
        super::agent_tools::agent_worker_generation_is_live(&id, generation),
        "the synthetic terminal must not zo a physical-exit ack"
    );

    // Now the worker actually exits (unregisters its own cancel signal): only
    // then does liveness flip false.
    super::agent_tools::unregister_agent_cancel_signal_for_tests(&id, generation);
    assert!(
        !super::agent_tools::agent_worker_generation_is_live(&id, generation),
        "liveness flips false only on the real physical-exit ack"
    );

    match prior_store {
        Some(value) => std::env::set_var("ZO_AGENT_STORE", value),
        None => std::env::remove_var("ZO_AGENT_STORE"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The background worktree-cleanup owner must drop a still-live worker's guard
/// **only after that exact worker generation physically exits**, never while it is
/// still writing its worktree — and there is deliberately **no cap** that would
/// tear a live worktree down. With a real (delayed) cooperative-cancel worker
/// simulated by a registered cancel signal, the guard stays undropped across a
/// span far longer than any old cap while the generation is live, and is dropped
/// promptly once the signal is unregistered. Bounded by an explicit 1s join
/// timeout so a regression fails loudly instead of hanging.
#[test]
fn deferred_worktree_cleanup_never_drops_before_exact_generation_exit() {
    let _guard = env_lock();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let id = format!("defer-{stamp}");
    let generation = super::agent_tools::AGENT_INITIAL_RUN_GENERATION;

    let dropped = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let guard = DropRecordingGuard {
        id: id.clone(),
        path: temp_dir(),
        dropped: std::sync::Arc::clone(&dropped),
    };

    // Worker is live: cooperative cancel has fired but the thread has not exited.
    super::agent_tools::register_agent_cancel_signal_for_tests(&id, generation);

    // Hand the still-live worker's guard to the background owner (fast poll).
    let handle = super::spawn_deferred_worktree_cleanup(
        id.clone(),
        generation,
        Box::new(guard),
        std::time::Duration::from_millis(5),
    )
    .expect("cleanup owner thread spawns");

    // While the generation is live the guard must NOT be dropped — held well past
    // the length of the removed 30s cap's intent, proving teardown is gated on the
    // exit ack, not elapsed time.
    std::thread::sleep(std::time::Duration::from_millis(120));
    assert!(
        !handle.is_finished(),
        "cleanup owner must keep owning the guard while the worker is live"
    );
    assert!(
        dropped.lock().expect("drop lock").is_empty(),
        "worktree guard must not be torn down while the worker is still live"
    );

    // The worker physically exits.
    super::agent_tools::unregister_agent_cancel_signal_for_tests(&id, generation);

    // The owner must observe the exit ack and drop the guard promptly. Join with
    // an explicit deadline so a stuck owner fails the test instead of hanging.
    let joined = std::time::Instant::now();
    while !handle.is_finished() {
        assert!(
            joined.elapsed() < std::time::Duration::from_secs(1),
            "cleanup owner must finish within 1s of the physical-exit ack"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    handle.join().expect("cleanup owner joins cleanly");

    assert_eq!(
        dropped.lock().expect("drop lock").as_slice(),
        std::slice::from_ref(&id),
        "the guard is dropped exactly once, strictly after the physical-exit ack"
    );
}

/// Generation ABA: the cancel-signal registry keys one entry per agent id, so a
/// same-id resume (a `SendMessage` steering restart) registers a *newer*
/// generation. The cancel-signal registry keys each generation separately, so the
/// old generation and the new generation coexist — a resume does NOT overwrite or
/// evict the old worker's signal. This is the safety the ABA fix guarantees: a
/// deferred cleanup owner bound to the old generation's worktree keeps owning it
/// while the old physical worker is still live (old exact-query stays true even
/// after the new generation registers), and drops it ONLY when the old generation
/// itself unregisters at physical exit — never merely because a new generation
/// took the id. The new generation is independently live until it too exits.
#[test]
fn deferred_cleanup_generation_binding_survives_same_id_resume() {
    let _guard = env_lock();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let id = format!("aba-{stamp}");
    let old_gen = super::agent_tools::AGENT_INITIAL_RUN_GENERATION;
    let new_gen = old_gen + 1;

    // Old generation worker is live and owns the old worktree.
    super::agent_tools::register_agent_cancel_signal_for_tests(&id, old_gen);
    let dropped = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let old_guard = DropRecordingGuard {
        id: id.clone(),
        path: temp_dir(),
        dropped: std::sync::Arc::clone(&dropped),
    };
    let handle = super::spawn_deferred_worktree_cleanup(
        id.clone(),
        old_gen,
        Box::new(old_guard),
        std::time::Duration::from_millis(5),
    )
    .expect("cleanup owner thread spawns");

    // A same-id resume registers the NEW generation. Both generations now coexist
    // in the registry: the any-generation HUD query is live, and BOTH exact
    // generation queries are true. Crucially the OLD generation stays live — the
    // resume did not evict the still-running old worker.
    super::agent_tools::register_agent_cancel_signal_for_tests(&id, new_gen);
    assert!(
        super::agent_tools::agent_worker_is_live(&id),
        "any-generation liveness is true while any generation is registered (HUD view)"
    );
    assert!(
        super::agent_tools::agent_worker_generation_is_live(&id, old_gen),
        "the OLD generation stays live across a same-id resume — no ABA eviction"
    );
    assert!(
        super::agent_tools::agent_worker_generation_is_live(&id, new_gen),
        "the resumed (new) generation is independently live"
    );

    // The old owner must NOT drop its guard just because a new generation
    // registered: give it ample time to (wrongly) observe an exit and fail if it
    // tears the old worktree down while the old worker is still live.
    std::thread::sleep(std::time::Duration::from_millis(120));
    assert!(
        !handle.is_finished(),
        "old-generation owner must keep owning its worktree while the old worker is live"
    );
    assert!(
        dropped.lock().expect("drop lock").is_empty(),
        "old worktree must not be torn down merely because a new generation took the id"
    );

    // Only when the OLD generation itself exits does its owner release the guard;
    // the NEW generation is untouched.
    super::agent_tools::unregister_agent_cancel_signal_for_tests(&id, old_gen);
    assert!(
        !super::agent_tools::agent_worker_generation_is_live(&id, old_gen),
        "old generation flips not-live only on its own exit ack"
    );
    let joined = std::time::Instant::now();
    while !handle.is_finished() {
        assert!(
            joined.elapsed() < std::time::Duration::from_secs(1),
            "old-generation owner must finish within 1s of the old exit ack"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    handle.join().expect("cleanup owner joins cleanly");
    assert_eq!(
        dropped.lock().expect("drop lock").as_slice(),
        std::slice::from_ref(&id),
        "the old-generation guard is dropped exactly once, after the old exit ack"
    );

    // The new generation remained live throughout and flips only on its own exit.
    assert!(
        super::agent_tools::agent_worker_generation_is_live(&id, new_gen),
        "the new generation stays live independent of the old owner's teardown"
    );
    super::agent_tools::unregister_agent_cancel_signal_for_tests(&id, new_gen);
    assert!(
        !super::agent_tools::agent_worker_generation_is_live(&id, new_gen),
        "new generation flips not-live only on its own exit ack"
    );
    assert!(
        !super::agent_tools::agent_worker_is_live(&id),
        "any-generation liveness is false once every generation has exited"
    );
}

/// HIGH-1 regression: the execute loop's post-reclaim recheck must bar a fresh
/// spawn when either the overall deadline has elapsed OR the reclaimed slot's
/// worker is still physically live (a cooperative cancel does not join the
/// worker). Exercises `spawn_barred_after_reclaim`, the exact decision the loop
/// makes after `reclaim_spawn_slots` returns.
#[test]
fn spawn_barred_after_reclaim_respects_deadline_and_physical_liveness() {
    let _guard = env_lock();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let window = 1;
    let generation = super::agent_tools::AGENT_INITIAL_RUN_GENERATION;

    // Case A: deadline still ahead, no live worker in the (empty) slot set —
    // spawning is allowed.
    let future = std::time::Instant::now() + std::time::Duration::from_secs(30);
    assert!(
        !super::spawn_barred_after_reclaim(&[], window, future),
        "an open slot before the deadline must permit a spawn"
    );

    // Case B: deadline already elapsed — barred regardless of slot state.
    let past = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_millis(1))
        .expect("instant is after the epoch by at least 1ms");
    assert!(
        super::spawn_barred_after_reclaim(&[], window, past),
        "an elapsed overall deadline must bar any further spawn"
    );

    // Case C: deadline ahead, but the reclaimed slot's worker is still physically
    // live (its cooperative cancel has not joined) — barred, so the live-worker
    // count never exceeds `window`.
    let live_id = format!("barred-{stamp}");
    super::agent_tools::register_agent_cancel_signal_for_tests(&live_id, generation);
    let in_flight = vec![live_id.clone()];
    assert!(
        super::spawn_barred_after_reclaim(&in_flight, window, future),
        "a still-live reclaimed worker must bar a spawn that would exceed the window"
    );

    // Once that worker physically exits, the slot is genuinely free again.
    super::agent_tools::unregister_agent_cancel_signal_for_tests(&live_id, generation);
    assert!(
        !super::spawn_barred_after_reclaim(&in_flight, window, future),
        "after the physical-exit ack the slot is free and a spawn is permitted"
    );
}

/// MEDIUM-1 regression: if the per-guard cleanup thread cannot be spawned, the
/// closure that owns the guard must NOT drop it (that would tear down a live
/// worktree). The caller must recover the exact same guard from the handoff cell
/// and park it in the process-global quarantine, which drops it only after the
/// exact `(agent_id, generation)` worker exits. Proves: (a) spawn failure while
/// live drops nothing, (b) drop happens only after the exact-generation exit ack,
/// (c) guard ownership moves exactly once (dropped exactly once). Bounded by an
/// explicit 1s timeout so a regression fails loudly instead of hanging.
#[test]
fn spawn_failure_quarantines_guard_until_exact_generation_exit() {
    let _guard = env_lock();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should be after epoch")
        .as_nanos();
    let id = format!("spawnfail-{stamp}");
    let generation = super::agent_tools::AGENT_INITIAL_RUN_GENERATION;

    let dropped = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let guard = DropRecordingGuard {
        id: id.clone(),
        path: temp_dir(),
        dropped: std::sync::Arc::clone(&dropped),
    };

    // Worker is live: cooperative cancel has fired but the thread has not exited.
    super::agent_tools::register_agent_cancel_signal_for_tests(&id, generation);

    // Force the next cleanup-thread spawn to fail, exercising the quarantine
    // handoff. The call returns None (no dedicated thread) but must have preserved
    // guard ownership by parking it in the quarantine.
    super::FORCE_DEFERRED_CLEANUP_SPAWN_FAILURE.store(true, std::sync::atomic::Ordering::SeqCst);
    let handle = super::spawn_deferred_worktree_cleanup(
        id.clone(),
        generation,
        Box::new(guard),
        std::time::Duration::from_millis(5),
    );
    assert!(
        handle.is_none(),
        "a forced spawn failure yields no dedicated cleanup thread handle"
    );

    // (a) While the worker is live the quarantined guard must NOT be dropped, even
    // though its dedicated thread never started.
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert!(
        dropped.lock().expect("drop lock").is_empty(),
        "spawn failure must not drop a still-live worker's worktree guard"
    );

    // The worker physically exits.
    super::agent_tools::unregister_agent_cancel_signal_for_tests(&id, generation);

    // (b) The quarantine drainer must observe the exact-generation exit ack and
    // drop the guard. Poll with an explicit deadline instead of a join handle
    // (the drainer is detached and shared).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if !dropped.lock().expect("drop lock").is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "quarantine must drop the guard within 1s of the physical-exit ack"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    // (c) Ownership moved exactly once: dropped exactly once, for this id.
    std::thread::sleep(std::time::Duration::from_millis(20));
    assert_eq!(
        dropped.lock().expect("drop lock").as_slice(),
        std::slice::from_ref(&id),
        "the quarantined guard is dropped exactly once, after the exact-generation exit ack"
    );
}

// ---- AGENT → MAIN mid-run channel ------------------------------------------

/// Every real sub-agent role can reach the parent conversation mid-run.
/// `classifier` is the deliberate exception: one reply, no tool loop, so it has
/// no mid-run to report from.
#[test]
fn subagent_default_toolsets_carry_the_upstream_send_message() {
    for kind in [
        "general-purpose",
        "Explore",
        "Plan",
        "deep-research",
        "Verification",
        "code-reviewer",
        "debugger",
        "data-analyst",
        "refactor",
        "zo-guide",
        "statusline-setup",
    ] {
        assert!(
            super::allowed_tools_for_subagent(kind).contains("SendMessage"),
            "`{kind}` must be able to message main mid-run"
        );
    }
    assert!(
        !super::allowed_tools_for_subagent("classifier").contains("SendMessage"),
        "a one-shot classifier has no mid-run to report from"
    );
}

fn test_caller() -> crate::SubagentIdentity {
    crate::SubagentIdentity {
        agent_id: "agent-7f31".to_string(),
        name: "runtime-scout".to_string(),
    }
}

/// The restriction that keeps `SendMessage` in a sub-agent's default toolset
/// safe: `main` is the ONLY address. A sibling name would be agent→agent
/// messaging with no orchestrator in the loop, and an id would additionally
/// expose the resume path (a re-spawn with the original permission envelope).
#[test]
fn subagent_send_message_rejects_every_target_but_main() {
    // The `main` variant below pushes onto the process-global completion
    // channel; hold the crate-wide lock (what every other channel-registering
    // test uses) so it cannot land in a sibling test's receiver.
    let _guard = env_lock();
    let caller = test_caller();
    for target in ["sibling", "agent-99ab", "MAIN "] {
        let input = super::SendMessageInput {
            to: target.to_string(),
            message: "partial finding".to_string(),
            attach_results: Vec::new(),
            refresh_harness: false,
            session_id: None,
        };
        let result = super::run_send_message(None, &input, Some(&caller), None, None, None, None);
        if target.trim().eq_ignore_ascii_case("main") {
            // Whitespace/case variants are still the reserved address.
            assert!(result.is_ok(), "`{target}` must resolve to the main edge");
            continue;
        }
        let error = result.expect_err("a sub-agent must not address `{target}`");
        let rendered = error.to_string();
        assert!(rendered.contains("only send to \"main\""), "{rendered}");
        assert!(rendered.contains(target), "{rendered}");
    }
}

/// The MAIN model's own `SendMessage(to: "main")` is a no-op mistake, not a
/// prefix match against a spawned agent whose name happens to start with "main".
#[test]
fn main_target_is_reserved_for_the_main_caller_too() {
    let input = super::SendMessageInput {
        to: "main".to_string(),
        message: "hello".to_string(),
        attach_results: Vec::new(),
            refresh_harness: false,
        session_id: None,
    };
    let error = super::run_send_message(None, &input, None, None, None, None, None)
        .expect_err("`main` must never be resolved as an agent name");
    assert!(error.to_string().contains("reserved address"), "{error}");
}

/// The upstream edge rides the SAME channel as completions, in FIFO order, so
/// "the agent said something" can never overtake or trail "the agent finished".
#[test]
fn agent_message_and_completion_keep_their_order_on_one_channel() {
    let _guard = env_lock();
    let mut rx = crate::register_agent_completion_channel();
    let caller = test_caller();

    let sent = super::run_send_message(
        None,
        &super::SendMessageInput {
            to: "main".to_string(),
            message: "the flag lives in config.rs".to_string(),
            attach_results: Vec::new(),
            refresh_harness: false,
            session_id: None,
        },
        Some(&caller),
        None,
        None,
        None,
        None,
    )
    .expect("upstream send");
    let sent: serde_json::Value = serde_json::from_str(&sent).expect("json result");
    assert_eq!(sent["delivered"], serde_json::json!(true));
    assert_eq!(sent["mode"], serde_json::json!("notify"));
    assert_eq!(sent["agentId"], serde_json::json!("agent-7f31"));

    super::agent_tools::publish_agent_completion_for_tests(super::AgentCompletion {
        agent_id: "agent-7f31".to_string(),
        name: "runtime-scout".to_string(),
        status: "completed".to_string(),
        result: Some("done".to_string()),
        structured: None,
        error: None,
        run: HelperRun::default(),
    });

    // Filter to THIS agent: the assertion is the relative order, which is the
    // actual contract — one transport, so a message can never overtake or
    // trail the completion of the same agent.
    let mut ours = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if crate::agent_message_source_id(&event.agent_id) == "agent-7f31" {
            ours.push(event);
        }
    }
    super::agent_tools::clear_agent_completion_channel_for_tests();

    assert_eq!(ours.len(), 2, "message then completion, both on one channel");
    assert_eq!(ours[0].status, crate::AGENT_MESSAGE_STATUS);
    // Suffixed so it can never collide with an awaited terminal id, but the
    // real address is recoverable for the reply pointer.
    assert_eq!(ours[0].agent_id, "agent-7f31#message");
    assert_eq!(ours[0].result.as_deref(), Some("the flag lives in config.rs"));
    assert_eq!(ours[1].agent_id, "agent-7f31");
    assert_eq!(ours[1].status, "completed");
}

/// No interactive host in this process (headless / serve): the sub-agent is
/// told its message went nowhere instead of being handed a fake `delivered`.
#[test]
fn upstream_send_reports_failure_when_no_host_consumes_the_channel() {
    let _guard = env_lock();
    super::agent_tools::clear_agent_completion_channel_for_tests();
    let caller = test_caller();
    let sent = super::run_send_message(
        None,
        &super::SendMessageInput {
            to: "main".to_string(),
            message: "anyone there?".to_string(),
            attach_results: Vec::new(),
            refresh_harness: false,
            session_id: None,
        },
        Some(&caller),
        None,
        None,
        None,
        None,
    )
    .expect("upstream send still returns a result");
    let sent: serde_json::Value = serde_json::from_str(&sent).expect("json result");
    assert_eq!(sent["delivered"], serde_json::json!(false));
    assert!(
        sent["info"].as_str().unwrap_or_default().contains("final result"),
        "{sent}"
    );
}

/// The tool layer's own guards, ahead of the store: a stop that cannot name an
/// agent, cannot say why, or cannot prove which session is asking never reaches
/// [`super::agent_tools::stop_agent_for_session`] at all. The ownership checks
/// themselves are pinned in `agent_tools`; what is pinned here is that the
/// model cannot skip past them with blank or unidentified input.
mod stop_agent_tool_guards {
    use super::super::{run_stop_agent, StopAgentInput};
    use crate::ToolError;

    fn input(agent_id: &str, reason: &str) -> StopAgentInput {
        StopAgentInput {
            agent_id: agent_id.to_string(),
            reason: reason.to_string(),
        }
    }

    fn invalid_input_message(result: Result<String, ToolError>) -> String {
        match result {
            Err(ToolError::InvalidInput(message)) => message,
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn blank_agent_id_is_refused() {
        let message =
            invalid_input_message(run_stop_agent(&super::super::agent_tools::AgentRegistry::unowned_from_cwd(), &input("   ", "drifted"), Some("session-a")));
        assert!(
            message.contains("agent_id"),
            "the refusal must name the missing field: {message}"
        );
    }

    #[test]
    fn blank_reason_is_refused() {
        let message =
            invalid_input_message(run_stop_agent(&super::super::agent_tools::AgentRegistry::unowned_from_cwd(), &input("agent-1", " \t "), Some("session-a")));
        assert!(
            message.contains("reason"),
            "the refusal must name the missing field: {message}"
        );
    }

    #[test]
    fn a_session_that_cannot_be_identified_cannot_stop_anything() {
        let message = invalid_input_message(run_stop_agent(&super::super::agent_tools::AgentRegistry::unowned_from_cwd(), &input("agent-1", "drifted"), None));
        assert!(
            message.contains("ownership"),
            "the refusal must say ownership is what is missing: {message}"
        );
    }
}

/// `ListAgents` answers from the SESSION's own registry — the same store the
/// Alt+A overview reads (`zo-ide/src/tui/agents.rs` → `subagent_progress`), not
/// a second roster of its own. So a helper running as a thread of this process
/// and a teammate running in a pane of its own both appear, each with the
/// executor it runs in and the receipt its last `SendMessage` came to, and a
/// child belonging to another session never does: read ownership is
/// `parentSessionId`, exactly as the overview scans it.
mod list_agents_tool {
    use super::super::{run_list_agents, ListAgentsInput, RosterFormat};
    use crate::misc_tools::agent_tools::AgentRegistry;
    use serde_json::json;
    use std::path::Path;

    /// One manifest on disk, in the shape the agent workers write.
    fn write_manifest(store: &Path, id: &str, session: &str, extra: &serde_json::Value) {
        std::fs::create_dir_all(store).expect("store");
        let mut manifest = json!({
            "agentId": id,
            "parentSessionId": session,
            "name": id,
            "description": "d",
            "status": "running",
            "outputFile": store.join(format!("{id}.md")).display().to_string(),
            "manifestFile": store.join(format!("{id}.json")).display().to_string(),
            "createdAt": "100",
            "startedAt": "100",
            "runGeneration": 1,
        });
        if let (Some(object), Some(more)) = (manifest.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                object.insert(key.clone(), value.clone());
            }
        }
        std::fs::write(
            store.join(format!("{id}.json")),
            serde_json::to_vec(&manifest).expect("json"),
        )
        .expect("write");
    }

    fn input(format: RosterFormat) -> ListAgentsInput {
        ListAgentsInput { format }
    }

    #[test]
    fn an_inline_and_a_pane_child_are_listed_with_execution_and_receipt() {
        let root = tempfile::tempdir().expect("root");
        write_manifest(
            root.path(),
            "agent-inline",
            "session-a",
            &json!({
                "label": "scout",
                "execution": "inline",
                "lastReceipt": { "receipt": "consumed", "at": 10 },
            }),
        );
        write_manifest(
            root.path(),
            "agent-pane",
            "session-a",
            &json!({
                "label": "builder",
                "execution": "pane",
                "pane": "term:7",
                "runGeneration": 3,
                "startedAt": "200",
                "lastReceipt": { "receipt": "queued", "at": 20 },
            }),
        );
        // Another session's child, in the SAME store: read ownership is the
        // manifest's parent session, not the directory it happens to sit in.
        write_manifest(root.path(), "agent-stranger", "session-b", &json!({}));

        let registry = AgentRegistry::at_root_for_tests("session-a", root.path());
        let rendered = run_list_agents(&registry, &input(RosterFormat::Json), Some("session-a"))
            .expect("the roster reads");
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("json result");
        let agents = value["agents"].as_array().expect("rows").clone();
        assert_eq!(
            agents.len(),
            2,
            "both of this session's children, and only those: {rendered}"
        );
        assert_eq!(value["count"], json!(2), "{rendered}");

        let inline = &agents[0];
        assert_eq!(inline["id"], json!("agent-inline"), "{rendered}");
        assert_eq!(inline["name"], json!("scout"), "the label is the name");
        assert_eq!(inline["status"], json!("running"), "{rendered}");
        assert_eq!(inline["execution"], json!("inline"), "{rendered}");
        assert_eq!(inline["pane"], json!(null), "a thread has no pane of its own");
        assert_eq!(inline["lastReceipt"], json!("consumed"), "{rendered}");
        assert_eq!(inline["run_generation"], json!(1), "{rendered}");
        assert_eq!(
            inline["completion_waiting"],
            json!(false),
            "nothing has been published for it yet: {rendered}"
        );

        let pane = &agents[1];
        assert_eq!(pane["id"], json!("agent-pane"), "started later, listed later");
        assert_eq!(pane["name"], json!("builder"), "{rendered}");
        assert_eq!(pane["execution"], json!("pane"), "{rendered}");
        assert_eq!(pane["pane"], json!("term:7"), "{rendered}");
        assert_eq!(pane["lastReceipt"], json!("queued"), "{rendered}");
        assert_eq!(pane["run_generation"], json!(3), "{rendered}");

        for row in &agents {
            assert_ne!(
                row["id"],
                json!("agent-stranger"),
                "another session's child is not this session's roster: {rendered}"
            );
        }
    }

    /// The compact text shape: one header row and one line per agent, with the
    /// same facts the JSON carries.
    #[test]
    fn the_table_shape_names_every_column_and_one_line_per_agent() {
        let root = tempfile::tempdir().expect("root");
        write_manifest(
            root.path(),
            "agent-pane",
            "session-a",
            &json!({ "execution": "pane", "pane": "term:7" }),
        );
        let registry = AgentRegistry::at_root_for_tests("session-a", root.path());
        let table = run_list_agents(&registry, &input(RosterFormat::Table), Some("session-a"))
            .expect("the roster reads");
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 2, "a header and one agent: {table}");
        for column in super::super::ROSTER_COLUMNS {
            assert!(
                lines[0].contains(column),
                "the header must name `{column}`: {table}"
            );
        }
        assert!(lines[1].contains("agent-pane"), "{table}");
        assert!(lines[1].contains("term:7"), "{table}");
    }

    /// An empty roster is a sentence, not an empty array the model has to
    /// interpret.
    #[test]
    fn an_empty_registry_says_so_in_one_line() {
        let root = tempfile::tempdir().expect("root");
        let registry = AgentRegistry::at_root_for_tests("session-a", root.path());
        let table = run_list_agents(&registry, &input(RosterFormat::Table), Some("session-a"))
            .expect("the roster reads");
        assert_eq!(table, super::super::NO_AGENTS_LINE);

        let rendered = run_list_agents(&registry, &input(RosterFormat::Json), Some("session-a"))
            .expect("the roster reads");
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("json result");
        assert_eq!(value["count"], json!(0), "{rendered}");
        assert_eq!(
            value["note"],
            json!(super::super::NO_AGENTS_LINE),
            "the JSON says it too: {rendered}"
        );
    }

    /// Without a session there is no roster to read: the tool says so rather
    /// than falling back to a store re-derived from the process cwd, which is
    /// exactly the drift t-2511 closed.
    #[test]
    fn a_session_that_cannot_be_identified_has_no_roster() {
        let root = tempfile::tempdir().expect("root");
        let registry = AgentRegistry::at_root_for_tests("session-a", root.path());
        let error = run_list_agents(&registry, &input(RosterFormat::Json), None)
            .expect_err("an unidentified session cannot own agents");
        match error {
            crate::ToolError::InvalidInput(message) => assert!(
                message.contains("session"),
                "the refusal must name what is missing: {message}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }
}

/* ---- PushNotification (t-2943): the receipt names the road ---- */

use super::push_notification::{push_road, PushLimits, PushNotice, PushRoad, PushSurface};
use super::{run_push_notification, PushNotificationInput};

/// A surface that answers every push with one fixed road and keeps the notice
/// it was handed, so a test can read both the receipt and what would have
/// gone out.
struct RoadChannel {
    road: PushRoad,
    seen: std::sync::Mutex<Option<PushNotice>>,
}

impl RoadChannel {
    fn new(road: PushRoad) -> Self {
        Self {
            road,
            seen: std::sync::Mutex::new(None),
        }
    }
}

impl UserQuestionChannel for RoadChannel {
    fn ask(
        &self,
        _question: &str,
        _header: Option<&str>,
        _options: &[QuestionOption],
        _multi_select: bool,
    ) -> Result<Vec<String>, ToolError> {
        Err(ToolError::Execution("not a question surface".into()))
    }

    fn push_notification(&self, notice: &PushNotice) -> Result<PushRoad, ToolError> {
        *self.seen.lock().expect("notice lock") = Some(notice.clone());
        Ok(self.road)
    }
}

fn push_input(message: &str) -> PushNotificationInput {
    PushNotificationInput {
        message: message.to_string(),
        status: Some("proactive".to_string()),
    }
}

fn push_receipt(ctx: &ToolContext, message: &str) -> serde_json::Value {
    let result = run_push_notification(&push_input(message), ctx).expect("push runs");
    serde_json::from_str(&result).expect("receipt is JSON")
}

/// One judgement, four answers, in this order: a person at the keyboard beats
/// every road, a window beats a terminal, a terminal beats nothing.
#[test]
fn the_road_is_judged_attended_then_window_then_terminal_then_nowhere() {
    let surface = |attended, window, terminal| PushSurface {
        attended,
        window,
        terminal,
    };
    assert_eq!(push_road(surface(true, true, true)), PushRoad::SkippedAttended);
    assert_eq!(push_road(surface(false, true, true)), PushRoad::Window);
    assert_eq!(push_road(surface(false, false, true)), PushRoad::Terminal);
    assert_eq!(push_road(surface(false, false, false)), PushRoad::SkippedNowhere);
    // The words the receipt says — pinned, because the model reads them and
    // the design table names them.
    assert_eq!(PushRoad::Window.as_str(), "window");
    assert_eq!(PushRoad::Terminal.as_str(), "terminal");
    assert_eq!(PushRoad::SkippedAttended.as_str(), "skipped: attended");
    assert_eq!(PushRoad::SkippedNowhere.as_str(), "skipped: nowhere");
}

/// The receipt says the road the surface took, and only the two real roads
/// count as delivered — a skip is expected and is not an error.
#[test]
fn the_receipt_names_the_road_the_surface_took() {
    for (road, delivered) in [
        (PushRoad::Window, true),
        (PushRoad::Terminal, true),
        (PushRoad::SkippedAttended, false),
        (PushRoad::SkippedNowhere, false),
    ] {
        let channel = std::sync::Arc::new(RoadChannel::new(road));
        let ctx = ToolContext::new().with_user_question_channel(channel.clone());
        let receipt = push_receipt(&ctx, "Build is green; merge when you are back");
        assert_eq!(receipt["road"], road.as_str(), "{receipt}");
        assert_eq!(receipt["delivered"], delivered, "{receipt}");
        assert_eq!(receipt["truncated"], false);
        let seen = channel.seen.lock().expect("notice lock").clone().expect("notice handed over");
        assert_eq!(seen.body, "Build is green; merge when you are back");
        assert!(!seen.title.is_empty(), "a notice always has a title");
    }
}

/// No surface at all — a headless run, a sub-agent — is `skipped: nowhere`,
/// and the message comes back inline so nothing the model wrote is lost.
#[test]
fn without_a_surface_the_push_is_skipped_nowhere_and_echoed_inline() {
    let ctx = ToolContext::new();
    let receipt = push_receipt(&ctx, "Done: 3 tests failed");
    assert_eq!(receipt["road"], "skipped: nowhere");
    assert_eq!(receipt["delivered"], false);
    assert_eq!(receipt["message"], "Done: 3 tests failed");
}

/// The body is one line, capped at the table's limit on a char boundary, and
/// the receipt says when it was cut.
#[test]
fn the_message_is_one_line_and_capped_at_the_table() {
    let limits = PushLimits::default();
    let channel = std::sync::Arc::new(RoadChannel::new(PushRoad::Terminal));
    let ctx = ToolContext::new().with_user_question_channel(channel.clone());

    let long = "가".repeat(limits.max_message_chars + 40);
    let receipt = push_receipt(&ctx, &format!("first line\n\n  second   line\n{long}"));
    assert_eq!(receipt["truncated"], true, "{receipt}");
    let body = receipt["message"].as_str().expect("message");
    assert!(!body.contains('\n'), "one line: {body:?}");
    assert!(body.starts_with("first line second line 가"), "{body:?}");
    assert_eq!(body.chars().count(), limits.max_message_chars);
    let seen = channel.seen.lock().expect("notice lock").clone().expect("notice handed over");
    assert_eq!(seen.body, body, "what went out is what the receipt shows");
}

#[test]
fn an_empty_message_is_refused_before_any_surface_is_asked() {
    let channel = std::sync::Arc::new(RoadChannel::new(PushRoad::Terminal));
    let ctx = ToolContext::new().with_user_question_channel(channel.clone());
    let error = run_push_notification(&push_input("  \n "), &ctx).expect_err("empty is refused");
    assert!(matches!(error, ToolError::InvalidInput(_)), "{error:?}");
    assert!(channel.seen.lock().expect("notice lock").is_none());
}
