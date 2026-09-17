//! The session's agent roster, read from the registry the overview reads.
//!
//! `ListAgents` and the Alt+A overview answer the same question — *which
//! helpers does this session have* — and the design point of t-2876 is that
//! they answer it from ONE source. The overview scans
//! [`AgentRegistry::manifest_paths`] and keeps the manifests whose
//! `parentSessionId` is its own (`zo-ide/src/session/subagent_progress.rs`);
//! so does this. No new store, no second index, and nothing here writes.
//!
//! The one deliberate difference is reach: the overview is a live screen and
//! shows only `running` children, while a model asking for its roster is
//! usually asking about ones that have already finished — so every status is
//! reported and the row says which.

use std::path::Path;

use super::manifest::load_agent_manifest_from_scanned_path;
use super::registry::AgentRegistry;
use super::{AgentOutput, EXECUTION_INLINE, EXECUTION_PANE};

/// One helper of this session, as the roster reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRosterRow {
    pub(crate) id: String,
    /// The agent's addressable name — its `label` when it was given one (that
    /// is what `SendMessage(to: …)` resolves), else the generated name.
    pub(crate) name: String,
    pub(crate) status: String,
    /// `inline` (a thread of this process) or `pane` (a zo of its own).
    pub(crate) execution: &'static str,
    /// The pane a teammate runs in. `None` for an inline helper, which has no
    /// screen of its own — naming one would point at the PARENT's.
    pub(crate) pane: Option<String>,
    /// What the most recent `SendMessage` to it came to, `None` until somebody
    /// sends one.
    pub(crate) last_receipt: Option<&'static str>,
    pub(crate) run_generation: u64,
    /// Whether a completion for it is sitting in the store, ready for
    /// `GetAgentCompletion` to collect. A finished agent whose result nobody
    /// has read yet is the one row a parent most needs to see.
    pub(crate) completion_waiting: bool,
}

/// Every agent `session_id` owns, oldest first.
///
/// A store scan: the caller runs it in response to a tool call, never from a
/// paint path.
pub(crate) fn session_agent_rows(
    registry: &AgentRegistry,
    session_id: &str,
) -> Vec<AgentRosterRow> {
    let mut rows: Vec<(u64, AgentRosterRow)> = registry
        .manifest_paths()
        .iter()
        .filter_map(|path| manifest_of_session(path, session_id))
        .map(|manifest| (started_at_of(&manifest), row_of(manifest)))
        .collect();
    // The overview's order, for the same reason: two same-labelled helpers
    // must not swap places between two readings.
    rows.sort_by(|(left_started, left), (right_started, right)| {
        left_started
            .cmp(right_started)
            .then_with(|| left.id.cmp(&right.id))
    });
    rows.into_iter().map(|(_, row)| row).collect()
}

/// The manifest at `path` when it belongs to `session_id`, else `None`.
fn manifest_of_session(path: &Path, session_id: &str) -> Option<AgentOutput> {
    let manifest = load_agent_manifest_from_scanned_path(path).ok()?;
    (manifest.parent_session_id.as_deref() == Some(session_id)).then_some(manifest)
}

/// When the agent started, for ordering. `startedAt` is the truth; a manifest
/// that never reached the start (or carries an unreadable stamp) is ordered by
/// the moment it was created instead.
fn started_at_of(manifest: &AgentOutput) -> u64 {
    let epoch = |value: &str| value.trim().parse::<u64>().ok();
    manifest
        .started_at
        .as_deref()
        .and_then(epoch)
        .or_else(|| epoch(&manifest.created_at))
        .unwrap_or_default()
}

fn row_of(manifest: AgentOutput) -> AgentRosterRow {
    let completion_waiting = super::completion::agent_completion_is_published(&manifest.agent_id);
    let name = manifest
        .label
        .filter(|label| !label.trim().is_empty())
        .unwrap_or(manifest.name);
    AgentRosterRow {
        execution: execution_of(
            manifest.lifecycle.execution.as_deref(),
            manifest.pane.as_deref(),
        ),
        id: manifest.agent_id,
        name,
        status: manifest.status,
        pane: manifest.pane,
        last_receipt: manifest
            .lifecycle
            .last_receipt
            .map(|record| record.receipt.as_str()),
        run_generation: manifest.run_generation,
        completion_waiting,
    }
}

/// The executor a manifest says it runs in. A manifest written before the
/// stamp existed carries none, and there the pane answers instead: a pane
/// child always has one and an inline helper never does.
fn execution_of(stamped: Option<&str>, pane: Option<&str>) -> &'static str {
    match stamped {
        Some(EXECUTION_PANE) => EXECUTION_PANE,
        Some(EXECUTION_INLINE) => EXECUTION_INLINE,
        _ if pane.is_some() => EXECUTION_PANE,
        _ => EXECUTION_INLINE,
    }
}
