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
    TABS.iter()
        .position(|one| *one == tab)
        .unwrap_or(TABS.len())
}

/// The word a stopped or finished pane keeps for what it is.
fn pane_what(waiting: &str) -> (&'static str, &'static str) {
    match waiting {
        "blocked" => ("blocked", "pane_blocked"),
        "question" => ("blocked", "pane_question"),
        _ => ("notice", "pane_finished"),
    }
}

/// `desk` and the `panes` that wait, as one list: each tab newest first, the
/// tabs in [`TABS`] order.
///
/// The counts are made from the same rows the list carries, except a
/// hand-in's: the desk sends at most a window of each stage's tasks and counts
/// the rest ([`DeskSnapshot::stages`]), so the 넘김 number is the stage's own
/// count, less the work nobody is coming back to.
pub(crate) fn inbox_of(desk: &DeskSnapshot, panes: &[WaitingRow]) -> Inbox {
    let runs: HashMap<&str, (&str, bool)> = desk
        .runs
        .iter()
        .map(|run| (run.run.as_str(), (run.name.as_str(), run.seat)))
        .collect();
    let named = |run: &str| runs.get(run).copied().unwrap_or(("", false));
    let mut items: Vec<InboxItem> = Vec::new();

    for letter in &desk.mail {
        let (project, seat) = named(&letter.run);
        items.push(InboxItem {
            id: format!("{}/{}", letter.run, letter.id),
            tab: "question",
            what: letter.kind,
            run: Some(letter.run.clone()),
            project: project.to_string(),
            at_ms: letter.created_ms,
            seat,
            letter: Some(letter.clone()),
            task: None,
            pane: None,
            landing: None,
        });
    }

    for task in desk
        .tasks
        .iter()
        .filter(|task| task.stage == HANDED_IN && task.left.is_none())
    {
        let (project, seat) = named(&task.run);
        items.push(InboxItem {
            id: format!("handoff:{}/{}", task.run, task.id),
            tab: "handoff",
            what: "handoff",
            run: Some(task.run.clone()),
            project: project.to_string(),
            at_ms: task
                .times
                .as_ref()
                .and_then(|times| times.handed_ms)
                .unwrap_or(task.created_ms),
            seat,
            letter: None,
            task: Some(task.clone()),
            pane: None,
            landing: None,
        });
    }

    for pane in panes {
        let (tab, what) = pane_what(pane.waiting);
        items.push(InboxItem {
            id: format!("pane:{}@{}", pane.pane, pane.since_ms),
            tab,
            what,
            run: None,
            project: pane
                .worktree
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or_default()
                .to_string(),
            at_ms: pane.since_ms,
            seat: false,
            letter: None,
            task: None,
            pane: Some(pane.clone()),
            landing: None,
        });
    }

    for line in &desk.news {
        let (project, seat) = named(&line.run);
        let tab = if BLOCKING_LETTERS.contains(&line.kind) {
            "blocked"
        } else {
            "notice"
        };
        items.push(InboxItem {
            id: format!("{}/{}", line.run, line.id),
            tab,
            what: line.kind,
            run: Some(line.run.clone()),
            project: project.to_string(),
            at_ms: line.created_ms,
            seat,
            letter: Some(line.clone()),
            task: None,
            pane: None,
            landing: None,
        });
    }

    for run in desk.runs.iter().filter(|run| run.landing > 0) {
        items.push(InboxItem {
            id: format!("landing:{}:{}", run.run, run.landing),
            tab: "notice",
            what: "landing",
            run: Some(run.run.clone()),
            project: run.name.clone(),
            // The ledger dates no waiting-to-land as one moment; the row has
            // none of its own and sinks to the bottom of the notices.
            at_ms: 0,
            seat: run.seat,
            letter: None,
            task: None,
            pane: None,
            landing: Some(run.landing),
        });
    }

    items.sort_by(|a, b| {
        rank(a.tab)
            .cmp(&rank(b.tab))
            .then(b.at_ms.cmp(&a.at_ms))
            .then(a.id.cmp(&b.id))
    });

    let in_tab = |tab: &str| items.iter().filter(|item| item.tab == tab).count();
    let handed_in = desk
        .stages
        .iter()
        .find(|stage| stage.stage == HANDED_IN)
        .map_or(0, |stage| stage.count.saturating_sub(stage.left));
    let (question, blocked, notice) = (in_tab("question"), in_tab("blocked"), in_tab("notice"));
    let handoff = handed_in.max(in_tab("handoff"));
    let counts = InboxCounts {
        all: question + handoff + blocked + notice,
        question,
        handoff,
        blocked,
        notice,
        waiting: question + handoff + blocked,
    };
    Inbox { items, counts }
}
