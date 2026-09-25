use std::{fs, path::Path, sync::Arc};

use plugins::{PluginHooks, PluginRegistry};
use runtime::ConfigLoader;
use tools::{GlobalToolRegistry, ToolContext};

use super::{
    build_runtime_lsp_state, build_runtime_mcp_state, tool_toggles::load_disabled_tool_names,
    BuiltRuntime, RuntimePluginState,
};
use crate::cli_args::AllowedToolSet;

pub(crate) fn build_runtime_plugin_state_with_loader(
    cwd: &Path,
    loader: &ConfigLoader,
    runtime_config: &runtime::RuntimeConfig,
    tasks: Option<runtime::task_registry::TaskRegistry>,
    // The model this runtime is being built for. Memory recall prefers entries
    // this model authored, and `/model` rebuilds the runtime, so the model at
    // build time is the model for the whole life of the retriever.
    active_model: Option<&str>,
) -> Result<RuntimePluginState, Box<dyn std::error::Error>> {
    let stage = std::time::Instant::now();
    let plugin_manager = crate::build_plugin_manager(cwd, loader, runtime_config);
    let plugin_registry: PluginRegistry = plugin_manager.plugin_registry()?;
    crate::runtime_support::log_boot_stage("plugin-state/plugins", stage);
    // Seated before the retriever, which asks it on every recall what
    // readers did with the pages it recalled before (t-6264) — the seat
    // reads its own rows and the road the person chose, so a record-only
    // mode ranks exactly as before.
    let recall_seat = Arc::new(tools::RerankShadow::at(cwd));
    let memory_retriever = runtime_config
        .auto_memory_enabled()
        .then(|| load_runtime_memory_retriever(cwd, active_model, Arc::clone(&recall_seat)))
        .flatten();
    let plugin_hook_config =
        runtime_hook_config_from_plugin_hooks(plugin_registry.aggregated_hooks()?);
    let feature_config = runtime_config
        .feature_config()
        .clone()
        .with_hooks(runtime_config.hooks().merged(&plugin_hook_config));
    crate::runtime_support::log_boot_stage("plugin-state/memory-hooks", stage);
    let force_mcp = super::mcp_runtime::eager_mcp_enabled();
    let has_mcp_config = !runtime_config.mcp().servers().is_empty();
    let (mcp_state, runtime_tools) = if force_mcp || has_mcp_config {
        build_runtime_mcp_state(runtime_config)
    } else {
        (None, Vec::new())
    };
    crate::runtime_support::log_boot_stage("plugin-state/mcp", stage);
    let disabled_tools = load_disabled_tool_names(cwd)?;
    crate::runtime_support::log_boot_stage("plugin-state/tool-toggles", stage);
    let (lsp_state, lsp_registry) = build_runtime_lsp_state(cwd, runtime_config)?;
    crate::runtime_support::log_boot_stage("plugin-state/lsp", stage);
    let hook_config = feature_config.hooks().clone();
    let mut tool_context = tasks.map_or_else(ToolContext::new, |tasks| {
        ToolContext::new().with_tasks(tasks)
    })
        .with_workspace_root(cwd.to_path_buf())
        .with_disabled_tools(disabled_tools)
        .with_hook_config(hook_config);
    tool_context.lsp = lsp_registry;
    // Bridge background bash task completion into the agent push-notification
    // path: when a `run_in_background` task reaches a terminal status, re-inject
    // its result as a follow-up turn (parity with background agents). The
    // `runtime` watcher cannot call `tools`, so the session installs this
    // callback; only Completed/Failed re-inject (Stopped is skipped, like agents).
    let task_completion: runtime::task_registry::TaskCompletionCallback = Box::new(
        |task_id: String,
         status: runtime::task_registry::TaskStatus,
         output: String,
         session_id: Option<String>| {
            let status_str = match status {
                runtime::task_registry::TaskStatus::Completed => "completed",
                runtime::task_registry::TaskStatus::Failed => "failed",
                _ => return,
            };
            let result = (!output.trim().is_empty()).then_some(output);
            tools::notify_background_task_completion(task_id, status_str, result, session_id);
        },
    );
    tool_context
        .tasks
        .set_completion_callback(Some(std::sync::Arc::new(task_completion)));
    let tool_registry: GlobalToolRegistry =
        GlobalToolRegistry::with_plugin_tools(plugin_registry.aggregated_tools()?)
            .map_err(|e| e.to_string())?
            .with_runtime_tools(runtime_tools)
            .map_err(|e| e.to_string())?
            .with_context(tool_context);
    crate::runtime_support::log_boot_stage("plugin-state/tool-registry", stage);
    Ok(RuntimePluginState {
        feature_config,
        tool_registry,
        plugin_registry,
        memory_retriever,
        // Seated whenever there is a retriever to sit beside; what it may do
        // per recall — record, or settle the order — is `smart.rerankShadow`.
        recall_seat: Some(recall_seat),
        // Seated always; what it may do per compaction — record, or drop —
        // is `smart.jevCompaction`, read at the boundary.
        compaction_seat: Some(Arc::new(tools::CompactionJudge::at(cwd))),
        // Seated always; what it may do per patch — record, or add a line to
        // the result — is `smart.jevPatchReview`, read at the edit.
        patch_review_seat: Some(Arc::new(tools::PatchReviewJudge::at(cwd))),
        // Seated always; what the two guards may do per call — record, or add
        // a line and a fence — is `smart.jevCommandGuard` and
        // `smart.jevToolTextGuard`, read at the call.
        tool_guard_seat: Some(Arc::new(tools::ToolGuardJudge::at(cwd))),
        // Seated at every public prompt boundary; its default mode records
        // candidate rankings until the same turn's edited files label them.
        file_pick_seat: Some(Arc::new(tools::FilePickJudge::at(cwd))),
        skill_suggestion_seat: Some(Arc::new(tools::SkillSuggestionJudge::at(cwd))),
        mcp_state,
        lsp_state,
    })
}

fn load_runtime_memory_retriever(
    cwd: &Path,
    active_model: Option<&str>,
    demand: Arc<tools::RerankShadow>,
) -> Option<std::sync::Arc<dyn runtime::MemoryRetriever + Send + Sync>> {
    runtime::load_memory_retriever(cwd, active_model, Some(demand))
        .or_else(|| load_legacy_project_memory_retriever(cwd, active_model))
}

fn load_legacy_project_memory_retriever(
    cwd: &Path,
    active_model: Option<&str>,
) -> Option<std::sync::Arc<dyn runtime::MemoryRetriever + Send + Sync>> {
    let legacy_dir = cwd.ancestors().find_map(|dir| {
        let candidate = dir
            .join(".zo")
            .join(runtime::memory::paths::MEMORY_STORE);
        candidate
            .join(runtime::memory::paths::MEMORY_INDEX_FILE)
            .exists()
            .then_some(candidate)
    })?;
    let index_path = legacy_dir.join(runtime::memory::paths::MEMORY_INDEX_FILE);
    let markdown = fs::read_to_string(index_path).ok()?;
    let entries = runtime::memory::parse_memory_index(&markdown);
    (!entries.is_empty()).then(|| {
        // The pre-global `.zo/memory` store is read-only history — nothing
        // writes there any more — so this one keeps its snapshot. It still
        // honors authorship for any entry that carries it.
        std::sync::Arc::new(
            runtime::memory::LexicalMemoryRetriever::new(entries)
                .with_active_model(active_model),
        ) as std::sync::Arc<dyn runtime::MemoryRetriever + Send + Sync>
    })
}

pub(crate) fn runtime_hook_config_from_plugin_hooks(
    hooks: PluginHooks,
) -> runtime::RuntimeHookConfig {
    runtime::RuntimeHookConfig::new(
        quote_plugin_hook_commands(hooks.pre_tool_use),
        quote_plugin_hook_commands(hooks.post_tool_use),
        quote_plugin_hook_commands(hooks.post_tool_use_failure),
    )
}

fn quote_plugin_hook_commands(commands: Vec<String>) -> Vec<String> {
    commands
        .into_iter()
        .map(|command| shell_quote_plugin_path(&command))
        .collect()
}

/// Build one alternate live client while preserving the foreground session's
/// tool registry, wire advertisement, auth route, and provider cache scope.
/// An unavailable credential/provider degrades to `None`; the caller then
/// clears that turn's optional runtime slot.
pub(crate) fn build_smart_live_client(
    runtime: &BuiltRuntime,
    session_id: &str,
    allowed_tools: Option<AllowedToolSet>,
    model: &str,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
) -> Option<Arc<dyn runtime::AsyncApiClient>> {
    let auth_route = crate::runtime_support::catalog_auth_route_for_model(model);
    let provider = if let Some(provider_kind) =
        crate::runtime_support::catalog_provider_for_model(model)
    {
        api::ProviderClient::from_provider_kind_with_auth_route(provider_kind, auth_route)
    } else {
        api::ProviderClient::from_model_with_auth_route(model, auth_route)
    }
    .ok()?
    .with_cache_scope(session_id);
    let main = runtime.api_client();
    Some(Arc::new(super::runtime_bridge::LiveAsyncApiClient::new(
        provider,
        model.to_string(),
        auth_route,
        main.enable_tools(),
        allowed_tools,
        main.tool_registry(),
        None,
        named_effort,
        effort_band_ceiling,
    )))
}

// The two arms must take the same argument: this one is only ever compiled on
// Windows, so a signature that drifted from its `cfg(not(windows))` twin went
// unnoticed until the first cross-check.
#[cfg(windows)]
fn shell_quote_plugin_path(command: &str) -> String {
    format!("\"{}\"", command.replace('"', "\"\""))
}

#[cfg(not(windows))]
fn shell_quote_plugin_path(command: &str) -> String {
    format!("'{}'", command.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;
    

    use super::{build_runtime_plugin_state_with_loader, runtime_hook_config_from_plugin_hooks};
    use crate::session::RuntimePluginState;
    use plugins::PluginHooks;
    use runtime::ConfigLoader;
    use runtime::HookRule;
    use serde_json::json;
    use tokio::time::timeout;

    fn mark_test_script_executable(path: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            let mut permissions = fs::metadata(path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).expect("chmod");
        }
        #[cfg(windows)]
        {
            // Windows has no executable mode bit. Assert the generated script
            // exists as a regular file; its interpreter/wrapper determines
            // launchability rather than Unix 0755.
            assert!(fs::metadata(path).expect("metadata").is_file());
        }
    }

    fn write_lsp_server_script() -> PathBuf {
        let root = crate::support::temp_dir("lsp-script");
        fs::create_dir_all(&root).expect("temp dir");
        let script_path = root.join("fake-lsp-server.py");
        let script = [
            "#!/usr/bin/env python3",
            "import json, sys",
            "",
            "def read_message():",
            "    header = b''",
            r"    while not header.endswith(b'\r\n\r\n'):",
            "        chunk = sys.stdin.buffer.read(1)",
            "        if not chunk:",
            "            return None",
            "        header += chunk",
            "    length = 0",
            r"    for line in header.decode().split('\r\n'):",
            r"        if line.lower().startswith('content-length:'):",
            r"            length = int(line.split(':', 1)[1].strip())",
            "    payload = sys.stdin.buffer.read(length)",
            "    return json.loads(payload.decode())",
            "",
            "def send_message(message):",
            "    payload = json.dumps(message).encode()",
            r"    sys.stdout.buffer.write(f'Content-Length: {len(payload)}\r\n\r\n'.encode() + payload)",
            "    sys.stdout.buffer.flush()",
            "",
            "while True:",
            "    request = read_message()",
            "    if request is None:",
            "        break",
            "    method = request['method']",
            "    if 'id' not in request:",
            "        continue",
            "    if method == 'initialize':",
            "        send_message({'jsonrpc': '2.0', 'id': request['id'], 'result': {'capabilities': {}}})",
            "    elif method == 'textDocument/hover':",
            "        send_message({'jsonrpc': '2.0', 'id': request['id'], 'result': {'contents': {'kind': 'plaintext', 'value': 'builder hover'}}})",
            "    else:",
            "        send_message({'jsonrpc': '2.0', 'id': request['id'], 'error': {'code': -32601, 'message': method}})",
            "",
        ]
        .join("\n");
        fs::write(&script_path, script).expect("write script");
        mark_test_script_executable(&script_path);
        script_path
    }

    #[test]
    fn runtime_hook_config_preserves_all_hook_vectors() {
        let hooks = PluginHooks {
            pre_tool_use: vec!["/tmp/plugin hooks/pre-a.sh".to_string()],
            post_tool_use: vec!["/tmp/plugin hooks/post-a.sh".to_string()],
            post_tool_use_failure: vec!["/tmp/plugin's hooks/fail-a.sh".to_string()],
        };
        let config = runtime_hook_config_from_plugin_hooks(hooks);
        let expected_pre = if cfg!(windows) {
            "\"/tmp/plugin hooks/pre-a.sh\""
        } else {
            "'/tmp/plugin hooks/pre-a.sh'"
        };
        let expected_post = if cfg!(windows) {
            "\"/tmp/plugin hooks/post-a.sh\""
        } else {
            "'/tmp/plugin hooks/post-a.sh'"
        };
        let expected_failure = if cfg!(windows) {
            "\"/tmp/plugin's hooks/fail-a.sh\""
        } else {
            "'/tmp/plugin'\\''s hooks/fail-a.sh'"
        };
        assert_eq!(config.pre_tool_use(), &[HookRule::any(expected_pre)]);
        assert_eq!(config.post_tool_use(), &[HookRule::any(expected_post)]);
        assert_eq!(
            config.post_tool_use_failure(),
            &[HookRule::any(expected_failure)]
        );
    }

    // Regression: this test previously built a standalone current-thread
    // runtime to drive dispatch, which accidentally hid the fact that
    // `build_runtime_plugin_state_with_loader` panicked when called from an
    // ambient tokio runtime. It now runs inside a multi-thread `tokio::test`
    // and calls the sync builder through `spawn_blocking`, which is what real
    // callers do.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn build_runtime_plugin_state_registers_configured_lsp_servers() {
        let root = crate::support::temp_dir("config");
        let cwd = root.join("project");
        let home = root.join("home").join(".zo");
        fs::create_dir_all(cwd.join(".zo")).expect("project config dir");
        fs::create_dir_all(&home).expect("home config dir");

        let script_path = write_lsp_server_script();
        fs::write(
            home.join("settings.json"),
            format!(
                r#"{{
                  "lspServers": {{
                    "fake-rust": {{
                      "language": "rust",
                      "command": "{}",
                      "capabilities": ["hover"]
                    }}
                  }}
                }}"#,
                script_path.display()
            ),
        )
        .expect("write lsp settings");

        let cwd_task = cwd.clone();
        let home_task = home.clone();
        let RuntimePluginState {
            tool_registry,
            lsp_state,
                ..
        } = tokio::task::spawn_blocking(move || {
            let loader = ConfigLoader::new(&cwd_task, &home_task);
            let runtime_config = loader.load().expect("config should load");
            build_runtime_plugin_state_with_loader(&cwd_task, &loader, &runtime_config, None, None)
                .expect("runtime state should build")
        })
        .await
        .expect("spawn_blocking join");

        let hover = timeout(
            Duration::from_secs(1),
            tool_registry.context().lsp.dispatch(
                "hover",
                Some("src/main.rs"),
                Some(1),
                Some(0),
                None,
            ),
        )
        .await
        .expect("hover dispatch should not hang")
        .expect("hover should dispatch through configured lsp server");
        // hover is now normalized to {content, language} (raw `contents` gone).
        assert_eq!(hover["content"], json!("builder hover"));

        if let Some(state) = lsp_state {
            tokio::task::spawn_blocking(move || {
                state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .shutdown()
                    .expect("lsp shutdown");
            })
            .await
            .expect("shutdown join");
        }

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }


    #[test]
    fn build_runtime_plugin_state_loads_project_tool_toggles() {
        let root = crate::support::temp_dir("tool-toggles");
        let cwd = root.join("project");
        let home = root.join("home").join(".zo");
        fs::create_dir_all(cwd.join(".zo")).expect("project config dir");
        fs::create_dir_all(&home).expect("home config dir");
        fs::write(
            cwd.join(".zo").join("tool-toggles.json"),
            r#"{
              "disabled_tools": ["web_search"],
              "disabled_mcp_tools": [
                { "server_id": "alpha", "tool_name": "echo" }
              ]
            }"#,
        )
        .expect("write tool toggles");

        let loader = ConfigLoader::new(&cwd, &home);
        let runtime_config = loader.load().expect("config should load");
        let RuntimePluginState { tool_registry, .. } =
            build_runtime_plugin_state_with_loader(&cwd, &loader, &runtime_config, None, None)
                .expect("runtime state should build");

        assert!(tool_registry.is_tool_disabled("WebSearch"));
        assert!(tool_registry.is_tool_disabled("mcp__alpha__echo"));
        assert!(
            !tool_registry
                .definitions(None)
                .iter()
                .any(|definition| definition.name == "WebSearch"),
            "disabled builtin is not advertised after runtime build"
        );
        let error = tool_registry
            .execute("WebSearch", &json!({}))
            .expect_err("disabled builtin should be rejected");
        assert!(
            matches!(error, tools::ToolError::PermissionDenied { .. }),
            "disabled builtin should return PermissionDenied, got {error:?}"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn build_runtime_plugin_state_loads_project_memory_retriever() {
        let root = crate::support::temp_dir("memory-retriever");
        let cwd = root.join("project").join("nested");
        let home = root.join("home").join(".zo");
        let memory_dir = root.join("project").join(".zo").join("memory");
        fs::create_dir_all(&cwd).expect("cwd dir");
        fs::create_dir_all(&home).expect("home config dir");
        fs::create_dir_all(&memory_dir).expect("memory dir");
        fs::write(
            memory_dir.join("MEMORY.md"),
            "- [agent-eval-harness-fairness](agent-eval-harness-fairness.md) — permission-denial false-positive fairness fix\n",
        )
        .expect("write memory index");

        let loader = ConfigLoader::new(&cwd, &home);
        let runtime_config = loader.load().expect("config should load");
        let RuntimePluginState {
            memory_retriever, ..
        } = build_runtime_plugin_state_with_loader(&cwd, &loader, &runtime_config, None, None)
            .expect("runtime state should build");

        let hits = memory_retriever
            .expect("memory retriever should load")
            .recall("permission-denial false-positive", 1);
        assert_eq!(hits[0].entry.slug, "agent-eval-harness-fairness");

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn build_runtime_plugin_state_skips_memory_retriever_when_disabled() {
        let root = crate::support::temp_dir("memory-retriever-disabled");
        let cwd = root.join("project").join("nested");
        let home = root.join("home").join(".zo");
        let memory_dir = root.join("project").join(".zo").join("memory");
        fs::create_dir_all(&cwd).expect("cwd dir");
        fs::create_dir_all(&home).expect("home config dir");
        fs::create_dir_all(&memory_dir).expect("memory dir");
        fs::write(
            home.join("settings.json"),
            r#"{"autoMemoryEnabled": false}"#,
        )
        .expect("write settings");
        fs::write(
            memory_dir.join("MEMORY.md"),
            "- [agent-eval-harness-fairness](agent-eval-harness-fairness.md) — should not load\n",
        )
        .expect("write memory index");

        let loader = ConfigLoader::new(&cwd, &home);
        let runtime_config = loader.load().expect("config should load");
        assert!(!runtime_config.auto_memory_enabled());
        let RuntimePluginState {
            memory_retriever, ..
        } = build_runtime_plugin_state_with_loader(&cwd, &loader, &runtime_config, None, None)
            .expect("runtime state should build");

        assert!(memory_retriever.is_none());
        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn runtime_rebuilds_reuse_the_session_task_registry() {
        let root = crate::support::temp_dir("shared-task-registry");
        let cwd = root.join("project");
        let home = root.join("home").join(".zo");
        fs::create_dir_all(&cwd).expect("cwd dir");
        fs::create_dir_all(&home).expect("home config dir");
        let loader = ConfigLoader::new(&cwd, &home);
        let runtime_config = loader.load().expect("config should load");
        let tasks = runtime::task_registry::TaskRegistry::new_in_memory();
        let task = tasks.create_background_process("sleep 30", None, Some("session-a"));

        let first = build_runtime_plugin_state_with_loader(
            &cwd,
            &loader,
            &runtime_config,
            Some(tasks.clone()),
            None,
        )
        .expect("first runtime state should build");
        let second = build_runtime_plugin_state_with_loader(
            &cwd,
            &loader,
            &runtime_config,
            Some(tasks),
            None,
        )
        .expect("rebuilt runtime state should build");

        let first_tasks = &first.tool_registry.context().tasks;
        let second_tasks = &second.tool_registry.context().tasks;
        assert!(first_tasks.get(&task.task_id).is_some());
        assert_eq!(
            second_tasks
                .live_background_process_count(Some("session-a"))
                .load(),
            1,
            "the rebuilt runtime sees the original watcher/live state"
        );
        assert!(second_tasks.remove(&task.task_id).is_some());
        assert!(
            first_tasks.get(&task.task_id).is_none(),
            "both runtime contexts share the same registry interior"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }
}
