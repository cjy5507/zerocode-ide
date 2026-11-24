use std::collections::BTreeSet;
use std::env;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use core_types::tool_names::tool_key;
use runtime::{PermissionMode, PermissionPolicy};
use tools::GlobalToolRegistry;

use crate::default_prompt_date;

/// Build the main-loop system prompt for `model`, for a session that reaches
/// what `offer` says it does.
///
/// `model` must be the session's *settled* model — the one the runtime is
/// about to be bound to, after preference resolution — because it is what the
/// prompt's `Model family:` line reports back to the model itself.
pub(crate) fn build_system_prompt_for_mode(
    cwd: &Path,
    mode: runtime::PromptMode,
    model: &str,
    offer: tools::ToolOffer<'_>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    // Main-loop prompt: the only surface that carries the configured
    // `outputStyle` (sub-agents and status views stay on the stock prompt).
    // A section that teaches only tools the session cannot reach comes out —
    // `# Delegation and workflow routing` alone is 1,315 tokens of `Agent` /
    // `SpawnMultiAgent` / `Workflow` rubric that would teach the model to reach
    // for tools it will be refused. Read from the session's own tool list and
    // deny set, the ones that decide what is advertised, so the prompt and the
    // wire can never disagree about what this session can call.
    let mut sections = runtime::load_system_prompt_for_main_with_mode_and_reach(
        cwd,
        default_prompt_date(),
        env::consts::OS,
        "unknown",
        Some(model),
        mode,
        runtime::PromptToolReach::from_fn(|tool| offer.reaches(tool)).with_ledger(offer.reaches_ledger()),
    )?;
    // Deferred-tool manifest: the model cannot ToolSearch for names it has
    // never seen. Session-stable, so it never disturbs the prompt cache — the
    // tools it names are what this process's `PATH` reaches, and that does not
    // change under a session (`tools::ToolReach`), less the tools the person's
    // rules refuse every call of, read once here (`rules_as_read`). Appended
    // here rather than in the runtime builder because the deferred set is
    // owned by the tools crate, which depends on runtime — not vice versa. A
    // session given neither of the doors it names (`CapabilityInvoke`,
    // `ToolSearch`) is not sent it: nothing in it could be called.
    if offer.reaches(tools::capability::CAPABILITY_INVOKE) || offer.reaches(TOOL_SEARCH) {
        let denied_by = rules_as_read(cwd);
        sections.push(tools::deferred_tool_manifest_section_for(
            tools::ToolReach::here(),
            denied_by.as_ref(),
        ));
    }
    Ok(sections)
}

/// Build the prompt the headless `profile` writes for a main-loop session in
/// `cwd` that reaches what `offer` says — written whole, so it names no tool
/// the session lacks and no deferred door, and its sandbox line speaks of the
/// shells the session runs. The session decided it may take the profile
/// (`session::headless_profile`).
pub(crate) fn build_headless_profile_prompt(
    cwd: &Path,
    offer: tools::ToolOffer<'_>,
    profile: runtime::HeadlessProfile,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(runtime::load_system_prompt_for_headless_profile(
        cwd,
        default_prompt_date(),
        env::consts::OS,
        "unknown",
        runtime::PromptToolReach::from_fn(|tool| offer.reaches(tool)).with_ledger(offer.reaches_ledger()),
        profile,
    )?)
}

/// The tool the deferred-tool manifest sends a model to for MCP and plugin
/// tools.
const TOOL_SEARCH: &str = "ToolSearch";

/// The permission rules a session under `cwd` reads, as a policy — what the
/// prompt asks [`PermissionPolicy::denies_every_call`] of. The same settings
/// documents and the same rule intake ([`rules_policy`]) the session's own
/// policy is built from ([`permission_policy`]); the mode does not enter that
/// answer. `None` when the settings do not load: then every tool is offered,
/// and every call is judged as it always is.
fn rules_as_read(cwd: &Path) -> Option<PermissionPolicy> {
    let config = runtime::ConfigLoader::default_for(cwd).load().ok()?;
    Some(rules_policy(PermissionMode::ReadOnly, config.feature_config()))
}

/// A policy in `mode` holding the configured permission rules and nothing
/// else — the one rule intake of [`permission_policy`] and of what the prompt
/// offers ([`rules_as_read`]).
fn rules_policy(mode: PermissionMode, feature_config: &runtime::RuntimeFeatureConfig) -> PermissionPolicy {
    PermissionPolicy::new(mode).with_permission_rules(feature_config.permission_rules())
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
            rules_policy(mode, feature_config).with_schedule_workspace(tool_registry.schedule_workspace()),
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

/// The local settings lines this process has queued, for the same reason as
/// [`WARNED_RULES`].
static WARNED_LOCAL_SETTINGS: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Queue a start-up warning when the workspace's `.zo/settings.local.json`
/// said something that went unused because zo did not write that file as it
/// stands (153), once in this process.
pub(crate) fn queue_unowned_local_settings_warning(feature_config: &runtime::RuntimeFeatureConfig) {
    let Some(line) = unowned_local_settings_warning(
        feature_config.unowned_local_settings(),
        feature_config.always_allow_for_the_session_only(),
    ) else {
        return;
    };
    let fresh = WARNED_LOCAL_SETTINGS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(line.clone());
    if fresh {
        crate::runtime_support::push_startup_warning(line);
    }
}

/// Why zo did not take a folder's local file as the person's, as the start-up
/// line says it: a file the folder came with, one an older zo wrote before it
/// kept a record, or one changed since — by hand, or by a change of zo's own
/// tools that widens what it grants, which only the person records (153).
const NOT_WRITTEN_BY_ZO: &str = "(it came with this folder, an older zo wrote it before zo kept a record, or it changed since, by hand or by a change that widens what it grants)";

/// What a person who wrote that file does about it.
const RECORD_IT_YOURSELF: &str =
    "If it is yours, read it over, then run `zo settings trust-local` in your own terminal.";

/// The line that says which settings of a folder's local file went unused,
/// why, and what a person who wrote that file can do, and whether an "always
/// allow" granted there lasts for the session only — or none when neither
/// holds. The names are the loader's own; nothing of the file's text is put
/// on the screen.
fn unowned_local_settings_warning(unused: &[&str], session_only: bool) -> Option<String> {
    let until_then = if session_only {
        " Until then, \"always allow\" here lasts for this session only."
    } else {
        ""
    };
    if !unused.is_empty() {
        return Some(format!(
            "zo did not use {} from .zo/settings.local.json: zo did not write that file as it stands {NOT_WRITTEN_BY_ZO}. {RECORD_IT_YOURSELF}{until_then}",
            unused.join(", ")
        ));
    }
    session_only.then(|| {
        format!(
            "\"Always allow\" here lasts for this session only: zo did not write .zo/settings.local.json as it stands {NOT_WRITTEN_BY_ZO}, and it grants something. {RECORD_IT_YOURSELF}"
        )
    })
}

/// One line for each configured permission rule whose tool name no tool zo
/// knows answers to: a rule meets a call by the tool's key
/// (`core_types::tool_names::tool_key` — case, `-`/`_` and another agent's name
/// for the same tool aside, so `Edit` is `edit_file` while Claude Code's `Task`
/// is not zo's `TaskCreate`), and such a rule never applies. The line names
/// the rule, its list and its settings file. An MCP tool is not judged: its
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
        .map(|name| tool_key(name))
        .collect();
    rules
        .into_iter()
        .filter_map(|named| {
            let tool = tool_key(named.tool);
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
            Some(format!(
                "permissions.{} rule '{}' in {place} names no zo tool, so it never applies",
                named.list, named.rule
            ))
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
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-opus-5", tools::ToolOffer::default())
                .expect("system prompt should load")
                .join("\n\n");

        assert!(prompt.contains("SUPPLIED_CWD_RULE"), "{prompt}");
        fs::remove_dir_all(root).ok();
    }

    /// A tool the settings deny outright (`WebSearch`) is not named in the
    /// deferred line, while one denied only for some subjects
    /// (`WebFetch(https://docs.rs/*)`) still is; and the policy the session is
    /// built with refuses a call to the hidden one in today's words. A
    /// benchmark run on 2026-11-14 found `WebSearch` through the line, loaded
    /// it and was refused five times by the bench's own deny rule.
    #[test]
    fn a_tool_the_settings_deny_outright_is_not_named_and_its_call_is_refused() {
        // The settings come from the config home the env names: no other test
        // may move it, or delete the folder it names, while this one reads it.
        let _lock = crate::test_env_lock();
        let root = crate::support::temp_dir("denied-outright");
        fs::create_dir_all(root.join(".zo")).expect("settings folder");
        fs::write(
            root.join(".zo").join("settings.json"),
            r#"{"permissions":{"deny":["WebSearch","WebFetch(https://docs.rs/*)"]}}"#,
        )
        .expect("write settings");

        let prompt =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-opus-5", tools::ToolOffer::default())
                .expect("system prompt should load")
                .join("\n\n");
        let line = prompt
            .lines()
            .find(|line| line.starts_with("Hidden but callable:"))
            .expect("the deferred line");
        assert!(!line.contains("WebSearch"), "denied outright, so not offered: {line}");
        assert!(line.contains("WebFetch"), "denied for one subject only, so still offered: {line}");

        let config = runtime::ConfigLoader::default_for(&root).load().expect("settings load");
        let policy = super::permission_policy(
            runtime::PermissionMode::DangerFullAccess,
            config.feature_config(),
            &tools::GlobalToolRegistry::builtin(),
        )
        .expect("the session's policy");
        assert!(
            matches!(
                policy.authorize("WebSearch", r#"{"query":"rust"}"#, None),
                runtime::PermissionOutcome::Deny { reason }
                    if reason.contains("Permission to use WebSearch has been denied by rule 'WebSearch'")
            ),
            "a call by name meets the refusal it always met"
        );
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
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-fable-5", tools::ToolOffer::default())
                .expect("system prompt should load")
                .join("\n\n");
        assert!(fable.contains("Model family: Claude Fable 5"), "{fable}");
        assert!(!fable.contains("Opus"), "fable session must not claim Opus");

        let opus =
            build_system_prompt_for_mode(&root, runtime::PromptMode::Interactive, "claude-opus-5", tools::ToolOffer::default())
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
    /// does, and every test that reads the config home, holds `test_env_lock()`.
    /// A test reads it when it calls one of the readers below, or calls a
    /// function of this crate that calls one itself (one step; the scan finds
    /// those functions, nobody lists them). A reader's name is tied to its
    /// definition the way Rust resolves the call — a method, a path, a `use`,
    /// the test's own module before its parent's — so `run`, defined in many
    /// modules, is only the reader where it is that module's `run`.
    /// The helpers whose callers hold it are named below, each with its reason,
    /// and so are the tests that call a reader but never on its reading road.
    ///
    /// What the scan does not see: a read two or more calls away from the
    /// test; a read inside another crate's function (runtime, tools); a nested
    /// `fn`, which cuts the function around it at its line, and an attribute
    /// written over several lines. And a test that calls a helper taking the
    /// lock counts as holding it for all its reads, so a read it makes after
    /// the helper has returned, outside the lock, goes unseen.
    #[test]
    #[allow(clippy::too_many_lines)] // one scan: the movers, the readers one step out, and the lists that answer for what it cannot see
    fn every_test_that_moves_or_reads_the_config_home_holds_the_env_lock() {
        use std::collections::{BTreeMap, BTreeSet, HashMap};
        const SETTERS: [&str; 3] = ["set_var(", "remove_var(", "EnvVarGuard::set("];
        const NAMES: [&str; 3] = ["\"HOME\"", "\"ZO_CONFIG_HOME\"", "\"ZO_HOME\""];
        const NAME_CONSTANTS: [&str; 2] = ["ZO_CONFIG_HOME_ENV", "ZO_HOME_ENV"];
        const HELPERS: [&str; 3] = ["IsolatedHome::new()", "NoLoginAnywhere::new()", "isolated()"];
        // Calls that read the config home the env names right now: the
        // prompt's settings, the home itself, and the loader of the person's
        // and the project's settings.
        const READERS: [&str; 3] = ["build_system_prompt_for_mode(", "default_config_home(", "ConfigLoader::default_for("];
        // (file, function): why it may move the home without taking the lock.
        const HOLDERS_ELSEWHERE: [(&str, &str, &str); 5] = [
            ("support.rs", "test_env_mutex", "the lock's own first use sets the shared test home"),
            ("support.rs", "new", "IsolatedHome::new: its callers hold the lock (and SessionRootPin::new takes it)"),
            ("support.rs", "drop", "SessionRootPin's drop runs while the pin still holds the lock"),
            ("session/runtime_bridge.rs", "new", "NoLoginAnywhere::new: its callers hold the lock"),
            ("session/wire_ceiling_tests.rs", "isolated", "builds NoLoginAnywhere; its callers hold the lock"),
        ];
        // (file, test): why it calls a reader of this crate by name and still
        // never reads the config home.
        const NOT_REACHING: [(&str, &str, &str); 3] = [
            (
                "session/runtime_builder.rs",
                "build_runtime_plugin_state_skips_memory_retriever_when_disabled",
                "its settings turn auto memory off, and the builder reads the config home only for auto memory's preferences",
            ),
            (
                "jev_cli/tests.rs",
                "every_builtin_definition_is_read_without_running_a_judgment",
                "`zo jev definition` answers from the built-in words; only `learning` and `rules` read the config home",
            ),
            ("jev_cli/tests.rs", "every_builtin_editor_exports_typed_questions_and_criteria", "the same `zo jev definition`"),
        ];
        // (name): why a test's call of this reader's name cannot be tied to one
        // definition, so the tests that call it go unchecked. None today: every
        // call is a method, a path, a `use` or a module's own name.
        const UNWATCHED: [(&str, &str); 0] = [];
        // How many calls the names above leave unchecked: the floor and the
        // ceiling are the same number, so a new one is named here or tied.
        const UNWATCHED_CALLS: std::ops::RangeInclusive<usize> = 0..=0;
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
        let mut items = Vec::new();
        let mut uses = HashMap::new();
        for (file, text) in &files {
            let lines: Vec<&str> = text.lines().collect();
            // Code only: a comment that names the lock does not hold it.
            let code_of = |range: &[&str]| range.iter().filter(|line| !line.trim_start().starts_with("//")).copied().collect::<Vec<_>>().join("\n");
            uses.insert(file.as_str(), use_statements(&without_strings(&code_of(&lines[..]))));
            let spans = test_modules(&lines);
            let starts: Vec<usize> = (0..lines.len()).filter(|&at| fn_name(lines[at]).is_some()).collect();
            for (index, &start) in starts.iter().enumerate() {
                let end = starts.get(index + 1).copied().unwrap_or(lines.len());
                let code = code_of(&lines[start..end]);
                let attributes = lines[..start].iter().rev().take_while(|line| {
                    let line = line.trim_start();
                    line.starts_with("#[") || line.starts_with("//")
                });
                let plain = without_strings(&code);
                items.push(ScannedFn {
                    file: file.clone(),
                    line: start + 1,
                    name: fn_name(lines[start]).expect("a function line").to_string(),
                    is_test: attributes.into_iter().any(|line| line.contains("#[test]") || line.contains("#[tokio::test")),
                    takes_self: takes_self(&plain),
                    owner: owner(&lines, start),
                    region: spans.iter().copied().find(|&(open, close)| open < start && start < close),
                    code,
                    plain,
                });
            }
        }
        let mut defs: HashMap<&str, Vec<usize>> = HashMap::new();
        for (at, item) in items.iter().enumerate().filter(|(_, item)| !item.is_test) {
            defs.entry(item.name.as_str()).or_default().push(at);
        }
        // One step out: the functions of this crate that call a reader themselves,
        // and those that take the lock themselves.
        let reads = |item: &ScannedFn| READERS.iter().any(|reader| item.plain.contains(reader) && reader.strip_suffix('(') != Some(item.name.as_str()));
        let readers: Vec<usize> = (0..items.len()).filter(|&at| !items[at].is_test && reads(&items[at])).collect();
        let lockers: Vec<usize> = (0..items.len()).filter(|&at| !items[at].is_test && items[at].plain.contains("test_env_lock()")).collect();
        let names_of = |list: &[usize]| list.iter().map(|&at| items[at].name.as_str()).collect::<BTreeSet<_>>();
        let (reader_names, locker_names) = (names_of(&readers), names_of(&lockers));

        let (mut movers, mut direct_readers, mut step_readers, mut settled) = (0, 0, 0, 0);
        let mut seen_elsewhere = Vec::new();
        let mut seen_not_reaching = Vec::new();
        let mut unwatched: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        let mut unlocked = Vec::new();
        for item in &items {
            let changes_home = moves_the_home(&item.code);
            let holds = item.plain.contains("test_env_lock()");
            let (mut through, mut held_by_a_helper) = (BTreeSet::new(), false);
            let direct = item.is_test && READERS.iter().any(|reader| item.plain.contains(reader));
            if item.is_test && item.name != ME {
                let file_uses = &uses[item.file.as_str()];
                for &name in &reader_names {
                    for form in calls_of(&item.plain, name) {
                        match resolve(&items, &defs[name], file_uses, item, name, &form) {
                            Callee::One(at) if readers.contains(&at) => {
                                through.insert(format!("{} {}", items[at].file, items[at].name));
                            }
                            Callee::Unsettled => unwatched.entry(name).or_default().push(format!("{}:{} {}", item.file, item.line, item.name)),
                            Callee::One(_) | Callee::Nothing => settled += 1,
                        }
                    }
                }
                for &name in &locker_names {
                    held_by_a_helper |= calls_of(&item.plain, name)
                        .iter()
                        .any(|form| matches!(resolve(&items, &defs[name], file_uses, item, name, form), Callee::One(at) if lockers.contains(&at)));
                }
            }
            let reads_home = direct || !through.is_empty();
            if item.name == ME || !(changes_home || reads_home) {
                continue;
            }
            movers += usize::from(changes_home);
            direct_readers += usize::from(direct);
            step_readers += usize::from(!direct && !through.is_empty());
            // A helper's lock covers a read inside it, not a move of the home
            // made before or after the call.
            if holds || (!changes_home && held_by_a_helper) {
                continue;
            }
            if HOLDERS_ELSEWHERE.iter().any(|(at, function, _)| *at == item.file && *function == item.name) {
                seen_elsewhere.push((item.file.as_str(), item.name.as_str()));
                continue;
            }
            if !changes_home && !direct && NOT_REACHING.iter().any(|(at, test, _)| *at == item.file && *test == item.name) {
                seen_not_reaching.push((item.file.as_str(), item.name.as_str()));
                continue;
            }
            unlocked.push(format!("{}:{} {} {through:?}", item.file, item.line, item.name));
        }
        assert!(movers >= 20 && direct_readers >= 3, "the scan found {movers} movers and {direct_readers} tests that call a reader");
        assert!(
            (20..=60).contains(&readers.len()) && (10..=60).contains(&step_readers),
            "the scan found {} functions that read the config home and {step_readers} tests that call one",
            readers.len()
        );
        assert!(settled >= 50, "the scan tied only {settled} calls of a reader's name to another function of that name or to none");
        for (at, function, why) in HOLDERS_ELSEWHERE {
            assert!(seen_elsewhere.contains(&(at, function)), "{at} {function} ({why}) no longer moves the home: take it off the list");
        }
        for (at, test, why) in NOT_REACHING {
            assert!(seen_not_reaching.contains(&(at, test)), "{at} {test} ({why}) no longer calls a reader without the lock: take it off the list");
        }
        let unchecked: usize = unwatched.values().map(Vec::len).sum();
        assert!(
            unwatched.keys().copied().eq(UNWATCHED.iter().map(|(name, _)| *name)) && UNWATCHED_CALLS.contains(&unchecked),
            "the readers' names a test calls that the scan cannot tie, with the tests ({unchecked} calls): {unwatched:#?}"
        );
        assert!(unlocked.is_empty(), "these move or read the config home without test_env_lock(): {unlocked:#?}");
    }

    /// One function the config-home scan found: where it starts, its code
    /// without comment lines, that code with every string blanked, and what
    /// ties a call to it — whether it takes `self`, the type its `impl` is
    /// for, and the `#[cfg(test)]` module it sits in.
    struct ScannedFn {
        file: String,
        line: usize,
        name: String,
        code: String,
        plain: String,
        is_test: bool,
        takes_self: bool,
        owner: Option<String>,
        region: Option<(usize, usize)>,
    }

    /// How a call is spelled: `.name(`, `path::name(`, or `name(`.
    enum CallForm {
        Method,
        Path(String),
        Bare,
    }

    /// What one call is tied to: one function of this crate, none of them
    /// (another crate's, or the standard library's), or more than one.
    enum Callee {
        One(usize),
        Nothing,
        Unsettled,
    }

    fn is_ident(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    /// `code` with the text of every string literal blanked, so a name a
    /// string holds (a fixture's `fn main() {}`) is not taken for a call.
    fn without_strings(code: &str) -> String {
        let bytes = code.as_bytes();
        let mut out = String::with_capacity(code.len());
        let mut at = 0;
        while at < code.len() {
            let rest = &code[at..];
            if let Some(quote) = ["'\"'", "'\\\"'"].into_iter().find(|quote| rest.starts_with(quote)) {
                out.push_str("' '");
                at += quote.len();
                continue;
            }
            let word_start = at == 0 || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_');
            if let Some(hashes) = rest.strip_prefix('b').unwrap_or(rest).strip_prefix('r').filter(|_| word_start) {
                let count = hashes.len() - hashes.trim_start_matches('#').len();
                if hashes[count..].starts_with('"') {
                    let open = rest.len() - hashes.len() + count + 1;
                    let close = format!("\"{}", "#".repeat(count));
                    at += rest[open..].find(&close).map_or(rest.len(), |found| open + found + close.len());
                    out.push_str("\"\"");
                    continue;
                }
            }
            if rest.starts_with('"') {
                let mut end = 1;
                while at + end < code.len() && bytes[at + end] != b'"' {
                    end += if bytes[at + end] == b'\\' { 2 } else { 1 };
                }
                at = (at + end + 1).min(code.len());
                out.push_str("\"\"");
                continue;
            }
            let next = rest.chars().next().expect("a character");
            out.push(next);
            at += next.len_utf8();
        }
        out
    }

    /// The line spans of each `#[cfg(test)] mod name {` … `}` in a file, laid
    /// out the way this tree writes them: the closing brace alone at the
    /// opening line's indent.
    fn test_modules(lines: &[&str]) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        for (at, line) in lines.iter().enumerate() {
            if line.trim() != "#[cfg(test)]" {
                continue;
            }
            let Some(open) = (at + 1..lines.len()).find(|&next| !lines[next].trim_start().starts_with("#[")) else { continue };
            let opening = lines[open].trim_start();
            let indent = &lines[open][..lines[open].len() - opening.len()];
            let head = opening.strip_prefix("pub(crate) ").or_else(|| opening.strip_prefix("pub ")).unwrap_or(opening);
            if !(head.starts_with("mod ") && head.ends_with('{')) {
                continue;
            }
            let close = (open + 1..lines.len()).find(|&next| lines[next].strip_prefix(indent) == Some("}")).unwrap_or(lines.len());
            spans.push((open, close));
        }
        spans
    }

    /// The module path a source file is: `jev_cli/learning.rs` is
    /// `jev_cli::learning`, `lib.rs` and `main.rs` are the crate's root.
    fn module_of(file: &str) -> Vec<&str> {
        let mut parts: Vec<&str> = file.strip_suffix(".rs").unwrap_or(file).split('/').collect();
        if parts.last() == Some(&"mod") {
            parts.pop();
        }
        if parts == ["lib"] || parts == ["main"] {
            parts.clear();
        }
        parts
    }

    /// Whether the function's first parameter is `self`, `&self` or `&mut self`.
    fn takes_self(plain: &str) -> bool {
        let signature = plain.split('{').next().unwrap_or(plain);
        let Some(after) = signature.find("fn ").map(|at| &signature[at..]) else { return false };
        let Some(open) = after.find('(') else { return false };
        let mut first = after[open + 1..].trim_start().trim_start_matches('&').trim_start();
        if first.starts_with('\'') {
            first = first.split_once(' ').map_or("", |(_, tail)| tail.trim_start());
        }
        first = first.strip_prefix("mut ").unwrap_or(first).trim_start();
        first.strip_prefix("self").is_some_and(|tail| !tail.starts_with(is_ident))
    }

    /// The type of the `impl` block a function line sits in, if it sits in one.
    fn owner(lines: &[&str], start: usize) -> Option<String> {
        let indent = lines[start].len() - lines[start].trim_start().len();
        let (_, outer) = lines[..start]
            .iter()
            .rev()
            .map(|line| (line.len() - line.trim_start().len(), line.trim_start()))
            .find(|(depth, text)| *depth < indent && !text.is_empty() && !text.starts_with("#[") && !text.starts_with("//"))?;
        let head = outer.strip_prefix("impl").filter(|rest| rest.starts_with([' ', '<']))?.split('{').next()?;
        let (mut depth, mut bare) = (0_i32, String::new());
        for c in head.chars() {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ if depth == 0 => bare.push(c),
                _ => {}
            }
        }
        let target = bare.split(" where ").next()?.rsplit(" for ").next()?;
        target.rsplit(|c: char| !is_ident(c)).find(|word| !word.is_empty()).map(str::to_string)
    }

    /// Every `use` statement in a file's code, each on one line.
    fn use_statements(plain: &str) -> Vec<String> {
        let mut statements = Vec::new();
        let mut open: Option<String> = None;
        for line in plain.lines().map(str::trim) {
            let statement = match open.take() {
                Some(head) => head + " " + line,
                None if ["use ", "pub use ", "pub(crate) use "].iter().any(|start| line.starts_with(start)) => line.to_string(),
                None => continue,
            };
            if statement.contains(';') {
                statements.push(statement);
            } else {
                open = Some(statement);
            }
        }
        statements
    }

    /// The path segment a `use` statement brings `name` in under — `super`
    /// for `use super::{parse, run};` — or an empty one when it renames
    /// something else to `name`; `None` when the statement does not name it.
    fn imported_from(statement: &str, name: &str) -> Option<String> {
        let (at, _) = statement.match_indices(name).find(|&(at, _)| {
            let after = &statement[at + name.len()..];
            !statement[..at].ends_with(is_ident) && !after.starts_with(is_ident) && !after.trim_start().starts_with("::")
        })?;
        let mut before = statement[..at].to_string();
        if before.trim_end().ends_with(" as") {
            return Some(String::new());
        }
        while let Some(close) = before.find('}') {
            let open = before[..close].rfind('{').unwrap_or(0);
            before.replace_range(open..=close, "");
        }
        if let Some(open) = before.rfind('{') {
            before.truncate(open);
        }
        let path = before.trim_end().trim_end_matches("::");
        Some(path.rsplit(|c: char| !is_ident(c)).find(|word| !word.is_empty()).unwrap_or_default().to_string())
    }

    /// Every call of `name` in `plain`, by how it is spelled; its definition
    /// (`fn name(`) is not a call.
    fn calls_of(plain: &str, name: &str) -> Vec<CallForm> {
        plain
            .match_indices(name)
            .filter_map(|(at, _)| {
                let before = &plain[..at];
                if before.ends_with(is_ident) || !plain[at + name.len()..].trim_start().starts_with('(') {
                    return None;
                }
                let spaced = before.trim_end();
                if spaced.strip_suffix("fn").is_some_and(|head| !head.ends_with(is_ident)) {
                    return None;
                }
                Some(if before.ends_with('.') {
                    CallForm::Method
                } else if let Some(path) = before.strip_suffix("::") {
                    CallForm::Path(path.rsplit(|c: char| !is_ident(c)).next().unwrap_or_default().to_string())
                } else {
                    CallForm::Bare
                })
            })
            .collect()
    }

    /// The function of this crate (among `defs`, the definitions of the
    /// called name) that `form`, written in `test`, calls — the way Rust
    /// would find it: a method on the test file's own type first; `Self`,
    /// `self`, `super`, `crate` or a module or type name for a path; and for
    /// a bare name the `use` that brings it in, then the test's own module,
    /// the file around an inline test module, the parent of a test file, and
    /// last the one free function of that name in the crate.
    fn resolve(items: &[ScannedFn], defs: &[usize], file_uses: &[String], test: &ScannedFn, name: &str, form: &CallForm) -> Callee {
        let one = |pick: &dyn Fn(&ScannedFn) -> bool| {
            let mut found = defs.iter().copied().filter(|&at| pick(&items[at]));
            match (found.next(), found.next()) {
                (None, _) => Callee::Nothing,
                (Some(at), None) => Callee::One(at),
                (Some(_), Some(_)) => Callee::Unsettled,
            }
        };
        let free = |item: &ScannedFn| item.owner.is_none() && !item.takes_self;
        let module = module_of(&test.file);
        let parent = &module[..module.len().saturating_sub(1)];
        let by_path = |path: &str| match path {
            "" => Callee::Unsettled,
            "Self" => one(&|item| item.file == test.file && item.owner.is_some()),
            "self" => one(&|item| free(item) && item.file == test.file && item.region == test.region),
            "super" if test.region.is_some() => one(&|item| free(item) && item.file == test.file && item.region.is_none()),
            "super" => one(&|item| free(item) && item.region.is_none() && module_of(&item.file) == parent),
            "crate" => one(&|item| free(item) && item.region.is_none() && module_of(&item.file).is_empty()),
            path => one(&|item| {
                item.owner.as_deref() == Some(path) || (free(item) && item.region.is_none() && module_of(&item.file).last() == Some(&path))
            }),
        };
        match form {
            CallForm::Method => match one(&|item| item.takes_self && item.file == test.file) {
                Callee::Nothing => one(&|item| item.takes_self),
                mine => mine,
            },
            CallForm::Path(path) => by_path(path.as_str()),
            CallForm::Bare => {
                if let Some(path) = file_uses.iter().find_map(|statement| imported_from(statement, name)) {
                    return by_path(path.as_str());
                }
                let scopes: [&dyn Fn(&ScannedFn) -> bool; 4] = [
                    &|item| item.file == test.file && item.region == test.region,
                    &|item| test.region.is_some() && item.file == test.file && item.region.is_none(),
                    &|item| !module.is_empty() && item.region.is_none() && module_of(&item.file) == parent,
                    &|item| item.region.is_none(),
                ];
                scopes
                    .iter()
                    .map(|scope| one(&|item| free(item) && scope(item)))
                    .find(|callee| !matches!(callee, Callee::Nothing))
                    .unwrap_or(Callee::Nothing)
            }
        }
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
    /// is warned of too; a tool that is only turned off is still zo's. A rule
    /// under another agent's name for the same tool reaches it (`Edit` is
    /// `edit_file`, t-96186) and is not warned of — while Claude Code's
    /// `Task`, a sub-agent, is not zo's `TaskCreate`.
    #[test]
    fn unknown_rule_warnings_name_the_rule_and_its_files_and_spare_another_agents_name_for_a_zo_tool() {
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
            !lines.iter().any(|line| line.contains("'Edit(*.env)'")),
            "a rule under Claude Code's name for edit_file reaches it: {lines:#?}"
        );
        assert_eq!(lines.len(), 2, "{lines:#?}");
    }
}


#[cfg(test)]
mod unowned_local_settings_warning_tests {
    use std::fs;

    use super::unowned_local_settings_warning;

    /// Nothing went unused and an "always allow" is kept: nothing is said.
    #[test]
    fn a_folder_whose_local_file_lost_nothing_is_not_warned_of() {
        assert_eq!(unowned_local_settings_warning(&[], false), None);
    }

    /// The line names what went unused by the loader's words, says why, and
    /// sends a person who wrote the file to their own terminal (153).
    #[test]
    fn the_line_names_what_went_unused_why_and_what_a_person_can_do() {
        let line =
            unowned_local_settings_warning(&["hooks", "permissions.allow"], true).expect("a line");
        assert!(
            line.starts_with("zo did not use hooks, permissions.allow from .zo/settings.local.json:"),
            "{line}"
        );
        for said in [
            "zo did not write that file as it stands",
            "an older zo wrote it before zo kept a record",
            "run `zo settings trust-local` in your own terminal",
            "\"always allow\" here lasts for this session only",
        ] {
            assert!(line.contains(said), "{said:?} is missing from: {line}");
        }
        assert!(!line.contains("git"), "git is no longer the reason: {line}");
    }

    /// A file that lost nothing to the repository's rules but grants
    /// something zo keeps out of (an `enabledPlugins`, a `smart` the person's
    /// own settings match) still keeps "always allow" to the session, and the
    /// person is told so once, with what to do (153).
    #[test]
    fn an_always_allow_kept_to_the_session_is_said_once_with_what_to_do() {
        let line = unowned_local_settings_warning(&[], true).expect("a line");
        assert!(
            line.starts_with("\"Always allow\" here lasts for this session only:"),
            "{line}"
        );
        assert!(line.contains("run `zo settings trust-local` in your own terminal"), "{line}");
    }

    /// From the loader to the line: a folder whose local file zo did not write
    /// and which carries a hook is warned of by that name.
    #[test]
    #[cfg_attr(windows, ignore = "a folder's local file is never the person's on Windows, so the loader names nothing there")]
    fn a_hook_in_a_local_file_zo_did_not_write_is_named() {
        let root = tempfile::tempdir().expect("a scratch folder");
        let home = root.path().join("home");
        let cwd = root.path().join("work");
        fs::create_dir_all(&home).expect("home");
        fs::create_dir_all(cwd.join(".zo")).expect(".zo");
        fs::write(
            cwd.join(".zo").join("settings.local.json"),
            r#"{"hooks":{"SessionStart":["echo came-with-the-folder"]},"model":"opus"}"#,
        )
        .expect("local settings");

        let config = runtime::ConfigLoader::new(&cwd, &home).load().expect("settings load");
        let features = config.feature_config();
        let line = unowned_local_settings_warning(
            features.unowned_local_settings(),
            features.always_allow_for_the_session_only(),
        )
        .expect("a line");

        assert!(line.starts_with("zo did not use hooks from"), "{line}");
        assert!(line.contains("lasts for this session only"), "{line}");
        assert!(!line.contains("came-with-the-folder"), "text of the file reached the screen: {line}");
    }

    /// From the loader to the line: a local file holding plain settings only
    /// (an older zo's, say) is one zo writes into, so nothing is said; one
    /// holding a `smart` setting that only narrows lost nothing but is kept
    /// out of, so the session-only line is said (153).
    #[test]
    #[cfg_attr(windows, ignore = "a folder's local file is never the person's on Windows, so the loader names nothing there")]
    fn a_plain_local_file_is_not_warned_of_and_a_granting_one_keeps_always_allow_to_the_session() {
        let said = |local: &str| {
            let root = tempfile::tempdir().expect("a scratch folder");
            let home = root.path().join("home");
            let cwd = root.path().join("work");
            fs::create_dir_all(&home).expect("home");
            fs::create_dir_all(cwd.join(".zo")).expect(".zo");
            fs::write(cwd.join(".zo").join("settings.local.json"), local).expect("local settings");
            let config = runtime::ConfigLoader::new(&cwd, &home).load().expect("settings load");
            let features = config.feature_config();
            unowned_local_settings_warning(
                features.unowned_local_settings(),
                features.always_allow_for_the_session_only(),
            )
        };
        assert_eq!(said(r#"{"model":"opus","reasoningEffort":"high"}"#), None);
        let line = said(r#"{"smart":{"autoClassifier":"off"}}"#).expect("a line");
        assert!(
            line.starts_with("\"Always allow\" here lasts for this session only:"),
            "{line}"
        );
    }
}
