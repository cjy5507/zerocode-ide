use std::collections::BTreeSet;
use std::env;
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
/// rule, its list and its settings file, and zo's tool when the name means
/// that tool elsewhere too (`Edit` is `edit_file`; Claude Code's `Task` is
/// not zo's `TaskCreate`). An MCP tool is not judged: its server registers it
/// after start.
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
            let zo_tool = tools::same_tool_alias(named.tool)
                .filter(|zo_tool| known.contains(&normalize_tool_name(zo_tool)));
            // zo offers the model its own name, so a rule written for the
            // other agent's name does not reach the tool it was meant for.
            Some(match zo_tool {
                Some(zo_tool) => format!(
                    "permissions.{} rule '{}' in {place} names no zo tool; zo's tool for {} is {zo_tool}, and this rule does not reach it",
                    named.list, named.rule, named.tool
                ),
                None => format!(
                    "permissions.{} rule '{}' in {place} names no zo tool, so it never applies",
                    named.list, named.rule
                ),
            })
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
        // The settings come from the config home the env names: no other test
        // may move it, or delete the folder it names, while this one reads it.
        let _lock = crate::test_env_lock();
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
        // The settings come from the config home the env names: no other test
        // may move it, or delete the folder it names, while this one reads it.
        let _lock = crate::test_env_lock();
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

    /// t-78118: the prompt reads its settings from the config home that
    /// `ZO_CONFIG_HOME`, `ZO_HOME` or `HOME` names at the moment it is built.
    /// A test in this binary that points one of them at a folder of its own
    /// and then deletes that folder, beside a test that builds the prompt
    /// without the env lock, gave Windows `Access is denied` (code 5: a file
    /// opened while it was being deleted) at `main_prompt_reports_the_bound_model`.
    /// So every function that moves one of the three, or calls a helper that
    /// does, and every test that builds the prompt, holds `test_env_lock()`.
    /// The helpers whose callers hold it are named below, each with its reason.
    #[test]
    fn every_test_that_moves_or_reads_the_config_home_holds_the_env_lock() {
        const SETTERS: [&str; 3] = ["set_var(", "remove_var(", "EnvVarGuard::set("];
        const NAMES: [&str; 3] = ["\"HOME\"", "\"ZO_CONFIG_HOME\"", "\"ZO_HOME\""];
        const NAME_CONSTANTS: [&str; 2] = ["ZO_CONFIG_HOME_ENV", "ZO_HOME_ENV"];
        const HELPERS: [&str; 3] = ["IsolatedHome::new()", "NoLoginAnywhere::new()", "isolated()"];
        const READERS: [&str; 1] = ["build_system_prompt_for_mode("];
        // (file, function): why it may move the home without taking the lock.
        const HOLDERS_ELSEWHERE: [(&str, &str, &str); 5] = [
            ("support.rs", "test_env_mutex", "the lock's own first use sets the shared test home"),
            ("support.rs", "new", "IsolatedHome::new: its callers hold the lock (and SessionRootPin::new takes it)"),
            ("support.rs", "drop", "SessionRootPin's drop runs while the pin still holds the lock"),
            ("session/runtime_bridge.rs", "new", "NoLoginAnywhere::new: its callers hold the lock"),
            ("session/wire_ceiling_tests.rs", "isolated", "builds NoLoginAnywhere; its callers hold the lock"),
        ];
        const ME: &str = "every_test_that_moves_or_reads_the_config_home_holds_the_env_lock";

        fn sources(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in fs::read_dir(dir).expect("read the source folder") {
                let path = entry.expect("a source entry").path();
                if path.is_dir() {
                    sources(&path, root, out);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let name = path.strip_prefix(root).expect("under src").to_string_lossy().replace('\\', "/");
                    out.push((name, fs::read_to_string(&path).expect("read a source file")));
                }
            }
        }
        fn fn_name(line: &str) -> Option<&str> {
            let mut rest = line.trim_start();
            if let Some(after) = rest.strip_prefix("pub") {
                rest = after.strip_prefix('(').and_then(|scoped| scoped.split_once(") ")).map_or(after.trim_start(), |(_, tail)| tail);
            }
            rest = rest.strip_prefix("async ").unwrap_or(rest);
            let name = rest.strip_prefix("fn ")?;
            Some(&name[..name.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(name.len())])
        }
        fn moves_the_home(code: &str) -> bool {
            let named = |at: usize| {
                let rest = code[at..].trim_start();
                NAMES.iter().any(|name| rest.starts_with(name)) || {
                    let path = &rest[..rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')).unwrap_or(rest.len())];
                    NAME_CONSTANTS.iter().any(|constant| path.ends_with(constant))
                }
            };
            SETTERS.iter().any(|setter| code.match_indices(setter).any(|(at, _)| named(at + setter.len())))
                || HELPERS.iter().any(|helper| {
                    code.match_indices(helper)
                        .any(|(at, _)| !code[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == ':') || helper.contains("::"))
                })
        }

        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &root, &mut files);
        let (mut movers, mut readers) = (0, 0);
        let mut seen_elsewhere = Vec::new();
        let mut unlocked = Vec::new();
        for (file, text) in &files {
            let lines: Vec<&str> = text.lines().collect();
            let starts: Vec<usize> = (0..lines.len()).filter(|&at| fn_name(lines[at]).is_some()).collect();
            for (index, &start) in starts.iter().enumerate() {
                let end = starts.get(index + 1).copied().unwrap_or(lines.len());
                let name = fn_name(lines[start]).expect("a function line");
                // Code only: a comment that names the lock does not hold it.
                let code = lines[start..end]
                    .iter()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");
                let attributes = lines[..start].iter().rev().take_while(|line| {
                    let line = line.trim_start();
                    line.starts_with("#[") || line.starts_with("//")
                });
                let is_test = attributes.into_iter().any(|line| line.contains("#[test]") || line.contains("#[tokio::test"));
                let changes_home = moves_the_home(&code);
                let builds_prompt = is_test && READERS.iter().any(|reader| code.contains(reader));
                if name == ME || !(changes_home || builds_prompt) {
                    continue;
                }
                movers += usize::from(changes_home);
                readers += usize::from(builds_prompt);
                if code.contains("test_env_lock()") {
                    continue;
                }
                if HOLDERS_ELSEWHERE.iter().any(|(at, function, _)| at == file && *function == name) {
                    seen_elsewhere.push((file.as_str(), name));
                    continue;
                }
                unlocked.push(format!("{file}:{} {name}", start + 1));
            }
        }
        assert!(movers >= 20 && readers >= 3, "the scan found {movers} movers and {readers} prompt builders");
        for (at, function, why) in HOLDERS_ELSEWHERE {
            assert!(seen_elsewhere.contains(&(at, function)), "{at} {function} ({why}) no longer moves the home: take it off the list");
        }
        assert!(unlocked.is_empty(), "these move or read the config home without test_env_lock(): {unlocked:#?}");
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

