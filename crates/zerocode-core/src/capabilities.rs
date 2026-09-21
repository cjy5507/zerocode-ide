//! The harness capability table: what every write door asks about an agent.
//!
//! Modelled on agent-orchestrator's `backend/internal/ports/agent.go`, where
//! an adapter's optional powers are separate interfaces (`AgentAuthChecker`,
//! `AgentPromptReadinessProvider`, `EmptyComposerDetector`,
//! `AgentInterfaceHandoff`, …) and a consumer asks `agent.(Capability)`
//! instead of switching on the adapter's name. Go's shape is interfaces; ours
//! is data. Every fact lives once, on the agent's own row of [`AGENT_SPECS`],
//! and [`AgentCapabilities`] is the typed answer a door reads off that row.
//!
//! The rule this module exists for: a write door — launch, resume, worker
//! split, automation, mail paste — never spells an agent's name. It asks the
//! row. Two P0s came from doors that knew better than the table: a launch
//! prompt carried on `claude --prefill` (seeded, never submitted), and an
//! advice line typed at a pane whose agent had left (the shell ran its
//! backticks). A door that asks has no name to get wrong, and changing what
//! one agent can do is one row of this file.
//!
//! [`AGENT_SPECS`]: crate::agent::AGENT_SPECS

use serde::Serialize;

use crate::account::{Provider, providers_for};
use crate::agent::{AgentSpec, ComposerClear, Injection, NudgeRoad, ReadyMark, agent_spec};

/// How the pane that runs this agent is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpawnRoad {
    /// A plain pty child, the road every vendor's CLI takes.
    Pty,
    /// The pane supervisor's socket-driven pane: `--events-bind`, a readiness
    /// fence, a channel registry and a subscriber. This project's own harness
    /// is the one agent that has such a pane, and a bare pty spawn of it
    /// restores the command line without the channel.
    SocketPane,
}

/// How the agent reports that Enter consumed the prompt it was handed.
///
/// A launch or a worker briefing arms a one-shot acknowledgement only where
/// the agent has a way to give one; an agent with none falls back to the
/// terminal-delivery receipt and reports one fact fewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmitAck {
    /// Nothing observable: a plain TUI with no lifecycle transport.
    None,
    /// A lifecycle hook's `UserPromptSubmit`.
    Hook,
    /// The pane channel's `turn{phase:"start"}` frame. The worker split parks
    /// the briefing until the subscriber has joined, or a fast turn can start
    /// and end before anything listens.
    Channel,
}

/// A first-run "do you trust this folder?" menu, named by the artifact the
/// window writes before the launch so the menu never reads the pasted prompt
/// as its keystroke (the map's P0-8). The writer lives with the window; the
/// table only says which agent opens which menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustMenu {
    Zo,
    /// Claude Code's "Quick safety check" folder-trust dialog, shown under
    /// `--dangerously-skip-permissions` too (measured on 2.1.273).
    Claude,
    Cursor,
    Copilot,
    Codex,
    Antigravity,
}

/// A native pointer route beside the pty, offered to a worker whose vendor
/// has one. Refused capability leaves argv untouched and the pty stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PointerRoute {
    /// Codex's per-process app server (`codex_queue`).
    CodexAppServer,
}

/// The keys that drive one CLI's own effort picker, read off its screen.
///
/// Measured on Claude Code 2.1.278 (2026-09-21, `docs/captures/
/// claude-code-2.1.278-effort-slash.txt`): `/effort` with no argument opens a
/// one-line picker — `←/→ to adjust · Enter to confirm · s for this session
/// only · Esc to cancel` — standing on the session's current level. `/effort
/// <level>` with a word applies at once too, but "saved as your default for
/// new sessions": it rewrites `modelSettings` in the account's settings file,
/// which every later launch of that account without `--effort` then starts
/// on. The picker's `s` is the one road that leaves the file alone, so the
/// beat takes it and moves one rung a time — one arrow, then `s` — which is
/// also why no ladder arithmetic is needed to drive it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PickerKeys {
    /// The slash command that opens the picker, typed as a line at the
    /// resting composer through the guarded door.
    pub open: &'static str,
    /// One rung down.
    pub lower: &'static str,
    /// One rung up.
    pub raise: &'static str,
    /// Confirms for this session only.
    pub session_only: &'static str,
    /// Words the picker prints while it is up. The keys are sent only after
    /// the screen shows them; a screen without them gets no key.
    pub legend: &'static str,
}

/// How a RUNNING agent's model or effort moves between two turns, read off
/// its row — the door the beat's step-effort seat asks before it types
/// anything at a worker (t-5637).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MoveRoad {
    /// One line at the resting composer: the command and the new word
    /// (`/model <id>`). Whether the CLI also saves the word as its default
    /// is the CLI's own habit — Claude Code does for `/model`.
    Line(&'static str),
    /// The command opens the CLI's own picker, and these measured keys drive
    /// it one rung a time without touching the CLI's saved defaults.
    Picker(PickerKeys),
    /// The command opens a picker whose keys this window has not measured:
    /// a composer chip types it and shows the person the screen; the beat
    /// moves nothing through it.
    Shown(&'static str),
    /// Nothing typed at a running session — Codex 0.155.1 sends `/model low`
    /// to the model as a message (its rollout keeps it as a user turn) and
    /// its picker persists to `config.toml`, so the only dial is the next
    /// summons' own (`orchestration::launch_tuning`'s row).
    Relaunch,
}

impl MoveRoad {
    /// The slash command a composer types to reach this road, if typing
    /// reaches it at all.
    #[must_use]
    pub const fn command(self) -> Option<&'static str> {
        match self {
            Self::Line(command) | Self::Shown(command) => Some(command),
            Self::Picker(keys) => Some(keys.open),
            Self::Relaunch => None,
        }
    }

    /// The one line that carries `word` down this road, or `None` for a road
    /// no line carries: a picker takes keys, a shown picker takes a person,
    /// a relaunch takes a summons.
    #[must_use]
    pub fn line(self, word: &str) -> Option<String> {
        match self {
            Self::Line(command) => Some(format!("{command} {word}")),
            Self::Picker(_) | Self::Shown(_) | Self::Relaunch => None,
        }
    }

    /// The picker's keys, when the road is one.
    #[must_use]
    pub const fn picker(self) -> Option<PickerKeys> {
        match self {
            Self::Picker(keys) => Some(keys),
            Self::Line(_) | Self::Shown(_) | Self::Relaunch => None,
        }
    }

    /// The word a ledger row keeps for the door this road is.
    #[must_use]
    pub const fn door(self) -> &'static str {
        match self {
            Self::Line(_) => "line",
            Self::Picker(_) => "picker",
            Self::Shown(_) => "shown",
            Self::Relaunch => "relaunch",
        }
    }

    /// Whether the beat can move a running session down this road on its
    /// own — a line it types or a picker it drives.
    #[must_use]
    pub const fn moves_between_turns(self) -> bool {
        matches!(self, Self::Line(_) | Self::Picker(_))
    }
}

/// The two between-turn moves of one agent and the ladder its effort words
/// stand on — three columns of the row, read together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TurnMoves {
    /// How the effort moves between turns; `None` where nobody measured one.
    pub effort: Option<MoveRoad>,
    /// How the model moves between turns; `None` where nobody measured one.
    pub model: Option<MoveRoad>,
    /// The effort words the CLI's own picker offers, lowest first — what a
    /// row's `to` word is read off when a move goes up or down one rung. A
    /// CLI whose picker was not read has an empty ladder and gets no `to`.
    pub ladder: &'static [&'static str],
}

impl TurnMoves {
    /// No measured road either way.
    pub const NONE: Self = Self {
        effort: None,
        model: None,
        ladder: &[],
    };

    /// The word one rung above or below `current` on this ladder — `None`
    /// off the ladder's end, and `None` for a word the ladder does not know,
    /// which is a word this row was not measured with.
    #[must_use]
    pub fn rung_from(&self, current: &str, up: bool) -> Option<&'static str> {
        let at = self.ladder.iter().position(|word| *word == current)?;
        let next = if up {
            at.checked_add(1)?
        } else {
            at.checked_sub(1)?
        };
        self.ladder.get(next).copied()
    }

    /// Whether `word` stands above `floor` on this ladder; two words the
    /// ladder does not both know stand nowhere in particular.
    #[must_use]
    pub fn stands_above(&self, word: &str, floor: &str) -> bool {
        let rung = |name: &str| self.ladder.iter().position(|held| *held == name);
        matches!((rung(word), rung(floor)), (Some(high), Some(low)) if high > low)
    }
}

/// Claude Code's effort picker, as its own screen spells it (2.1.278).
pub const CLAUDE_EFFORT_PICKER: PickerKeys = PickerKeys {
    open: "/effort",
    lower: "\x1b[D",
    raise: "\x1b[C",
    session_only: "s",
    legend: "s for this session only",
};

/// The levels Claude Code's picker offers, lowest first, as its own line
/// draws them (`low medium high xhigh max ultracode`); `ultracode` is left
/// off because it is xhigh plus workflow orchestration, not a rung above
/// max — and `auto`, which `/effort` also takes, is a return to the model's
/// default rather than a level.
pub const CLAUDE_EFFORT_LADDER: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The reasoning efforts Codex 0.155.1's picker offers, lowest first, read
/// off its binary's own list (`minimal low medium high xhigh max ultra`);
/// which of them a model supports is the model's own row in its catalog.
pub const CODEX_EFFORT_LADDER: &[&str] =
    &["minimal", "low", "medium", "high", "xhigh", "max", "ultra"];

/// Which witness says whether a resumed conversation was cut mid-turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakeMark {
    /// The hook's word at the last persist, as stored.
    Hook,
    /// The vendor's own rollout file, read at wake time — it outlives the
    /// process and is final where the stored hook word is stale both ways
    /// (`docs/design/restart-nudge-delivery.md` §4).
    Rollout,
}

/// One resume/continue selector on the agent's own command line.
///
/// A resume door writes exactly one authoritative selector, so any of these
/// saved into the launch args is spliced out first (#12982). Long flags take
/// their value bare or joined with `=`; a short flag only bare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Selector {
    pub flag: &'static str,
    pub takes_value: bool,
}

impl Selector {
    const fn bare(flag: &'static str) -> Self {
        Self {
            flag,
            takes_value: false,
        }
    }

    const fn with_value(flag: &'static str) -> Self {
        Self {
            flag,
            takes_value: true,
        }
    }

    fn matches_joined(&self, arg: &str) -> bool {
        self.takes_value
            && self.flag.starts_with("--")
            && arg.len() > self.flag.len() + 1
            && arg.starts_with(self.flag)
            && arg.as_bytes()[self.flag.len()] == b'='
    }
}

/// Take every resume/continue selector out of a launch line, leaving the
/// rest as written. A selector's value rides the next token when that token
/// is not itself a flag; a flag after a bare `--resume` is not its value and
/// stays.
#[must_use]
pub fn args_without_selectors(selectors: &[Selector], args: &[String]) -> Vec<String> {
    if selectors.is_empty() {
        return args.to_vec();
    }
    let mut kept = Vec::with_capacity(args.len());
    let mut walk = args.iter().peekable();
    while let Some(arg) = walk.next() {
        match selectors.iter().find(|selector| selector.flag == arg) {
            Some(selector) => {
                if selector.takes_value && walk.peek().is_some_and(|next| !next.starts_with('-')) {
                    walk.next();
                }
            }
            None if selectors
                .iter()
                .any(|selector| selector.matches_joined(arg)) => {}
            None => kept.push(arg.clone()),
        }
    }
    kept
}

/// The shape an id has to wear before it may reach an agent's argv.
///
/// The value came off a payload or a directory listing, and this is the line
/// where it stops being data and starts being a command-line argument — so a
/// crafted "id" can never smuggle an extra argument shape into the launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdShape {
    /// A lowercase hex UUID — the stem of a Claude session store file.
    Uuid,
}

impl IdShape {
    #[must_use]
    pub fn accepts(self, said: &str) -> bool {
        match self {
            Self::Uuid => {
                said.len() == 36
                    && said.bytes().enumerate().all(|(spot, byte)| match spot {
                        8 | 13 | 18 | 23 => byte == b'-',
                        _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
                    })
            }
        }
    }
}

/// Re-entering a conversation from the agent's own session store, by id,
/// from the launch door — the road the notes menu's "resume" takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StoreResume {
    /// The flag the id rides behind.
    pub flag: &'static str,
    /// What an id from that store looks like; anything else is refused.
    pub id: IdShape,
}

/// What "blocked" looks like on an agent — the stops the window can see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockedSignal {
    /// A first-run trust menu, pre-answered by the named artifact.
    TrustMenu(TrustMenu),
    /// An account gate that repaints after the composer first appears, so
    /// the composer's Enter is discarded until it settles; the agent's longer
    /// quiet window is how the launch waits it out.
    LoginGate,
    /// A permission question arrives as a hook's `NeedsAttention`.
    HookAsk,
    /// A permission question arrives as a pane-channel `ask` frame.
    ChannelAsk,
    /// Nothing announces the stop: the stall probe reads screen silence.
    Silence,
}

/// The harness facts of one agent that used to be agent-name branches at the
/// write doors, now one field each on its row of [`AGENT_SPECS`].
///
/// [`PLAIN`](Self::PLAIN) is the row of a vendor TUI with none of them, and
/// most rows are exactly that: a pty child, no acknowledgement transport, no
/// trust menu, no pointer route, a plain resume.
///
/// [`AGENT_SPECS`]: crate::agent::AGENT_SPECS
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Harness {
    pub spawn: SpawnRoad,
    pub submit_ack: SubmitAck,
    /// The MEASURED stops — trust menus and login gates. The ask transport
    /// and silence are derived from the row's other facts; see
    /// [`AgentCapabilities::blocked_signals`].
    pub blocked: &'static [BlockedSignal],
    pub pointer_route: Option<PointerRoute>,
    /// The selectors a resume splices out of saved launch args.
    pub resume_selectors: &'static [Selector],
    /// The launch door's store-resume road, where the window scans a store.
    pub resume_store: Option<StoreResume>,
    pub wake_mark: WakeMark,
    /// Whether a vault reopen of this agent carries the launch plan's args —
    /// Orca's `RESUMABLE_TUI_AGENTS` intersected with the agents the vault
    /// scans (renderer ai-vault-resume-command.ts:163-224).
    pub vault_resume_carries_launch_args: bool,
    /// The local witnesses a readiness probe may ask about this agent's
    /// login; empty for an agent whose login this window cannot see.
    pub auth_probes: &'static [AuthProbeKind],
    /// How the agent's effort and model move between two turns of a running
    /// session, and the effort ladder its words stand on (t-5637) — the
    /// beat's step-effort seat and the composer's model chip both read
    /// these; neither spells a command of its own.
    pub moves: TurnMoves,
}

impl Harness {
    /// A vendor TUI with none of the special roads.
    pub const PLAIN: Self = Self {
        spawn: SpawnRoad::Pty,
        submit_ack: SubmitAck::None,
        blocked: &[],
        pointer_route: None,
        resume_selectors: &[],
        resume_store: None,
        wake_mark: WakeMark::Hook,
        vault_resume_carries_launch_args: false,
        auth_probes: &[],
        moves: TurnMoves::NONE,
    };
}

/// Claude's resume/continue selectors (`buildClaudeResumeLaunchCommand`,
/// #12982): the two spellings of each, the long one also joined with `=`.
pub const CLAUDE_RESUME_SELECTORS: &[Selector] = &[
    Selector::bare("--continue"),
    Selector::bare("-c"),
    Selector::with_value("--resume"),
    Selector::with_value("-r"),
];

/// How a LAUNCH prompt reaches the agent — the road the launch words take,
/// read off the measured [`Injection`] mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Submit {
    /// Bare on the command line, behind the separator when the agent wants
    /// one; the agent runs it at once.
    Argv { separator: Option<&'static str> },
    /// Behind this fixed flag on the command line.
    Flag(&'static str),
    /// Typed at the agent after its ready signal — the delivery pump's road.
    Typed,
}

/// The startup facts a mounting delivery waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Startup {
    pub mark: ReadyMark,
    /// A quiet window of the agent's own after the handshake, or the shared
    /// one when `None`.
    pub quiet_ms: Option<u32>,
    /// A hard deadline of the agent's own, or the shared one when `None`.
    pub timeout_ms: Option<u32>,
}

/// Steering a RUNNING agent: every catalog agent is steered by a paste at its
/// resting composer; `clear` says whether edit keys may empty that composer
/// first, which only a measured agent consumes as commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Steer {
    pub clear: ComposerClear,
}

/// How the window proves the composer is empty before it pastes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EmptyComposerProof {
    /// The measured clear burst empties whatever is on the line.
    ClearKeys,
    /// The window's own ledger of hands and drafts on the line; a line it
    /// cannot account for is refused at the write, never cleared.
    DraftLedger,
}

/// One local witness a readiness probe may ask about an agent's login
/// (t-3996). Which witnesses an agent has is a fact of its row; the probe
/// reads the list and runs the checks the account modules already own,
/// never a login and never a network call of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthProbeKind {
    /// The CLI's own identity file in the home a launch would read —
    /// `settings.json`'s account block for Claude, `auth.json` for Codex.
    IdentityFile,
    /// The scoped keychain item this window seeds for a managed Claude home.
    /// Asked only about a home this window owns; the person's own item is
    /// never read (its access list does not name us, and a read would put
    /// a dialog on their screen).
    Keychain,
    /// `gh auth status`, for an agent that rides the GitHub CLI's login.
    GhAuthStatus,
}

/// Whether the window can read this agent's login before a launch: the
/// accounts it materialises and switches for it, and the witnesses its
/// readiness probe may ask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthProbe {
    pub providers: &'static [Provider],
    pub probes: &'static [AuthProbeKind],
}

/// Everything about re-entering a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Resume {
    /// Where a restart continuation goes after the resume.
    pub nudge: NudgeRoad,
    pub selectors: &'static [Selector],
    pub store: Option<StoreResume>,
    pub wake_mark: WakeMark,
    pub vault_carries_launch_args: bool,
}

impl Resume {
    /// The launch plan's args with this agent's resume selectors spliced out,
    /// so the resume door's own selector is the only one on the line.
    #[must_use]
    pub fn launch_args_without_selectors(&self, args: &[String]) -> Vec<String> {
        args_without_selectors(self.selectors, args)
    }
}

/// One agent's harness capabilities — the typed answer a write door reads
/// off the agent's row, grouped the way agent.go groups its interfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentCapabilities {
    pub id: &'static str,
    pub submit: Submit,
    /// The measured mode `submit` was read from, kept for the one rule that
    /// distinguishes the two typed vendors: a quick command refuses only an
    /// agent measured as `StdinAfterStart`.
    pub injection: Injection,
    pub submit_ack: SubmitAck,
    pub startup: Startup,
    pub steer: Steer,
    pub empty_composer_proof: EmptyComposerProof,
    pub blocked: &'static [BlockedSignal],
    pub auth_probe: Option<AuthProbe>,
    pub resume: Resume,
    pub spawn: SpawnRoad,
    pub pointer_route: Option<PointerRoute>,
    /// The between-turn moves and the effort ladder, off the row.
    pub moves: TurnMoves,
}

impl AgentCapabilities {
    /// The trust menu this agent opens on first run, if it opens one.
    #[must_use]
    pub fn trust_menu(&self) -> Option<TrustMenu> {
        self.blocked.iter().find_map(|signal| match signal {
            BlockedSignal::TrustMenu(menu) => Some(*menu),
            _ => None,
        })
    }

    /// Every way this agent's stop can reach the window: the measured stops
    /// on the row, the ask transport its acknowledgement road implies, and
    /// silence, which the stall probe reads on everybody.
    #[must_use]
    pub fn blocked_signals(&self) -> Vec<BlockedSignal> {
        let mut signals = self.blocked.to_vec();
        match self.submit_ack {
            SubmitAck::Hook => signals.push(BlockedSignal::HookAsk),
            SubmitAck::Channel => signals.push(BlockedSignal::ChannelAsk),
            SubmitAck::None => {}
        }
        signals.push(BlockedSignal::Silence);
        signals
    }

    /// Whether a prompt can be handed to this agent as it starts — the
    /// quick-command rule. Only an agent measured as `StdinAfterStart` is
    /// refused; the flag modes, argv, and the vendor with its own query
    /// subcommand all take a prompt at the launcher.
    #[must_use]
    pub const fn takes_prompt_at_start(&self) -> bool {
        !matches!(self.injection, Injection::StdinAfterStart)
    }
}

impl AgentSpec {
    /// This row, read as the capabilities a write door asks about.
    #[must_use]
    pub fn capabilities(&self) -> AgentCapabilities {
        let submit = match self.injection {
            Injection::Argv => Submit::Argv {
                separator: self.argv_separator,
            },
            Injection::FlagPrompt => Submit::Flag("--prompt"),
            Injection::FlagPromptInteractive => Submit::Flag("--prompt-interactive"),
            Injection::FlagInteractive => Submit::Flag("-i"),
            // Hermes takes the typed road deliberately: Orca plans a bespoke
            // query invocation for it, and a paste an agent reads is better
            // than a subcommand this window would be guessing at.
            Injection::StdinAfterStart | Injection::HermesQuery => Submit::Typed,
        };
        let providers = providers_for(self.id);
        AgentCapabilities {
            id: self.id,
            submit,
            injection: self.injection,
            submit_ack: self.harness.submit_ack,
            startup: Startup {
                mark: self.ready,
                quiet_ms: self.ready_quiet_ms,
                timeout_ms: self.ready_timeout_ms,
            },
            steer: Steer {
                clear: self.composer_clear,
            },
            empty_composer_proof: match self.composer_clear {
                ComposerClear::Keys => EmptyComposerProof::ClearKeys,
                ComposerClear::None => EmptyComposerProof::DraftLedger,
            },
            blocked: self.harness.blocked,
            auth_probe: (!providers.is_empty() || !self.harness.auth_probes.is_empty()).then_some(
                AuthProbe {
                    providers,
                    probes: self.harness.auth_probes,
                },
            ),
            resume: Resume {
                nudge: self.resume_nudge,
                selectors: self.harness.resume_selectors,
                store: self.harness.resume_store,
                wake_mark: self.harness.wake_mark,
                vault_carries_launch_args: self.harness.vault_resume_carries_launch_args,
            },
            spawn: self.harness.spawn,
            pointer_route: self.harness.pointer_route,
            moves: self.harness.moves,
        }
    }
}

/// The capabilities of the agent called `agent`, or `None` for a name the
/// catalogue does not know — which a door refuses rather than guesses at.
#[must_use]
pub fn agent_capabilities(agent: &str) -> Option<AgentCapabilities> {
    agent_spec(agent).map(AgentSpec::capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AGENT_SPECS, Injected, prompt_injection};

    fn ids_where(pick: impl Fn(&AgentCapabilities) -> bool) -> Vec<&'static str> {
        AGENT_SPECS
            .iter()
            .map(AgentSpec::capabilities)
            .filter(|caps| pick(caps))
            .map(|caps| caps.id)
            .collect()
    }

    #[test]
    fn every_agent_answers_through_the_one_door() {
        for spec in &AGENT_SPECS {
            let asked = agent_capabilities(spec.id).expect("every row answers");
            assert_eq!(asked, spec.capabilities(), "{}", spec.id);
            assert_eq!(asked.id, spec.id);
        }
        assert_eq!(agent_capabilities("not-an-agent"), None);
        assert_eq!(agent_capabilities("Claude"), None);
        assert_eq!(agent_capabilities(""), None);
    }

    /// The facts the write doors used to spell as agent names, pinned as the
    /// rows now carry them. Each list is the doors' former `match` arms.
    #[test]
    fn the_harness_facts_are_the_ones_the_doors_used_to_branch_on() {
        // The launch and resume doors spawned `zo` through the pane
        // supervisor and everyone else through a bare pty.
        assert_eq!(
            ids_where(|caps| caps.spawn == SpawnRoad::SocketPane),
            ["zo"]
        );
        // The worker split parked zo's briefing until its channel joined;
        // `AgentKind::reports_prompt_submit` named the hook agents.
        assert_eq!(
            ids_where(|caps| caps.submit_ack == SubmitAck::Channel),
            ["zo"]
        );
        assert_eq!(
            ids_where(|caps| caps.submit_ack == SubmitAck::Hook),
            [
                "claude", "codex", "devin", "cursor", "droid", "kimi", "copilot", "grok"
            ]
        );
        // `agent_trust_presets::preset_of`, by name.
        let menus: Vec<(&str, TrustMenu)> = AGENT_SPECS
            .iter()
            .filter_map(|spec| spec.capabilities().trust_menu().map(|menu| (spec.id, menu)))
            .collect();
        assert_eq!(
            menus,
            [
                ("zo", TrustMenu::Zo),
                ("claude", TrustMenu::Claude),
                ("codex", TrustMenu::Codex),
                ("antigravity", TrustMenu::Antigravity),
                ("cursor", TrustMenu::Cursor),
                ("copilot", TrustMenu::Copilot),
            ]
        );
        // The four-second quiet window's reason, said as a signal.
        assert_eq!(
            ids_where(|caps| caps.blocked.contains(&BlockedSignal::LoginGate)),
            ["antigravity"]
        );
        // The worker split offered the Codex app server by name.
        assert_eq!(
            ids_where(|caps| caps.pointer_route == Some(PointerRoute::CodexAppServer)),
            ["codex"]
        );
        // `claude_args_without_selectors` ran for `kind == AgentKind::Claude`.
        assert_eq!(
            ids_where(|caps| !caps.resume.selectors.is_empty()),
            ["claude"]
        );
        assert_eq!(
            agent_capabilities("claude")
                .expect("claude")
                .resume
                .selectors,
            CLAUDE_RESUME_SELECTORS
        );
        // The launch door's `resume` argument was Claude-only, its id a UUID.
        assert_eq!(ids_where(|caps| caps.resume.store.is_some()), ["claude"]);
        assert_eq!(
            agent_capabilities("claude").expect("claude").resume.store,
            Some(StoreResume {
                flag: "--resume",
                id: IdShape::Uuid,
            })
        );
        // `wake_interrupted` read the rollout for `agent == codex` alone.
        assert_eq!(
            ids_where(|caps| caps.resume.wake_mark == WakeMark::Rollout),
            ["codex"]
        );
        // `vault::resume_carries_launch_args`'s nine, in row order.
        assert_eq!(
            ids_where(|caps| caps.resume.vault_carries_launch_args),
            [
                "claude",
                "codex",
                "devin",
                "opencode",
                "pi",
                "omp",
                "antigravity",
                "droid",
                "grok"
            ]
        );
        // The login witnesses a readiness probe may ask (t-3996): the
        // providers the window materialises (`providers_for`) and the witness
        // kinds the row allows — the identity file and the scoped keychain
        // item for the Anthropic homes, the identity file alone for Codex,
        // and `gh auth status` for the one agent that rides gh's login.
        assert_eq!(
            ids_where(|caps| caps.auth_probe.is_some()),
            ["zo", "claude", "codex", "copilot"]
        );
        assert_eq!(
            agent_capabilities("zo").expect("zo").auth_probe,
            Some(AuthProbe {
                providers: &[Provider::Anthropic, Provider::OpenAi],
                probes: &[AuthProbeKind::IdentityFile, AuthProbeKind::Keychain],
            })
        );
        assert_eq!(
            agent_capabilities("codex").expect("codex").auth_probe,
            Some(AuthProbe {
                providers: &[Provider::OpenAi],
                probes: &[AuthProbeKind::IdentityFile],
            })
        );
        assert_eq!(
            agent_capabilities("copilot").expect("copilot").auth_probe,
            Some(AuthProbe {
                providers: &[],
                probes: &[AuthProbeKind::GhAuthStatus],
            })
        );
    }

    /// The submit road is the injection mode's spelling, and the launch words
    /// are built from the road — one place for the flag spellings.
    #[test]
    fn a_launch_prompt_is_spelled_by_the_submit_road() {
        for spec in &AGENT_SPECS {
            let caps = spec.capabilities();
            let expected = match spec.injection {
                Injection::Argv => Submit::Argv {
                    separator: spec.argv_separator,
                },
                Injection::FlagPrompt => Submit::Flag("--prompt"),
                Injection::FlagPromptInteractive => Submit::Flag("--prompt-interactive"),
                Injection::FlagInteractive => Submit::Flag("-i"),
                Injection::StdinAfterStart | Injection::HermesQuery => Submit::Typed,
            };
            assert_eq!(caps.submit, expected, "{}", spec.id);
            match (caps.submit, prompt_injection(spec, "일")) {
                (Submit::Argv { separator: None }, Some(Injected::Argv(words))) => {
                    assert_eq!(words, ["일"], "{}", spec.id);
                }
                (
                    Submit::Argv {
                        separator: Some(sep),
                    },
                    Some(Injected::Argv(words)),
                ) => {
                    assert_eq!(words, [sep, "일"], "{}", spec.id);
                }
                (Submit::Flag(flag), Some(Injected::Argv(words))) => {
                    assert_eq!(words, [flag, "일"], "{}", spec.id);
                }
                (Submit::Typed, Some(Injected::AfterStart(text))) => {
                    assert_eq!(text, "일", "{}", spec.id);
                }
                (road, words) => panic!("{} takes {road:?} but spelled {words:?}", spec.id),
            }
        }
        // The quick-command rule keeps its one distinction between the two
        // typed vendors: hermes has a query subcommand and is not refused.
        let refused = ids_where(|caps| !caps.takes_prompt_at_start());
        assert!(
            refused.contains(&"zo") && refused.contains(&"devin"),
            "{refused:?}"
        );
        assert!(!refused.contains(&"hermes"), "{refused:?}");
        assert_eq!(
            refused,
            ids_where(|caps| caps.injection == Injection::StdinAfterStart)
        );
    }

    /// Changing what one agent can do is one row edit, and every answer a
    /// door reads follows the row — nothing about the agent is decided by
    /// its name anywhere else.
    #[test]
    fn changing_one_agents_capability_is_one_row_edit() {
        let claude = agent_spec("claude").expect("in the registry");
        let before = claude.capabilities();
        assert_eq!(before.spawn, SpawnRoad::Pty);
        assert_eq!(before.startup.mark, ReadyMark::ComposerPrompt('\u{276f}'));
        assert_eq!(before.empty_composer_proof, EmptyComposerProof::DraftLedger);
        assert_eq!(before.trust_menu(), Some(TrustMenu::Claude));
        assert!(matches!(
            prompt_injection(claude, "고쳐"),
            Some(Injected::Argv(_))
        ));

        let edited = AgentSpec {
            injection: Injection::StdinAfterStart,
            composer_clear: ComposerClear::Keys,
            ready: ReadyMark::ComposerPrompt('\u{203a}'),
            harness: Harness {
                spawn: SpawnRoad::SocketPane,
                submit_ack: SubmitAck::Channel,
                blocked: &[BlockedSignal::TrustMenu(TrustMenu::Codex)],
                wake_mark: WakeMark::Rollout,
                ..claude.harness
            },
            ..*claude
        };
        let after = edited.capabilities();
        assert_eq!(after.submit, Submit::Typed);
        assert_eq!(
            prompt_injection(&edited, "고쳐"),
            Some(Injected::AfterStart("고쳐".to_string())),
            "the launch words follow the row, not the agent's name"
        );
        assert!(!after.takes_prompt_at_start());
        assert_eq!(after.spawn, SpawnRoad::SocketPane);
        assert_eq!(after.submit_ack, SubmitAck::Channel);
        assert_eq!(after.startup.mark, ReadyMark::ComposerPrompt('\u{203a}'));
        assert_eq!(after.steer.clear, ComposerClear::Keys);
        assert_eq!(after.empty_composer_proof, EmptyComposerProof::ClearKeys);
        assert_eq!(after.trust_menu(), Some(TrustMenu::Codex));
        assert_eq!(after.resume.wake_mark, WakeMark::Rollout);
        assert_eq!(
            after.blocked_signals(),
            [
                BlockedSignal::TrustMenu(TrustMenu::Codex),
                BlockedSignal::ChannelAsk,
                BlockedSignal::Silence,
            ]
        );
        // What the edit did not touch is what it was.
        assert_eq!(after.resume.selectors, before.resume.selectors);
        assert_eq!(after.resume.store, before.resume.store);
        assert_eq!(after.auth_probe, before.auth_probe);
    }

    #[test]
    fn blocked_signals_are_the_row_plus_what_its_transports_imply() {
        let signals = |id: &str| agent_capabilities(id).expect(id).blocked_signals();
        assert_eq!(
            signals("zo"),
            [
                BlockedSignal::TrustMenu(TrustMenu::Zo),
                BlockedSignal::ChannelAsk,
                BlockedSignal::Silence,
            ]
        );
        assert_eq!(
            signals("claude"),
            [
                BlockedSignal::TrustMenu(TrustMenu::Claude),
                BlockedSignal::HookAsk,
                BlockedSignal::Silence,
            ]
        );
        assert_eq!(
            signals("antigravity"),
            [
                BlockedSignal::TrustMenu(TrustMenu::Antigravity),
                BlockedSignal::LoginGate,
                BlockedSignal::Silence,
            ]
        );
        assert_eq!(signals("aider"), [BlockedSignal::Silence]);
        for spec in &AGENT_SPECS {
            assert_eq!(
                spec.capabilities().blocked_signals().last(),
                Some(&BlockedSignal::Silence),
                "{} lost the stall probe's floor",
                spec.id
            );
        }
    }

    /// The store id gate the launch road had for Claude, now the shape's own
    /// rule: a lowercase hex UUID and nothing that could be read as a flag.
    #[test]
    fn a_store_id_has_to_wear_the_shape_of_its_store() {
        assert!(IdShape::Uuid.accepts("4954d220-4dd9-43e5-b394-889b162931c8"));
        assert!(!IdShape::Uuid.accepts("4954D220-4dd9-43e5-b394-889b162931c8"));
        assert!(!IdShape::Uuid.accepts("4954d220-4dd9-43e5-b394-889b16293 c8"));
        assert!(!IdShape::Uuid.accepts("--resume"));
        assert!(!IdShape::Uuid.accepts(""));
    }

    /// The selector splice is the agent's own list, so an agent with none
    /// gets its line back untouched and Claude's four spellings all go.
    #[test]
    fn resume_selectors_are_spliced_by_the_rows_own_list() {
        let words =
            |line: &str| -> Vec<String> { line.split_whitespace().map(str::to_string).collect() };
        let claude = agent_capabilities("claude").expect("claude").resume;
        assert_eq!(
            claude.launch_args_without_selectors(&words("--resume abc --model opus")),
            ["--model", "opus"]
        );
        assert_eq!(
            claude.launch_args_without_selectors(&words("--resume --model opus")),
            ["--model", "opus"]
        );
        assert_eq!(
            claude.launch_args_without_selectors(&words("--continue -c -r --resume=abc --verbose")),
            ["--verbose"]
        );
        // A short flag never takes `=`: this is somebody else's argument.
        assert_eq!(
            claude.launch_args_without_selectors(&words("-r=abc")),
            ["-r=abc"]
        );
        let codex = agent_capabilities("codex").expect("codex").resume;
        let line = words("--resume abc --continue");
        assert_eq!(codex.launch_args_without_selectors(&line), line);
    }
}
