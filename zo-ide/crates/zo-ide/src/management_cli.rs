//! Public adapters for the existing plugin manager and hook configuration.
use std::path::{Path, PathBuf};
use serde_json::json;

pub fn manager(cwd: &Path) -> Result<plugins::PluginManager, String> {
    let loader = runtime::ConfigLoader::default_for(cwd);
    let config = loader.load().map_err(|e| e.to_string())?;
    Ok(crate::support::build_plugin_manager(cwd, &loader, &config))
}

fn options(args: &[String], cwd: &Path) -> Result<(Vec<String>, PathBuf, bool), String> {
    let mut words = Vec::new();
    let mut cwd = cwd.to_path_buf();
    let mut as_json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => { as_json = true; i += 1; }
            "--cwd" => { cwd = PathBuf::from(args.get(i + 1).ok_or("--cwd requires a directory")?); i += 2; }
            _ => { words.push(args[i].clone()); i += 1; }
        }
    }
    Ok((words, cwd, as_json))
}

pub fn plugins(args: &[String], cwd: &Path) -> Result<String, String> {
    let usage = "zo plugin list|get|install|validate|enable|disable|uninstall|update [<id|source>] [--cwd <dir>] [--json]";
    if matches!(args, [flag] if flag == "--help" || flag == "-h") { return Ok(usage.into()); }
    let (words, cwd, as_json) = options(args, cwd)?;
    let mut manager = manager(&cwd)?;
    let Some((verb, rest)) = words.split_first() else { return Err(usage.into()); };
    let result = match (verb.as_str(), rest) {
        ("list", []) => {
            let report = manager.plugin_registry_report().map_err(|e| e.to_string())?;
            // A corrupt plugin is an actionable failure, not an empty successful list.
            if report.has_failures() { return Err(report.failures().iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")); }
            serde_json::Value::Array(report.summaries().iter().map(|plugin| json!({
                "id":plugin.metadata.id,"name":plugin.metadata.name,"version":plugin.metadata.version,
                "enabled":plugin.enabled,"kind":plugin.metadata.kind.to_string(),"root":plugin.metadata.root,
                "source":plugin.metadata.source
            })).collect())
        }
        ("get", [id]) => {
            let registry = manager.plugin_registry().map_err(|e| e.to_string())?;
            let plugin = registry.get(id).ok_or_else(|| format!("plugin '{id}' was not found"))?;
            json!({"id":plugin.metadata().id,"enabled":plugin.is_enabled(),"root":plugin.metadata().root,"commands":plugin.commands(),"hooks":plugin.hooks()})
        }
        ("validate", [source]) => { let manifest = manager.validate_plugin_source(source).map_err(|e| e.to_string())?; json!({"valid":true,"name":manifest.name,"version":manifest.version}) }
        ("install", [source]) => { let result = manager.install(source).map_err(|e| e.to_string())?; json!({"installed":result.plugin_id,"version":result.version,"path":result.install_path}) }
        ("enable", [id]) => { manager.enable(id).map_err(|e| e.to_string())?; json!({"enabled":id}) }
        ("disable", [id]) => { manager.disable(id).map_err(|e| e.to_string())?; json!({"disabled":id}) }
        ("uninstall", [id]) => { manager.uninstall(id).map_err(|e| e.to_string())?; json!({"uninstalled":id}) }
        ("update", [id]) => { let result = manager.update(id).map_err(|e| e.to_string())?; json!({"updated":result.plugin_id,"version":result.new_version}) }
        _ => return Err(usage.into()),
    };
    if as_json { Ok(result.to_string()) } else { serde_json::to_string_pretty(&result).map_err(|e| e.to_string()) }
}

pub fn hooks(args: &[String], cwd: &Path) -> Result<String, String> {
    if matches!(args, [flag] if flag == "--help" || flag == "-h") { return Ok("zo hooks list|show [--cwd <dir>] [--json]".into()); }
    let (words, cwd, as_json) = options(args, cwd)?;
    if !matches!(words.as_slice(), [word] if word == "list" || word == "show") { return Err("zo hooks list|show [--cwd <dir>] [--json]".into()); }
    let loader = runtime::ConfigLoader::default_for(&cwd);
    let config = loader.load().map_err(|e| e.to_string())?;
    let registry = crate::support::build_plugin_manager(&cwd, &loader, &config).plugin_registry().map_err(|e| e.to_string())?;
    let plugin_hooks = crate::session::runtime_builder::runtime_hook_config_from_plugin_hooks(registry.aggregated_hooks().map_err(|e| e.to_string())?);
    let merged = config.hooks().merged(&plugin_hooks);
    let rows = [
        ("PreToolUse", merged.pre_tool_use()), ("PostToolUse", merged.post_tool_use()),
        ("PostToolUseFailure", merged.post_tool_use_failure()), ("SessionStart", merged.session_start()),
        ("SessionEnd", merged.session_end()), ("UserPromptSubmit", merged.user_prompt_submit()),
        ("PreCompact", merged.pre_compact()), ("PostCompact", merged.post_compact()),
        ("SubagentStart", merged.subagent_start()), ("SubagentStop", merged.subagent_stop()),
        ("TurnStart", merged.turn_start()), ("TurnEnd", merged.turn_end()),
        ("PermissionRequest", merged.permission_request()), ("PermissionDenied", merged.permission_denied()),
        ("CwdChanged", merged.cwd_changed()), ("TaskCreated", merged.task_created()),
        ("TaskCompleted", merged.task_completed()), ("FileChanged", merged.file_changed()), ("Notification", merged.notification()),
    ];
    let rows: Vec<_> = rows.into_iter().flat_map(|(event, rules)| rules.iter().map(move |rule| json!({"event":event,"command":rule.command(),"rule":rule.to_string()}))).collect();
    let value = json!({"hooks":rows,"timeout_seconds":merged.timeout_seconds()});
    if as_json { Ok(value.to_string()) } else { serde_json::to_string_pretty(&value).map_err(|e| e.to_string()) }
}
