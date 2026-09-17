//! Permission enforcement plus small utility tools (sleep, config, plan mode, monitor, messaging).
//!
//! Split out of the old flat `tests.rs` (4,133 lines) by domain;
//! shared fixtures live in the parent module.

use super::*;

#[test]
fn exit_plan_mode_v2_persists_plan_artifact_under_zo_plans() {
    // End-to-end: run ExitPlanModeV2 → write_plan_artifact → build_v2_output,
    // isolated in a temp cwd so the real repo is never polluted. Proves the
    // gate property (never self-exits) AND the artifact write/markdown body.
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("plan-artifact");
    std::fs::create_dir_all(&root).expect("mkdir temp root");
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("set cwd");

    let output = run_tool(
        "ExitPlanModeV2",
        &json!({ "plan": "step 1\nstep 2", "summary": "My Plan" }),
    )
    .expect("ExitPlanModeV2 should return ok");
    let parsed: serde_json::Value = serde_json::from_str(&output).expect("json");

    assert_eq!(parsed["planModeExited"], false, "must never self-approve");
    let plan_path = parsed["planPath"].as_str().expect("planPath present");
    assert!(plan_path.contains(".zo/plans/"), "got {plan_path}");
    let contents = std::fs::read_to_string(plan_path).expect("artifact file exists");
    assert!(contents.starts_with("# My Plan\n"), "got: {contents}");
    assert!(contents.contains("step 1"));
    assert!(contents.contains("step 2"));

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn sleep_waits_and_reports_duration() {
    let started = std::time::Instant::now();
    let result = run_tool("Sleep", &json!({"duration_ms": 20})).expect("Sleep should succeed");
    let elapsed = started.elapsed();
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["duration_ms"], 20);
    assert!(output["message"]
        .as_str()
        .expect("message")
        .contains("Slept for 20ms"));
    assert!(elapsed >= Duration::from_millis(15));
}

#[test]
fn given_excessive_duration_when_sleep_then_clamps_to_max() {
    let result = run_tool("Sleep", &json!({"duration_ms": 999_999_999_u64}))
        .expect("excessive sleep should be clamped, not rejected");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["duration_ms"], 5_000);
    assert!(output["message"].as_str().unwrap().contains("clamped"));
}

#[test]
fn given_zero_duration_when_sleep_then_succeeds() {
    let result = run_tool("Sleep", &json!({"duration_ms": 0})).expect("0ms sleep should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["duration_ms"], 0);
}

#[test]
fn given_already_slept_flag_when_sleep_then_skips_blocking_sleep() {
    // live 디스패치는 런타임이 `tokio::time::sleep` 으로 이미 비차단
    // 대기한 뒤 `__zo_already_slept=true` 를 주입한다. 이때
    // `execute_sleep` 은 동기 `std::thread::sleep` 을 건너뛰어 ① 대기
    // 2배, ② `block_in_place` spinner freeze 를 막아야 한다. 보고
    // 메시지·duration 은 그대로 유지된다.
    let started = std::time::Instant::now();
    let result = run_tool(
        "Sleep",
        &json!({"duration_ms": 5_000, "__zo_already_slept": true}),
    )
    .expect("Sleep with already_slept flag should succeed");
    let elapsed = started.elapsed();
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["duration_ms"], 5_000);
    assert!(output["message"]
        .as_str()
        .expect("message")
        .contains("Slept for 5000ms"));
    assert!(
        elapsed < Duration::from_millis(500),
        "already_slept=true 인데 동기 슬립이 발생함: {elapsed:?}"
    );
}

#[test]
fn send_to_user_without_channel_echoes_content_inline() {
    // The shared `test_ctx` carries no `user_question_channel` (as in a headless
    // run or a sub-agent's fresh context), so the push degrades to an inline
    // echo instead of surfacing on a TUI — the content is never lost.
    let result = run_tool("send_to_user", &json!({ "message": "verbatim finding" }))
        .expect("send_to_user should succeed");

    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["delivered"], false, "no channel → not delivered");
    assert_eq!(output["truncated"], false);
    assert_eq!(output["message"], "verbatim finding", "echoes the content");
    assert_eq!(output["note"], "no interactive surface; returned inline");
}

#[test]
fn send_to_user_legacy_aliases_share_the_same_runner() {
    // `SendUserMessage` / `Brief` are legacy aliases: all three names must route
    // through one runner, so a bare `{message}` payload degrades identically.
    for name in ["send_to_user", "SendUserMessage", "Brief"] {
        let result = run_tool(name, &json!({ "message": "aligned" }))
            .unwrap_or_else(|error| panic!("{name} should succeed: {error:?}"));
        let output: serde_json::Value = serde_json::from_str(&result).expect("json");
        assert_eq!(output["delivered"], false, "{name}");
        assert_eq!(output["message"], "aligned", "{name}");
    }
}

#[test]
fn send_to_user_rejects_empty_message() {
    let error = run_tool("send_to_user", &json!({ "message": "   " }))
        .expect_err("blank message must be rejected");
    assert!(
        matches!(error, ToolError::InvalidInput(_)),
        "got {error:?}"
    );
}

#[test]
fn send_to_user_with_channel_pushes_and_reports_delivered() {
    use std::sync::{Arc, Mutex};

    /// Records every pushed message so the test can assert the channel was hit.
    struct RecordingChannel(Arc<Mutex<Vec<String>>>);
    impl crate::UserQuestionChannel for RecordingChannel {
        fn ask(
            &self,
            _question: &str,
            _header: Option<&str>,
            _options: &[runtime::message_stream::QuestionOption],
            _multi_select: bool,
        ) -> Result<Vec<String>, ToolError> {
            unreachable!("send_to_user must not call ask")
        }
        fn send_to_user(&self, message: &str) -> Result<(), ToolError> {
            self.0.lock().expect("lock").push(message.to_string());
            Ok(())
        }
    }

    let sink = Arc::new(Mutex::new(Vec::new()));
    let ctx = ToolContext::new()
        .with_user_question_channel(Arc::new(RecordingChannel(Arc::clone(&sink))));

    let result = execute_tool(&ctx, "send_to_user", &json!({ "message": "pushed body" }))
        .expect("send_to_user should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");

    assert_eq!(output["delivered"], true, "channel present → delivered");
    assert_eq!(output["truncated"], false);
    assert!(output.get("message").is_none(), "no inline echo when delivered");
    assert_eq!(sink.lock().expect("lock").as_slice(), ["pushed body"]);
}

#[test]
fn send_to_user_channel_install_reaches_pre_existing_registry_clones() {
    use std::sync::{Arc, Mutex};

    /// Mirrors `RecordingChannel` above; a channel that records pushes.
    struct RecordingChannel(Arc<Mutex<Vec<String>>>);
    impl crate::UserQuestionChannel for RecordingChannel {
        fn ask(
            &self,
            _question: &str,
            _header: Option<&str>,
            _options: &[runtime::message_stream::QuestionOption],
            _multi_select: bool,
        ) -> Result<Vec<String>, ToolError> {
            unreachable!("send_to_user must not call ask")
        }
        fn send_to_user(&self, message: &str) -> Result<(), ToolError> {
            self.0.lock().expect("lock").push(message.to_string());
            Ok(())
        }
    }

    // Production topology: the concurrent-dispatch closure captures a registry
    // clone at session BOOT, while the TUI installs the channel per-TURN into
    // the executor's clone. The install must write through to the boot-time
    // clone, or live `send_to_user` silently degrades to its inline echo.
    let mut registry = crate::GlobalToolRegistry::builtin();
    let dispatch_clone = registry.clone();

    let sink = Arc::new(Mutex::new(Vec::new()));
    registry
        .context_mut()
        .set_user_question_channel(Some(Arc::new(RecordingChannel(Arc::clone(&sink)))));

    let result = dispatch_clone
        .execute("send_to_user", &json!({ "message": "cross-clone push" }))
        .expect("send_to_user should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");

    assert_eq!(
        output["delivered"], true,
        "install after clone must still reach the dispatch clone"
    );
    assert_eq!(sink.lock().expect("lock").as_slice(), ["cross-clone push"]);
}

#[test]
fn send_to_user_truncates_past_the_cap_and_flags_it() {
    let long = "x".repeat(crate::misc_tools::MAX_SEND_TO_USER_CHARS + 500);
    let result = run_tool("send_to_user", &json!({ "message": long }))
        .expect("send_to_user should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");

    assert_eq!(output["truncated"], true, "over-cap push flags truncation");
    let echoed = output["message"].as_str().expect("message echoed inline");
    assert!(
        echoed.contains("truncated at"),
        "carries the truncation marker: {echoed}"
    );
    assert!(
        echoed.chars().count() < crate::misc_tools::MAX_SEND_TO_USER_CHARS + 200,
        "trimmed to the cap plus a short marker"
    );
}

#[test]
fn config_reads_and_writes_supported_values() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = std::env::temp_dir().join(format!(
        "zo-config-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let home = root.join("home");
    let cwd = root.join("cwd");
    std::fs::create_dir_all(home.join(".zo")).expect("home dir");
    std::fs::create_dir_all(cwd.join(".zo")).expect("cwd dir");
    std::fs::write(
        home.join(".zo").join("settings.json"),
        r#"{"verbose":false}"#,
    )
    .expect("write global settings");

    let original_home = std::env::var("HOME").ok();
    let original_config_home = std::env::var("ZO_CONFIG_HOME").ok();
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_var("HOME", &home);
    std::env::remove_var("ZO_CONFIG_HOME");
    std::env::set_current_dir(&cwd).expect("set cwd");

    let get = run_tool("Config", &json!({"setting": "verbose"})).expect("get config");
    let get_output: serde_json::Value = serde_json::from_str(&get).expect("json");
    assert_eq!(get_output["value"], false);

    let set = run_tool(
        "Config",
        &json!({"setting": "permissions.defaultMode", "value": "plan"}),
    )
    .expect("set config");
    let set_output: serde_json::Value = serde_json::from_str(&set).expect("json");
    assert_eq!(set_output["operation"], "set");
    assert_eq!(set_output["newValue"], "plan");

    let invalid = run_tool(
        "Config",
        &json!({"setting": "permissions.defaultMode", "value": "bogus"}),
    )
    .expect_err("invalid config value should error");
    assert!(invalid.to_string().contains("Invalid value"));

    let unknown = run_tool("Config", &json!({"setting": "nope"})).expect("unknown setting result");
    let unknown_output: serde_json::Value = serde_json::from_str(&unknown).expect("json");
    assert_eq!(unknown_output["success"], false);

    let smart_unknown = run_tool("Config", &json!({"setting": "smart.enabled", "value": true}))
        .expect("smart.enabled should be rejected as unsupported config setting");
    let smart_unknown_output: serde_json::Value =
        serde_json::from_str(&smart_unknown).expect("json");
    assert_eq!(smart_unknown_output["success"], false);

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match original_config_home {
        Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
        None => std::env::remove_var("ZO_CONFIG_HOME"),
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn enter_and_exit_plan_mode_round_trip_existing_local_override() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = std::env::temp_dir().join(format!(
        "zo-plan-mode-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let home = root.join("home");
    let cwd = root.join("cwd");
    std::fs::create_dir_all(home.join(".zo")).expect("home dir");
    std::fs::create_dir_all(cwd.join(".zo")).expect("cwd dir");
    std::fs::write(
        cwd.join(".zo").join("settings.local.json"),
        r#"{"permissions":{"defaultMode":"acceptEdits"}}"#,
    )
    .expect("write local settings");

    let original_home = std::env::var("HOME").ok();
    let original_config_home = std::env::var("ZO_CONFIG_HOME").ok();
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_var("HOME", &home);
    std::env::remove_var("ZO_CONFIG_HOME");
    std::env::set_current_dir(&cwd).expect("set cwd");

    let enter = run_tool("EnterPlanMode", &json!({})).expect("enter plan mode");
    let enter_output: serde_json::Value = serde_json::from_str(&enter).expect("json");
    assert_eq!(enter_output["changed"], true);
    assert_eq!(enter_output["managed"], true);
    assert_eq!(enter_output["previousLocalMode"], "acceptEdits");
    assert_eq!(enter_output["currentLocalMode"], "plan");

    let local_settings = std::fs::read_to_string(cwd.join(".zo").join("settings.local.json"))
        .expect("local settings after enter");
    assert!(local_settings.contains(r#""defaultMode": "plan""#));
    let state =
        std::fs::read_to_string(cwd.join(".zo").join("tool-state").join("plan-mode.json"))
            .expect("plan mode state");
    assert!(state.contains(r#""hadLocalOverride": true"#));
    assert!(state.contains(r#""previousLocalMode": "acceptEdits""#));

    let exit = run_tool("ExitPlanMode", &json!({})).expect("exit plan mode");
    let exit_output: serde_json::Value = serde_json::from_str(&exit).expect("json");
    assert_eq!(exit_output["changed"], true);
    assert_eq!(exit_output["managed"], false);
    assert_eq!(exit_output["previousLocalMode"], "acceptEdits");
    assert_eq!(exit_output["currentLocalMode"], "acceptEdits");

    let local_settings = std::fs::read_to_string(cwd.join(".zo").join("settings.local.json"))
        .expect("local settings after exit");
    assert!(local_settings.contains(r#""defaultMode": "acceptEdits""#));
    assert!(!cwd
        .join(".zo")
        .join("tool-state")
        .join("plan-mode.json")
        .exists());

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match original_config_home {
        Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
        None => std::env::remove_var("ZO_CONFIG_HOME"),
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn exit_plan_mode_clears_override_when_enter_created_it_from_empty_local_state() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = std::env::temp_dir().join(format!(
        "zo-plan-mode-empty-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    let home = root.join("home");
    let cwd = root.join("cwd");
    std::fs::create_dir_all(home.join(".zo")).expect("home dir");
    std::fs::create_dir_all(cwd.join(".zo")).expect("cwd dir");

    let original_home = std::env::var("HOME").ok();
    let original_config_home = std::env::var("ZO_CONFIG_HOME").ok();
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_var("HOME", &home);
    std::env::remove_var("ZO_CONFIG_HOME");
    std::env::set_current_dir(&cwd).expect("set cwd");

    let enter = run_tool("EnterPlanMode", &json!({})).expect("enter plan mode");
    let enter_output: serde_json::Value = serde_json::from_str(&enter).expect("json");
    assert_eq!(enter_output["previousLocalMode"], serde_json::Value::Null);
    assert_eq!(enter_output["currentLocalMode"], "plan");

    let exit = run_tool("ExitPlanMode", &json!({})).expect("exit plan mode");
    let exit_output: serde_json::Value = serde_json::from_str(&exit).expect("json");
    assert_eq!(exit_output["changed"], true);
    assert_eq!(exit_output["currentLocalMode"], serde_json::Value::Null);

    let local_settings = std::fs::read_to_string(cwd.join(".zo").join("settings.local.json"))
        .expect("local settings after exit");
    let local_settings_json: serde_json::Value =
        serde_json::from_str(&local_settings).expect("valid settings json");
    assert_eq!(
        local_settings_json.get("permissions"),
        None,
        "permissions override should be removed on exit"
    );
    assert!(!cwd
        .join(".zo")
        .join("tool-state")
        .join("plan-mode.json")
        .exists());

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
    match original_config_home {
        Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
        None => std::env::remove_var("ZO_CONFIG_HOME"),
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn structured_output_echoes_input_payload() {
    let result = run_tool("StructuredOutput", &json!({"ok": true, "items": [1, 2, 3]}))
        .expect("StructuredOutput should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["data"], "Structured output provided successfully");
    assert_eq!(output["structured_output"]["ok"], true);
    assert_eq!(output["structured_output"]["items"][1], 2);
}

#[test]
fn given_empty_payload_when_structured_output_then_rejects_with_error() {
    let result = run_tool("StructuredOutput", &json!({}));
    let error = result.expect_err("empty payload should fail");
    assert!(error.to_string().contains("must not be empty"));
}

fn read_only_registry() -> crate::GlobalToolRegistry {
    use runtime::permission_enforcer::PermissionEnforcer;
    use runtime::PermissionPolicy;

    let policy = mvp_tool_specs().iter().fold(
        PermissionPolicy::new(runtime::PermissionMode::ReadOnly),
        |policy, spec| policy.with_tool_requirement(spec.name, spec.required_permission),
    );
    let mut registry = crate::GlobalToolRegistry::builtin();
    registry.set_enforcer(PermissionEnforcer::new(policy));
    registry
}

/// A provably read-only command (`echo`) passes the classifier, so its
/// effective requirement drops to `ReadOnly` and the call executes — the
/// CC-parity behavior that makes read-only sessions usable for analysis.
#[test]
fn given_read_only_enforcer_when_read_only_safe_bash_then_allowed() {
    let registry = read_only_registry();
    let output = registry
        .execute("bash", &json!({ "command": "echo zo-readonly-ok" }))
        .expect("read-only-safe bash must run in read-only mode");
    assert!(
        output.contains("zo-readonly-ok"),
        "command output should round-trip: {output}"
    );
}

/// A mutating command still fails in read-only mode, now with the
/// command-intent gate's specific reason instead of the generic tool-level
/// mode-ladder denial.
#[test]
fn given_read_only_enforcer_when_mutating_bash_then_denied() {
    let registry = read_only_registry();
    let err = registry
        .execute("bash", &json!({ "command": "rm notes.txt" }))
        .expect_err("mutating bash should be denied in read-only mode");
    assert!(
        err.to_string().contains("not allowed in read-only mode"),
        "should carry the command-specific reason: {err}"
    );
    assert!(
        err.to_string().contains("Permission audit:"),
        "should include permission audit context: {err}"
    );
    assert!(
        err.to_string().contains("/permissions workspace-write"),
        "should name the minimal escalation: {err}"
    );
}

#[test]
fn given_read_only_enforcer_when_write_file_then_denied() {
    let registry = read_only_registry();
    let err = registry
        .execute(
            "write_file",
            &json!({ "path": "/tmp/x.txt", "content": "x" }),
        )
        .expect_err("write_file should be denied in read-only mode");
    assert!(
        err.to_string().contains("current mode is read-only"),
        "should cite active mode: {err}"
    );
    assert!(
        err.to_string().contains("Permission audit:"),
        "should include permission audit context: {err}"
    );
    assert!(
        err.to_string().contains("/permissions workspace-write"),
        "should explain explicit mode escalation: {err}"
    );
}

#[test]
fn given_read_only_enforcer_when_edit_file_then_denied() {
    let registry = read_only_registry();
    let err = registry
        .execute(
            "edit_file",
            &json!({ "path": "/tmp/x.txt", "old_string": "a", "new_string": "b" }),
        )
        .expect_err("edit_file should be denied in read-only mode");
    assert!(
        err.to_string().contains("current mode is read-only"),
        "should cite active mode: {err}"
    );
    assert!(
        err.to_string().contains("Permission audit:"),
        "should include permission audit context: {err}"
    );
    assert!(
        err.to_string().contains("/permissions workspace-write"),
        "should explain explicit mode escalation: {err}"
    );
}

#[test]
fn given_read_only_enforcer_when_read_file_then_not_permission_denied() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("perm-read");
    fs::create_dir_all(&root).expect("create root");
    let file = root.join("readable.txt");
    fs::write(&file, "content\n").expect("write test file");

    let registry = read_only_registry();
    let result = registry.execute("read_file", &json!({ "path": file.display().to_string() }));
    assert!(result.is_ok(), "read_file should be allowed: {result:?}");

    let _ = fs::remove_dir_all(root);
}

#[test]
fn given_read_only_enforcer_when_glob_search_then_not_permission_denied() {
    let registry = read_only_registry();
    // Explicit path: this asserts the PERMISSION outcome, and a bare pattern
    // resolves against the process cwd — which parallel env_lock tests move to
    // (and then delete) temp dirs, making the cwd-relative form flake.
    let result = registry.execute(
        "glob_search",
        &json!({ "pattern": "*.rs", "path": env!("CARGO_MANIFEST_DIR") }),
    );
    assert!(
        result.is_ok(),
        "glob_search should be allowed in read-only mode: {result:?}"
    );
}

/// What the enforcer says about an action under each mode.
///
/// These three used to go through a `TestingPermission` tool whose own
/// description said "Test-only tool for verifying permission enforcement
/// behavior" — a test fixture registered on the PRODUCT registry. That was
/// merely untidy until `CapabilityInvoke` let the model call any registered
/// name; it is now a `danger-full-access` surface a model can reach. The
/// property was never about the tool, so it is asserted on the enforcer that
/// actually holds it, and the tool is gone.
#[test]
fn the_enforcer_reports_what_each_mode_permits() {
    use runtime::permission_enforcer::PermissionEnforcer;
    use runtime::{PermissionMode, PermissionPolicy};

    let policy_for = |mode: PermissionMode| {
        mvp_tool_specs().iter().fold(PermissionPolicy::new(mode), |policy, spec| {
            policy.with_tool_requirement(spec.name, spec.required_permission)
        })
    };

    let danger = PermissionEnforcer::new(policy_for(PermissionMode::DangerFullAccess));
    assert_eq!(danger.active_mode(), PermissionMode::DangerFullAccess);
    assert!(danger.is_allowed("bash", ""), "danger permits a shell");
    assert!(danger.is_allowed("write_file", ""), "danger permits a write");

    let read_only = PermissionEnforcer::new(policy_for(PermissionMode::ReadOnly));
    assert_eq!(read_only.active_mode(), PermissionMode::ReadOnly);
    assert!(
        !read_only.is_allowed("write_file", ""),
        "read-only must deny a write"
    );
    assert!(
        read_only.is_allowed("glob_search", ""),
        "read-only still permits a read"
    );
}

#[test]
fn run_task_packet_creates_packet_backed_task() {
    let ctx = test_ctx();
    let result = run_task_packet(
        &ctx.tasks,
        TaskPacket {
            objective: "Ship packetized runtime task".to_string(),
            scope: "runtime/task system".to_string(),
            repo: "zo-parity".to_string(),
            branch_policy: "origin/main only".to_string(),
            acceptance_tests: vec![
                "cargo build --workspace".to_string(),
                "cargo test --workspace".to_string(),
            ],
            commit_policy: "single commit".to_string(),
            reporting_contract: "print build/test result and sha".to_string(),
            escalation_policy: "manual escalation".to_string(),
        },
    )
    .expect("task packet should create a task");

    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["status"], "created");
    assert_eq!(output["prompt"], "Ship packetized runtime task");
    assert_eq!(output["description"], "runtime/task system");
    assert_eq!(output["task_packet"]["repo"], "zo-parity");
    assert_eq!(
        output["task_packet"]["acceptance_tests"][1],
        "cargo test --workspace"
    );
}

// --- Monitor tool tests ---

#[test]
fn monitor_returns_not_found_when_no_output_file() {
    let result = run_tool("Monitor", &json!({"process_id": "nonexistent-proc-12345"}))
        .expect("Monitor should succeed even without output file");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["process_id"], "nonexistent-proc-12345");
    assert_eq!(output["line_count"], 0);
    assert_eq!(output["source"], "not_found");
}

#[test]
fn monitor_rejects_empty_process_id() {
    let error =
        run_tool("Monitor", &json!({})).expect_err("Monitor should reject missing process_id");
    assert!(
        error.to_string().contains("process_id")
            || error.to_string().contains("command")
            || error.to_string().contains("must be provided"),
        "unexpected error: {error}"
    );
}

// --- SendMessage tool tests ---

#[test]
fn send_message_reports_honest_non_delivery_for_unknown_agent() {
    let result = run_tool(
        "SendMessage",
        &json!({"to": "agent-unknown-999", "message": "hello"}),
    )
    .expect("SendMessage should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["to"], "agent-unknown-999");
    // The delivery half is real now: an unknown target is an explicit
    // non-delivery, never a "recorded, best-effort" pretense.
    assert_eq!(output["delivered"], false);
    assert_eq!(output["agentStatus"], "not_found");
    assert!(output["sentAt"].as_str().is_some());
}

#[test]
fn send_message_rejects_empty_to() {
    let error = run_tool("SendMessage", &json!({"to": "", "message": "hi"}))
        .expect_err("SendMessage should reject empty to");
    assert!(error.to_string().contains("must not be empty"));
}

#[test]
fn send_message_rejects_empty_message() {
    let error = run_tool("SendMessage", &json!({"to": "agent-1", "message": "  "}))
        .expect_err("SendMessage should reject empty message");
    assert!(error.to_string().contains("must not be empty"));
}

// --- ScheduleWakeup tool tests ---

#[test]
fn schedule_wakeup_stamps_session_and_preserves_headless_legacy_shape() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("schedule-wakeup");
    std::fs::create_dir_all(&root).expect("mkdir temp root");
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("set cwd");

    let ctx = ToolContext::new();
    ctx.set_session_id("session-a");
    let result = execute_tool(
        &ctx,
        "ScheduleWakeup",
        &json!({
            "delaySeconds": 30,
            "reason": "poll CI status",
            "prompt": "Check if the CI pipeline has finished"
        }),
    )
    .expect("ScheduleWakeup should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    let state: serde_json::Value = std::fs::read_to_string(
        output["stateFile"].as_str().expect("state file path"),
    )
    .ok()
    .and_then(|contents| serde_json::from_str(&contents).ok())
    .expect("state file json");

    let headless_result = execute_tool(
        &ToolContext::new(),
        "ScheduleWakeup",
        &json!({
            "delaySeconds": 5,
            "reason": "headless check",
            "prompt": "Run without a session slot"
        }),
    )
    .expect("headless ScheduleWakeup should succeed");
    let headless_output: serde_json::Value =
        serde_json::from_str(&headless_result).expect("headless output json");
    let headless_state: serde_json::Value = std::fs::read_to_string(
        headless_output["stateFile"]
            .as_str()
            .expect("headless state file path"),
    )
    .ok()
    .and_then(|contents| serde_json::from_str(&contents).ok())
    .expect("headless state file json");

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(output["delaySeconds"], json!(30.0));
    assert_eq!(output["reason"], "poll CI status");
    assert!(output["wakeupId"].as_str().unwrap().starts_with("wakeup-"));
    assert!(output["scheduledAt"].as_str().is_some());
    assert_eq!(state["sessionId"], "session-a");
    assert!(
        headless_state.get("sessionId").is_none(),
        "an unset session slot keeps the legacy shape"
    );
}

#[test]
fn schedule_wakeup_stop_alone_writes_a_stop_record_and_names_the_scheduler() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("schedule-wakeup-stop");
    std::fs::create_dir_all(&root).expect("mkdir temp root");
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("set cwd");

    let ctx = ToolContext::new();
    ctx.set_session_id("session-stop");
    let scheduled = execute_tool(
        &ctx,
        "ScheduleWakeup",
        &json!({"delaySeconds": 45, "reason": "poll", "prompt": "look again", "noop": true}),
    )
    .expect("schedule");
    let stopped = execute_tool(&ctx, "ScheduleWakeup", &json!({"stop": true}))
        .expect("stop alone is a complete call");
    let records = crate::wakeup_store::take_for_session(&root, "session-stop");

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = std::fs::remove_dir_all(&root);

    let scheduled: serde_json::Value = serde_json::from_str(&scheduled).expect("json");
    assert_eq!(scheduled["scheduler"], "session_idle");
    assert_eq!(scheduled["noop"], true);
    assert!(
        !scheduled.to_string().contains("not_wired"),
        "the receipt must not promise a scheduler that is not there"
    );
    let stopped: serde_json::Value = serde_json::from_str(&stopped).expect("json");
    assert_eq!(stopped["stopped"], true);
    assert_eq!(stopped["scheduler"], "session_idle");
    assert_eq!(records.len(), 2, "one reader takes both records in order");
    assert!(!records[0].stop && records[0].noop && records[0].prompt == "look again");
    assert!(records[1].stop, "the stop record comes second");
}

#[test]
fn schedule_wakeup_without_a_session_says_nothing_will_fire() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("schedule-wakeup-unowned");
    std::fs::create_dir_all(&root).expect("mkdir temp root");
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(&root).expect("set cwd");

    let result = execute_tool(
        &ToolContext::new(),
        "ScheduleWakeup",
        &json!({"delaySeconds": 5, "reason": "headless", "prompt": "nobody listens"}),
    )
    .expect("unowned ScheduleWakeup still records");

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = std::fs::remove_dir_all(&root);

    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["scheduler"], "unowned");
}

#[test]
fn schedule_wakeup_requires_a_delay_unless_stopping() {
    let error = run_tool(
        "ScheduleWakeup",
        &json!({"reason": "poll", "prompt": "do something"}),
    )
    .expect_err("a schedule without a delay is incomplete");
    assert!(error.to_string().contains("delaySeconds"));
}

#[test]
fn schedule_wakeup_rejects_empty_reason() {
    let error = run_tool(
        "ScheduleWakeup",
        &json!({"delaySeconds": 5, "reason": "", "prompt": "do something"}),
    )
    .expect_err("ScheduleWakeup should reject empty reason");
    assert!(error.to_string().contains("must not be empty"));
}

#[test]
fn schedule_wakeup_rejects_empty_prompt() {
    let error = run_tool(
        "ScheduleWakeup",
        &json!({"delaySeconds": 5, "reason": "test", "prompt": "  "}),
    )
    .expect_err("ScheduleWakeup should reject empty prompt");
    assert!(error.to_string().contains("must not be empty"));
}
