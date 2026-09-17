//! Bash, `PowerShell`, and REPL execution paths.
//!
//! Split out of the old flat `tests.rs` (4,133 lines) by domain;
//! shared fixtures live in the parent module.

use super::*;

#[test]
fn bash_tool_reports_success_exit_failure_timeout_and_background() {
    let cwd = sandbox_disabled_cwd("bash-structured-cwd");
    let success = run_tool_in_cwd("bash", &json!({ "command": "printf 'hello'" }), &cwd)
        .expect("bash should succeed");
    let success_output: serde_json::Value = serde_json::from_str(&success).expect("json");
    assert_eq!(success_output["stdout"], "hello");
    assert_eq!(success_output["interrupted"], false);

    let failure = run_tool_in_cwd(
        "bash",
        &json!({ "command": "printf 'oops' >&2; exit 7" }),
        &cwd,
    )
    .expect("bash failure should still return structured output");
    let failure_output: serde_json::Value = serde_json::from_str(&failure).expect("json");
    assert_eq!(failure_output["returnCodeInterpretation"], "exit_code:7");
    assert!(failure_output["stderr"]
        .as_str()
        .expect("stderr")
        .contains("oops"));

    // `timeout` is milliseconds; values below MIN_BASH_TIMEOUT_MS (1s) are
    // treated as a ms/s unit slip and fall back to the default, so use a
    // genuine 1s deadline against a command that runs well past it.
    let timeout = run_tool_in_cwd(
        "bash",
        &json!({ "command": "sleep 30", "timeout": 1000 }),
        &cwd,
    )
    .expect("bash timeout should return output");
    let timeout_output: serde_json::Value = serde_json::from_str(&timeout).expect("json");
    assert_eq!(timeout_output["interrupted"], true);
    assert_eq!(timeout_output["returnCodeInterpretation"], "timeout");
    assert!(timeout_output["stderr"]
        .as_str()
        .expect("stderr")
        .contains("Command exceeded timeout"));

    let background = run_tool_in_cwd(
        "bash",
        &json!({ "command": "sleep 1", "run_in_background": true }),
        &cwd,
    )
    .expect("bash background should succeed");
    let background_output: serde_json::Value = serde_json::from_str(&background).expect("json");
    // The dispatch path wires the shared TaskRegistry, so a background run
    // returns a pollable `task_…` id and its output IS retrievable via
    // TaskOutput — `noOutputExpected` is therefore false (registry contract;
    // the bare-PID/discarded-output legacy shape only applies without a
    // registry in scope).
    let task_id = background_output["backgroundTaskId"]
        .as_str()
        .expect("background task id");
    assert!(
        task_id.starts_with("task_"),
        "registry-backed id expected, got {task_id}"
    );
    assert_eq!(background_output["noOutputExpected"], false);
    let _ = fs::remove_dir_all(cwd);
}

#[test]
fn bash_background_launch_uses_session_and_stop_waits_for_reap() {
    let cwd = sandbox_disabled_cwd("bash-background-session-cwd");
    let ctx = ToolContext::new().with_cwd(cwd.clone());
    ctx.set_session_id("visible-session");
    let live = ctx
        .tasks
        .live_background_process_count(Some("visible-session"));

    let output = execute_tool(
        &ctx,
        "bash",
        &json!({ "command": "sleep 30", "run_in_background": true }),
    )
    .expect("background bash should launch");
    let output: serde_json::Value = serde_json::from_str(&output).expect("json");
    let task_id = output["backgroundTaskId"]
        .as_str()
        .expect("background task id");
    assert_eq!(live.load(), 1, "launch is stamped to ToolContext session");

    execute_tool(&ctx, "TaskStop", &json!({ "task_id": task_id }))
        .expect("TaskStop should stop the process task");
    assert_eq!(
        live.load(),
        1,
        "TaskStop does not decrement before the watcher reaps the child"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while live.load() != 0 && std::time::Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let task = ctx.tasks.get(task_id);
    assert_eq!(
        live.load(),
        0,
        "the watcher confirms process exit once; task={task:?}"
    );

    let _ = fs::remove_dir_all(cwd);
}

#[test]
fn bash_tool_surfaces_destructive_safety_warning() {
    // A command matching a known destructive pattern still runs but
    // carries a non-blocking advisory on the structured result.
    let cwd = sandbox_disabled_cwd("bash-warning-cwd");
    let dangerous = run_tool_in_cwd("bash", &json!({ "command": "printf 'rm -rf /'" }), &cwd)
        .expect("bash should run");
    let output: serde_json::Value = serde_json::from_str(&dangerous).expect("json");
    assert!(
        output["safetyWarning"]
            .as_str()
            .is_some_and(|warning| warning.to_lowercase().contains("destructive")),
        "expected a destructive safety warning, got: {output}"
    );

    // A benign command omits the advisory entirely (skip_serializing_if).
    let benign = run_tool_in_cwd("bash", &json!({ "command": "printf 'hello'" }), &cwd)
        .expect("bash should run");
    let benign_output: serde_json::Value = serde_json::from_str(&benign).expect("json");
    assert!(benign_output.get("safetyWarning").is_none());
    let _ = fs::remove_dir_all(cwd);
}

#[test]
fn audit_tool_summarizes_the_invocation_ledger() {
    // WI-E2: the Audit tool turns the otherwise write-only shadow ledger into a
    // readable rollup. Share one context so every dispatch lands in its ledger.
    let ctx = ToolContext::new();
    // A succeeding read-only tool, then a dispatch that fails (unknown tool).
    let _ = execute_tool(&ctx, "ToolSearch", &json!({ "query": "read" }));
    let unknown = execute_tool(&ctx, "DefinitelyNotATool", &json!({}));
    assert!(unknown.is_err(), "unknown tool should fail");

    // run_audit reads the ledger before the current Audit call is recorded, so
    // it sees the two prior invocations.
    let out = execute_tool(&ctx, "Audit", &json!({})).expect("audit runs");
    let summary: serde_json::Value = serde_json::from_str(&out).expect("audit json");

    assert!(
        summary["total"].as_u64().expect("total") >= 2,
        "audit should count prior invocations, got {summary}"
    );
    assert!(
        summary["failed"].as_u64().expect("failed") >= 1,
        "the unknown-tool dispatch should show as failed, got {summary}"
    );
    assert!(
        summary["succeeded"].as_u64().expect("succeeded") >= 1,
        "the ToolSearch dispatch should show as succeeded, got {summary}"
    );
}

#[test]
fn bash_workspace_tests_are_blocked_when_branch_is_behind_main() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("workspace-test-preflight");
    let original_dir = std::env::current_dir().expect("cwd");
    init_git_repo(&root);
    run_git(&root, &["checkout", "-b", "feature/stale-tests"]);
    run_git(&root, &["checkout", "main"]);
    commit_file(
        &root,
        "hotfix.txt",
        "fix from main\n",
        "fix: unblock workspace tests",
    );
    run_git(&root, &["checkout", "feature/stale-tests"]);
    std::env::set_current_dir(&root).expect("set cwd");

    let output = run_tool(
        "bash",
        &json!({ "command": "cargo test --workspace --all-targets" }),
    )
    .expect("preflight should return structured output");
    let output_json: serde_json::Value = serde_json::from_str(&output).expect("json");
    assert_eq!(
        output_json["returnCodeInterpretation"],
        "preflight_blocked:branch_divergence"
    );
    assert!(output_json["stderr"]
        .as_str()
        .expect("stderr")
        .contains("branch divergence detected before workspace tests"));
    assert_eq!(
        output_json["structuredContent"][0]["event"],
        "branch.stale_against_main"
    );
    assert_eq!(
        output_json["structuredContent"][0]["failureClass"],
        "branch_divergence"
    );
    assert_eq!(
        output_json["structuredContent"][0]["data"]["missingCommits"][0],
        "fix: unblock workspace tests"
    );

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn bash_targeted_tests_skip_branch_preflight() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let root = temp_path("targeted-test-no-preflight");
    let original_dir = std::env::current_dir().expect("cwd");
    init_git_repo(&root);
    run_git(&root, &["checkout", "-b", "feature/targeted-tests"]);
    run_git(&root, &["checkout", "main"]);
    commit_file(
        &root,
        "hotfix.txt",
        "fix from main\n",
        "fix: only broad tests should block",
    );
    run_git(&root, &["checkout", "feature/targeted-tests"]);

    fs::create_dir_all(root.join(".zo")).expect("create sandbox config dir");
    fs::write(
        root.join(".zo").join("settings.json"),
        r#"{"sandbox":{"enabled":false}}"#,
    )
    .expect("write sandbox settings");
    let output = run_tool_in_cwd(
        "bash",
        &json!({ "command": "printf 'targeted ok'; cargo test -p runtime stale_branch" }),
        &root,
    )
    .expect("targeted commands should still execute");
    let output_json: serde_json::Value = serde_json::from_str(&output).expect("json");
    assert_ne!(
        output_json["returnCodeInterpretation"],
        "preflight_blocked:branch_divergence"
    );

    std::env::set_current_dir(&original_dir).expect("restore cwd");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn repl_executes_python_code_in_a_persistent_kernel() {
    let result = run_tool(
        "REPL",
        &json!({"language": "python", "code": "print(1 + 1)", "timeout_ms": 20000}),
    )
    .expect("REPL should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["language"], "python");
    assert_eq!(output["ok"], true);
    assert_eq!(output["kernel"]["persistent"], true);
    assert!(output["stdout"].as_str().expect("stdout").contains('2'));

    // The whole point of the kernel: dispatch-level calls share one namespace,
    // so a variable from an earlier call answers a later expression.
    run_tool("REPL", &json!({"language": "python", "code": "seen = 123"}))
        .expect("assignment should succeed");
    let result = run_tool("REPL", &json!({"language": "python", "code": "seen + 1"}))
        .expect("follow-up should succeed");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["value"], "124");
}

#[test]
fn given_empty_code_when_repl_then_rejects_with_error() {
    let result = run_tool("REPL", &json!({"language": "python", "code": "   "}));

    let error = result.expect_err("empty REPL code should fail");
    assert!(error.to_string().contains("code must not be empty"));
}

#[test]
fn given_unsupported_language_when_repl_then_rejects_with_error() {
    let result = run_tool("REPL", &json!({"language": "ruby", "code": "puts 1"}));

    let error = result.expect_err("unsupported REPL language should fail");
    assert!(error
        .to_string()
        .contains("unsupported REPL language: ruby"));
}

#[test]
fn given_timeout_ms_when_repl_blocks_then_returns_timeout_error() {
    let result = run_tool(
        "REPL",
        &json!({
            "language": "python",
            "code": "import time\ntime.sleep(1)",
            "timeout_ms": 10
        }),
    );

    let error = result.expect_err("timed out REPL execution should fail");
    let message = error.to_string();
    assert!(
        message.contains("exceeded timeout of 10 ms"),
        "timeout wording expected: {message}"
    );
    // The persistent kernel is discarded on timeout and the model is told its
    // state is gone — the honest half of the persistence contract.
    assert!(
        message.contains("variables are lost"),
        "state-loss notice expected: {message}"
    );
}

#[test]
fn powershell_runs_via_stub_shell() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = temp_path("pwsh-bin");
    let cwd = temp_path("pwsh-cwd");
    std::fs::create_dir_all(&dir).expect("create dir");
    std::fs::create_dir_all(cwd.join(".zo")).expect("create cwd config dir");
    std::fs::write(
        cwd.join(".zo").join("settings.json"),
        r#"{"sandbox":{"enabled":false}}"#,
    )
    .expect("write sandbox settings");
    let stub = r#"#!/bin/sh
while [ "$1" != "-Command" ] && [ $# -gt 0 ]; do shift; done
shift
printf 'pwsh:%s' "$1"
"#;
    for name in ["pwsh", "powershell"] {
        let script = dir.join(name);
        std::fs::write(&script, stub).expect("write script");
        std::process::Command::new("/bin/chmod")
            .arg("+x")
            .arg(&script)
            .status()
            .expect("chmod");
    }
    let original_path = std::env::var("PATH").unwrap_or_default();
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_var("PATH", format!("{}:/bin:/usr/bin", dir.display()));
    std::env::set_current_dir(&cwd).expect("set cwd");

    let result = run_tool_isolated("PowerShell", &json!({"command": "Write-Output hello"}))
        .expect("PowerShell should succeed");

    let background = run_tool_isolated(
        "PowerShell",
        &json!({"command": "Write-Output hello", "run_in_background": true}),
    )
    .expect("PowerShell background should succeed");

    std::env::set_current_dir(original_dir).expect("restore cwd");
    std::env::set_var("PATH", original_path);
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(cwd);

    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["stdout"], "pwsh:Write-Output hello");
    assert!(output["stderr"].as_str().expect("stderr").is_empty());

    let background_output: serde_json::Value = serde_json::from_str(&background).expect("json");
    assert!(background_output["backgroundTaskId"].as_str().is_some());
    assert_eq!(background_output["backgroundedByUser"], true);
    assert_eq!(background_output["assistantAutoBackgrounded"], false);
}

#[test]
fn powershell_fails_closed_when_sandbox_fallback_is_reported() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = temp_path("pwsh-fallback-bin");
    let cwd = temp_path("pwsh-fallback-cwd");
    let marker = cwd.join("marker");
    std::fs::create_dir_all(&dir).expect("create dir");
    std::fs::create_dir_all(cwd.join(".zo")).expect("create cwd config dir");
    std::fs::write(
        cwd.join(".zo").join("settings.json"),
        r#"{"sandbox":{"enabled":true,"namespaceRestrictions":false,"networkIsolation":false,"filesystemMode":"allow-list","allowedMounts":[]}}"#,
    )
    .expect("write sandbox settings");
    // Project-scope `sandbox` is now supply-chain gated (a repo could otherwise
    // DISABLE the sandbox), so the operator opts it in from the trusted User
    // config home. `default_for` reads ZO_CONFIG_HOME as the User scope.
    let config_home = temp_path("pwsh-fallback-home");
    std::fs::create_dir_all(&config_home).expect("create config home");
    std::fs::write(
        config_home.join("settings.json"),
        r#"{"enableAllProjectSandbox":true}"#,
    )
    .expect("write sandbox opt-in");
    let original_config_home = std::env::var("ZO_CONFIG_HOME").ok();
    std::env::set_var("ZO_CONFIG_HOME", &config_home);
    let stub = r#"#!/bin/sh
while [ "$1" != "-Command" ] && [ $# -gt 0 ]; do shift; done
shift
printf 'ran' > marker
printf 'pwsh:%s' "$1"
"#;
    for name in ["pwsh", "powershell"] {
        let script = dir.join(name);
        std::fs::write(&script, stub).expect("write script");
        std::process::Command::new("/bin/chmod")
            .arg("+x")
            .arg(&script)
            .status()
            .expect("chmod");
    }
    let original_path = std::env::var("PATH").unwrap_or_default();
    let original_dir = std::env::current_dir().expect("cwd");
    std::env::set_var("PATH", format!("{}:/bin:/usr/bin", dir.display()));
    std::env::set_current_dir(&cwd).expect("set cwd");

    let err = run_tool_isolated("PowerShell", &json!({"command": "Write-Output hello"}))
        .expect_err("PowerShell should fail before spawning when sandbox is unavailable");
    let marker_exists = marker.exists();

    std::env::set_current_dir(original_dir).expect("restore cwd");
    std::env::set_var("PATH", original_path);
    match original_config_home {
        Some(value) => std::env::set_var("ZO_CONFIG_HOME", value),
        None => std::env::remove_var("ZO_CONFIG_HOME"),
    }
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(cwd);
    let _ = std::fs::remove_dir_all(config_home);

    let error = err.to_string();
    assert!(error.contains("sandbox requested but unavailable"));
    assert!(error.contains("filesystem allow-list requested without configured mounts"));
    assert!(error.contains("sandbox.enabled=false"));
    assert!(!marker_exists, "PowerShell stub must not be spawned");
}

#[test]
fn powershell_errors_when_shell_is_missing() {
    let _guard = env_lock()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let original_path = std::env::var("PATH").unwrap_or_default();
    let empty_dir = std::env::temp_dir().join(format!(
        "zo-empty-bin-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    ));
    std::fs::create_dir_all(&empty_dir).expect("create empty dir");
    std::env::set_var("PATH", empty_dir.display().to_string());

    let err = run_tool_isolated("PowerShell", &json!({"command": "Write-Output hello"}))
        .expect_err("PowerShell should fail when shell is missing");

    std::env::set_var("PATH", original_path);
    let _ = std::fs::remove_dir_all(empty_dir);

    assert!(err.to_string().contains("PowerShell executable not found"));
}

#[test]
fn given_no_enforcer_when_bash_then_executes_normally() {
    let cwd = sandbox_disabled_cwd("bash-no-enforcer-cwd");
    let mut ctx = ToolContext::new();
    ctx.cwd = Some(cwd.clone());
    let registry = crate::GlobalToolRegistry::builtin().with_context(ctx);
    let result = registry
        .execute("bash", &json!({ "command": "printf 'ok'" }))
        .expect("bash should succeed without enforcer");
    let output: serde_json::Value = serde_json::from_str(&result).expect("json");
    assert_eq!(output["stdout"], "ok");
    let _ = fs::remove_dir_all(cwd);
}
