//! The inbox page's one reading (t-67483): everything that waits on the
//! person, from every project, in one list.
//!
//! Four kinds of thing wait on a person, and each comes from the place that
//! already knows it:
//!
//! - 질문 — the letters a coordinator owes an answer ([`DeskSnapshot::mail`]);
//! - 넘김 — work a worker handed in and nobody has passed yet (the desk's
//!   `reported` tasks that someone is still coming back to);
//! - 막힘 — a worker that cannot go on: a pane stopped on a permission or its
//!   own question, a quota wall or a deadlock the ledger told (t-67483 §1:
//!   the pane's state is read here, into the same list, so no count of this
//!   page is made anywhere else);
//! - 알림 — the rest of the ledger's news, the verified work not yet landed,
//!   and a finish nobody has looked at.
//!
//! Nothing here reads the ledger a second way. The list is a view over the
//! desk snapshot the standing-order beat already published and the panes the
//! sidebar's list used to read (`cmd::waiting::pane_waits`); the numbers are
//! counted once, here, and the screen draws them without counting again.

use std::collections::HashMap;

use serde::Serialize;

use super::desk::{DeskMail, DeskSnapshot, DeskTask};
use crate::cmd::waiting::WaitingRow;

/// The stage whose tasks are a worker's hand-in waiting for a pass.
const HANDED_IN: &str = "reported";

/// The ledger's notices that mean a worker cannot go on until something
/// changes: a wall to wait out, or two workers waiting on each other. They
/// stand in 막힘 beside the panes that are stopped.
const BLOCKING_LETTERS: [&str; 2] = ["quota_walled", "deadlocked"];

/// The four kinds a list row belongs to, in the order the page lists them: a
/// worker that waits on an answer first, a notice last.
pub(crate) const TABS: [&str; 4] = ["question", "handoff", "blocked", "notice"];

/// One row of the inbox.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct InboxItem {
    /// Names the row for as long as it stands: the key the screen keeps its
    /// row node and its read mark under.
    pub(crate) id: String,
    /// One of [`TABS`].
    pub(crate) tab: &'static str,
    /// What it is, in the ledger's or the pane's word: a letter kind,
    /// `handoff`, `landing`, or `pane_blocked` / `pane_question` /
    /// `pane_finished`.
    pub(crate) what: &'static str,
    /// The run it belongs to, where it belongs to one.
    pub(crate) run: Option<String>,
    /// The run's name — the project the row is about.
    pub(crate) project: String,
    /// When it began to wait, in epoch milliseconds.
    pub(crate) at_ms: i64,
    /// Whether this window holds the run's coordinator seat: an answer and a
    /// pass are written as that seat, and only where it is held.
    pub(crate) seat: bool,
    /// The letter, for a question and for the ledger's notices.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) letter: Option<DeskMail>,
    /// The task, for a hand-in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) task: Option<DeskTask>,
    /// The pane, for a stopped pane and for a finish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pane: Option<WaitingRow>,
    /// How many verified tasks a `landing` row stands for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) landing: Option<usize>,
}

/// The inbox's numbers, counted once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct InboxCounts {
    pub(crate) all: usize,
    pub(crate) question: usize,
    pub(crate) handoff: usize,
    pub(crate) blocked: usize,
    pub(crate) notice: usize,
    /// What waits on the person right now — the sidebar's number: everything
    /// but the notices.
    pub(crate) waiting: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Inbox {
    pub(crate) items: Vec<InboxItem>,
    pub(crate) counts: InboxCounts,
}

fn rank(tab: &str) -> usize {
    TABS.iter().position(|one| *one == tab).unwrap_or(TABS.len())
}

/// The word a stopped or finished pane keeps for what it is.
fn pane_what(waiting: &str) -> (&'static str, &'static str) {
    match waiting {
        "blocked" => ("blocked", "pane_blocked"),
        "question" => ("blocked", "pane_question"),
        _ => ("notice", "pane_finished"),
    }
}

/// Red stand-in (t-67483): the page's reading is not made yet.
pub(crate) fn inbox_of(_desk: &DeskSnapshot, _panes: &[WaitingRow]) -> Inbox {
    Inbox::default()
}
