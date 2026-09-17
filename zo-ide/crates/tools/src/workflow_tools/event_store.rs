//! Phase-6 `SQLite` event/artifact store.
//!
//! The append-only JSONL log ([`super::progress::EventLogSink`]) has been the
//! shadow source of truth since Phase-3. This module evolves it into a
//! `SQLite`-backed store **without removing the JSONL path** (doc §5.2: introduce
//! `SQLite` only once the JSONL shadow has stabilized, and keep it as the
//! compatibility fallback). The two run side by side:
//!
//! * **Dual-write** — every workflow event is written to *both* the JSONL log
//!   and the `SQLite` `events` table ([`SqliteEventStore::record_event`]). Both
//!   are best-effort: a persistence failure never breaks the run.
//! * **`SQLite`-first read with JSONL fallback + migration** —
//!   [`super::progress::read_event_log`] reads from `SQLite` when present; an old
//!   JSONL-only run (no `SQLite` rows yet) is imported on first read and then
//!   served from the store, and a missing/locked DB degrades to reading the
//!   JSONL directly (old-session compatibility + corruption recovery).
//! * **Store trait** — [`EventStore`] is the read seam both backends satisfy.
//!   Introduced now that a second impl exists (the JSONL store was the only one
//!   before), per the doc's "no one-impl abstraction" guidance.
//!
//! Concurrency: the engine thread writes through the sink's long-lived
//! connection while the TUI thread reads through its own short-lived one. WAL
//! journal mode lets a reader and a writer proceed concurrently, and every
//! connection sets a 5 s `busy_timeout` ([`SqliteEventStore::open_at`]) so a
//! transient `SQLITE_BUSY` lock is retried instead of bubbling up — `SQLite`'s
//! default is 0 (fail immediately), which would otherwise drop the TUI reader
//! straight to the JSONL fallback on any concurrent write.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

use super::cache::workflow_store_dir;
use super::progress::{read_event_log_at, WorkflowEventRecord};
use crate::artifacts::ArtifactRef;

/// One `SQLite` database for the whole store, alongside the JSONL logs and resume
/// caches under `.zo/workflows/`. Holds the `events`, `artifacts`, and
/// `schema_version` tables.
const EVENTS_DB_FILE: &str = "events.db";

/// On-disk store schema version, stamped once into `schema_version`. Bump when
/// the table shapes change; the reader tolerates unknown event `kind`s at the
/// record level (the JSON column round-trips through
/// [`super::progress::WorkflowEventKind`]'s `#[serde(other)]`).
const SCHEMA_VERSION: i64 = 2;

/// The read seam over a workflow event log. Both backends — the original
/// append-only JSONL ([`JsonlEventStore`]) and the Phase-6 `SQLite` store
/// ([`SqliteEventStore`]) — satisfy it, so [`super::progress::read_event_log`]
/// can read whichever is present without caring which.
pub(crate) trait EventStore {
    /// A run's events in `seq` order, lossily skipping any record that fails to
    /// parse (a partially written / corrupt row is dropped, never fatal —
    /// mirroring the JSONL reader).
    fn read_events(&self, run_id: &str) -> Vec<WorkflowEventRecord>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunIndexRow {
    pub(crate) run_id: String,
    pub(crate) event_count: usize,
    pub(crate) first_ts_ms: u64,
    pub(crate) last_ts_ms: u64,
}

/// JSONL-backed [`EventStore`]: the pre-Phase-6 log, now the compatibility
/// fallback and the migration source. A `None` path (no resolvable store dir)
/// reads as empty.
pub(crate) struct JsonlEventStore {
    pub(crate) path: Option<PathBuf>,
}

impl EventStore for JsonlEventStore {
    fn read_events(&self, _run_id: &str) -> Vec<WorkflowEventRecord> {
        self.path
            .as_deref()
            .map(read_event_log_at)
            .unwrap_or_default()
    }
}

/// `SQLite`-backed event/artifact store (the Phase-6 evolution of the JSONL log).
pub(crate) struct SqliteEventStore {
    conn: Connection,
}

impl SqliteEventStore {
    /// Open (creating if absent) the store at the resolved
    /// `.zo/workflows/events.db`, or `None` when no store dir is resolvable or
    /// the open fails. The **write** seam — used by [`EventLogSink::new`], which
    /// legitimately materializes the DB on the run's first event. Readers use
    /// [`Self::open_existing`] so a mere read never creates the DB.
    ///
    /// [`EventLogSink::new`]: super::progress::EventLogSink
    pub(crate) fn open() -> Option<Self> {
        let dir = workflow_store_dir()?;
        Self::open_at(&dir.join(EVENTS_DB_FILE)).ok()
    }

    /// Open an **existing** store, or `None` when the DB file is absent (or no
    /// store dir resolves). The **read** seam: a read — including the TUI's
    /// per-poll [`super::progress::read_event_log`] — must never materialize an
    /// empty `events.db` as a side effect, so callers fall back to the JSONL log
    /// when this returns `None`.
    ///
    /// Opens a fresh connection per call, keeping the readers free of shared
    /// state (and correct under the test/`-p` `ZO_WORKFLOW_STORE` redirect);
    /// the schema is tiny and WAL keeps the open sub-millisecond. A thread-local
    /// connection cache is the obvious optimization if it ever shows on a
    /// profile.
    pub(crate) fn open_existing() -> Option<Self> {
        let path = workflow_store_dir()?.join(EVENTS_DB_FILE);
        if !path.exists() {
            return None;
        }
        Self::open_at(&path).ok()
    }

    /// Open (creating if absent) the store at an explicit path. `pub(crate)` so
    /// tests drive a temp DB without the process-global `ZO_WORKFLOW_STORE`
    /// (the env-free pattern the resume-cache tests use).
    pub(crate) fn open_at(db_path: &Path) -> rusqlite::Result<Self> {
        if let Some(parent) = db_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let conn = Connection::open(db_path)?;
        // Retry transient `SQLITE_BUSY` for up to 5 s instead of failing
        // immediately (SQLite's default is 0). Without this a concurrent
        // write makes the TUI reader fall straight through to the JSONL path.
        conn.busy_timeout(Duration::from_secs(5))?;
        // WAL: a reader (TUI) and the writer (engine) proceed concurrently.
        // Best-effort — a filesystem that rejects WAL (e.g. some network mounts)
        // simply stays in the default rollback journal.
        let _ = conn.pragma_update(None, "journal_mode", "WAL");
        conn.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS events (
                 run_id TEXT    NOT NULL,
                 seq    INTEGER NOT NULL,
                 ts_ms  INTEGER NOT NULL,
                 schema INTEGER NOT NULL,
                 record TEXT    NOT NULL,
                 PRIMARY KEY (run_id, seq)
             );
             CREATE TABLE IF NOT EXISTS artifacts (
                 sha256       TEXT PRIMARY KEY,
                 kind         TEXT    NOT NULL,
                 size_bytes   INTEGER NOT NULL,
                 last_seen_ms INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
             COMMIT;",
        )?;
        // Best-effort migration for stores created before artifact recency was
        // tracked. Duplicate-column errors are ignored so opening remains
        // idempotent across versions.
        let _ = conn.execute(
            "ALTER TABLE artifacts ADD COLUMN last_seen_ms INTEGER NOT NULL DEFAULT 0",
            [],
        );
        // Stamp the schema version exactly once.
        conn.execute(
            "INSERT INTO schema_version (version)
             SELECT ?1 WHERE NOT EXISTS (SELECT 1 FROM schema_version)",
            [SCHEMA_VERSION],
        )?;
        Ok(Self { conn })
    }

    /// Persist one event. Keyed by `(run_id, seq)` with `INSERT OR IGNORE`, so a
    /// re-emit (a resumed run restarts `seq` at 0 and re-appends) or a duplicate
    /// migration is idempotent. Best-effort: a failed write is swallowed.
    pub(crate) fn record_event(&self, record: &WorkflowEventRecord) {
        let Ok(json) = serde_json::to_string(record) else {
            return;
        };
        let _ = self.conn.execute(
            "INSERT OR IGNORE INTO events (run_id, seq, ts_ms, schema, record)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![record.run_id, record.seq, record.ts_ms, record.schema, json],
        );
    }

    /// Bulk-import records (the JSONL → `SQLite` migration) in a single
    /// transaction. Idempotent via `record_event`'s `INSERT OR IGNORE`. A failed
    /// transaction degrades to per-row best-effort.
    pub(crate) fn import(&self, records: &[WorkflowEventRecord]) {
        match self.conn.unchecked_transaction() {
            Ok(tx) => {
                for record in records {
                    self.record_event(record);
                }
                let _ = tx.commit();
            }
            Err(_) => {
                for record in records {
                    self.record_event(record);
                }
            }
        }
    }

    /// List recent workflow runs by their event timestamps. Corrupt aggregate
    /// rows are skipped by returning an empty list on query failure; callers can
    /// fall back to JSONL when this store is absent.
    pub(crate) fn list_runs(&self, limit: usize) -> Vec<RunIndexRow> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX).max(0);
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT run_id, COUNT(*), MIN(ts_ms), MAX(ts_ms)
             FROM events
             GROUP BY run_id
             ORDER BY MAX(ts_ms) DESC
             LIMIT ?1",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([limit], |row| {
            let event_count: i64 = row.get(1)?;
            let first_ts_ms: i64 = row.get(2)?;
            let last_ts_ms: i64 = row.get(3)?;
            Ok(RunIndexRow {
                run_id: row.get(0)?,
                event_count: usize::try_from(event_count).unwrap_or_default(),
                first_ts_ms: u64::try_from(first_ts_ms).unwrap_or_default(),
                last_ts_ms: u64::try_from(last_ts_ms).unwrap_or_default(),
            })
        }) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok).collect()
    }

    /// Best-effort retention: keep the latest workflow runs and bound artifact
    /// metadata rows so the process-global store cannot grow without limit.
    pub(crate) fn prune_retention(&self, max_runs: usize, max_artifact_rows: usize) -> Vec<String> {
        let max_runs = i64::try_from(max_runs).unwrap_or(i64::MAX);
        let max_artifact_rows = i64::try_from(max_artifact_rows).unwrap_or(i64::MAX);
        let _ = self.conn.execute(
            "DELETE FROM events
             WHERE run_id IN (
                 SELECT run_id FROM (
                     SELECT run_id, MAX(ts_ms) AS last_ts
                     FROM events
                     GROUP BY run_id
                     ORDER BY last_ts DESC, run_id DESC
                     LIMIT -1 OFFSET ?1
                 )
             )",
            [max_runs],
        );

        let pruned_artifacts = self.artifact_shas_over_limit(max_artifact_rows);
        for sha in &pruned_artifacts {
            let _ = self
                .conn
                .execute("DELETE FROM artifacts WHERE sha256 = ?1", [sha]);
        }
        pruned_artifacts
    }

    fn artifact_shas_over_limit(&self, max_artifact_rows: i64) -> Vec<String> {
        let Ok(mut stmt) = self.conn.prepare(
            "SELECT sha256 FROM artifacts
             ORDER BY last_seen_ms ASC, rowid ASC
             LIMIT (
                 SELECT CASE
                     WHEN COUNT(*) > ?1 THEN COUNT(*) - ?1
                     ELSE 0
                 END
                 FROM artifacts
             )",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([max_artifact_rows], |row| row.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok).collect()
    }

    /// Record artifact metadata (content-addressed by `sha256`). The bytes stay
    /// in the content-addressed file store ([`crate::artifacts`]); the `SQLite`
    /// `artifacts` table is the audit/dedup index. Idempotent on `sha256`,
    /// best-effort.
    pub(crate) fn record_artifact(&self, artifact: &ArtifactRef) {
        let kind = serde_json::to_value(artifact.kind)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        let _ = self.conn.execute(
            "INSERT INTO artifacts (sha256, kind, size_bytes, last_seen_ms)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(sha256) DO UPDATE SET
                 kind = excluded.kind,
                 size_bytes = excluded.size_bytes,
                 last_seen_ms = excluded.last_seen_ms",
            rusqlite::params![artifact.sha256, kind, artifact.size_bytes, now_ms()],
        );
    }
}

fn now_ms() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    i64::try_from(millis).unwrap_or(i64::MAX)
}

impl EventStore for SqliteEventStore {
    fn read_events(&self, run_id: &str) -> Vec<WorkflowEventRecord> {
        let Ok(mut stmt) = self
            .conn
            .prepare("SELECT record FROM events WHERE run_id = ?1 ORDER BY seq")
        else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([run_id], |row| row.get::<_, String>(0)) else {
            return Vec::new();
        };
        rows.filter_map(Result::ok)
            // Corruption recovery: a row whose JSON won't parse is skipped, not
            // fatal — the same tolerance as the JSONL reader.
            .filter_map(|json| serde_json::from_str::<WorkflowEventRecord>(&json).ok())
            .collect()
    }
}

/// Best-effort artifact-metadata record into the `SQLite` store. Called from the
/// production artifact path after the bytes are persisted.
///
/// Uses [`SqliteEventStore::open_existing`], so a plain non-workflow session that
/// merely truncated an output doesn't materialize `events.db` just to index one
/// artifact — the table is populated only while a workflow's store is already
/// live. A missing/locked store is silently skipped; artifacts never depend on
/// `SQLite`.
pub(crate) fn record_artifact_meta(artifact: &ArtifactRef) {
    if let Some(store) = SqliteEventStore::open_existing() {
        store.record_artifact(artifact);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifacts::ArtifactKind;
    use crate::workflow_tools::WorkflowEventKind;

    fn temp_db(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zo-eventstore-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir.join(EVENTS_DB_FILE)
    }

    fn record(run_id: &str, seq: u64, event: WorkflowEventKind) -> WorkflowEventRecord {
        WorkflowEventRecord {
            schema: u32::try_from(SCHEMA_VERSION).expect("schema version fits u32"),
            run_id: run_id.to_string(),
            seq,
            ts_ms: seq + 1,
            event,
        }
    }

    #[test]
    fn record_then_read_round_trips_in_seq_order() {
        let path = temp_db("roundtrip");
        let store = SqliteEventStore::open_at(&path).expect("open");
        store.record_event(&record(
            "r",
            1,
            WorkflowEventKind::PhaseEnter {
                id: "read".to_string(),
                round: 1,
            },
        ));
        // Inserted out of order — the read must still come back ordered by seq.
        store.record_event(&record(
            "r",
            0,
            WorkflowEventKind::Started {
                name: "wf".to_string(),
                description: String::new(),
                mode: "phases".to_string(),
                phases: Vec::new(),
            },
        ));
        let events = store.read_events("r");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[1].seq, 1);
        assert!(matches!(events[0].event, WorkflowEventKind::Started { .. }));
        // A different run id is isolated.
        assert!(store.read_events("other").is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn artifact_retention_uses_last_seen_not_original_rowid() {
        let path = temp_db("artifact-last-seen");
        let store = SqliteEventStore::open_at(&path).expect("open store");

        for sha in ["sha-old", "sha-mid", "sha-reused"] {
            store.record_artifact(&ArtifactRef {
                sha256: sha.to_string(),
                size_bytes: 1,
                kind: crate::artifacts::ArtifactKind::Generic,
                preview: String::new(),
            });
        }
        store
            .conn
            .execute(
                "UPDATE artifacts SET last_seen_ms = 1 WHERE sha256 = 'sha-reused'",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE artifacts SET last_seen_ms = 2 WHERE sha256 = 'sha-old'",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "UPDATE artifacts SET last_seen_ms = 3 WHERE sha256 = 'sha-mid'",
                [],
            )
            .unwrap();

        store.record_artifact(&ArtifactRef {
            sha256: "sha-reused".to_string(),
            size_bytes: 1,
            kind: crate::artifacts::ArtifactKind::Generic,
            preview: String::new(),
        });

        let pruned = store.prune_retention(usize::MAX, 2);
        assert_eq!(pruned, vec!["sha-old".to_string()]);
        let remaining: Vec<String> = store
            .conn
            .prepare("SELECT sha256 FROM artifacts ORDER BY sha256")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(remaining, vec!["sha-mid", "sha-reused"]);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn record_event_is_idempotent_on_run_id_seq() {
        let path = temp_db("idempotent");
        let store = SqliteEventStore::open_at(&path).expect("open");
        let ev = record("r", 0, WorkflowEventKind::SynthesizeEnter);
        store.record_event(&ev);
        store.record_event(&ev); // duplicate (run_id, seq) → INSERT OR IGNORE
        assert_eq!(store.read_events("r").len(), 1, "no duplicate row");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn prune_retention_keeps_latest_runs_and_bounds_artifacts() {
        let path = temp_db("retention");
        let store = SqliteEventStore::open_at(&path).expect("open");
        for run in 0..5 {
            store.record_event(&record(
                &format!("r{run}"),
                0,
                WorkflowEventKind::PhaseEnter {
                    id: format!("phase-{run}"),
                    round: 1,
                },
            ));
        }
        for index in 0..5 {
            store
                .conn
                .execute(
                    "INSERT INTO artifacts (sha256, kind, size_bytes) VALUES (?1, 'file', ?2)",
                    rusqlite::params![format!("sha-{index}"), index],
                )
                .unwrap();
        }

        let pruned_artifacts = store.prune_retention(2, 2);

        assert!(store.read_events("r0").is_empty());
        assert!(store.read_events("r1").is_empty());
        assert!(store.read_events("r2").is_empty());
        assert_eq!(store.read_events("r3").len(), 1);
        assert_eq!(store.read_events("r4").len(), 1);
        let artifact_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM artifacts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(artifact_count, 2);
        assert_eq!(
            pruned_artifacts,
            vec![
                "sha-0".to_string(),
                "sha-1".to_string(),
                "sha-2".to_string()
            ]
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn read_events_skips_corrupt_rows() {
        let path = temp_db("corrupt");
        let store = SqliteEventStore::open_at(&path).expect("open");
        store.record_event(&record("r", 0, WorkflowEventKind::SynthesizeEnter));
        // Inject a row whose `record` column is not valid JSON.
        store
            .conn
            .execute(
                "INSERT INTO events (run_id, seq, ts_ms, schema, record) VALUES ('r', 1, 1, 1, '{ not json')",
                [],
            )
            .unwrap();
        let events = store.read_events("r");
        assert_eq!(events.len(), 1, "the corrupt row is skipped, not fatal");
        assert_eq!(events[0].seq, 0);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn import_migrates_jsonl_records_idempotently() {
        let path = temp_db("import");
        let store = SqliteEventStore::open_at(&path).expect("open");
        let records = vec![
            record("r", 0, WorkflowEventKind::SynthesizeEnter),
            record(
                "r",
                1,
                WorkflowEventKind::Finished {
                    status: "completed".to_string(),
                },
            ),
        ];
        store.import(&records);
        store.import(&records); // re-import is a no-op
        let events = store.read_events("r");
        assert_eq!(events.len(), 2);
        assert_eq!(
            super::super::progress::event_log_terminal_status(&events).as_deref(),
            Some("completed"),
            "the migrated run reconstructs its terminal status from the store"
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn artifact_metadata_round_trips() {
        let path = temp_db("artifact");
        let store = SqliteEventStore::open_at(&path).expect("open");
        let artifact = ArtifactRef {
            sha256: "abc123".to_string(),
            size_bytes: 4096,
            kind: ArtifactKind::TestLog,
            preview: "irrelevant".to_string(),
        };
        store.record_artifact(&artifact);
        store.record_artifact(&artifact); // idempotent on sha256
        let (count, size): (i64, i64) = store
            .conn
            .query_row(
                "SELECT COUNT(*), COALESCE(MAX(size_bytes), 0) FROM artifacts WHERE sha256 = ?1",
                ["abc123"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 1, "deduped on sha256");
        assert_eq!(size, 4096, "size persisted");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reopening_a_store_keeps_its_rows_and_one_schema_row() {
        let path = temp_db("reopen");
        {
            let store = SqliteEventStore::open_at(&path).expect("open");
            store.record_event(&record("r", 0, WorkflowEventKind::SynthesizeEnter));
        }
        // Re-open the same file: schema creation is idempotent, data persists.
        let store = SqliteEventStore::open_at(&path).expect("reopen");
        assert_eq!(store.read_events("r").len(), 1);
        let versions: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM schema_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(versions, 1, "schema version stamped exactly once");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
