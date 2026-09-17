use std::env;
use std::path::Path;

use runtime::{
    PermissionMode, PermissionPolicy,
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


