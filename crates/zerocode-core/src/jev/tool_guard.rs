//! The two tool guards' shared half (t-6348; moved here by t-10916): what the
//! command guard and the tool text guard ask, today's rule each is held
//! against, the places a command names and the stamps its run moves, what
//! became of a command or a text, and the rows both seats write — one reading
//! of all of it for every program that asks the two seats. zo asks right
//! before it runs a shell command or hands a tool's text to its model; the
//! window asks when an agent in one of its panes reports the same moment
//! through its hooks.
//!
//! Nothing here asks the wire, reads a setting or holds a book. Which calls a
//! host reads, when it asks, and where it keeps what waits on hindsight are
//! the host's own; what a question says, what a row carries and what a label
//! makes of a fact are this file's, so a row asked in one program and a row
//! asked in the other are one seat's rows.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::COMMAND_GUARD_REGRET_TURNS;
use super::promote;
use super::questions::{
    COMMAND_GUARD_QUESTIONS, COMMAND_GUARD_STATE_KEYS, INSTRUCTED, TOOL_TEXT_GUARD_STATE_KEYS,
    TOOL_TEXT_INSTRUCTED_ASKS, TOOL_TEXT_INSTRUCTED_NO, TOOL_TEXT_INSTRUCTED_YES,
};
use super::summary::LABEL_ROW_KIND;
use crate::guarded::{self, ControlKind};
use crate::shell_rule::scope::first_workspace_scope_violation;
use crate::shell_rule::{
    CommandIntent, ValidationResult, check_destructive, classify_command, path_within_root,
    proven_read_only, reaches_outside_workspace, split_command_segments,
};

/// The shortest words a next call must share with a text before it counts as
/// carrying the text out. Under it a match is a word the text and the call
/// share by chance — `ls`, `cargo test` — and not an order followed: twelve
/// characters is a command with an argument or a path of a few parts. A
/// policy line, not a measured one.
pub const FOLLOWED_MIN_CHARS: usize = 12;

/// Paths one command's stamps watch — outside the project for what changed
/// under it, and among every place it names for what it changed (t-9087) —
/// more than a command with a list of arguments names, few enough that
/// stamping them before it runs and after costs its path nothing it would
/// feel (a `stat` each).
pub const WATCHED_PATHS_CAP: usize = 16;

/// Calls one project's book holds while they wait on their hindsight. A
/// runtime whose turns no host labels — a sub-agent's — would otherwise keep
/// every call it made; past this the oldest go unlabeled.
pub const BOOK_CAP: usize = 512;

/// The POSIX temporary folder: a command's scratch writes there are not the
/// project's, and not outside it either (the second Noul's own words). The
/// per-user temporary folder is read from the system ([`std::env::temp_dir`]).
pub const SHARED_TEMP_DIR: &str = "/tmp";

/// The git verbs that put a path back as it was — `git restore <path>`,
/// `git checkout [<rev>] -- <path>`. A later command of these putting back
/// what a guarded command changed ([`restores`]) is that command's regret.
pub const RESTORING_GIT_VERBS: [&str; 2] = ["restore", "checkout"];

/// The word a text label names a followed block by, and an ignored one.
pub const FOLLOWED: &str = "followed";
pub const IGNORED: &str = "ignored";

/// The input keys an edit or a write carries the words it writes under — what
/// a call of the step after a text carries out when it is not a shell command.
pub const WRITTEN_WORDS_KEYS: [&str; 2] = ["content", "new_string"];

/// What every MCP tool's name opens with — the prefix an agent hosting MCP
/// writes before the server's name (`mcp__<server>__<tool>`): an MCP tool's
/// answer is a text the text guard reads.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/* ---- the kind of text, and what its host says of its fence ------------------ */

/// The kind of tool a block the text guard reads came from — the `source` its
/// state names, so the judgment knows a web page from a file of the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSource {
    /// A file the agent read (`read_file`).
    File,
    /// A page or a search a web tool fetched (`WebFetch`, `WebSearch`).
    Web,
    /// An answer the window put inside its fence before the shell tool handed
    /// it back — the window's browser above all (`zerocode-browser read`),
    /// and the emulator's and an issue tracker's words, which arrive the same
    /// way.
    Browser,
    /// What an MCP server's tool or resource handed back.
    Mcp,
}

impl TextSource {
    /// The word the state and a row name this kind by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Web => "web",
            Self::Browser => "browser",
            Self::Mcp => "mcp",
        }
    }
}

/// What the host that hands a block to the model can say of the fence around
/// it — the host's own word, never the block's bytes (t-6982). One fact, two
/// readers asking two questions of it (t-7058): the acting guard skips its
/// own fence only on [`Self::Fenced`], and today's rule is graded on the fact
/// where there is one ([`todays_text_rule`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFraming {
    /// A host attested, out of band, that the block reached the model inside
    /// its fence. Nothing in this runtime says so today: a shell answer that
    /// looks like the window's is bytes, and bytes attest to nothing.
    Fenced,
    /// This runtime handed the bytes over bare: its own file, web and MCP
    /// tools put nothing around their answers before the guard.
    Unfenced,
    /// The bytes may carry another host's fence — the window's browser CLI's —
    /// which this runtime cannot verify. Nothing is known, and nothing is
    /// guessed: no fence is skipped on it, and no rule is graded on it.
    Unknown,
}

impl HostFraming {
    /// The word a row carries.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Fenced => "fenced",
            Self::Unfenced => "unfenced",
            Self::Unknown => "unknown",
        }
    }

    /// What zo's runtime can say at its own seam of a block of `source`: it
    /// fenced none of its own tools' answers, and it cannot read another
    /// host's fence off a shell answer's bytes.
    #[must_use]
    pub const fn at_this_runtimes_seam(source: TextSource) -> Self {
        match source {
            TextSource::File | TextSource::Web | TextSource::Mcp => Self::Unfenced,
            TextSource::Browser => Self::Unknown,
        }
    }
}

/* ---- the question: state, Nouls, and what an answer says -------------------- */

/// Whether the command guard asks about `command` at all: a command today's
/// rules prove read-only is not asked — every segment a program the intent
/// table knows reads only ([`classify_command`]) and nothing in it that
/// writes, redirects or escapes ([`proven_read_only`]). The product already
/// knows what that one does, and a question whose answer code has is not one
/// to pay the wire for. The read-only check alone is not the proof: it passes
/// a program it has no row for — `terraform destroy`, `redis-cli FLUSHALL` —
/// which is exactly the command the guard is for.
#[must_use]
pub fn asks_about(command: &str) -> bool {
    let read_only_proven =
        classify_command(command) == CommandIntent::ReadOnly && proven_read_only(command);
    !command.trim().is_empty() && !read_only_proven
}

/// The task line a command is judged for: the first line with words of the
/// person's newest message — what the person said the turn is for.
#[must_use]
pub fn task_line_of(persons_words: &str) -> String {
    persons_words
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

/// The command guard's state: the command, its folder, the task line — under
/// the catalog's keys. The door cuts each to the use table's caps.
#[must_use]
pub fn command_state(command: &str, cwd: &Path, task: &str) -> Value {
    let [command_key, cwd_key, task_key] = COMMAND_GUARD_STATE_KEYS;
    Value::Object(Map::from_iter([
        (command_key.to_string(), Value::from(command)),
        (
            cwd_key.to_string(),
            Value::from(cwd.to_string_lossy().into_owned()),
        ),
        (task_key.to_string(), Value::from(task)),
    ]))
}

/// The command guard's two Nouls, by the ids their answers come back under,
/// each built by the asking program's own constructor for a Noul (`noul` —
/// zo's typed question, the window's [`super::noul::question`]) out of the
/// catalog's words.
#[must_use]
pub fn command_questions<Q>(noul: impl Fn(&str, &str, &str) -> Q) -> BTreeMap<String, Q> {
    COMMAND_GUARD_QUESTIONS
        .iter()
        .map(|[id, asks, yes, no]| ((*id).to_string(), noul(asks, yes, no)))
        .collect()
}

/// The tool text guard's state: the kind of tool, and the head of its text.
#[must_use]
pub fn text_state(source: TextSource, head: &str) -> Value {
    let [source_key, text_key] = TOOL_TEXT_GUARD_STATE_KEYS;
    Value::Object(Map::from_iter([
        (source_key.to_string(), Value::from(source.word())),
        (text_key.to_string(), Value::from(head)),
    ]))
}

/// The tool text guard's one Noul — the screen guard's, asked of a block —
/// built by the asking program's own constructor, as [`command_questions`]'s
/// are.
#[must_use]
pub fn text_questions<Q>(noul: impl Fn(&str, &str, &str) -> Q) -> BTreeMap<String, Q> {
    BTreeMap::from([(
        INSTRUCTED.to_string(),
        noul(
            TOOL_TEXT_INSTRUCTED_ASKS,
            TOOL_TEXT_INSTRUCTED_YES,
            TOOL_TEXT_INSTRUCTED_NO,
        ),
    )])
}

/// What the code made of one answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// A Noul reached the guard's yes line: the command may not be undone or
    /// reaches outside the project; the text addresses the agent.
    Flagged,
    /// Every Noul stayed under it.
    Plain,
    /// Nothing answered — the call reads as it did before the seat.
    Unavailable,
}

impl Verdict {
    /// The word a row carries.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Flagged => "flagged",
            Self::Plain => "plain",
            Self::Unavailable => "unavailable",
        }
    }

    /// The verdict `answers` come to against `floor_permille`, read in the
    /// floor's own units ([`promote::permille`]).
    #[must_use]
    pub fn of(answers: Option<&BTreeMap<String, f64>>, floor_permille: u16) -> Self {
        match answers {
            None => Self::Unavailable,
            Some(answers)
                if answers
                    .values()
                    .any(|yes| promote::permille(*yes) >= floor_permille) =>
            {
                Self::Flagged
            }
            Some(_) => Self::Plain,
        }
    }

    /// Whether a later fact that says yes — regretted, followed — is what
    /// this verdict called; `None` for an answer that was never given.
    #[must_use]
    pub const fn agrees_with(self, fact: bool) -> Option<bool> {
        match self {
            Self::Flagged => Some(fact),
            Self::Plain => Some(!fact),
            Self::Unavailable => None,
        }
    }
}

/// How sure the answer that decided the verdict was: the lean `|2p − 1|` of
/// its highest yes — the band a Noul seat reads (`ConfidenceBands::on_a_noul`).
#[must_use]
pub fn confidence_of(answers: &BTreeMap<String, f64>) -> Option<f64> {
    answers
        .values()
        .copied()
        .fold(None, |most: Option<f64>, yes| {
            Some(most.map_or(yes, |most| most.max(yes)))
        })
        .map(|yes| (2.0 * yes - 1.0).abs())
}

/* ---- the rows --------------------------------------------------------------- */

/// What one request of either guard came to — the facts every row of both
/// ledgers carries, flattened into it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asked {
    /// The door's answered outcome, a failure's ledger token, or the door's
    /// refusal token.
    pub outcome: String,
    /// Each Noul's probability of yes, on an answered row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answers: Option<BTreeMap<String, f64>>,
    /// Which reader the call went on with: the guard's
    /// ([`super::ROUTE_USE_APPLIED`]), a recording mode's word, or the call
    /// alone ([`super::ROUTE_USE_FALLBACK`]).
    pub route_use: String,
    /// Whether the guard acted on an answered question.
    pub applied: bool,
    pub elapsed_ms: u64,
    pub retries: u32,
    /// Requests this question sent: none when the door refused it.
    pub requests: u32,
    /// Lines the door withheld from what was sent.
    pub redacted_lines: u32,
    /// The model that answered, as the response named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Bytes of the body the door let through — what one question costs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_bytes: Option<usize>,
    /// The request's receipt ([`super::digest_of`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    /// Which of a reply's rules refused it, on a row whose `outcome` is
    /// `schema`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejected: Option<String>,
    /// The answer came from the memo, for bytes asked before: no request left
    /// ([`crate::jev::summary::CACHED`] — out of the latency population).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cached: bool,
}

/// What a command row says of when a window pane's command was asked: right
/// before it ran — or, for an agent whose hooks carry nothing before a tool
/// runs, after (`crate::hook_guard::Unseen::NoEventBefore`).
pub const ASKED_BEFORE: &str = "before";
pub const ASKED_AFTER: &str = "after";

/// One command's row: what was asked, what came back, what the code made of it
/// and whether a line joined the result. No words and no path: the command is
/// a fingerprint and its length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandGuardRow {
    /// Unix milliseconds when the row was made.
    pub at: u64,
    /// The turn the command ran inside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// Fingerprint of the turn and the call — what a label row names.
    pub judged: u64,
    pub rubric_version: u32,
    /// Fingerprint of the command.
    pub command: String,
    /// Characters of the command, before the door's cap.
    pub command_chars: usize,
    /// What today's rule made of the command: `flagged` or `plain`.
    pub rule: String,
    /// Paths it named outside the project, stamped for the label.
    pub outside_paths: usize,
    /// `flagged`, `plain` or `unavailable` — the code's judgment.
    pub verdict: String,
    /// Whether a line joined the result the model read.
    pub noted: bool,
    /// Who asked ([`crate::jev::summary::FROM`]): `zo`'s own runtime, or the
    /// agent of a window pane (t-10916) — so rows asked by every program
    /// that shares this seat's ledger keep their source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// For a pane's command, when it was asked ([`ASKED_BEFORE`],
    /// [`ASKED_AFTER`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moment: Option<String>,
    /// The folder the words came from, by its last name — the project a
    /// row of this machine's one ledger belongs to, as the window's other
    /// seats name a pane's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(flatten)]
    pub asked: Asked,
}

/// One command's hindsight, shaped like the other hindsight seats' labels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)] // each bool is a column the ledger keeps — the act, the two marks, the run's own failure — not a state machine
pub struct CommandGuardLabelRow {
    pub kind: String,
    pub at: u64,
    /// The row this grades — its `judged` fingerprint, spelled as text.
    pub label: String,
    pub verdict: String,
    pub applied: bool,
    /// Whether the verdict called what became of the command.
    pub agreed: bool,
    /// Whether today's rule did.
    pub baseline_agreed: bool,
    /// What settled it: `stopped`, `outside` or `restored` for a regretted
    /// command, `stood` for one whose window passed (`CommandHindsight`).
    pub hindsight: String,
    /// Whether the command's own result was an error — recorded, not graded.
    pub failed: bool,
    /// Turns after the command's own at which it was settled.
    pub turns_later: u32,
    /// The deciding answer's lean ([`confidence_of`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/// What became of a command — its own facts, never the turn's alone
/// (t-10916).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandHindsight {
    /// The person stopped this call: Esc while it ran — a call its host
    /// reports stopped, or one still running when the person stopped the
    /// turn — or refused it when asked. A turn the person stopped is not by
    /// itself a regret of the commands that had finished in it: 35 of 35 of
    /// this seat's marks on 2026-09-27 were such turns, a question about the
    /// turn graded as one about the command.
    Stopped,
    /// A path it named outside the project changed while it ran.
    Outside,
    /// A later command put a path it named back.
    Restored,
    /// Its window passed with none of those.
    Stood,
}

impl CommandHindsight {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Outside => "outside",
            Self::Restored => "restored",
            Self::Stood => "stood",
        }
    }

    /// Whether this is a regret — the fact a `flagged` verdict calls.
    #[must_use]
    pub const fn regretted(self) -> bool {
        !matches!(self, Self::Stood)
    }
}

/// One block's row. No words: the block is its tool, its kind and its length.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTextGuardRow {
    pub at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    pub judged: u64,
    pub rubric_version: u32,
    /// The tool that handed the block back.
    pub tool: String,
    /// Its kind — the state's `source`.
    pub source: String,
    /// Characters of the whole block.
    pub text_chars: usize,
    /// What the host said of the fence around it before the model read it
    /// ([`HostFraming::word`]) — what today's rule is graded on.
    pub framing: String,
    /// `flagged`, `plain` or `unavailable`.
    pub verdict: String,
    /// Whether the guard put the block inside the fence.
    pub fenced: bool,
    /// Whether a line joined the result.
    pub noted: bool,
    /// Who asked, as a command row says it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The folder the words came from, by its last name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    #[serde(flatten)]
    pub asked: Asked,
}

/// One block's hindsight: whether the block held an order to the agent and
/// what the agent did with it, written apart (t-10916) — a block the next
/// step left alone says nothing of whether an order was there, and 45 of 45
/// of this seat's marks on 2026-09-27 read such blocks as plain ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTextGuardLabelRow {
    pub kind: String,
    pub at: u64,
    pub label: String,
    pub verdict: String,
    pub applied: bool,
    /// Whether the verdict called whether the block held an order — marked
    /// only where hindsight proves one ([`Self::instructed`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agreed: Option<bool>,
    /// Whether today's rule — the host's fence — did; absent where the host
    /// could not say what it fenced ([`todays_text_rule`]) or hindsight
    /// proved nothing, so the judge counts no mark there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_agreed: Option<bool>,
    /// Whether the block held an order to the agent, as far as hindsight
    /// proves: `true` where the next step carried out words only the block
    /// spelled; absent where it did not, which proves nothing either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructed: Option<bool>,
    /// Whether the next step carried the block out — what the agent did.
    pub followed: bool,
    /// Why the row carries no mark ([`crate::jev::summary::NOT_COMPARED`]):
    /// [`IGNORED`], a block the next step left alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_compared: Option<String>,
    /// What the host said of the fence — the rule's input, beside its mark.
    pub framing: String,
    /// `followed` or `ignored`.
    pub hindsight: String,
    /// The tool of the call that carried the block out, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f64>,
}

/* ---- the command guard's reading of a command ------------------------------- */

/// Today's rule on a command — the readers the product already has: zo's
/// destructive warnings and hard blocks and its intent table, the shared-tree
/// table, the Computer Use words a control that cannot be taken back carries,
/// and the path rules. Each is asked, none is copied. `(cannot be undone,
/// reaches outside)`.
#[must_use]
pub fn todays_rule(command: &str, cwd: &Path) -> (bool, bool) {
    let irreversible = check_destructive(command) != ValidationResult::Allow
        || classify_command(command) == CommandIntent::Destructive
        || first_workspace_scope_violation(command).is_some()
        || guarded::kind_of(command) == ControlKind::Destructive;
    (irreversible, reaches_outside_workspace(command, cwd))
}

/// A path's state as a label compares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stamp {
    /// Nothing is there.
    Absent,
    /// Something is: whether it is a folder, its length and when it last
    /// changed.
    Present {
        dir: bool,
        len: u64,
        modified: Option<SystemTime>,
    },
}

impl Stamp {
    /// What stands at the path, if anything: a folder or not — the entry
    /// itself, whatever is in it or written to it.
    const fn entry(self) -> Option<bool> {
        match self {
            Self::Absent => None,
            Self::Present { dir, .. } => Some(dir),
        }
    }
}

/// A place a command named and its run changed, as a later restore is
/// matched on it (t-9087).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    /// The place.
    pub path: PathBuf,
    /// The entry itself moved — the run made it, removed it, or turned a
    /// folder into a file or back — and took every path under it along. A
    /// folder whose listing or time alone moved had one of its children
    /// change and says nothing of which one (astra R-GUARD-1: `cd <root> &&
    /// touch other.tmp` moves the root's stamp and leaves `a.rs` as it was).
    pub entry_moved: bool,
}

impl Changed {
    /// How `path`'s stamp moved from `before` to `after`, if it moved.
    #[must_use]
    pub fn between(path: &Path, before: Stamp, after: Stamp) -> Option<Self> {
        (after != before).then(|| Self {
            path: path.to_path_buf(),
            entry_moved: after.entry() != before.entry(),
        })
    }

    /// Whether a restore of `restored` puts back what this change did: the
    /// place itself, a folder holding it, or — only where the entry itself
    /// moved — a path under it.
    fn put_back_by(&self, restored: &Path) -> bool {
        self.path.starts_with(restored) || (self.entry_moved && restored.starts_with(&self.path))
    }
}

/// The stamp of `path`, or `None` for a place not worth watching — a device,
/// a socket, a pipe: `/dev/null` changes under every redirect and nothing of
/// the person's lives there.
#[must_use]
pub fn stamp(path: &Path) -> Option<Stamp> {
    match std::fs::symlink_metadata(path) {
        Err(_) => Some(Stamp::Absent),
        Ok(meta) => {
            let kind = meta.file_type();
            (kind.is_file() || kind.is_dir() || kind.is_symlink()).then(|| Stamp::Present {
                dir: kind.is_dir(),
                len: meta.len(),
                modified: meta.modified().ok(),
            })
        }
    }
}

/// One word of a command read as a place: quotes, a trailing `;` and a
/// redirect's operator taken off (`2>>~/.log` is `~/.log`). `None` for a flag
/// and for nothing left.
fn place_word(word: &str) -> Option<String> {
    let word = match word.find(['>', '<']) {
        Some(at) if word[..at].chars().all(|c| c.is_ascii_digit() || c == '&') => {
            word[at..].trim_start_matches(['>', '<', '&', '|'])
        }
        _ => word,
    };
    let word = word.trim_matches(|c| matches!(c, '"' | '\'' | ';' | '(' | ')'));
    (!word.is_empty() && !word.starts_with('-')).then(|| word.to_string())
}

/// The words of a command that name a place: each segment's arguments after
/// its program, each read as a place — quotes, a trailing `;` and a
/// redirect's operator taken off, flags left out: `rm -rf build` names
/// `build`, `echo x 2>> ~/.log` names `~/.log`.
#[must_use]
pub fn named_places(command: &str) -> Vec<String> {
    split_command_segments(command)
        .into_iter()
        .flat_map(|segment| segment.split_whitespace().skip(1))
        .filter_map(place_word)
        .collect()
}

/// A place as a path: `~`, `$HOME` and `${HOME}` read as the home folder, a
/// relative place against `cwd`. `None` for what cannot be stamped — a glob,
/// another variable, another person's home.
#[must_use]
pub fn resolve_place(place: &str, cwd: &Path) -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    let path = if place == "~" {
        home()?
    } else if let Some(rest) = ["~/", "$HOME/", "${HOME}/"]
        .iter()
        .find_map(|head| place.strip_prefix(head))
    {
        home()?.join(rest)
    } else if place.starts_with('~') || place.contains(['$', '*', '?', '[', '{', '`']) {
        return None;
    } else {
        PathBuf::from(place)
    };
    Some(if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    })
}

/// The folders a command's task owns: the folder it runs in, the checkout
/// that folder is in, the project the seat stands in, and the temporary
/// folders.
fn task_roots(cwd: &Path, project: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        cwd.to_path_buf(),
        project.to_path_buf(),
        std::env::temp_dir(),
    ];
    if let Some(checkout) = cwd
        .ancestors()
        .find(|folder| crate::git_dir::of(folder).is_some())
    {
        roots.push(checkout.to_path_buf());
    }
    let shared = Path::new(SHARED_TEMP_DIR);
    roots.push(shared.to_path_buf());
    if let Ok(real) = shared.canonicalize() {
        roots.push(real);
    }
    roots
}

/// The paths `command` names outside every root its task owns, at most
/// [`WATCHED_PATHS_CAP`] of them.
#[must_use]
pub fn outside_places(command: &str, cwd: &Path, project: &Path) -> Vec<PathBuf> {
    let roots = task_roots(cwd, project);
    let mut outside: Vec<PathBuf> = named_places(command)
        .iter()
        .filter_map(|place| resolve_place(place, cwd))
        .filter(|path| {
            let spelled = path.to_string_lossy();
            !roots.iter().any(|root| path_within_root(root, &spelled))
        })
        .collect();
    outside.dedup();
    outside.truncate(WATCHED_PATHS_CAP);
    outside
}

/// Whether a later shell command `later` puts back what the guarded command
/// changed — `changed`, the places it named whose stamps its run moved
/// (t-9087): a restoring git verb ([`RESTORING_GIT_VERBS`]) naming a changed
/// place, or a folder holding one, or a path under a place whose entry the
/// run made or removed ([`Changed::entry_moved`]). A path under a folder
/// whose listing alone moved is not one: the stamps say one of the folder's
/// children changed, never that it was this one.
#[must_use]
pub fn restores(later: &str, changed: &[Changed], cwd: &Path) -> bool {
    split_command_segments(later).into_iter().any(|segment| {
        let words: Vec<&str> = segment.split_whitespace().collect();
        let program = words
            .first()
            .map(|program| program.rsplit('/').next().unwrap_or(program));
        let verb = words
            .iter()
            .skip(1)
            .position(|word| !word.starts_with('-'))
            .map(|at| at + 1);
        let (Some("git"), Some(verb)) = (program, verb) else {
            return false;
        };
        RESTORING_GIT_VERBS.contains(&words[verb])
            && words[verb + 1..]
                .iter()
                .filter_map(|word| place_word(word))
                .filter_map(|place| resolve_place(&place, cwd))
                .any(|restored| changed.iter().any(|one| one.put_back_by(&restored)))
    })
}

/// The places `command` names, resolved against `cwd`, each named once and
/// stamped as it stands now — at most [`WATCHED_PATHS_CAP`] of them.
#[must_use]
pub fn stamped_places(command: &str, cwd: &Path) -> Vec<(PathBuf, Stamp)> {
    let mut stamped: Vec<(PathBuf, Stamp)> = Vec::new();
    for path in named_places(command)
        .iter()
        .filter_map(|place| resolve_place(place, cwd))
    {
        if stamped.len() == WATCHED_PATHS_CAP {
            break;
        }
        if stamped.iter().any(|(seen, _)| *seen == path) {
            continue;
        }
        if let Some(before) = stamp(&path) {
            stamped.push((path, before));
        }
    }
    stamped
}

/* ---- the tool text guard's reading of a text -------------------------------- */

/// Today's rule on a block — the fence the host put around it before the
/// model read it, as the host itself says ([`HostFraming`]): flagged when it
/// stood inside one, plain when this runtime's own tool handed it over bare,
/// and nothing at all when the host cannot say — a shell answer carrying
/// another host's marker (t-7058). The label writer grades this against what
/// the next step did, and the replay counts it on the synthetic cases; a
/// block it says nothing of carries no baseline mark, so the judge's
/// baseline count leaves it out rather than reading a guess as a `plain`.
///
/// Version 1 of the rubric read "fenced before" off the block's own bytes
/// (a phrase in the body), version 2 held it at `false` for every block —
/// the same constant-plain mark on a file the runtime handed over bare and
/// on a browser answer the window had wrapped — and version 3 is this word.
#[must_use]
pub const fn todays_text_rule(framing: HostFraming) -> Option<bool> {
    match framing {
        HostFraming::Fenced => Some(true),
        HostFraming::Unfenced => Some(false),
        HostFraming::Unknown => None,
    }
}

/// `text` with every run of whitespace one space — so a command copied out of
/// a wrapped paragraph still matches.
#[must_use]
pub fn squeezed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `words` — what one call of the agent's next step carried out, a
/// shell command or the words an edit wrote, squeezed — carry out the block
/// `block` spells: at least [`FOLLOWED_MIN_CHARS`] long, spelled by the block
/// and not by the person's words `persons` (both squeezed as well).
#[must_use]
pub fn carries_out(words: &str, block: &str, persons: &str) -> bool {
    words.chars().count() >= FOLLOWED_MIN_CHARS && block.contains(words) && !persons.contains(words)
}

/* ---- what waits on hindsight ------------------------------------------------ */

/// One command a host keeps while it waits on its hindsight: what its label
/// needs, as it arrives.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)] // each bool is an independent fact about one command, not a state machine
pub struct CommandWaiting {
    pub judged: u64,
    /// Whose later turns settle it — a runtime session, a pane.
    pub owner: String,
    /// The call that ran it.
    pub tool_use_id: String,
    pub cwd: PathBuf,
    /// The places the command named, as paths, each with its stamp before the
    /// run — at most [`WATCHED_PATHS_CAP`] of them.
    pub named: Vec<(PathBuf, Stamp)>,
    /// The named places whose stamp the run moved — what a later restore is
    /// matched on ([`restores`], t-9087).
    pub changed: Vec<Changed>,
    /// The places outside the project, each with its stamp before the run.
    pub outside: Vec<(PathBuf, Stamp)>,
    /// Today's rule flagged it.
    pub rule_flagged: bool,
    pub verdict: Option<Verdict>,
    pub confidence: Option<f64>,
    pub applied: bool,
    pub failed: bool,
    pub cancelled: bool,
    pub changed_outside: bool,
    pub turns: u32,
    pub decided: Option<CommandHindsight>,
}

impl CommandWaiting {
    /// A command about to run in `cwd` for the project `project`: what it
    /// names stamped as it stands now — outside the project for what changes
    /// under it, and every place for what it changes — and today's rule on
    /// it.
    #[must_use]
    pub fn asked(
        judged: u64,
        owner: &str,
        tool_use_id: &str,
        command: &str,
        cwd: &Path,
        project: &Path,
    ) -> Self {
        let named = stamped_places(command, cwd);
        let outside: Vec<(PathBuf, Stamp)> = outside_places(command, cwd, project)
            .into_iter()
            .filter_map(|path| stamp(&path).map(|before| (path, before)))
            .collect();
        let (irreversible, reaches) = todays_rule(command, cwd);
        Self {
            judged,
            owner: owner.to_string(),
            tool_use_id: tool_use_id.to_string(),
            cwd: cwd.to_path_buf(),
            named,
            changed: Vec::new(),
            outside,
            rule_flagged: irreversible || reaches,
            verdict: None,
            confidence: None,
            applied: false,
            failed: false,
            cancelled: false,
            changed_outside: false,
            turns: 0,
            decided: None,
        }
    }

    /// What today's rule made of it, as a row spells it.
    #[must_use]
    pub const fn rule(&self) -> Verdict {
        if self.rule_flagged {
            Verdict::Flagged
        } else {
            Verdict::Plain
        }
    }

    /// The command has run: its facts, and its paths stamped again — what
    /// moved outside the project, and which named places it changed.
    pub fn ran(&mut self, failed: bool, cancelled: bool) {
        self.failed = failed;
        self.cancelled = cancelled;
        self.changed_outside = self
            .outside
            .iter()
            .any(|(path, before)| stamp(path).is_some_and(|after| after != *before));
        self.changed = self
            .named
            .iter()
            .filter_map(|(path, before)| Changed::between(path, *before, stamp(path)?))
            .collect();
    }

    /// A turn of its owner's ended: `later` the shell commands that ran after
    /// it in that turn — every one, for a command of an earlier turn — or
    /// `None` for a turn the person stopped. Settles what became of it once a
    /// fact of its own says — the call stopped, a place outside the project
    /// changed under it, a restore of what it changed — and counts the turn
    /// otherwise. A stopped turn settles nothing by itself and is none of its
    /// turns: it says the person stopped the turn, not this command
    /// ([`CommandHindsight::Stopped`], t-10916).
    pub fn settle_turn(&mut self, later: Option<&[&str]>) {
        if self.decided.is_some() {
            return;
        }
        // What the run changed, not what it spelled (t-9087).
        let restored = later.is_some_and(|later| {
            later
                .iter()
                .any(|command| restores(command, &self.changed, &self.cwd))
        });
        self.decided = if self.cancelled {
            Some(CommandHindsight::Stopped)
        } else if self.changed_outside {
            Some(CommandHindsight::Outside)
        } else if restored {
            Some(CommandHindsight::Restored)
        } else if later.is_none() {
            None
        } else if self.turns >= COMMAND_GUARD_REGRET_TURNS {
            Some(CommandHindsight::Stood)
        } else {
            self.turns += 1;
            None
        };
    }

    /// Its label, written at `at` — `None` until both its verdict and its
    /// hindsight are in, and for a verdict nothing answered.
    #[must_use]
    pub fn label(&self, at: u64) -> Option<CommandGuardLabelRow> {
        let (verdict, hindsight) = (self.verdict?, self.decided?);
        Some(CommandGuardLabelRow {
            kind: LABEL_ROW_KIND.to_string(),
            at,
            label: self.judged.to_string(),
            verdict: verdict.word().to_string(),
            applied: self.applied,
            agreed: verdict.agrees_with(hindsight.regretted())?,
            baseline_agreed: self.rule_flagged == hindsight.regretted(),
            hindsight: hindsight.word().to_string(),
            failed: self.failed,
            turns_later: self.turns,
            confidence: self.confidence,
        })
    }
}

/// One block a host keeps while it waits on what the next step did with it.
#[derive(Debug, Clone)]
pub struct TextWaiting {
    pub judged: u64,
    pub owner: String,
    pub tool_use_id: String,
    pub framing: HostFraming,
    pub verdict: Option<Verdict>,
    pub confidence: Option<f64>,
    pub applied: bool,
    /// Whether the next step followed it, and the tool that did.
    pub decided: Option<(bool, Option<String>)>,
}

impl TextWaiting {
    /// Its label, written at `at` — `None` until both its verdict and what
    /// the next step did are in, and for a verdict nothing answered.
    ///
    /// The question is whether the block held an order to the agent. A next
    /// step that carried out words only the block spelled proves one was
    /// there; one that did not proves nothing either way — the agent may
    /// have seen through it — so that row carries no mark for the verdict or
    /// for today's rule, and says why ([`IGNORED`]).
    #[must_use]
    pub fn label(&self, at: u64) -> Option<ToolTextGuardLabelRow> {
        let (verdict, (followed, next_tool)) = (self.verdict?, self.decided.clone()?);
        if verdict == Verdict::Unavailable {
            return None;
        }
        let instructed = followed.then_some(true);
        Some(ToolTextGuardLabelRow {
            kind: LABEL_ROW_KIND.to_string(),
            at,
            label: self.judged.to_string(),
            verdict: verdict.word().to_string(),
            applied: self.applied,
            agreed: instructed.and_then(|fact| verdict.agrees_with(fact)),
            // Today's rule is the fence the host put around the block
            // before the model read it, graded where the host could say
            // ([`todays_text_rule`]) and hindsight proved an order: agreed
            // when the host had fenced it, disagreed when it had left it
            // bare, and no mark on a block whose framing it does not know.
            baseline_agreed: instructed
                .zip(todays_text_rule(self.framing))
                .map(|(fact, flags)| flags == fact),
            instructed,
            followed,
            not_compared: instructed.is_none().then(|| IGNORED.to_string()),
            framing: self.framing.word().to_string(),
            hindsight: if followed { FOLLOWED } else { IGNORED }.to_string(),
            next_tool,
            confidence: self.confidence,
        })
    }
}

/// Push `one` onto a book's list, dropping the oldest past [`BOOK_CAP`].
pub fn shelve<W>(waiting: &mut Vec<W>, one: W) {
    waiting.push(one);
    if waiting.len() > BOOK_CAP {
        let over = waiting.len() - BOOK_CAP;
        waiting.drain(..over);
    }
}

#[cfg(test)]
mod tests;
