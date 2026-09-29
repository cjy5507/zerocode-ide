//! The ruler of one pane's turn (t-14869): what an agent's tool calls came
//! to between the person's prompt and the turn's end, counted off the same
//! hook events the seats read — how many calls, how many looked around
//! before the first edit, the same call run again, the calls that failed,
//! the skills loaded. No Jev: numbers only, as the t-14656 baseline counted
//! them off the transcripts, so a turn measured here and a turn measured
//! there are one series.
//!
//! A call is named by a fingerprint of its tool and its input — the input
//! written with its keys in order, less the keys that only say how to run it
//! ([`DROPPED_INPUT_KEYS`]) — so two runs of the same thing are one name and
//! nothing the agent ran is kept. A row carries counts only: no command, no
//! path, no words.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{SkillLoad, sight};
use crate::agent::AgentKind;
use crate::hook::{self, Phase, Tool};
use crate::payload::HookPayload;

/// Where a pane's turns are counted: one ledger beside the seats' own.
pub const PANE_TURNS_LEDGER: &str = "pane-turns.jsonl";

/// The same call this many times in one turn is a run of it.
pub const REPEAT_RUN: usize = 3;

/// Input keys that say how to run a call, not what it does: two calls that
/// differ only here are the same call.
pub const DROPPED_INPUT_KEYS: [&str; 3] = ["description", "timeout", "run_in_background"];

/// Tools that only look around, by their reduced name
/// ([`hook::Tool::named`]'s spelling), beside the read and search verbs.
pub const EXPLORE_TOOL_NAMES: [&str; 3] = ["ls", "notebookread", "toolsearch"];

/// The programs a shell command only looks around with.
pub const EXPLORE_COMMAND_WORDS: [&str; 18] = [
    "cat", "sed", "head", "tail", "grep", "rg", "ls", "find", "wc", "awk", "tree", "fd", "less",
    "file", "stat", "du", "nl", "jq",
];

/// The subcommands of `git` that only read.
pub const GIT_READ_WORDS: [&str; 9] = [
    "log",
    "show",
    "diff",
    "status",
    "grep",
    "blame",
    "ls-files",
    "rev-parse",
    "branch",
];

/// The file a skill is loaded from, where an agent reads it with its shell.
pub const SKILL_FILE: &str = "SKILL.md";

/// What kind of call one is, for the ruler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // three facts of one call, not a state machine
pub struct CallKind {
    /// It only looks around: a read, a search, a listing.
    pub explore: bool,
    /// It changes a file.
    pub edit: bool,
    /// It loads a skill.
    pub skill: bool,
}

/// What one hook event tells the ruler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tallied {
    /// A call: its name and its kind.
    Called { key: String, kind: CallKind },
    /// A call that failed, by its name.
    Failed { key: String },
}

/// What one hook event of `agent`'s tells the ruler: a call where its row
/// sees one begin — where nothing comes before a tool runs, where it comes
/// back — and a failure where one comes back failed. Nothing for any other
/// event.
#[must_use]
pub fn tallied_parsed(agent: AgentKind, event: &str, parsed: &HookPayload<'_>) -> Vec<Tallied> {
    let _ = (agent, event, parsed);
    Vec::new()
}

/// A call's name: its tool and its input, keys in order, less
/// [`DROPPED_INPUT_KEYS`].
#[must_use]
pub fn call_key(tool: &str, input: Option<&Value>) -> String {
    let _ = (tool, input);
    String::new()
}

/// Whether a shell command only looks around: its program, past a leading
/// `cd … &&` and any `NAME=value` before it, is one of
/// [`EXPLORE_COMMAND_WORDS`], or `git` reading ([`GIT_READ_WORDS`]).
#[must_use]
pub fn explore_command(command: &str) -> bool {
    let _ = command;
    false
}

/// One turn's calls, as they came.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnTally {
    calls: Vec<String>,
    explore: usize,
    explore_before_edit: usize,
    first_edit: Option<usize>,
    failed: Vec<String>,
    skills: usize,
}

impl TurnTally {
    /// Count one thing the hooks told.
    pub fn take(&mut self, tallied: Tallied) {
        let _ = tallied;
    }

    /// Whether the turn made no call.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// The calls that looked around before the turn's first edit — `None`
    /// for a turn that edited nothing.
    #[must_use]
    pub fn explore_before_first_edit(&self) -> Option<usize> {
        None
    }

    /// The turn's row, made at `at` for the agent `from` in the folder
    /// `pane` — `None` for a turn that made no call, which the baseline does
    /// not count either.
    #[must_use]
    pub fn row(&self, at: u64, from: &str, pane: Option<String>) -> Option<PaneTurnRow> {
        Some(PaneTurnRow {
            at,
            from: from.to_string(),
            pane,
            calls: 0,
            explore_calls: 0,
            calls_before_first_edit: None,
            explore_before_first_edit: None,
            duplicate_calls: 0,
            repeat_runs: 0,
            repeat_calls: 0,
            failed_calls: 0,
            same_failure_again: false,
            skill_loads: 0,
        })
    }
}

/// One pane turn's counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneTurnRow {
    pub at: u64,
    /// The pane's agent.
    pub from: String,
    /// The folder the pane works in, by its last name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    pub calls: usize,
    pub explore_calls: usize,
    /// Calls before the turn's first edit — none when it edited nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calls_before_first_edit: Option<usize>,
    /// Calls that looked around before the turn's first edit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explore_before_first_edit: Option<usize>,
    /// Calls that ran a call the turn had already run.
    pub duplicate_calls: usize,
    /// Calls run [`REPEAT_RUN`] times or more, each once.
    pub repeat_runs: usize,
    /// The runs' calls past each one's first.
    pub repeat_calls: usize,
    pub failed_calls: usize,
    /// Whether one failing call failed again.
    pub same_failure_again: bool,
    pub skill_loads: usize,
}

#[cfg(test)]
mod tests;
