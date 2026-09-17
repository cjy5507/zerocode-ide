//! Workflow live-progress signalling (topology events → on-disk snapshot).
//!
//! The engine ([`super::engine`]) is a pure synchronous orchestrator: it spawns
//! no threads, awaits nothing, and historically surfaced its `WorkflowReport`
//! only at the very end. That left the TUI blind to *workflow structure*
//! progress — which phase is running, how many agents it spawned, whether a
//! phase replayed from cache — even though individual sub-agents already surface
//! live via their per-agent `.zo/agents/<id>.json` manifests (the parent
//! sidebar polls those for `currentTool`).
//!
//! This module closes that gap the same way `SpawnMultiAgent` did for per-agent
//! `currentTool` (commit 47f552c): a thin **stamp** that records a snapshot to
//! disk for the TUI to poll — *without* touching the core runtime. The engine
//! calls a [`ProgressSink`] at each topology boundary; the production
//! [`LiveProgressSink`] folds each event into one small JSON file
//! (`.zo/workflows/_active.progress.json`) and the viewer joins its phase →
//! `agent_ids` map against the per-agent manifests it already reads.
//!
//! ## Why this can't introduce a bottleneck
//!
//! The engine is synchronous (no `.await` to yield on), so a sink's [`emit`] is
//! called on the orchestration thread. It must be cheap and non-blocking: the
//! production sink does a single best-effort atomic write of a tiny document
//! (no network, no lock contention — `RefCell`, single-threaded engine). A
//! failed write is swallowed; progress is advisory, never load-bearing. The
//! `render_tx` bounded channel is deliberately *not* used (its backpressure
//! could stall the turn loop); the file+poll bridge has neither problem.
//!
//! [`emit`]: ProgressSink::emit

use std::cell::{Cell, OnceCell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::cache::{append_line, workflow_store_dir, write_atomic};
use super::event_store::{EventStore, JsonlEventStore, SqliteEventStore};

/// Fixed filename for the active workflow's progress snapshot, under the same
/// `.zo/workflows/` directory as the resume cache. A new workflow overwrites
/// it; the embedded `run_id` disambiguates and `status` lets the viewer tell a
/// finished run from a live one. One fixed file keeps the TUI poll O(1).
const ACTIVE_PROGRESS_FILE: &str = "_active.progress.json";
const EVENT_RETENTION_MARKER_FILE: &str = "_events.retention.stamp";

#[cfg(test)]
const WORKFLOW_RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);
#[cfg(not(test))]
const WORKFLOW_RETENTION_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

#[cfg(test)]
const MAX_WORKFLOW_EVENT_RUNS: usize = 3;
#[cfg(not(test))]
const MAX_WORKFLOW_EVENT_RUNS: usize = 256;

#[cfg(test)]
const MAX_WORKFLOW_ARTIFACT_ROWS: usize = 8;
#[cfg(not(test))]
const MAX_WORKFLOW_ARTIFACT_ROWS: usize = 4_096;

/// Snapshot document schema version. Bumped if the on-disk shape changes; the
/// reader tolerates unknown/missing fields regardless (forward/backward compat,
/// like the resume cache).
const PROGRESS_SCHEMA: u32 = 1;

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// A phase known up-front (before any agent spawns), so a viewer can draw the
/// full tree immediately rather than discovering phases as they run.
pub(crate) struct PhaseSkeleton {
    pub id: String,
    /// `"fanout"` | `"over"` | `"single"` — derived from the phase source.
    pub kind: &'static str,
}

/// A topology event emitted at an engine boundary. Borrows from the engine's own
/// data (the engine is single-threaded, so borrows are sound); the sink copies
/// only what it persists. `Copy` (every field is a shared reference or scalar)
/// so [`TeeProgressSink`] can forward one event to several sinks for free.
#[derive(Clone, Copy)]
pub(crate) enum ProgressEvent<'a> {
    /// Emitted once at the start with the full phase skeleton.
    Started {
        name: &'a str,
        description: &'a str,
        /// `"phases"` | `"pipeline"`.
        mode: &'a str,
        phases: &'a [PhaseSkeleton],
    },
    /// A phase (or pipeline stage) began running its `round`-th round (1-based).
    PhaseEnter { id: &'a str, round: u32 },
    /// Agents were spawned for `phase_id`. Their ids join to the per-agent
    /// manifests the TUI already polls, so the viewer can attach
    /// name/currentTool/tokens/elapsed per agent.
    AgentsSpawned {
        phase_id: &'a str,
        agent_ids: &'a [String],
    },
    /// One agent reached a terminal completion while its phase barrier is
    /// still collecting — emitted in true completion order, so live viewers
    /// can advance the phase tally per agent instead of freezing at 0% until
    /// [`ProgressEvent::PhaseDone`].
    AgentDone {
        phase_id: &'a str,
        agent_id: &'a str,
        /// `"completed"` | `"failed"` | `"stopped"` | `"still_running"`.
        status: &'a str,
    },
    /// A phase finished, with a terminal item-status tally.
    PhaseDone {
        id: &'a str,
        completed: usize,
        failed: usize,
        still_running: usize,
        carried: usize,
        retried: usize,
        skipped: usize,
    },
    FindingQueued {
        phase_id: &'a str,
        finding_id: &'a str,
    },
    ItemCarried {
        phase_id: &'a str,
        item_index: usize,
    },
    ItemInvalidated {
        phase_id: &'a str,
        item_index: usize,
        reason: &'a str,
    },
    SelectiveRetryStarted {
        phase_id: &'a str,
        finding_id: &'a str,
    },
    FindingBlocked {
        phase_id: &'a str,
        finding_id: &'a str,
        reason: &'a str,
    },
    /// A completed phase was replayed from the resume cache (no agents spawned).
    /// Surfaced explicitly so the viewer shows "resumed" instead of an empty
    /// phase that reads as "nothing happened".
    PhaseResumed { id: &'a str },
    /// The synthesize agent began.
    SynthesizeEnter,
    /// The run reached a terminal state: `"completed"` | `"cancelled"` |
    /// `"budget_exhausted"`.
    Finished { status: &'a str },
}

/// The seam the engine emits through. Injected via `RunOptions::progress`;
/// `None` (the default) makes every emit a no-op, so offline tests and the
/// resume path pay nothing.
pub(crate) trait ProgressSink {
    fn emit(&self, event: ProgressEvent<'_>);
}

// ---------------------------------------------------------------------------
// On-disk snapshot document
// ---------------------------------------------------------------------------

#[derive(Serialize, Default)]
struct ProgressDoc {
    schema: u32,
    run_id: String,
    /// Foreground session id that spawned this workflow, when available. Lets the
    /// TUI trust doc-only just-spawned agent ids without leaking a concurrent
    /// session's workflow into the current HUD.
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_session_id: Option<String>,
    name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    description: String,
    mode: String,
    /// `"running"` | `"completed"` | `"cancelled"` | `"budget_exhausted"`.
    status: String,
    /// Index into `phases` of the phase currently running (0 before the first).
    current_phase: usize,
    phases: Vec<ProgressPhaseDoc>,
    /// True while the final synthesize agent runs.
    synthesizing: bool,
    updated_at_ms: u64,
}

#[derive(Serialize, Default, Clone)]
struct ProgressPhaseDoc {
    id: String,
    kind: String,
    /// `"pending"` | `"running"` | `"done"` | `"resumed"`.
    status: String,
    /// Current round (1-based) for a `repeat` phase; 0/1 otherwise.
    round: u32,
    /// Ids of agents spawned for this phase (accumulated across rounds). The
    /// viewer joins these to the per-agent manifests.
    agent_ids: Vec<String>,
    completed: usize,
    failed: usize,
    still_running: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    carried: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    retried: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    skipped: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    findings: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    blocked: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    invalidated: usize,
    #[serde(skip_serializing_if = "is_zero_usize")]
    selective_retries: usize,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

// ---------------------------------------------------------------------------
// Production sink
// ---------------------------------------------------------------------------

/// Production [`ProgressSink`]: folds each event into [`ProgressDoc`] and writes
/// it atomically after every event. Single-threaded (engine), so a `RefCell`
/// suffices — no lock, no contention. A missing store directory disables writes
/// (the sink still folds state but flushes nowhere), so a non-project cwd never
/// errors the run.
pub(crate) struct LiveProgressSink {
    run_id: String,
    doc: RefCell<ProgressDoc>,
    path: Option<PathBuf>,
}

impl LiveProgressSink {
    pub(crate) fn new(run_id: String, parent_session_id: Option<&str>) -> Self {
        let path = workflow_store_dir().map(|dir| dir.join(ACTIVE_PROGRESS_FILE));
        Self::with_path(run_id, parent_session_id, path)
    }

    /// A sink that folds state but never writes to disk. Used by tests so they
    /// assert the fold logic without polluting the project's `.zo/` directory.
    #[cfg(test)]
    fn detached(run_id: String) -> Self {
        Self::with_path(run_id, None, None)
    }

    #[cfg(test)]
    fn detached_for_session(run_id: String, parent_session_id: &str) -> Self {
        Self::with_path(run_id, Some(parent_session_id), None)
    }

    fn with_path(run_id: String, parent_session_id: Option<&str>, path: Option<PathBuf>) -> Self {
        Self {
            run_id,
            doc: RefCell::new(ProgressDoc {
                schema: PROGRESS_SCHEMA,
                parent_session_id: parent_session_id.map(str::to_string),
                ..ProgressDoc::default()
            }),
            path,
        }
    }

    /// Best-effort atomic write. A failure is swallowed: progress is advisory.
    fn flush(&self, doc: &ProgressDoc) {
        let Some(path) = self.path.as_ref() else {
            return;
        };
        if let Ok(text) = serde_json::to_string(doc) {
            let _ = write_atomic(path, &text);
        }
    }
}

#[allow(clippy::too_many_lines)]
impl ProgressSink for LiveProgressSink {
    fn emit(&self, event: ProgressEvent<'_>) {
        let mut doc = self.doc.borrow_mut();
        match event {
            ProgressEvent::Started {
                name,
                description,
                mode,
                phases,
            } => {
                doc.run_id.clone_from(&self.run_id);
                doc.name = name.to_string();
                doc.description = description.to_string();
                doc.mode = mode.to_string();
                doc.status = "running".to_string();
                doc.current_phase = 0;
                doc.synthesizing = false;
                doc.phases = phases
                    .iter()
                    .map(|p| ProgressPhaseDoc {
                        id: p.id.clone(),
                        kind: p.kind.to_string(),
                        status: "pending".to_string(),
                        ..ProgressPhaseDoc::default()
                    })
                    .collect();
            }
            ProgressEvent::PhaseEnter { id, round } => {
                if let Some(idx) = doc.phases.iter().position(|p| p.id == id) {
                    doc.phases[idx].status = "running".to_string();
                    doc.phases[idx].round = round;
                    doc.current_phase = idx;
                }
            }
            ProgressEvent::AgentsSpawned {
                phase_id,
                agent_ids,
            } => {
                if let Some(idx) = doc.phases.iter().position(|p| p.id == phase_id) {
                    doc.phases[idx].agent_ids.extend(agent_ids.iter().cloned());
                }
            }
            ProgressEvent::AgentDone {
                phase_id, status, ..
            } => {
                if let Some(idx) = doc.phases.iter().position(|p| p.id == phase_id) {
                    let phase = &mut doc.phases[idx];
                    // Live tally while the phase collects; `PhaseDone` later
                    // overwrites with the authoritative report. Non-terminal
                    // placeholders (`still_running`) don't move the tally.
                    match status {
                        "completed" => phase.completed = phase.completed.saturating_add(1),
                        "failed" | "stopped" => phase.failed = phase.failed.saturating_add(1),
                        _ => {}
                    }
                    let total = phase.agent_ids.len();
                    if total > 0 {
                        phase.completed = phase.completed.min(total);
                        phase.failed = phase.failed.min(total - phase.completed);
                        phase.still_running = total - phase.completed - phase.failed;
                    }
                }
            }
            ProgressEvent::PhaseDone {
                id,
                completed,
                failed,
                still_running,
                carried,
                retried,
                skipped,
            } => {
                if let Some(idx) = doc.phases.iter().position(|p| p.id == id) {
                    let phase = &mut doc.phases[idx];
                    phase.status = "done".to_string();
                    phase.completed = completed;
                    phase.failed = failed;
                    phase.still_running = still_running;
                    phase.carried = carried;
                    phase.retried = retried;
                    phase.skipped = skipped;
                }
            }
            ProgressEvent::FindingQueued { phase_id, .. } => {
                if let Some(phase) = doc.phases.iter_mut().find(|p| p.id == phase_id) {
                    phase.findings = phase.findings.saturating_add(1);
                }
            }
            ProgressEvent::ItemCarried { phase_id, .. } => {
                if let Some(phase) = doc.phases.iter_mut().find(|p| p.id == phase_id) {
                    phase.carried = phase.carried.saturating_add(1);
                }
            }
            ProgressEvent::ItemInvalidated { phase_id, .. } => {
                if let Some(phase) = doc.phases.iter_mut().find(|p| p.id == phase_id) {
                    phase.invalidated = phase.invalidated.saturating_add(1);
                }
            }
            ProgressEvent::SelectiveRetryStarted { phase_id, .. } => {
                if let Some(phase) = doc.phases.iter_mut().find(|p| p.id == phase_id) {
                    phase.selective_retries = phase.selective_retries.saturating_add(1);
                }
            }
            ProgressEvent::FindingBlocked { phase_id, .. } => {
                if let Some(phase) = doc.phases.iter_mut().find(|p| p.id == phase_id) {
                    phase.blocked = phase.blocked.saturating_add(1);
                }
            }
            ProgressEvent::PhaseResumed { id } => {
                if let Some(idx) = doc.phases.iter().position(|p| p.id == id) {
                    doc.phases[idx].status = "resumed".to_string();
                }
            }
            ProgressEvent::SynthesizeEnter => {
                doc.synthesizing = true;
            }
            ProgressEvent::Finished { status } => {
                doc.status = status.to_string();
                doc.synthesizing = false;
            }
        }
        doc.updated_at_ms = now_ms();
        self.flush(&doc);
    }
}

// ---------------------------------------------------------------------------
// Phase-3 append-only event log (shadow mode)
// ---------------------------------------------------------------------------

/// Event-log schema version. Independent of [`PROGRESS_SCHEMA`]; the reader
/// tolerates unknown/missing fields regardless (forward/backward compat).
const EVENT_SCHEMA: u32 = 1;

/// One persisted line of the append-only workflow event log. `run_id` + `seq`
/// give a total order within a process run; file (append) order is authoritative
/// across runs — a resume re-opens the same `run_id` and appends, restarting
/// `seq` at 0. Readers skip lines they cannot parse, like the snapshot reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowEventRecord {
    pub schema: u32,
    pub run_id: String,
    pub seq: u64,
    pub ts_ms: u64,
    pub event: WorkflowEventKind,
}

/// The owned, serializable projection of a `ProgressEvent`. The borrowed
/// `ProgressEvent` stays the engine's hot-path currency; this is what lands on
/// disk so the timeline can be reconstructed without re-running the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkflowEventKind {
    Started {
        name: String,
        description: String,
        mode: String,
        phases: Vec<EventPhase>,
    },
    PhaseEnter {
        id: String,
        round: u32,
    },
    AgentsSpawned {
        phase_id: String,
        agent_ids: Vec<String>,
    },
    AgentDone {
        phase_id: String,
        agent_id: String,
        status: String,
    },
    PhaseDone {
        id: String,
        completed: usize,
        failed: usize,
        still_running: usize,
        #[serde(default)]
        carried: usize,
        #[serde(default)]
        retried: usize,
        #[serde(default)]
        skipped: usize,
    },
    FindingQueued {
        phase_id: String,
        finding_id: String,
    },
    ItemCarried {
        phase_id: String,
        item_index: usize,
    },
    ItemInvalidated {
        phase_id: String,
        item_index: usize,
        reason: String,
    },
    SelectiveRetryStarted {
        phase_id: String,
        finding_id: String,
    },
    FindingBlocked {
        phase_id: String,
        finding_id: String,
        reason: String,
    },
    PhaseResumed {
        id: String,
    },
    SynthesizeEnter,
    Finished {
        status: String,
    },
    /// Forward-compat catch-all: a `kind` written by a newer Zo that this
    /// reader doesn't know deserializes here instead of dropping the whole line,
    /// so the timeline keeps the event's slot (its `seq`/`run_id`/`ts_ms` survive
    /// at the record level). Never produced from a known `ProgressEvent`.
    #[serde(other)]
    Unknown,
}

/// A phase in the up-front skeleton, owned for persistence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPhase {
    pub id: String,
    pub kind: String,
}

impl WorkflowEventKind {
    fn from_event(event: ProgressEvent<'_>) -> Self {
        match event {
            ProgressEvent::Started {
                name,
                description,
                mode,
                phases,
            } => Self::Started {
                name: name.to_string(),
                description: description.to_string(),
                mode: mode.to_string(),
                phases: phases
                    .iter()
                    .map(|p| EventPhase {
                        id: p.id.clone(),
                        kind: p.kind.to_string(),
                    })
                    .collect(),
            },
            ProgressEvent::PhaseEnter { id, round } => Self::PhaseEnter {
                id: id.to_string(),
                round,
            },
            ProgressEvent::AgentsSpawned {
                phase_id,
                agent_ids,
            } => Self::AgentsSpawned {
                phase_id: phase_id.to_string(),
                agent_ids: agent_ids.to_vec(),
            },
            ProgressEvent::AgentDone {
                phase_id,
                agent_id,
                status,
            } => Self::AgentDone {
                phase_id: phase_id.to_string(),
                agent_id: agent_id.to_string(),
                status: status.to_string(),
            },
            ProgressEvent::PhaseDone {
                id,
                completed,
                failed,
                still_running,
                carried,
                retried,
                skipped,
            } => Self::PhaseDone {
                id: id.to_string(),
                completed,
                failed,
                still_running,
                carried,
                retried,
                skipped,
            },
            ProgressEvent::FindingQueued {
                phase_id,
                finding_id,
            } => Self::FindingQueued {
                phase_id: phase_id.to_string(),
                finding_id: finding_id.to_string(),
            },
            ProgressEvent::ItemCarried {
                phase_id,
                item_index,
            } => Self::ItemCarried {
                phase_id: phase_id.to_string(),
                item_index,
            },
            ProgressEvent::ItemInvalidated {
                phase_id,
                item_index,
                reason,
            } => Self::ItemInvalidated {
                phase_id: phase_id.to_string(),
                item_index,
                reason: reason.to_string(),
            },
            ProgressEvent::SelectiveRetryStarted {
                phase_id,
                finding_id,
            } => Self::SelectiveRetryStarted {
                phase_id: phase_id.to_string(),
                finding_id: finding_id.to_string(),
            },
            ProgressEvent::FindingBlocked {
                phase_id,
                finding_id,
                reason,
            } => Self::FindingBlocked {
                phase_id: phase_id.to_string(),
                finding_id: finding_id.to_string(),
                reason: reason.to_string(),
            },
            ProgressEvent::PhaseResumed { id } => Self::PhaseResumed { id: id.to_string() },
            ProgressEvent::SynthesizeEnter => Self::SynthesizeEnter,
            ProgressEvent::Finished { status } => Self::Finished {
                status: status.to_string(),
            },
        }
    }
}

/// Append-only event-log sink (Phase-3 "shadow mode"). Serializes each
/// [`ProgressEvent`] as one JSONL line to `<run_id>.events.jsonl`, running in
/// parallel with [`LiveProgressSink`] (joined by a [`TeeProgressSink`]). The
/// snapshot stays the TUI's O(1) poll target; this log is the on-demand,
/// replayable timeline behind the doc's append-only-event requirement. A missing
/// store directory disables writes, so a non-project cwd never errors the run.
pub(crate) struct EventLogSink {
    run_id: String,
    seq: Cell<u64>,
    path: Option<PathBuf>,
    /// Whether to dual-write to the Phase-6 `SQLite` store (on via [`Self::new`],
    /// off for the JSONL-only test constructor).
    sqlite_enabled: bool,
    /// `SQLite` store, opened **lazily** on the first emit (cached
    /// thereafter). Like the JSONL file — which `append_line` creates only on
    /// first write — deferring the open means a sink that never emits (e.g. a
    /// workflow that fails before its first event) leaves no empty `events.db`
    /// behind.
    sqlite: OnceCell<Option<SqliteEventStore>>,
    /// Retention is best-effort and only needs to run once per sink/run.
    retention_checked: Cell<bool>,
}

impl EventLogSink {
    pub(crate) fn new(run_id: String) -> Self {
        let path = event_log_path(&run_id);
        Self {
            run_id,
            seq: Cell::new(0),
            path,
            sqlite_enabled: true,
            sqlite: OnceCell::new(),
            retention_checked: Cell::new(false),
        }
    }

    /// Construct with an explicit log path and **no** `SQLite` dual-write.
    /// Test-only: integration/unit tests drive the engine into a temp JSONL
    /// store without touching the process-global `ZO_WORKFLOW_STORE` (the
    /// env-free pattern the resume-cache tests use); the `SQLite` store is covered
    /// directly by the `event_store` tests. Production goes through [`Self::new`]
    /// (which also opens the dual-write store).
    #[cfg(test)]
    pub(crate) fn with_path(run_id: String, path: Option<PathBuf>) -> Self {
        Self {
            run_id,
            seq: Cell::new(0),
            path,
            sqlite_enabled: false,
            sqlite: OnceCell::new(),
            retention_checked: Cell::new(false),
        }
    }

    /// A sink that advances `seq` but never writes, for tests that assert the
    /// projection/seq without touching the store.
    #[cfg(test)]
    fn detached(run_id: String) -> Self {
        Self::with_path(run_id, None)
    }

    /// Test seam: a sink that dual-writes to a `SQLite` store at an explicit path
    /// (and a JSONL path), env-free — so the dual-write is verified without the
    /// process-global `ZO_WORKFLOW_STORE`. Pre-fills the lazy cell with the
    /// injected store.
    #[cfg(test)]
    fn with_sqlite_at(run_id: String, path: Option<PathBuf>, db_path: &std::path::Path) -> Self {
        let sink = Self {
            run_id,
            seq: Cell::new(0),
            path,
            sqlite_enabled: true,
            sqlite: OnceCell::new(),
            retention_checked: Cell::new(false),
        };
        let _ = sink.sqlite.set(SqliteEventStore::open_at(db_path).ok());
        sink
    }
    fn apply_retention(&self) {
        if self.retention_checked.replace(true) {
            return;
        }
        let retention_dir = self.path.as_ref().and_then(|path| path.parent());
        let now = SystemTime::now();
        if self.sqlite_enabled {
            if let Some(store) = self.sqlite.get_or_init(SqliteEventStore::open).as_ref() {
                let pruned_artifacts =
                    store.prune_retention(MAX_WORKFLOW_EVENT_RUNS, MAX_WORKFLOW_ARTIFACT_ROWS);
                crate::artifacts::delete_artifact_files(pruned_artifacts);
            }
        }
        if let Some(dir) = retention_dir {
            if event_retention_marker_is_fresh(dir, now) {
                return;
            }
            prune_jsonl_event_logs(dir, MAX_WORKFLOW_EVENT_RUNS);
            mark_event_retention_checked(dir, now);
        }
    }
}

impl ProgressSink for EventLogSink {
    fn emit(&self, event: ProgressEvent<'_>) {
        let seq = self.seq.get();
        self.seq.set(seq.saturating_add(1));
        let record = WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: self.run_id.clone(),
            seq,
            ts_ms: now_ms(),
            event: WorkflowEventKind::from_event(event),
        };
        // Phase-6 dual-write: `SQLite` is the evolving store, JSONL the proven
        // shadow. Each is independent and best-effort, so a failure (or absence)
        // of one never blocks the other and the run is never load-bearing on
        // either. The store is opened lazily here (not in `new`) so it is created
        // only once there is an event to write — matching the JSONL file.
        if self.sqlite_enabled {
            if let Some(store) = self.sqlite.get_or_init(SqliteEventStore::open).as_ref() {
                store.record_event(&record);
            }
        }
        if let Some(path) = self.path.as_ref() {
            if let Ok(line) = serde_json::to_string(&record) {
                let _ = append_line(path, &line);
            }
        }
        self.apply_retention();
    }
}

/// Path of the append-only event log for `run_id`, alongside the resume cache's
/// `<run_id>.json`. `None` when no store directory is resolvable.
fn event_log_path(run_id: &str) -> Option<PathBuf> {
    workflow_store_dir().map(|dir| dir.join(format!("{run_id}.events.jsonl")))
}

fn event_retention_marker_path(dir: &Path) -> PathBuf {
    dir.join(EVENT_RETENTION_MARKER_FILE)
}

fn event_retention_marker_is_fresh(dir: &Path, now: SystemTime) -> bool {
    let marker = event_retention_marker_path(dir);
    let Ok(modified) = fs::metadata(marker).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    matches!(
        now.duration_since(modified),
        Ok(age) if age < WORKFLOW_RETENTION_INTERVAL
    )
}

fn mark_event_retention_checked(dir: &Path, now: SystemTime) {
    let marker = event_retention_marker_path(dir);
    let seconds = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let _ = write_atomic(&marker, &format!("{seconds}\n"));
}

fn prune_jsonl_event_logs(dir: &Path, keep_runs: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut logs: Vec<_> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_event_log = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".events.jsonl"));
            if !is_event_log {
                return None;
            }
            let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
            Some((modified, path))
        })
        .collect();
    if logs.len() <= keep_runs {
        return;
    }
    logs.sort_by(|(left_modified, left_path), (right_modified, right_path)| {
        right_modified
            .cmp(left_modified)
            .then_with(|| right_path.cmp(left_path))
    });
    for (_, path) in logs.into_iter().skip(keep_runs) {
        let _ = fs::remove_file(path);
    }
}

/// Reconstruct a run's ordered timeline from its append-only event log — the
/// reader behind the doc's "event log로 최소 run timeline을 재구성 가능" criterion,
/// and the source the TUI event-log inspector reads. Lines that fail to parse are
/// skipped (partial final write / forward-compat). Returns events in file
/// (append) order, which is chronological regardless of per-process `seq` resets.
/// Empty when the log is absent or unreadable.
#[must_use]
pub fn read_event_log(run_id: &str) -> Vec<WorkflowEventRecord> {
    // `open_existing`, not `open`: a read must never materialize an empty
    // `events.db` (the TUI calls this every poll). Absent DB → JSONL fallback.
    let sqlite = SqliteEventStore::open_existing();
    let jsonl_path = event_log_path(run_id);
    read_event_log_resolved(run_id, sqlite.as_ref(), jsonl_path.as_deref())
}

/// `SQLite`-first read with JSONL fallback + migrate-on-miss (Phase-6). Split out
/// with both stores injected so the policy is unit-tested env-free:
/// * `SQLite` has the run → serve from the store (new dual-written runs).
/// * `SQLite` is empty but JSONL has history → an old JSONL-only run: import it
///   once, then return it (subsequent reads come from the store).
/// * No `SQLite` store (no dir / open failed) → read the JSONL directly
///   (old-session compatibility and the corruption-recovery path).
pub(crate) fn read_event_log_resolved(
    run_id: &str,
    sqlite: Option<&SqliteEventStore>,
    jsonl_path: Option<&std::path::Path>,
) -> Vec<WorkflowEventRecord> {
    let jsonl = JsonlEventStore {
        path: jsonl_path.map(std::path::Path::to_path_buf),
    };
    let Some(store) = sqlite else {
        return jsonl.read_events(run_id);
    };
    let events = store.read_events(run_id);
    if !events.is_empty() {
        return events;
    }
    let from_jsonl = jsonl.read_events(run_id);
    if !from_jsonl.is_empty() {
        store.import(&from_jsonl);
    }
    from_jsonl
}

pub(crate) fn read_event_log_at(path: &std::path::Path) -> Vec<WorkflowEventRecord> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<WorkflowEventRecord>(line).ok())
        .collect()
}

/// Render an event log into human-readable timeline lines (one per record) for
/// the TUI event-log inspector or a CLI dump. Pure and ordering-preserving, so
/// the inspector's formatting is unit-tested without a terminal; the viewer just
/// renders the returned strings. `seq` prefixes each line so the total order is
/// visible at a glance.
#[must_use]
pub fn event_timeline_lines(records: &[WorkflowEventRecord]) -> Vec<String> {
    records.iter().map(format_event_line).collect()
}

/// The run's authoritative terminal status, derived from its append-only event
/// log (the last `Finished` event wins). `None` while the run is still in
/// progress. Phase-5 seam: lets a TUI reconcile a stale "running" snapshot
/// against the event log, which — being append-only — never loses the run's last
/// word even when the snapshot's final overwrite was dropped (e.g. a crash
/// between the event append and the snapshot write).
#[must_use]
pub fn event_log_terminal_status(records: &[WorkflowEventRecord]) -> Option<String> {
    records.iter().rev().find_map(|record| match &record.event {
        WorkflowEventKind::Finished { status } => Some(status.clone()),
        _ => None,
    })
}

/// The authoritative per-phase status derived from the append-only event log,
/// keyed by phase id. The last lifecycle event for each id wins, mapped to the
/// snapshot's vocabulary: `PhaseEnter` → `"running"`, `PhaseDone` → `"done"`,
/// `PhaseResumed` → `"resumed"`. A phase the log has not touched yet is absent
/// (so the snapshot's own `"pending"`/`"running"` stands).
///
/// Phase-5 seam: a TUI read model overlays this onto the polled snapshot so a
/// phase row that still reads `"running"` after the log recorded the phase done
/// is corrected — the log being append-only, it never loses the phase's last
/// word even when the snapshot's overwrite for that phase was dropped. Pure, so
/// the overlay is unit-tested without an on-disk log.
#[must_use]
pub fn event_phase_statuses(records: &[WorkflowEventRecord]) -> HashMap<String, String> {
    let mut statuses = HashMap::new();
    for record in records {
        let (id, status) = match &record.event {
            WorkflowEventKind::PhaseEnter { id, .. } => (id, "running"),
            WorkflowEventKind::PhaseDone { id, .. } => (id, "done"),
            WorkflowEventKind::PhaseResumed { id } => (id, "resumed"),
            _ => continue,
        };
        statuses.insert(id.clone(), status.to_string());
    }
    statuses
}

fn format_event_line(record: &WorkflowEventRecord) -> String {
    let body = match &record.event {
        WorkflowEventKind::Started {
            name, mode, phases, ..
        } => format!("started '{name}' ({mode}, {} phases)", phases.len()),
        WorkflowEventKind::PhaseEnter { id, round } => {
            format!("phase '{id}' → running (round {round})")
        }
        WorkflowEventKind::AgentsSpawned {
            phase_id,
            agent_ids,
        } => format!("phase '{phase_id}' spawned {} agent(s)", agent_ids.len()),
        WorkflowEventKind::AgentDone {
            phase_id,
            agent_id,
            status,
        } => format!("phase '{phase_id}' agent '{agent_id}' → {status}"),
        WorkflowEventKind::PhaseDone {
            id,
            completed,
            failed,
            still_running,
            carried,
            retried,
            skipped,
        } => {
            let extra = if *carried == 0 && *retried == 0 && *skipped == 0 {
                String::new()
            } else {
                format!(", {carried} carried, {retried} retried, {skipped} skipped")
            };
            format!("phase '{id}' done ({completed} ok, {failed} failed, {still_running} running{extra})")
        }
        WorkflowEventKind::FindingQueued {
            phase_id,
            finding_id,
        } => format!("phase '{phase_id}' queued finding '{finding_id}'"),
        WorkflowEventKind::ItemCarried {
            phase_id,
            item_index,
        } => format!("phase '{phase_id}' carried item {item_index}"),
        WorkflowEventKind::ItemInvalidated {
            phase_id,
            item_index,
            reason,
        } => format!("phase '{phase_id}' invalidated item {item_index}: {reason}"),
        WorkflowEventKind::SelectiveRetryStarted {
            phase_id,
            finding_id,
        } => format!("phase '{phase_id}' selective retry for finding '{finding_id}'"),
        WorkflowEventKind::FindingBlocked {
            phase_id,
            finding_id,
            reason,
        } => format!("phase '{phase_id}' blocked finding '{finding_id}': {reason}"),
        WorkflowEventKind::PhaseResumed { id } => format!("phase '{id}' resumed from cache"),
        WorkflowEventKind::SynthesizeEnter => "synthesize → running".to_string(),
        WorkflowEventKind::Finished { status } => format!("finished: {status}"),
        WorkflowEventKind::Unknown => "unknown event (newer schema)".to_string(),
    };
    format!("{:>4}  {body}", record.seq)
}

// ---------------------------------------------------------------------------
// Tee
// ---------------------------------------------------------------------------

/// Fan one event out to several sinks, keeping the engine's single
/// `Option<&dyn ProgressSink>` seam unchanged while Phase 3 records in parallel.
/// `ProgressEvent` is `Copy`, so each forward is a cheap pointer copy.
pub(crate) struct TeeProgressSink<'a> {
    sinks: Vec<&'a dyn ProgressSink>,
}

impl<'a> TeeProgressSink<'a> {
    pub(crate) fn new(sinks: Vec<&'a dyn ProgressSink>) -> Self {
        Self { sinks }
    }
}

impl ProgressSink for TeeProgressSink<'_> {
    fn emit(&self, event: ProgressEvent<'_>) {
        for sink in &self.sinks {
            sink.emit(event);
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_sink_folds_started_then_phase_lifecycle() {
        let sink = LiveProgressSink::detached("run-1".to_string());
        let skel = [PhaseSkeleton {
            id: "read".to_string(),
            kind: "fanout",
        }];
        sink.emit(ProgressEvent::Started {
            name: "demo",
            description: "",
            mode: "phases",
            phases: &skel,
        });
        sink.emit(ProgressEvent::PhaseEnter {
            id: "read",
            round: 1,
        });
        sink.emit(ProgressEvent::AgentsSpawned {
            phase_id: "read",
            agent_ids: &["a0".to_string(), "a1".to_string()],
        });
        sink.emit(ProgressEvent::PhaseDone {
            id: "read",
            completed: 2,
            failed: 0,
            still_running: 0,
            carried: 0,
            retried: 0,
            skipped: 0,
        });
        sink.emit(ProgressEvent::Finished {
            status: "completed",
        });

        let doc = sink.doc.borrow();
        assert_eq!(doc.run_id, "run-1");
        assert_eq!(doc.status, "completed");
        assert_eq!(doc.phases.len(), 1);
        assert_eq!(doc.phases[0].status, "done");
        assert_eq!(doc.phases[0].agent_ids, vec!["a0", "a1"]);
        assert_eq!(doc.phases[0].completed, 2);
    }

    #[test]
    fn live_sink_stamps_parent_session_id_for_tui_scoping() {
        let sink = LiveProgressSink::detached_for_session("run-scoped".to_string(), "sess-current");
        sink.emit(ProgressEvent::Started {
            name: "demo",
            description: "",
            mode: "phases",
            phases: &[],
        });

        let doc = sink.doc.borrow();
        assert_eq!(doc.run_id, "run-scoped");
        assert_eq!(doc.parent_session_id.as_deref(), Some("sess-current"));
    }

    /// 모달 0% 고착 회귀: 배리어가 끝나기 전에도 `AgentDone` 이 끝나는 순서대로
    /// phase 집계를 전진시킨다 — 그리고 비종결(`still_running`) placeholder 와
    /// 중복 초과분은 집계를 흔들지 못한다.
    #[test]
    fn agent_done_advances_phase_tally_before_phase_done() {
        let sink = LiveProgressSink::detached("run-live".to_string());
        let skel = [PhaseSkeleton {
            id: "read".to_string(),
            kind: "fanout",
        }];
        sink.emit(ProgressEvent::Started {
            name: "demo",
            description: "",
            mode: "phases",
            phases: &skel,
        });
        sink.emit(ProgressEvent::PhaseEnter {
            id: "read",
            round: 1,
        });
        sink.emit(ProgressEvent::AgentsSpawned {
            phase_id: "read",
            agent_ids: &["a0".to_string(), "a1".to_string(), "a2".to_string()],
        });

        sink.emit(ProgressEvent::AgentDone {
            phase_id: "read",
            agent_id: "a1",
            status: "completed",
        });
        {
            let doc = sink.doc.borrow();
            assert_eq!(doc.phases[0].completed, 1, "first completion moves tally");
            assert_eq!(doc.phases[0].still_running, 2);
            assert_eq!(doc.phases[0].status, "running", "phase still collecting");
        }

        sink.emit(ProgressEvent::AgentDone {
            phase_id: "read",
            agent_id: "a0",
            status: "failed",
        });
        // 비종결 placeholder 는 무시.
        sink.emit(ProgressEvent::AgentDone {
            phase_id: "read",
            agent_id: "a2",
            status: "still_running",
        });
        {
            let doc = sink.doc.borrow();
            assert_eq!(doc.phases[0].completed, 1);
            assert_eq!(doc.phases[0].failed, 1);
            assert_eq!(doc.phases[0].still_running, 1);
        }

        // PhaseDone 은 권위 집계로 덮어쓴다.
        sink.emit(ProgressEvent::PhaseDone {
            id: "read",
            completed: 2,
            failed: 1,
            still_running: 0,
            carried: 0,
            retried: 0,
            skipped: 0,
        });
        let doc = sink.doc.borrow();
        assert_eq!(doc.phases[0].completed, 2);
        assert_eq!(doc.phases[0].failed, 1);
        assert_eq!(doc.phases[0].still_running, 0);
        assert_eq!(doc.phases[0].status, "done");
    }

    #[test]
    fn resumed_phase_is_marked_distinctly() {
        let sink = LiveProgressSink::detached("run-2".to_string());
        let skel = [PhaseSkeleton {
            id: "p0".to_string(),
            kind: "single",
        }];
        sink.emit(ProgressEvent::Started {
            name: "demo",
            description: "",
            mode: "phases",
            phases: &skel,
        });
        sink.emit(ProgressEvent::PhaseResumed { id: "p0" });
        assert_eq!(sink.doc.borrow().phases[0].status, "resumed");
    }

    /// A per-test temp directory that never touches the project's `.zo/` or
    /// the process-global `ZO_WORKFLOW_STORE` (so it can't race other tests).
    fn unique_temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("zo-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn event_log_sink_appends_and_reads_back_in_order() {
        let dir = unique_temp_dir("events-roundtrip");
        let path = dir.join("run-x.events.jsonl");
        let sink = EventLogSink::with_path("run-x".to_string(), Some(path.clone()));
        let skel = [PhaseSkeleton {
            id: "read".to_string(),
            kind: "fanout",
        }];
        sink.emit(ProgressEvent::Started {
            name: "demo",
            description: "",
            mode: "phases",
            phases: &skel,
        });
        sink.emit(ProgressEvent::PhaseEnter {
            id: "read",
            round: 1,
        });
        sink.emit(ProgressEvent::AgentsSpawned {
            phase_id: "read",
            agent_ids: &["a0".to_string()],
        });
        sink.emit(ProgressEvent::PhaseDone {
            id: "read",
            completed: 1,
            failed: 0,
            still_running: 0,
            carried: 0,
            retried: 0,
            skipped: 0,
        });
        sink.emit(ProgressEvent::Finished {
            status: "completed",
        });

        let events = read_event_log_at(&path);
        assert_eq!(events.len(), 5, "every emitted event is one appended line");
        // run_id + seq give a total order; seq is monotonic from 0.
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(events
            .iter()
            .all(|e| e.run_id == "run-x" && e.schema == EVENT_SCHEMA));
        match &events[0].event {
            WorkflowEventKind::Started { name, phases, .. } => {
                assert_eq!(name, "demo");
                assert_eq!(phases.len(), 1);
                assert_eq!(phases[0].id, "read");
                assert_eq!(phases[0].kind, "fanout");
            }
            other => panic!("expected started, got {other:?}"),
        }
        assert_eq!(
            events[4].event,
            WorkflowEventKind::Finished {
                status: "completed".to_string()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_event_log_skips_malformed_lines() {
        let dir = unique_temp_dir("events-malformed");
        let path = dir.join("run-y.events.jsonl");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let valid = serde_json::to_string(&WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: "run-y".to_string(),
            seq: 0,
            ts_ms: 1,
            event: WorkflowEventKind::SynthesizeEnter,
        })
        .expect("serialize record");
        // valid line, then garbage, then a truncated partial final write.
        std::fs::write(&path, format!("{valid}\nnot json\n{{\"partial\":")).expect("seed log");

        let events = read_event_log_at(&path);
        assert_eq!(events.len(), 1, "malformed lines are skipped, not fatal");
        assert_eq!(events[0].event, WorkflowEventKind::SynthesizeEnter);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_event_log_absent_is_empty() {
        let dir = unique_temp_dir("events-absent");
        let path = dir.join("never-written.events.jsonl");
        assert!(read_event_log_at(&path).is_empty());
    }

    #[test]
    fn prune_jsonl_event_logs_keeps_latest_files() {
        let dir = unique_temp_dir("events-retention");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        for run in 0..5 {
            std::fs::write(dir.join(format!("run-{run}.events.jsonl")), "{}\n")
                .expect("write event log");
        }
        std::fs::write(dir.join("notes.txt"), "keep me").expect("write unrelated file");

        prune_jsonl_event_logs(&dir, 3);

        let mut remaining: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.ends_with(".events.jsonl"))
            .collect();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![
                "run-2.events.jsonl".to_string(),
                "run-3.events.jsonl".to_string(),
                "run-4.events.jsonl".to_string(),
            ]
        );
        assert!(dir.join("notes.txt").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn event_log_retention_writes_marker_after_prune() {
        let dir = unique_temp_dir("events-retention-marker-write");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        for run in 0..5 {
            std::fs::write(dir.join(format!("run-{run}.events.jsonl")), "{}\n")
                .expect("write event log");
        }
        let sink = EventLogSink::with_path(
            "run-current".to_string(),
            Some(dir.join("run-current.events.jsonl")),
        );

        sink.apply_retention();

        let event_log_count = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".events.jsonl"))
            })
            .count();
        assert_eq!(event_log_count, MAX_WORKFLOW_EVENT_RUNS);
        assert!(event_retention_marker_path(&dir).exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn event_log_retention_marker_skips_recent_directory_scan() {
        let dir = unique_temp_dir("events-retention-marker-skip");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        for run in 0..5 {
            std::fs::write(dir.join(format!("run-{run}.events.jsonl")), "{}\n")
                .expect("write event log");
        }
        mark_event_retention_checked(&dir, SystemTime::now());
        let sink = EventLogSink::with_path(
            "run-current".to_string(),
            Some(dir.join("run-current.events.jsonl")),
        );

        sink.apply_retention();

        let event_log_count = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".events.jsonl"))
            })
            .count();
        assert_eq!(
            event_log_count, 5,
            "fresh marker should throttle the retention directory scan"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn event_log_retention_marker_skips_jsonl_scan_but_not_sqlite_prune() {
        let dir = unique_temp_dir("events-retention-marker-sqlite");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        for run in 0..5 {
            std::fs::write(dir.join(format!("run-{run}.events.jsonl")), "{}\n")
                .expect("write event log");
        }
        let db_path = dir.join("events.db");
        let store = SqliteEventStore::open_at(&db_path).expect("open sqlite store");
        for run in 0_u64..5 {
            store.record_event(&WorkflowEventRecord {
                schema: EVENT_SCHEMA,
                run_id: format!("run-{run}"),
                seq: 0,
                ts_ms: run,
                event: WorkflowEventKind::SynthesizeEnter,
            });
        }
        mark_event_retention_checked(&dir, SystemTime::now());
        let sink = EventLogSink::with_sqlite_at(
            "run-current".to_string(),
            Some(dir.join("run-current.events.jsonl")),
            &db_path,
        );

        sink.apply_retention();

        let event_log_count = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(".events.jsonl"))
            })
            .count();
        assert_eq!(event_log_count, 5, "fresh marker throttles JSONL scan");
        let pruned_store = SqliteEventStore::open_at(&db_path).expect("reopen sqlite store");
        assert!(
            pruned_store.read_events("run-0").is_empty(),
            "SQLite retention should still run when JSONL scan is throttled"
        );
        assert_eq!(pruned_store.read_events("run-4").len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn read_event_log_resolved_prefers_sqlite_falls_back_and_migrates() {
        let dir = unique_temp_dir("read-resolved");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("events.db");
        let jsonl_path = dir.join("run-x.events.jsonl");
        let event = WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: "run-x".to_string(),
            seq: 0,
            ts_ms: 1,
            event: WorkflowEventKind::SynthesizeEnter,
        };
        append_line(&jsonl_path, &serde_json::to_string(&event).unwrap()).unwrap();

        // No `SQLite` store → JSONL fallback (old-session compatibility).
        let none: Option<&SqliteEventStore> = None;
        assert_eq!(
            read_event_log_resolved("run-x", none, Some(&jsonl_path)).len(),
            1
        );

        // `SQLite` present but empty for the run → import the JSONL and return it.
        let store = SqliteEventStore::open_at(&db_path).unwrap();
        assert_eq!(
            read_event_log_resolved("run-x", Some(&store), Some(&jsonl_path)).len(),
            1,
            "a JSONL-only run is read on first access"
        );
        // The migration means the next read is served from `SQLite` even with the
        // JSONL gone — the store is now the source of truth for this run.
        std::fs::remove_file(&jsonl_path).unwrap();
        assert_eq!(
            read_event_log_resolved("run-x", Some(&store), Some(&jsonl_path)).len(),
            1,
            "subsequent read comes from the migrated `SQLite` store"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn emit_dual_writes_to_both_jsonl_and_sqlite() {
        let dir = unique_temp_dir("dual-write");
        std::fs::create_dir_all(&dir).unwrap();
        let jsonl = dir.join("r.events.jsonl");
        let db = dir.join("events.db");
        let sink = EventLogSink::with_sqlite_at("r".to_string(), Some(jsonl.clone()), &db);
        sink.emit(ProgressEvent::SynthesizeEnter);

        // JSONL shadow got the line…
        assert_eq!(read_event_log_at(&jsonl).len(), 1, "JSONL written");
        // …and the SQLite store got the row (read back via a fresh store).
        let store = SqliteEventStore::open_at(&db).expect("reopen db");
        assert_eq!(store.read_events("r").len(), 1, "SQLite dual-write");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_does_not_open_sqlite_until_first_emit() {
        // A sink that never emits must not materialize an empty `events.db` (the
        // regression the lazy open fixed): `new` leaves the lazy cell untouched,
        // so the store is opened only on the first `emit`.
        let sink = EventLogSink::new("r".to_string());
        assert!(
            sink.sqlite.get().is_none(),
            "lazy SQLite cell is uninitialized at construction"
        );
    }

    #[test]
    fn read_event_log_degrades_unknown_event_kind() {
        let dir = unique_temp_dir("events-forward-compat");
        let path = dir.join("run-z.events.jsonl");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        // A record from a hypothetical newer Zo: valid record shape, an event
        // `kind` this reader has never seen, plus an extra unknown field.
        let future = concat!(
            r#"{"schema":2,"run_id":"run-z","seq":0,"ts_ms":7,"#,
            r#""event":{"kind":"approval_requested","actor":"alice"}}"#
        );
        let known = serde_json::to_string(&WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: "run-z".to_string(),
            seq: 1,
            ts_ms: 8,
            event: WorkflowEventKind::SynthesizeEnter,
        })
        .expect("serialize record");
        std::fs::write(&path, format!("{future}\n{known}")).expect("seed log");

        let events = read_event_log_at(&path);
        // The unknown-kind line is kept (its slot/seq/run_id survive), not dropped.
        assert_eq!(events.len(), 2, "unknown event kinds degrade, not vanish");
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[0].run_id, "run-z");
        assert_eq!(events[0].event, WorkflowEventKind::Unknown);
        assert_eq!(events[1].event, WorkflowEventKind::SynthesizeEnter);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tee_forwards_to_every_sink() {
        let live = LiveProgressSink::detached("run-tee".to_string());
        let events = EventLogSink::detached("run-tee".to_string());
        let tee = TeeProgressSink::new(vec![
            &live as &dyn ProgressSink,
            &events as &dyn ProgressSink,
        ]);
        let skel = [PhaseSkeleton {
            id: "p0".to_string(),
            kind: "single",
        }];
        tee.emit(ProgressEvent::Started {
            name: "demo",
            description: "",
            mode: "phases",
            phases: &skel,
        });
        tee.emit(ProgressEvent::Finished {
            status: "completed",
        });

        // The snapshot sink folded both events...
        assert_eq!(live.doc.borrow().status, "completed");
        // ...and the event sink saw both (seq advanced twice, detached → no file).
        assert_eq!(events.seq.get(), 2);
    }

    #[test]
    fn event_timeline_lines_render_each_record_in_order() {
        let rec = |seq, event| WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: "r".to_string(),
            seq,
            ts_ms: seq + 1,
            event,
        };
        let records = vec![
            rec(
                0,
                WorkflowEventKind::Started {
                    name: "demo".to_string(),
                    description: String::new(),
                    mode: "phases".to_string(),
                    phases: vec![EventPhase {
                        id: "read".to_string(),
                        kind: "fanout".to_string(),
                    }],
                },
            ),
            rec(
                1,
                WorkflowEventKind::AgentsSpawned {
                    phase_id: "read".to_string(),
                    agent_ids: vec!["a0".to_string(), "a1".to_string()],
                },
            ),
            rec(
                2,
                WorkflowEventKind::Finished {
                    status: "completed".to_string(),
                },
            ),
        ];
        let lines = event_timeline_lines(&records);
        assert_eq!(lines.len(), 3, "one line per record, in order");
        assert!(lines[0].contains("started 'demo'"));
        assert!(lines[0].contains("1 phases"));
        assert!(
            lines[0].trim_start().starts_with('0'),
            "seq-prefixed: {}",
            lines[0]
        );
        assert!(lines[1].contains("spawned 2 agent"));
        assert!(lines[2].contains("finished: completed"));
    }

    #[test]
    fn event_log_terminal_status_reflects_finished_event() {
        let rec = |seq, event| WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: "r".to_string(),
            seq,
            ts_ms: seq + 1,
            event,
        };
        // No Finished event yet → the run is still in progress.
        let running = vec![rec(
            0,
            WorkflowEventKind::PhaseEnter {
                id: "p".to_string(),
                round: 1,
            },
        )];
        assert_eq!(event_log_terminal_status(&running), None);

        // A Finished event is authoritative for the terminal status...
        let mut done = running.clone();
        done.push(rec(
            1,
            WorkflowEventKind::Finished {
                status: "completed".to_string(),
            },
        ));
        assert_eq!(
            event_log_terminal_status(&done).as_deref(),
            Some("completed")
        );

        // ...including a non-completed terminal state.
        let cancelled = vec![rec(
            0,
            WorkflowEventKind::Finished {
                status: "cancelled".to_string(),
            },
        )];
        assert_eq!(
            event_log_terminal_status(&cancelled).as_deref(),
            Some("cancelled")
        );
    }

    #[test]
    fn event_phase_statuses_take_each_phases_last_lifecycle_event() {
        let rec = |seq, event| WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: "r".to_string(),
            seq,
            ts_ms: seq + 1,
            event,
        };
        let records = vec![
            // `read` runs then finishes.
            rec(
                0,
                WorkflowEventKind::PhaseEnter {
                    id: "read".to_string(),
                    round: 1,
                },
            ),
            rec(
                1,
                WorkflowEventKind::PhaseDone {
                    id: "read".to_string(),
                    completed: 2,
                    failed: 0,
                    still_running: 0,
                    carried: 0,
                    retried: 0,
                    skipped: 0,
                },
            ),
            // `test` is replayed from the resume cache.
            rec(
                2,
                WorkflowEventKind::PhaseResumed {
                    id: "test".to_string(),
                },
            ),
            // `synth` has only entered — still running.
            rec(
                3,
                WorkflowEventKind::PhaseEnter {
                    id: "synth".to_string(),
                    round: 1,
                },
            ),
        ];

        let statuses = event_phase_statuses(&records);
        assert_eq!(statuses.get("read").map(String::as_str), Some("done"));
        assert_eq!(statuses.get("test").map(String::as_str), Some("resumed"));
        assert_eq!(statuses.get("synth").map(String::as_str), Some("running"));
        // A phase the log never mentioned is absent (the snapshot's status stands).
        assert_eq!(statuses.get("never"), None);
    }

    #[test]
    fn event_phase_statuses_last_event_wins_for_a_repeat_phase() {
        let rec = |seq, event| WorkflowEventRecord {
            schema: EVENT_SCHEMA,
            run_id: "r".to_string(),
            seq,
            ts_ms: seq + 1,
            event,
        };
        // A `repeat` phase re-enters after a round's PhaseDone: the latest event
        // (round 2 entering) is its true current status, not the earlier done.
        let records = vec![
            rec(
                0,
                WorkflowEventKind::PhaseDone {
                    id: "loop".to_string(),
                    completed: 1,
                    failed: 0,
                    still_running: 0,
                    carried: 0,
                    retried: 0,
                    skipped: 0,
                },
            ),
            rec(
                1,
                WorkflowEventKind::PhaseEnter {
                    id: "loop".to_string(),
                    round: 2,
                },
            ),
        ];
        assert_eq!(
            event_phase_statuses(&records)
                .get("loop")
                .map(String::as_str),
            Some("running")
        );
    }
}
