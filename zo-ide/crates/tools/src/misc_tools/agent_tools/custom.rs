//! File-based custom sub-agent definitions.
//!
//! Mirrors Claude Code's agent format so users can define new harnesses
//! WITHOUT recompiling zo: drop a Markdown file at
//! `.zo/agents/<name>.md` with YAML-ish frontmatter and a body:
//!
//! ```text
//! ---
//! name: debug-agent
//! description: Reproduce and root-cause runtime crashes
//! tools: bash, read_file, grep_search, edit_file
//! model: opus
//! permissionMode: read-only
//! permission: bash(git *)=allow, bash(rm *)=deny
//! ---
//!
//! You are a debugging specialist. Reproduce the failure first, then ...
//! ```
//!
//! The frontmatter fields are all optional; the body becomes the agent's
//! system-prompt addition. Built-in types (`Explore`, `Plan`, ...) always
//! take precedence and can never be shadowed by a file, so the static tool
//! allowlists stay stable.
//!
//! ## Per-agent permissions (optional)
//!
//! `permissionMode` sets the agent's coarse permission floor — one of
//! `read-only`, `workspace-write`, or `danger-full-access`. Omitted, it stays
//! `danger-full-access`, identical to the historical behavior.
//!
//! `permission` is a comma-separated list of `<rule>=<allow|deny|ask>` tokens,
//! reusing the exact rule grammar from `settings.json` (so `bash(git *)`,
//! `edit_file(*.env)`, `read_file` all work). Use zo's *canonical* tool
//! names (`edit_file`, `write_file`, `bash`, `read_file`, `grep_search`, ...);
//! a mistyped name compiles into a rule that never fires. Evaluation semantics:
//!
//! - **deny beats ask beats allow** (deny is checked first).
//! - **first match wins** *within* a category — note this differs from
//!   `OpenCode`'s last-match-wins ordering.
//! - **`ask` resolves to deny** here: a spawned sub-agent runs headless with no
//!   prompter, so a rule that would prompt instead blocks.
//!
//! Both fields fail *closed*: a `permission`/`permissionMode` that is present
//! but unreadable (e.g. an indented YAML block the flat parser can't see, or an
//! unknown decision/mode keyword) rejects the whole definition rather than
//! silently spawning an agent with weaker restrictions than intended.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use runtime::{PermissionMode, RuntimePermissionRuleConfig};

/// A custom sub-agent harness parsed from a definition file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CustomAgent {
    pub name: String,
    pub description: String,
    /// Explicit tool allowlist. `None` means "inherit the general-purpose
    /// default set" so a definition can be body-only.
    pub tools: Option<Vec<String>>,
    pub model: Option<String>,
    /// Markdown body appended to the base system prompt.
    pub system_prompt: String,
    /// Optional allow/deny/ask permission rules, reusing the settings.json
    /// rule grammar. `None` (the default) keeps the policy rule-free, so the
    /// spawned sub-agent's enforcer behaves exactly as before this field.
    pub permission: Option<RuntimePermissionRuleConfig>,
    /// Optional coarse permission mode for the spawned sub-agent. `None`
    /// (the default) keeps `DangerFullAccess` — byte-identical to today.
    pub permission_mode: Option<PermissionMode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedCustomAgent {
    pub name: String,
    pub model: Option<String>,
    pub permission_mode: Option<PermissionMode>,
    pub source_path: PathBuf,
}

/// Built-in subagent types that a file definition must never shadow.
pub(crate) const BUILTIN_SUBAGENT_TYPES: &[&str] = &[
    "general-purpose",
    "Explore",
    "Plan",
    "Verification",
    "deep-research",
    "code-reviewer",
    "debugger",
    "data-analyst",
    "refactor",
    "zo-guide",
    "statusline-setup",
];

fn is_builtin(name: &str) -> bool {
    BUILTIN_SUBAGENT_TYPES.contains(&name)
}

/// Directories searched (in order) for `<name>.md` agent definitions.
///
/// `ZO_AGENT_DEFS_DIR` overrides the search entirely (used by tests so
/// they never depend on the process cwd). Otherwise we walk the cwd and a
/// bounded number of ancestors, checking `.zo/agents`, then the Zo user-global
/// homes.
fn agent_def_dirs() -> Vec<PathBuf> {
    if let Ok(dir) = std::env::var("ZO_AGENT_DEFS_DIR") {
        if !dir.trim().is_empty() {
            return vec![PathBuf::from(dir)];
        }
    }

    let mut dirs = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors().take(8) {
            dirs.push(ancestor.join(".zo").join("agents"));
        }
    }
    for root in runtime::zo_global_config_roots() {
        dirs.push(root.join("agents"));
    }
    dirs
}

/// Load the custom agent named `name`, or `None` for built-in / missing /
/// unparseable definitions.
pub(crate) fn load_custom_agent(name: &str) -> Option<CustomAgent> {
    let name = safe_agent_name(name)?;
    if is_builtin(name) {
        return None;
    }
    for dir in agent_def_dirs() {
        let Ok(dir) = std::fs::canonicalize(&dir) else {
            continue;
        };
        let path = dir.join(format!("{name}.md"));
        let Ok(path) = std::fs::canonicalize(&path) else {
            continue;
        };
        if !path.starts_with(&dir) {
            continue;
        }
        if let Ok(contents) = std::fs::read_to_string(&path) {
            if let Some(agent) = parse_custom_agent(name, &contents) {
                return Some(agent);
            }
        }
    }
    None
}

/// List the active custom definitions using the same root order, parser, and
/// first-valid-definition precedence as `load_custom_agent`.
#[must_use]
pub fn loaded_custom_agents() -> Vec<LoadedCustomAgent> {
    let mut loaded = Vec::new();
    let mut seen = BTreeSet::new();
    for dir in agent_def_dirs() {
        let Ok(dir) = std::fs::canonicalize(&dir) else {
            continue;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Some(invocation_name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let Some(invocation_name) = safe_agent_name(invocation_name) else {
                continue;
            };
            if is_builtin(invocation_name) || seen.contains(invocation_name) {
                continue;
            }
            let Ok(canonical_path) = std::fs::canonicalize(&path) else {
                continue;
            };
            if !canonical_path.starts_with(&dir) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&canonical_path) else {
                continue;
            };
            let Some(agent) = parse_custom_agent(invocation_name, &contents) else {
                continue;
            };
            seen.insert(invocation_name.to_string());
            loaded.push(LoadedCustomAgent {
                name: agent.name,
                model: agent.model,
                permission_mode: agent.permission_mode,
                source_path: canonical_path,
            });
        }
    }
    loaded
}

fn safe_agent_name(name: &str) -> Option<&str> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        return None;
    }
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if component == name => Some(name),
        _ => None,
    }
}

/// Parse a definition file into a [`CustomAgent`]. `name_hint` is the file
/// stem, used when the frontmatter omits an explicit `name`.
pub(crate) fn parse_custom_agent(name_hint: &str, contents: &str) -> Option<CustomAgent> {
    let contents = contents.trim_start_matches('\u{feff}');

    let mut name = name_hint.trim().to_string();
    let mut description = String::new();
    let mut tools = None;
    let mut model = None;
    let mut permission = None;
    let mut permission_mode = None;

    let body = if let Some(after_open) = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))
    {
        match split_frontmatter(after_open) {
            Some((front, rest)) => {
                for line in front.lines() {
                    let Some((key, value)) = line.split_once(':') else {
                        continue;
                    };
                    let key = key.trim().to_ascii_lowercase();
                    let value = value.trim().trim_matches('"').trim_matches('\'').trim();
                    match key.as_str() {
                        "name" if !value.is_empty() => name = value.to_string(),
                        "description" => description = value.to_string(),
                        "tools" => tools = Some(parse_tool_list(value)),
                        "model" if !value.is_empty() => model = Some(value.to_string()),
                        // Security-critical: a present-but-unreadable permission
                        // field (e.g. a nested YAML block the flat parser can't
                        // see, leaving an empty value) must reject the whole
                        // agent. Silently dropping it would spawn a sub-agent
                        // with *fewer* restrictions than the author intended.
                        "permission" => match parse_permission_rules(value) {
                            Some(rules) => permission = Some(rules),
                            None => return None,
                        },
                        "permissionmode" | "permission_mode" => {
                            match PermissionMode::parse(value) {
                                Some(mode) => permission_mode = Some(mode),
                                None => return None,
                            }
                        }
                        _ => {}
                    }
                }
                rest.trim().to_string()
            }
            None => contents.trim().to_string(),
        }
    } else {
        contents.trim().to_string()
    };

    if body.is_empty() && description.is_empty() {
        return None;
    }

    Some(CustomAgent {
        name,
        description,
        tools,
        model,
        system_prompt: body,
        permission,
        permission_mode,
    })
}

/// Parse a `permission:` frontmatter value into allow/deny/ask rule buckets,
/// reusing the settings.json rule grammar: comma-separated
/// `<rule>=<allow|deny|ask>` tokens. Each token is split on its *last* `=` so a
/// rule subject may itself contain `=` (e.g. `bash(FOO=bar)=allow`). A trailing
/// or doubled comma is tolerated.
///
/// Returns `None` — so the caller fails closed and rejects the agent — when the
/// value is empty or any token is malformed (no `=`, an empty rule, or an
/// unrecognized decision keyword). This is deliberate: a half-understood
/// permission spec must never silently weaken the spawned sub-agent.
fn parse_permission_rules(value: &str) -> Option<RuntimePermissionRuleConfig> {
    let mut allow = Vec::new();
    let mut deny = Vec::new();
    let mut ask = Vec::new();
    for token in value.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (rule, decision) = token.rsplit_once('=')?;
        let rule = rule.trim();
        if rule.is_empty() {
            return None;
        }
        let bucket = match decision.trim().to_ascii_lowercase().as_str() {
            "allow" => &mut allow,
            "deny" => &mut deny,
            "ask" => &mut ask,
            _ => return None,
        };
        bucket.push(rule.to_string());
    }
    if allow.is_empty() && deny.is_empty() && ask.is_empty() {
        return None;
    }
    Some(RuntimePermissionRuleConfig::new(allow, deny, ask))
}

/// Split the text following the opening `---` fence into
/// `(frontmatter, body)` at the next line that is exactly `---`.
fn split_frontmatter(after_open: &str) -> Option<(&str, &str)> {
    let mut idx = 0;
    for line in after_open.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            let front = &after_open[..idx];
            let body = &after_open[idx + line.len()..];
            return Some((front, body));
        }
        idx += line.len();
    }
    None
}

/// Parse a `tools:` value into a deduplicated, order-preserving list.
/// Accepts comma- and whitespace-separated forms, with optional `[ ]`.
fn parse_tool_list(value: &str) -> Vec<String> {
    let trimmed = value.trim().trim_start_matches('[').trim_end_matches(']');
    let mut out = Vec::new();
    for token in trimmed.split([',', ' ', '\t']) {
        let tool = token.trim().trim_matches('"').trim_matches('\'').trim();
        if !tool.is_empty() && !out.iter().any(|existing| existing == tool) {
            out.push(tool.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        load_custom_agent, parse_custom_agent, parse_permission_rules, parse_tool_list,
        safe_agent_name, PermissionMode,
    };
    /// Serializes the few tests that mutate `ZO_AGENT_DEFS_DIR` — via the
    /// crate-wide env lock, because `agent_tools/tests.rs` mutates the same
    /// variable and a module-local mutex cannot exclude it.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn parses_full_frontmatter_and_body() {
        let src = "---\nname: debug-agent\ndescription: Root-cause crashes\ntools: bash, read_file, grep_search\nmodel: claude-opus-4-8\n---\n\nYou are a debugging specialist. Reproduce first.\n";
        let agent = parse_custom_agent("ignored-hint", src).expect("parses");
        assert_eq!(agent.name, "debug-agent");
        assert_eq!(agent.description, "Root-cause crashes");
        assert_eq!(
            agent.tools,
            Some(vec![
                "bash".to_string(),
                "read_file".to_string(),
                "grep_search".to_string()
            ])
        );
        assert_eq!(agent.model.as_deref(), Some("claude-opus-4-8"));
        assert!(agent
            .system_prompt
            .starts_with("You are a debugging specialist."));
    }

    #[test]
    fn falls_back_to_name_hint_and_body_only() {
        let agent =
            parse_custom_agent("doc-agent", "Just a body, no frontmatter.").expect("parses");
        assert_eq!(agent.name, "doc-agent");
        assert!(agent.tools.is_none());
        assert!(agent.model.is_none());
        assert_eq!(agent.system_prompt, "Just a body, no frontmatter.");
    }

    #[test]
    fn empty_definition_is_rejected() {
        assert!(parse_custom_agent("x", "---\n---\n").is_none());
        assert!(parse_custom_agent("x", "   ").is_none());
    }

    #[test]
    fn tool_list_dedups_and_splits_mixed_separators() {
        assert_eq!(
            parse_tool_list("[bash, read_file read_file,, edit_file]"),
            vec![
                "bash".to_string(),
                "read_file".to_string(),
                "edit_file".to_string()
            ]
        );
    }

    #[test]
    fn parses_permission_rules_and_mode() {
        let src = "---\nname: reviewer\ndescription: Reviews diffs\npermission: bash(git *)=allow, bash(rm *)=deny, edit_file=deny\npermissionMode: read-only\n---\nYou review code.\n";
        let agent = parse_custom_agent("reviewer", src).expect("parses");
        assert_eq!(agent.permission_mode, Some(PermissionMode::ReadOnly));
        let rules = agent.permission.expect("permission rules present");
        assert_eq!(rules.allow(), ["bash(git *)"]);
        assert_eq!(rules.deny(), ["bash(rm *)", "edit_file"]);
        assert!(rules.ask().is_empty());
    }

    #[test]
    fn permission_rule_subject_may_contain_equals() {
        // The token is split on its LAST `=`, so the decision is `allow` and the
        // whole `bash(FOO=bar)` stays the rule.
        let rules = parse_permission_rules("bash(FOO=bar)=allow").expect("parses");
        assert_eq!(rules.allow(), ["bash(FOO=bar)"]);
    }

    #[test]
    fn permission_underscore_alias_is_accepted() {
        let agent = parse_custom_agent(
            "x",
            "---\ndescription: d\npermission_mode: workspace-write\n---\nbody\n",
        )
        .expect("parses");
        assert_eq!(agent.permission_mode, Some(PermissionMode::WorkspaceWrite));
    }

    #[test]
    fn empty_permission_value_rejects_agent_fail_closed() {
        // A nested-YAML `permission:` block the flat parser can't read leaves an
        // empty value. Rejecting (not silently dropping) is the safe behavior.
        assert!(parse_custom_agent(
            "x",
            "---\ndescription: d\npermission:\n  bash(rm *): deny\n---\nbody\n",
        )
        .is_none());
    }

    #[test]
    fn malformed_permission_tokens_reject_agent() {
        // No `=`.
        assert!(parse_custom_agent(
            "x",
            "---\ndescription: d\npermission: bash(rm *)\n---\nbody\n",
        )
        .is_none());
        // Unknown decision keyword.
        assert!(parse_custom_agent(
            "x",
            "---\ndescription: d\npermission: bash(rm *)=maybe\n---\nbody\n",
        )
        .is_none());
        // Empty rule before `=`.
        assert!(parse_permission_rules("=deny").is_none());
    }

    #[test]
    fn unknown_permission_mode_rejects_agent() {
        assert!(parse_custom_agent(
            "x",
            "---\ndescription: d\npermissionMode: read only\n---\nbody\n",
        )
        .is_none());
    }

    #[test]
    fn absent_permission_fields_default_to_none() {
        let agent = parse_custom_agent("x", "---\ndescription: d\ntools: bash\n---\nbody\n")
            .expect("parses");
        assert!(agent.permission.is_none());
        assert!(agent.permission_mode.is_none());
    }

    #[test]
    fn builtin_types_are_never_loaded_from_disk() {
        let dir = std::env::temp_dir().join(format!(
            "zo-agent-defs-{}-builtins",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        for name in [
            "Explore",
            "general-purpose",
            "deep-research",
            "code-reviewer",
            "debugger",
            "data-analyst",
            "refactor",
        ] {
            std::fs::write(
                dir.join(format!("{name}.md")),
                "---\ndescription: malicious shadow\ntools: bash\n---\nshadow",
            )
            .expect("write def");
        }

        let _guard = env_lock();
        let prev = std::env::var_os("ZO_AGENT_DEFS_DIR");
        std::env::set_var("ZO_AGENT_DEFS_DIR", &dir);
        assert!(load_custom_agent("Explore").is_none());
        assert!(load_custom_agent("general-purpose").is_none());
        assert!(load_custom_agent("deep-research").is_none());
        assert!(load_custom_agent("code-reviewer").is_none());
        assert!(load_custom_agent("debugger").is_none());
        assert!(load_custom_agent("data-analyst").is_none());
        assert!(load_custom_agent("refactor").is_none());
        assert!(load_custom_agent("").is_none());
        match prev {
            Some(value) => std::env::set_var("ZO_AGENT_DEFS_DIR", value),
            None => std::env::remove_var("ZO_AGENT_DEFS_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn loads_definition_from_override_dir() {
        let dir =
            std::env::temp_dir().join(format!("zo-agent-defs-{}-load", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(
            dir.join("triage-bot.md"),
            "---\ndescription: Triage incoming issues\ntools: read_file, grep_search\n---\nYou triage issues.\n",
        )
        .expect("write def");

        // Scope the env override and restore it so the test is order-safe.
        let _guard = env_lock();
        let prev = std::env::var_os("ZO_AGENT_DEFS_DIR");
        std::env::set_var("ZO_AGENT_DEFS_DIR", &dir);
        let loaded = load_custom_agent("triage-bot");
        match prev {
            Some(value) => std::env::set_var("ZO_AGENT_DEFS_DIR", value),
            None => std::env::remove_var("ZO_AGENT_DEFS_DIR"),
        }
        let _ = std::fs::remove_dir_all(&dir);

        let agent = loaded.expect("custom agent loads from override dir");
        assert_eq!(agent.name, "triage-bot");
        assert_eq!(
            agent.tools,
            Some(vec!["read_file".to_string(), "grep_search".to_string()])
        );
    }

    #[test]
    fn load_custom_agent_rejects_path_traversal_names() {
        assert_eq!(safe_agent_name("triage-bot"), Some("triage-bot"));
        for name in ["", ".", "..", "../escape", "nested/agent", "nested\\agent", "/abs"] {
            assert!(safe_agent_name(name).is_none(), "rejected unsafe name: {name:?}");
        }

        let root = std::env::temp_dir().join(format!(
            "zo-agent-defs-{}-traversal",
            std::process::id()
        ));
        let defs = root.join("agents");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&defs).expect("mkdir defs");
        std::fs::write(
            root.join("escape.md"),
            "---\ndescription: escaped\n---\nThis must not load.\n",
        )
        .expect("write escaped");

        let _guard = env_lock();
        let prev = std::env::var_os("ZO_AGENT_DEFS_DIR");
        std::env::set_var("ZO_AGENT_DEFS_DIR", &defs);
        assert!(load_custom_agent("../escape").is_none());
        match prev {
            Some(value) => std::env::set_var("ZO_AGENT_DEFS_DIR", value),
            None => std::env::remove_var("ZO_AGENT_DEFS_DIR"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn load_custom_agent_rejects_symlink_escape() {
        let root = std::env::temp_dir().join(format!(
            "zo-agent-defs-{}-symlink",
            std::process::id()
        ));
        let defs = root.join("agents");
        let outside = root.join("outside");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&defs).expect("mkdir defs");
        std::fs::create_dir_all(&outside).expect("mkdir outside");
        std::fs::write(
            outside.join("escape.md"),
            "---\ndescription: escaped\n---\nThis must not load.\n",
        )
        .expect("write outside");
        std::os::unix::fs::symlink(outside.join("escape.md"), defs.join("escape.md"))
            .expect("symlink");

        let _guard = env_lock();
        let prev = std::env::var_os("ZO_AGENT_DEFS_DIR");
        std::env::set_var("ZO_AGENT_DEFS_DIR", &defs);
        assert!(load_custom_agent("escape").is_none());
        match prev {
            Some(value) => std::env::set_var("ZO_AGENT_DEFS_DIR", value),
            None => std::env::remove_var("ZO_AGENT_DEFS_DIR"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
