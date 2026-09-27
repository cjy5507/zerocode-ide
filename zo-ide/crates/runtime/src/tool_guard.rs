//! The tool guards' runtime half (t-6348): what the runtime hands the two
//! guards and what it does with what they hand back — the shell command a turn
//! is about to run (`smart.jevCommandGuard`) and the text a tool hands back
//! (`smart.jevToolTextGuard`).
//!
//! Nothing here calls anything. It names which calls each guard reads, builds
//! what a seat is handed, and applies what an acting guard hands back. The
//! questions, the door, the wire, the rows, the labels and the judge are the
//! tools crate's (`smart_router::tool_guard`); the runtime reaches them through
//! the [`ToolGuardSeat`] a host installs, as it reaches the patch review.
//!
//! # Nothing gets worse for asking
//!
//! A seat that is off, that only records, that was refused, failed or missed
//! its wall hands back nothing, and the result reads as it did without the
//! seat, to the byte. A seat that acts adds one line — and around a text it
//! read as an order to the agent, the one fence for words an agent did not
//! write ([`zerocode_core::untrusted`]). Neither guard stops a command or
//! refuses a read: this product is not a sandbox, and a guard that claimed to
//! have stopped something would be claiming what it cannot do.

use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use serde::Deserialize;
use zerocode_core::jev::door::cut;
use zerocode_core::jev::{Cap, TOOL_TEXT_GUARD_TEXT_CHAR_CAP};
use zerocode_core::untrusted;

use crate::context_compression::{wire_tool_output, WireRewrite};
use crate::patch_review::persons_words;
use crate::session::ConversationMessage;
use zerocode_core::jev::tool_guard::{asks_about, task_line_of};
// The kind of a text and what its host says of its fence are the two guards'
// shared words (t-10916): the window asks the same seats of its panes.
pub use zerocode_core::jev::tool_guard::{HostFraming, TextSource};

/// zo's shell tool — the one tool the command guard reads, and the one whose
/// answers the window's fence arrives in.
pub const SHELL_TOOL: &str = "bash";

/// What the one line an acting command guard adds to a result opens with.
pub const COMMAND_GUARD_NOTE_PREFIX: &str = "[zo:command-guard]";

/// What the one line an acting tool text guard adds to a result opens with.
pub const TOOL_TEXT_GUARD_NOTE_PREFIX: &str = "[zo:tool-text-guard]";

/// The tools whose answers the text guard reads by name, each with the kind
/// it names. MCP tools are read by their prefix and the window's answers by
/// their fence ([`text_source_of`]).
pub const TEXT_TOOLS: [(&str, TextSource); 4] = [
    ("read_file", TextSource::File),
    ("WebFetch", TextSource::Web),
    ("WebSearch", TextSource::Web),
    ("ReadMcpResource", TextSource::Mcp),
];

/// What every MCP tool's name opens with.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// The kind of source a tool's own `output` is, for a text the guard reads —
/// `None` for every other tool, and for a shell answer the window did not
/// fence (a shell's own output is the agent's command speaking, not a page).
#[must_use]
pub fn text_source_of(tool_name: &str, output: &str) -> Option<TextSource> {
    if let Some((_, source)) = TEXT_TOOLS.iter().find(|(name, _)| *name == tool_name) {
        return Some(*source);
    }
    if tool_name.starts_with(MCP_TOOL_PREFIX) {
        return Some(TextSource::Mcp);
    }
    // A shell answer with this marker is a candidate browser result. This is
    // source classification only: its bytes cannot attest to host framing.
    (tool_name == SHELL_TOOL && output.contains(untrusted::PHRASE)).then_some(TextSource::Browser)
}

/// What the command guard is handed right before a shell command runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandAsk {
    /// The turn the command runs inside, as the runtime names it.
    pub attempt: String,
    /// The runtime session whose future turns can settle this command.
    pub owner: String,
    /// The call that runs it.
    pub tool_use_id: String,
    /// The command, whole — the seat's door cuts what is sent to the use
    /// table's cap; the label reads the whole.
    pub command: String,
    /// The folder the shell executor uses, after its input/context fallback.
    pub cwd: PathBuf,
    /// The first line of the person's newest words ([`task_line`]).
    pub task: String,
}

/// What became of a command the guard was asked about, once it ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRan {
    pub owner: String,
    pub tool_use_id: String,
    /// The tool handed back an error.
    pub failed: bool,
    /// The person stopped it while it ran (Esc).
    pub cancelled: bool,
}

/// What the text guard is handed for one block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextAsk {
    /// The turn the read ran inside.
    pub attempt: String,
    /// The runtime session whose next step can settle this text.
    pub owner: String,
    /// The call whose answer this is.
    pub tool_use_id: String,
    pub tool_name: String,
    pub source: TextSource,
    /// The head of what the model reads of the tool's output, cut to the use
    /// table's cap ([`text_ask`]).
    pub head: String,
    /// Characters of all the model reads of it.
    pub chars: usize,
    /// What the host says of the fence around it ([`HostFraming`]) — set by
    /// the host's own seam, never by the output's bytes.
    pub framing: HostFraming,
}

/// What an acting text guard hands back for one block: the label a fence
/// names its source by when it read the block as an order to the agent, and
/// the one line it adds. Empty — the default — for every other answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextGuard {
    pub fence: Option<String>,
    pub note: Option<String>,
}

/// The two guards, installed by a host. The runtime records nothing itself:
/// the seat files its own rows and its own labels. Asks are handed over whole,
/// so a seat that only records can keep asking after the result has gone back.
pub trait ToolGuardSeat: Send + Sync {
    /// A shell command is about to run. Never waits: a seat that asks does so
    /// beside the command.
    fn command(&self, ask: CommandAsk);
    /// A shell command has run: its facts for the label, and the line an
    /// acting guard adds to its result. A call the guard was never asked
    /// about answers `None`.
    fn command_ran(&self, ran: CommandRan) -> BoxFuture<'_, Option<String>>;
    /// A tool's text is about to reach the model.
    fn text(&self, ask: TextAsk) -> BoxFuture<'_, TextGuard>;
}

/// What a shell call's input carries that the guard reads.
#[derive(Deserialize)]
struct ShellInput {
    command: String,
    #[serde(default)]
    cwd: Option<PathBuf>,
}

fn guarded_shell_input(tool_name: &str, input: &str) -> Option<ShellInput> {
    if tool_name != SHELL_TOOL {
        return None;
    }
    let shell = serde_json::from_str::<ShellInput>(input).ok()?;
    asks_about(&shell.command).then_some(shell)
}

/// The command and effective cwd the Bash executor will use. A server-supplied
/// input cwd wins over its tool context; only then does Bash inherit the live
/// process cwd. Relative pinned paths are resolved from that process cwd too.
#[must_use]
pub fn command_with_cwd(tool_name: &str, input: &str, context_cwd: Option<&Path>) -> Option<(String, PathBuf)> {
    let shell = guarded_shell_input(tool_name, input)?;
    let selected = shell.cwd.as_deref().or(context_cwd);
    let cwd = match selected {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => std::env::current_dir().ok()?.join(path),
        None => std::env::current_dir().ok()?,
    };
    Some((shell.command, cwd))
}

/// The command a call asks the shell to run, when the guard asks about it —
/// `None` for another tool, an input with no command, and a command today's
/// rules prove read-only ([`asks_about`]).
#[must_use]
pub fn command_of(tool_name: &str, input: &str) -> Option<String> {
    guarded_shell_input(tool_name, input).map(|shell| shell.command)
}

/// The task line a command is judged for: the first line with words of the
/// person's newest message — what the person said the turn is for.
#[must_use]
pub fn task_line(messages: &[ConversationMessage]) -> String {
    task_line_of(&persons_words(messages))
}

/// The text ask for one tool's own `output`, or `None` when the text guard
/// does not read it — an error, an empty answer, or a tool it does not name.
///
/// What is asked about is what the model reads: the result as the newest one
/// in a request is handed over ([`wire_tool_output`], lossless) — a file's
/// lines, a shell answer's own words — and not its envelope. The envelope
/// holds a whole file in one JSON line, and the door withholds a whole line
/// that may carry a credential: 20 of 64 of this repository's files asked
/// that way reached the judgment with under 5% of the text (t-6348 replay).
#[must_use]
pub fn text_ask(attempt: &str, tool_use_id: &str, tool_name: &str, output: &str) -> Option<TextAsk> {
    let source = text_source_of(tool_name, output)?;
    let read = wire_tool_output(output, tool_name, false, WireRewrite::Lossless);
    (!read.trim().is_empty()).then(|| TextAsk {
        attempt: attempt.to_string(),
        owner: attempt.to_string(),
        tool_use_id: tool_use_id.to_string(),
        tool_name: tool_name.to_string(),
        source,
        head: cut(&read, Cap::Chars(TOOL_TEXT_GUARD_TEXT_CHAR_CAP)),
        chars: read.chars().count(),
        // The runtime's own word, off the kind of tool and never off the
        // bytes: its own tools' answers arrive bare; a browser CLI's inner
        // marker arrives as shell bytes, which say nothing a host can be held
        // to. An acting guard places one verified outer fence either way.
        framing: HostFraming::at_this_runtimes_seam(source),
    })
}

/// `output` — the model-facing result, whose head is the tool's own
/// `pristine` output (every hook merge appends after it) — as it reads once
/// the text guard answered: the pristine part inside the fence when the guard
/// names one, then its line. A guard that names neither leaves the output as
/// it was, to the byte.
#[must_use]
pub fn guarded_output(output: String, pristine: &str, guard: &TextGuard) -> String {
    let mut output = match &guard.fence {
        Some(source) if !pristine.is_empty() && output.starts_with(pristine) => {
            let fenced = untrusted::fence(source, pristine, usize::MAX);
            let fenced = fenced.strip_suffix('\n').unwrap_or(&fenced);
            format!("{fenced}{}", &output[pristine.len()..])
        }
        _ => output,
    };
    if let Some(note) = &guard.note {
        output.push_str("\n\n");
        output.push_str(note);
    }
    output
}

#[cfg(test)]
mod tests;
