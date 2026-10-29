use std::collections::BTreeSet;
use std::env;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use runtime::{
    normalize_tool_name, PermissionMode, PermissionPolicy,
};
use tools::GlobalToolRegistry;

use crate::default_prompt_date;

/// Build the main-loop system prompt for `model`.
///
/// `model` must be the session's *settled* model — the one the runtime is
/// about to be bound to, after preference resolution — because it is what the
/// prompt's `Model family:` line reports back to the model itself.
pub(crate) fn build_system_prompt_for_mode(
    cwd: &Path,
    mode: runtime::PromptMode,
    model: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Main-loop prompt: the only surface that carries the configured
    // `outputStyle` (sub-agents and status views stay on the stock prompt).
    // `# Delegation and workflow routing` is 1,315 tokens of `Agent` /
    // `SpawnMultiAgent` / `Workflow` rubric. With the spawn family turned off it
    // teaches the model to reach for tools it will be refused, so it comes out
    // — read from the same session deny set that decides what is advertised
    // (`crate::runtime_support::disallowed_tool_names`), so the prompt and the
    // wire can never disagree about whether this session can delegate.
    let mut sections = runtime::load_system_prompt_for_main_with_mode_and_spawn(
        cwd,
        default_prompt_date(),
        env::consts::OS,
        "unknown",
        Some(model),
        mode,
        spawn_family_enabled(),
    )?;
    // Deferred-tool manifest: the model cannot ToolSearch for names it has
    // never seen. Session-stable, so it never disturbs the prompt cache.
    // Appended here rather than in the runtime builder because the deferred
    // set is owned by the tools crate, which depends on runtime — not vice
    // versa.
    sections.push(tools::deferred_tool_manifest_section());
    Ok(sections)
}

/// Whether this session can delegate at all.
///
/// True unless every member of [`crate::session::plain_session::SPAWN_FAMILY_TOOLS`]
/// is in the session deny set — which is exactly what `--no-spawn` installs.
/// A partial deny (someone passing `--disallowedTools Agent` alone) keeps the
/// rubric, because the remaining two tools are still callable and the rubric is
/// how the model chooses between them.
fn spawn_family_enabled() -> bool {
    let Some(disallowed) = crate::runtime_support::disallowed_tool_names() else {
        return true;
    };
    !crate::session::plain_session::SPAWN_FAMILY_TOOLS
        .iter()
        .all(|tool| disallowed.contains(*tool))
}


pub(crate) fn permission_policy(
    mode: PermissionMode,
    feature_config: &runtime::RuntimeFeatureConfig,
    tool_registry: &GlobalToolRegistry,
) -> Result<PermissionPolicy, String> {
    Ok(tool_registry
        .permission_specs(None)
        .map_err(|e| e.to_string())?
        .into_iter()
        .fold(
            PermissionPolicy::new(mode).with_permission_rules(feature_config.permission_rules()),
            |policy, (name, required_permission)| {
                policy.with_tool_requirement(name, required_permission)
            },
        ))
}

/// The rule warnings this process has queued, so a rebuilt runtime (a
/// `/resume`, a model handoff) does not say one again.
static WARNED_RULES: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Queue a start-up warning for each permission rule that names no tool zo
/// has, and that this process has not warned of yet (t-81312).
pub(crate) fn queue_unknown_rule_warnings(
    feature_config: &runtime::RuntimeFeatureConfig,
    tool_registry: &GlobalToolRegistry,
) {
    for line in unknown_rule_warnings(feature_config, tool_registry) {
        let fresh = WARNED_RULES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(line.clone());
        if fresh {
            crate::runtime_support::push_startup_warning(line);
        }
    }
}

/// One line for each configured permission rule whose tool name no tool zo
/// knows answers to: a rule is matched against the name a tool is called by,
/// case and `-`/`_` aside, so such a rule never applies. The line names the
/// rule, its list and its settings file, and zo's tool when the alias table
/// knows the name (`Edit` is `edit_file`). An MCP tool is not judged: its
/// server registers it after start.
pub(crate) fn unknown_rule_warnings(
    feature_config: &runtime::RuntimeFeatureConfig,
    tool_registry: &GlobalToolRegistry,
) -> Vec<String> {
    let rules = runtime::rule_tool_names(feature_config.permission_rules());
    if rules.is_empty() {
        return Vec::new();
    }
    let known: BTreeSet<String> = tool_registry
        .known_tool_names()
        .iter()
        .map(|name| normalize_tool_name(name))
        .collect();
    rules
        .into_iter()
        .filter_map(|named| {
            let tool = normalize_tool_name(named.tool);
            if tool.starts_with(runtime::tool_guard::MCP_TOOL_PREFIX) || known.contains(&tool) {
                return None;
            }
            let files = feature_config.permission_rule_files(named.list, named.rule);
            let place = if files.is_empty() {
                "your settings".to_string()
            } else {
                files
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let mut line = format!(
                "permissions.{} rule '{}' in {place} names no zo tool, so it never applies",
                named.list, named.rule
            );
            let alias = tools::canonical_tool_name(named.tool);
            let alias_key = normalize_tool_name(&alias);
            if alias_key != tool && known.contains(&alias_key) {
                let _ = write!(line, " — zo's tool for {} is {alias}", named.tool);
            }
            Some(line)
        })
        .collect()
}

// `convert_messages` and the cache-breakpoint marking live in the runtime
// crate (the single source of truth, beside the
// `context_compression::wire_tool_output` they depend on) so the sub-agent
// provider clients in `crates/tools` share them. Re-export under the in-crate
// names so existing `pub(crate)` callers stay unchanged.
pub(crate) use runtime::mark_conversation_cache_breakpoints;

#[cfg(test)]
mod system_prompt_tests {
    use super::build_system_prompt_for_mode;
    use std::fs;
    


    #[test]
    fn build_system_prompt_for_reads_supplied_cwd() {
        let root = crate::support::temp_dir("cwd");
        fs::write(root.join("context.md"), "SUPPLIED_CWD_RULE").expect("write instructions");

        let prompt =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-opus-5")
                .expect("system prompt should load")
                .join("\n\n");

        assert!(prompt.contains("SUPPLIED_CWD_RULE"), "{prompt}");
        fs::remove_dir_all(root).ok();
    }

    /// The main-loop prompt reports the SESSION's model back to it. Before this
    /// the `Model family:` line was a hardcoded release name, so every session
    /// — and every sub-agent — was told it was that model no matter what it
    /// actually ran on.
    #[test]
    fn main_prompt_reports_the_bound_model() {
        let root = crate::support::temp_dir("bound-model");

        let fable =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-fable-5")
                .expect("system prompt should load")
                .join("\n\n");
        assert!(fable.contains("Model family: Claude Fable 5"), "{fable}");
        assert!(!fable.contains("Opus"), "fable session must not claim Opus");

        let opus =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-opus-5")
                .expect("system prompt should load")
                .join("\n\n");
        assert!(opus.contains("Model family: Claude Opus 5"), "{opus}");

        fs::remove_dir_all(root).ok();
    }
}

#[cfg(test)]
mod unknown_rule_warning_tests {
    use std::collections::BTreeSet;
    use std::fs;

    use super::unknown_rule_warnings;

    /// The warnings for the settings `home` and `project` hold, with
    /// `disabled` tools turned off in the registry and `offered` added to it
    /// as runtime tools.
    fn warnings_for(
        home: &str,
        project: Option<&str>,
        disabled: &[&str],
        offered: &[&str],
    ) -> (Vec<String>, String, String) {
        let root = tempfile::tempdir().expect("a scratch folder");
        let home_dir = root.path().join("home");
        let cwd = root.path().join("work");
        fs::create_dir_all(&home_dir).expect("home");
        fs::create_dir_all(cwd.join(".zo")).expect(".zo");
        let home_file = home_dir.join("settings.json");
        let project_file = cwd.join(".zo").join("settings.json");
        fs::write(&home_file, home).expect("home settings");
        if let Some(project) = project {
            fs::write(&project_file, project).expect("project settings");
        }
        let config = runtime::ConfigLoader::new(&cwd, &home_dir).load().expect("settings load");
        let registry = tools::GlobalToolRegistry::builtin();
        registry
            .context()
            .set_disabled_tools(disabled.iter().map(|tool| (*tool).to_string()).collect::<BTreeSet<_>>());
        registry
            .set_runtime_tools(
                offered
                    .iter()
                    .map(|name| tools::RuntimeToolDefinition {
                        name: (*name).to_string(),
                        description: None,
                        input_schema: serde_json::json!({"type": "object"}),
                        required_permission: runtime::PermissionMode::WorkspaceWrite,
                    })
                    .collect(),
            )
            .expect("the runtime tools register");
        (
            unknown_rule_warnings(config.feature_config(), &registry),
            home_file.display().to_string(),
            project_file.display().to_string(),
        )
    }

    /// One line per rule no tool answers to (t-81312). The same rule in two
    /// files names both, comma-separated; a rule in the ordered `rules` list
    /// is warned of too; a tool that is only turned off is still zo's. The
    /// alias hint gives zo's tool and says the rule does not reach it — and
    /// Claude Code's `Task`, a sub-agent, gets no hint at zo's `TaskCreate`.
    #[test]
    fn unknown_rule_warnings_name_the_rule_its_files_and_zos_tool_only_where_it_is_the_same_tool() {
        let (lines, home, project) = warnings_for(
            r#"{"permissions": {"deny": ["Edti(x)", "WebSearch", "Task(x)", "Edit(*.env)"]}}"#,
            Some(r#"{"permissions": {"deny": ["Edti(x)"]}}"#),
            &["WebSearch"],
            // zo keeps `TaskCreate` from the model today; were it offered, a
            // rule written for Claude Code's `Task` must still not point at it.
            &["TaskCreate"],
        );
        assert!(
            lines.contains(&format!(
                "permissions.deny rule 'Edti(x)' in {home}, {project} names no zo tool, so it never applies"
            )),
            "the rule in two files: {lines:#?}"
        );
        assert!(
            !lines.iter().any(|line| line.contains("'WebSearch'")),
            "a turned-off tool is warned of as unknown: {lines:#?}"
        );

        let (ordered, ordered_home, _) =
            warnings_for(r#"{"permissions": {"rules": ["Edti(x)=deny"]}}"#, None, &[], &[]);
        assert_eq!(
            ordered,
            vec![format!(
                "permissions.rules rule 'Edti(x)=deny' in {ordered_home} names no zo tool, so it never applies"
            )]
        );

        assert!(
            lines.contains(&format!(
                "permissions.deny rule 'Task(x)' in {home} names no zo tool, so it never applies"
            )),
            "Claude Code's Task is pointed at zo's TaskCreate: {lines:#?}"
        );
        assert!(
            lines.contains(&format!(
                "permissions.deny rule 'Edit(*.env)' in {home} names no zo tool; zo's tool for Edit is edit_file, and this rule does not reach it"
            )),
            "the alias hint: {lines:#?}"
        );
        assert_eq!(lines.len(), 3, "{lines:#?}");
    }
}

