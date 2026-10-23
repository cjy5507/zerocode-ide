//! Process-exit observations, separate from a worker attempt's completion.
//!
//! A failed kill must not hold the ledger's single host-effect fence forever.
//! These rows share its private SQLite store, but authorize no host action:
//! recovery may observe a saved process identity, never signal a saved PID.
use rusqlite::{OptionalExtension as _, params};
use zerocode_core::orchestration::ExitWitness;

use crate::workflow_store::{WorkflowStore, WorkflowStoreError};

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS terminal_exit_observations (
    identity TEXT PRIMARY KEY NOT NULL CHECK(length(identity) = 64),
    term INTEGER NOT NULL CHECK(term BETWEEN 0 AND 4294967295),
    process_group INTEGER CHECK(process_group BETWEEN 1 AND 4294967295),
    process_started TEXT CHECK(length(process_started) BETWEEN 1 AND 128),
    state TEXT NOT NULL CHECK(state IN ('requested', 'unconfirmed', 'failed', 'gone')),
    signal_failed INTEGER NOT NULL DEFAULT 0 CHECK(signal_failed IN (0, 1)),
    requested_ms INTEGER NOT NULL CHECK(requested_ms >= 0),
    observed_ms INTEGER NOT NULL CHECK(observed_ms >= requested_ms),
    CHECK(process_started IS NULL OR process_group IS NOT NULL)
);
CREATE INDEX IF NOT EXISTS terminal_exit_pending ON terminal_exit_observations(observed_ms)
    WHERE state != 'gone' AND process_group IS NOT NULL;";

/// Only `Gone` proves an exit. A successful signal is still `Unconfirmed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitState {
    Requested,
    Unconfirmed,
    Failed,
    Gone,
}

impl ExitState {
    const fn word(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Unconfirmed => "unconfirmed",
            Self::Failed => "failed",
            Self::Gone => "gone",
        }
    }
}

/// Identifiers and facts only; never command lines, credentials or screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitRecord {
    pub identity: String,
    pub term: u32,
    pub witness: Option<ExitWitness>,
    pub state: ExitState,
    pub signal_failed: bool,
    pub requested_ms: i64,
    pub observed_ms: i64,
}

/// Cheap handle to the already-opened authority store.
#[derive(Clone)]
pub struct TerminalExitJournal {
    store: WorkflowStore,
}

impl TerminalExitJournal {
    #[must_use]
    pub const fn new(store: WorkflowStore) -> Self {
        Self { store }
    }

    /// Commit the intent before the first signal. Retrying the same live
    /// incarnation reads its original row; a changed identity is refused.
    pub fn begin(
        &self,
        identity: &str,
        term: u32,
        witness: Option<&ExitWitness>,
        now_ms: i64,
    ) -> Result<ExitRecord, WorkflowStoreError> {
        if identity.len() != 64 || !identity.bytes().all(|b| b.is_ascii_hexdigit()) || now_ms < 0 {
            return Err(WorkflowStoreError::InvalidInput {
                field: "terminal_exit_identity",
            });
        }
        if witness.is_some_and(|w| {
            w.group == 0
                || w.started
                    .as_ref()
                    .is_some_and(|s| s.is_empty() || s.len() > 128)
        }) {
            return Err(WorkflowStoreError::InvalidInput {
                field: "terminal_exit_witness",
            });
        }
        let mut connection = self.store.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| WorkflowStoreError::Database)?;
        transaction
            .execute(
                "INSERT INTO terminal_exit_observations
             (identity, term, process_group, process_started, state, requested_ms, observed_ms)
             VALUES (?1, ?2, ?3, ?4, 'requested', ?5, ?5) ON CONFLICT(identity) DO NOTHING",
                params![
                    identity,
                    term,
                    witness.map(|w| w.group),
                    witness.and_then(|w| w.started.as_deref()),
                    now_ms
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        let record = read_one(&transaction, identity)?.ok_or(WorkflowStoreError::Corrupt)?;
        if record.term != term || record.witness.as_ref() != witness {
            return Err(WorkflowStoreError::InvalidInput {
                field: "terminal_exit_incarnation",
            });
        }
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(record)
    }

    /// Gone is a latch: a slower observer cannot undo it. Signal failure is
    /// retained even if a later look observes the process finally leaving.
    pub fn observe(
        &self,
        identity: &str,
        state: ExitState,
        now_ms: i64,
    ) -> Result<(), WorkflowStoreError> {
        if state == ExitState::Requested || now_ms < 0 {
            return Err(WorkflowStoreError::InvalidInput {
                field: "terminal_exit_observation",
            });
        }
        let connection = self.store.connection()?;
        let changed = connection
            .execute(
                "UPDATE terminal_exit_observations SET
                 state = CASE WHEN state = 'gone' THEN state ELSE ?2 END,
                 signal_failed = signal_failed OR ?3,
                 observed_ms = MAX(observed_ms, ?4)
             WHERE identity = ?1",
                params![identity, state.word(), state == ExitState::Failed, now_ms],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        if changed != 1 {
            return Err(WorkflowStoreError::Corrupt);
        }
        Ok(())
    }

    /// Recheck oldest observations first, with caller-bounded memory. A row
    /// without a local witness stays unknown; there is no PID to guess at.
    pub fn pending(
        &self,
        limit: std::num::NonZeroU32,
    ) -> Result<Vec<ExitRecord>, WorkflowStoreError> {
        let connection = self.store.connection()?;
        let mut statement = connection.prepare(
            "SELECT identity, term, process_group, process_started, state, signal_failed, requested_ms, observed_ms
             FROM terminal_exit_observations WHERE state != 'gone' AND process_group IS NOT NULL
             ORDER BY observed_ms, identity LIMIT ?1",
        ).map_err(|_| WorkflowStoreError::Database)?;
        statement
            .query_map([limit.get()], decode)
            .map_err(|_| WorkflowStoreError::Database)?
            .collect::<Result<_, _>>()
            .map_err(|_| WorkflowStoreError::Corrupt)
    }

    pub fn read(&self, identity: &str) -> Result<Option<ExitRecord>, WorkflowStoreError> {
        read_one(&self.store.connection()?, identity)
    }
}

fn read_one(
    connection: &rusqlite::Connection,
    identity: &str,
) -> Result<Option<ExitRecord>, WorkflowStoreError> {
    connection.query_row(
        "SELECT identity, term, process_group, process_started, state, signal_failed, requested_ms, observed_ms
         FROM terminal_exit_observations WHERE identity = ?1", [identity], decode,
    ).optional().map_err(|_| WorkflowStoreError::Database)
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExitRecord> {
    let state = match row.get::<_, String>(4)?.as_str() {
        "requested" => ExitState::Requested,
        "unconfirmed" => ExitState::Unconfirmed,
        "failed" => ExitState::Failed,
        "gone" => ExitState::Gone,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let group: Option<u32> = row.get(2)?;
    let started = row.get(3)?;
    Ok(ExitRecord {
        identity: row.get(0)?,
        term: row.get(1)?,
        witness: group.map(|group| ExitWitness { group, started }),
        state,
        signal_failed: row.get(5)?,
        requested_ms: row.get(6)?,
        observed_ms: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store(root: &std::path::Path) -> TerminalExitJournal {
        let private = root.join("private");
        std::fs::create_dir_all(&private).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        TerminalExitJournal::new(WorkflowStore::open(private.join("authority.sqlite")).unwrap())
    }
    fn witness() -> ExitWitness {
        ExitWitness {
            group: 1234,
            started: Some("test-birth".into()),
        }
    }

    #[test]
    fn interrupted_intent_and_failed_signal_survive_reopening_without_becoming_gone() {
        let root = tempfile::tempdir().unwrap();
        let journal = store(root.path());
        let id = "a".repeat(64);
        journal.begin(&id, 9, Some(&witness()), 10).unwrap();
        drop(journal);
        let reopened = store(root.path());
        assert_eq!(
            reopened.read(&id).unwrap().unwrap().state,
            ExitState::Requested
        );
        reopened.observe(&id, ExitState::Failed, 11).unwrap();
        drop(reopened);
        let reopened = store(root.path());
        assert_eq!(
            reopened.read(&id).unwrap().unwrap().state,
            ExitState::Failed
        );
        reopened.observe(&id, ExitState::Gone, 12).unwrap();
        reopened.observe(&id, ExitState::Unconfirmed, 11).unwrap();
        let row = reopened.read(&id).unwrap().unwrap();
        assert_eq!(row.state, ExitState::Gone);
        assert!(row.signal_failed);
        assert_eq!(row.observed_ms, 12);
    }

    #[test]
    fn duplicate_requests_share_one_intent_but_a_different_incarnation_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let journal = store(root.path());
        let id = "b".repeat(64);
        let first = journal.begin(&id, 9, Some(&witness()), 10).unwrap();
        assert_eq!(journal.begin(&id, 9, Some(&witness()), 20).unwrap(), first);
        assert!(journal.begin(&id, 10, Some(&witness()), 20).is_err());
        let mut other = witness();
        other.started = Some("replacement".into());
        assert!(journal.begin(&id, 9, Some(&other), 20).is_err());
        assert_eq!(journal.pending(std::num::NonZeroU32::MIN).unwrap(), [first]);
    }

    #[test]
    fn observations_rotate_a_bounded_batch_and_do_not_invent_remote_processes() {
        let root = tempfile::tempdir().unwrap();
        let journal = store(root.path());
        for (id, at, process) in [
            ("a", 10, Some(witness())),
            ("b", 11, Some(witness())),
            ("c", 12, None),
        ] {
            journal
                .begin(&id.repeat(64), 9, process.as_ref(), at)
                .unwrap();
        }
        let first = journal.pending(std::num::NonZeroU32::MIN).unwrap();
        assert_eq!(first[0].identity, "a".repeat(64));
        journal
            .observe(&first[0].identity, ExitState::Unconfirmed, 20)
            .unwrap();
        assert_eq!(
            journal.pending(std::num::NonZeroU32::MIN).unwrap()[0].identity,
            "b".repeat(64)
        );
        assert_eq!(
            journal.read(&"c".repeat(64)).unwrap().unwrap().state,
            ExitState::Requested
        );
    }
}
