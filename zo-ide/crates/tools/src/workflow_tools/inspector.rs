//! Read-only workflow run inspector (C3).
//!
//! Owns timeline/list views over the existing event store. This module never
//! writes; production listing opens `SQLite` only with `open_existing` and falls
//! back to already-present JSONL logs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{json, Value};

use super::cache::workflow_store_dir;
use super::event_store::{EventStore, RunIndexRow, SqliteEventStore};
use super::progress::{event_log_terminal_status, read_event_log, read_event_log_at};
use super::{WorkflowEventKind, WorkflowEventRecord};
use crate::ToolError;

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;
const RAW_TAIL_LIMIT: usize = 100;

#[derive(Debug, Deserialize)]
struct WorkflowRunsInput {
    #[serde(default)]
    run_id: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

pub(crate) fn run(input: &Value) -> Result<String, ToolError> {
    let input: WorkflowRunsInput = serde_json::from_value(input.clone())
        .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    let output = if let Some(run_id) = input.run_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        show_run(run_id, &read_event_log(run_id))
    } else {
        let limit = clamp_limit(input.limit);
        list_runs(limit)
    };
    crate::to_pretty_json(output)
}

fn clamp_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
}

fn list_runs(limit: usize) -> Value {
    let sqlite = SqliteEventStore::open_existing();
    let jsonl_dir = workflow_store_dir();
    list_runs_merged(sqlite.as_ref(), jsonl_dir.as_deref(), limit)
}

/// Merge the `SQLite` run index with any JSONL-only logs. A hybrid store is
/// normal mid-migration: `read_event_log` imports a JSONL run into `SQLite`
/// only when that specific run is first *read*, so runs that predate the DB
/// (or were written while it was locked) exist only as `<run_id>.events.jsonl`.
/// Listing from `SQLite` alone would silently omit them — and then
/// `WorkflowRuns {run_id}` would still show the run, an inconsistency worse
/// than the extra directory scan. `SQLite` rows win on conflict (they are the
/// migrated superset); ordering is `last_ts_ms DESC` across both sources, then
/// `limit`.
fn list_runs_merged(
    sqlite: Option<&SqliteEventStore>,
    jsonl_dir: Option<&Path>,
    limit: usize,
) -> Value {
    let mut rows: Vec<(RunIndexRow, Vec<WorkflowEventRecord>)> = Vec::new();
    if let Some(store) = sqlite {
        // Fetch up to `limit` newest runs from the index; events are re-read
        // per run for the status/phase summary.
        for row in store.list_runs(limit) {
            let events = store.read_events(&row.run_id);
            rows.push((row, events));
        }
    }
    if let Some(dir) = jsonl_dir {
        let seen: BTreeSet<String> = rows.iter().map(|(row, _)| row.run_id.clone()).collect();
        rows.extend(jsonl_run_rows(dir).into_iter().filter(|(row, _)| !seen.contains(&row.run_id)));
    }
    rows.sort_by(|(left, _), (right, _)| {
        right
            .last_ts_ms
            .cmp(&left.last_ts_ms)
            .then_with(|| left.run_id.cmp(&right.run_id))
    });
    rows.truncate(limit);
    let runs: Vec<Value> = rows
        .iter()
        .map(|(row, events)| run_list_entry(row, events))
        .collect();
    json!({ "runs": runs, "count": runs.len() })
}

fn jsonl_run_rows(dir: &Path) -> Vec<(RunIndexRow, Vec<WorkflowEventRecord>)> {
    let mut rows = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return rows;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(run_id) = jsonl_run_id(&path) else {
            continue;
        };
        let events = read_event_log_at(&path);
        if events.is_empty() {
            continue;
        }
        let first_ts_ms = events.first().map_or(0, |record| record.ts_ms);
        let last_ts_ms = events.last().map_or(0, |record| record.ts_ms);
        rows.push((
            RunIndexRow {
                run_id,
                event_count: events.len(),
                first_ts_ms,
                last_ts_ms,
            },
            events,
        ));
    }
    rows
}

fn jsonl_run_id(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    name.strip_suffix(".events.jsonl").map(str::to_string)
}

fn run_list_entry(row: &RunIndexRow, events: &[WorkflowEventRecord]) -> Value {
    let started = started_summary(events);
    json!({
        "run_id": row.run_id,
        "event_count": row.event_count,
        "first_ts_ms": row.first_ts_ms,
        "last_ts_ms": row.last_ts_ms,
        "status": derive_status(events),
        "workflow": started.get("name").cloned().unwrap_or(Value::Null),
        "description": started.get("description").cloned().unwrap_or(Value::Null),
        "mode": started.get("mode").cloned().unwrap_or(Value::Null),
        "phase_count": started.get("phase_count").cloned().unwrap_or(Value::Null),
    })
}

fn started_summary(events: &[WorkflowEventRecord]) -> serde_json::Map<String, Value> {
    events
        .iter()
        .find_map(|record| match &record.event {
            WorkflowEventKind::Started {
                name,
                description,
                mode,
                phases,
            } => Some(json!({
                "name": name,
                "description": description,
                "mode": mode,
                "phase_count": phases.len(),
            })),
            _ => None,
        })
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

fn derive_status(events: &[WorkflowEventRecord]) -> String {
    event_log_terminal_status(events).unwrap_or_else(|| "running/incomplete".to_string())
}

fn show_run(run_id: &str, events: &[WorkflowEventRecord]) -> Value {
    let total_events = events.len();
    let started_ts_ms = events.first().map(|record| record.ts_ms);
    let finished_ts_ms = events
        .iter()
        .rev()
        .find(|record| matches!(record.event, WorkflowEventKind::Finished { .. }))
        .map(|record| record.ts_ms);
    let tail_start = total_events.saturating_sub(RAW_TAIL_LIMIT);
    let tail: Vec<Value> = events
        .iter()
        .skip(tail_start)
        .filter_map(|record| serde_json::to_value(record).ok())
        .collect();
    json!({
        "run_id": run_id,
        "status": derive_status(events),
        "started_ts_ms": started_ts_ms,
        "finished_ts_ms": finished_ts_ms,
        "workflow": started_summary(events),
        "phases": phase_summaries(events),
        "agents_by_phase": agents_by_phase(events),
        "findings": finding_events(events),
        "events": tail,
        "total_events": total_events,
    })
}

fn phase_summaries(events: &[WorkflowEventRecord]) -> Vec<Value> {
    #[derive(Default)]
    struct PhaseState {
        id: String,
        kind: Option<String>,
        enters: Vec<Value>,
        done: Option<Value>,
        resumed: bool,
    }

    let mut phases: BTreeMap<String, PhaseState> = BTreeMap::new();
    let mut order = Vec::new();
    let ensure_phase = |id: &str,
                        kind: Option<&str>,
                        phases: &mut BTreeMap<String, PhaseState>,
                        order: &mut Vec<String>| {
        if !phases.contains_key(id) {
            order.push(id.to_string());
        }
        let phase = phases.entry(id.to_string()).or_default();
        phase.id = id.to_string();
        if let Some(kind) = kind {
            phase.kind = Some(kind.to_string());
        }
    };

    for record in events {
        match &record.event {
            WorkflowEventKind::Started { phases: skeleton, .. } => {
                for phase in skeleton {
                    ensure_phase(&phase.id, Some(&phase.kind), &mut phases, &mut order);
                }
            }
            WorkflowEventKind::PhaseEnter { id, round } => {
                ensure_phase(id, None, &mut phases, &mut order);
                if let Some(phase) = phases.get_mut(id) {
                    phase.enters.push(json!({ "seq": record.seq, "ts_ms": record.ts_ms, "round": round }));
                }
            }
            WorkflowEventKind::PhaseDone {
                id,
                completed,
                failed,
                still_running,
                carried,
                retried,
                skipped,
            } => {
                ensure_phase(id, None, &mut phases, &mut order);
                if let Some(phase) = phases.get_mut(id) {
                    phase.done = Some(json!({
                        "seq": record.seq,
                        "ts_ms": record.ts_ms,
                        "completed": completed,
                        "failed": failed,
                        "still_running": still_running,
                        "carried": carried,
                        "retried": retried,
                        "skipped": skipped,
                    }));
                }
            }
            WorkflowEventKind::PhaseResumed { id } => {
                ensure_phase(id, None, &mut phases, &mut order);
                if let Some(phase) = phases.get_mut(id) {
                    phase.resumed = true;
                }
            }
            _ => {}
        }
    }

    order
        .into_iter()
        .filter_map(|id| phases.remove(&id))
        .map(|phase| {
            json!({
                "id": phase.id,
                "kind": phase.kind,
                "status": phase.done.as_ref().map_or(if phase.resumed { "resumed" } else { "running/incomplete" }, |_| "done"),
                "enters": phase.enters,
                "done": phase.done,
                "resumed": phase.resumed,
            })
        })
        .collect()
}

fn agents_by_phase(events: &[WorkflowEventRecord]) -> Value {
    let mut spawned: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut done: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for record in events {
        match &record.event {
            WorkflowEventKind::AgentsSpawned { phase_id, agent_ids } => {
                spawned
                    .entry(phase_id.clone())
                    .or_default()
                    .extend(agent_ids.iter().cloned());
            }
            WorkflowEventKind::AgentDone {
                phase_id,
                agent_id,
                status,
            } => done.entry(phase_id.clone()).or_default().push(json!({
                "seq": record.seq,
                "ts_ms": record.ts_ms,
                "agent_id": agent_id,
                "status": status,
            })),
            _ => {}
        }
    }

    let phase_ids: BTreeSet<String> = spawned.keys().chain(done.keys()).cloned().collect();
    let mut out = serde_json::Map::new();
    for phase_id in phase_ids {
        let spawned_ids: Vec<String> = spawned
            .remove(&phase_id)
            .unwrap_or_default()
            .into_iter()
            .collect();
        let done_events = done.remove(&phase_id).unwrap_or_default();
        out.insert(
            phase_id,
            json!({
                "spawned_count": spawned_ids.len(),
                "spawned": spawned_ids,
                "done_count": done_events.len(),
                "done": done_events,
            }),
        );
    }
    Value::Object(out)
}

fn finding_events(events: &[WorkflowEventRecord]) -> Vec<Value> {
    let mut out = Vec::new();
    for record in events {
        match &record.event {
            WorkflowEventKind::FindingQueued { phase_id, finding_id } => out.push(json!({
                "kind": "finding_queued", "seq": record.seq, "ts_ms": record.ts_ms,
                "phase_id": phase_id, "finding_id": finding_id,
            })),
            WorkflowEventKind::SelectiveRetryStarted { phase_id, finding_id } => out.push(json!({
                "kind": "selective_retry_started", "seq": record.seq, "ts_ms": record.ts_ms,
                "phase_id": phase_id, "finding_id": finding_id,
            })),
            WorkflowEventKind::FindingBlocked { phase_id, finding_id, reason } => out.push(json!({
                "kind": "finding_blocked", "seq": record.seq, "ts_ms": record.ts_ms,
                "phase_id": phase_id, "finding_id": finding_id, "reason": reason,
            })),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
fn list_runs_at_for_test(dir: &Path, sqlite: Option<&SqliteEventStore>, limit: usize) -> Value {
    list_runs_merged(sqlite, Some(dir), limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflow_tools::progress::EventPhase;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-workflow-runs-{tag}-{}-{unique}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn record(run_id: &str, seq: u64, event: WorkflowEventKind) -> WorkflowEventRecord {
        WorkflowEventRecord {
            schema: 1,
            run_id: run_id.to_string(),
            seq,
            ts_ms: seq + 10,
            event,
        }
    }

    fn started(run_id: &str, seq: u64, phase_ids: &[&str]) -> WorkflowEventRecord {
        record(
            run_id,
            seq,
            WorkflowEventKind::Started {
                name: format!("wf-{run_id}"),
                description: "desc".to_string(),
                mode: "phases".to_string(),
                phases: phase_ids
                    .iter()
                    .map(|id| EventPhase {
                        id: (*id).to_string(),
                        kind: "single".to_string(),
                    })
                    .collect(),
            },
        )
    }

    #[test]
    fn run_list_from_temp_sqlite_store_derives_status_and_phase_count() {
        let dir = temp_dir("sqlite-list");
        let db_path = dir.join("events.db");
        let store = SqliteEventStore::open_at(&db_path).expect("open sqlite");
        for event in [
            started("run-a", 0, &["one"]),
            record(
                "run-a",
                1,
                WorkflowEventKind::Finished {
                    status: "completed".to_string(),
                },
            ),
            started("run-b", 10, &["one", "two"]),
            record(
                "run-b",
                11,
                WorkflowEventKind::PhaseEnter {
                    id: "one".to_string(),
                    round: 1,
                },
            ),
        ] {
            store.record_event(&event);
        }

        let listed = list_runs_at_for_test(&dir, Some(&store), 20);
        assert_eq!(listed["count"], 2);
        assert_eq!(listed["runs"][0]["run_id"], "run-b");
        assert_eq!(listed["runs"][0]["status"], "running/incomplete");
        assert_eq!(listed["runs"][0]["phase_count"], 2);
        assert_eq!(listed["runs"][1]["status"], "completed");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn run_show_timeline_shape_and_bounded_tail() {
        let mut events = vec![
            started("run-show", 0, &["build"]),
            record(
                "run-show",
                1,
                WorkflowEventKind::PhaseEnter {
                    id: "build".to_string(),
                    round: 1,
                },
            ),
            record(
                "run-show",
                2,
                WorkflowEventKind::AgentsSpawned {
                    phase_id: "build".to_string(),
                    agent_ids: vec!["a1".to_string()],
                },
            ),
            record(
                "run-show",
                3,
                WorkflowEventKind::AgentDone {
                    phase_id: "build".to_string(),
                    agent_id: "a1".to_string(),
                    status: "completed".to_string(),
                },
            ),
            record(
                "run-show",
                4,
                WorkflowEventKind::FindingQueued {
                    phase_id: "build".to_string(),
                    finding_id: "f1".to_string(),
                },
            ),
            record(
                "run-show",
                5,
                WorkflowEventKind::PhaseDone {
                    id: "build".to_string(),
                    completed: 1,
                    failed: 0,
                    still_running: 0,
                    carried: 2,
                    retried: 3,
                    skipped: 4,
                },
            ),
            record(
                "run-show",
                6,
                WorkflowEventKind::Finished {
                    status: "completed".to_string(),
                },
            ),
        ];
        for seq in 7..130 {
            events.push(record("run-show", seq, WorkflowEventKind::SynthesizeEnter));
        }
        let shown = show_run("run-show", &events);
        assert_eq!(shown["status"], "completed");
        assert_eq!(shown["total_events"], 130);
        assert_eq!(shown["events"].as_array().unwrap().len(), RAW_TAIL_LIMIT);
        assert_eq!(shown["phases"][0]["done"]["completed"], 1);
        assert_eq!(shown["phases"][0]["done"]["carried"], 2);
        assert_eq!(shown["agents_by_phase"]["build"]["done"][0]["status"], "completed");
        assert_eq!(shown["findings"][0]["finding_id"], "f1");
    }

    #[test]
    fn empty_store_list_is_clean() {
        let dir = temp_dir("empty");
        let listed = list_runs_at_for_test(&dir, None, 20);
        assert_eq!(listed, json!({ "runs": [], "count": 0 }));
        let _ = fs::remove_dir_all(dir);
    }

    /// Hybrid store: a run that exists only as `<run_id>.events.jsonl` (not
    /// yet imported into `SQLite`) must still appear in the list, merged and
    /// ordered by `last_ts_ms DESC` with `SQLite` rows winning on conflict.
    /// Guards the regression where an existing `events.db` made listing go
    /// `SQLite`-only and silently drop JSONL-only runs that `WorkflowRuns
    /// {run_id}` could still show.
    #[test]
    fn hybrid_store_lists_jsonl_only_runs_alongside_sqlite() {
        let dir = temp_dir("hybrid");
        let store = SqliteEventStore::open_at(&dir.join("events.db")).expect("open sqlite");
        // SQLite-known run (older), duplicated in JSONL to prove SQLite wins.
        for event in [
            started("run-db", 0, &["one"]),
            record(
                "run-db",
                1,
                WorkflowEventKind::Finished {
                    status: "completed".to_string(),
                },
            ),
        ] {
            store.record_event(&event);
        }
        let stale_duplicate = serde_json::to_string(&started("run-db", 0, &["one"])).unwrap();
        fs::write(dir.join("run-db.events.jsonl"), format!("{stale_duplicate}\n"))
            .expect("write duplicate jsonl");
        // JSONL-only run (newer): never touched SQLite.
        let jsonl_events = [
            started("run-jsonl", 20, &["solo"]),
            record(
                "run-jsonl",
                21,
                WorkflowEventKind::Finished {
                    status: "failed".to_string(),
                },
            ),
        ];
        let mut lines = String::new();
        for event in &jsonl_events {
            lines.push_str(&serde_json::to_string(event).unwrap());
            lines.push('\n');
        }
        fs::write(dir.join("run-jsonl.events.jsonl"), lines).expect("write jsonl log");

        let listed = list_runs_at_for_test(&dir, Some(&store), 20);
        assert_eq!(listed["count"], 2, "jsonl-only run must not be dropped");
        assert_eq!(listed["runs"][0]["run_id"], "run-jsonl");
        assert_eq!(listed["runs"][0]["status"], "failed");
        assert_eq!(listed["runs"][1]["run_id"], "run-db");
        assert_eq!(
            listed["runs"][1]["event_count"], 2,
            "sqlite row wins over the stale jsonl duplicate"
        );
        // limit=1 keeps only the newest across both sources.
        let limited = list_runs_at_for_test(&dir, Some(&store), 1);
        assert_eq!(limited["count"], 1);
        assert_eq!(limited["runs"][0]["run_id"], "run-jsonl");
        let _ = fs::remove_dir_all(dir);
    }
}
