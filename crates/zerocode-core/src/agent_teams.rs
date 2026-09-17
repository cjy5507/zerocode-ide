//! An orchestrating agent's teammates, as panes of the pane it was started in.
//!
//! ## The reported break
//!
//! "에이전트들이 뜨면 실제로 터미널이 여러 개로 분할되고 그게 보여야 하는데
//! 안 되는 것 같다." Every by-hand road into a terminal already divides the
//! stage (`tilePlacement`), and a scheduled run already lands as a pane
//! nobody's hands were moved for. The road that did NOT exist is the one the
//! sentence is actually about: **an agent that starts other agents**. A
//! coordinator delegating to four teammates produced four sessions inside its
//! own process and no panes at all, because nothing in this window was
//! listening for "I am starting one more of me".
//!
//! ## What Orca actually does — measured
//!
//! Orca does not invent a protocol for this. Claude Code already ships a way
//! to run a team of agents (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`), and the
//! way it starts a teammate is to run **tmux**. So Orca puts a fake `tmux` on
//! that one launch's `PATH` and answers it
//! (`ClaudeAgentTeamsTmuxDispatcher`, out/main/index.js:173049-173258;
//! `ClaudeAgentTeamsService.createLaunchEnv`, :173262-173301). `split-window`
//! becomes a real pane split of the caller's own pane
//! (`splitPtyBackedTerminal`, :187911-187985, which reveals with
//! `splitFromLeafId` + `splitDirection` and `activate: opts.activate !== false`
//! — and `splitWindow` passes `activate: false`, :173129).
//!
//! Three measured consequences, and they are the whole design:
//!
//!   - **A teammate is a PANE of the leader's tab**, not a tab and not a
//!     hidden session. Orca's genuinely headless presentation
//!     (`presentation: "background"`) is reserved for remote/mobile callers;
//!     nothing local ever uses it.
//!   - **The leader keeps the keyboard.** Every teammate arrives with
//!     `activate: false`. Focus moves only when the leader asks for it by
//!     name (`select-pane`), which is [`Effect::Focus`] here.
//!   - **The layout is `main-vertical`.** Claude asks for it by name, and
//!     from then on a side-by-side split is redirected onto the last pane of
//!     the right-hand column ([`resolve_split_target`]) — so the leader keeps
//!     the main area and the teammates stack in one column beside it, rather
//!     than the leader being halved again for every teammate.
//!
//! ## Why the rules are here and the doing is not
//!
//! Everything in this module is a decision about text: which tmux verb was
//! asked for, which pane it names, which way the split goes, what the answer
//! reads like. None of it opens a pty. [`plan`] returns an [`Effect`] the
//! caller performs and a [`Reply`] the caller sends back, so the entire
//! protocol can be exercised in unit tests without a terminal — and so the
//! one place that decides "this becomes a split" cannot drift from the one
//! place that decides "and the leader keeps focus".
//!
//! ## The one refusal
//!
//! `c-c`, `c-d`, `c-z` and `bspace` map to **nothing**
//! (`tmuxSpecialKeyText`, :172972-172988). Orca drops them and so do we: a
//! coordinator that can address any pane by name must not be able to
//! interrupt, EOF or suspend a program a person is talking to. It is the same
//! rule as "a shell nobody is watching does not press Enter at a screen it did
//! not put there", arriving from the other direction — this time the typist is
//! another agent.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The leader's pane, in tmux's own spelling. Orca hard-codes `%1`
/// (:173265) and counts up from `2` for teammates.
pub const LEADER_PANE: &str = "%1";

/// What `tmux -V` answers. A version Claude accepts, and the one Orca claims
/// (:173053).
pub const TMUX_VERSION: &str = "tmux 3.4\n";

/// The session name Orca reports in every format string (:173289).
pub const SESSION_NAME: &str = "orca";

/// The window name (`window_name`, :173034). Ours by the repository rule —
/// the mechanism is measured, the words are our own.
pub const WINDOW_NAME: &str = "agent-teams";

/// Which way a split cuts. `Vertical` divides left|right, `Horizontal` top over
/// bottom — the window's spellings, which are `pane_layout`'s, which are
/// Orca's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Vertical,
    Horizontal,
}

impl Direction {
    /// The word the window reads off the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Direction::Vertical => "vertical",
            Direction::Horizontal => "horizontal",
        }
    }
}

/// One pane of a team, as the leader knows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamPane {
    /// `%1`, `%2`, … — the id handed to the agent as `TMUX_PANE`.
    pub id: String,
    /// The shell this pane is, in the window's own numbering. The leader never
    /// sees it.
    pub term: u32,
    /// Position in `paneOrder`, which is what `list-panes` walks.
    pub index: usize,
    /// The pane this one was cut out of, and which way. `respawn-pane` needs
    /// both: it re-cuts from the same source rather than from wherever the
    /// caller happens to be standing (:173161-173168).
    pub split_from: Option<String>,
    pub split_direction: Option<Direction>,
}

/// The `main-vertical` layout, once Claude has asked for it by name.
///
/// `main_pane` is the leader; `last_column_pane` is the bottom of the column
/// beside it, which is what the next side-by-side split actually cuts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainVertical {
    pub main_pane: String,
    pub last_column_pane: Option<String>,
}

/// One coordinator and the panes it has opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub leader_pane: String,
    /// The leader's own shell. Kept apart from `panes` lookups so a team can
    /// be found by the terminal that closed.
    pub leader_term: u32,
    panes: HashMap<String, TeamPane>,
    order: Vec<String>,
    next_pane_number: u32,
    main_vertical: Option<MainVertical>,
    previously_focused: Option<String>,
}

impl Team {
    /// A team whose only pane is the leader — the state
    /// `createLaunchEnv` leaves behind (:173282-173300).
    /// Construct planner state. The leader capability is accepted at this API
    /// boundary for launch-call compatibility but deliberately not retained:
    /// authentication state belongs to the shell's redacted pane registry,
    /// never to this Debug/serde planner type.
    pub fn new(
        id: impl Into<String>,
        _leader_capability: impl Into<String>,
        leader_term: u32,
    ) -> Self {
        let leader = TeamPane {
            id: LEADER_PANE.to_string(),
            term: leader_term,
            index: 0,
            split_from: None,
            split_direction: None,
        };
        Self {
            id: id.into(),
            leader_pane: LEADER_PANE.to_string(),
            leader_term,
            panes: HashMap::from([(LEADER_PANE.to_string(), leader)]),
            order: vec![LEADER_PANE.to_string()],
            next_pane_number: 2,
            main_vertical: None,
            previously_focused: None,
        }
    }

    pub fn pane(&self, id: &str) -> Option<&TeamPane> {
        self.panes.get(id)
    }

    /// Every pane, in `list-panes` order.
    pub fn panes(&self) -> impl Iterator<Item = &TeamPane> {
        self.order.iter().filter_map(|id| self.panes.get(id))
    }

    /// The shell behind one of this team's pane ids.
    pub fn term_of(&self, pane: &str) -> Option<u32> {
        self.panes.get(pane).map(|one| one.term)
    }

    /// Take the pane id a split will use, before the split happens.
    ///
    /// Handed out here rather than after the spawn for the same reason a pane
    /// key is: the teammate's own environment has to carry the id it will
    /// report under, and an id minted after the child exists is one the child
    /// never saw.
    fn take_pane_id(&mut self) -> String {
        let id = format!("%{}", self.next_pane_number);
        self.next_pane_number += 1;
        id
    }

    /// Write down a split that actually happened.
    ///
    /// Separate from [`plan`] because the shell has to spawn the child first —
    /// a split that failed to start must not leave a pane in the table that
    /// `list-panes` would then report and `send-keys` would then address.
    pub fn record_split(&mut self, pane_id: &str, term: u32, from: &str, direction: Direction) {
        let pane = TeamPane {
            id: pane_id.to_string(),
            term,
            index: self.order.len(),
            split_from: Some(from.to_string()),
            split_direction: Some(direction),
        };
        self.panes.insert(pane_id.to_string(), pane);
        self.order.push(pane_id.to_string());
        // `updateMainVerticalAfterSplit` (:173025-173032): once a column
        // exists, every later teammate joins the bottom of it; and a
        // side-by-side split of the LEADER is what creates the column in the
        // first place, even if nobody named the layout.
        if let Some(main) = self.main_vertical.as_mut() {
            main.last_column_pane = Some(pane_id.to_string());
        } else if direction == Direction::Vertical && from == self.leader_pane {
            self.main_vertical = Some(MainVertical {
                main_pane: self.leader_pane.clone(),
                last_column_pane: Some(pane_id.to_string()),
            });
        }
    }

    /// Put a fresh shell behind a pane that keeps its name.
    ///
    /// `respawn-pane`'s other half. The id, the index and the place in
    /// `list-panes` all stay; only the terminal changes, which is why this is
    /// not remove-then-record — a teammate that restarted would otherwise
    /// walk to the end of the leader's list and lose the name it was told.
    pub fn respawn_pane(&mut self, pane_id: &str, term: u32) {
        if let Some(pane) = self.panes.get_mut(pane_id) {
            pane.term = term;
        }
    }

    /// Take a pane out, the way `kill-pane` does (:173230-173237).
    pub fn remove_pane(&mut self, pane_id: &str) {
        self.panes.remove(pane_id);
        self.order.retain(|held| held != pane_id);
        if let Some(main) = self.main_vertical.as_mut()
            && main.last_column_pane.as_deref() == Some(pane_id)
        {
            // The column's new bottom is the last pane that is not the leader
            // — measured, and the reason it is not simply "the last pane" is
            // that a team down to the leader alone has no column left.
            main.last_column_pane = self
                .order
                .iter()
                .rev()
                .find(|held| *held != &self.leader_pane)
                .cloned();
        }
    }
}

/// A terminal and its unlogged launch capability, captured under the pane table.
#[derive(Clone, PartialEq, Eq)]
pub struct PaneIncarnation {
    pub term: u32,
    pub capability: String,
}

impl std::fmt::Debug for PaneIncarnation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneIncarnation")
            .field("term", &self.term)
            .finish_non_exhaustive()
    }
}

/// What the shell has to actually do, once [`plan`] has decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Nothing but the reply. Every tmux verb Orca answers without touching a
    /// terminal lands here.
    None,
    /// Cut `from` and start `command` in the new half. `pane` is already
    /// reserved: the child's environment carries it.
    Split {
        pane: String,
        from: String,
        direction: Direction,
        /// The teammate's command line, as tmux was given it. Empty means "a
        /// shell", which is tmux's own default for a bare `split-window`.
        command: String,
        /// Which helper this pane IS, when the split said: the value of a
        /// `-e ZO_AGENT_ID=<id>` pair on the argv — zo's pane lane names its
        /// child by the same id its `SubagentStart` hook already announced.
        /// The window folds the hook's helper row and this pane's row into
        /// one by it (t-3024). `None` for every split that carries no such
        /// pair: Claude's teammates, ledger workers, a bare shell.
        helper: Option<String>,
    },
    /// Read the screen of a pane the LEDGER knows by seat — `team/pane` —
    /// rather than by a terminal in the caller's own table.
    ///
    /// `worker-read` asks this for a worker seated in another leader's team:
    /// an orphan whose leader exited, a worker of the coordinator the caller
    /// took the seat over from. Planning `capture-pane` against the caller's
    /// table answered `unknown pane` for every one of them (2026-09-05), and
    /// the ledger cannot resolve the terminal itself — only the window holds
    /// the other team's table — so the seat travels and the window resolves
    /// it, refusing "worker is gone" when no table maps it (t-2512).
    CaptureSeat {
        team: String,
        pane: String,
        lines: usize,
    },
    /// Retire the ledger worker's own seat, resolved and fenced by the host.
    /// Lifecycle settlement waits for that proof; pane numbers are team-local.
    WorkerTerminal {
        seat: crate::orchestration::WorkerSeat,
        incarnation: Option<PaneIncarnation>,
        handover: Option<Box<crate::orchestration::HandoverPlan>>,
        /// Some reason stops an attempt; None archives a completed worker.
        stop: Option<String>,
        lines: usize,
    },
    /// Type at one pane. Already translated out of tmux's key names.
    Send { term: u32, text: String },
    /// Paste prose at one pane and submit it.
    ///
    /// The text is a DOCUMENT — a task's own spec, words nobody vetted for
    /// key grammar — where [`Effect::Send`] carries keys. The split matters
    /// because the two are armed differently: keys pass through untouched,
    /// prose has every escape made inert before it goes anywhere near a
    /// terminal, and the bracketed envelope is the window's to wrap, because
    /// only the pane's own grid knows whether the program asked for it.
    Paste { term: u32, text: String },
    /// Restart one teammate: cut `from` again the way it was cut before, then
    /// end the shell that was there. In that order — a pane closed before its
    /// replacement exists is a hole in the layout for however long the spawn
    /// takes, and a spawn that fails leaves the leader with neither.
    Respawn {
        /// The pane id being reused. The leader keeps addressing it by this
        /// name, so the id survives the restart even though the shell does not
        /// — measured (`pane.handle = split.handle`, :173176, with the fake
        /// pane id untouched).
        pane: String,
        from: String,
        direction: Direction,
        command: String,
        /// The shell to end once the replacement is up.
        term: u32,
    },
    /// Read one pane's tail back to the leader.
    Capture { term: u32, lines: usize },
    /// Gather what a CHECKOUT can prove about itself — its Git changes, the
    /// receipts and decisions stored against it — and answer that.
    ///
    /// The ledger knows which checkout to ask about and cannot read one: Git
    /// and the authority store are the window's, not the ledger's. So the
    /// decision names the checkout and the window does the reading, the same
    /// division [`Effect::CaptureSeat`] keeps for a screen.
    ///
    /// `checkout` is the ledger's own word for a named worker's tree. `None`
    /// means "the tree the asking pane is sitting in", which only the window
    /// knows — a coordinator's pane is nobody's worker row.
    WorktreeEvidence { checkout: Option<String> },
    /// Move the keyboard. The ONE road by which a teammate pane takes focus.
    Focus { term: u32 },
    /// End one pane.
    Close { pane: String, term: u32 },
}

/// What the shim prints, and with what status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl Reply {
    pub fn ok(stdout: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: 0,
        }
    }

    /// Orca's failure shape (`handleTmuxCompat`, :173321-173328): the message
    /// on stderr behind a `tmux: ` prefix, and a non-zero status, so the agent
    /// sees a tmux that said no rather than a tmux that vanished.
    pub fn refused(why: impl std::fmt::Display) -> Self {
        Self {
            stdout: String::new(),
            stderr: format!("tmux: {why}\n"),
            exit_code: 1,
        }
    }
}

/// A decided request: what to do, and what to say once it is done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub effect: Effect,
    /// The answer, for the effects that already know it — which is most of
    /// them, including `Split`: the new pane's id is minted before the split
    /// happens, precisely so the reply can be written without waiting for a
    /// process to exist.
    ///
    /// `Capture` is the one exception, because its text IS the screen. It
    /// leaves a placeholder here that [`capture_reply`] fills in, and the
    /// placeholder is a NUL so a reply nobody finished is visible rather than
    /// silently empty.
    pub reply: Reply,
}

/// How many lines `capture-pane` hands back. Orca's own limit
/// (`readTerminal(pane.handle, { limit: 1e3 })`, :173219).
pub const CAPTURE_LINES: usize = 1000;

/* ---- the argument grammar ----
 *
 * tmux's own, which is not getopt's: single-letter flags cluster, a value flag
 * takes the rest of its cluster or the next word, `--` ends the flags, and
 * anything unrecognised is positional rather than an error. Ported from
 * `parseTmuxArgs` (:172892-172941) because a shim that mis-parses
 * `split-window -dh -t %1 claude …` starts the teammate with the wrong flags
 * in its command line. */

/// One parsed command line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    flags: Vec<String>,
    values: Vec<(String, String)>,
    /// Everything that was not a flag, in order — the command for
    /// `split-window`, the keys for `send-keys`, the layout for
    /// `select-layout`.
    pub positional: Vec<String>,
}

impl Parsed {
    pub fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|held| held == flag)
    }

    /// The LAST value given for a flag — `tmuxValue`'s `.at(-1)` (:172943).
    /// A caller that says `-t` twice means the second one.
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.values
            .iter()
            .rev()
            .find(|(name, _)| name == flag)
            .map(|(_, value)| value.as_str())
    }
}

/// The environment pair that names WHICH helper a split's pane is.
///
/// zo's pane lane cuts a helper's pane with `-e ZO_AGENT_ID=<id>`, the id its
/// `SubagentStart` hook already announced to the window; the window folds the
/// hook's row and the pane's row into one by it (t-3024). One spelling here
/// and one in zo (`runtime::subagent_panes::ZO_AGENT_ID_VAR`), pinned by the
/// plan test on the argv zo actually types.
pub const HELPER_ENV_VAR: &str = "ZO_AGENT_ID";

/// The helper a `split-window` names, if it names one.
///
/// Only the identity pair is read; every other `-e` pair is tmux's
/// environment and stays exactly that — neither a helper nor a word of the
/// command. An empty value is no identity.
fn split_helper(parsed: &Parsed) -> Option<String> {
    parsed
        .values
        .iter()
        .filter(|(flag, _)| flag == "-e")
        .filter_map(|(_, pair)| pair.strip_prefix(HELPER_ENV_VAR))
        .filter_map(|rest| rest.strip_prefix('='))
        .map(str::trim)
        .find(|id| !id.is_empty())
        .map(str::to_string)
}

/// Split a command line into flags, values and positionals.
pub fn parse_args(args: &[String], value_flags: &[&str], bool_flags: &[&str]) -> Parsed {
    let mut parsed = Parsed::default();
    let mut past_terminator = false;
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if past_terminator {
            parsed.positional.push(arg.to_string());
            index += 1;
            continue;
        }
        if arg == "--" {
            past_terminator = true;
            index += 1;
            continue;
        }
        if !arg.starts_with('-') || arg == "-" || arg.starts_with("--") {
            parsed.positional.push(arg.to_string());
            index += 1;
            continue;
        }
        let cluster: Vec<char> = arg.chars().skip(1).collect();
        let mut cursor = 0;
        let mut recognized = false;
        while cursor < cluster.len() {
            let flag = format!("-{}", cluster[cursor]);
            if bool_flags.contains(&flag.as_str()) {
                if !parsed.has(&flag) {
                    parsed.flags.push(flag);
                }
                cursor += 1;
                recognized = true;
                continue;
            }
            if value_flags.contains(&flag.as_str()) {
                // The rest of the cluster, or the next word if the cluster
                // ended with the flag itself.
                let rest: String = cluster[cursor + 1..].iter().collect();
                let value = if rest.is_empty() {
                    index += 1;
                    args.get(index).cloned().unwrap_or_default()
                } else {
                    rest
                };
                parsed.values.push((flag, value));
                recognized = true;
                cursor = cluster.len();
                continue;
            }
            recognized = false;
            break;
        }
        if !recognized {
            parsed.positional.push(arg.to_string());
        }
        index += 1;
    }
    parsed
}

/// Take the verb off a tmux command line, skipping the global flags that come
/// before it (`splitTmuxCommand`, :172870-172891).
///
/// `-V`/`-v` ARE the whole command; `-L`/`-S`/`-f` each swallow the word after
/// them. A line with no verb at all is refused rather than guessed at.
pub fn split_command(argv: &[String]) -> Result<(String, Vec<String>), String> {
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            break;
        }
        if !arg.starts_with('-') || arg == "-" {
            return Ok((arg.to_lowercase(), argv[index + 1..].to_vec()));
        }
        if matches!(arg, "-V" | "-v") {
            return Ok((arg.to_string(), Vec::new()));
        }
        if matches!(arg, "-L" | "-S" | "-f") {
            index += 1;
        }
        index += 1;
    }
    Err("tmux shim requires a command".to_string())
}

/// The facts a format string can name, for one pane.
fn format_context(team: &Team, pane: &TeamPane) -> Vec<(&'static str, String)> {
    vec![
        ("session_name", SESSION_NAME.to_string()),
        ("session_id", "$0".to_string()),
        ("window_id", "@0".to_string()),
        ("window_index", "0".to_string()),
        ("window_name", WINDOW_NAME.to_string()),
        ("window_active", "1".to_string()),
        ("window_flags", "*".to_string()),
        ("pane_id", pane.id.clone()),
        ("pane_index", pane.index.to_string()),
        (
            "pane_active",
            if pane.id == team.leader_pane {
                "1"
            } else {
                "0"
            }
            .to_string(),
        ),
        ("pane_title", String::new()),
        ("pane_width", String::new()),
        ("pane_height", String::new()),
        ("pane_left", String::new()),
        ("pane_top", String::new()),
        ("window_width", String::new()),
        ("window_height", String::new()),
    ]
}

/// Fill `#{name}` holes, drop the ones nothing answered, and fall back when
/// what is left is nothing (`renderTmuxFormat`, :172945-172951).
///
/// The fallback matters more than it looks: `split-window -P` with no `-F`
/// must still print the new pane's id, because that string is how the leader
/// learns what to address next.
pub fn render_format(
    format: Option<&str>,
    context: &[(&'static str, String)],
    fallback: &str,
) -> String {
    let Some(format) = format.filter(|one| !one.is_empty()) else {
        return fallback.to_string();
    };
    let mut rendered = format.to_string();
    for (key, value) in context {
        rendered = rendered.replace(&format!("#{{{key}}}"), value);
    }
    rendered = drop_unfilled(&rendered).trim().to_string();
    if rendered.is_empty() {
        fallback.to_string()
    } else {
        rendered
    }
}

/// `/#\{[^}]+\}/g` without a regex crate: every `#{…}` that nothing filled.
fn drop_unfilled(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes: Vec<char> = value.chars().collect();
    let mut index = 0;
    while index < bytes.len() {
        // `[^}]+` — at least one character inside, so a bare `#{}` is text.
        if bytes[index] == '#'
            && bytes.get(index + 1) == Some(&'{')
            && let Some(end) = bytes[index + 2..].iter().position(|one| *one == '}')
            && end > 0
        {
            index += 2 + end + 1;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

/// One tmux key name as the bytes it means, or `None` for a plain word.
///
/// The four that answer with nothing are the refusal this module exists to
/// keep: `c-c`, `c-d`, `c-z` interrupt, end and suspend whatever a pane is
/// running, and `bspace` edits a line somebody else is typing.
pub fn special_key(token: &str) -> Option<&'static str> {
    match token.to_lowercase().as_str() {
        "enter" | "c-m" | "kpenter" => Some("\r"),
        "tab" | "c-i" => Some("\t"),
        "space" => Some(" "),
        "bspace" | "backspace" => Some(""),
        "escape" | "esc" | "c-[" => Some("\u{1b}"),
        "c-c" | "c-d" | "c-z" => Some(""),
        "c-l" => Some("\u{c}"),
        _ => None,
    }
}

/// What `send-keys` actually types (`tmuxSendKeysText`, :172952-172971).
///
/// `-l` is literal: the words go through joined by single spaces and nothing
/// is a key name. Without it, a key name types its bytes and **cancels the
/// space that would have followed the word before it** — which is why
/// `send-keys hello Enter` sends `hello\r` and not `hello \r`.
pub fn send_keys_text(tokens: &[String], literal: bool) -> String {
    if literal {
        return tokens.join(" ");
    }
    let mut out = String::new();
    let mut pending_space = false;
    for token in tokens {
        if let Some(special) = special_key(token) {
            out.push_str(special);
            pending_space = false;
            continue;
        }
        if pending_space {
            out.push(' ');
        }
        out.push_str(token);
        pending_space = true;
    }
    out
}

/// Which pane a split actually cuts, and which way
/// (`resolveSplitTarget`, :173011-173023).
///
/// `horizontal` here is tmux's `-h`, which means "side by side" — the opposite
/// of what this codebase's [`Direction`] calls horizontal, and the reason the
/// translation lives in exactly one function. Under `main-vertical` a
/// side-by-side ask is redirected onto the bottom of the column and cut the
/// OTHER way, which is what keeps the leader's pane whole while teammates
/// stack beside it.
pub fn resolve_split_target(team: &Team, target: &str, horizontal: bool) -> (String, Direction) {
    if horizontal
        && let Some(main) = &team.main_vertical
        && let Some(last) = &main.last_column_pane
        && team.panes.contains_key(last)
    {
        return (last.clone(), Direction::Horizontal);
    }
    (
        target.to_string(),
        if horizontal {
            Direction::Vertical
        } else {
            Direction::Horizontal
        },
    )
}

/// Decide one tmux request.
///
/// `env_pane` is the caller's own pane, out of its `TMUX_PANE` — the default
/// target for every verb that takes `-t`. A request naming a pane this team
/// does not hold is refused, which is what stops one team's leader from
/// reaching into another's.
pub fn plan(team: &mut Team, argv: &[String], env_pane: &str) -> Planned {
    match plan_inner(team, argv, env_pane) {
        Ok(planned) => planned,
        Err(why) => Planned {
            effect: Effect::None,
            reply: Reply::refused(why),
        },
    }
}

fn resolve_pane(team: &Team, target: &str) -> Result<TeamPane, String> {
    team.panes
        .get(target)
        .cloned()
        .ok_or_else(|| format!("unknown pane: {target}"))
}

/// A `-t` that names the window or the session rather than a pane. Orca's
/// `resolvePaneOrWindow` (:173240-173246): anything with a colon, the session
/// name, or an `@window` id.
fn names_window(target: &str) -> bool {
    target.contains(':') || target == SESSION_NAME || target.starts_with('@')
}

fn plan_inner(team: &mut Team, argv: &[String], env_pane: &str) -> Result<Planned, String> {
    let (command, args) = split_command(argv)?;
    let said = |stdout: &str| {
        Ok(Planned {
            effect: Effect::None,
            reply: Reply::ok(stdout),
        })
    };
    match command.as_str() {
        "-v" | "-V" => said(TMUX_VERSION),
        // `show-options extended-keys` is the one option Claude asks about,
        // and the only one Orca answers (:173067-173074). Anything else is a
        // refusal rather than a made-up value.
        "show-options" | "show-option" | "show" => {
            let parsed = parse_args(&args, &["-t"], &["-g", "-q", "-s", "-v", "-w"]);
            let option = parsed.positional.last().map(String::as_str).unwrap_or("");
            if option != "extended-keys" {
                return Err(format!("unsupported option: {option}"));
            }
            said(if parsed.has("-v") {
                "on\n"
            } else {
                "extended-keys on\n"
            })
        }
        "display-message" | "display" | "displayp" => {
            let parsed = parse_args(&args, &["-F", "-t"], &["-p"]);
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            // A window target answers about the CALLER, not about a pane
            // nobody named (:173077-173081).
            let pane = if names_window(&target) {
                resolve_pane(team, env_pane)?
            } else {
                resolve_pane(team, &target)?
            };
            let format = if parsed.positional.is_empty() {
                parsed.value("-F").map(str::to_string)
            } else {
                Some(parsed.positional.join(" "))
            };
            let context = format_context(team, &pane);
            said(&format!(
                "{}\n",
                render_format(format.as_deref(), &context, "")
            ))
        }
        "split-window" | "splitw" => {
            // `-e` takes a value (an environment pair) in tmux's grammar. Read
            // as a value flag so the pair never lands in the command; the one
            // pair the window acts on is the helper's identity.
            let parsed = parse_args(
                &args,
                &["-c", "-e", "-F", "-l", "-t"],
                &["-P", "-b", "-d", "-f", "-h", "-v"],
            );
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            let target_pane = resolve_pane(team, &target)?;
            let (from, direction) = resolve_split_target(team, &target_pane.id, parsed.has("-h"));
            let pane_id = team.take_pane_id();
            // The reply is written now, off the id the split will carry: the
            // leaf does not exist yet, and every field a format can name is
            // already known except the ones Orca leaves blank anyway.
            let reply = if parsed.has("-P") {
                let pane = TeamPane {
                    id: pane_id.clone(),
                    term: 0,
                    index: team.order.len(),
                    split_from: Some(from.clone()),
                    split_direction: Some(direction),
                };
                let context = format_context(team, &pane);
                Reply::ok(format!(
                    "{}\n",
                    render_format(parsed.value("-F"), &context, &pane_id)
                ))
            } else {
                Reply::ok("")
            };
            Ok(Planned {
                effect: Effect::Split {
                    pane: pane_id,
                    from,
                    direction,
                    command: parsed.positional.join(" "),
                    helper: split_helper(&parsed),
                },
                reply,
            })
        }
        "respawn-pane" | "respawnp" => {
            let parsed = parse_args(&args, &["-c", "-e", "-t"], &["-k"]);
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            let pane = resolve_pane(team, &target)?;
            if pane.id == team.leader_pane {
                return Err("refusing to respawn leader pane".to_string());
            }
            let command = parsed.positional.join(" ");
            if command.is_empty() {
                return said("");
            }
            // Re-cut from where this pane came from, the way it was cut
            // (:173161-173168) — a respawn that split whatever the caller
            // happened to be standing in would walk the layout sideways every
            // time a teammate restarted.
            let from = pane
                .split_from
                .clone()
                .filter(|one| team.panes.contains_key(one))
                .unwrap_or_else(|| team.leader_pane.clone());
            let direction = pane.split_direction.unwrap_or(Direction::Horizontal);
            Ok(Planned {
                effect: Effect::Respawn {
                    pane: pane.id.clone(),
                    from,
                    direction,
                    command,
                    term: pane.term,
                },
                reply: Reply::ok(""),
            })
        }
        "select-layout" => {
            let parsed = parse_args(&args, &["-t"], &[]);
            let layout = parsed.positional.first().map(String::as_str).unwrap_or("");
            if layout == "main-vertical" {
                let target = parsed.value("-t").unwrap_or(env_pane).to_string();
                let named = if names_window(&target) {
                    None
                } else {
                    resolve_pane(team, &target).ok().map(|pane| pane.id)
                };
                let already = team
                    .main_vertical
                    .as_ref()
                    .and_then(|main| main.last_column_pane.clone());
                team.main_vertical = Some(MainVertical {
                    main_pane: team.leader_pane.clone(),
                    last_column_pane: already
                        .or_else(|| named.filter(|one| one != &team.leader_pane)),
                });
            } else if !layout.is_empty() {
                team.main_vertical = None;
            }
            said("")
        }
        // A pane's size is the window's business, and the window sizes panes
        // evenly. Answered rather than refused, because a leader that cannot
        // resize must still be able to lay out (:173059).
        "resize-pane" | "resizep" => said(""),
        "list-panes" | "lsp" => {
            let parsed = parse_args(&args, &["-F", "-t"], &[]);
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            if !names_window(&target) {
                resolve_pane(team, &target)?;
            }
            let format = parsed.value("-F");
            let mut lines: Vec<String> = Vec::new();
            for pane in team.panes() {
                let context = format_context(team, pane);
                lines.push(render_format(format, &context, &pane.id));
            }
            said(&format!("{}\n", lines.join("\n")))
        }
        "send-keys" | "send" => {
            let parsed = parse_args(&args, &["-t"], &["-l"]);
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            let pane = resolve_pane(team, &target)?;
            let text = send_keys_text(&parsed.positional, parsed.has("-l"));
            Ok(Planned {
                effect: if text.is_empty() {
                    Effect::None
                } else {
                    Effect::Send {
                        term: pane.term,
                        text,
                    }
                },
                reply: Reply::ok(""),
            })
        }
        "capture-pane" | "capturep" => {
            let parsed = parse_args(&args, &["-E", "-S", "-t"], &["-J", "-N", "-p"]);
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            let pane = resolve_pane(team, &target)?;
            Ok(Planned {
                effect: Effect::Capture {
                    term: pane.term,
                    lines: CAPTURE_LINES,
                },
                // `-p` is "print it"; without it tmux would have put the text
                // in a buffer, and a buffer nobody can read is an empty answer
                // rather than a refusal (:173222).
                reply: Reply::ok(if parsed.has("-p") { "\u{0}" } else { "" }),
            })
        }
        "select-pane" | "selectp" => {
            let parsed = parse_args(&args, &["-P", "-T", "-t"], &[]);
            // `-P`/`-T` colour and title a pane. Nothing here draws either,
            // and answering "" is what keeps a leader that decorates its panes
            // from failing (:173226).
            if parsed.value("-P").is_some() || parsed.value("-T").is_some() {
                return said("");
            }
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            let pane = resolve_pane(team, &target)?;
            team.previously_focused = Some(env_pane.to_string());
            Ok(Planned {
                effect: Effect::Focus { term: pane.term },
                reply: Reply::ok(""),
            })
        }
        "kill-pane" | "killp" => {
            let parsed = parse_args(&args, &["-t"], &[]);
            let target = parsed.value("-t").unwrap_or(env_pane).to_string();
            let pane = resolve_pane(team, &target)?;
            if pane.id == team.leader_pane {
                return Err("refusing to kill leader pane".to_string());
            }
            Ok(Planned {
                effect: Effect::Close {
                    pane: pane.id.clone(),
                    term: pane.term,
                },
                reply: Reply::ok(""),
            })
        }
        "last-pane" => {
            let previous = team
                .previously_focused
                .as_ref()
                .and_then(|id| team.panes.get(id))
                .cloned();
            Ok(Planned {
                effect: match previous {
                    Some(pane) => Effect::Focus { term: pane.term },
                    None => Effect::None,
                },
                reply: Reply::ok(""),
            })
        }
        // The verbs a tmux client says on its way in and out. Answered with
        // nothing on purpose: they are about a server we do not have, and a
        // refusal here reads to Claude as a broken tmux (:173087-173098).
        "set-option" | "set" | "set-window-option" | "setw" | "set-hook" | "refresh-client"
        | "attach-session" | "detach-client" | "source-file" | "wait-for" | "has-session"
        | "has" => said(""),
        other => Err(format!("unsupported command: {other}")),
    }
}

/* ---- the launch ----
 *
 * What has to be true of the LEADER's command line and environment before any
 * of the above can happen. Measured off `buildClaudeAgentTeamsLaunchPlan`
 * (:173348-173363) and `createLaunchEnv` (:173262-173301). */

/// Whether a person has asked for this at all.
///
/// Orca ships it OFF (`claudeAgentTeamsMode: "off"`, :3999, and
/// `claude-agent-teams` is added to the disabled list on migration, :15570) —
/// a coordinator that starts four processes is not something to turn on behind
/// somebody's back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TeamsMode {
    /// No teams. The agent runs exactly as it did before this module existed.
    Off,
    /// Teammates inside the leader's own process — Claude's own fallback, and
    /// what Orca uses where a pane shim cannot be put on `PATH`.
    /// No panes are made, which is honest rather than broken.
    InProcess,
    /// Teammates are panes. Orca ships this OFF by default; the person asked
    /// for the split by name, twice — "소넷과 codex 다 소환하고 … 화면 분활이
    /// 이루어저야" — and a white label answers to its owner before its
    /// original. The settings picker still turns it off.
    #[default]
    Panes,
}

/// Is this command line one we can safely append a flag to?
///
/// `isDirectClaudeCommand` (:172989-172995). A command carrying a shell
/// metacharacter is not one word we can rewrite — inserting a flag into
/// `claude | tee log` puts it in the wrong place — and a command that is not
/// claude has no such flag at all.
pub fn is_direct_claude_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains([';', '&', '|', '<', '>', '`']) {
        return false;
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    first == "claude" || first.ends_with("/claude")
}

/// The flag that tells Claude how to run its team, added only if the person
/// has not already said (`addClaudeTeammateMode*`, :172997-173003).
pub fn with_teammate_mode(command: &str, mode: TeamsMode) -> String {
    let said = match mode {
        TeamsMode::Off => return command.to_string(),
        TeamsMode::InProcess => "in-process",
        TeamsMode::Panes => "auto",
    };
    // Already answered — by the person, in the args field. Theirs wins.
    if command
        .split_whitespace()
        .any(|word| word == "--teammate-mode" || word.starts_with("--teammate-mode="))
    {
        return command.to_string();
    }
    let mut words = command.trim().splitn(2, char::is_whitespace);
    let program = words.next().unwrap_or("");
    match words.next() {
        Some(rest) => format!("{program} --teammate-mode {said} {rest}"),
        None => format!("{program} --teammate-mode {said}"),
    }
}

/// The same answer for a command that is already a word list.
///
/// [`with_teammate_mode`] is the measured shape and works on the string Orca
/// holds; a launch here holds an argv, and rebuilding a string out of it to
/// re-split would break the first prompt containing a space. The words to
/// insert **after the program** — empty when the mode is off, when the agent
/// has already been told, or when the program is not a claude we can add a
/// flag to.
pub fn teammate_mode_args(argv: &[String], mode: TeamsMode) -> Vec<String> {
    let said = match mode {
        TeamsMode::Off => return Vec::new(),
        TeamsMode::InProcess => "in-process",
        TeamsMode::Panes => "auto",
    };
    let Some(program) = argv.first() else {
        return Vec::new();
    };
    if !is_direct_claude_command(program) {
        return Vec::new();
    }
    if argv
        .iter()
        .any(|word| word == "--teammate-mode" || word.starts_with("--teammate-mode="))
    {
        return Vec::new();
    }
    vec!["--teammate-mode".to_string(), said.to_string()]
}

/// The environment one leader launch carries.
///
/// `shim_dir` goes on the FRONT of `PATH` so the fake `tmux` is the one found;
/// `TMUX` is what makes Claude believe it is already inside a session, and
/// `TMUX_PANE` is the leader's own address. `TERM` stays `xterm-256color` —
/// the dialect our grid actually reads — NOT Orca's `screen-256color`, which
/// only its xterm.js renderer can afford to claim (1-fr: claiming it here
/// underlined every glyph on screen).
///
/// Returned as pairs rather than applied here: the launch assembles one
/// environment from several contributors in a measured order, and this is one
/// contributor.
pub fn team_launch_env(
    team_id: &str,
    token: &str,
    shim_dirs: &[&str],
    path: &str,
    colorterm: &str,
    program: &str,
) -> Vec<(String, String)> {
    let joined = shim_path(shim_dirs, path);
    let mut said: Vec<(String, String)> = Vec::new();
    if Dialect::of(program) == Dialect::Tmux {
        // Claude's Agent Teams is behind an experimental switch, and the
        // switch is CLAUDE's — not the road's. A zo leader reaches the same
        // panes without it, and setting it in a zo pane would arm Agent Teams
        // inside any `claude` that pane's own tools happen to run: a team
        // nobody asked for, cut out of a leader that is not claude.
        if is_direct_claude_command(program) {
            said.push(("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".into(), "1".into()));
        }
        // The two that make a leader look for a multiplexer. An agent that
        // never asked to be inside one is not told it is: `TMUX` set for a
        // codex leader would have every tool in that pane — a shell prompt, a
        // pager, an editor — believing it is running under tmux, and behaving
        // differently for a multiplexer that is not there.
        said.push((
            "TMUX".into(),
            format!("/tmp/zerocode-agent-teams/{team_id},0,1"),
        ));
        said.push(("TMUX_PANE".into(), LEADER_PANE.into()));
    }
    said.extend(vec![
        ("PATH".into(), joined),
        // Which pane this shell is, said in a name that is ours. `TMUX_PANE`
        // carries the same answer for a tmux team and is read as a fallback,
        // but a ledger team has no `TMUX_PANE` to read — and inventing one
        // would be inventing the multiplexer along with it.
        (TEAM_PANE_VAR.into(), LEADER_PANE.into()),
        // Orca ships `TERM=screen-256color` here because its renderer is
        // xterm.js, which reads every dialect a client could pick from that
        // name. OUR grid is the terminal, and it speaks xterm's dialect — a
        // claude told it is on `screen` emits for a terminal we are not, and
        // the difference arrived as every glyph underlined ("화면이
        // 깨지는데"). `TERM` names what the READER really is; the fake tmux
        // is detected through `TMUX`, not through this.
        ("TERM".into(), "xterm-256color".into()),
        (
            "COLORTERM".into(),
            if colorterm.is_empty() {
                "truecolor".into()
            } else {
                colorterm.to_string()
            },
        ),
        (TEAM_ID_VAR.into(), team_id.into()),
        (TEAM_TOKEN_VAR.into(), token.into()),
        (TEAM_LEADER_VAR.into(), LEADER_PANE.into()),
    ]);
    said
}

/// Which dialect a leader's team speaks.
///
/// Not a preference — a fact about the agent. TWO programs start a teammate by
/// running `tmux split-window` and reading the pane id out of the answer:
/// Claude's Agent Teams, and zo's own `Agent` tool in panes mode
/// (`runtime::subagent_panes`). Both are handed the fake one. Everyone else
/// reaches the same panes through the ledger, and handing them a fake `tmux`
/// would put a lie in front of any real one they might run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Tmux,
    Ledger,
}

impl Dialect {
    /// Which dialect this program speaks.
    pub fn of(program: &str) -> Self {
        if is_direct_claude_command(program) || is_direct_zo_command(program) {
            Self::Tmux
        } else {
            Self::Ledger
        }
    }
}

/// Is this command line a zo we can hand a team to?
///
/// The same question [`is_direct_claude_command`] asks, and the same two
/// refusals: a line carrying a shell metacharacter is not one word (the pane
/// a teammate cuts would be the wrong one), and a program merely STARTING
/// with `zo` — `zoom`, `zoxide` — is somebody else's.
pub fn is_direct_zo_command(command: &str) -> bool {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.contains([';', '&', '|', '<', '>', '`']) {
        return false;
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    first == "zo" || first.ends_with("/zo")
}

/// What `PATH` is separated by here.
///
/// Written down rather than assumed: a `:` on Windows joins two directories
/// into one nonexistent path, and the shim that was supposed to be in front
/// would simply not be found — a feature that silently does nothing, which is
/// the worst way for a platform difference to arrive.
pub const PATH_SEPARATOR: char = if cfg!(windows) { ';' } else { ':' };

/// A `PATH` with the shim directories in FRONT of whatever was already there.
///
/// One function because it is one rule, and the second caller is the reason it
/// exists: a summoned worker's environment is assembled by a different road
/// than its leader's, and that road was ending with a `PATH` that had lost the
/// shims entirely. Writing the join a second time there would be writing the
/// separator decision a second time too.
///
/// An empty base is a base, not a bug — a leader launched from a window that
/// never hydrated a shell PATH still has to find its own shim.
pub fn shim_path(shim_dirs: &[&str], base: &str) -> String {
    let mut ahead: Vec<&str> = shim_dirs.to_vec();
    if !base.is_empty() {
        ahead.push(base);
    }
    ahead.join(&PATH_SEPARATOR.to_string())
}

/// Which pane a shell is, in a name that does not claim a multiplexer.
pub const TEAM_PANE_VAR: &str = "ZEROCODE_AGENT_TEAM_PANE";

/// The variables the shim reads back out of its own environment. Namespaced
/// `ZEROCODE_*` for the same reason the hook variables are: `ZO_*` belongs to
/// the harness CLI.
pub const TEAM_ID_VAR: &str = "ZEROCODE_AGENT_TEAM_ID";
pub const TEAM_TOKEN_VAR: &str = "ZEROCODE_AGENT_TEAM_TOKEN";
pub const TEAM_LEADER_VAR: &str = "ZEROCODE_AGENT_TEAM_LEADER";

/// How long a shim waits for the window before giving up, in seconds.
///
/// It must stay **above** the bridge's own deadline
/// (`zerocode_hookd::TEAM_DEADLINE`): the window answers a request it cannot
/// serve with a refusal an agent can read, and a shim that hung up first would
/// replace that sentence with "could not reach the window" — the one message
/// that means something else entirely. `zerocode_hookd` holds the test that
/// keeps the two from drifting past each other; this crate cannot see that
/// constant, because the dependency runs the other way.
pub const SHIM_DEADLINE_SECONDS: u32 = 15;

/// What the shim adds on top of a caller's own `--timeout-ms`, in seconds.
///
/// The ladder, per request: the sleeper holds exactly the budget, the bridge
/// adds its three, and the shim adds these eight — so however long a wait
/// is, silence still comes back as the window's own `{"count":0}` rather
/// than as a hung-up connection. Eight because it must clear the bridge's
/// grace with room for a slow loopback under load, and stay small enough
/// that a dead window is still an answer inside anyone's patience. While a
/// shim waits like this it prints `{"_keepalive":true,"elapsedMs":N}` to
/// stderr every fifteen seconds — the line that keeps a calling tool's
/// silence budget from killing a healthy wait, stdout staying one clean
/// payload the whole time.
pub const SHIM_WAIT_GRACE_SECONDS: u32 = 8;

/// The fake `tmux`, as a POSIX shell script.
///
/// Orca's shim re-enters its own CLI (`orca agent-teams-tmux`, :173397-173404).
/// This window ships no CLI, so the script talks to the hook bridge that is
/// already listening on loopback with a token — one endpoint, one secret, one
/// thing to keep private, rather than a second server for a second protocol.
///
/// **It fails CLOSED, not open.** The hook script's rule is the opposite: a
/// hook that cannot reach the bridge must let the agent keep working. This one
/// is a `tmux`, and a `tmux` that exits 0 having done nothing tells Claude a
/// teammate is running when none is — so an unreachable bridge is a non-zero
/// status and a word on stderr.
///
/// The status comes back on its own last line (`-w '\n%{http_code}'`) rather
/// than through `--fail-with-body`, which is curl 7.76 and newer. On an older
/// curl that flag is an unknown option, curl exits 2 having sent nothing, and
/// the shim would report a failure for a request that was never made — but
/// worse, an installation that "fixed" it by dropping the flag would exit 0
/// on a 401 and print the refusal as though it were tmux's answer. A trailing
/// line every curl can write costs four lines of shell and removes both.
/// `voice` is the name the script refuses under, and it is a parameter because
/// this same script is installed twice under two names. A `tmux` that could not
/// reach the window has to say `tmux:`; an orchestration verb that could not
/// has to say `orchestration:`, or an agent goes looking for a terminal
/// multiplexer it never invoked.
///
/// The four edges the `zerocode-browser` shim was hardened on (1-g4 리뷰) are
/// the same four here, because it is the same shape of script — and this one
/// runs on **every** tmux verb an agent team makes, which is far more often:
/// - **The bridge token and pane capability never touch argv or a named
///   file.** `ps` shows every local user every process's arguments, and two
///   agents run as the same user, so mode 0600 is not isolation between them.
///   Both secrets ride an inherited file descriptor that curl reads as its
///   config. The team id and pane are routing, not secrets.
/// - **curl's own config cannot redirect it.** `-q` ignores `.curlrc`,
///   `--noproxy '*'` ignores a proxy in the environment (an agent inheriting
///   `http_proxy` would otherwise send every split through it), `--proto =http`
///   pins the scheme, and `--max-time` bounds the wait — the bridge's deadline
///   only starts once the request arrives, so a connection that hangs before
///   that has nothing stopping it and the leader waits forever.
/// - **A separator inside an argument is refused**, because the blob would
///   silently split it into two arguments on the far side.
/// - **A missing `curl` says so**, rather than being reported as a window that
///   could not be reached.
///
/// One fork, not one per argument: `$(printf '\037')` inside the loop spawned
/// a subshell for every word of every verb.
pub fn shim_script(port_var: &str, token_var: &str, voice: &str) -> String {
    // A budget-less `ask` still blocks for the ledger's default ten minutes,
    // so the shim's patience — the ladder's top rung — starts from that
    // number, not from the short deadline every other verb wears.
    let ask_wait_s = crate::orchestration::ASK_BUDGET_DEFAULT_MS / 1000 + SHIM_WAIT_GRACE_SECONDS;
    let worker_wait_s =
        crate::orchestration::READY_TIMEOUT_DEFAULT_MS / 1000 + SHIM_WAIT_GRACE_SECONDS;
    // `jq` is not something we can count on, so the argv is sent as one
    // NUL-separated blob in the body and split on the far side: it is the one
    // encoding that cannot be confused by a quote, a newline or a space in a
    // teammate's prompt.
    format!(
        r#"#!/usr/bin/env sh
set -eu
port="${{{port_var}:-}}"
token="${{{token_var}:-}}"
team="${{{TEAM_ID_VAR}:-}}"
pane="${{{TEAM_PANE_VAR}:-${{TMUX_PANE:-{LEADER_PANE}}}}}"
pane_token="${{{TEAM_TOKEN_VAR}:-}}"
if [ -z "$port" ] || [ -z "$token" ] || [ -z "$team" ] || [ -z "$pane_token" ]; then
  echo "{voice}: this shell is not part of an agent team" >&2
  exit 1
fi
if ! command -v curl >/dev/null 2>&1; then
  echo "{voice}: curl is not installed" >&2
  exit 1
fi
sep=$(printf '\037')
body=""
budget={SHIM_DEADLINE_SECONDS}
waiting=""
want_ms=""
case "${{1:-}}" in
  ask) waiting=1; budget={ask_wait_s} ;;
  worker-start) waiting=1; budget={worker_wait_s} ;;
esac
for arg in "$@"; do
  case "$arg" in
    *"$sep"*)
      echo "{voice}: an argument may not contain the separator byte" >&2
      exit 1
      ;;
  esac
  if [ -n "$want_ms" ]; then
    case "$arg" in
      ''|*[!0-9]*) : ;;
      *) budget=$((arg / 1000 + {SHIM_WAIT_GRACE_SECONDS})) ;;
    esac
    want_ms=""
  fi
  case "$arg" in
    --wait) waiting=1 ;;
    --timeout-ms) want_ms=1 ;;
  esac
  body="$body$arg$sep"
done
keep=""
if [ -n "$waiting" ]; then
  (
    elapsed=0
    while :; do
      # The timer must not inherit the command-substitution pipe. Killing the
      # loop otherwise leaves its current sleep holding stderr open, and every
      # completed long request waits one extra keepalive interval to exit.
      sleep 15 </dev/null >/dev/null 2>&1
      elapsed=$((elapsed + 15000))
      printf '{{"_keepalive":true,"elapsedMs":%d}}\n' "$elapsed" >&2
    done
  ) &
  keep=$!
fi
out=$(printf '%s' "$body" | curl -q -sS --noproxy '*' --proto =http \
  --max-time "$budget" -w '\n%{{http_code}}' \
  -K /dev/fd/3 \
  -H "x-zerocode-team-id: $team" \
  -H "x-zerocode-team-pane: $pane" \
  -H "content-type: application/octet-stream" \
  --data-binary @- \
  "http://127.0.0.1:$port/agent-teams" 2>/dev/null 3<<EOF
header = "x-zerocode-hook-token: $token"
header = "x-zerocode-team-token: $pane_token"
EOF
) || {{
  if [ -n "$keep" ]; then kill "$keep" 2>/dev/null || :; fi
  echo "{voice}: could not reach the window" >&2
  exit 1
}}
if [ -n "$keep" ]; then kill "$keep" 2>/dev/null || :; fi
code="${{out##*
}}"
said="${{out%
*}}"
if [ "$code" != "200" ]; then
  printf '%s' "$said" >&2
  exit 1
fi
printf '%s' "$said"
"#
    )
}

/// The same contract, for a shell that cannot read a shebang.
///
/// On Windows an agent's verbs arrive from three hosts, and the POSIX script
/// above serves only one of them. Git Bash and WSL read the shebang and run it
/// as `sh`; PowerShell and cmd do not — a file with no extension is not a
/// command there at all, so `zerocode-orc` was "installed" and unreachable,
/// which is the worst way for a platform difference to arrive (the words are
/// [`PATH_SEPARATOR`]'s, and this is the same lesson one layer up).
///
/// It is PowerShell rather than a `.cmd` around `curl.exe`, and that choice is
/// the OPPOSITE of the managed hook scripts' rule (H-W: a Windows hook must
/// not start an interpreter), for a reason worth writing down rather than
/// looking inconsistent: a hook fires every few seconds for as long as an
/// agent runs, while a verb is typed when an agent has something to say — and
/// `curl -K` has exactly one road for its config that is neither argv nor a
/// named file, stdin, which this request already spends on the body. cmd
/// cannot make a second one. PowerShell posts in-process, so the hook token
/// and the pane capability touch no argv and no file at all — stronger than
/// the descriptor trick, bought with an interpreter start at a frequency
/// where that cost is noise.
///
/// Everything else mirrors the script above, edge for edge:
/// - **It fails CLOSED.** A bridge it cannot reach is a word on stderr and a
///   non-zero exit, never a 0 that did nothing.
/// - **A separator inside an argument is refused**, because the blob would
///   silently split it into two arguments on the far side.
/// - **No proxy.** An inherited `http_proxy` must not stand between a pane
///   and its own window; `Proxy = $null` is `--noproxy '*'` said in .NET.
/// - **The deadline is [`SHIM_DEADLINE_SECONDS`]**, still above the bridge's,
///   so a refusal the window can word always outruns "could not reach the
///   window".
/// - **The answer goes where the status says**: stdout on 200, stderr and
///   exit 1 for everything else, byte for byte, no added newline.
pub fn shim_script_powershell(port_var: &str, token_var: &str, voice: &str) -> String {
    let deadline_ms = SHIM_DEADLINE_SECONDS * 1000;
    let grace_ms = SHIM_WAIT_GRACE_SECONDS * 1000;
    // The same ask default the POSIX sibling carries, in this dialect's unit.
    let ask_wait_ms = crate::orchestration::ASK_BUDGET_DEFAULT_MS + grace_ms;
    let worker_wait_ms = crate::orchestration::READY_TIMEOUT_DEFAULT_MS + grace_ms;
    format!(
        r#"# Generated by ZeroCode. The POSIX sibling beside this file serves sh-hosted
# agents (Git Bash and every unix; NOT WSL2, whose 127.0.0.1 is not the
# Windows loopback — that door fails closed until the hook work opens it);
# this one exists because PowerShell runs a .ps1 it finds on PATH and cannot
# read a shebang. It runs only where ExecutionPolicy allows a script at all —
# the default on client Windows does not, and PowerShell's command search
# prefers .ps1 over PATHEXT, so a bare `zerocode-orc` cannot fall through to
# the .cmd door either; spelling `zerocode-orc.cmd` always works.
try {{ [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 }} catch {{}}
$port = $env:{port_var}
$token = $env:{token_var}
$team = $env:{TEAM_ID_VAR}
$pane = $env:{TEAM_PANE_VAR}
if (-not $pane) {{ $pane = $env:TMUX_PANE }}
if (-not $pane) {{ $pane = '{LEADER_PANE}' }}
$paneToken = $env:{TEAM_TOKEN_VAR}
if (-not $port -or -not $token -or -not $team -or -not $paneToken) {{
  [Console]::Error.WriteLine('{voice}: this shell is not part of an agent team')
  exit 1
}}
$sep = [string][char]0x1f
$body = ''
$deadlineMs = {deadline_ms}
$waiting = $false
$wantMs = $false
if ($args.Count -gt 0 -and ([string]$args[0]) -eq 'ask') {{
  $waiting = $true
  $deadlineMs = {ask_wait_ms}
}}
if ($args.Count -gt 0 -and ([string]$args[0]) -eq 'worker-start') {{
  $waiting = $true
  $deadlineMs = {worker_wait_ms}
}}
foreach ($arg in $args) {{
  $word = [string]$arg
  if ($word.Contains($sep)) {{
    [Console]::Error.WriteLine('{voice}: an argument may not contain the separator byte')
    exit 1
  }}
  if ($wantMs) {{
    $parsed = 0
    if ([int64]::TryParse($word, [ref]$parsed) -and $parsed -gt 0) {{
      $deadlineMs = $parsed + {grace_ms}
    }}
    $wantMs = $false
  }}
  if ($word -eq '--wait') {{ $waiting = $true }}
  if ($word -eq '--timeout-ms') {{ $wantMs = $true }}
  $body = $body + $word + $sep
}}
$bytes = [System.Text.Encoding]::UTF8.GetBytes($body)
try {{
  $request = [System.Net.HttpWebRequest]::Create("http://127.0.0.1:$port/agent-teams")
  $request.Method = 'POST'
  $request.Proxy = $null
  $request.Timeout = $deadlineMs
  $request.ReadWriteTimeout = $deadlineMs
  $request.ContentType = 'application/octet-stream'
  $request.Headers.Add('x-zerocode-hook-token', $token)
  $request.Headers.Add('x-zerocode-team-token', $paneToken)
  $request.Headers.Add('x-zerocode-team-id', $team)
  $request.Headers.Add('x-zerocode-team-pane', $pane)
  $asking = $request.GetRequestStream()
  $asking.Write($bytes, 0, $bytes.Length)
  $asking.Close()
  # Begin/End rather than GetResponse, because .NET ignores Timeout on the
  # async road and a synchronous wait could not speak: every fifteen quiet
  # seconds of a --wait, one keepalive line goes to stderr so the calling
  # tool's silence budget spares a healthy wait; past the deadline the
  # request is aborted by hand, which is the async road's honest timeout.
  $pending = $request.BeginGetResponse($null, $null)
  $elapsedMs = 0
  while (-not $pending.AsyncWaitHandle.WaitOne(15000)) {{
    $elapsedMs = $elapsedMs + 15000
    if ($waiting) {{
      [Console]::Error.WriteLine('{{"_keepalive":true,"elapsedMs":' + $elapsedMs + '}}')
    }}
    if ($elapsedMs -ge $deadlineMs) {{
      $request.Abort()
    }}
  }}
  $response = $request.EndGetResponse($pending)
  $reader = New-Object System.IO.StreamReader($response.GetResponseStream(), [System.Text.Encoding]::UTF8)
  $said = $reader.ReadToEnd()
  $response.Close()
  [Console]::Out.Write($said)
  exit 0
}} catch {{
  # PowerShell hands a .NET method's exception over wrapped in a
  # MethodInvocationException, so the WebException carrying the bridge's
  # answer is one or more InnerException hops down. Walk to it: a 409 with
  # the window's own sentence must reach the agent as that sentence, not be
  # flattened into "could not reach the window".
  $failed = $_.Exception
  while ($null -ne $failed -and -not ($failed -is [System.Net.WebException])) {{
    $failed = $failed.InnerException
  }}
  $answer = if ($null -ne $failed) {{ $failed.Response }} else {{ $null }}
  if ($null -ne $answer) {{
    $reader = New-Object System.IO.StreamReader($answer.GetResponseStream(), [System.Text.Encoding]::UTF8)
    [Console]::Error.Write($reader.ReadToEnd())
    exit 1
  }}
  [Console]::Error.WriteLine('{voice}: could not reach the window')
  exit 1
}}
"#
    )
}

/// cmd's road to the PowerShell sibling.
///
/// cmd resolves commands through `PATHEXT`, which lists `.CMD` and never
/// `.PS1` — so a cmd-hosted agent finds this, and this finds the script that
/// can keep a secret. `%*` forwards the tail exactly as cmd tokenized it,
/// which is the most cmd can promise: an agent whose quoting must be held
/// byte for byte should be PowerShell- or sh-hosted, and the two files
/// beside this one are for them.
pub fn shim_cmd(powershell_name: &str) -> String {
    format!(
        "@echo off\r\n{}exit /b %ERRORLEVEL%\r\n",
        powershell_file_line(powershell_name)
    )
}

/// The line every `.cmd` door runs its sibling script with: a per-process
/// `-ExecutionPolicy Bypass` (the machine policy is never touched), no
/// profile, no prompts, the tail as cmd tokenized it.
fn powershell_file_line(powershell_name: &str) -> String {
    format!(
        "powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0{powershell_name}\" %*\r\n"
    )
}

/// An agent's `.cmd` door: [`shim_cmd`]'s road to the sibling script, and
/// the `real` agent binary when this machine will not run that script.
///
/// `-ExecutionPolicy Bypass` sets the Process scope, and a Group Policy
/// (the MachinePolicy and UserPolicy scopes — "Turn on Script Execution"
/// Disabled, or "Allow only signed scripts") outranks it: PowerShell
/// refuses the unsigned `.impl.ps1`, and since this door sits at the head
/// of every pane's PATH, the agent a person typed never started at all —
/// before the door existed the same word reached the real binary.
///
/// The policy is kept, not bypassed. Only when the script road ends
/// non-zero does the door ask PowerShell for the effective policy under the
/// same Process scope (a `-Command`, which no execution policy governs);
/// `Restricted` or `AllSigned` means the script cannot have run, so the
/// real agent runs instead, without the mirror — as it did before. Any
/// other answer means the agent ran and failed, and its own exit code goes
/// back untouched. `setlocal` keeps the saved code out of a cmd session
/// that typed the agent. `real` rides as a batch literal
/// (`batch_literal`); a `.cmd` real binary is chained to, which hands
/// its exit code back the same way.
pub fn agent_shim_cmd(powershell_name: &str, real: &str) -> String {
    let real = batch_literal(real);
    format!(
        "@echo off\r\n\
         setlocal\r\n\
         {script_road}\
         set \"ZEROCODE_DOOR_EXIT=%ERRORLEVEL%\"\r\n\
         if \"%ZEROCODE_DOOR_EXIT%\"==\"0\" exit /b 0\r\n\
         powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command \"if ((Get-ExecutionPolicy) -in 'Restricted','AllSigned') {{ exit 0 }}; exit 1\"\r\n\
         if errorlevel 1 exit /b %ZEROCODE_DOOR_EXIT%\r\n\
         \"{real}\" %*\r\n\
         exit /b %ERRORLEVEL%\r\n",
        script_road = powershell_file_line(powershell_name),
    )
}

/// `text` as it must be spelled inside a batch file to reach a command
/// unchanged between double quotes: `%` doubled, since cmd expands it even
/// there. A Windows path can hold no `"`, and `&`, `^`, `|` are inert
/// inside quotes.
fn batch_literal(text: &str) -> String {
    text.replace('%', "%%")
}

/// How the shim packs its argv, and how the bridge unpacks it.
///
/// ASCII Unit Separator: it cannot appear in a command line a shell could have
/// produced, so nothing needs escaping and nothing can be smuggled.
pub const ARGV_SEPARATOR: char = '\u{1f}';

/// Take an argv back out of the shim's body. A trailing separator is the
/// script's own terminator, not an empty final argument.
pub fn unpack_argv(body: &str) -> Vec<String> {
    let trimmed = body.strip_suffix(ARGV_SEPARATOR).unwrap_or(body);
    if trimmed.is_empty() {
        return Vec::new();
    }
    trimmed.split(ARGV_SEPARATOR).map(str::to_string).collect()
}

/// What `capture-pane -p` prints, once the shell has read the screen.
///
/// The placeholder [`plan`] leaves is a NUL, which cannot occur in a format
/// string or a screen line — so a reply that was never finished is visible
/// rather than silently empty.
pub fn capture_reply(planned: &Planned, tail: &str) -> Reply {
    if planned.reply.stdout == "\u{0}" {
        return Reply::ok(format!("{tail}\n"));
    }
    planned.reply.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_string).collect()
    }

    fn team() -> Team {
        Team::new("team-1", "secret", 7)
    }

    #[test]
    fn the_verb_comes_off_the_line_the_way_tmux_takes_it() {
        assert_eq!(
            split_command(&words("split-window -h claude")).expect("verb"),
            ("split-window".into(), words("-h claude"))
        );
        // Global flags before the verb: `-L` swallows a word, `-V` IS the
        // whole command.
        assert_eq!(
            split_command(&words("-L default list-panes")).expect("verb"),
            ("list-panes".into(), Vec::new())
        );
        assert_eq!(
            split_command(&words("-V")).expect("verb"),
            ("-V".into(), Vec::new())
        );
        // Upper-cased verbs are the same verb.
        assert_eq!(
            split_command(&words("Send-Keys hi")).expect("verb").0,
            "send-keys"
        );
        assert!(split_command(&words("-L default")).is_err());
        assert!(split_command(&[]).is_err());
    }

    #[test]
    fn flags_cluster_and_a_value_takes_the_rest_or_the_next_word() {
        let parsed = parse_args(
            &words("-dh -t %2 claude --teammate-mode auto"),
            &["-t"],
            &["-d", "-h"],
        );
        assert!(parsed.has("-d") && parsed.has("-h"));
        assert_eq!(parsed.value("-t"), Some("%2"));
        // `--teammate-mode` is a long flag: positional, not parsed.
        assert_eq!(parsed.positional, words("claude --teammate-mode auto"));

        // The value glued to its flag, and the last one winning.
        let glued = parse_args(&words("-t%3 -t %4"), &["-t"], &[]);
        assert_eq!(glued.value("-t"), Some("%4"));

        // `--` ends the flags: everything after it is a word even if it looks
        // like one.
        let after = parse_args(&words("-l -- -t hi"), &["-t"], &["-l"]);
        assert!(after.has("-l"));
        assert_eq!(after.value("-t"), None);
        assert_eq!(after.positional, words("-t hi"));

        // An unrecognised flag is a word, not an error — tmux's own leniency,
        // and what stops a teammate command line from being eaten.
        let unknown = parse_args(&words("-Z hello"), &["-t"], &["-l"]);
        assert_eq!(unknown.positional, words("-Z hello"));
    }

    #[test]
    fn a_teammate_is_a_split_of_the_pane_that_asked_and_the_leader_keeps_the_keyboard() {
        let mut team = team();
        let planned = plan(
            &mut team,
            &words("split-window -d -h -P -F #{pane_id} claude"),
            LEADER_PANE,
        );
        // Side by side, so the window cuts left|right.
        let Effect::Split {
            pane,
            from,
            direction,
            command,
            helper,
        } = planned.effect.clone()
        else {
            panic!("a teammate did not become a split: {:?}", planned.effect);
        };
        assert_eq!(helper, None, "a Claude teammate is nobody's helper row");
        assert_eq!(from, LEADER_PANE);
        assert_eq!(direction, Direction::Vertical);
        assert_eq!(command, "claude");
        assert_eq!(pane, "%2");
        // `-P` prints the id, which is how the leader learns what to address.
        assert_eq!(planned.reply.stdout, "%2\n");
        assert_eq!(planned.reply.exit_code, 0);
        // Nothing about the plan focuses anything: the ONLY effect that moves
        // the keyboard is `select-pane`.
        assert!(!matches!(planned.effect, Effect::Focus { .. }));
    }

    #[test]
    fn the_column_takes_every_teammate_after_the_first() {
        // The measured layout: leader on one side, teammates stacked on the
        // other. Without `resolve_split_target` the leader would be halved
        // again for every teammate and end up unreadable.
        let mut team = team();
        let first = plan(&mut team, &words("split-window -h claude"), LEADER_PANE);
        let Effect::Split {
            pane, direction, ..
        } = first.effect
        else {
            panic!("no split");
        };
        assert_eq!(direction, Direction::Vertical);
        team.record_split(&pane, 8, LEADER_PANE, direction);

        plan(
            &mut team,
            &words("select-layout main-vertical"),
            LEADER_PANE,
        );

        let second = plan(&mut team, &words("split-window -h claude"), LEADER_PANE);
        let Effect::Split {
            pane: second_pane,
            from,
            direction,
            ..
        } = second.effect
        else {
            panic!("no split");
        };
        // Cut the BOTTOM OF THE COLUMN, top over bottom — not the leader.
        assert_eq!(from, "%2");
        assert_eq!(direction, Direction::Horizontal);
        team.record_split(&second_pane, 9, &from, direction);

        let third = plan(&mut team, &words("split-window -h claude"), LEADER_PANE);
        let Effect::Split {
            from, direction, ..
        } = third.effect
        else {
            panic!("no split");
        };
        assert_eq!(from, "%3", "the column stopped growing at its bottom");
        assert_eq!(direction, Direction::Horizontal);
    }

    #[test]
    fn a_side_by_side_split_of_the_leader_starts_the_column_without_being_told() {
        // `updateMainVerticalAfterSplit`'s second half: the layout can be
        // implied by the first split rather than named. A leader that never
        // says `select-layout` still gets the readable shape.
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        let next = plan(&mut team, &words("split-window -h claude"), LEADER_PANE);
        let Effect::Split {
            from, direction, ..
        } = next.effect
        else {
            panic!("no split");
        };
        assert_eq!(from, "%2");
        assert_eq!(direction, Direction::Horizontal);

        // …but a split that was top-over-bottom implies nothing, because a
        // column is not what it made.
        let mut plain = Team::new("team-1", "secret", 7);
        plain.record_split("%2", 8, LEADER_PANE, Direction::Horizontal);
        let after = plan(&mut plain, &words("split-window -h claude"), LEADER_PANE);
        let Effect::Split { from, .. } = after.effect else {
            panic!("no split");
        };
        assert_eq!(from, LEADER_PANE);
    }

    #[test]
    fn the_three_keys_that_could_end_someone_elses_work_type_nothing() {
        // The refusal this module exists for. A coordinator can address any
        // pane by name; it must not be able to interrupt, EOF or suspend one.
        for key in ["C-c", "c-C", "C-d", "C-z", "BSpace"] {
            assert_eq!(
                send_keys_text(&[key.to_string()], false),
                "",
                "`{key}` reached a pane"
            );
        }
        // And what a prompt actually looks like: the key cancels the space
        // that would have followed the word before it.
        assert_eq!(
            send_keys_text(&words("hello there Enter"), false),
            "hello there\r"
        );
        assert_eq!(send_keys_text(&words("Escape"), false), "\u{1b}");
        // `-l` means literal — nothing is a key name, not even `Enter`.
        assert_eq!(send_keys_text(&words("hello Enter"), true), "hello Enter");
    }

    #[test]
    fn send_keys_with_nothing_to_type_touches_no_terminal() {
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        let planned = plan(&mut team, &words("send-keys -t %2 C-c"), LEADER_PANE);
        assert_eq!(planned.effect, Effect::None);
        assert_eq!(planned.reply.exit_code, 0);

        let typed = plan(&mut team, &words("send-keys -t %2 hi Enter"), LEADER_PANE);
        assert_eq!(
            typed.effect,
            Effect::Send {
                term: 8,
                text: "hi\r".into()
            }
        );
    }

    #[test]
    fn a_pane_this_team_does_not_hold_is_refused_rather_than_reached() {
        let mut team = team();
        let planned = plan(
            &mut team,
            &words("send-keys -t %9 rm -rf / Enter"),
            LEADER_PANE,
        );
        assert_eq!(planned.effect, Effect::None);
        assert_eq!(planned.reply.exit_code, 1);
        assert_eq!(planned.reply.stderr, "tmux: unknown pane: %9\n");
        assert!(planned.reply.stdout.is_empty());

        // And the leader is not something a teammate can end.
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        let killed = plan(&mut team, &words("kill-pane -t %1"), "%2");
        assert_eq!(killed.effect, Effect::None);
        assert_eq!(killed.reply.stderr, "tmux: refusing to kill leader pane\n");
        let respawned = plan(&mut team, &words("respawn-pane -t %1 claude"), "%2");
        assert_eq!(
            respawned.reply.stderr,
            "tmux: refusing to respawn leader pane\n"
        );
    }

    #[test]
    fn a_verb_nobody_measured_says_so_instead_of_pretending() {
        let mut team = team();
        let planned = plan(&mut team, &words("new-window"), LEADER_PANE);
        assert_eq!(planned.reply.exit_code, 1);
        assert_eq!(
            planned.reply.stderr,
            "tmux: unsupported command: new-window\n"
        );
        // But the ones a client says on its way in and out are answered, or
        // the session never starts.
        for quiet in ["set-option -g status off", "has-session", "refresh-client"] {
            let quiet = plan(&mut team, &words(quiet), LEADER_PANE);
            assert_eq!(quiet.reply.exit_code, 0, "{quiet:?}");
            assert_eq!(quiet.effect, Effect::None);
        }
    }

    #[test]
    fn the_list_and_the_message_answer_in_the_format_that_was_asked_for() {
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        team.record_split("%3", 9, "%2", Direction::Horizontal);

        let listed = plan(
            &mut team,
            &words("list-panes -F #{pane_id}:#{pane_active}"),
            LEADER_PANE,
        );
        assert_eq!(listed.reply.stdout, "%1:1\n%2:0\n%3:0\n");

        // No format at all falls back to the pane id, which is the only field
        // a caller can be sure it wanted.
        let bare = plan(&mut team, &words("list-panes"), LEADER_PANE);
        assert_eq!(bare.reply.stdout, "%1\n%2\n%3\n");

        // A hole nothing fills is dropped rather than printed raw.
        let held = plan(
            &mut team,
            &words("display-message -p #{pane_id}|#{nonsense}"),
            "%2",
        );
        assert_eq!(held.reply.stdout, "%2|\n");
    }

    #[test]
    fn a_window_target_is_answered_about_the_caller() {
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        // `-t orca:0` is the window, not a pane — and the pane it reports is
        // the one that asked.
        let planned = plan(
            &mut team,
            &words("display-message -p -t orca:0 #{pane_id}"),
            "%2",
        );
        assert_eq!(planned.reply.stdout, "%2\n");
        // A list asked of the window is the whole window.
        let listed = plan(&mut team, &words("list-panes -t @0"), "%2");
        assert_eq!(listed.reply.stdout, "%1\n%2\n");
    }

    #[test]
    fn capture_hands_back_the_screen_and_only_when_it_was_asked_to_print() {
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        let printed = plan(&mut team, &words("capture-pane -p -t %2"), LEADER_PANE);
        assert_eq!(
            printed.effect,
            Effect::Capture {
                term: 8,
                lines: CAPTURE_LINES
            }
        );
        assert_eq!(capture_reply(&printed, "one\ntwo").stdout, "one\ntwo\n");

        let buffered = plan(&mut team, &words("capture-pane -t %2"), LEADER_PANE);
        assert_eq!(capture_reply(&buffered, "one\ntwo").stdout, "");
    }

    #[test]
    fn focus_moves_only_when_a_pane_is_named_and_last_pane_can_walk_back() {
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        let chosen = plan(&mut team, &words("select-pane -t %2"), LEADER_PANE);
        assert_eq!(chosen.effect, Effect::Focus { term: 8 });
        // Where it came FROM is what `last-pane` walks back to.
        let back = plan(&mut team, &words("last-pane"), "%2");
        assert_eq!(back.effect, Effect::Focus { term: 7 });
        // Colouring a pane is not focusing it.
        let painted = plan(
            &mut team,
            &words("select-pane -t %2 -P bg=red"),
            LEADER_PANE,
        );
        assert_eq!(painted.effect, Effect::None);
    }

    #[test]
    fn a_closed_pane_leaves_the_table_and_the_column_finds_its_new_bottom() {
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        team.record_split("%3", 9, "%2", Direction::Horizontal);
        assert_eq!(
            team.main_vertical
                .as_ref()
                .and_then(|m| m.last_column_pane.clone()),
            Some("%3".into())
        );
        team.remove_pane("%3");
        assert_eq!(
            team.main_vertical
                .as_ref()
                .and_then(|m| m.last_column_pane.clone()),
            Some("%2".into()),
            "the column's bottom is gone and nothing took its place"
        );
        // Down to the leader alone, there is no column left to name.
        team.remove_pane("%2");
        assert_eq!(
            team.main_vertical
                .as_ref()
                .and_then(|m| m.last_column_pane.clone()),
            None
        );
        // And the pane really is gone from every answer.
        let listed = plan(&mut team, &words("list-panes"), LEADER_PANE);
        assert_eq!(listed.reply.stdout, "%1\n");
        assert_eq!(team.term_of("%2"), None);
    }

    #[test]
    fn a_respawn_re_cuts_where_the_pane_came_from() {
        let mut team = team();
        team.record_split("%2", 8, LEADER_PANE, Direction::Vertical);
        team.record_split("%3", 9, "%2", Direction::Horizontal);
        let planned = plan(
            &mut team,
            &words("respawn-pane -k -t %3 claude"),
            LEADER_PANE,
        );
        let Effect::Respawn {
            pane,
            from,
            direction,
            command,
            term,
        } = planned.effect
        else {
            panic!("respawn did not re-cut: {:?}", planned.effect);
        };
        // The pane keeps its name — the leader goes on addressing `%3` — and
        // only the shell behind it is replaced.
        assert_eq!(pane, "%3");
        assert_eq!(from, "%2");
        assert_eq!(direction, Direction::Horizontal);
        assert_eq!(command, "claude");
        assert_eq!(term, 9);
        // A respawn with nothing to run is a no-op rather than a pane killed
        // and not replaced.
        let empty = plan(&mut team, &words("respawn-pane -t %3"), LEADER_PANE);
        assert_eq!(empty.effect, Effect::None);
    }

    #[test]
    fn only_a_plain_claude_command_line_gets_the_flag() {
        assert!(is_direct_claude_command("claude"));
        assert!(is_direct_claude_command("/usr/local/bin/claude --resume"));
        // Not claude, and not one word we can rewrite.
        assert!(!is_direct_claude_command("codex"));
        assert!(!is_direct_claude_command("claude | tee log"));
        assert!(!is_direct_claude_command("claude; rm -rf /"));
        assert!(!is_direct_claude_command("   "));
        // `claudette` is somebody else's program, not a claude with a suffix.
        assert!(!is_direct_claude_command("claudette"));
    }

    #[test]
    fn the_teammate_mode_lands_after_the_program_and_never_twice() {
        assert_eq!(
            with_teammate_mode("claude --dangerously-skip-permissions", TeamsMode::Panes),
            "claude --teammate-mode auto --dangerously-skip-permissions"
        );
        assert_eq!(
            with_teammate_mode("claude", TeamsMode::InProcess),
            "claude --teammate-mode in-process"
        );
        // Off changes nothing at all — the feature is not merely unused, it is
        // absent from the command line.
        assert_eq!(with_teammate_mode("claude", TeamsMode::Off), "claude");
        // A person who said it themselves is not corrected.
        for said in [
            "claude --teammate-mode in-process",
            "claude --teammate-mode=in-process",
        ] {
            assert_eq!(with_teammate_mode(said, TeamsMode::Panes), said);
        }
    }

    #[test]
    fn an_argv_launch_gets_the_same_answer_without_being_re_split() {
        let argv = |line: &str| words(line);
        assert_eq!(
            teammate_mode_args(
                &argv("claude --dangerously-skip-permissions"),
                TeamsMode::Panes
            ),
            words("--teammate-mode auto")
        );
        // Off, said already, and not-claude all answer with nothing to insert.
        assert!(teammate_mode_args(&argv("claude"), TeamsMode::Off).is_empty());
        assert!(
            teammate_mode_args(&argv("claude --teammate-mode auto"), TeamsMode::Panes).is_empty()
        );
        assert!(teammate_mode_args(&argv("codex"), TeamsMode::Panes).is_empty());
        assert!(teammate_mode_args(&[], TeamsMode::Panes).is_empty());
        // And a prompt with spaces in it is not something this has to survive
        // re-splitting, which is the whole reason it takes an argv.
        let with_prompt = vec!["claude".to_string(), "fix the flaky test".to_string()];
        assert_eq!(
            teammate_mode_args(&with_prompt, TeamsMode::InProcess),
            words("--teammate-mode in-process")
        );
    }

    /// Orca ships `claudeAgentTeamsMode` off; ours ships PANES — the person
    /// asked for the split by name, twice ("소넷과 codex 다 소환하고 … 화면
    /// 분활이 이루어저야"). Pinned so a fidelity sweep cannot quietly fold
    /// the recorded deviation back to Orca's default.
    #[test]
    fn the_default_teams_mode_is_panes_by_request() {
        assert_eq!(TeamsMode::default(), TeamsMode::Panes);
    }

    #[test]
    fn the_shim_is_found_before_a_real_tmux_and_the_leader_knows_its_own_name() {
        let sep = PATH_SEPARATOR;
        let env = team_launch_env(
            "team-9",
            "s3cret",
            &["/tmp/shim"],
            &format!("/usr/bin{sep}/bin"),
            "",
            "claude",
        );
        let held = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(
            held("PATH"),
            format!("/tmp/shim{sep}/usr/bin{sep}/bin"),
            "a real tmux wins"
        );
        assert_eq!(held("TMUX_PANE"), LEADER_PANE);
        assert_eq!(held(TEAM_PANE_VAR), LEADER_PANE);
        // NOT Orca's `screen-256color`: our grid speaks xterm's dialect, and
        // `TERM` names what the reader really is (the underline-everything
        // break, 1-fr).
        assert_eq!(held("TERM"), "xterm-256color");
        assert_eq!(held("COLORTERM"), "truecolor");
        assert_eq!(held("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"), "1");
        assert!(held("TMUX").ends_with("team-9,0,1"));
        assert_eq!(held(TEAM_TOKEN_VAR), "s3cret");
        // An inherited COLORTERM is kept rather than overwritten.
        let inherited = team_launch_env("t", "s", &["/shim"], "", "24bit", "claude");
        assert!(inherited.contains(&("COLORTERM".into(), "24bit".into())));
        assert!(inherited.contains(&("PATH".into(), "/shim".into())));
    }

    /// An agent that never asked to be inside a multiplexer is not told it is.
    ///
    /// The three variables that make Claude look for a `tmux` are the three a
    /// codex leader must not carry: `TMUX` set for it would have every tool in
    /// that pane — a prompt, a pager, an editor — behaving for a multiplexer
    /// that is not there. What it does get is the road, under a name that
    /// claims nothing.
    #[test]
    fn a_leader_that_does_not_speak_tmux_is_not_handed_one() {
        let ledger = team_launch_env("team-2", "s", &["/orc"], "/usr/bin", "", "codex");
        let named: Vec<&str> = ledger.iter().map(|(key, _)| key.as_str()).collect();
        for absent in ["TMUX", "TMUX_PANE", "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"] {
            assert!(!named.contains(&absent), "a ledger leader carries {absent}");
        }
        // But it knows which pane it is, and how to reach the window.
        let held = |name: &str| {
            ledger
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
                .unwrap_or_default()
        };
        assert_eq!(held(TEAM_PANE_VAR), LEADER_PANE);
        assert_eq!(held(TEAM_ID_VAR), "team-2");
        assert_eq!(held(TEAM_TOKEN_VAR), "s");
        assert!(held("PATH").starts_with("/orc"));

        // And the dialect is a fact about the program, not a preference.
        assert_eq!(Dialect::of("claude"), Dialect::Tmux);
        assert_eq!(Dialect::of("codex"), Dialect::Ledger);
        assert_eq!(Dialect::of(""), Dialect::Ledger);
    }

    /// The argv zo's panes executor actually types, planned end to end.
    ///
    /// `split-window -h -P -F #{pane_id} -- zo --teammate <dir>`: the `--`
    /// keeps zo's own flags out of tmux's parse, `-P -F` is how the executor
    /// learns the pane id it must later kill, and the second child of one
    /// wave stacks under the first rather than halving the leader again.
    #[test]
    fn a_zo_teammate_argv_becomes_a_split_that_answers_with_a_pane_id() {
        let mut team = team();
        let brief = "/tmp/zo-agents/a1";
        let first = plan(
            &mut team,
            &words(&format!(
                "split-window -h -P -F #{{pane_id}} -- zo --teammate {brief}"
            )),
            LEADER_PANE,
        );
        let Effect::Split {
            pane,
            from,
            direction,
            command,
            ..
        } = first.effect.clone()
        else {
            panic!("no split: {:?}", first.effect);
        };
        assert_eq!(from, LEADER_PANE);
        assert_eq!(direction, Direction::Vertical);
        assert_eq!(command, format!("zo --teammate {brief}"));
        // The id is in the ANSWER, which is the only way the executor can
        // learn which pane to kill when the parent's turn is cancelled.
        assert_eq!(first.reply.stdout, format!("{pane}\n"));
        assert_eq!(first.reply.exit_code, 0);
        team.record_split(&pane, 8, LEADER_PANE, direction);

        // A wave's second child stacks below the first — Claude's own shape.
        let second = plan(
            &mut team,
            &words(&format!(
                "split-window -v -P -F #{{pane_id}} -- zo --teammate {brief}-2"
            )),
            LEADER_PANE,
        );
        let Effect::Split {
            from, direction, ..
        } = second.effect
        else {
            panic!("no split");
        };
        assert_eq!(from, LEADER_PANE);
        assert_eq!(direction, Direction::Horizontal);
    }

    /// One identity, one row (t-3024).
    ///
    /// zo's pane lane cuts its helper with `-e ZO_AGENT_ID=<id>` — the id the
    /// `SubagentStart` hook already named — and the plan lifts that pair onto
    /// the split as `helper`, so the window can fold the hook's row and the
    /// pane's row into one. Any other `-e` pair is tmux's environment: not a
    /// helper, and not a word of the command either. A split without the
    /// pair is what it always was.
    #[test]
    fn a_zo_split_that_names_its_helper_hands_the_id_to_the_window() {
        let mut team = team();
        let named = plan(
            &mut team,
            &words(
                "split-window -h -d -P -F #{pane_id} -e ZO_AGENT_ID=agent-1757 -- zo --teammate /tmp/a",
            ),
            LEADER_PANE,
        );
        let Effect::Split {
            helper, command, ..
        } = named.effect
        else {
            panic!("no split");
        };
        assert_eq!(helper.as_deref(), Some("agent-1757"));
        assert_eq!(
            command, "zo --teammate /tmp/a",
            "the identity pair leaked into the command"
        );
        assert_eq!(named.reply.exit_code, 0);

        // Another pair is tmux's environment, untouched and not an identity.
        let other = plan(
            &mut team,
            &words("split-window -h -e FOO=bar claude"),
            LEADER_PANE,
        );
        let Effect::Split {
            helper, command, ..
        } = other.effect
        else {
            panic!("no split");
        };
        assert_eq!(helper, None);
        assert_eq!(command, "claude", "a foreign pair ate the command");

        // Beside another pair, the identity is still found.
        let both = plan(
            &mut team,
            &words("split-window -v -e FOO=bar -e ZO_AGENT_ID=agent-2 -- zo"),
            LEADER_PANE,
        );
        let Effect::Split {
            helper, command, ..
        } = both.effect
        else {
            panic!("no split");
        };
        assert_eq!(helper.as_deref(), Some("agent-2"));
        assert_eq!(command, "zo");

        // Bare, as every Claude teammate is.
        let bare = plan(&mut team, &words("split-window -h claude"), LEADER_PANE);
        let Effect::Split { helper, .. } = bare.effect else {
            panic!("no split");
        };
        assert_eq!(helper, None);
        // And an empty value is no identity.
        let empty = plan(
            &mut team,
            &words("split-window -h -e ZO_AGENT_ID= claude"),
            LEADER_PANE,
        );
        let Effect::Split { helper, .. } = empty.effect else {
            panic!("no split");
        };
        assert_eq!(helper, None);
    }

    /// Two programs run `tmux` to start a teammate, and one of them is ours.
    ///
    /// zo's `Agent` tool splits a pane by typing `tmux split-window` and
    /// reading the pane id back, exactly as Claude's Agent Teams does. So the
    /// same fact — "this program speaks the multiplexer" — has to answer yes
    /// for it, or a zo leader is handed a team it cannot see and every
    /// sub-agent stays inside its own process.
    #[test]
    fn zo_speaks_the_multiplexer_and_a_look_alike_does_not() {
        assert_eq!(Dialect::of("zo"), Dialect::Tmux);
        assert_eq!(Dialect::of("/usr/local/bin/zo"), Dialect::Tmux);
        // A leader is a program, never a shell line: the flag we would append
        // has nowhere to go in `zo | tee log`.
        assert_eq!(Dialect::of("zo | tee log"), Dialect::Ledger);
        // And the name has to BE zo, not merely start with it.
        assert_eq!(Dialect::of("zoom"), Dialect::Ledger);
        assert_eq!(Dialect::of("codex"), Dialect::Ledger);
    }

    /// A zo leader is told about the multiplexer and NOT about Claude's flag.
    ///
    /// The three tmux variables are what a leader reads to find its team;
    /// `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS` is one vendor's feature switch,
    /// and setting it in a zo pane would arm Agent Teams inside any `claude`
    /// that pane's own tools happen to run — a team nobody asked for, cut out
    /// of a leader that is not claude.
    #[test]
    fn a_zo_leader_gets_the_multiplexer_without_claudes_feature_switch() {
        let env = team_launch_env("team-zo", "s", &["/shim"], "/usr/bin", "", "zo");
        let held = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(held("TMUX_PANE").as_deref(), Some(LEADER_PANE));
        assert!(held("TMUX").is_some_and(|value| value.contains("team-zo")));
        assert_eq!(held(TEAM_PANE_VAR).as_deref(), Some(LEADER_PANE));
        assert!(held("PATH").is_some_and(|value| value.starts_with("/shim")));
        assert_eq!(
            held("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS"),
            None,
            "a zo leader must not arm another vendor's feature switch"
        );
        // Claude still gets it — this is one program's switch, not a policy.
        let claude = team_launch_env("team-cc", "s", &["/shim"], "", "", "claude");
        assert!(
            claude
                .iter()
                .any(|(key, value)| key == "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS" && value == "1")
        );
    }

    /// The shim reads our own name for a pane before tmux's.
    ///
    /// A ledger team has no `TMUX_PANE` to read, and a shim that only knew that
    /// name would report every worker as the leader — after which the first
    /// `worker-start` a worker ran would cut the LEADER's pane.
    #[test]
    fn the_shim_asks_our_own_name_for_a_pane_first() {
        let script = shim_script("PORT_V", "TOKEN_V", "orchestration");
        let asking = script
            .lines()
            .find(|line| line.starts_with("pane="))
            .expect("the shim stopped resolving a pane");
        let ours = asking.find(TEAM_PANE_VAR).expect("our own name is gone");
        let theirs = asking.find("TMUX_PANE").expect("the fallback is gone");
        assert!(ours < theirs, "tmux's name is asked first: {asking}");
        assert!(asking.contains(LEADER_PANE), "no last resort: {asking}");
    }

    #[test]
    fn an_argv_survives_the_shim_whatever_is_in_it() {
        // The one thing that must not break: a teammate prompt with spaces,
        // quotes and newlines in it.
        let argv = vec![
            "send-keys".to_string(),
            "-t".to_string(),
            "%2".to_string(),
            "read \"the file\"\nand report".to_string(),
        ];
        let packed: String = argv
            .iter()
            .map(|one| format!("{one}{ARGV_SEPARATOR}"))
            .collect();
        assert_eq!(unpack_argv(&packed), argv);
        // An empty body is no arguments, not one empty argument — and the
        // separator the script always writes last is a terminator, not the
        // start of an empty final word.
        assert!(unpack_argv("").is_empty());
        assert!(unpack_argv(&ARGV_SEPARATOR.to_string()).is_empty());
        // An argument that really IS empty still survives, because the one
        // before it ends with a separator of its own.
        assert_eq!(
            unpack_argv(&format!("send-keys{ARGV_SEPARATOR}{ARGV_SEPARATOR}")),
            vec!["send-keys".to_string(), String::new()]
        );
    }

    #[test]
    fn the_shim_refuses_rather_than_lying_when_it_cannot_reach_the_window() {
        let script = shim_script("ZEROCODE_HOOK_PORT", "ZEROCODE_HOOK_TOKEN", "tmux");
        assert!(script.starts_with("#!/usr/bin/env sh\nset -eu\n"));
        // The failure path is an exit 1. A tmux that exits 0 having done
        // nothing tells Claude a teammate is running when none is.
        assert!(script.contains("exit 1"));
        assert!(!script.contains("exit 0"));
        // It reads the pane out of its own environment, so a teammate asking
        // is not mistaken for the leader. Our own name first, tmux's as the
        // fallback, the leader as the last resort — held whole rather than by
        // its pieces, and the ordering is `the_shim_asks_our_own_name_first`.
        assert!(script.contains(&format!(
            "pane=\"${{{TEAM_PANE_VAR}:-${{TMUX_PANE:-{LEADER_PANE}}}}}\""
        )));
        assert!(script.contains(&format!("pane_token=\"${{{TEAM_TOKEN_VAR}:-}}\"")));
        assert!(script.contains("x-zerocode-team-token"));
        assert!(script.contains("/agent-teams"));
        assert!(
            script.contains("127.0.0.1"),
            "the shim must not leave loopback"
        );
        // The status is read off a line curl always knows how to write. A
        // `--fail-*` flag would be a curl-version dependency in the one place
        // where the fallback ("exit 0 and print the refusal") is the exact lie
        // this script exists to avoid.
        assert!(script.contains(r#"-w '\n%{http_code}'"#));
        assert!(script.contains(r#"if [ "$code" != "200" ]; then"#));
        assert!(!script.contains("--fail"));
    }

    /// The four edges the browser shim was hardened on, on the road that runs
    /// far more often — this script answers every tmux verb an agent team makes.
    #[test]
    fn the_shim_keeps_the_edges_its_sibling_was_given() {
        let script = shim_script("ZEROCODE_HOOK_PORT", "ZEROCODE_HOOK_TOKEN", "tmux");
        // One fork for the separator, not one per argument. The loop body only
        // concatenates; the substitution is hoisted above it.
        assert!(script.contains("sep=$(printf '\\037')"), "{script}");
        assert!(script.contains(r#"body="$body$arg$sep""#), "{script}");
        assert!(
            !script.contains(r#"$arg$(printf"#),
            "the separator forks a subshell per argument again:\n{script}"
        );
        // The token never reaches argv, where `ps` shows it to every local
        // user, or a named file that another process under this user can read.
        assert!(script.contains("-K /dev/fd/3"), "{script}");
        assert!(
            !script.contains(r#"-H "x-zerocode-hook-token"#),
            "the token is back on the command line:\n{script}"
        );
        assert!(!script.contains("mktemp"), "{script}");
        assert!(!script.contains("$headers"), "{script}");
        // The team id and the pane are routing, not secrets — they stay where
        // a person debugging a team can read them.
        assert!(
            script.contains(r#"-H "x-zerocode-team-id: $team""#),
            "{script}"
        );
        // curl's own config cannot redirect it, and a connection that hangs
        // before the bridge's deadline starts cannot hold the leader forever.
        // The deadline is a VARIABLE now — a wait brings its own budget — but
        // its default is still the constant, and both facts are pinned.
        for hardening in [
            "-q",
            "--noproxy '*'",
            "--proto =http",
            "--max-time \"$budget\"",
        ] {
            assert!(script.contains(hardening), "curl lost `{hardening}`");
        }
        assert!(
            script.contains(&format!("budget={SHIM_DEADLINE_SECONDS}")),
            "the default budget is no longer the shim deadline:\n{script}"
        );
        // A wait stretches the budget by the ladder's grace and keeps the
        // calling tool alive: the flag scan, the arithmetic, and the exact
        // keepalive shape are all load-bearing — losing any of them turns a
        // long wait back into \"could not reach the window\" at second 15.
        for waiting in [
            "--timeout-ms) want_ms=1",
            &format!("budget=$((arg / 1000 + {SHIM_WAIT_GRACE_SECONDS}))"),
            "--wait) waiting=1",
            // A question blocks with no flag: the first word alone turns the
            // keepalive on and stretches the budget to the ledger's default.
            &format!(
                "ask) waiting=1; budget={}",
                crate::orchestration::ASK_BUDGET_DEFAULT_MS / 1000 + SHIM_WAIT_GRACE_SECONDS
            ),
            &format!(
                "worker-start) waiting=1; budget={}",
                crate::orchestration::READY_TIMEOUT_DEFAULT_MS / 1000 + SHIM_WAIT_GRACE_SECONDS
            ),
            r#"printf '{"_keepalive":true,"elapsedMs":%d}\n' "$elapsed" >&2"#,
        ] {
            assert!(
                script.contains(waiting),
                "the wait road lost `{waiting}`:\n{script}"
            );
        }
        // A separator inside an argument would silently split it in two on the
        // far side, and a missing curl is not an unreachable window.
        assert!(script.contains("may not contain the separator byte"));
        assert!(script.contains("curl is not installed"));
        // The voice is the shim's own name in every one of those refusals.
        let orchestration = shim_script("PORT_V", "TOKEN_V", "orchestration");
        assert!(orchestration.contains("orchestration: curl is not installed"));
        assert!(!orchestration.contains("tmux: curl"));
    }

    #[test]
    fn the_powershell_shim_keeps_the_same_contract_without_a_process() {
        let script = shim_script_powershell("PORT_V", "TOKEN_V", "orchestration");
        // The secrets are read from the environment and posted in-process:
        // nothing external is started, so there is no argv anywhere for them
        // to ride and no file for another same-user process to read.
        assert!(script.contains("$env:PORT_V"), "{script}");
        assert!(script.contains("$env:TOKEN_V"), "{script}");
        assert!(script.contains("HttpWebRequest"), "{script}");
        for external in ["curl", "Start-Process", "Invoke-Expression", "cmd /c"] {
            assert!(
                !script.contains(external),
                "the PowerShell shim started `{external}`:\n{script}"
            );
        }
        // The same environment words as the POSIX sibling, so one launch
        // serves both hosts.
        for var in [TEAM_ID_VAR, TEAM_TOKEN_VAR, TEAM_PANE_VAR] {
            assert!(script.contains(&format!("$env:{var}")), "{script}");
        }
        // Fails closed, in its own voice, on every edge the POSIX script has.
        assert!(script.contains("orchestration: this shell is not part of an agent team"));
        assert!(script.contains("orchestration: could not reach the window"));
        assert!(script.contains("may not contain the separator byte"));
        // The separator is the same byte the bridge unpacks.
        assert!(script.contains("[char]0x1f"), "{script}");
        assert_eq!(ARGV_SEPARATOR as u32, 0x1f);
        // The deadline still sits above the bridge's, said in milliseconds —
        // now as a variable whose DEFAULT is the constant, because a wait
        // brings its own budget. Both facts are pinned, and so is the whole
        // async wait road: .NET ignores `Timeout` on Begin/End, so the manual
        // Abort IS the timeout there, and the keepalive line is what keeps a
        // calling tool's silence budget from killing a healthy long wait.
        assert!(
            script.contains(&format!("$deadlineMs = {}", SHIM_DEADLINE_SECONDS * 1000)),
            "{script}"
        );
        for wired in [
            "$request.Timeout = $deadlineMs",
            "$request.ReadWriteTimeout = $deadlineMs",
            &format!("$parsed + {}", SHIM_WAIT_GRACE_SECONDS * 1000),
            // The ask default, in this dialect: first word, keepalive on,
            // deadline stretched — the same three facts the POSIX pin holds.
            &format!(
                "$deadlineMs = {}",
                crate::orchestration::ASK_BUDGET_DEFAULT_MS + SHIM_WAIT_GRACE_SECONDS * 1000
            ),
            &format!(
                "$deadlineMs = {}",
                crate::orchestration::READY_TIMEOUT_DEFAULT_MS + SHIM_WAIT_GRACE_SECONDS * 1000
            ),
            "$request.BeginGetResponse($null, $null)",
            "$request.EndGetResponse($pending)",
            "$request.Abort()",
            "\"_keepalive\":true",
        ] {
            assert!(
                script.contains(wired),
                "the wait road lost `{wired}`:\n{script}"
            );
        }
        // An inherited proxy must not stand between a pane and its window.
        assert!(script.contains("$request.Proxy = $null"), "{script}");
        // A refusal's body survives the wrapper. PowerShell wraps a .NET
        // method's exception in a MethodInvocationException, so the naive
        // `$_.Exception.Response` is always `$null` and every 409 the bridge
        // words would flatten into "could not reach the window". The walk to
        // the WebException is the contract; losing it loses every refusal.
        assert!(script.contains(".InnerException"), "{script}");
        assert!(script.contains("-is [System.Net.WebException]"), "{script}");
        // All four headers the bridge reads, by their exact names.
        for header in [
            "x-zerocode-hook-token",
            "x-zerocode-team-token",
            "x-zerocode-team-id",
            "x-zerocode-team-pane",
        ] {
            assert!(script.contains(header), "{script}");
        }
        // A refusal goes to stderr and never to stdout — the same rule the
        // execution test pins for the POSIX script.
        assert!(script.contains("[Console]::Error.Write"), "{script}");
    }

    #[test]
    fn the_cmd_wrapper_holds_nothing_worth_reading() {
        let script = shim_cmd("zerocode-orc.ps1");
        // One hop to the sibling, tail forwarded as cmd tokenized it, exit
        // code passed through rather than swallowed.
        assert!(script.contains("\"%~dp0zerocode-orc.ps1\" %*"), "{script}");
        assert!(script.contains("exit /b %ERRORLEVEL%"), "{script}");
        // No secret, no header, no endpoint is ever spelled here.
        assert!(!script.contains("x-zerocode"), "{script}");
        assert!(!script.contains("127.0.0.1"), "{script}");
        // A user's profile must not run under an agent's verb, and a policy
        // refusing scripts must not turn a verb into a dialog.
        assert!(script.contains("-NoProfile"), "{script}");
        assert!(script.contains("-ExecutionPolicy Bypass"), "{script}");
        // CRLF, because cmd is the one reader of this file.
        assert!(script.ends_with("\r\n"), "{script}");
        for line in script.split("\r\n") {
            assert!(!line.contains('\r'), "a bare CR inside a line: {script}");
            assert!(!line.contains('\n'), "a bare LF inside a line: {script}");
        }
    }

    /// An agent's `.cmd` door sits at the head of PATH, so a Group Policy
    /// that outranks `-ExecutionPolicy Bypass` (scripts Disabled, or only
    /// signed ones) must not leave `claude` unable to start: when the
    /// script road ends non-zero the door asks the effective policy under
    /// the same Process scope, and only a policy that refuses the unsigned
    /// script — so it cannot have run — sends it to the real binary. A run
    /// that did happen keeps its own exit code, and the door's variable
    /// does not leak into a cmd session that typed the agent.
    #[test]
    fn an_agents_cmd_door_reaches_the_real_agent_when_policy_refuses_the_script() {
        let real = r"C:\Users\x\100%\.local\bin\claude.exe";
        let script = agent_shim_cmd("claude.impl.ps1", real);
        let lines: Vec<&str> = script.split("\r\n").collect();
        let at = |needle: &str| {
            lines
                .iter()
                .position(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("no line holds {needle:?}:\n{script}"))
        };
        let script_road = at("-File \"%~dp0claude.impl.ps1\" %*");
        let asked = at("(Get-ExecutionPolicy) -in 'Restricted','AllSigned'");
        let fallback = at(r#""C:\Users\x\100%%\.local\bin\claude.exe" %*"#);
        assert!(
            script_road < asked && asked < fallback,
            "script first, the policy asked on failure, the real agent last:\n{script}"
        );
        assert!(
            lines[asked].contains("-ExecutionPolicy Bypass"),
            "the policy must be asked under the Process scope the script ran under:\n{script}"
        );
        // A clean run and an allowed-but-failed run both leave with the
        // agent's own code; only then may the fallback line be reached.
        assert!(script.contains("exit /b 0"), "{script}");
        assert!(
            lines[asked + 1].starts_with("if errorlevel 1 exit /b %"),
            "an allowed script's failure is the agent's, passed back:\n{script}"
        );
        assert!(
            lines[1] == "setlocal",
            "the saved code leaks into the caller:\n{script}"
        );
        assert!(script.ends_with("\r\n"), "{script}");
        for line in script.split("\r\n") {
            assert!(!line.contains('\r') && !line.contains('\n'), "{script}");
        }
        // The doors without a real binary keep their three lines.
        assert_eq!(shim_cmd("x.impl.ps1").matches("\r\n").count(), 3);
    }

    #[test]
    fn the_shim_runs_as_a_shell_script_and_answers_a_real_reply() {
        // The script is the one piece of this module that is not Rust, so it is
        // exercised as what it is: `sh` runs it against a `curl` that answers a
        // canned body and status. Without this the whole team road rests on a
        // string nobody ever executed.
        let root = std::env::temp_dir().join("zerocode-agent-teams-shim-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        let shim = root.join("tmux");
        std::fs::write(&shim, shim_script("PORT_V", "TOKEN_V", "tmux")).expect("shim");
        // A `curl` that swallows the body it was piped, writes down the argv it
        // was handed, then prints what the test wants the window to have said.
        // The argv is the point of the second half of this test: it is what
        // `ps` shows to every other user on the machine.
        let fake = root.join("curl");
        let sink = root.join("argv");
        std::fs::write(
            &fake,
            format!(
                "#!/usr/bin/env sh\nprintf '%s\\n' \"$@\" > '{}'\ncat >/dev/null\nprintf '%s\\n%s' \"${{FAKE_BODY}}\" \"${{FAKE_CODE}}\"\n",
                sink.display()
            ),
        )
        .expect("curl");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [&shim, &fake] {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
            }
        }
        let run_argv = |code: &str, body: &str, team: &str, argv: &[&str]| {
            std::process::Command::new("sh")
                .arg(&shim)
                .args(argv)
                // The scratch directory FIRST, so the fake `curl` is the one
                // found — and the real one behind it, because `sh` itself
                // lives there.
                .env("PATH", format!("{}:/usr/bin:/bin", root.display()))
                .env("PORT_V", "1234")
                .env("TOKEN_V", "tok")
                .env(TEAM_ID_VAR, team)
                .env(TEAM_TOKEN_VAR, "pane-secret")
                .env("TMUX_PANE", "%2")
                .env("FAKE_CODE", code)
                .env("FAKE_BODY", body)
                .output()
                .expect("sh")
        };
        let run = |code: &str, body: &str, team: &str| {
            run_argv(code, body, team, &["split-window", "-h", "claude"])
        };
        // 200: the body is the answer, on stdout, with a clean status.
        let ok = run("200", "%3", "team-1");
        assert!(
            ok.status.success(),
            "{}",
            String::from_utf8_lossy(&ok.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&ok.stdout), "%3");
        // A refusal is a refusal: nothing on stdout, the reason on stderr, and
        // a non-zero status. This is the failure the whole design turns on — a
        // `tmux` that exits 0 having done nothing tells Claude a teammate is
        // running when none is.
        let no = run("409", "tmux: unknown pane: %9", "team-1");
        assert!(!no.status.success());
        assert!(
            no.stdout.is_empty(),
            "a refusal reached stdout as an answer"
        );
        assert!(String::from_utf8_lossy(&no.stderr).contains("unknown pane"));
        // And a shell that is not part of a team says so without calling out
        // at all.
        let loose = run("200", "%3", "");
        assert!(!loose.status.success());
        assert!(String::from_utf8_lossy(&loose.stderr).contains("not part of an agent team"));
        // The secret is NOT on the command line the shim exec'd `curl` with —
        // `ps` shows argv to every local user, and this shim runs on every verb
        // an agent team makes. The routing headers stay where they are readable.
        let handed = std::fs::read_to_string(&sink).expect("the fake curl wrote no argv");
        assert!(
            !handed.contains("tok"),
            "the hook token was handed to curl on argv:\n{handed}"
        );
        assert!(
            !handed.contains("pane-secret"),
            "the pane capability was handed to curl on argv:\n{handed}"
        );
        assert!(handed.contains("x-zerocode-team-id: team-1"), "{handed}");
        // The config rides an inherited descriptor, not a same-UID-readable
        // named file.
        assert!(
            handed.lines().any(|line| line == "/dev/fd/3"),
            "the token stopped riding the inherited descriptor:\n{handed}"
        );
        // A separator inside an argument is refused rather than silently split
        // into two arguments on the far side.
        let smuggled = format!("read{ARGV_SEPARATOR}kill-server");
        let split = run_argv("200", "%3", "team-1", &["send-keys", "-t", "%2", &smuggled]);
        assert!(!split.status.success());
        assert!(
            String::from_utf8_lossy(&split.stderr).contains("may not contain the separator byte"),
            "{}",
            String::from_utf8_lossy(&split.stderr)
        );
        // A budgeted wait stretches the patience curl is given: sixty seconds
        // of caller budget arrives as sixty-eight of `--max-time` — the
        // ladder's top rung, computed by the script itself and witnessed off
        // the argv the fake curl wrote down. The answer still comes back
        // clean on stdout, keepalive lines being stderr's business alone.
        let waited = run_argv(
            "200",
            "{\"count\":0}",
            "team-1",
            &["check", "--wait", "--timeout-ms", "60000"],
        );
        assert!(
            waited.status.success(),
            "{}",
            String::from_utf8_lossy(&waited.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&waited.stdout), "{\"count\":0}");
        let saw = std::fs::read_to_string(&sink).expect("argv sink");
        let budget = saw.lines().skip_while(|line| *line != "--max-time").nth(1);
        assert_eq!(
            budget,
            Some("68"),
            "the script did not stretch curl's patience to budget + grace:\n{saw}"
        );
        // And a bare `ask` — no flag anywhere — arrives already patient: the
        // ledger's ten-minute default plus the same grace, witnessed off the
        // same argv.
        let asked = run_argv(
            "200",
            "{\"answered\":true}",
            "team-1",
            &["ask", "--body", "which-way?"],
        );
        assert!(
            asked.status.success(),
            "{}",
            String::from_utf8_lossy(&asked.stderr)
        );
        let saw = std::fs::read_to_string(&sink).expect("argv sink");
        let patient = saw.lines().skip_while(|line| *line != "--max-time").nth(1);
        let ask_wait = (crate::orchestration::ASK_BUDGET_DEFAULT_MS / 1000
            + SHIM_WAIT_GRACE_SECONDS)
            .to_string();
        assert_eq!(
            patient.map(str::to_string),
            Some(ask_wait),
            "a bare ask did not carry the default question patience:\n{saw}"
        );
        let started = run_argv(
            "200",
            "{\"workerId\":\"w-1\"}",
            "team-1",
            &["worker-start", "--agent", "codex"],
        );
        assert!(
            started.status.success(),
            "{}",
            String::from_utf8_lossy(&started.stderr)
        );
        let saw = std::fs::read_to_string(&sink).expect("argv sink");
        let patient = saw.lines().skip_while(|line| *line != "--max-time").nth(1);
        let worker_wait = (crate::orchestration::READY_TIMEOUT_DEFAULT_MS / 1000
            + SHIM_WAIT_GRACE_SECONDS)
            .to_string();
        assert_eq!(
            patient.map(str::to_string),
            Some(worker_wait),
            "a bare worker-start did not carry the default readiness patience:\n{saw}"
        );
    }

    /// The PowerShell shim, run as what it is — on the one platform that has
    /// its interpreter. The content test above runs everywhere; this one is
    /// the Windows half of the same witness, against a real listener rather
    /// than a fake curl, because the script starts no external program for a
    /// fake to stand in for.
    #[cfg(windows)]
    #[test]
    fn the_powershell_shim_runs_and_answers_a_real_reply() {
        use std::io::{Read, Write};
        let root = std::env::temp_dir().join("zerocode-agent-teams-ps-shim-test");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch");
        let shim = root.join("zerocode-orc.ps1");
        std::fs::write(
            &shim,
            shim_script_powershell("PORT_V", "TOKEN_V", "orchestration"),
        )
        .expect("shim");
        let listening = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listening.local_addr().expect("addr").port();
        let serving = std::thread::spawn(move || {
            let (mut socket, _) = listening.accept().expect("accept");
            let mut asked = Vec::new();
            let mut chunk = [0u8; 4096];
            // Read the whole request — headers, then as many body bytes as
            // content-length promised — before answering. An answer sent while
            // the client is still writing its body is a reset, not a reply.
            loop {
                let got = socket.read(&mut chunk).expect("read");
                asked.extend_from_slice(&chunk[..got]);
                let Some(split) = asked
                    .windows(4)
                    .position(|four| four == b"\r\n\r\n")
                    .map(|at| at + 4)
                else {
                    assert!(got != 0, "the request ended before its headers did");
                    continue;
                };
                let head = String::from_utf8_lossy(&asked[..split]).to_lowercase();
                let promised: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .map(|count| count.trim().parse().expect("content-length"))
                    .unwrap_or(0);
                if asked.len() >= split + promised {
                    break;
                }
                assert!(got != 0, "the request ended before its body did");
            }
            let head = String::from_utf8_lossy(&asked).to_string();
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-length: 8\r\nconnection: close\r\n\r\n{\"ok\":1}",
                )
                .expect("answer");
            head
        });
        let ran = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&shim)
            .args(["check", "--wait"])
            .env("PORT_V", port.to_string())
            .env("TOKEN_V", "tok")
            .env(TEAM_ID_VAR, "team-1")
            .env(TEAM_TOKEN_VAR, "pane-secret")
            .env(TEAM_PANE_VAR, "%2")
            .output()
            .expect("powershell");
        let head = serving.join().expect("served");
        assert!(
            ran.status.success(),
            "{}",
            String::from_utf8_lossy(&ran.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&ran.stdout), "{\"ok\":1}");
        // The headers arrived; the argv rode the body as a separated blob.
        assert!(head.contains("x-zerocode-hook-token: tok"), "{head}");
        assert!(
            head.contains("x-zerocode-team-token: pane-secret"),
            "{head}"
        );
        assert!(head.contains("x-zerocode-team-id: team-1"), "{head}");
        assert!(head.contains("x-zerocode-team-pane: %2"), "{head}");
    }

    #[test]
    fn the_version_and_the_one_option_are_answered_so_the_session_starts() {
        let mut team = team();
        assert_eq!(
            plan(&mut team, &words("-V"), LEADER_PANE).reply.stdout,
            TMUX_VERSION
        );
        assert_eq!(
            plan(
                &mut team,
                &words("show-options -g extended-keys"),
                LEADER_PANE
            )
            .reply
            .stdout,
            "extended-keys on\n"
        );
        assert_eq!(
            plan(
                &mut team,
                &words("show-options -gv extended-keys"),
                LEADER_PANE
            )
            .reply
            .stdout,
            "on\n"
        );
        // Any other option is a refusal — an invented value is worse than a
        // no.
        assert_eq!(
            plan(&mut team, &words("show-options -g status"), LEADER_PANE)
                .reply
                .exit_code,
            1
        );
    }
}
