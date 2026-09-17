//! Focused contracts for the forge-code candidate ports.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use runtime::Session;
use zo_ide::doctor::{DoctorReport, Finding, Status};
use zo_ide::effort::Effort;
use zo_ide::preferences::{load_from_path, save_to_path};
use zo_ide::resume::{list_recent_sessions_limited, render_recent_sessions, session_cwd_path};
use zo_ide::status_format::{
    context_usage_percent, format_estimated_cost, format_usage_status,
};

fn test_root(label: &str) -> PathBuf {
    static COUNTER: OnceLock<Mutex<u64>> = OnceLock::new();
    let counter = COUNTER.get_or_init(|| Mutex::new(0));
    let mut value = counter.lock().expect("counter lock");
    let path = std::env::temp_dir().join(format!(
        "zo-port-{label}-{}-{value}",
        std::process::id()
    ));
    *value += 1;
    fs::create_dir_all(&path).expect("create test root");
    path
}

#[test]
fn global_preference_file_round_trips_selected_defaults() {
    let root = test_root("preferences");
    let path = root.join("settings.json");
    save_to_path(&path, api::ANTHROPIC_OPUS_MODEL_ALIAS, Effort::Smart)
        .expect("save preferences");
    let loaded = load_from_path(&path);
    let expected_model = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
    assert_eq!(loaded.model.as_deref(), Some(expected_model.as_str()));
    assert_eq!(loaded.effort, Some(Effort::Smart));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn smart_uses_the_runtime_dynamic_band_without_a_settings_loader() {
    let floor = Effort::Smart.level().expect("smart floor");
    let ceiling = Effort::Smart.band_ceiling().expect("smart ceiling");
    let model = api::resolve_catalog_alias(api::OPENAI_LATEST_MODEL_ALIAS);
    assert_eq!(floor, api::EffortLevel::Xhigh);
    assert_eq!(ceiling, api::EffortLevel::Max);

    let light = api::resolve_effort_band(
        floor,
        ceiling,
        &model,
        api::BandDifficulty::default(),
    );
    let heavy = api::resolve_effort_band(
        floor,
        ceiling,
        &model,
        api::BandDifficulty {
            heavy_intent: true,
            ..api::BandDifficulty::default()
        },
    );
    assert_eq!(light, api::EffortLevel::Xhigh);
    assert_eq!(heavy, api::EffortLevel::Max);
}

#[test]
fn doctor_report_is_pipe_friendly_and_secret_free() {
    let secret = "access-token-never-rendered";
    let report = DoctorReport {
        findings: vec![Finding {
            label: "Claude".to_string(),
            status: Status::Pass,
            value: "logged in via keychain".to_string(),
        }],
    };
    let rendered = report.render();
    assert!(rendered.contains("Claude"));
    assert!(rendered.contains("PASS"));
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("\x1b[91m"));
    assert!(rendered.contains("\x1b[32m"));
}

#[test]
fn resume_rows_include_cwd_and_first_prompt() {
    let root = test_root("resume");
    let sessions = root.join("sessions");
    fs::create_dir_all(&sessions).expect("create sessions directory");
    let session = Session::new();
    let id = session.session_id.clone();
    let transcript = sessions.join(format!("{id}.jsonl"));
    session
        .save_to_path(&transcript)
        .expect("save session transcript");
    fs::write(
        session_cwd_path(&transcript),
        r#"{"cwd":"/tmp/port-candidate-workspace"}"#,
    )
    .expect("save cwd sidecar");
    let mut populated = Session::load_from_path(&transcript).expect("reload session");
    populated
        .push_user_text("first prompt from the port test")
        .expect("append prompt");
    populated
        .save_to_path(&transcript)
        .expect("save prompt");

    let prior_root = std::env::var_os("ZO_SESSION_ROOT");
    std::env::set_var("ZO_SESSION_ROOT", &root);
    let rows = list_recent_sessions_limited(5).expect("list sessions");
    let rendered = render_recent_sessions(5).expect("render sessions");
    match prior_root {
        Some(value) => std::env::set_var("ZO_SESSION_ROOT", value),
        None => std::env::remove_var("ZO_SESSION_ROOT"),
    }

    let row = rows.iter().find(|row| row.id == id).expect("listed row");
    assert_eq!(row.cwd.as_deref(), Some(Path::new("/tmp/port-candidate-workspace")));
    assert_eq!(
        row.first_prompt.as_deref(),
        Some("first prompt from the port test")
    );
    assert!(rendered.contains(&id));
    assert!(rendered.contains("/tmp/port-candidate-workspace"));
    assert!(rendered.contains("first prompt from the port test"));
    let _ = fs::remove_dir_all(root);
}

#[test]
#[allow(clippy::float_cmp)]
fn status_helpers_report_context_cost_and_tokens() {
    let model = api::resolve_catalog_alias(api::OPENAI_LATEST_MODEL_ALIAS);
    let usage = runtime::TokenUsage {
        input_tokens: 50_000,
        output_tokens: 2_000,
        cache_creation_input_tokens: 10_000,
        cache_read_input_tokens: 40_000,
        output_tokens_details: None,
    };
    assert_eq!(context_usage_percent(100_000, 200_000), 50.0);
    assert!(format_estimated_cost(&model, usage).starts_with('$'));
    let status = format_usage_status(&model, usage, 200_000);
    assert!(status.contains("ctx 50.0%"));
    assert!(status.contains("tokens 102.0k"));
    assert!(status.contains("cost $"));
}
