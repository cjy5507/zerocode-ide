//! The coding agents a lane can host.
//!
//! Each agent is a CLI that ZeroCode runs inside a PTY; the slug is both the
//! hook endpoint segment (`POST /hook/<slug>`) and the generated hook script
//! name (`<slug>-hook.sh`), so it must stay stable once shipped.

use serde::{Deserialize, Serialize};

use crate::capabilities::{
    AuthProbeKind, BlockedSignal, CLAUDE_EFFORT_LADDER, CLAUDE_EFFORT_PICKER,
    CLAUDE_RESUME_SELECTORS, CODEX_EFFORT_LADDER, Harness, IdShape, MoveRoad, PointerRoute,
    SpawnRoad, StoreResume, Submit, SubmitAck, TrustMenu, TurnMoves, WakeMark,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    /// This project's own harness (`zo`), the only agent ZeroCode drives over a
    /// socket rather than only through a PTY.
    Zo,
    Claude,
    Codex,
    Cursor,
    Droid,
    Copilot,
    Grok,
    Kimi,
    Devin,
    Antigravity,
    Opencode,
    CommandCode,
    /// Reports through a PLUGIN rather than a hook script — it loads code
    /// instead of reading a hook table. See `zerocode-hookd::plugin`.
    Amp,
}

/// What a new terminal should do when no action chose an agent explicitly.
///
/// This is a tagged value on the wire so `Auto` and `Blank` cannot collapse
/// into the same `null`, and an agent id cannot collide with a control-word
/// sentinel. The backend still validates the id against [`AGENT_SPECS`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DefaultAgentPreference {
    /// Use the configured terminal command, if any.
    #[default]
    Auto,
    /// Always start the user's plain shell.
    Blank,
    /// Launch this registered coding agent.
    Agent { id: String },
}

impl DefaultAgentPreference {
    #[must_use]
    pub fn from_legacy(value: &str) -> Option<Self> {
        if value == "blank" {
            return Some(Self::Blank);
        }
        agent_spec(value).map(|spec| Self::Agent {
            id: spec.id.to_string(),
        })
    }

    #[must_use]
    pub fn validated(self) -> Option<Self> {
        match self {
            Self::Agent { ref id } if agent_spec(id).is_none() => None,
            preference => Some(preference),
        }
    }

    #[must_use]
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Self::Agent { id } => Some(id),
            Self::Auto | Self::Blank => None,
        }
    }
}

/// Every agent, in the order the lane switcher lists them.
pub const ALL_AGENTS: [AgentKind; 13] = [
    AgentKind::Zo,
    AgentKind::Claude,
    AgentKind::Codex,
    AgentKind::Cursor,
    AgentKind::Droid,
    AgentKind::Copilot,
    AgentKind::Grok,
    AgentKind::Kimi,
    AgentKind::Devin,
    AgentKind::Antigravity,
    AgentKind::Opencode,
    AgentKind::CommandCode,
    AgentKind::Amp,
];

/// How this provider accepts context from a lifecycle hook.
///
/// This is a measured capability of the CLI, independent of which model the
/// person selects inside it. A new provider is one catalog entry here, not a
/// new branch in the hook bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookAdditionalContext {
    pub session_start_event: &'static str,
    pub prompt_submit_event: &'static str,
    /// Event after which the provider will rebuild model context without a
    /// fresh SessionStart. The next prompt restores the contract.
    pub context_reset_event: Option<&'static str>,
}

impl AgentKind {
    pub const fn slug(self) -> &'static str {
        match self {
            AgentKind::Zo => "zo",
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Cursor => "cursor",
            AgentKind::Droid => "droid",
            AgentKind::Copilot => "copilot",
            AgentKind::Grok => "grok",
            AgentKind::Kimi => "kimi",
            AgentKind::Devin => "devin",
            AgentKind::Antigravity => "antigravity",
            AgentKind::Opencode => "opencode",
            AgentKind::CommandCode => "command-code",
            AgentKind::Amp => "amp",
        }
    }

    /// Whether this provider reports prompt submission through a lifecycle
    /// transport the window can observe.
    ///
    /// This is a provider capability, not an orchestration special case.
    /// Providers whose first event is only a tool/agent-start event cannot
    /// acknowledge that Enter consumed a composer draft and use the terminal-
    /// delivery fallback instead.
    ///
    /// Read off the agent's row ([`AgentSpec::harness`], `submit_ack`), not
    /// spelled here: this project's own harness emits `turn{phase:"start"}`
    /// on its pane event channel and the hook agents send
    /// `UserPromptSubmit`; the shell folds either into the same one-shot
    /// acknowledgement. A row with [`SubmitAck::None`] reports nothing.
    pub fn reports_prompt_submit(self) -> bool {
        agent_spec(self.slug()).is_some_and(|spec| spec.harness.submit_ack != SubmitAck::None)
    }

    /// Whose `SubagentStop` closes the SPAWN rather than the helper.
    ///
    /// zo keys its `SubagentStart`/`SubagentStop` pair by the spawning tool
    /// call and fires the stop when that call RETURNS. For a background
    /// `Agent` that is milliseconds after the start, while the child runs on
    /// in a pane of its own; the child's real life travels by its agent id —
    /// the `background_tasks` roll call the same stop carries, and the
    /// `subagents` frame (zo-ide `ide/reporter.rs`, the stated LIMIT of the
    /// pair). A stop from such a vendor finishes nothing. Claude's pair
    /// brackets the helper itself — its background helpers never fire the
    /// pair at all — so its stop is a finish, and so is everybody else's.
    pub const fn subagent_stop_closes_the_spawn(self) -> bool {
        matches!(self, Self::Zo)
    }

    /// Event names and empty-output contract for providers whose command hook
    /// can inject `hookSpecificOutput.additionalContext` into the model.
    ///
    /// Verified against the installed Claude 2.1.246 and Codex 0.149.1 CLI
    /// bundles. Other providers keep reporting lifecycle state exactly as
    /// before; adding one after measuring its output schema is data-only here.
    pub const fn hook_additional_context(self) -> Option<HookAdditionalContext> {
        match self {
            Self::Claude | Self::Codex => Some(HookAdditionalContext {
                session_start_event: "SessionStart",
                prompt_submit_event: "UserPromptSubmit",
                context_reset_event: None,
            }),
            _ => None,
        }
    }

    /// Human label for the lane header. Kept beside `slug` so the two can never
    /// drift into separate lookup tables.
    pub const fn label(self) -> &'static str {
        match self {
            AgentKind::Zo => "ZO",
            AgentKind::Claude => "Claude",
            AgentKind::Codex => "Codex",
            AgentKind::Cursor => "Cursor",
            AgentKind::Droid => "Droid",
            AgentKind::Copilot => "Copilot",
            AgentKind::Grok => "Grok",
            AgentKind::Kimi => "Kimi",
            AgentKind::Devin => "Devin",
            AgentKind::Antigravity => "Antigravity",
            AgentKind::Opencode => "OpenCode",
            AgentKind::CommandCode => "Command Code",
            AgentKind::Amp => "Amp",
        }
    }

    /// The primary executable name in the terminal catalogue.
    ///
    /// `AgentSpec::detect` is the one authority for a binary name. The lane
    /// enum and the terminal catalogue answer different questions, but a
    /// hook parser or PATH shim must not keep a third copy of that spelling.
    /// The fallback is only for a future lane whose terminal entry has not
    /// landed yet; it keeps the wire enum usable while making the omission
    /// visible to the registry parity test.
    pub fn command(self) -> &'static str {
        agent_spec(self.slug())
            .map(|spec| spec.detect)
            .unwrap_or_else(|| self.slug())
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        ALL_AGENTS.into_iter().find(|kind| kind.slug() == slug)
    }
}

/* ---- the registry: which agents this window knows how to drive -------------
 *
 * A different question from `AgentKind` above, and the two are deliberately
 * separate sets. `AgentKind` is the WIRE type: its slug is a hook endpoint
 * (`POST /hook/<slug>`) and a generated script name, so it lists the agents
 * that report events back to us. This lists the agents we know how to start
 * in a terminal and hand a prompt to — which is a much larger set, because
 * driving a TUI needs no cooperation from it.
 *
 * Measured from Orca's `TUI_AGENT_CONFIG` (`store-BgJxB0hr.js:1295-1527`)
 * crossed with `TUI_AGENT_DISPLAY_NAMES` (`plugin-manifest-Dq3wpxrr.js:4384`),
 * and generated from that reading rather than transcribed: thirty-four
 * agents times a dozen fields is not something to copy by hand. A thirty-fifth
 * entry is our own and was measured here — see the addition below.
 *
 * TWO DELIBERATE OMISSIONS, both about not pretending to be another product:
 *
 *   - Orca's list has a thirty-fifth entry whose `detectCmd` is `orca` and
 *     whose `launchCmd` runs Orca's own CLI. Driving a competitor's binary is
 *     not a feature of this window, and its name is a trademark that does not
 *     belong in shipped code.
 *   - two agents accept a prefill through an `ORCA_*_PREFILL` environment
 *     variable — their CLIs read it by name. Setting it would be this window
 *     telling an agent it is Orca. They take the bracketed-paste path
 *     instead, which is what thirty of these agents use anyway and which
 *     `zerocode-pty` already implements.
 *
 * AND ONE ADDITION THAT IS NOT ORCA'S: `zo`, this project's own harness, is a
 * terminal REPL like every other entry here and belongs in the list a new
 * terminal reads its default from. It is also the one agent `AgentKind` above
 * drives over a socket — the same program, answered for by both catalogues
 * because they are asked different questions. Its fields are measured off the
 * program in a pty, not read out of anybody's bundle.
 */

/// How a LAUNCH prompt reaches an agent — and it runs, whichever road.
///
/// Only the typed roads need the readiness machine. The rest hand the prompt
/// over on the command line, where there is nothing to wait for. Drafts are
/// a different question with a different door ([`AgentSpec::draft_flag`],
/// [`AgentSpec::takes_a_paste`]): a draft is seeded without submitting, a
/// launch prompt is executed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Injection {
    /// As an argument when the agent starts.
    Argv,
    /// Behind a flag that carries the prompt.
    FlagPrompt,
    /// Behind a flag that carries the prompt AND asks for interactive mode.
    FlagPromptInteractive,
    /// Behind a flag that only asks for interactive mode.
    FlagInteractive,
    /// Typed at the agent once it is up and listening. The paste path.
    StdinAfterStart,
    /// This agent's own query subcommand.
    HermesQuery,
}

/// What an agent does to say "I am reading keys now".
///
/// The four Orca distinguishes (`DraftPasteReadySignal`,
/// draft-paste-ready-scanner.ts:36-70), and the four [`ReadySignal`] in
/// `zerocode-pty` implements. Thirty of the thirty-five give no signal at all
/// and are read by silence, which is why the quiet timer is not a fallback
/// here but the common case.
///
/// A glyph mark carries its glyph: which character a composer wears is the
/// agent's fact, measured on that agent, so it lives on the agent's row and
/// nowhere else — two agents wear `❯` for different reasons and under
/// different doors.
///
/// [`ReadySignal`]: https://docs.rs/zerocode-pty
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadyMark {
    /// Nothing observable: 1500ms of no output after the handshake.
    Quiet,
    /// `ESC[?25h` — DECTCEM, the cursor being shown.
    CursorShown,
    /// This glyph drawn after the handshake: the composer line standing.
    /// No quiet floor — silence is exactly what a program gives while it is
    /// still between input handlers.
    ComposerPrompt(char),
    /// This glyph too — but one that only counts inside the ALTERNATE SCREEN,
    /// because outside it the same character is the default prompt of popular
    /// shells (starship, pure); silence stays on as the floor for launches
    /// that render inline (Orca's `grok-composer-prompt`,
    /// draft-paste-ready-scanner.ts:15-21,49-63).
    AltScreenPrompt(char),
}

/// How a running agent's composer may be cleared before a fresh send.
///
/// These keys are not a terminal property: Codex consumes Ctrl+U/Ctrl+K as
/// editor commands, while Claude and the other measured TUIs may insert the
/// same C0 bytes as literal text. An unmeasured agent therefore gets `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComposerClear {
    Keys,
    None,
}

/// How a restart continuation reaches a resumed agent.
///
/// This is independent of launch injection: a provider may accept an initial
/// prompt on argv while its resume subcommand only restores the conversation.
/// `Argv` is reserved for a resume spelling measured to submit the extra
/// argument; every other agent takes the same ready-composer road as a live
/// send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NudgeRoad {
    Argv,
    Composer,
}

/// One agent, and everything needed to start it and talk to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentSpec {
    /// Stable id. Orca's own key, kept so a measurement can be re-checked
    /// against the source it came from.
    pub id: &'static str,
    /// Whether a send to an already-running composer may use edit keys first.
    pub composer_clear: ComposerClear,
    /// Where a continuation prompt goes after this agent is resumed.
    pub resume_nudge: NudgeRoad,
    pub name: &'static str,
    /// The vendor's own site, whose favicon is this agent's icon.
    ///
    /// Orca resolves an icon through a chain (`AgentIcon`,
    /// `agent-catalog-1Y3pTpm8.js:452-490`): hand-drawn marks for nine
    /// agents, favicon PNGs bundled into its build for the rest, then
    /// `https://www.google.com/s2/favicons?domain=…&sz=64` at runtime, then a
    /// letter tile. The drawn marks and bundled PNGs are third-party artwork
    /// and stay out of this repository, so every agent here goes through the
    /// runtime steps of that same chain. Where Orca's catalog names a
    /// `faviconDomain` it is kept verbatim; the nine agents Orca draws by
    /// hand name their vendor's own domain instead (`codex` → `openai.com`).
    ///
    /// Empty for `zo` alone, and deliberately: every other entry is somebody
    /// else's product with somebody else's site, while `zo` is this project's
    /// own harness and has no vendor to fetch a mark from. An invented domain
    /// would be a request to a host nobody owns; the empty string stops the
    /// chain one step early and the letter tile — the last step of that same
    /// chain — is its face.
    pub favicon_domain: &'static str,
    /// The vendor's page for this agent, kept verbatim from Orca's catalog
    /// (`homepageUrl`, `agent-catalog-CQp2IFaf.js:211-448`).
    ///
    /// One URL wearing two names: Orca's `AgentRow` ends every row with it
    /// behind an external-link icon titled `Docs` when the agent is detected
    /// and `Install` when it is not
    /// (`plugin-command-keybindings-Do9s02Z7.js:1290`) — which is what makes
    /// the "available to install" list actionable rather than a label.
    ///
    /// Empty for `zo` alone, for the same reason its favicon domain is: this
    /// project's own harness has no vendor page to send anybody to, and a
    /// link to an invented address would be worse than no link.
    pub homepage_url: &'static str,
    /// The command to look for on `PATH` to decide this agent is installed.
    pub detect: &'static str,
    /// Other names the same agent ships under.
    pub detect_aliases: &'static [&'static str],
    /// Commands that must ALSO be present — a wrapper is no use without the
    /// thing it wraps.
    pub requires: &'static [&'static str],
    /// Platforms this agent does not run on, as `std::env::consts::OS` spells
    /// them.
    pub unsupported: &'static [&'static str],
    /// What to run, verbatim — some agents need a subcommand.
    pub launch: &'static str,
    /// The process name to expect in the pty, which is not always the launch
    /// command: a wrapper execs the real thing.
    pub expected_process: &'static str,
    pub injection: Injection,
    /// The agent's own flag for seeding a DRAFT — text placed in its composer
    /// **without submitting**. Orca's `draftPromptFlag`, whose one entry says
    /// why in place: "`claude --prefill <text>` seeds the input without
    /// submitting, avoiding the paste-after-ready race"
    /// (`tui-agent-config.ts:59-60`). A LAUNCH prompt never rides this flag:
    /// argv injection puts it on the command line bare, where the agent runs
    /// it at once (`buildAgentStartupPlan`, `tui-agent-startup.ts:100-113`).
    /// Carrying a launch prompt behind `--prefill` was the bug where claude
    /// opened with the instruction sitting unsent in its composer.
    pub draft_flag: Option<&'static str>,
    /// The environment variable a draft can ride instead — Orca's
    /// `draftPromptEnvVar` (`ORCA_PI_PREFILL`, `ORCA_OMP_PREFILL`). Same
    /// contract as [`Self::draft_flag`]: seeds without submitting.
    pub draft_env: Option<&'static str>,
    /// Some agents want their prompt after a bare `--`.
    pub argv_separator: Option<&'static str>,
    pub ready: ReadyMark,
    /// A provider-specific quiet window after its bracketed-paste handshake.
    /// `None` uses the shared 1500ms. This is longer only when the provider
    /// draws an input line before a startup gate behind it can accept work.
    pub ready_quiet_ms: Option<u32>,
    /// The agent's own hard deadline for [`Self::ready`], in milliseconds —
    /// Orca's `draftPasteReadyTimeoutMs`, carried only where an agent needs
    /// more than the shared eight seconds. `None` means the shared default;
    /// the resolver lives with the delivery
    /// (`resolveDraftPasteReadyTimeoutMs`: override ?? this ?? 8000).
    pub ready_timeout_ms: Option<u32>,
    /// The harness facts the write doors ask about — spawn road,
    /// acknowledgement transport, trust menu, pointer route, resume
    /// selectors and store, wake witness — that used to be agent-name
    /// branches at those doors. [`Harness::PLAIN`] for a vendor TUI with
    /// none of them; see `capabilities.rs` for the answer a door reads.
    pub harness: Harness,
}

impl AgentSpec {
    /// Whether a DRAFT for this agent has to be typed at it.
    ///
    /// Orca's rule exactly (`agentDeliversDraftViaNativePrefill`): an agent
    /// with a native draft door — a flag or an environment variable that
    /// seeds its composer without submitting — is never pasted at. The
    /// readiness machine and the bracketed envelope exist for the others.
    /// Launch prompts are a different road entirely: they ride the injection
    /// table and DO run (`buildAgentStartupPlan`).
    pub const fn takes_a_paste(&self) -> bool {
        self.draft_flag.is_none() && self.draft_env.is_none()
    }

    /// Whether this agent can run on the platform named the way
    /// `std::env::consts::OS` names it.
    pub fn runs_on(&self, os: &str) -> bool {
        !self.unsupported.contains(&os)
    }

    /// Every name that means this agent on `PATH`.
    pub fn detect_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.detect).chain(self.detect_aliases.iter().copied())
    }
}

pub fn agent_spec(id: &str) -> Option<&'static AgentSpec> {
    AGENT_SPECS.iter().find(|spec| spec.id == id)
}

/// The spec whose expected process wears this name — the send menu's third
/// road (1-g7): a foreground program detected on a pty names the agent
/// running there, the way Orca's `expectedProcess` table does. Unambiguous
/// because no two specs share a process name; the test below holds that.
#[must_use]
pub fn agent_spec_by_process(program: &str) -> Option<&'static AgentSpec> {
    AGENT_SPECS
        .iter()
        .find(|spec| spec.expected_process == program)
}

/// How a launch prompt reaches an agent.
///
/// Two roads, and which one an agent takes is [`AgentSpec::injection`]'s fact,
/// not the caller's: argv mode puts the prompt on the command line BARE —
/// behind a separator where the agent wants one (`grok -- …`), positional
/// otherwise (`claude "…"`) — and the agent runs it at once. The flag modes
/// spell a fixed flag. Everyone else is typed at after their ready signal.
///
/// Never behind the DRAFT flag. `claude --prefill <text>` seeds the composer
/// **without submitting** (`draftPromptFlag`, tui-agent-config.ts:59-60), and
/// carrying a launch prompt on it left claude sitting with the instruction
/// unsent — the map's P0-1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Injected {
    /// Append these to the command line, in order.
    Argv(Vec<String>),
    /// Nothing goes on the command line; this is typed at the agent once it
    /// says it is listening.
    AfterStart(String),
}

/// Where this agent's launch prompt goes.
///
/// `None` for a prompt with nothing in it — an agent started to be worked with
/// by hand, which is not the same as one started with an empty instruction.
///
/// Lives here rather than at a launch site because it is a pure fact about a
/// spec, and because there is now more than one launch site: a person typing
/// into the launcher, and a coordinator summoning a worker. Two copies of this
/// table would be correct exactly until an agent changed its prompt flag.
#[must_use]
pub fn prompt_injection(spec: &AgentSpec, prompt: &str) -> Option<Injected> {
    if prompt.trim().is_empty() {
        return None;
    }
    // The road is the row's ([`AgentSpec::capabilities`]): the flag
    // spellings and the typed vendors are decided there, once, and this
    // only puts the words on it.
    Some(match spec.capabilities().submit {
        Submit::Argv { separator } => {
            let mut words = Vec::new();
            if let Some(separator) = separator {
                words.push(separator.to_string());
            }
            words.push(prompt.to_string());
            Injected::Argv(words)
        }
        Submit::Flag(flag) => Injected::Argv(vec![flag.to_string(), prompt.to_string()]),
        Submit::Typed => Injected::AfterStart(prompt.to_string()),
    })
}

/// The terminal catalogue: every agent this window can start in a pty.
///
/// It is a DIFFERENT list from [`ALL_AGENTS`], which names the agents a lane
/// can host, and the two answer different questions — `AgentKind` is the hook
/// endpoint and the lane header, `AgentSpec` is the command line and the
/// readiness signal. `zo` is in both, and had to be: it is driven over a
/// socket as a lane AND it is a terminal REPL like any other, so a catalogue
/// that only knew it as a lane left it out of the picker every new terminal
/// reads its default from. Neither entry is the other's copy — an id that
/// appears in one and not the other is a normal thing here, and the two are
/// looked up through separate doors (`agent_spec` / `AgentKind::from_slug`).
// A `static`, not a `const`: the table is read by reference everywhere, and
// a const this size would be copied whole at each use (clippy's
// `large_const_arrays`, crossed when the rows grew their between-turn move
// columns).
pub static AGENT_SPECS: [AgentSpec; 35] = [
    // First because it is this project's own harness: the list a person picks
    // a default from opens with the agent this window ships with.
    //
    // Measured in a pty rather than read off a help page. Started with no
    // arguments it is an interactive REPL, and its first output turns
    // bracketed paste on (`ESC[?2004h`) — no cursor toggle, no composer glyph
    // — so silence after the handshake is the only mark it gives and `Quiet`
    // is the measurement, not a fallback. It carries no prefill flag, so the
    // interactive door is a paste once it is ready.
    //
    // Re-measured 2026-08-28 against `zo 0.1.0`, the rebuilt CLI that now
    // lives in the zerocode-cli checkout. The flags this note used to name —
    // `--fullscreen`, `--allowedTools`, and a separate `zo prompt TEXT` — are
    // gone from its help, and `--model`/`--effort`/`--permission-mode`/
    // `--resume` stand in their place. The readiness mark and the paste door
    // both survived the rebuild; only the flag list had gone stale. `--effort`
    // is why `zo` now sits in `orchestration::TUNABLE`.
    AgentSpec {
        id: "zo",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "ZO",
        favicon_domain: "",
        homepage_url: "",
        detect: "zo",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "zo",
        expected_process: "zo",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            spawn: SpawnRoad::SocketPane,
            submit_ack: SubmitAck::Channel,
            blocked: &[BlockedSignal::TrustMenu(TrustMenu::Zo)],
            auth_probes: &[AuthProbeKind::IdentityFile, AuthProbeKind::Keychain],
            // `zo commands --json` (1.1.10, 2026-09-21) lists `/model` and no
            // `/effort`: the model takes its id on the line, the effort is
            // the second stage of that same picker — and zo moves its own
            // effort per request from inside (its step governor), so the
            // beat leaves it alone. Its ladder is its own `--effort` help.
            moves: TurnMoves {
                effort: Some(MoveRoad::Shown("/model")),
                model: Some(MoveRoad::Line("/model")),
                ladder: &[
                    "off", "low", "medium", "high", "xhigh", "max", "ultra", "smart",
                ],
            },
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "claude",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Argv,
        name: "Claude",
        favicon_domain: "claude.ai",
        homepage_url: "https://docs.anthropic.com/claude/docs/claude-code",
        detect: "claude",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "claude",
        expected_process: "claude",
        injection: Injection::Argv,
        draft_flag: Some("--prefill"),
        draft_env: None,
        argv_separator: None,
        // Silence was the measurement until Claude Code 2.1.274 (2026-09-17):
        // it turns bracketed paste on 0.3 s into its start, is silent for
        // three seconds, then takes that input handler down (`?2004l`) with
        // whatever was pasted into it and mounts its composer under a new
        // one. The quiet window opened inside that silence, and every worker
        // briefing went to the handler about to go. Every `❯` it draws comes
        // under the handler that stays — fullscreen and inline, 2.1.273 and
        // 2.1.274, fresh and resumed (zerocode-pty's recordings) — and a
        // paste right after the first one showed in the composer within
        // 131 ms in each. No quiet floor: silence is what it gives between
        // handlers.
        ready: ReadyMark::ComposerPrompt('\u{276f}'),
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            // Its folder-trust dialog stands in front of the composer in any
            // repository its config has not trusted, skip-permissions or not.
            blocked: &[BlockedSignal::TrustMenu(TrustMenu::Claude)],
            resume_selectors: CLAUDE_RESUME_SELECTORS,
            resume_store: Some(StoreResume {
                flag: "--resume",
                id: IdShape::Uuid,
            }),
            vault_resume_carries_launch_args: true,
            auth_probes: &[AuthProbeKind::IdentityFile, AuthProbeKind::Keychain],
            // Measured 2026-09-21 on 2.1.278 in a pty of its own
            // (docs/captures/claude-code-2.1.278-effort-slash.txt): `/effort
            // low` typed between two turns put `effort: low` on the next
            // assistant record — and "saved as your default for new
            // sessions" into the account's `modelSettings`, which the
            // picker's `s` does not. `/model <id>` takes the id the same way.
            moves: TurnMoves {
                effort: Some(MoveRoad::Picker(CLAUDE_EFFORT_PICKER)),
                model: Some(MoveRoad::Line("/model")),
                ladder: CLAUDE_EFFORT_LADDER,
            },
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "openclaude",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "OpenClaude",
        favicon_domain: "openclaude.gitlawb.com",
        homepage_url: "https://openclaude.gitlawb.com/",
        detect: "openclaude",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "openclaude",
        expected_process: "openclaude",
        injection: Injection::Argv,
        draft_flag: Some("--prefill"),
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "codex",
        composer_clear: ComposerClear::Keys,
        resume_nudge: NudgeRoad::Composer,
        name: "Codex",
        favicon_domain: "openai.com",
        homepage_url: "https://github.com/openai/codex",
        detect: "codex",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "codex",
        expected_process: "codex",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        // `›` (U+203A), printed on the composer line once it is drawn.
        ready: ReadyMark::ComposerPrompt('\u{203a}'),
        ready_quiet_ms: None,
        // Orca's one per-agent deadline (config `draftPasteReadyTimeoutMs:
        // 20_000`): codex routinely takes past the shared eight seconds to
        // mount its composer on a cold start, and the shared deadline firing
        // first dropped every slow launch's prompt.
        ready_timeout_ms: Some(20_000),
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            blocked: &[BlockedSignal::TrustMenu(TrustMenu::Codex)],
            pointer_route: Some(PointerRoute::CodexAppServer),
            wake_mark: WakeMark::Rollout,
            vault_resume_carries_launch_args: true,
            auth_probes: &[AuthProbeKind::IdentityFile],
            // Measured 2026-09-21 on 0.155.1 in a pty of its own: `/model
            // low` went to the model as a user message (the rollout holds
            // `"text":"/model low"` under `role: user`), `/reasoning` is
            // "Unrecognized command", and `/model` alone opens the two-stage
            // picker (zo-ide/docs/codex-tui-mechanics.md). So nothing the
            // beat types moves its effort; a summons' `-c
            // model_reasoning_effort=` does.
            moves: TurnMoves {
                effort: Some(MoveRoad::Relaunch),
                model: Some(MoveRoad::Shown("/model")),
                ladder: CODEX_EFFORT_LADDER,
            },
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "devin",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Devin",
        favicon_domain: "devin.ai",
        homepage_url: "https://devin.ai/cli",
        detect: "devin",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "devin",
        expected_process: "devin",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            vault_resume_carries_launch_args: true,
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "ante",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Ante",
        favicon_domain: "antigma.ai",
        homepage_url: "https://github.com/AntigmaLabs/ante-preview",
        detect: "ante",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "ante",
        expected_process: "ante",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "trae",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Trae",
        favicon_domain: "www.trae.cn",
        homepage_url: "https://docs.trae.cn/cli_get-started-with-trae-cli",
        detect: "traecli",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "traecli",
        expected_process: "traecli",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: Some("--"),
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "autohand",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Autohand Code",
        favicon_domain: "autohand.ai",
        homepage_url: "https://github.com/autohandai/code-cli",
        detect: "autohand",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "autohand",
        expected_process: "autohand",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "opencode",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "OpenCode",
        favicon_domain: "opencode.ai",
        homepage_url: "https://opencode.ai/docs/cli/",
        detect: "opencode",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "opencode",
        expected_process: "opencode",
        injection: Injection::FlagPrompt,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::CursorShown,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            vault_resume_carries_launch_args: true,
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "mimo-code",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "MiMo Code",
        favicon_domain: "mimo.xiaomi.com",
        homepage_url: "https://mimo.xiaomi.com/coder",
        detect: "mimo",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "mimo",
        expected_process: "mimo",
        injection: Injection::FlagPrompt,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::CursorShown,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "pi",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Pi",
        favicon_domain: "pi.dev",
        homepage_url: "https://pi.dev",
        detect: "pi",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "pi",
        expected_process: "pi",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            vault_resume_carries_launch_args: true,
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "omp",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "OMP",
        favicon_domain: "omp.sh",
        homepage_url: "https://omp.sh",
        detect: "omp",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "omp",
        expected_process: "omp",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            vault_resume_carries_launch_args: true,
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "prime-agent",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Prime Agent",
        favicon_domain: "primeintellect.ai",
        homepage_url: "https://github.com/PrimeIntellect-ai/prime-agent",
        detect: "prime-agent",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "prime-agent",
        expected_process: "prime-agent",
        injection: Injection::Argv,
        draft_flag: None,
        // No prefill variable: Pi has one because its startup races a paste,
        // and the original gives Prime Agent none.
        draft_env: None,
        // `prime-agent [options] [@files...] [message...]` takes the task as
        // positional argv, so a prompt starting with `help`, `agents` or a dash
        // would be read as a subcommand or a flag. Its own help documents `--`
        // as "treat all following arguments as messages".
        argv_separator: Some("--"),
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "antigravity",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Antigravity",
        favicon_domain: "antigravity.google",
        homepage_url: "https://antigravity.google/docs/cli-overview",
        detect: "agy",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "agy",
        expected_process: "agy",
        injection: Injection::FlagPromptInteractive,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        // agy 1.1.22 enables bracketed paste before authentication has
        // finished. Its composer appeared around 2.5s, its account gate
        // repainted around 5.4s, and the shared 1.5s quiet window submitted at
        // 4.0s into the gate that discarded the prompt. Four quiet seconds
        // after the last startup paint landed the same prompt every run.
        ready_quiet_ms: Some(4_000),
        ready_timeout_ms: Some(20_000),
        harness: Harness {
            blocked: &[
                BlockedSignal::TrustMenu(TrustMenu::Antigravity),
                BlockedSignal::LoginGate,
            ],
            vault_resume_carries_launch_args: true,
            // Gemini CLI's `/model` opens its manage dialog (the console table's
            // own note) and `--effort` is a launch flag on this machine
            // (`orchestration::TUNABLE`): shown and relaunched, no ladder read.
            moves: TurnMoves {
                effort: Some(MoveRoad::Relaunch),
                model: Some(MoveRoad::Shown("/model")),
                ladder: &[],
            },
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "aider",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Aider",
        favicon_domain: "aider.chat",
        homepage_url: "https://aider.chat/docs/",
        detect: "aider",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "aider",
        expected_process: "aider",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "goose",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Goose",
        favicon_domain: "goose-docs.ai",
        homepage_url: "https://block.github.io/goose/docs/quickstart/",
        detect: "goose",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "goose",
        expected_process: "goose",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "amp",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Amp",
        favicon_domain: "ampcode.com",
        homepage_url: "https://ampcode.com/manual#install",
        detect: "amp",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "amp",
        expected_process: "amp",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "kilo",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Kilocode",
        favicon_domain: "kilo.ai",
        homepage_url: "https://kilo.ai/docs/cli",
        detect: "kilo",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "kilo",
        expected_process: "kilo",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "kiro",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Kiro",
        favicon_domain: "kiro.dev",
        homepage_url: "https://kiro.dev/docs/cli/",
        detect: "kiro-cli",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "kiro-cli chat --tui",
        expected_process: "kiro-cli",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "crush",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Charm",
        favicon_domain: "charm.sh",
        homepage_url: "https://github.com/charmbracelet/crush",
        detect: "crush",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "crush",
        expected_process: "crush",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "aug",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Auggie",
        favicon_domain: "augmentcode.com",
        homepage_url: "https://docs.augmentcode.com/cli/overview",
        detect: "auggie",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "auggie",
        expected_process: "auggie",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "cline",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Cline",
        favicon_domain: "cline.bot",
        homepage_url: "https://docs.cline.bot/cline-cli/overview",
        detect: "cline",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "cline",
        expected_process: "cline",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "codebuff",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Codebuff",
        favicon_domain: "codebuff.com",
        homepage_url: "https://www.codebuff.com/docs/help/quick-start",
        detect: "codebuff",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "codebuff",
        expected_process: "codebuff",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "command-code",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Command Code",
        favicon_domain: "commandcode.ai",
        homepage_url: "https://commandcode.ai/docs/quickstart",
        detect: "command-code",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "command-code --trust",
        expected_process: "command-code",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "continue",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Continue",
        favicon_domain: "continue.dev",
        homepage_url: "https://docs.continue.dev/guides/cli",
        detect: "cn",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "cn",
        expected_process: "cn",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "cursor",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Cursor",
        favicon_domain: "cursor.com",
        homepage_url: "https://cursor.com/cli",
        // The CLI's own name is `agent`; `cursor-agent` is the legacy link
        // the installer still makes beside it — "Create symlinks to the
        // Cursor Agent executable (primary: agent, legacy: cursor-agent)"
        // (cursor.com/install, the script the docs hand out). Both point at
        // one executable FILE named `cursor-agent`, which is why the process
        // to expect in a pty keeps that name while the command to run and
        // detect is the one the docs now spell (t-6120, t-6009's survey).
        detect: "agent",
        detect_aliases: &["cursor-agent"],
        requires: &[],
        unsupported: &[],
        launch: "agent",
        expected_process: "cursor-agent",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            blocked: &[BlockedSignal::TrustMenu(TrustMenu::Cursor)],
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "droid",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Droid",
        favicon_domain: "factory.ai",
        homepage_url: "https://docs.factory.ai/cli/getting-started/quickstart",
        detect: "droid",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "droid",
        expected_process: "droid",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            vault_resume_carries_launch_args: true,
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "kimi",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Kimi",
        favicon_domain: "moonshot.cn",
        homepage_url: "https://www.kimi.com/code/docs/en/kimi-code-cli/getting-started.html",
        detect: "kimi",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "kimi",
        expected_process: "kimi",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "mistral-vibe",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Mistral Vibe",
        favicon_domain: "mistral.ai",
        homepage_url: "https://github.com/mistralai/mistral-vibe",
        detect: "vibe",
        detect_aliases: &["mistral-vibe"],
        requires: &[],
        unsupported: &[],
        launch: "vibe",
        expected_process: "vibe",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "qwen-code",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Qwen Code",
        favicon_domain: "qwenlm.github.io",
        homepage_url: "https://github.com/QwenLM/qwen-code",
        detect: "qwen",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "qwen",
        expected_process: "qwen",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "rovo",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Rovo Dev",
        favicon_domain: "atlassian.com",
        homepage_url: "https://support.atlassian.com/rovo/docs/install-and-run-rovo-dev-cli-on-your-device/",
        // Rovo Dev is an extension of Atlassian's CLI, not a binary of its
        // own: "Rovo Dev CLI is an extension for Atlassian Command Line
        // Interface (ACLI)." and the documented way to run it is
        // `acli rovodev run` (support.atlassian.com/rovo/docs/…). A bare
        // `rovo` appears nowhere in Atlassian's documentation — it was a
        // name this catalog invented, and a launch nobody could start
        // (t-6120, t-6009's survey). The vault's reopen already spelled it
        // `acli rovodev run --restore`.
        detect: "acli",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "acli rovodev run",
        expected_process: "acli",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "hermes",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Hermes",
        favicon_domain: "nousresearch.com",
        homepage_url: "https://hermes-agent.nousresearch.com/docs/",
        detect: "hermes",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "hermes --tui",
        expected_process: "hermes",
        injection: Injection::HermesQuery,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "openclaw",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "OpenClaw",
        favicon_domain: "openclaw.ai",
        homepage_url: "https://github.com/openclaw/openclaw",
        detect: "openclaw",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "openclaw",
        expected_process: "openclaw",
        injection: Injection::StdinAfterStart,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness::PLAIN,
    },
    AgentSpec {
        id: "copilot",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "GitHub Copilot",
        favicon_domain: "github.com",
        homepage_url: "https://docs.github.com/en/copilot/how-tos/set-up/install-copilot-cli",
        detect: "copilot",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "copilot",
        expected_process: "copilot",
        injection: Injection::FlagInteractive,
        draft_flag: None,
        draft_env: None,
        argv_separator: None,
        ready: ReadyMark::Quiet,
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            blocked: &[BlockedSignal::TrustMenu(TrustMenu::Copilot)],
            // The Copilot CLI takes the GitHub CLI's login when there is
            // one, so gh's status is a witness FOR it — never against it:
            // the CLI also holds a login of its own, which is why a gh "no"
            // reads as unknown rather than as signed out.
            auth_probes: &[AuthProbeKind::GhAuthStatus],
            ..Harness::PLAIN
        },
    },
    AgentSpec {
        id: "grok",
        composer_clear: ComposerClear::None,
        resume_nudge: NudgeRoad::Composer,
        name: "Grok",
        favicon_domain: "x.ai",
        homepage_url: "https://x.ai/cli",
        detect: "grok",
        detect_aliases: &[],
        requires: &[],
        unsupported: &[],
        launch: "grok",
        expected_process: "grok",
        injection: Injection::Argv,
        draft_flag: None,
        draft_env: None,
        argv_separator: Some("--"),
        // Quiet was the documented failure here, not a measurement: grok
        // shimmers its startup logo until the session opens, so the stream
        // never settles and every launch prompt waited out the whole hard
        // timeout (Orca's why on `grok-composer-prompt`,
        // tui-agent-config.ts:325-327). Its `❯` lands ~0.6s in — inside the
        // alternate screen, where a shell prompt cannot forge it.
        ready: ReadyMark::AltScreenPrompt('\u{276f}'),
        ready_quiet_ms: None,
        ready_timeout_ms: None,
        harness: Harness {
            submit_ack: SubmitAck::Hook,
            vault_resume_carries_launch_args: true,
            ..Harness::PLAIN
        },
    },
];

/// One agent, crossed with what this machine actually has.
///
/// The registry says how to drive an agent; this says whether it is here. Both
/// halves are needed before a picker can be honest: a list of thirty-five
/// names is not a list of choices, and offering one that is not installed
/// makes the first thing a person tries fail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentPresence {
    pub id: &'static str,
    pub name: &'static str,
    /// Where the window fetches this agent's icon from at runtime.
    pub favicon_domain: &'static str,
    /// The vendor's page — install instructions while the agent is absent,
    /// docs once it is here. Empty only for `zo`, which has no vendor.
    pub homepage_url: &'static str,
    /// Present and runnable here.
    pub installed: bool,
    /// The name that was found, when one was — which can be an alias rather
    /// than the primary command.
    pub found_as: Option<String>,
    /// Why it is not offered, when the reason is the platform rather than the
    /// absence of a binary. Kept apart from `installed` because "will never
    /// work here" and "not installed yet" are different sentences to show.
    pub unsupported_here: bool,
    /// A required companion command that is missing, when that is what is
    /// wrong. A wrapper without the thing it wraps is not installed, however
    /// present its own binary is.
    pub missing_requirement: Option<&'static str>,
    pub takes_a_paste: bool,
    pub ready: ReadyMark,
    /// The agent's own console, as its screen shows it ([`agent_voice`]): the
    /// mark it wears (`✻` for Claude Code, `◎` for Codex), the word it shows
    /// while it works, the provider whose models it drives, and the roads a
    /// running session takes to change model and permission mode. The
    /// window's conversation view speaks in these, never in words of its own;
    /// empty where the catalog knows no console for the agent.
    pub glyph: &'static str,
    /// The marks the CLI's own spinner cycles through while it works, in
    /// order (Claude Code: `·✢*✶✻✽` and back, one every 120 ms); empty for a
    /// console whose mark stands still.
    pub glyph_cycle: &'static [&'static str],
    pub busy_word: &'static str,
    pub models_provider: Option<&'static str>,
    pub model_command: Option<&'static str>,
    pub model_command_takes_id: bool,
    pub permission_road: Option<&'static str>,
    /// What each of the CLI's permission modes lets it do on its own
    /// ([`PermissionMode`]): the composer's send button and the spinner wear
    /// the reach, the way Claude Code's panel colours them by mode.
    pub permission_modes: &'static [PermissionMode],
    /// The protocol this CLI speaks without a screen — `app-server` (Codex,
    /// JSON-RPC over stdio), `acp` (Gemini CLI `--acp`) or `claude-stream`
    /// (Claude Code stream-json) — when the window can drive it as a wire
    /// session; `None` for a CLI that is only a TUI.
    pub wire: Option<&'static str>,
    /// Whether a pane's screen session can be handed over to that wire as
    /// the same conversation: the wire resumes a session by flag and the
    /// CLI has an exit command the window can type.
    pub wire_resumes: bool,
    /// The CLI's own word for winning context back ([`AgentVoice`]), which
    /// the composer's context meter sends when it is pressed. `None` leaves
    /// that chip a reading with no door — the window never invents a
    /// command a CLI did not name.
    pub compact_command: Option<&'static str>,
    /// Where the CLI's read tool counts its `offset` from
    /// ([`AgentVoice::read_offset_base`]) — what the conversation's tool row
    /// opens a read's file at.
    pub read_offset_base: Option<u8>,
    /// The key that interrupts the CLI's turn ([`AgentVoice::interrupt_key`]).
    pub interrupt_key: Option<&'static str>,
    /// The verbs the CLI's spinner turns through ([`AgentVoice::spinner_verbs`]).
    pub spinner_verbs: &'static [&'static str],
    /// The CLI's todo tool ([`AgentVoice::todo_tool`]).
    pub todo_tool: Option<&'static str>,
}

/// How far a permission mode lets the agent act before it asks — the one
/// fact the window draws about a mode (Claude Code's panel colours its send
/// button by it: edits taken without asking, a planning session that writes
/// nothing, every permission bypassed). A mode the table does not name asks,
/// and wears the plain send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionReach {
    /// The agent asks before it acts.
    Ask,
    /// Edits land without a question; commands still ask.
    Edits,
    /// The agent reads and plans, and writes nothing.
    Plan,
    /// Nothing asks.
    Bypass,
}

/// One permission mode of one CLI, spelled as that CLI reports it, and its
/// reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PermissionMode {
    pub mode: &'static str,
    pub reach: PermissionReach,
    /// The CLI's own word for the mode, as its screen and its panel say it
    /// (Claude Code 2.1.280: "Manual", "Edit automatically", …). Empty where
    /// it was not measured; the page then opens the mode's own spelling.
    pub label: &'static str,
    /// Whether Shift+Tab steps through the mode. `false` for a mode the CLI
    /// keeps in its cycle only while it is already in it (Claude Code's
    /// `dontAsk`: the first press leaves it and it drops out).
    pub cycles: bool,
    /// Other spellings the CLI gives the same mode — Claude Code 2.1.280's
    /// `--help` lists `manual` where its sessions report `default`.
    pub aliases: &'static [&'static str],
}

const fn mode(mode: &'static str, reach: PermissionReach) -> PermissionMode {
    PermissionMode {
        mode,
        reach,
        label: "",
        cycles: true,
        aliases: &[],
    }
}

/// A mode with the CLI's own word for it, in its cycle.
const fn worded(mode: &'static str, reach: PermissionReach, label: &'static str) -> PermissionMode {
    PermissionMode {
        mode,
        reach,
        label,
        cycles: true,
        aliases: &[],
    }
}

/// How a CLI is driven without its screen: the arguments that start it on a
/// JSON-RPC wire over stdio, and which protocol then flows on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WireRoad {
    /// `app-server` (Codex's own protocol), `acp` (Agent Client Protocol) or
    /// `claude-stream` (Claude Code's stream-json lines).
    pub protocol: &'static str,
    pub args: &'static [&'static str],
    /// The flag that continues an existing conversation on the wire, taking
    /// the session id the CLI's own hooks reported (`claude --resume <id>`
    /// works in print mode too: measured 2026-09-16, same session id back,
    /// the earlier turns remembered). `None` for a wire that only starts
    /// fresh threads from its arguments.
    pub resume: Option<&'static str>,
}

/// How an agent's own screen marks itself, says it is working, and takes the
/// two commands a composer chip sends it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentVoice {
    pub glyph: &'static str,
    /// The spinner's marks in order, played forward and back while the CLI
    /// works — Claude Code's own `·✢*✶✻✽` (its panel and its TUI both cycle
    /// them at 120 ms). Empty where the mark stands still.
    pub glyph_cycle: &'static [&'static str],
    pub busy_word: &'static str,
    /// The provider whose models this CLI drives, as zo's live catalog names
    /// its rows (`zo models --json`, `provider`): the composer's model menu
    /// lists those rows. `None` lists every row — zo drives them all.
    pub models_provider: Option<&'static str>,
    /// How the permission mode changes mid-session: `shift-tab` cycles it on
    /// the CLI's own keyboard; a slash command opens the CLI's picker.
    pub permission_road: Option<&'static str>,
    /// The CLI's permission modes as it reports them (hook payload, channel
    /// status, wire mode list), each with its reach. Only modes whose reach
    /// is documented are named; the rest ask.
    pub permission_modes: &'static [PermissionMode],
    /// The screenless mode the window can drive as a wire session (unit 6:
    /// docs/design/agent-wire-sessions-20260915.md), when the CLI has one.
    pub wire: Option<WireRoad>,
    /// The slash command that ends the CLI's own screen session cleanly —
    /// what the window types to hand a pane's conversation over to its wire
    /// (`/exit`, on Claude Code's own command list). `None` where the exit
    /// word was not measured; such a pane keeps its screen.
    pub exit_command: Option<&'static str>,
    /// The tool this CLI asks permission with when it is asking for a PLAN to
    /// be approved — Claude Code's `ExitPlanMode`, whose input carries the
    /// plan itself. The extension draws that one permission as its own card
    /// ("Claude's Plan", 2.1.268-275: the plan previewed, and a reason field
    /// on the refusal), and this is the only fact that tells the two apart.
    /// `None` for a CLI whose plan tool was not measured; its permission
    /// requests stay ordinary approvals rather than guessed plans.
    pub plan_tool: Option<&'static str>,
    /// The slash command that summarises a session to win its context back.
    /// The composer's context meter IS that button, so the word has to be
    /// this CLI's own; `None` leaves the meter a reading and no door.
    pub compact_command: Option<&'static str>,
    /// The line its read tool's `offset` counts from — `1` when the offset
    /// IS the first line returned, `0` when it is 0-based. The page opens a
    /// read's file where the read began (the extension's tool header does)
    /// and says the lines it read, so the count must be the CLI's own. `None`
    /// where it was not measured: the file then opens at its top.
    pub read_offset_base: Option<u8>,
    /// The key that interrupts the CLI's turn on its own screen — what a
    /// pane's Esc and its stop button press. `None` where the CLI's screen
    /// was not read for it; the page then sends the terminal's own interrupt.
    pub interrupt_key: Option<&'static str>,
    /// The verbs the CLI's spinner says while it works, one picked at random
    /// and picked again as the turn goes on ([`CLAUDE_SPINNER_VERBS`]). Empty
    /// for a console that says one word (`busy_word`) the whole turn.
    pub spinner_verbs: &'static [&'static str],
    /// The tool the CLI keeps its todo list with — a call whose input is
    /// `{todos: [{content, status}]}` — which the conversation draws as that
    /// list, as the extension draws `TodoWrite` (2.1.280 `qD1`). `None`
    /// where the CLI has none or its shape was not read.
    pub todo_tool: Option<&'static str>,
}

/// Claude Code's spinner verbs — the words its own screen and its panel say
/// while it works (the 2.1.280 binary carries them; the extension's webview
/// holds the same 84, `tD1`, and picks one at random, again at 2 s, 5 s, 10 s
/// and every 5 s after). Words read off the package, not code: the gate
/// `the_conversation_wears_the_extensions_own_measures` holds this list to
/// the panel snapshot's (`claude-code-panel.json` `words.spinnerVerbs`).
pub const CLAUDE_SPINNER_VERBS: &[&str] = &[
    "Accomplishing",
    "Actioning",
    "Actualizing",
    "Baking",
    "Booping",
    "Brewing",
    "Calculating",
    "Cerebrating",
    "Channeling",
    "Churning",
    "Clauding",
    "Coalescing",
    "Cogitating",
    "Computing",
    "Combobulating",
    "Concocting",
    "Considering",
    "Contemplating",
    "Cooking",
    "Crafting",
    "Creating",
    "Crunching",
    "Deciphering",
    "Deliberating",
    "Determining",
    "Discombobulating",
    "Doing",
    "Effecting",
    "Elucidating",
    "Enchanting",
    "Envisioning",
    "Finagling",
    "Flibbertigibbeting",
    "Forging",
    "Forming",
    "Frolicking",
    "Generating",
    "Germinating",
    "Hatching",
    "Herding",
    "Honking",
    "Ideating",
    "Imagining",
    "Incubating",
    "Inferring",
    "Manifesting",
    "Marinating",
    "Meandering",
    "Moseying",
    "Mulling",
    "Mustering",
    "Musing",
    "Noodling",
    "Percolating",
    "Perusing",
    "Philosophizing",
    "Pontificating",
    "Pondering",
    "Processing",
    "Puttering",
    "Puzzling",
    "Reticulating",
    "Ruminating",
    "Scheming",
    "Schlepping",
    "Shimmying",
    "Simmering",
    "Smooshing",
    "Spelunking",
    "Spinning",
    "Stewing",
    "Sussing",
    "Synthesizing",
    "Thinking",
    "Tinkering",
    "Transmuting",
    "Unfurling",
    "Unraveling",
    "Vibing",
    "Wandering",
    "Whirring",
    "Wibbling",
    "Working",
    "Wrangling",
];

/// The consoles this catalog knows — read off each CLI's own screen and its
/// documentation, so the conversation view's status line says what the
/// terminal would have said (Claude Code `✻ Pondering…`, Codex `Thinking…`),
/// the model menu lists the provider that CLI really drives, and a pick
/// travels the road that CLI really has (Claude Code and zo take `/model
/// <id>`; Codex's `/model` only opens its picker, so the window shows the
/// screen). One table; an agent it does not name has no console, and the
/// view then shows no status word and no roads rather than guessed ones.
/// The model road itself — `/model <id>` on the line, or a picker the chip
/// only shows — is the row's `moves.model` in the capability table, not a
/// second spelling here.
const AGENT_VOICES: [(&str, AgentVoice); 4] = [
    (
        "claude",
        AgentVoice {
            glyph: "✻",
            glyph_cycle: &["·", "✢", "*", "✶", "✻", "✽"],
            busy_word: "Pondering…",
            models_provider: Some("claude"),
            // `shift+tab to cycle`, on Claude Code's own status line.
            permission_road: Some("shift-tab"),
            // In the order Shift+Tab steps through them and in Claude Code's
            // own words — the extension's cycle (2.1.280 `d6`: dontAsk only
            // while it is the mode, then default, acceptEdits, plan, auto,
            // bypassPermissions) and its labels (`AB0`). The reach is what the
            // extension's stylesheet colours (`sendButton[data-permission-
            // mode=…]`): acceptEdits inverts the button, plan wears the plan
            // colour, bypassPermissions and auto the error colour; the two
            // that ask wear nothing.
            permission_modes: &[
                PermissionMode {
                    cycles: false,
                    ..worded("dontAsk", PermissionReach::Ask, "Don't ask")
                },
                PermissionMode {
                    aliases: &["manual"],
                    ..worded("default", PermissionReach::Ask, "Manual")
                },
                worded("acceptEdits", PermissionReach::Edits, "Edit automatically"),
                worded("plan", PermissionReach::Plan, "Plan"),
                worded("auto", PermissionReach::Bypass, "Auto"),
                worded(
                    "bypassPermissions",
                    PermissionReach::Bypass,
                    "Bypass permissions",
                ),
            ],
            // `claude -p` fed and read as stream-json: the lines its own
            // panel streams from (`--include-partial-messages`), permission
            // questions as `control_request`s on stdio (2.1.272 read
            // 2026-09-16).
            wire: Some(WireRoad {
                protocol: "claude-stream",
                args: &[
                    "-p",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                    "--permission-prompt-tool",
                    "stdio",
                ],
                resume: Some("--resume"),
            }),
            exit_command: Some("/exit"),
            // The tool Claude Code asks plan approval with; its input's
            // `plan` is the plan the card previews.
            plan_tool: Some("ExitPlanMode"),
            // On Claude Code's own command list — the `system.init` frame of
            // the stream-json wire named `compact` among 138 slash commands,
            // and the command answered down that wire (measured 2026-09-21).
            compact_command: Some("/compact"),
            // `Read {offset: 10930}` came back opening `10930→` (2.1.280,
            // measured 2026-09-23): the offset is the first line itself.
            read_offset_base: Some(1),
            // "esc to interrupt" on its own status line (the 2.1.280 binary
            // says it twice; the extension's Esc is the same interrupt).
            interrupt_key: Some("Escape"),
            spinner_verbs: CLAUDE_SPINNER_VERBS,
            // The extension's own name for it (`XN="TodoWrite"`).
            todo_tool: Some("TodoWrite"),
        },
    ),
    (
        "codex",
        AgentVoice {
            glyph: "◎",
            glyph_cycle: &[],
            busy_word: "Thinking…",
            models_provider: Some("openai"),
            permission_road: Some("/permissions"),
            // app-server `approvalPolicy` (docs/design/agent-wire-sessions
            // §2): `untrusted` and `on-request` ask; `on-failure` runs and
            // asks only when a command fails; `never` asks nothing.
            permission_modes: &[
                mode("on-failure", PermissionReach::Edits),
                mode("never", PermissionReach::Bypass),
            ],
            // `codex app-server`: JSON-RPC over stdio, the protocol its own
            // `generate-json-schema` describes (0.154 read 2026-09-15). A
            // thread is resumed by request (`thread/resume`), not by flag —
            // not wired yet.
            wire: Some(WireRoad {
                protocol: "app-server",
                args: &["app-server"],
                resume: None,
            }),
            exit_command: None,
            plan_tool: None,
            // In the 0.155.1 binary's own slash blob — "summarize
            // conversation to prevent hitting the context limit"
            // (docs/design/zo-vs-codex-gaps-20260921.md #23, `S:53003`).
            compact_command: Some("/compact"),
            // Codex reads files through its shell; it has no read tool.
            read_offset_base: None,
            // The 0.156.0 binary names no interrupt key in its words.
            interrupt_key: None,
            spinner_verbs: &[],
            todo_tool: None,
        },
    ),
    (
        "zo",
        AgentVoice {
            glyph: "◐",
            glyph_cycle: &[],
            busy_word: "Working…",
            models_provider: None,
            permission_road: Some("/permissions"),
            // zo's own three (zo-ide/README.md: `read-only |
            // workspace-write | danger-full-access`; it accepts Claude Code's
            // spellings too, and reports its own).
            permission_modes: &[
                mode("read-only", PermissionReach::Plan),
                mode("workspace-write", PermissionReach::Edits),
                mode("danger-full-access", PermissionReach::Bypass),
            ],
            wire: None,
            exit_command: None,
            plan_tool: None,
            // zo's own twelve (`zo-ide/README.md:36`).
            compact_command: Some("/compact"),
            // `read_file`'s schema: "a 0-based line window".
            read_offset_base: Some(0),
            // Its status line: "Working (0s • esc to interrupt)" (tui/view.rs).
            interrupt_key: Some("Escape"),
            spinner_verbs: &[],
            // zo-ide's `TodoWrite` (tools/task_tools.rs) takes the same
            // `todos: [{content, status, activeForm}]`.
            todo_tool: Some("TodoWrite"),
        },
    ),
    (
        "antigravity",
        AgentVoice {
            glyph: "✦",
            glyph_cycle: &[],
            busy_word: "Thinking…",
            models_provider: Some("google"),
            permission_road: None,
            // agy's modes are not measured here; every mode asks until they are.
            permission_modes: &[],
            // The catalog's `antigravity` is Antigravity CLI `agy` (Go), not
            // Gemini CLI: it has no `--acp`, and its own stream-json print
            // mode denies every tool on this machine through its pre-tool
            // hooks (probed 2026-09-16). No wire until that adapter exists —
            // a door that always fails is a dead control. The ACP adapter
            // stays for the day the catalog names an ACP speaker.
            wire: None,
            exit_command: None,
            plan_tool: None,
            // agy's compaction word is not measured here.
            compact_command: None,
            // agy's read tool is not measured here.
            read_offset_base: None,
            // Nor its interrupt key.
            interrupt_key: None,
            spinner_verbs: &[],
            todo_tool: None,
        },
    ),
];

/// The silent console — no mark, no word, no roads.
const SILENT_CONSOLE: AgentVoice = AgentVoice {
    glyph: "",
    glyph_cycle: &[],
    busy_word: "",
    models_provider: None,
    permission_road: None,
    permission_modes: &[],
    wire: None,
    exit_command: None,
    plan_tool: None,
    compact_command: None,
    read_offset_base: None,
    interrupt_key: None,
    spinner_verbs: &[],
    todo_tool: None,
};

/// One agent's console, or the silent one for an agent the table does not
/// name.
#[must_use]
pub fn agent_voice(id: &str) -> AgentVoice {
    AGENT_VOICES
        .iter()
        .find(|(said, _)| *said == id)
        .map_or(SILENT_CONSOLE, |(_, voice)| *voice)
}

/// Resolve one runnable command on `path_var` to the first PATH entry that owns it.
///
/// The same rules as the harness's own discovery (`ZoBinary::discover_in`):
/// a file, and on unix one with an execute bit. Windows has no execute bit, so
/// being a file under the right name on `PATH` is as much as can be checked
/// without running it.
#[must_use]
pub fn resolve_on_path(
    path_var: Option<&std::ffi::OsStr>,
    command: &str,
) -> Option<std::path::PathBuf> {
    let path_var = path_var?;
    std::env::split_paths(path_var).find_map(|dir| {
        let candidate = dir.join(command);
        let metadata = std::fs::metadata(&candidate).ok()?;
        if !metadata.is_file() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            (metadata.permissions().mode() & 0o111 != 0).then_some(candidate)
        }
        #[cfg(not(unix))]
        {
            Some(candidate)
        }
    })
}

fn on_path(path_var: Option<&std::ffi::OsStr>, command: &str) -> bool {
    resolve_on_path(path_var, command).is_some()
}

/// Every agent, with whether this machine can run it.
///
/// Ordered as the registry is, not by what is installed: a picker that
/// reorders itself as tools come and go is a picker whose rows move under the
/// cursor. Callers that want only the usable ones filter.
pub fn agent_presence(path_var: Option<&std::ffi::OsStr>, os: &str) -> Vec<AgentPresence> {
    AGENT_SPECS
        .iter()
        .map(|spec| {
            let unsupported_here = !spec.runs_on(os);
            let found_as = spec
                .detect_names()
                .find(|name| on_path(path_var, name))
                .map(str::to_string);
            let missing_requirement = spec
                .requires
                .iter()
                .copied()
                .find(|needed| !on_path(path_var, needed));
            AgentPresence {
                id: spec.id,
                name: spec.name,
                favicon_domain: spec.favicon_domain,
                homepage_url: spec.homepage_url,
                installed: found_as.is_some() && !unsupported_here && missing_requirement.is_none(),
                found_as,
                unsupported_here,
                missing_requirement,
                takes_a_paste: spec.takes_a_paste(),
                ready: spec.ready,
                glyph: agent_voice(spec.id).glyph,
                glyph_cycle: agent_voice(spec.id).glyph_cycle,
                busy_word: agent_voice(spec.id).busy_word,
                models_provider: agent_voice(spec.id).models_provider,
                model_command: spec.harness.moves.model.and_then(MoveRoad::command),
                model_command_takes_id: spec
                    .harness
                    .moves
                    .model
                    .is_some_and(|road| matches!(road, MoveRoad::Line(_))),
                permission_road: agent_voice(spec.id).permission_road,
                permission_modes: agent_voice(spec.id).permission_modes,
                wire: agent_voice(spec.id).wire.map(|road| road.protocol),
                wire_resumes: agent_voice(spec.id)
                    .wire
                    .is_some_and(|road| road.resume.is_some())
                    && agent_voice(spec.id).exit_command.is_some(),
                compact_command: agent_voice(spec.id).compact_command,
                read_offset_base: agent_voice(spec.id).read_offset_base,
                interrupt_key: agent_voice(spec.id).interrupt_key,
                spinner_verbs: agent_voice(spec.id).spinner_verbs,
                todo_tool: agent_voice(spec.id).todo_tool,
            }
        })
        .collect()
}

/// The wire an agent can be driven on, or `None` for a CLI that is only a
/// screen.
#[must_use]
pub fn wire_road(id: &str) -> Option<WireRoad> {
    agent_voice(id).wire
}

/// How far `mode` lets `agent` act on its own — [`PermissionReach::Ask`] for
/// a mode the console does not name, or no mode at all. A mode is known by
/// its reported spelling or any other the CLI gives it.
#[must_use]
pub fn permission_reach(agent: &str, mode: &str) -> PermissionReach {
    agent_voice(agent)
        .permission_modes
        .iter()
        .find(|row| row.mode == mode || row.aliases.contains(&mode))
        .map_or(PermissionReach::Ask, |row| row.reach)
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.slug())
    }
}

#[cfg(test)]
mod tests {
    /// The console table names catalog agents only, and every road it gives
    /// is one the composer can walk: a model command is a slash command, a
    /// permission road is `shift-tab` or a slash command.
    #[test]
    fn every_console_names_a_catalog_agent_and_walkable_roads() {
        for (id, console) in super::AGENT_VOICES {
            assert!(
                super::AGENT_SPECS.iter().any(|spec| spec.id == id),
                "{id} has a console but no catalog row"
            );
            assert!(
                !console.glyph.is_empty() && !console.busy_word.is_empty(),
                "{id}"
            );
            // A spinner that cycles comes back to the mark the head wears.
            assert!(
                console.glyph_cycle.is_empty() || console.glyph_cycle.contains(&console.glyph),
                "{id}: the cycle {:?} never shows {}",
                console.glyph_cycle,
                console.glyph
            );
            if let Some(road) = console.permission_road {
                assert!(road == "shift-tab" || road.starts_with('/'), "{id}: {road}");
            }
            if let Some(road) = console.wire {
                assert!(
                    matches!(road.protocol, "app-server" | "acp" | "claude-stream")
                        && !road.args.is_empty(),
                    "{id}: {road:?}"
                );
                if let Some(flag) = road.resume {
                    assert!(flag.starts_with("--"), "{id}: {flag}");
                }
            }
            if let Some(exit) = console.exit_command {
                assert!(exit.starts_with('/'), "{id}: {exit}");
            }
            // A plan tool is a TOOL's name, not a slash command: it is
            // compared against the permission request's `tool_name`.
            if let Some(plan) = console.plan_tool {
                assert!(!plan.is_empty() && !plan.starts_with('/'), "{id}: {plan}");
            }
            if let Some(compact) = console.compact_command {
                assert!(compact.starts_with('/'), "{id}: {compact}");
            }
        }
        assert_eq!(
            super::wire_road("codex").map(|road| road.protocol),
            Some("app-server")
        );
        assert!(
            super::wire_road("antigravity").is_none(),
            "agy has no wire until its stream-json adapter lands"
        );
        let claude_wire = super::wire_road("claude").expect("Claude Code has a wire");
        assert_eq!(claude_wire.protocol, "claude-stream");
        assert!(
            claude_wire.args.contains(&"--include-partial-messages")
                && claude_wire.args.contains(&"stdio"),
            "the Claude wire streams partial messages and answers permission on stdio: {claude_wire:?}"
        );
        assert_eq!(claude_wire.resume, Some("--resume"));
        assert_eq!(super::agent_voice("claude").exit_command, Some("/exit"));
        assert!(super::agent_voice("codex").exit_command.is_none());
        // The context meter's door: three CLIs name the word, one does not,
        // and the silent console never does.
        for named in ["claude", "codex", "zo"] {
            assert_eq!(
                super::agent_voice(named).compact_command,
                Some("/compact"),
                "{named}"
            );
        }
        assert!(super::agent_voice("antigravity").compact_command.is_none());
        assert!(super::SILENT_CONSOLE.compact_command.is_none());
        // Where a read's offset counts from is each CLI's own (measured for
        // two), and the presence row the page reads carries it.
        assert_eq!(super::agent_voice("claude").read_offset_base, Some(1));
        assert_eq!(super::agent_voice("zo").read_offset_base, Some(0));
        assert!(super::agent_voice("codex").read_offset_base.is_none());
        assert!(super::SILENT_CONSOLE.read_offset_base.is_none());
        let rows = super::agent_presence(None, "macos");
        assert!(
            rows.iter()
                .all(|row| row.read_offset_base == super::agent_voice(row.id).read_offset_base)
        );
        assert!(super::wire_road("zo").is_none());
        let silent = super::agent_voice("nobody");
        assert_eq!(silent, super::SILENT_CONSOLE);
        let claude = super::agent_voice("claude");
        assert_eq!(claude.models_provider, Some("claude"));
    }

    /// The composer chip's model road is the capability row's, so a chip
    /// that types `/model <id>` at Claude Code and only shows Codex its
    /// picker reads both facts off the one table.
    #[test]
    fn the_presence_rows_model_road_is_the_capability_rows() {
        let rows = super::agent_presence(None, "macos");
        let of = |id: &str| rows.iter().find(|row| row.id == id).expect(id);
        assert_eq!(of("claude").model_command, Some("/model"));
        assert!(of("claude").model_command_takes_id);
        assert_eq!(of("codex").model_command, Some("/model"));
        assert!(!of("codex").model_command_takes_id);
        assert_eq!(of("zo").model_command, Some("/model"));
        assert!(of("zo").model_command_takes_id);
        assert_eq!(of("aider").model_command, None);
        assert!(!of("aider").model_command_takes_id);
        for row in &rows {
            if let Some(command) = row.model_command {
                assert!(command.starts_with('/'), "{}: {command}", row.id);
            }
        }
    }

    /// A mode's reach is the console's word, spelled as that CLI reports it;
    /// a mode nobody measured asks, and so does a silent console.
    #[test]
    fn a_permission_modes_reach_is_read_off_the_console_and_unknown_modes_ask() {
        use super::{PermissionReach, permission_reach};
        assert_eq!(
            permission_reach("claude", "acceptEdits"),
            PermissionReach::Edits
        );
        assert_eq!(permission_reach("claude", "plan"), PermissionReach::Plan);
        assert_eq!(
            permission_reach("claude", "bypassPermissions"),
            PermissionReach::Bypass
        );
        assert_eq!(permission_reach("claude", "auto"), PermissionReach::Bypass);
        assert_eq!(permission_reach("claude", "default"), PermissionReach::Ask);
        assert_eq!(
            permission_reach("zo", "workspace-write"),
            PermissionReach::Edits
        );
        assert_eq!(permission_reach("zo", "read-only"), PermissionReach::Plan);
        assert_eq!(
            permission_reach("zo", "danger-full-access"),
            PermissionReach::Bypass
        );
        assert_eq!(permission_reach("codex", "never"), PermissionReach::Bypass);
        assert_eq!(
            permission_reach("codex", "on-request"),
            PermissionReach::Ask
        );
        assert_eq!(permission_reach("nobody", "anything"), PermissionReach::Ask);
        assert_eq!(permission_reach("claude", "manual"), PermissionReach::Ask);
        let claude = super::agent_voice("claude").permission_modes;
        let edits = claude
            .iter()
            .find(|row| row.mode == "acceptEdits")
            .expect("acceptEdits is named");
        assert_eq!(
            serde_json::to_value(edits).unwrap(),
            serde_json::json!({"mode": "acceptEdits", "reach": "edits",
                "label": "Edit automatically", "cycles": true, "aliases": []})
        );
    }

    /// Claude Code's modes stand in the order its panel's Shift+Tab steps
    /// through them (2.1.280 `d6`), each in its own word (`AB0`): `dontAsk`
    /// only while it is the mode, `default` also spelled `manual` by `--help`.
    #[test]
    fn claude_codes_modes_are_its_cycle_in_its_own_words() {
        let claude = super::agent_voice("claude").permission_modes;
        let order: Vec<(&str, &str, bool)> = claude
            .iter()
            .map(|row| (row.mode, row.label, row.cycles))
            .collect();
        assert_eq!(
            order,
            vec![
                ("dontAsk", "Don't ask", false),
                ("default", "Manual", true),
                ("acceptEdits", "Edit automatically", true),
                ("plan", "Plan", true),
                ("auto", "Auto", true),
                ("bypassPermissions", "Bypass permissions", true),
            ]
        );
        assert_eq!(claude[1].aliases, ["manual"]);
        // The other consoles name their modes without words of their own.
        for id in ["codex", "zo"] {
            assert!(
                super::agent_voice(id)
                    .permission_modes
                    .iter()
                    .all(|row| row.label.is_empty() && row.cycles && row.aliases.is_empty()),
                "{id}"
            );
        }
        // The interrupt key is named where the CLI's own screen says it.
        assert_eq!(super::agent_voice("claude").interrupt_key, Some("Escape"));
        assert_eq!(super::agent_voice("zo").interrupt_key, Some("Escape"));
        assert!(super::agent_voice("codex").interrupt_key.is_none());
        assert!(super::SILENT_CONSOLE.interrupt_key.is_none());
        // The spinner's verbs are Claude Code's own list, and only its.
        let verbs = super::agent_voice("claude").spinner_verbs;
        assert_eq!(verbs.len(), 84);
        assert_eq!((verbs[0], verbs[83]), ("Accomplishing", "Wrangling"));
        let unique: std::collections::BTreeSet<_> = verbs.iter().collect();
        assert_eq!(unique.len(), verbs.len(), "a verb stands twice");
        for id in ["codex", "zo", "antigravity"] {
            assert!(super::agent_voice(id).spinner_verbs.is_empty(), "{id}");
        }
        // The todo tool is named where its shape was read: Claude Code's and
        // zo's `TodoWrite`; Codex's plan tool keeps another shape.
        assert_eq!(super::agent_voice("claude").todo_tool, Some("TodoWrite"));
        assert_eq!(super::agent_voice("zo").todo_tool, Some("TodoWrite"));
        assert!(super::agent_voice("codex").todo_tool.is_none());
        assert!(super::SILENT_CONSOLE.todo_tool.is_none());
    }

    use super::*;

    #[test]
    fn slugs_are_unique_and_round_trip() {
        let mut seen = std::collections::BTreeSet::new();
        for agent in ALL_AGENTS {
            assert!(seen.insert(agent.slug()), "duplicate slug {}", agent.slug());
            assert_eq!(AgentKind::from_slug(agent.slug()), Some(agent));
        }
        assert_eq!(seen.len(), ALL_AGENTS.len());
    }

    #[test]
    fn serde_uses_the_same_spelling_as_the_slug() {
        for agent in ALL_AGENTS {
            let json = serde_json::to_string(&agent).expect("serialize");
            assert_eq!(json, format!("\"{}\"", agent.slug()));
        }
    }

    #[test]
    fn additional_context_is_a_provider_capability_not_a_model_or_target_rule() {
        for provider in [AgentKind::Claude, AgentKind::Codex] {
            let hook = provider
                .hook_additional_context()
                .unwrap_or_else(|| panic!("{} lost context hooks", provider.slug()));
            assert_eq!(hook.session_start_event, "SessionStart");
        }
        assert!(AgentKind::Amp.hook_additional_context().is_none());
    }

    /* ---- the registry ---- */

    #[test]
    fn the_registry_is_the_measurement_and_nothing_is_duplicated() {
        // The arithmetic, spelled out because a bare number cannot be
        // re-derived a year later: Orca's `TuiAgent` union held 36. One of
        // them — `claude-agent-teams` — is deliberately absent here, because
        // for us that is Claude launched in teams mode rather than a second
        // binary to detect and start (see `agent_teams` on the shell side).
        // And one entry is not Orca's at all: `zo`, this project's own
        // harness. One retired CLI has since been removed: 36 − 1 + 1 − 1.
        assert_eq!(
            AGENT_SPECS.len(),
            35,
            "the agent registry changed size — a different number means a drift \
             from the measured 35, and the note above says how it is derived"
        );
        assert!(
            agent_spec("claude-agent-teams").is_none(),
            "teams became a second agent to start rather than a way of \
             starting Claude"
        );
        let mut ids = std::collections::BTreeSet::new();
        let mut names = std::collections::BTreeSet::new();
        for spec in &AGENT_SPECS {
            assert!(ids.insert(spec.id), "duplicate agent id {}", spec.id);
            assert!(
                names.insert(spec.name),
                "duplicate display name {}",
                spec.name
            );
            assert!(!spec.id.is_empty() && !spec.name.is_empty());
            assert!(!spec.detect.is_empty(), "{} has nothing to detect", spec.id);
            assert!(!spec.launch.is_empty(), "{} has nothing to launch", spec.id);
            assert!(
                !spec.expected_process.is_empty(),
                "{} expects no process, so a timeout can never be checked",
                spec.id
            );
            assert_eq!(agent_spec(spec.id), Some(spec));
        }
    }

    /// This window's own harness is one of the agents a terminal can start.
    ///
    /// It was not, and the shape of the miss is worth keeping written down:
    /// `zo` was in the LANE catalogue ([`ALL_AGENTS`]) and absent from the
    /// TERMINAL one, and it is the terminal one the default-agent picker and
    /// every new terminal read. So `zo` sat on `PATH` and the setting that
    /// says "start this when a terminal opens" simply had no row for it —
    /// the one agent this project ships was the one agent it could not start.
    ///
    /// Asked of the registry rather than of the source text: the spec is a
    /// value a test can call, and a call cannot be satisfied by a comment
    /// that happens to spell the same words.
    #[test]
    fn the_windows_own_harness_can_be_started_in_a_terminal() {
        let zo = agent_spec("zo").expect("the terminal catalogue knows `zo`");
        // What actually runs. Anything else here is a picker offering ZO and
        // a pty starting somebody else — or nothing.
        assert_eq!(zo.launch, "zo");
        assert_eq!(zo.detect, "zo");
        assert_eq!(zo.expected_process, "zo");
        assert_eq!(zo.name, "ZO");
        // Measured in a pty: the REPL turns bracketed paste on and then says
        // nothing more of its own. No composer glyph, no cursor toggle — so
        // silence after the handshake is the mark, and any other answer here
        // waits for a signal `zo` never sends.
        assert_eq!(zo.ready, ReadyMark::Quiet);
        // And there is no prefill flag to hand it one with (`zo prompt TEXT`
        // is the non-interactive door, not this one), so a prompt is typed
        // after the ready signal. Argv here would push the prompt on as a
        // bare word `zo` does not accept.
        assert_eq!(zo.injection, Injection::StdinAfterStart);
        assert_eq!(zo.draft_flag, None);
        assert_eq!(zo.draft_env, None);
        assert!(zo.takes_a_paste());
        assert!(zo.runs_on("macos") && zo.runs_on("linux") && zo.runs_on("windows"));
        assert!(zo.requires.is_empty());
        // The picker follows registry order, so position IS the ordering: the
        // list a person picks a default from opens with this window's own.
        assert_eq!(AGENT_SPECS[0].id, "zo");
        // The two catalogues coexist rather than merge. Both answer to `zo`
        // and neither is the other's lookup — a lane is opened through the
        // supervisor by `AgentKind`, a terminal is spawned through the spec.
        assert_eq!(AgentKind::from_slug(zo.id), Some(AgentKind::Zo));
        assert_eq!(AgentKind::Zo.slug(), zo.id);
        assert_eq!(AgentKind::Zo.label(), zo.name);
    }

    /// And having it on `PATH` is what makes it offerable, through the same
    /// answer the picker reads — no separate road for our own agent.
    #[test]
    fn the_harness_on_path_is_offered_like_any_other_agent() {
        let (_dir, path) = fake_path(&["zo"]);
        let found = agent_presence(Some(&path), "macos");
        let installed: Vec<&str> = found
            .iter()
            .filter(|one| one.installed)
            .map(|one| one.id)
            .collect();
        assert_eq!(installed, ["zo"], "{installed:?}");
        let row = found.first().expect("a first row");
        assert_eq!(row.id, "zo");
        assert_eq!(row.found_as.as_deref(), Some("zo"));
        assert!(row.takes_a_paste && !row.unsupported_here);
        assert_eq!(row.ready, ReadyMark::Quiet);
        // No vendor site, so no mark to ask for: the window's icon chain sees
        // an empty domain and keeps the letter tile instead of asking the
        // favicon service for nothing.
        assert!(row.favicon_domain.is_empty());
        // And no vendor page either, so the row offers no install link —
        // while every other row carries its spec's URL through unchanged.
        assert!(row.homepage_url.is_empty());
        let claude = found.iter().find(|one| one.id == "claude").expect("row");
        assert_eq!(
            claude.homepage_url,
            agent_spec("claude").expect("in the registry").homepage_url
        );
        assert!(!claude.homepage_url.is_empty());
    }

    /// Neither of the two omissions may creep back in. Both are about not
    /// pretending to be another product: one entry existed only to run a
    /// competitor's CLI, and two agents accept a prefill through an
    /// environment variable named after it.
    #[test]
    fn the_registry_names_no_other_product() {
        for spec in &AGENT_SPECS {
            for field in [
                spec.id,
                spec.name,
                spec.favicon_domain,
                spec.detect,
                spec.launch,
                spec.expected_process,
            ] {
                assert!(
                    !field.to_ascii_lowercase().contains("orca"),
                    "`{}` carries another product's name in `{field}`",
                    spec.id
                );
            }
            if let Some(env) = spec.draft_env {
                assert!(
                    !env.to_ascii_lowercase().contains("orca"),
                    "`{}` would set `{env}`, which tells the agent it is \
                     talking to a different application",
                    spec.id
                );
            }
        }
    }

    /// Every agent's icon is a runtime fetch of its vendor's favicon, so
    /// every spec must name a host that fetch can be made against — a bare
    /// domain, not a URL, not a path, and never an empty string that would
    /// quietly ask the favicon service for nothing.
    ///
    /// One agent has no vendor: `zo` is this project's own harness, so there
    /// is no third-party site to fetch a mark from and no domain may be
    /// invented for it. It names nothing and wears the letter tile, which is
    /// the last step of the same chain. That exemption is held to exactly one
    /// id here — an empty domain arriving on any other entry is the drift this
    /// test exists to catch.
    #[test]
    fn every_agent_names_the_site_its_icon_comes_from() {
        let ours: Vec<&str> = AGENT_SPECS
            .iter()
            .filter(|spec| spec.favicon_domain.is_empty())
            .map(|spec| spec.id)
            .collect();
        assert_eq!(ours, ["zo"], "{ours:?}");
        for spec in AGENT_SPECS.iter().filter(|spec| spec.id != "zo") {
            let domain = spec.favicon_domain;
            assert!(
                domain.contains('.')
                    && !domain.contains('/')
                    && !domain.contains(':')
                    && !domain.contains(char::is_whitespace),
                "`{}` cannot fetch a favicon from `{domain}`",
                spec.id
            );
        }
    }

    /// Every agent names the page its install instructions live on — the URL
    /// Orca's `AgentRow` ends every row with, titled `Install` while the
    /// agent is absent and `Docs` once it is here
    /// (`plugin-command-keybindings-Do9s02Z7.js:1290`, catalog values from
    /// `agent-catalog-CQp2IFaf.js:211-448`). This is what makes the
    /// "available to install" list actionable rather than a label, so a URL
    /// has to be an https address a browser can be handed — and never an
    /// empty string that would render a button leading nowhere.
    ///
    /// The one exemption is the same one the favicon test holds: `zo` is this
    /// project's own harness, has no vendor page, and an invented address
    /// would be worse than no link. Held to exactly one id.
    #[test]
    fn every_agent_names_where_its_install_instructions_live() {
        let ours: Vec<&str> = AGENT_SPECS
            .iter()
            .filter(|spec| spec.homepage_url.is_empty())
            .map(|spec| spec.id)
            .collect();
        assert_eq!(ours, ["zo"], "{ours:?}");
        for spec in AGENT_SPECS.iter().filter(|spec| spec.id != "zo") {
            let url = spec.homepage_url;
            assert!(
                url.starts_with("https://") && !url.contains(char::is_whitespace),
                "`{}` cannot hand a browser `{url}`",
                spec.id
            );
        }
    }

    /// The one question the readiness machine asks of this table.
    ///
    /// Orca's rule: an agent that can be handed its prompt at startup is never
    /// typed at. Getting this backwards either strands a prompt on an agent
    /// that was waiting for one, or types at a program that already had it.
    #[test]
    fn only_agents_with_no_prefill_are_pasted_at() {
        let pasted: Vec<&str> = AGENT_SPECS
            .iter()
            .filter(|spec| spec.takes_a_paste())
            .map(|spec| spec.id)
            .collect();
        let prefilled: Vec<&str> = AGENT_SPECS
            .iter()
            .filter(|spec| !spec.takes_a_paste())
            .map(|spec| spec.id)
            .collect();
        // Two, and they are the two that carry `--prefill`. The other two Orca
        // prefills read an environment variable named after it, so they are
        // pasted at here instead.
        assert_eq!(prefilled, ["claude", "openclaude"], "{prefilled:?}");
        assert_eq!(pasted.len(), AGENT_SPECS.len() - 2);
        for spec in &AGENT_SPECS {
            assert_eq!(
                spec.takes_a_paste(),
                spec.draft_flag.is_none() && spec.draft_env.is_none()
            );
        }
    }

    #[test]
    fn prompt_submission_acknowledgement_is_a_hook_capability() {
        let reporting = ALL_AGENTS
            .into_iter()
            .filter(|agent| agent.reports_prompt_submit())
            .map(AgentKind::slug)
            .collect::<Vec<_>>();
        assert_eq!(
            reporting,
            [
                "zo", "claude", "codex", "cursor", "droid", "copilot", "grok", "kimi", "devin",
            ]
        );
        assert!(AgentKind::Claude.reports_prompt_submit());
        assert!(AgentKind::Codex.reports_prompt_submit());
    }

    /// Whose stop pair brackets the spawn rather than the helper — zo alone,
    /// as its reporter states (a background `Agent` returns `running` and
    /// the stop fires on that return). Everybody else's stop is a finish.
    #[test]
    fn only_zos_subagent_stop_closes_the_spawn() {
        let spawn_bracketed = ALL_AGENTS
            .into_iter()
            .filter(|agent| agent.subagent_stop_closes_the_spawn())
            .map(AgentKind::slug)
            .collect::<Vec<_>>();
        assert_eq!(spawn_bracketed, ["zo"]);
        assert!(!AgentKind::Claude.subagent_stop_closes_the_spawn());
    }

    /// Which agent gives which signal, as measured. Thirty-odd read by
    /// silence, which is why the quiet timer is the common case and not a
    /// fallback.
    #[test]
    fn the_ready_signals_are_the_ones_that_were_measured() {
        let with = |mark: ReadyMark| -> Vec<&str> {
            AGENT_SPECS
                .iter()
                .filter(|spec| spec.ready == mark)
                .map(|spec| spec.id)
                .collect()
        };
        assert_eq!(with(ReadyMark::CursorShown), ["opencode", "mimo-code"]);
        assert_eq!(with(ReadyMark::Quiet).len(), 30);
        // The glyph marks, each with the glyph its own row measured. Claude
        // Code's `❯` left silence for its glyph on 2026-09-17: 2.1.274 turns
        // bracketed paste on 0.3 s in, is silent for three seconds, and takes
        // that input handler down (`?2004l`) with whatever a quiet wait
        // pasted into it; its composer's `❯` is the first thing drawn under
        // the handler that stays (recordings in zerocode-pty's fixtures).
        let composer: Vec<(&str, char)> = AGENT_SPECS
            .iter()
            .filter_map(|spec| match spec.ready {
                ReadyMark::ComposerPrompt(glyph) => Some((spec.id, glyph)),
                _ => None,
            })
            .collect();
        assert_eq!(composer, [("claude", '\u{276f}'), ("codex", '\u{203a}')]);
        let alternate: Vec<(&str, char)> = AGENT_SPECS
            .iter()
            .filter_map(|spec| match spec.ready {
                ReadyMark::AltScreenPrompt(glyph) => Some((spec.id, glyph)),
                _ => None,
            })
            .collect();
        assert_eq!(alternate, [("grok", '\u{276f}')]);
        // And the measured timing exceptions. agy draws a composer before its
        // authentication gate is ready to accept that composer's Enter, so it
        // needs a longer quiet window as well as room for that window. Codex
        // keeps Orca's existing cold-mount deadline.
        let quieter: Vec<(&str, u32)> = AGENT_SPECS
            .iter()
            .filter_map(|spec| spec.ready_quiet_ms.map(|ms| (spec.id, ms)))
            .collect();
        assert_eq!(quieter, [("antigravity", 4_000)]);
        let slower: Vec<(&str, u32)> = AGENT_SPECS
            .iter()
            .filter_map(|spec| spec.ready_timeout_ms.map(|ms| (spec.id, ms)))
            .collect();
        assert_eq!(slower, [("codex", 20_000), ("antigravity", 20_000)]);
    }

    /// A platform an agent cannot run on is not offered on that platform.
    #[test]
    fn an_agent_is_not_offered_where_it_cannot_run() {
        for spec in &AGENT_SPECS {
            for os in spec.unsupported {
                assert!(!spec.runs_on(os), "{} claims to run on {os}", spec.id);
            }
            assert!(spec.runs_on("macos") || spec.unsupported.contains(&"macos"));
        }
    }

    #[test]
    fn every_alias_is_a_way_to_find_the_same_agent() {
        for spec in &AGENT_SPECS {
            let names: Vec<&str> = spec.detect_names().collect();
            assert_eq!(names[0], spec.detect, "the primary name comes first");
            assert_eq!(names.len(), 1 + spec.detect_aliases.len());
            let mut seen = std::collections::BTreeSet::new();
            for name in names {
                assert!(seen.insert(name), "{} lists {name} twice", spec.id);
            }
        }
    }

    /* ---- crossing the registry with the machine ---- */

    /// A fake `PATH` holding exactly the named executables, so presence is
    /// testable without depending on what happens to be installed.
    fn fake_path(commands: &[&str]) -> (tempfile::TempDir, std::ffi::OsString) {
        let dir = tempfile::tempdir().expect("temp dir");
        for command in commands {
            let file = dir.path().join(command);
            std::fs::write(&file, b"#!/bin/sh\n").expect("write");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
        let var = dir.path().as_os_str().to_os_string();
        (dir, var)
    }

    #[test]
    fn presence_answers_for_every_agent_and_finds_only_what_is_there() {
        let (_dir, path) = fake_path(&["codex", "claude"]);
        let found = agent_presence(Some(&path), "macos");
        assert_eq!(
            found.len(),
            AGENT_SPECS.len(),
            "an agent went unanswered for"
        );
        let installed: Vec<&str> = found
            .iter()
            .filter(|one| one.installed)
            .map(|one| one.id)
            .collect();
        assert_eq!(installed, ["claude", "codex"], "{installed:?}");
        // Order follows the registry, not what is installed: a picker whose
        // rows move as tools come and go is a picker you cannot aim at.
        let ids: Vec<&str> = found.iter().map(|one| one.id).collect();
        let registry: Vec<&str> = AGENT_SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, registry);
    }

    #[test]
    fn resolve_on_path_returns_the_same_executable_presence_uses() {
        let (_dir, path) = fake_path(&["agy"]);
        let resolved = resolve_on_path(Some(&path), "agy").expect("agy on PATH");
        assert_eq!(
            resolved.file_name().and_then(|name| name.to_str()),
            Some("agy")
        );
        assert!(on_path(Some(&path), "agy"));
        assert_eq!(resolve_on_path(Some(&path), "missing-agent"), None);
    }

    /// An alias is a real way to have an agent, and the answer says which name
    /// was found — `vibe` and `mistral-vibe` are the same tool.
    #[test]
    fn an_alias_counts_as_installed_and_is_named() {
        let vibe = agent_spec("mistral-vibe").expect("in the registry");
        assert!(vibe.detect_aliases.contains(&"mistral-vibe"));
        let (_dir, path) = fake_path(&["mistral-vibe"]);
        let found = agent_presence(Some(&path), "macos");
        let row = found
            .iter()
            .find(|one| one.id == "mistral-vibe")
            .expect("row");
        assert!(row.installed);
        assert_eq!(row.found_as.as_deref(), Some("mistral-vibe"));
    }

    /// A wrapper without the thing it wraps is not installed, however present
    /// its own binary is — and the answer says which companion is missing
    /// rather than only that it will not run.
    #[test]
    fn a_missing_companion_command_is_named_not_swallowed() {
        // Constructed rather than found in the registry: the entry that had a
        // requirement is the one deliberately absent, so this proves the rule
        // holds for whichever agent grows one next.
        let spec = AgentSpec {
            requires: &["a-tool-that-is-not-here"],
            ..*agent_spec("codex").expect("in the registry")
        };
        assert!(spec.requires.iter().any(|needed| !on_path(None, needed)));
        let (_dir, path) = fake_path(&["codex"]);
        assert!(on_path(Some(&path), "codex"));
        assert!(!on_path(Some(&path), "a-tool-that-is-not-here"));
    }

    #[test]
    fn nothing_is_installed_when_there_is_no_path_at_all() {
        for row in agent_presence(None, "macos") {
            assert!(!row.installed, "{} was found with no PATH", row.id);
            assert_eq!(row.found_as, None);
        }
    }

    /// Present on disk but not runnable here is a different sentence from not
    /// installed, and the answer keeps them apart.
    #[test]
    fn a_platform_an_agent_cannot_run_on_is_reported_as_such() {
        let spec = AgentSpec {
            unsupported: &["macos"],
            ..*agent_spec("codex").expect("in the registry")
        };
        assert!(!spec.runs_on("macos"));
        assert!(spec.runs_on("linux"));
    }

    #[test]
    fn a_directory_named_like_an_agent_is_not_an_agent() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir(dir.path().join("codex")).expect("mkdir");
        let path = dir.path().as_os_str().to_os_string();
        assert!(!on_path(Some(&path), "codex"));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_without_an_execute_bit_is_not_an_agent() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("temp dir");
        let file = dir.path().join("codex");
        std::fs::write(&file, b"not executable").expect("write");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let path = dir.path().as_os_str().to_os_string();
        assert!(!on_path(Some(&path), "codex"));
    }

    #[test]
    fn unknown_agent_ids_are_rejected_rather_than_guessed() {
        assert!(agent_spec("not-an-agent").is_none());
        assert!(agent_spec("Claude").is_none());
        assert!(agent_spec("").is_none());
    }

    #[test]
    fn default_agent_preference_keeps_auto_blank_and_agent_distinct() {
        let cases = [
            DefaultAgentPreference::Auto,
            DefaultAgentPreference::Blank,
            DefaultAgentPreference::Agent {
                id: "claude".to_string(),
            },
        ];
        for preference in cases {
            let json = serde_json::to_string(&preference).expect("serialize preference");
            let restored: DefaultAgentPreference =
                serde_json::from_str(&json).expect("restore preference");
            assert_eq!(restored, preference);
        }
        assert_eq!(
            serde_json::to_string(&DefaultAgentPreference::Blank).unwrap(),
            r#"{"kind":"blank"}"#
        );
        assert_eq!(
            DefaultAgentPreference::from_legacy("claude"),
            Some(DefaultAgentPreference::Agent {
                id: "claude".to_string()
            })
        );
        assert_eq!(
            DefaultAgentPreference::from_legacy("blank"),
            Some(DefaultAgentPreference::Blank)
        );
        assert!(
            DefaultAgentPreference::Agent {
                id: "not-an-agent".to_string()
            }
            .validated()
            .is_none()
        );
    }

    #[test]
    fn unknown_slug_is_rejected_rather_than_guessed() {
        assert_eq!(AgentKind::from_slug("not-an-agent"), None);
        assert_eq!(AgentKind::from_slug("Claude"), None);
        assert_eq!(AgentKind::from_slug(""), None);
    }
    /// A pty's foreground program names exactly one agent — the detection
    /// road stays honest only while no two specs claim one process name.
    #[test]
    fn a_foreground_program_names_its_agent_and_only_one() {
        assert_eq!(
            agent_spec_by_process("claude").map(|s| s.id),
            Some("claude")
        );
        assert_eq!(
            agent_spec_by_process("agy").map(|s| s.id),
            Some("antigravity"),
            "antigravity answers to its real binary name"
        );
        assert_eq!(agent_spec_by_process("zsh"), None);
        assert_eq!(agent_spec_by_process(""), None);
        let mut seen = std::collections::HashSet::new();
        // The lane enum and the registry answer for the same binary: a
        // command() the binary does not wear probes PATH for a file that
        // never exists (antigravity ships as `agy`).
        assert_eq!(AgentKind::Antigravity.command(), "agy");
        assert_eq!(
            agent_spec("antigravity").expect("in the registry").launch,
            AgentKind::Antigravity.command(),
        );
        for spec in AGENT_SPECS {
            assert!(
                seen.insert(spec.expected_process),
                "two agents claim the process `{}`",
                spec.expected_process
            );
        }
    }

    /// Clearing a running composer is a measured provider capability, not a
    /// universal terminal-editor assumption.
    #[test]
    fn only_codex_consumes_the_composer_clear_keys() {
        assert_eq!(
            agent_spec("codex").map(|spec| spec.composer_clear),
            Some(ComposerClear::Keys)
        );
        for id in ["claude", "zo", "openclaude"] {
            assert_eq!(
                agent_spec(id).map(|spec| spec.composer_clear),
                Some(ComposerClear::None),
                "{id} inserts the clear burst as text"
            );
        }
        assert!(
            AGENT_SPECS
                .iter()
                .filter(|spec| spec.id != "codex")
                .all(|spec| spec.composer_clear == ComposerClear::None),
            "an unmeasured agent was opted into destructive clear keys"
        );
    }

    /// The lane enum and terminal catalogue share one binary-name authority.
    #[test]
    fn lane_commands_follow_the_terminal_catalogue() {
        for kind in ALL_AGENTS {
            let spec = agent_spec(kind.slug()).expect("every lane has a terminal entry");
            assert_eq!(kind.command(), spec.detect, "{} drifted", kind.slug());
            assert_eq!(
                spec.launch.split_whitespace().next(),
                Some(spec.detect),
                "{} launches a different binary than it detects",
                spec.id
            );
        }
    }

    /// Two rows the catalog named after a product rather than after a
    /// command, until the login survey read the vendors' own documents
    /// (t-6009, t-6120).
    ///
    /// Rovo Dev is an extension of Atlassian's CLI — `acli rovodev run` —
    /// and a bare `rovo` is in no Atlassian document; Cursor's CLI is
    /// `agent` now, with `cursor-agent` kept as the legacy link its
    /// installer still makes. Both are detected by the command they launch,
    /// and the process to expect in a pty is the executable's own name.
    #[test]
    fn the_two_renamed_clis_are_detected_by_the_command_their_vendors_document() {
        let rovo = agent_spec("rovo").expect("in the registry");
        assert_eq!(rovo.detect, "acli");
        assert_eq!(rovo.launch, "acli rovodev run");
        assert_eq!(rovo.expected_process, "acli");
        let cursor = agent_spec("cursor").expect("in the registry");
        assert_eq!(cursor.launch, "agent");
        assert_eq!(cursor.detect, "agent");
        assert_eq!(cursor.detect_aliases, &["cursor-agent"]);
        // Both names still mean Cursor on a machine that has either.
        assert_eq!(
            cursor.detect_names().collect::<Vec<_>>(),
            vec!["agent", "cursor-agent"]
        );
        // The symlinks point at one file called `cursor-agent`, so that is
        // what a pty shows.
        assert_eq!(cursor.expected_process, "cursor-agent");
        // And no other row answers to those names.
        for spec in &AGENT_SPECS {
            if spec.id == "cursor" || spec.id == "rovo" {
                continue;
            }
            for name in spec.detect_names() {
                assert!(
                    !["agent", "cursor-agent", "acli"].contains(&name),
                    "{} answers to a name the renamed rows took",
                    spec.id
                );
            }
        }
    }

    /// A launch prompt goes where the spec says, and nowhere else.
    #[test]
    fn a_launch_prompt_rides_the_road_its_agent_reads() {
        let road = |id: &str, prompt: &str| {
            prompt_injection(agent_spec(id).expect("in the registry"), prompt)
        };

        // Positional and bare — claude runs it at once. NOT `--prefill`, which
        // seeds the composer without submitting (the map's P0-1).
        assert_eq!(
            road("claude", "고쳐"),
            Some(Injected::Argv(vec!["고쳐".to_string()])),
        );
        assert!(
            !matches!(road("claude", "고쳐"), Some(Injected::Argv(ref words)) if
                words.iter().any(|word| word.starts_with("--prefill"))),
            "the launch prompt took the draft flag"
        );

        // An agent that wants a separator gets one, before the prompt.
        let separated: Vec<&'static AgentSpec> = AGENT_SPECS
            .iter()
            .filter(|spec| spec.argv_separator.is_some() && spec.injection == Injection::Argv)
            .collect();
        assert!(!separated.is_empty(), "no agent wants a separator any more");
        for spec in separated {
            let separator = spec.argv_separator.expect("filtered for it");
            assert_eq!(
                prompt_injection(spec, "일"),
                Some(Injected::Argv(vec![
                    separator.to_string(),
                    "일".to_string()
                ])),
                "{} lost its separator",
                spec.id
            );
        }

        // The flag modes spell fixed flags, and the typed road carries no argv
        // at all — a prompt on the command line is a prompt in every `ps` on
        // the machine.
        for spec in AGENT_SPECS {
            match (spec.injection, prompt_injection(&spec, "일")) {
                (Injection::FlagPrompt, Some(Injected::Argv(words))) => {
                    assert_eq!(words[0], "--prompt", "{}", spec.id);
                }
                (Injection::FlagPromptInteractive, Some(Injected::Argv(words))) => {
                    assert_eq!(words[0], "--prompt-interactive", "{}", spec.id);
                }
                (Injection::FlagInteractive, Some(Injected::Argv(words))) => {
                    assert_eq!(words[0], "-i", "{}", spec.id);
                }
                (Injection::Argv, Some(Injected::Argv(_))) => {}
                (
                    Injection::StdinAfterStart | Injection::HermesQuery,
                    Some(Injected::AfterStart(text)),
                ) => {
                    assert_eq!(text, "일", "{}", spec.id);
                }
                (mode, other) => panic!("{} takes {mode:?} but answered {other:?}", spec.id),
            }
        }

        // Nothing to say is not the same as an empty instruction.
        for spec in AGENT_SPECS {
            assert_eq!(prompt_injection(&spec, "   "), None, "{}", spec.id);
        }
    }
}
