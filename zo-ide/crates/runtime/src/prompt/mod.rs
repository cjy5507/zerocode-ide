use std::fmt::Write as _;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::{ConfigError, ConfigLoader, RuntimeConfig};
use crate::git_snapshot::read_git_root;
use zerocode_core::jev::{JevMode, SKILLS, SKILL_TOP_CAP};

pub mod output_style;
mod ablation;
mod sections;

use sections::{
    get_actions_section, get_clarification_section, get_context_trust_label_section,
    get_default_coding_harness_section, get_delegation_section, get_grounding_in_code_section,
    get_memory_protocol_section, get_responding_section, get_response_style_contract_section,
    get_simple_doing_tasks_section, get_simple_intro_section, get_simple_system_section,
    get_skills_and_docs_section, get_turn_discipline_section, render_config_section,
    render_memory_index, render_second_brain_section,
};

/// Which interaction surface the prompt is built for. The behavioral contract
/// is identical across models; only the turn-ending discipline varies —
/// interactive sessions may hand a genuine decision back to the user, an
/// autonomous surface (headless one-shot) must not ask mid-task questions,
/// and sub-agents carry their own completion contract in the agent profile.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PromptMode {
    /// A human is present at the terminal (REPL/TUI sessions).
    #[default]
    Interactive,
    /// Nobody can answer mid-task (headless `-p` one-shots, CI runs).
    Autonomous,
    /// Delegated sub-agents and informational surfaces.
    Subagent,
}

/// Errors raised while assembling the final system prompt.
#[derive(Debug)]
pub enum PromptBuildError {
    Io(std::io::Error),
    Config(ConfigError),
}

impl std::fmt::Display for PromptBuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Config(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PromptBuildError {}

impl From<std::io::Error> for PromptBuildError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<ConfigError> for PromptBuildError {
    fn from(value: ConfigError) -> Self {
        Self::Config(value)
    }
}

/// Marker separating static prompt scaffolding from dynamic runtime context.
pub const SYSTEM_PROMPT_DYNAMIC_BOUNDARY: &str = "__SYSTEM_PROMPT_DYNAMIC_BOUNDARY__";
/// Human-readable model-family label used when a prompt is built without a
/// bound model (diagnostic surfaces, harnesses that have not picked a model
/// yet). Resolved from the provider catalog's Opus family head on every call —
/// never a hardcoded release id — so bumping the catalog moves this with it.
///
/// Prompts built *with* a model must not use this: they render that model's
/// own label via [`SystemPromptBuilder::with_model`], so a Fable/GPT/Gemini
/// session is never told it is an Opus.
#[must_use]
pub fn frontier_model_name() -> String {
    api::latest_anthropic_family_model(api::ANTHROPIC_OPUS_MODEL_ALIAS)
        .and_then(crate::model_catalog::model_family_label)
        .unwrap_or_else(|| "Claude".to_string())
}
const MAX_INSTRUCTION_FILE_CHARS: usize = 4_000;
const MAX_TOTAL_INSTRUCTION_CHARS: usize = 12_000;
const MAX_SKILL_INDEX_ENTRIES: usize = 32;
/// The skill index's share of the fixed harness (r15 §2.2, made the release
/// contract in r49): 900 tokens in the repo's `chars / 4 + 1` estimate. The
/// bucket table in `zo_ide::prompt_input` reads this constant, so the renderer
/// and the gate ask one number. Entries past it fold into one line that says
/// how many were left out — the `Skill` tool loads any installed skill by
/// name, so nothing becomes unreachable, and a machine that installs more
/// skills pays the same on every request (2026-09-10: seventeen `~/.zo/skills`
/// read 922/900 before this).
pub const SKILL_INDEX_BUDGET_TOKENS: usize = 900;
/// Which of the two ways a turn is told about the skills this machine has
/// installed.
///
/// The index was the only road until t-5629: every skill's name and compacted
/// description, on every request, under [`SKILL_INDEX_BUDGET_TOKENS`], with
/// the tail past that budget folded into a line that names a count and
/// nothing else. The judgment seat (`zerocode_core::jev::SKILLS`) offers the
/// other: no list at all, and a tool that ranks the WHOLE catalog against the
/// task and hands the best ones back whole, where the cached prefix never
/// sees them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillsIndexRoad {
    /// Render the index, as every request has always done.
    #[default]
    Index,
    /// Leave the index out and name the two tools instead.
    Tools,
}

impl SkillsIndexRoad {
    /// The road for a catalog of `skills`, given what the person set the
    /// skill seat to and whether that seat's own evidence has raised it.
    ///
    /// Two things send a turn down the tool road, and they are different
    /// kinds of reason:
    ///
    /// 1. **The seat acts.** A person wrote `on`, or wrote `auto` and the
    ///    seat's ledger has since earned it ([`JevMode::applies_with`]). That
    ///    is a decision about this machine's skills, and it holds whatever
    ///    the catalog's size.
    /// 2. **The index no longer fits.** Past the budget the index is not the
    ///    catalog any more — it is a prefix of it plus a count — so the
    ///    promise it makes, that a model can read the list and choose, is one
    ///    it has already stopped keeping. A tool that ranks every skill keeps
    ///    that promise for the skills the fold dropped, and the word match
    ///    behind it keeps working with no key and no network
    ///    (`crate::skill_rank::lexical_rank`), so nothing becomes
    ///    unreachable.
    ///
    /// A person's `off` outranks neither: `off` is about asking Jev anything,
    /// and an index that overflows overflows whether or not a judgment is
    /// allowed to read it.
    #[must_use]
    pub fn decide(mode: JevMode, raised: bool, skills: &[SkillIndexEntry]) -> Self {
        if mode.applies_with(raised) || index_overflows(skills) {
            Self::Tools
        } else {
            Self::Index
        }
    }
}

/// Prompt-only cap for one skill's trigger summary. The full frontmatter value
/// remains on [`SkillIndexEntry`] for deterministic routing; only the cached
/// catalog shown on every request is compacted.
const MAX_SKILL_DESCRIPTION_CHARS: usize = 240;
/// Cap on the `git status` snapshot embedded in the system prompt. A large
/// monorepo, or a branch with thousands of untracked/modified paths, can emit a
/// `git status --short` of tens of thousands of lines. Embedding it verbatim on
/// **every** turn balloons the request — worst case a near-1M-token prefill that
/// the provider rejects with `overloaded_error` (the "다른 폴더에서 hi만 쳐도
/// Overloaded" report). The model can always run `git status` itself for the
/// full list, so a bounded preview is sufficient.
const MAX_GIT_STATUS_LINES: usize = 80;
const MAX_GIT_STATUS_CHARS: usize = 4_000;

/// Contents of an instruction file included in prompt construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
    pub globs: Vec<String>,
    pub always_apply: bool,
}

impl ContextFile {
    fn new(path: PathBuf, content: String) -> Self {
        Self {
            path,
            content,
            globs: Vec::new(),
            always_apply: true,
        }
    }

    fn instruction(path: PathBuf, content: &str) -> Self {
        let (frontmatter, body) = parse_frontmatter(content).map_or_else(
            || (Vec::new(), content.trim().to_string()),
            |(fields, body)| (fields, body.trim().to_string()),
        );
        let globs = parse_globs(&frontmatter);
        let always_apply = parse_always_apply(&frontmatter).unwrap_or(globs.is_empty());
        Self {
            path,
            content: body,
            globs,
            always_apply,
        }
    }

    fn is_always_instruction(&self) -> bool {
        self.always_apply || self.globs.is_empty()
    }
}

/// How a skill may be invoked when its triggers match a turn. Mirrors the
/// Claude Code "control who invokes a skill" idea: `Manual` is recommended only
/// on an explicit mention, `Suggest` (default) is nudged whenever triggers
/// match, and `Auto` is the strongest nudge. The recommendation is always
/// advisory — it never force-loads a skill body — so even `Auto` only asks the
/// model to call the `Skill` tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillInvocationMode {
    /// Recommend only when the user names the skill explicitly.
    Manual,
    /// Recommend whenever triggers match (default).
    #[default]
    Suggest,
    /// Strongest recommendation when triggers match.
    Auto,
}

impl SkillInvocationMode {
    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "suggest" => Some(Self::Suggest),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// Optional Codex-style implicit-invocation metadata parsed from a skill's
/// frontmatter. All fields default to empty, so a skill without trigger
/// metadata is simply never auto-recommended (it stays listed in the prompt's
/// `# Available skills` index exactly as before).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillTriggers {
    pub keywords: Vec<String>,
    pub paths: Vec<String>,
    pub excludes: Vec<String>,
}

impl SkillTriggers {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keywords.is_empty() && self.paths.is_empty() && self.excludes.is_empty()
    }
}

/// Compact metadata for a project-local skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillIndexEntry {
    pub name: String,
    pub description: Option<String>,
    pub path: PathBuf,
    /// Implicit-invocation mode (default [`SkillInvocationMode::Suggest`]).
    pub invocation_mode: SkillInvocationMode,
    /// Trigger metadata for keyword/path matching. Empty for legacy skills.
    pub triggers: SkillTriggers,
}

impl SkillIndexEntry {
    /// Convenience constructor for callers (and tests) that only care about the
    /// prompt-index fields; trigger/invocation metadata default to empty.
    #[must_use]
    pub fn new(name: String, description: Option<String>, path: PathBuf) -> Self {
        Self {
            name,
            description,
            path,
            invocation_mode: SkillInvocationMode::default(),
            triggers: SkillTriggers::default(),
        }
    }
}

/// Project-local context injected into the rendered system prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectContext {
    pub cwd: PathBuf,
    pub project_root: Option<PathBuf>,
    pub current_date: String,
    pub git_status: Option<String>,
    pub git_diff: Option<String>,
    pub instruction_files: Vec<ContextFile>,
    /// Global per-project `memory/MEMORY.md` index for `cwd`, if any.
    /// Only the compact index is carried into the prompt — full entries stay
    /// on disk and the model reads them on demand, keeping token cost low.
    pub memory_index: Option<ContextFile>,
    /// Skills discovered from Zo roots plus enabled local Claude Code/Codex
    /// user and plugin catalogs.
    /// Only frontmatter metadata is carried; the skill body is loaded on demand.
    pub skills_index: Vec<SkillIndexEntry>,
    /// Whether this turn is shown the index or told about the tools that
    /// replace it ([`SkillsIndexRoad`]).
    ///
    /// Decided once, when the context is discovered, and not per request: a
    /// prompt prefix that changed road mid-session would break the cache it
    /// exists to keep, and a person who moves the switch is one restart away
    /// from the road they chose.
    pub skills_index_road: SkillsIndexRoad,
}

impl ProjectContext {
    pub fn discover(
        cwd: impl Into<PathBuf>,
        current_date: impl Into<String>,
    ) -> std::io::Result<Self> {
        let cwd = cwd.into();
        let instruction_started = std::time::Instant::now();
        let instruction_files = discover_instruction_files(&cwd);
        log_prompt_boot_component("prompt/instructions", instruction_started);
        let memory_started = std::time::Instant::now();
        let memory_index = discover_memory_index(&cwd);
        log_prompt_boot_component("prompt/memory-index", memory_started);
        let skills_started = std::time::Instant::now();
        let skills_index = discover_skills_index(&cwd);
        let skills_index_road = discover_skills_index_road(&cwd, &skills_index);
        log_prompt_boot_component("prompt/skills-index", skills_started);
        Ok(Self {
            cwd,
            project_root: None,
            current_date: current_date.into(),
            git_status: None,
            git_diff: None,
            instruction_files,
            memory_index,
            skills_index,
            skills_index_road,
        })
    }

    /// The context a SUB-AGENT's prompt is built from: the same local walk
    /// as [`Self::discover`] plus the project root, and no `git status`
    /// snapshot (t-2902).
    ///
    /// The snapshot is one `git` subprocess — 15–28 ms on macOS's `/usr/bin/git`
    /// shim — and every `Agent` call paid it between the parent's tool call and
    /// the child's first provider request, for a status that is stale by the
    /// time the child runs its first command and that the child can read for
    /// itself in one `git status`. Claude Code's spawn reaches the child's first
    /// request in 14 ms; nothing that forks a process fits in that. The root
    /// stays: instruction files and skill roots are discovered against it, and
    /// [`crate::git_snapshot::read_git_root`] answers it without a process once
    /// the session's own boot has asked.
    pub fn discover_for_subagent(
        cwd: impl Into<PathBuf>,
        current_date: impl Into<String>,
    ) -> std::io::Result<Self> {
        let cwd = cwd.into();
        let mut context = Self::discover(cwd.clone(), current_date)?;
        context.project_root = read_git_root(&cwd);
        Ok(context)
    }

    pub fn discover_with_git(
        cwd: impl Into<PathBuf>,
        current_date: impl Into<String>,
    ) -> std::io::Result<Self> {
        // Two independent bodies of work, so they run at the same time.
        //
        // Each git fact is a separate `git` subprocess whose fork+exec latency
        // dominates, and those were already gathered concurrently — but they
        // started only *after* [`Self::discover`] had finished walking the
        // filesystem for instructions, memory and skills. Neither reads what
        // the other writes, so the wait was serial for no reason: measured
        // (`ZO_PROFILE_BOOT=1 zo --status`) 34.0ms of local scanning followed
        // by 40.8ms of git, 74.8ms of the 76.9ms the whole prompt cost.
        //
        // Spawning the git side first makes the two overlap, so the cost is the
        // slower one instead of the sum.
        //
        // The full working-tree diff is intentionally NOT gathered: it never
        // reaches the system prompt (`render_project_context` omits it; the
        // model runs `git diff` on demand), so spawning it would be pure
        // startup tax.
        let cwd = cwd.into();
        let git_cwd = cwd.clone();
        let overlapped = std::time::Instant::now();
        let (context, root, status) = std::thread::scope(|scope| {
            let root = scope.spawn(|| read_git_root(&git_cwd));
            let status = scope.spawn(|| read_git_status(&git_cwd));
            // The local walk runs on this thread while those two are in flight.
            let context = Self::discover(cwd.clone(), current_date);
            (
                context,
                root.join().ok().flatten(),
                status.join().ok().flatten(),
            )
        });
        // The components logged inside now sum to MORE than this total. That is
        // the overlap, and it is the point.
        log_prompt_boot_component("prompt/scan+git", overlapped);
        let mut context = context?;
        context.project_root = root;
        context.git_status = status;
        Ok(context)
    }
}

fn log_prompt_boot_component(label: &str, started: std::time::Instant) {
    if std::env::var_os("ZO_PROFILE_BOOT").is_some() {
        eprintln!(
            "[BOOT-COMPONENT] {label}={}us",
            started.elapsed().as_micros()
        );
    }
}

/// Builder for the runtime system prompt and dynamic environment sections.
///
/// `Default` is written out rather than derived: `Self::spawn_family` must
/// default to **on**, and a derived `bool` defaults to `false` — which would
/// silently drop the delegation rubric from every prompt, not just the
/// `--no-spawn` ones.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPromptBuilder {
    output_style_name: Option<String>,
    output_style_prompt: Option<String>,
    os_name: Option<String>,
    os_version: Option<String>,
    append_sections: Vec<String>,
    project_context: Option<ProjectContext>,
    config: Option<RuntimeConfig>,
    mode: PromptMode,
    /// Model this prompt is built for — the session model on the main loop, a
    /// sub-agent's resolved model on a spawn. Rendered as the `Model family:`
    /// line so the model is told what it actually is. `None` (no bound model
    /// yet) falls back to [`frontier_model_name`].
    model: Option<String>,
    /// Whether this session can actually delegate. `false` drops
    /// `# Delegation and workflow routing` — every one of its bullets names
    /// `Agent`, `SpawnMultiAgent` or `Workflow`, so with the spawn family
    /// turned off (`--no-spawn`) it is 1,315 tokens teaching the model to use
    /// tools it will be refused. Defaults to `true`: delegation is on unless a
    /// launch flag says otherwise.
    spawn_family: bool,
    /// The knowledge vault this session should consult, when one is configured
    /// and the session is not already running inside it.
    second_brain: Option<crate::second_brain::SecondBrain>,
    /// Session-fixed reminders a host supplies at build time. Empty by default,
    /// and empty on the live interactive path: a session's reminders change
    /// with its query, and a system section that changes mid-session re-bills
    /// the whole transcript (see `ConversationRuntime::transient_reminders`),
    /// so the live path sends them as transient wire reminders instead. What
    /// this slot is for is the cold view — `zo --prompt-input` measuring what a
    /// harness costs before a turn exists — and any host whose reminders really
    /// are fixed for the session.
    reminders: Vec<ReminderCandidate>,
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self {
            output_style_name: None,
            output_style_prompt: None,
            os_name: None,
            os_version: None,
            append_sections: Vec::new(),
            project_context: None,
            config: None,
            mode: PromptMode::default(),
            model: None,
            // Delegation is on unless a launch flag turns it off.
            spawn_family: true,
            second_brain: None,
            reminders: Vec::new(),
        }
    }
}

impl SystemPromptBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_output_style(mut self, name: impl Into<String>, prompt: impl Into<String>) -> Self {
        self.output_style_name = Some(name.into());
        self.output_style_prompt = Some(prompt.into());
        self
    }

    #[must_use]
    pub fn with_os(mut self, os_name: impl Into<String>, os_version: impl Into<String>) -> Self {
        self.os_name = Some(os_name.into());
        self.os_version = Some(os_version.into());
        self
    }

    #[must_use]
    pub fn with_project_context(mut self, project_context: ProjectContext) -> Self {
        self.project_context = Some(project_context);
        self
    }

    #[must_use]
    pub fn with_runtime_config(mut self, config: RuntimeConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Turn the delegation rubric off for a session that cannot delegate.
    #[must_use]
    pub fn with_spawn_family(mut self, spawn_family: bool) -> Self {
        self.spawn_family = spawn_family;
        self
    }

    #[must_use]
    pub fn with_mode(mut self, mode: PromptMode) -> Self {
        self.mode = mode;
        self
    }

    /// Bind the model this prompt is built for. See `Self::model`.
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Name the second-brain vault this session should consult. Absent by
    /// default: a session with no vault builds exactly the prompt it built
    /// before the vault existed.
    #[must_use]
    pub fn with_second_brain(mut self, vault: crate::second_brain::SecondBrain) -> Self {
        self.second_brain = Some(vault);
        self
    }

    /// Offer session-fixed reminders, already ranked. See
    /// `SystemPromptBuilder::reminders` for why the live session does not use
    /// this and [`render_reminders`] for what it renders.
    #[must_use]
    pub fn with_reminders(mut self, reminders: Vec<ReminderCandidate>) -> Self {
        self.reminders = reminders;
        self
    }

    #[must_use]
    pub fn build(&self) -> Vec<String> {
        let mut sections = Vec::new();
        // An experiment arm may hold static policy out (`ZO_ABLATE_PROMPT`).
        // A malformed spec is loud rather than ignored — see
        // [`ablation::PromptAblation`] — but this is a `#[must_use] -> Vec`,
        // so the loudness lands on stderr here and the run continues on the
        // production arm rather than silently reporting a holdout it did not
        // apply.
        let held_out = match ablation::PromptAblation::from_env() {
            Ok(set) => set,
            Err(error) => {
                eprintln!("zo: {error}; continuing with the full prompt");
                ablation::PromptAblation::none()
            }
        };
        sections.push(get_simple_intro_section(self.output_style_name.is_some()));
        sections.push(get_response_style_contract_section());
        if let (Some(name), Some(prompt)) = (&self.output_style_name, &self.output_style_prompt) {
            sections.push(format!("# Output Style: {name}\n{prompt}"));
        }
        sections.push(get_simple_system_section());
        sections.push(get_responding_section());
        sections.push(get_clarification_section());
        sections.push(get_default_coding_harness_section());
        sections.push(get_grounding_in_code_section());
        sections.push(get_simple_doing_tasks_section());
        if self.spawn_family {
            sections.push(get_delegation_section());
        }
        sections.push(get_actions_section());
        if let Some(discipline) = get_turn_discipline_section(self.mode) {
            sections.push(discipline);
        }
        sections.push(get_skills_and_docs_section());
        sections.push(get_memory_protocol_section());
        sections.push(get_context_trust_label_section());
        // Filter once, here, rather than at each push: the arm is a property of
        // the whole static region, and a `retain` cannot forget a section a
        // future push adds.
        if !held_out.is_production_arm() {
            // Say the arm out loud, once. A treatment that applies silently is
            // the failure this module's own docs name — the operator cannot
            // tell a measured holdout from a spec that never took, and stderr
            // is durable here (an interactive run sends it to
            // `~/.zo/logs/zo-ide.log`).
            eprintln!(
                "zo: prompt ablation arm — holding out {}",
                held_out.keys().join(", ")
            );
            sections.retain(|section| {
                section
                    .lines()
                    .next()
                    .is_none_or(|heading| !held_out.suppresses(heading))
            });
        }
        sections.push(SYSTEM_PROMPT_DYNAMIC_BOUNDARY.to_string());
        sections.push(self.environment_section());
        if let Some(project_context) = &self.project_context {
            sections.push(render_project_context(project_context));
            let always_instruction_files = project_context
                .instruction_files
                .iter()
                .filter(|file| file.is_always_instruction())
                .cloned()
                .collect::<Vec<_>>();
            let scoped_instruction_files = project_context
                .instruction_files
                .iter()
                .filter(|file| !file.is_always_instruction())
                .cloned()
                .collect::<Vec<_>>();
            if !always_instruction_files.is_empty() {
                sections.push(render_instruction_files(&always_instruction_files));
            }
            if !scoped_instruction_files.is_empty() {
                sections.push(render_scoped_instruction_index(&scoped_instruction_files));
            }
            if !project_context.skills_index.is_empty() {
                sections.push(match project_context.skills_index_road {
                    SkillsIndexRoad::Index => render_skills_index(&project_context.skills_index),
                    SkillsIndexRoad::Tools => {
                        render_skills_tools(project_context.skills_index.len())
                    }
                });
            }
            if let Some(memory) = &project_context.memory_index {
                sections.push(render_memory_index(memory));
            }
        }
        if let Some(vault) = &self.second_brain {
            sections.push(render_second_brain_section(vault));
        }
        if let Some(config) = &self.config {
            sections.push(render_config_section(config));
        }
        // Behind the boundary, deliberately and only here: a reminder is the
        // freshest thing in the prompt and the least cacheable, so it sits
        // after everything a turn can reuse.
        sections.extend(render_reminders(&self.reminders, &[]));
        sections.extend(self.append_sections.iter().cloned());
        sections
    }

    #[must_use]
    pub fn render(&self) -> String {
        self.build().join("\n\n")
    }

    fn environment_section(&self) -> String {
        let cwd = self.project_context.as_ref().map_or_else(
            || "unknown".to_string(),
            |context| context.cwd.display().to_string(),
        );
        let date = self.project_context.as_ref().map_or_else(
            || "unknown".to_string(),
            |context| context.current_date.clone(),
        );
        let model_family = model_family_line_value(self.model.as_deref());
        let mut lines = vec![ENVIRONMENT_SECTION_HEADING.to_string()];
        lines.extend(prepend_bullets(vec![
            format!("{MODEL_FAMILY_LABEL}{model_family}"),
            format!("Working directory: {cwd}"),
            format!("Date: {date}"),
            format!(
                "Platform: {} {}",
                self.os_name.as_deref().unwrap_or("unknown"),
                self.os_version.as_deref().unwrap_or("unknown")
            ),
        ]));
        lines.join("\n")
    }
}

/// Heading of the environment-context section carrying the `Model family:`
/// line. Shared by the builder and [`retarget_prompt_model`].
const ENVIRONMENT_SECTION_HEADING: &str = "# Environment context";
/// Bullet label preceding the model-family value.
const MODEL_FAMILY_LABEL: &str = "Model family: ";

/// The `Model family:` value for `model`.
///
/// An id the built-in catalog does not carry (custom provider, user-added row)
/// renders as its bare wire id — telling the model its real id beats naming a
/// family it does not belong to. `None` falls back to [`frontier_model_name`].
fn model_family_line_value(model: Option<&str>) -> String {
    model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map_or_else(frontier_model_name, |model| {
            crate::model_catalog::model_family_label(model)
                .unwrap_or_else(|| api::wire_model_id(model))
        })
}

/// Point an already-built prompt's `Model family:` line at `model`.
///
/// A live `/model` swap must not leave the prompt telling the model it is the
/// model it just replaced. Rewriting the one line is why this exists instead of
/// rebuilding: a full rebuild re-reads git status, config, and instruction
/// files, which would both cost I/O on a slash command and churn the dynamic
/// cache block for a change that only touches four words.
///
/// A prompt without an environment section (a micro-prompt harness) is left
/// untouched.
pub fn retarget_prompt_model(sections: &mut [String], model: &str) {
    let value = model_family_line_value(Some(model));
    let bullet = format!(" - {MODEL_FAMILY_LABEL}");
    for section in sections
        .iter_mut()
        .filter(|section| section.starts_with(ENVIRONMENT_SECTION_HEADING))
    {
        if !section.contains(&bullet) {
            continue;
        }
        *section = section
            .lines()
            .map(|line| {
                if line.starts_with(&bullet) {
                    format!("{bullet}{value}")
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
}

/// Formats each item as an indented bullet for prompt sections.
#[must_use]
pub fn prepend_bullets(items: Vec<String>) -> Vec<String> {
    items.into_iter().map(|item| format!(" - {item}")).collect()
}

/// The Claude Code identity line. On the Anthropic OAuth (Claude Max) path the
/// `system` array's FIRST text block must be exactly this string — any other
/// first block is rejected as a client-fingerprint mismatch, which the API
/// surfaces as a 429 rate-limit error.
pub const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Lower a joined system prompt into wire-level [`api::SystemBlock`]s with the
/// Claude Code identity isolated and cache breakpoints placed for maximal reuse.
///
/// The prompt is cut into up to three blocks:
/// 1. the Claude Code identity line — kept verbatim as the first block with
///    **no** `cache_control` (Claude Max OAuth fingerprint requirement, see
///    [`CLAUDE_CODE_IDENTITY`]);
/// 2. the static scaffolding (everything up to [`SYSTEM_PROMPT_DYNAMIC_BOUNDARY`])
///    as its own 1h-cached block, identical across turns and sessions;
/// 3. the dynamic context after the boundary — stable within a session.
///
/// The boundary marker itself is removed so it never reaches the model.
///
/// This is the single source of truth for *every* Anthropic-bound request —
/// the foreground TUI/headless turn and background sub-agents alike. The
/// sub-agent path previously sent the whole prompt as one plain block
/// (identity not isolated, no cache breakpoints), which the OAuth path
/// rejects as a fingerprint mismatch: the zo-only agent 429s.
#[must_use]
pub fn split_system_with_identity(system_text: &str) -> Vec<api::SystemBlock> {
    let mut blocks = Vec::with_capacity(3);
    let body = if let Some(rest) = system_text.strip_prefix(CLAUDE_CODE_IDENTITY) {
        blocks.push(api::SystemBlock::Text {
            text: CLAUDE_CODE_IDENTITY.to_string(),
            cache_control: None,
        });
        rest
    } else {
        system_text
    };
    match body.split_once(SYSTEM_PROMPT_DYNAMIC_BOUNDARY) {
        Some((static_part, dynamic_part)) => {
            push_cache_block(&mut blocks, static_part);
            push_cache_block(&mut blocks, dynamic_part);
        }
        None => push_cache_block(&mut blocks, body),
    }
    blocks
}

/// Push `text` (trimmed of surrounding blank lines) as a 1h-cached system
/// block, skipping it when empty.
fn push_cache_block(blocks: &mut Vec<api::SystemBlock>, text: &str) {
    let text = text.trim_matches('\n');
    if !text.is_empty() {
        blocks.push(api::SystemBlock::Text {
            text: text.to_string(),
            cache_control: Some(api::CacheControl::ephemeral_1h()),
        });
    }
}

fn discover_instruction_files(cwd: &Path) -> Vec<ContextFile> {
    let mut directories = Vec::new();
    let mut cursor = Some(cwd);
    while let Some(dir) = cursor {
        directories.push(dir.to_path_buf());
        cursor = dir.parent();
    }
    directories.reverse();

    let mut files = Vec::new();
    for dir in directories {
        for candidate in [
            dir.join("context.md"),
            dir.join("CONTEXT.md"),
            dir.join("context.local.md"),
            dir.join("CONTEXT.local.md"),
            dir.join(".zo").join("context.md"),
            dir.join(".zo").join("CONTEXT.md"),
        ] {
            push_context_file(&mut files, candidate);
        }
    }
    dedupe_instruction_files(files)
}

/// Find the persistent-memory index for `cwd` in Zo's global per-project
/// store (`memory/MEMORY.md`) and then the machine-local overlay
/// (`memory.local/MEMORY.md`). Read-only and best-effort: any IO error or
/// absent file yields `None` so a missing memory store never blocks prompt
/// assembly. Only the index is loaded as a locator; query-aware merged recall of
/// the global stores happens in `memory::recall`, and individual entries stay on
/// disk for on-demand reads, keeping the per-turn token cost to the small index.
fn discover_memory_index(cwd: &Path) -> Option<ContextFile> {
    for path in crate::memory::paths::memory_index_candidates(cwd) {
        if let Ok(content) = fs::read_to_string(&path) {
            if !content.trim().is_empty() {
                return Some(ContextFile::new(path, content));
            }
        }
    }
    None
}

fn discover_skills_index(cwd: &Path) -> Vec<SkillIndexEntry> {
    discover_skills_index_inner(cwd)
}

/// Discover the active project + global skills for `cwd` — the same set the
/// system prompt's `# Available skills` index lists (proposed skills excluded).
/// Public so the per-turn skill router can re-scan for implicit invocation
/// without rebuilding the whole prompt; re-scanning each turn matches Claude
/// Code/Codex live change detection.
#[must_use]
pub fn discover_skills(cwd: &Path) -> Vec<SkillIndexEntry> {
    discover_skills_index_inner(cwd)
}

fn discover_skills_index_inner(cwd: &Path) -> Vec<SkillIndexEntry> {
    let mut entries = Vec::new();
    // The catalog is the single source of trusted skill candidates in precedence
    // order (project Zo → global Zo → enabled Claude → enabled Codex), so
    // provider skills flow into the prompt index and the implicit router
    // identically to Zo skills. Per-candidate parsing (and the proposed-draft
    // filter) still happens here, and `push_unique_skill_entry` keeps the
    // highest-precedence entry on a name collision.
    for candidate in crate::skill_sources::SkillCatalog::discover(cwd).candidates() {
        let Ok(content) = fs::read_to_string(&candidate.skill_md) else {
            continue;
        };
        if let Some(entry) = parse_skill_index_entry(candidate.skill_md.clone(), &content) {
            push_unique_skill_entry(&mut entries, entry);
        }
        if entries.len() >= MAX_SKILL_INDEX_ENTRIES {
            entries.truncate(MAX_SKILL_INDEX_ENTRIES);
            return entries;
        }
    }
    entries
}

/// Every directory skills are discovered from for `cwd`, in precedence order:
/// walk-up `.zo/skills` → the Zo global homes. Only Zo roots are read; skills
/// from another tool must be placed (or symlinked) under a Zo root. The single source of
/// truth shared by the prompt index, the per-turn skill router, and the
/// `Skill` tool's loader — the loader once kept its own copy of this walk,
/// drifted, and answered "unknown skill" for a skill the index had just
/// advertised.
#[must_use]
pub fn skill_search_roots(cwd: &Path) -> Vec<PathBuf> {
    let project_root = read_git_root(cwd);
    let mut directories = Vec::new();
    let mut cursor = Some(cwd);
    while let Some(dir) = cursor {
        directories.push(dir.to_path_buf());
        if project_root.as_ref().is_some_and(|root| dir == root) {
            break;
        }
        cursor = dir.parent();
    }
    if let Some(project_root) = project_root {
        if !directories.iter().any(|dir| dir == &project_root) {
            directories.push(project_root);
        }
    }

    let mut skill_roots = Vec::new();
    for dir in &directories {
        push_unique_path(&mut skill_roots, dir.join(".zo").join("skills"));
    }
    for dir in zo_global_skill_roots() {
        push_unique_path(&mut skill_roots, dir);
    }
    skill_roots
}

fn zo_global_skill_roots() -> Vec<PathBuf> {
    // Resolve the global skill homes through the single source of truth so the
    // lookup order (`ZO_CONFIG_HOME` → `ZO_HOME` → `~/.zo`) stays
    // identical to sessions, agents, and MCP discovery.
    crate::config::zo_global_config_roots()
        .into_iter()
        .map(|root| root.join("skills"))
        .collect()
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn parse_skill_index_entry(path: PathBuf, content: &str) -> Option<SkillIndexEntry> {
    let mut name = None;
    let mut description = None;
    let mut state = None;
    let mut invocation_mode = SkillInvocationMode::default();
    let mut triggers = SkillTriggers::default();

    let fields = parse_frontmatter_fields(content);
    for (key, value) in &fields {
        match key.as_str() {
            "name" => name = Some(value.clone()),
            "description" => description = Some(value.clone()),
            "state" => state = Some(value.clone()),
            "invocation" | "invocation_mode" | "invocationMode" => {
                if let Some(mode) = SkillInvocationMode::parse(value) {
                    invocation_mode = mode;
                }
            }
            "keyword" | "keywords" | "triggers.keywords" | "trigger_keywords" => {
                push_trigger_values(&mut triggers.keywords, value);
            }
            "trigger_path" | "trigger_paths" | "triggers.paths" | "paths" => {
                push_trigger_values(&mut triggers.paths, value);
            }
            "exclude" | "excludes" | "triggers.excludes" | "trigger_excludes" => {
                push_trigger_values(&mut triggers.excludes, value);
            }
            _ => {}
        }
    }
    if state
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("proposed"))
    {
        return None;
    }

    let fallback_name = path.parent()?.file_name()?.to_string_lossy().into_owned();
    let name = name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback_name);
    let description = description.filter(|value| !value.trim().is_empty());
    Some(SkillIndexEntry {
        name,
        description,
        path,
        invocation_mode,
        triggers,
    })
}

/// Append the frontmatter value(s) for a trigger key, skipping blanks. Reuses
/// [`split_frontmatter_list_value`] so inline `[a, b]` arrays and repeated
/// `- item` list lines (already flattened to repeated key/value pairs by
/// [`parse_frontmatter`]) are handled identically.
fn push_trigger_values(target: &mut Vec<String>, value: &str) {
    for item in split_frontmatter_list_value(value) {
        let trimmed = item.trim();
        if !trimmed.is_empty() && !target.iter().any(|existing| existing == trimmed) {
            target.push(trimmed.to_string());
        }
    }
}

fn parse_frontmatter_fields(content: &str) -> Vec<(String, String)> {
    parse_frontmatter(content).map_or_else(Vec::new, |(fields, _body)| fields)
}

fn parse_frontmatter(content: &str) -> Option<(Vec<(String, String)>, &str)> {
    let after_open = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let mut fields = Vec::new();
    let mut current_list_key: Option<String> = None;
    let mut body_start = content.len();
    let open_len = content.len() - after_open.len();
    let mut consumed = 0;

    for line in after_open.split_inclusive('\n') {
        consumed += line.len();
        let trimmed = line.trim();
        if trimmed == "---" {
            body_start = open_len + consumed;
            break;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            if let Some(key) = &current_list_key {
                fields.push((
                    key.clone(),
                    trim_frontmatter_scalar(item.trim()).to_string(),
                ));
            }
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            current_list_key = None;
            continue;
        };
        let key = key.trim().to_string();
        let value = value.trim();
        if value.is_empty() {
            current_list_key = Some(key);
        } else {
            fields.push((key, trim_frontmatter_scalar(value).to_string()));
            current_list_key = None;
        }
    }

    if body_start == content.len() {
        return None;
    }
    Some((fields, &content[body_start..]))
}

fn trim_frontmatter_scalar(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|stripped| stripped.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|stripped| stripped.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn parse_globs(fields: &[(String, String)]) -> Vec<String> {
    fields
        .iter()
        .filter(|(key, _)| matches!(key.as_str(), "glob" | "globs"))
        .flat_map(|(_, value)| split_frontmatter_list_value(value))
        .filter(|value| !value.trim().is_empty())
        .collect()
}

fn parse_always_apply(fields: &[(String, String)]) -> Option<bool> {
    fields
        .iter()
        .rev()
        .find(|(key, _)| matches!(key.as_str(), "alwaysApply" | "always_apply"))
        .and_then(
            |(_, value)| match value.trim().to_ascii_lowercase().as_str() {
                "true" => Some(true),
                "false" => Some(false),
                _ => None,
            },
        )
}

fn split_frontmatter_list_value(value: &str) -> Vec<String> {
    let trimmed = value.trim();
    let list = trimmed
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(trimmed);
    list.split(',')
        .map(|item| trim_frontmatter_scalar(item.trim()).to_string())
        .collect()
}

fn push_unique_skill_entry(entries: &mut Vec<SkillIndexEntry>, entry: SkillIndexEntry) {
    if entries
        .iter()
        .any(|existing| existing.name == entry.name || existing.path == entry.path)
    {
        return;
    }
    entries.push(entry);
}

fn push_context_file(files: &mut Vec<ContextFile>, path: PathBuf) {
    match fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => {
            let expanded = expand_context_imports(&content, &path);
            files.push(ContextFile::instruction(path, &expanded));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        // Instruction files are best-effort context, and the ancestor walk
        // probes fixed candidate paths in every directory up to the
        // filesystem root — an unreadable probe there (permissions on a
        // parent directory, a transient error on a shared temp root) must
        // not kill session startup. Warn and boot without the file.
        Err(error) => {
            eprintln!(
                "warning: skipping unreadable instruction file {}: {error}",
                path.display()
            );
        }
    }
}

/// Maximum `@path` import recursion depth, matching Claude Code's 5-hop limit.
/// Beyond this an unexpanded `@path` token is left verbatim so a deep (or
/// cyclic) chain degrades gracefully rather than looping or exploding the prompt.
const MAX_CONTEXT_IMPORT_DEPTH: usize = 5;

/// Inline `@path` import references inside a `context.md` instruction file.
/// A whitespace-
/// delimited `@<path>` token on a non-code line is replaced with the (recursively
/// expanded) contents of the referenced file, resolved relative to the importing
/// file's directory (absolute `@/...` paths are honored too). The expansion is
/// deterministic and best-effort: a missing/unreadable target, a cycle, or a
/// depth-limit hit leaves the original `@path` token untouched. Tokens inside
/// fenced code blocks or inline code spans are not treated as imports.
fn expand_context_imports(content: &str, path: &Path) -> String {
    let mut visited = Vec::new();
    if let Ok(canonical) = fs::canonicalize(path) {
        visited.push(canonical);
    }
    expand_context_imports_inner(content, path, &mut visited, 0)
}

fn expand_context_imports_inner(
    content: &str,
    source_path: &Path,
    visited: &mut Vec<PathBuf>,
    depth: usize,
) -> String {
    if !content.contains('@') {
        return content.to_string();
    }
    let base_dir = source_path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = String::with_capacity(content.len());
    let mut in_fenced_block = false;
    for (index, line) in content.lines().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if is_code_fence_line(line) {
            in_fenced_block = !in_fenced_block;
            out.push_str(line);
            continue;
        }
        if in_fenced_block {
            out.push_str(line);
            continue;
        }
        out.push_str(&expand_import_line(line, base_dir, visited, depth));
    }
    // `lines()` drops a trailing newline; preserve it so re-joined content keeps
    // the source file's final-newline shape (keeps the expansion deterministic).
    if content.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Returns true for a fenced code-block delimiter (triple-backtick or
/// triple-tilde, optionally indented and followed by an info string), which
/// toggles code-block state.
fn is_code_fence_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

/// Expand every resolvable `@path` import token on a single non-code line.
fn expand_import_line(
    line: &str,
    base_dir: &Path,
    visited: &mut Vec<PathBuf>,
    depth: usize,
) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_inline_code = false;
    let mut chars = line.char_indices();
    while let Some((offset, ch)) = chars.next() {
        if ch == '`' {
            in_inline_code = !in_inline_code;
            out.push(ch);
            continue;
        }
        // An import token starts at `@` either at line start or after whitespace,
        // so mid-word `@` (e.g. an email address) is never treated as an import.
        let at_token_boundary = offset == 0
            || line[..offset]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        if in_inline_code || ch != '@' || !at_token_boundary {
            out.push(ch);
            continue;
        }
        let rest = &line[offset + ch.len_utf8()..];
        let token: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        match resolve_import(&token, base_dir, visited, depth) {
            Some(inlined) => {
                out.push_str(&inlined);
                // Skip the path token we just consumed from the iterator.
                for _ in 0..token.chars().count() {
                    chars.next();
                }
            }
            None => out.push(ch),
        }
    }
    out
}

/// Read and recursively expand the file named by an `@path` token, returning
/// `None` (so the caller leaves the token verbatim) when the token is not a
/// usable import: empty, depth-exhausted, missing/unreadable, or a cycle.
fn resolve_import(
    token: &str,
    base_dir: &Path,
    visited: &mut Vec<PathBuf>,
    depth: usize,
) -> Option<String> {
    if token.is_empty() || depth >= MAX_CONTEXT_IMPORT_DEPTH {
        return None;
    }
    let raw = Path::new(token);
    let candidate = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        base_dir.join(raw)
    };
    // Canonicalize for the cycle guard; this also fails fast on missing files.
    let canonical = fs::canonicalize(&candidate).ok()?;
    if visited.contains(&canonical) {
        return None;
    }
    let imported = fs::read_to_string(&canonical).ok()?;
    visited.push(canonical.clone());
    let expanded = expand_context_imports_inner(&imported, &canonical, visited, depth + 1);
    // Pop so sibling imports of the same file on later lines still expand once
    // each; only an active ancestor chain is treated as a cycle.
    visited.pop();
    Some(expanded)
}

fn read_git_status(cwd: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "--no-optional-locks",
            "status",
            "--short",
            "--branch",
            "--",
            ":(exclude)target",
            ":(exclude)node_modules",
            ":(exclude).build",
        ])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(truncate_git_status_snapshot(trimmed))
    }
}

/// Clamp a `git status --short --branch` snapshot to a bounded size so a repo
/// with a huge working tree cannot blow up every request's system prompt. Keeps
/// the leading lines (the `## branch...ahead/behind` header sorts first, so it
/// is always retained) up to the line/char budget and appends a one-line summary
/// of what was dropped.
fn truncate_git_status_snapshot(text: &str) -> String {
    let total_lines = text.lines().count();
    let mut out = String::new();
    let mut kept = 0usize;
    for line in text.lines() {
        // +1 for the joining newline; compare on chars so multibyte paths are
        // measured the same way the budget is expressed.
        if kept >= MAX_GIT_STATUS_LINES
            || out.chars().count() + line.chars().count() + 1 > MAX_GIT_STATUS_CHARS
        {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        kept += 1;
    }
    let omitted = total_lines.saturating_sub(kept);
    if omitted > 0 {
        use std::fmt::Write as _;
        // `write!` into a String is infallible; ignore the formatter Result.
        let _ = write!(
            out,
            "\n… ({omitted} more changed path(s) omitted; run `git status` for the full list)"
        );
    }
    out
}

fn render_project_context(project_context: &ProjectContext) -> String {
    let mut lines = vec!["# Project context".to_string()];
    let mut bullets = vec![
        format!("Today's date is {}.", project_context.current_date),
        format!("Working directory: {}", project_context.cwd.display()),
    ];
    if let Some(project_root) = &project_context.project_root {
        if project_root != &project_context.cwd {
            bullets.push(format!("Project root: {}.", project_root.display()));
        }
    }
    if !project_context.instruction_files.is_empty() {
        bullets.push(format!(
            "Project instruction files discovered: {}.",
            project_context.instruction_files.len()
        ));
    }
    if !project_context.skills_index.is_empty() {
        bullets.push(format!(
            "Project skills discovered: {}.",
            project_context.skills_index.len()
        ));
    }
    lines.extend(prepend_bullets(bullets));
    if let Some(status) = &project_context.git_status {
        lines.push(String::new());
        lines.push("Git status snapshot:".to_string());
        lines.push(status.clone());
    }
    // The full working-tree diff is intentionally absent from the prompt — it
    // burns input tokens every turn (the model runs `git diff` on demand), so
    // `discover_with_git` no longer even gathers it (`git_diff` stays `None`).
    lines.join("\n")
}

fn render_instruction_files(files: &[ContextFile]) -> String {
    let mut sections = vec!["# Project instructions".to_string()];
    let mut remaining_chars = MAX_TOTAL_INSTRUCTION_CHARS;
    for file in files {
        if remaining_chars == 0 {
            sections.push(
                "_Additional instruction content omitted after reaching the prompt budget._"
                    .to_string(),
            );
            break;
        }

        let rendered_content = render_instruction_content_for_file(file, remaining_chars);
        let consumed = rendered_content.chars().count().min(remaining_chars);
        remaining_chars = remaining_chars.saturating_sub(consumed);

        sections.push(format!("## {}", describe_instruction_file(file, files)));
        sections.push(rendered_content);
    }
    sections.join("\n\n")
}

fn render_scoped_instruction_index(files: &[ContextFile]) -> String {
    let mut lines = vec![
        "# Scoped instructions".to_string(),
        "These instruction files declare path globs and are not always applied. Read the file when the task touches a matching path; do not assume the rule applies globally.".to_string(),
    ];
    lines.extend(prepend_bullets(
        files
            .iter()
            .map(|file| {
                format!(
                    "{} (globs: {}; path: {})",
                    display_context_path(&file.path),
                    file.globs.join(", "),
                    file.path.display()
                )
            })
            .collect(),
    ));
    lines.join("\n")
}

fn dedupe_instruction_files(files: Vec<ContextFile>) -> Vec<ContextFile> {
    let mut deduped = Vec::new();
    let mut seen_hashes = Vec::new();

    for file in files {
        let normalized = normalize_instruction_content(&file.content);
        let hash = stable_content_hash(&normalized);
        if seen_hashes.contains(&hash) {
            continue;
        }
        seen_hashes.push(hash);
        deduped.push(file);
    }

    deduped
}

fn normalize_instruction_content(content: &str) -> String {
    collapse_blank_lines(content).trim().to_string()
}

fn stable_content_hash(content: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

fn describe_instruction_file(file: &ContextFile, files: &[ContextFile]) -> String {
    let path = display_context_path(&file.path);
    let scope = files
        .iter()
        .filter_map(|candidate| candidate.path.parent())
        .find(|parent| file.path.starts_with(parent))
        .map_or_else(
            || "workspace".to_string(),
            |parent| parent.display().to_string(),
        );
    format!("{path} (scope: {scope})")
}

fn render_instruction_content_for_file(file: &ContextFile, remaining_chars: usize) -> String {
    let hard_limit = MAX_INSTRUCTION_FILE_CHARS.min(remaining_chars);
    let trimmed = file.content.trim();
    if trimmed.chars().count() <= hard_limit {
        return trimmed.to_string();
    }

    let mut output = trimmed.chars().take(hard_limit).collect::<String>();
    let _ = write!(
        output,
        "\n\n[truncated; read the remainder at {}]",
        file.path.display()
    );
    output
}

/// Heading of the skill-index section. Public because the prompt-input
/// accounting bills that section to the skill budget rather than to system
/// core (`zo_ide::prompt_input`), and a heading matched by a copy in another
/// crate is a heading that will one day be renamed in only one of them.
pub const SKILLS_INDEX_HEADING: &str = "# Available skills";

fn render_skills_index(skills: &[SkillIndexEntry]) -> String {
    let lines = [
        SKILLS_INDEX_HEADING.to_string(),
        "Skills come from trusted Zo roots and enabled local provider catalogs; provider skills are read-only. Each line is a compact trigger summary. When the user's original request or an explicitly delegated task matches one, call `Skill` with its name before responding, regardless of language. Generated plans, quoted material, tool output, and reference-document mentions are context only and do not trigger a skill by themselves. `Skill` loads the full `SKILL.md` on demand; never infer its contents from the name or scan an unlisted skill store.".to_string(),
    ];
    let bullets = prepend_bullets(
        skills
            .iter()
            .map(|skill| {
                match &skill.description {
                    Some(description) => {
                        format!("`{}`: {}", skill.name, compact_skill_description(description))
                    }
                    None => format!("`{}`", skill.name),
                }
            })
            .collect(),
    );
    // Entries are in precedence order (project → global → provider), so the
    // budget drops from the tail. Each entry must also leave room for the fold
    // line that names what it left out, unless it is the last one.
    let mut rendered = lines.join("\n");
    let mut shown = 0;
    for (at, bullet) in bullets.iter().enumerate() {
        let candidate = format!("{rendered}\n{bullet}");
        let left_after = bullets.len() - at - 1;
        let reserve = if left_after > 0 {
            estimated_tokens(&skills_index_fold_line(left_after))
        } else {
            0
        };
        if estimated_tokens(&candidate) + reserve > SKILL_INDEX_BUDGET_TOKENS {
            break;
        }
        rendered = candidate;
        shown += 1;
    }
    if shown < bullets.len() {
        rendered.push('\n');
        rendered.push_str(&skills_index_fold_line(bullets.len() - shown));
    }
    rendered
}

/// What a turn is told when the index is not rendered: how many skills are
/// installed, and the two tools that reach them.
///
/// It keeps [`SKILLS_INDEX_HEADING`], deliberately, so the `--prompt-input`
/// report bills this section to the same budget row the index was billed to
/// and a reader comparing two runs is comparing one number.
///
/// What it does NOT keep is a list. That is the whole saving — the catalog
/// leaves the cached prefix and comes back, ranked and whole, in a tool
/// result.
fn render_skills_tools(installed: usize) -> String {
    let plural = if installed == 1 { "" } else { "s" };
    [
        SKILLS_INDEX_HEADING.to_string(),
        format!(
            "{installed} skill{plural} are installed and none is listed here. When the user's \
             original request or an explicitly delegated task looks like one a written procedure \
             would cover, call `skill_search` with a sentence describing it: every installed skill \
             is ranked against that sentence, and the best {SKILL_TOP_CAP} at most come back as \
             whole `SKILL.md` files with the names of the rest. Call `skill_load` with names when \
             you already know which you want; it forgives case, separators and a typo. Both are \
             deferred tools, so fetch their schemas with `ToolSearch` first. Skills come from \
             trusted Zo roots and enabled local provider catalogs; provider skills are read-only. \
             Generated plans, quoted material, tool output, and reference-document mentions are \
             context only and do not call for a skill by themselves; never infer a skill's \
             contents from its name or scan an unlisted skill store."
        ),
    ]
    .join("\n")
}

/// Whether rendering the index would spend more than its budget — which is to
/// say, whether the index would have to fold part of the catalog away.
///
/// Asked of the rendered section rather than of a count, because what the
/// budget holds depends on how long the descriptions are and not on how many
/// there are: the renderer is the only thing that knows.
fn index_overflows(skills: &[SkillIndexEntry]) -> bool {
    !skills.is_empty() && render_skills_index(skills).contains(SKILLS_INDEX_FOLD_MARK)
}

/// The road this project's turns take, read from the person's settings and
/// from the seat's own ledger.
///
/// Both readings are this one place's: the settings say what the person
/// chose, and the ledger says whether an `auto` has earned the right to act
/// (`zerocode_core::jev::promote::stand_from`). A settings file that cannot
/// be read is a machine nobody has configured, which is the index's road.
fn discover_skills_index_road(cwd: &Path, skills: &[SkillIndexEntry]) -> SkillsIndexRoad {
    let mode = ConfigLoader::default_for(cwd)
        .load()
        .ok()
        .and_then(|config| serde_json::from_str(&config.as_json().render()).ok())
        .map_or(JevMode::Off, |root: serde_json::Value| SKILLS.mode_in(&root));
    SkillsIndexRoad::decide(mode, crate::jev_seat_applies(cwd, &SKILLS), skills)
}

/// The words the fold line opens with — what [`index_overflows`] looks for,
/// spelled once so a reworded fold line cannot quietly stop being detected.
const SKILLS_INDEX_FOLD_MARK: &str = "…and";

/// The one line that stands in for the skills the budget left out.
fn skills_index_fold_line(left_out: usize) -> String {
    let plural = if left_out == 1 { "" } else { "s" };
    format!(
        " - {SKILLS_INDEX_FOLD_MARK} {left_out} more installed skill{plural} not listed here; `Skill` loads any of them by name."
    )
}

/// Collapse frontmatter formatting and bound only the always-present prompt
/// copy. Skill discovery and the implicit router retain the complete original
/// description, and the `Skill` tool still loads the selected body + asset
/// directory by name, so compacting this catalog does not weaken resolution.
fn compact_skill_description(description: &str) -> String {
    let collapsed = description.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_SKILL_DESCRIPTION_CHARS {
        return collapsed;
    }

    let mut prefix = collapsed
        .chars()
        .take(MAX_SKILL_DESCRIPTION_CHARS.saturating_sub(1))
        .collect::<String>();
    if let Some(word_boundary) = prefix.rfind(char::is_whitespace) {
        if word_boundary >= MAX_SKILL_DESCRIPTION_CHARS / 2 {
            prefix.truncate(word_boundary);
        }
    }
    prefix.push('…');
    prefix
}

fn display_context_path(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn collapse_blank_lines(content: &str) -> String {
    let mut result = String::new();
    let mut previous_blank = false;
    for line in content.lines() {
        let is_blank = line.trim().is_empty();
        if is_blank && previous_blank {
            continue;
        }
        result.push_str(line.trim_end());
        result.push('\n');
        previous_blank = is_blank;
    }
    result
}

/// Loads config and project context, then renders the system prompt text.
///
/// This variant never applies the configured output style — it is the prompt
/// for sub-agents and informational surfaces. The main conversation loop uses
/// [`load_system_prompt_for_main`], which is the only consumer of the
/// `outputStyle` setting (Claude Code parity: styles shape the main loop, not
/// delegated agents).
///
/// `model` is the model this prompt will run on (a sub-agent's resolved model);
/// `None` falls back to [`frontier_model_name`].
pub fn load_system_prompt(
    cwd: impl Into<PathBuf>,
    current_date: impl Into<String>,
    os_name: impl Into<String>,
    os_version: impl Into<String>,
    model: Option<&str>,
) -> Result<Vec<String>, PromptBuildError> {
    load_system_prompt_with_options(
        &cwd.into(),
        current_date.into(),
        os_name,
        os_version,
        model,
        false,
        PromptMode::Subagent,
        // A sub-agent keeps the rubric: it can still delegate further, and the
        // host's `--no-spawn` is enforced at the wire, not by this prompt.
        true,
    )
}

/// [`load_system_prompt`] plus the configured `outputStyle` (settings key or
/// `/output-style`), resolved via [`output_style::resolve`].
pub fn load_system_prompt_for_main(
    cwd: impl Into<PathBuf>,
    current_date: impl Into<String>,
    os_name: impl Into<String>,
    os_version: impl Into<String>,
    model: Option<&str>,
) -> Result<Vec<String>, PromptBuildError> {
    load_system_prompt_for_main_with_mode(
        cwd,
        current_date,
        os_name,
        os_version,
        model,
        PromptMode::Interactive,
    )
}

/// [`load_system_prompt_for_main`] with an explicit [`PromptMode`], for
/// surfaces where nobody can answer mid-task questions (headless one-shots).
pub fn load_system_prompt_for_main_with_mode(
    cwd: impl Into<PathBuf>,
    current_date: impl Into<String>,
    os_name: impl Into<String>,
    os_version: impl Into<String>,
    model: Option<&str>,
    mode: PromptMode,
) -> Result<Vec<String>, PromptBuildError> {
    load_system_prompt_for_main_with_mode_and_spawn(
        cwd,
        current_date,
        os_name,
        os_version,
        model,
        mode,
        true,
    )
}

/// [`load_system_prompt_for_main_with_mode`] plus the host's answer to "can
/// this session delegate?". `false` drops the delegation rubric — see
/// [`SystemPromptBuilder::with_spawn_family`].
pub fn load_system_prompt_for_main_with_mode_and_spawn(
    cwd: impl Into<PathBuf>,
    current_date: impl Into<String>,
    os_name: impl Into<String>,
    os_version: impl Into<String>,
    model: Option<&str>,
    mode: PromptMode,
    spawn_family: bool,
) -> Result<Vec<String>, PromptBuildError> {
    load_system_prompt_with_options(
        &cwd.into(),
        current_date.into(),
        os_name,
        os_version,
        model,
        true,
        mode,
        spawn_family,
    )
}

#[allow(clippy::too_many_arguments)] // 평평한 인자 목록 — 옵션 구조체는 별건이다.
fn load_system_prompt_with_options(
    cwd: &Path,
    current_date: String,
    os_name: impl Into<String>,
    os_version: impl Into<String>,
    model: Option<&str>,
    apply_output_style: bool,
    mode: PromptMode,
    spawn_family: bool,
) -> Result<Vec<String>, PromptBuildError> {
    let prompt_started = std::time::Instant::now();
    // A sub-agent's context skips the `git status` subprocess (see
    // `discover_for_subagent`); the main loop's carries it.
    let project_context = match mode {
        PromptMode::Subagent => ProjectContext::discover_for_subagent(cwd, current_date)?,
        PromptMode::Interactive | PromptMode::Autonomous => {
            ProjectContext::discover_with_git(cwd, current_date)?
        }
    };
    let config_started = std::time::Instant::now();
    let config = ConfigLoader::default_for(cwd).load()?;
    log_prompt_boot_component("prompt/config-load", config_started);
    let style_started = std::time::Instant::now();
    let style = apply_output_style
        .then(|| {
            config
                .get("outputStyle")
                .and_then(|value| value.as_str())
                .and_then(|name| output_style::resolve(cwd, name))
        })
        .flatten();
    log_prompt_boot_component("prompt/output-style", style_started);
    let render_started = std::time::Instant::now();
    // Rendered for every session a vault is configured for, INCLUDING one whose
    // cwd is the vault itself.
    //
    // That case looks like the one to skip — the vault carries an `AGENTS.md`
    // stating the same rules — but zo never reads it: `discover_instruction_files`
    // above collects `context.md`/`CONTEXT.md`/`.zo/context.md` and nothing
    // else. Skipping there would leave the session sitting *inside* the second
    // brain as the only one told nothing about it.
    let second_brain = crate::second_brain::SecondBrain::resolve(&config);
    let mut builder = SystemPromptBuilder::new()
        .with_os(os_name, os_version)
        .with_project_context(project_context)
        .with_runtime_config(config)
        .with_mode(mode)
        .with_spawn_family(spawn_family);
    if let Some(vault) = second_brain {
        builder = builder.with_second_brain(vault);
    }
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        builder = builder.with_model(model);
    }
    if let Some((name, prompt)) = style {
        builder = builder.with_output_style(name, prompt);
    }
    let built = builder.build();
    log_prompt_boot_component("prompt/render", render_started);
    log_prompt_boot_component("prompt/total", prompt_started);
    Ok(built)
}


// ---------------------------------------------------------------------------
// Reminders: what this project learned, offered back before it is asked for.
// ---------------------------------------------------------------------------

/// Heading of the reminder block.
pub const REMINDERS_SECTION_HEADING: &str = "# What this project has already learned";

/// Reminder lines one block may carry.
///
/// A cap on attention, not on bytes. Three lessons at the top of a turn are
/// read; a tenth line is scrolled past, and the cost of the ones nobody reads
/// is paid on every request.
pub const MAX_REMINDER_LINES: usize = 3;

/// Estimated tokens the whole block may cost, in the `chars / 4 + 1` unit every
/// budget in this repo is written in — the same estimator
/// `zo_ide::prompt_input` bills the Reminders bucket with, so the guard here
/// and the report there can never disagree about one block.
pub const MAX_REMINDER_TOKENS: usize = 300;

/// Prefix of every reminder line. Named so the tests, the counter and the e2e
/// all look for the same seven characters.
pub const REMINDER_LINE_PREFIX: &str = "reminder: ";

/// Characters one lesson may spend, so a single long lesson cannot take the
/// whole block and leave the other two unsaid.
const MAX_REMINDER_LESSON_CHARS: usize = (MAX_REMINDER_TOKENS * 4) / MAX_REMINDER_LINES;

/// One promoted lesson offered as a reminder.
///
/// Candidates arrive already ranked — the caller has the retriever and the
/// promotion order, this function has neither. Ranking is deliberately not
/// this function's job: it renders and it enforces the budget, and both are
/// testable without a memory store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderCandidate {
    /// The lesson's memory slug, shown so the model can go read the entry.
    pub slug: String,
    /// The lesson itself, one line.
    pub lesson: String,
}

/// Render up to [`MAX_REMINDER_LINES`] reminders, or `None` when there is
/// nothing left to say.
///
/// `already_recalled` are the slugs the recalled-memory section is carrying
/// this turn. They are dropped rather than repeated: an entry whose summary is
/// already in the request does not become more true for being said twice, and
/// the duplicate is the whole block's budget spent on nothing.
///
/// The budget is enforced by dropping whole lines, never by cutting the block
/// mid-sentence — a truncated reminder is a reminder of something else.
#[must_use]
pub fn render_reminders(
    candidates: &[ReminderCandidate],
    already_recalled: &[String],
) -> Option<String> {
    let mut section = String::from(REMINDERS_SECTION_HEADING);
    let mut lines = 0usize;
    for candidate in candidates {
        if lines == MAX_REMINDER_LINES {
            break;
        }
        if already_recalled
            .iter()
            .any(|recalled| recalled == &candidate.slug)
        {
            continue;
        }
        let lesson = reminder_lesson(&candidate.lesson);
        if lesson.is_empty() {
            continue;
        }
        let line = format!("\n{REMINDER_LINE_PREFIX}{lesson} ({})", candidate.slug);
        if estimated_tokens(&(section.clone() + &line)) > MAX_REMINDER_TOKENS {
            // Whole lines only. A later candidate is shorter often enough that
            // trying the rest is worth one more comparison.
            continue;
        }
        section.push_str(&line);
        lines += 1;
    }
    (lines > 0).then_some(section)
}

/// One lesson as one line: collapsed whitespace, no leading wikilink, bounded
/// length, and no heading marker of its own that could look like the start of a
/// new section.
///
/// The wikilink matters. A summary that came from the vault opens with its own
/// `[[slug]] — `, and the line already ends with `(slug)`, so keeping both
/// spends a third of a reminder line naming the same page twice — measured on
/// the r49 bench, where three lines carried three redundant slugs.
fn reminder_lesson(lesson: &str) -> String {
    let collapsed = lesson
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_start_matches(['#', '-', '*', '>'])
        .trim()
        .to_string();
    let collapsed = strip_leading_wikilink(&collapsed);
    if collapsed.chars().count() <= MAX_REMINDER_LESSON_CHARS {
        return collapsed;
    }

    let cut: String = collapsed
        .chars()
        .take(MAX_REMINDER_LESSON_CHARS.saturating_sub(1))
        .collect();
    format!("{}…", cut.trim_end())
}

/// Drop a pointer summary's own `[[slug]] — ` opener. Only when the text really
/// starts with a wikilink, so an em dash inside ordinary prose is left alone.
fn strip_leading_wikilink(summary: &str) -> String {
    let Some(rest) = summary.strip_prefix("[[") else {
        return summary.to_string();
    };
    let Some((_, after)) = rest.split_once("]]") else {
        return summary.to_string();
    };
    after
        .trim_start()
        .strip_prefix("—")
        .map_or_else(|| summary.to_string(), |tail| tail.trim().to_string())
}

/// The repo's own token estimate: `chars / 4 + 1`, matching
/// [`crate::conversation::helpers::estimate_system_prompt_tokens`].
fn estimated_tokens(text: &str) -> usize {
    text.chars().count() / 4 + 1
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod reminder_tests {
    //! The reminder block: what it says, what it costs, and where it sits.

    use super::{
        render_reminders, ReminderCandidate, SystemPromptBuilder, MAX_REMINDER_LINES,
        MAX_REMINDER_TOKENS, REMINDERS_SECTION_HEADING, REMINDER_LINE_PREFIX,
        SYSTEM_PROMPT_DYNAMIC_BOUNDARY,
    };

    fn candidate(slug: &str, lesson: &str) -> ReminderCandidate {
        ReminderCandidate {
            slug: slug.to_string(),
            lesson: lesson.to_string(),
        }
    }

    fn lessons(count: usize) -> Vec<ReminderCandidate> {
        (0..count)
            .map(|index| {
                candidate(
                    &format!("gotcha-{index}"),
                    &format!("lesson number {index} about a real trap"),
                )
            })
            .collect()
    }

    fn estimated_tokens(text: &str) -> usize {
        text.chars().count() / 4 + 1
    }

    #[test]
    fn nothing_to_say_renders_nothing() {
        assert_eq!(render_reminders(&[], &[]), None);
        assert_eq!(
            render_reminders(&lessons(1), &["gotcha-0".to_string()]),
            None,
            "the only candidate was already recalled, so the block is empty"
        );
    }

    #[test]
    fn a_reminder_names_the_lesson_and_its_slug() {
        let section =
            render_reminders(&[candidate("gotcha-zsh-pipe", "check `$?` without a pipe")], &[])
                .expect("section");
        assert_eq!(
            section,
            format!("{REMINDERS_SECTION_HEADING}\n{REMINDER_LINE_PREFIX}check `$?` without a pipe (gotcha-zsh-pipe)")
        );
    }

    #[test]
    fn at_most_three_lines_however_many_were_offered() {
        let section = render_reminders(&lessons(20), &[]).expect("section");
        assert_eq!(
            section.matches(REMINDER_LINE_PREFIX).count(),
            MAX_REMINDER_LINES
        );
    }

    #[test]
    fn an_entry_the_recall_section_carries_is_never_repeated() {
        let section = render_reminders(
            &lessons(6),
            &["gotcha-0".to_string(), "gotcha-1".to_string()],
        )
        .expect("section");
        assert!(
            !section.contains("(gotcha-0)") && !section.contains("(gotcha-1)"),
            "a recalled entry does not become more true for being said twice: {section}"
        );
        assert!(section.contains("(gotcha-2)"), "{section}");
        assert_eq!(
            section.matches(REMINDER_LINE_PREFIX).count(),
            MAX_REMINDER_LINES,
            "the block refills from behind the recalled ones"
        );
    }

    #[test]
    fn a_vault_pointer_does_not_name_its_page_twice() {
        let section = render_reminders(
            &[candidate(
                "wiki/zo/gotcha-pipeline",
                "[[wiki/zo/gotcha-pipeline]] — \"a pipeline reports its last command\" · tags: zo",
            )],
            &[],
        )
        .expect("section");
        assert_eq!(
            section.matches("wiki/zo/gotcha-pipeline").count(),
            1,
            "the line already ends with the slug: {section}"
        );
        assert!(
            section.contains("a pipeline reports its last command"),
            "{section}"
        );
        assert!(
            render_reminders(&[candidate("gotcha-em-dash", "one thing — and another")], &[])
                .expect("section")
                .contains("one thing — and another"),
            "an em dash in ordinary prose is not a wikilink opener"
        );
    }

    #[test]
    fn the_block_never_exceeds_its_token_budget() {
        // Three lessons that would blow the budget on their own.
        let fat: Vec<ReminderCandidate> = (0..MAX_REMINDER_LINES)
            .map(|index| candidate(&format!("gotcha-{index}"), &"word ".repeat(600)))
            .collect();
        let section = render_reminders(&fat, &[]).expect("section");
        assert!(
            estimated_tokens(&section) <= MAX_REMINDER_TOKENS,
            "block estimated at {} tokens, over the {MAX_REMINDER_TOKENS} budget",
            estimated_tokens(&section)
        );
        assert!(
            section.contains('…'),
            "a lesson is trimmed to fit rather than dropping the whole block: {section}"
        );
    }

    #[test]
    fn a_lesson_is_collapsed_to_one_line() {
        let section = render_reminders(
            &[candidate(
                "gotcha-multiline",
                "# heading\n\n  first    half\n  second half\n",
            )],
            &[],
        )
        .expect("section");
        assert_eq!(
            section.lines().count(),
            2,
            "one heading, one reminder — a lesson cannot open a section of its own: {section}"
        );
        assert!(section.contains("heading first half second half"), "{section}");
    }

    #[test]
    fn the_block_sits_behind_the_cache_boundary() {
        let sections = SystemPromptBuilder::new()
            .with_reminders(lessons(2))
            .build();
        let boundary = sections
            .iter()
            .position(|section| section == SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
            .expect("boundary");
        let block = sections
            .iter()
            .position(|section| section.starts_with(REMINDERS_SECTION_HEADING))
            .expect("reminder block");
        assert!(
            block > boundary,
            "a reminder in front of the boundary would move the 1h-cached prefix \
             every time it changed (block {block}, boundary {boundary})"
        );
    }

    #[test]
    fn changing_the_reminders_leaves_the_cached_prefix_byte_identical() {
        let first = SystemPromptBuilder::new()
            .with_reminders(vec![candidate("gotcha-one", "the first lesson")])
            .render();
        let second = SystemPromptBuilder::new()
            .with_reminders(vec![candidate("gotcha-two", "an entirely different lesson")])
            .render();

        let prefix = |rendered: &str| {
            rendered
                .split_once(SYSTEM_PROMPT_DYNAMIC_BOUNDARY)
                .expect("boundary")
                .0
                .to_string()
        };
        assert_eq!(
            prefix(&first),
            prefix(&second),
            "the static, 1h-cached prefix must not move when a reminder does"
        );
        assert_ne!(
            first, second,
            "the reminder text itself did change — otherwise this proves nothing"
        );
        assert!(first.contains("the first lesson") && second.contains("an entirely different lesson"));
    }

    #[test]
    fn a_session_with_no_reminders_builds_exactly_what_it_built_before() {
        assert_eq!(
            SystemPromptBuilder::new().build(),
            SystemPromptBuilder::new().with_reminders(Vec::new()).build(),
            "an empty offer must add no section, not an empty one"
        );
    }
}


#[cfg(test)]
mod import_tests {
    use super::{
        expand_context_imports, push_context_file, ContextFile, MAX_CONTEXT_IMPORT_DEPTH,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be after epoch")
            .as_nanos();
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "runtime-prompt-import-{}-{nanos}-{counter}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn inlines_relative_at_path_reference() {
        let dir = temp_dir();
        fs::write(dir.join("conventions.md"), "Use tabs, not spaces.")
            .expect("write conventions");
        let root = dir.join("context.md");
        fs::write(&root, "Project rules:\n@./conventions.md\nDone.").expect("write root");

        let content = fs::read_to_string(&root).expect("read root");
        let expanded = expand_context_imports(&content, &root);

        assert!(
            expanded.contains("Use tabs, not spaces."),
            "relative @path import should be inlined: {expanded}"
        );
        assert!(
            !expanded.contains("@./conventions.md"),
            "the @path token should be consumed once inlined: {expanded}"
        );
    }

    #[test]
    fn missing_import_target_left_verbatim() {
        let dir = temp_dir();
        let root = dir.join("context.md");
        let content = "See @./does-not-exist.md for details.";
        let expanded = expand_context_imports(content, &root);
        assert_eq!(
            expanded, content,
            "an unresolvable @path must degrade to the original line"
        );
    }

    #[test]
    fn cycle_does_not_loop() {
        let dir = temp_dir();
        let a = dir.join("a.md");
        let b = dir.join("b.md");
        fs::write(&a, "A imports @./b.md").expect("write a");
        fs::write(&b, "B imports @./a.md").expect("write b");

        let content = fs::read_to_string(&a).expect("read a");
        // Must terminate (no infinite recursion) and break the cycle by leaving
        // the back-reference to the root verbatim.
        let expanded = expand_context_imports(&content, &a);
        assert!(
            expanded.contains("B imports"),
            "the first hop should inline: {expanded}"
        );
        assert!(
            expanded.contains("@./a.md"),
            "the cyclic back-reference must be left verbatim: {expanded}"
        );
    }

    #[test]
    fn depth_is_bounded() {
        let dir = temp_dir();
        // Chain of files each importing the next, longer than the depth limit.
        let chain_len = MAX_CONTEXT_IMPORT_DEPTH + 3;
        for i in 0..chain_len {
            let path = dir.join(format!("d{i}.md"));
            if i + 1 < chain_len {
                fs::write(&path, format!("level {i} @./d{}.md", i + 1)).expect("write level");
            } else {
                fs::write(&path, format!("level {i} END")).expect("write tail");
            }
        }

        let root = dir.join("d0.md");
        let content = fs::read_to_string(&root).expect("read root");
        let expanded = expand_context_imports(&content, &root);

        // Exactly MAX_CONTEXT_IMPORT_DEPTH hops expand; the file at the depth
        // limit keeps its @path token rather than recursing further.
        assert!(
            expanded.contains(&format!("level {MAX_CONTEXT_IMPORT_DEPTH}")),
            "should expand up to the depth limit: {expanded}"
        );
        assert!(
            expanded.contains(&format!("@./d{}.md", MAX_CONTEXT_IMPORT_DEPTH + 1)),
            "the token at the depth limit must be left verbatim: {expanded}"
        );
        assert!(
            !expanded.contains("END"),
            "expansion past the depth limit must not occur: {expanded}"
        );
    }

    #[test]
    fn import_inside_code_block_not_expanded() {
        let dir = temp_dir();
        fs::write(dir.join("secret.md"), "INLINED").expect("write secret");
        let root = dir.join("context.md");
        let content = "```\n@./secret.md\n```\nplain @./secret.md";
        let expanded = expand_context_imports(content, &root);

        // The fenced occurrence stays literal; the plain one is inlined.
        assert!(
            expanded.contains("@./secret.md\n```"),
            "fenced @path must not be expanded: {expanded}"
        );
        assert!(
            expanded.contains("plain INLINED"),
            "non-fenced @path must be expanded: {expanded}"
        );
    }

    #[test]
    fn push_context_file_expands_imports_into_stored_content() {
        let dir = temp_dir();
        fs::write(dir.join("conventions.md"), "House style applies.")
            .expect("write conventions");
        let root = dir.join("context.md");
        fs::write(&root, "Top.\n@./conventions.md").expect("write root");

        let mut files: Vec<ContextFile> = Vec::new();
        push_context_file(&mut files, root);

        let stored = &files.first().expect("one context file").content;
        assert!(
            stored.contains("House style applies."),
            "stored ContextFile content must carry the inlined import: {stored}"
        );
    }
}
