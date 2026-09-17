//! SQLite-backed workflow authority.
//!
//! Every mutating method takes a `BEGIN IMMEDIATE` transaction, reads the
//! current revision, and updates that exact revision. The immediate lock makes
//! duplicate callers queue before they decide what state exists; the revision
//! predicate keeps that decision explicit in the SQL that changes it.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension as _, Transaction, TransactionBehavior, params,
};

use crate::handoff::{HandoffManifestV1, PublishPolicy};
use crate::publish::PublishPermit;
use crate::workflow::{
    ArchivedGeneration, NewAssignment, PublishExpectation, RefBinding, ReviewDecision,
    ReviewReceipt, TrustedReviewVerifier, WorkflowRecord, WorkflowState, decoded_policy,
    encoded_policy, source_ref, valid_branch_ref, valid_identity, valid_oid,
    valid_publish_target_ref,
};

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// WAL conversion can return BUSY without invoking SQLite's busy handler.
/// A short pause avoids spinning while another first opener finishes it.
const WAL_RETRY_DELAY: Duration = Duration::from_millis(10);
/* Seven is also the FIRST version a production window ever writes: the store
 * ships with the actor cutover, so every store in the wild is either empty or
 * already seven. The versions below seven only ever existed inside this
 * repository's own test fixtures, which is why anything else is refused
 * rather than migrated. */
const STORE_SCHEMA_VERSION: i64 = 7;
/// Every table a fresh store creates, version five through this one.
///
/// A named constant rather than an inline string so the no-comments net
/// below can sweep it: SQLite keeps CREATE text verbatim in `sqlite_schema`,
/// and `normalized_schema_sql` is comment-blind.
const FRESH_TABLES_SQL: &str = "CREATE TABLE IF NOT EXISTS workflow_generations (
                     generation INTEGER PRIMARY KEY AUTOINCREMENT
                 );
                 CREATE TABLE IF NOT EXISTS workflows (
                     workflow_id TEXT PRIMARY KEY NOT NULL,
                     generation INTEGER UNIQUE NOT NULL,
                     parent_generation INTEGER,
                     assignee_id TEXT NOT NULL,
                     repository_id TEXT NOT NULL,
                     worktree_id TEXT NOT NULL,
                     publisher_scope TEXT NOT NULL,
                     base_oid TEXT NOT NULL,
                     target_ref TEXT NOT NULL,
                     target_oid TEXT NOT NULL,
                     state TEXT NOT NULL CHECK (state IN (
                         'assigned', 'submitted', 'approved', 'changes_requested',
                         'publishing', 'publish_unknown', 'published', 'publish_failed'
                     )),
                     manifest_id TEXT,
                     manifest_json BLOB,
                     source_ref TEXT NOT NULL,
                     source_oid TEXT,
                     policy_json BLOB,
                     policy_digest TEXT,
                     reviewer_id TEXT,
                     review_decision TEXT,
                     publish_attempt INTEGER NOT NULL DEFAULT 0,
                     publish_nonce TEXT,
                     published_oid TEXT,
                     revision INTEGER NOT NULL DEFAULT 0,
                     FOREIGN KEY (generation) REFERENCES workflow_generations(generation),
                     FOREIGN KEY (parent_generation) REFERENCES workflow_generations(generation)
                 );
                 CREATE TABLE IF NOT EXISTS workflow_history (
                     workflow_id TEXT NOT NULL,
                     generation INTEGER NOT NULL,
                     parent_generation INTEGER,
                     manifest_id TEXT NOT NULL,
                     manifest_json BLOB NOT NULL,
                     reviewer_id TEXT NOT NULL,
                     review_decision TEXT NOT NULL,
                     PRIMARY KEY (workflow_id, generation),
                     FOREIGN KEY (generation) REFERENCES workflow_generations(generation),
                     FOREIGN KEY (parent_generation) REFERENCES workflow_generations(generation)
                 );
                 CREATE TABLE IF NOT EXISTS trusted_test_evidence (
                     manifest_id TEXT NOT NULL CHECK (length(manifest_id) BETWEEN 1 AND 512),
                     test_name TEXT NOT NULL CHECK (length(test_name) BETWEEN 1 AND 512),
                     command_id TEXT NOT NULL CHECK (length(command_id) BETWEEN 1 AND 512),
                     snapshot_digest TEXT NOT NULL CHECK (length(snapshot_digest) = 64),
                     source_oid TEXT NOT NULL CHECK (length(source_oid) IN (40, 64)),
                     started_at_ms INTEGER NOT NULL,
                     ended_at_ms INTEGER NOT NULL,
                     exit_code INTEGER NOT NULL CHECK (exit_code BETWEEN -2147483648 AND 2147483647),
                     succeeded INTEGER NOT NULL CHECK (succeeded IN (0, 1)),
                     command_digest TEXT NOT NULL CHECK (length(command_digest) = 64),
                     stdout_digest TEXT NOT NULL CHECK (length(stdout_digest) = 64),
                     stderr_digest TEXT NOT NULL CHECK (length(stderr_digest) = 64),
                     evidence_digest TEXT NOT NULL CHECK (length(evidence_digest) = 64),
                     output_artifact_id TEXT NOT NULL CHECK (length(output_artifact_id) = 78),
                     output_truncated INTEGER NOT NULL CHECK (output_truncated IN (0, 1)),
                     PRIMARY KEY (manifest_id, test_name, command_id, snapshot_digest),
                     CHECK (started_at_ms <= ended_at_ms),
                     CHECK (succeeded = (exit_code = 0))
                 );
                 CREATE TABLE IF NOT EXISTS host_effect_operations (
                     ledger_id TEXT NOT NULL,
                     request_slot TEXT NOT NULL CHECK (length(request_slot) = 64),
                     request_fingerprint TEXT NOT NULL
                         CHECK (length(request_fingerprint) = 64),
                     attempt INTEGER NOT NULL CHECK (attempt > 0),
                     operation_id TEXT NOT NULL UNIQUE CHECK (length(operation_id) = 64),
                     effect_kind TEXT NOT NULL CHECK (effect_kind IN (
                         'split', 'release', 'close', 'respawn'
                     )),
                     effect_state TEXT NOT NULL CHECK (effect_state IN (
                         'prepared', 'applied', 'not_started', 'unknown'
                     )),
                     binding_digest TEXT NOT NULL CHECK (length(binding_digest) = 64),
                     host_epoch TEXT NOT NULL CHECK (length(host_epoch) = 64),
                     operation_authority_epoch INTEGER NOT NULL DEFAULT 0
                         CHECK (operation_authority_epoch >= 0),
                     prepared_revision INTEGER NOT NULL CHECK (prepared_revision > 0),
                     result_digest TEXT CHECK (
                         result_digest IS NULL OR length(result_digest) = 64
                     ),
                     failure_kind TEXT CHECK (
                         failure_kind IS NULL OR failure_kind IN (
                             'refused', 'timeout', 'disconnected', 'unavailable', 'panicked'
                         )
                     ),
                     tombstone_digest TEXT CHECK (
                         tombstone_digest IS NULL OR length(tombstone_digest) = 64
                     ),
                     retryable INTEGER NOT NULL DEFAULT 0 CHECK (retryable IN (0, 1)),
                     retry_not_before_ms INTEGER CHECK (
                         retry_not_before_ms IS NULL OR retry_not_before_ms >= 0
                     ),
                     row_revision INTEGER NOT NULL DEFAULT 0 CHECK (row_revision >= 0),
                     prepared_at_ms INTEGER NOT NULL CHECK (prepared_at_ms >= 0),
                     updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= prepared_at_ms),
                     settled_at_ms INTEGER CHECK (
                         settled_at_ms IS NULL OR settled_at_ms >= prepared_at_ms
                     ),
                     PRIMARY KEY (ledger_id, request_slot, attempt),
                     FOREIGN KEY (ledger_id)
                         REFERENCES orchestration_ledger_heads(ledger_id)
                         ON DELETE CASCADE,
                     CHECK (
                         (effect_state = 'prepared'
                             AND result_digest IS NULL
                             AND failure_kind IS NULL
                             AND tombstone_digest IS NULL
                             AND retryable = 0
                             AND retry_not_before_ms IS NULL
                             AND settled_at_ms IS NULL)
                         OR (effect_state = 'unknown'
                             AND result_digest IS NULL
                             AND failure_kind IS NOT NULL
                             AND tombstone_digest IS NULL
                             AND retryable = 0
                             AND retry_not_before_ms IS NULL
                             AND settled_at_ms IS NULL)
                         OR (effect_state = 'applied'
                             AND result_digest IS NOT NULL
                             AND failure_kind IS NULL
                             AND tombstone_digest IS NULL
                             AND retryable = 0
                             AND retry_not_before_ms IS NULL
                             AND settled_at_ms IS NOT NULL)
                         OR (effect_state = 'not_started'
                             AND result_digest IS NULL
                             AND failure_kind IS NOT NULL
                             AND tombstone_digest IS NOT NULL
                             AND settled_at_ms IS NOT NULL
                             AND (
                                 (retryable = 0 AND retry_not_before_ms IS NULL)
                                 OR (retryable = 1
                                     AND retry_not_before_ms IS NOT NULL
                                     AND retry_not_before_ms > settled_at_ms)
                             ))
                     )
                 );
                 CREATE INDEX IF NOT EXISTS host_effect_request_attempts
                     ON host_effect_operations (ledger_id, request_slot, attempt DESC);
                 CREATE INDEX IF NOT EXISTS host_effect_recovery
                     ON host_effect_operations (ledger_id, effect_state, prepared_at_ms);
                 CREATE TABLE IF NOT EXISTS host_effect_steps (
                     operation_id TEXT NOT NULL
                         REFERENCES host_effect_operations(operation_id)
                             ON DELETE CASCADE,
                     ordinal INTEGER NOT NULL CHECK (ordinal >= 0 AND ordinal < 16),
                     step TEXT NOT NULL CHECK (length(step) BETWEEN 1 AND 64),
                     at_ms INTEGER NOT NULL CHECK (at_ms >= 0),
                     PRIMARY KEY (operation_id, ordinal)
                 );";

const SINGLE_IN_FLIGHT_INDEX_SQL: &str =
    "CREATE UNIQUE INDEX IF NOT EXISTS host_effect_single_in_flight
         ON host_effect_operations (ledger_id)
      WHERE effect_state IN ('prepared', 'unknown');";

const AUTHORITY_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS orchestration_authorities (
    ledger_id TEXT PRIMARY KEY NOT NULL CHECK (length(ledger_id) BETWEEN 1 AND 512),
    authority_epoch INTEGER NOT NULL CHECK (authority_epoch > 0),
    owner_state TEXT NOT NULL CHECK (owner_state IN ('claimed', 'released')),
    token_digest TEXT CHECK (token_digest IS NULL OR length(token_digest) = 64),
    owner_instance_digest TEXT CHECK (
        owner_instance_digest IS NULL OR length(owner_instance_digest) = 64
    ),
    owner_revision INTEGER NOT NULL DEFAULT 0 CHECK (owner_revision >= 0),
    claimed_at_ms INTEGER NOT NULL CHECK (claimed_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= claimed_at_ms),
    released_at_ms INTEGER CHECK (
        released_at_ms IS NULL OR released_at_ms >= claimed_at_ms
    ),
    CHECK (
        (owner_state = 'claimed' AND token_digest IS NOT NULL
            AND owner_instance_digest IS NOT NULL AND released_at_ms IS NULL)
        OR (owner_state = 'released' AND token_digest IS NULL
            AND owner_instance_digest IS NULL AND released_at_ms IS NOT NULL)
    )
);";
const AUTHORITY_RECOVERY_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS
    host_effect_authority_recovery ON host_effect_operations (
        ledger_id, operation_authority_epoch, effect_state
    );";
pub const MAX_WORKFLOW_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_WORKFLOW_POLICY_BYTES: usize = 512 * 1024;
pub const MAX_WORKFLOW_REQUIRED_TESTS: usize = 256;

/// A durable store whose local file name never enters Debug or serialization.
#[derive(Clone)]
pub struct WorkflowStore {
    path: PathBuf,
}

impl std::fmt::Debug for WorkflowStore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkflowStore")
            .field("path", &"<private-workflow-store>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorkflowStoreError {
    #[error("workflow storage is unavailable")]
    Database,
    #[error("workflow storage contains an invalid row")]
    Corrupt,
    #[error("workflow storage schema {found} is unsupported (expected {expected})", expected = STORE_SCHEMA_VERSION)]
    UnsupportedSchema { found: i64 },
    #[error("workflow storage path is not a private regular file")]
    UnsafeStorePath,
    #[error("{field} is not a valid workflow value")]
    InvalidInput { field: &'static str },
    #[error("workflow `{workflow_id}` does not exist")]
    NotFound { workflow_id: String },
    #[error("workflow `{workflow_id}` generation is {actual}, not {expected}")]
    GenerationMismatch {
        workflow_id: String,
        expected: u64,
        actual: u64,
    },
    #[error("workflow `{workflow_id}` already has a different immutable assignment")]
    ImmutableAssignment { workflow_id: String },
    #[error("workflow `{workflow_id}` already has a different submission binding")]
    ImmutableSubmission { workflow_id: String },
    #[error("workflow `{workflow_id}` submission does not match {field}")]
    SubmissionBinding {
        workflow_id: String,
        field: &'static str,
    },
    #[error("the assignee cannot review workflow `{workflow_id}`")]
    SelfReview { workflow_id: String },
    #[error("the review attestation is not trusted")]
    UntrustedReview,
    #[error("workflow `{workflow_id}` is {actual:?}, expected {expected:?}")]
    InvalidState {
        workflow_id: String,
        expected: WorkflowState,
        actual: WorkflowState,
    },
    #[error("workflow `{workflow_id}` changed while its transition was being committed")]
    ConcurrentTransition { workflow_id: String },
}

pub(crate) struct PublishMaterial {
    pub(crate) record: WorkflowRecord,
    pub(crate) manifest: HandoffManifestV1,
    pub(crate) policy: PublishPolicy,
    pub(crate) revision: u64,
    pub(crate) attempt: u64,
    pub(crate) publish_nonce: Option<String>,
}

impl PublishMaterial {
    pub(crate) fn expectation(&self) -> Result<PublishExpectation, WorkflowStoreError> {
        Ok(PublishExpectation {
            workflow_id: self.record.workflow_id.clone(),
            generation: self.record.generation,
            manifest_id: self
                .record
                .manifest_id
                .clone()
                .ok_or(WorkflowStoreError::Corrupt)?,
            repository_id: self.record.repository_id.clone(),
            worktree_id: self.record.worktree_id.clone(),
            publisher_scope: self.record.publisher_scope.clone(),
            source: self
                .record
                .source
                .clone()
                .ok_or(WorkflowStoreError::Corrupt)?,
            target: self.record.target.clone(),
            policy_digest: self
                .record
                .policy_digest
                .clone()
                .ok_or(WorkflowStoreError::Corrupt)?,
        })
    }
}

struct DatabaseRow {
    workflow_id: String,
    generation: i64,
    parent_generation: Option<i64>,
    assignee_id: String,
    repository_id: String,
    worktree_id: String,
    publisher_scope: String,
    base_oid: String,
    target_ref: String,
    target_oid: String,
    state: String,
    manifest_id: Option<String>,
    manifest_json: Option<Vec<u8>>,
    source_ref: String,
    source_oid: Option<String>,
    policy_json: Option<Vec<u8>>,
    policy_digest: Option<String>,
    reviewer_id: Option<String>,
    review_decision: Option<String>,
    publish_attempt: i64,
    publish_nonce: Option<String>,
    published_oid: Option<String>,
    revision: i64,
}

impl DatabaseRow {
    fn generation(&self) -> Result<u64, WorkflowStoreError> {
        u64::try_from(self.generation).map_err(|_| WorkflowStoreError::Corrupt)
    }

    fn attempt(&self) -> Result<u64, WorkflowStoreError> {
        u64::try_from(self.publish_attempt).map_err(|_| WorkflowStoreError::Corrupt)
    }

    fn revision(&self) -> Result<u64, WorkflowStoreError> {
        u64::try_from(self.revision).map_err(|_| WorkflowStoreError::Corrupt)
    }

    fn state(&self) -> Result<WorkflowState, WorkflowStoreError> {
        WorkflowState::from_db(&self.state).ok_or(WorkflowStoreError::Corrupt)
    }

    fn record(&self) -> Result<WorkflowRecord, WorkflowStoreError> {
        Ok(WorkflowRecord {
            workflow_id: self.workflow_id.clone(),
            generation: self.generation()?,
            parent_generation: self
                .parent_generation
                .map(u64::try_from)
                .transpose()
                .map_err(|_| WorkflowStoreError::Corrupt)?,
            assignee_id: self.assignee_id.clone(),
            repository_id: self.repository_id.clone(),
            worktree_id: self.worktree_id.clone(),
            publisher_scope: self.publisher_scope.clone(),
            base_oid: self.base_oid.clone(),
            source: self.source_oid.as_ref().map(|oid| RefBinding {
                name: self.source_ref.clone(),
                oid: oid.clone(),
            }),
            target: RefBinding {
                name: self.target_ref.clone(),
                oid: self.target_oid.clone(),
            },
            manifest_id: self.manifest_id.clone(),
            policy_digest: self.policy_digest.clone(),
            reviewer_id: self.reviewer_id.clone(),
            state: self.state()?,
            published_oid: self.published_oid.clone(),
        })
    }

    fn material(self) -> Result<PublishMaterial, WorkflowStoreError> {
        let manifest = serde_json::from_slice::<HandoffManifestV1>(
            self.manifest_json
                .as_deref()
                .ok_or(WorkflowStoreError::Corrupt)?,
        )
        .map_err(|_| WorkflowStoreError::Corrupt)?;
        let policy = decoded_policy(
            self.policy_json
                .as_deref()
                .ok_or(WorkflowStoreError::Corrupt)?,
        )
        .map_err(|_| WorkflowStoreError::Corrupt)?;
        let revision = self.revision()?;
        let attempt = self.attempt()?;
        let record = self.record()?;
        Ok(PublishMaterial {
            record,
            manifest,
            policy,
            revision,
            attempt,
            publish_nonce: self.publish_nonce,
        })
    }
}

impl WorkflowStore {
    pub(crate) fn private_path(&self) -> &Path {
        &self.path
    }

    /// Open or create one workflow database.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, WorkflowStoreError> {
        prepare_store_path(path.as_ref())?;
        let path =
            fs::canonicalize(path.as_ref()).map_err(|_| WorkflowStoreError::UnsafeStorePath)?;
        let store = Self { path };
        let mut connection = store.connection()?;
        /* Read the version BEFORE anything can write, because a file this
         * window will not touch has to leave here untouched.
         *
         * Setting `journal_mode` rewrites the header of a database that was
         * not already in WAL, so asking that first would edit the very file we
         * are about to refuse. This read takes no lock and changes nothing;
         * the authoritative read still happens under the migration lock below,
         * where a second opener cannot slip between the question and the
         * answer.
         */
        let found: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| WorkflowStoreError::Database)?;
        if found != 0 && found != STORE_SCHEMA_VERSION {
            return Err(WorkflowStoreError::UnsupportedSchema { found });
        }
        // WAL mode is a connection-level choice and SQLite refuses to change it
        // from inside a transaction. Make that choice first, then take the
        // migration lock BEFORE reading `user_version`: a second opener may have
        // advanced it while this one was waiting, and branching on a version
        // read before the lock can migrate stale state or move the number back.
        // Tables and the version that names them then land in one transaction.
        configure_wal(&connection).map_err(|_| WorkflowStoreError::Database)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| WorkflowStoreError::Database)?;
        let schema: i64 = transaction
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| WorkflowStoreError::Database)?;
        /* Asked again, under the lock this time.
         *
         * The read above happens without a lock, because refusing a file this
         * window will not touch has to happen before anything can write to it.
         * This one is the same question asked where a second opener cannot
         * slip between the question and the answer. No mutation test tells the
         * two apart today — nothing left in the world writes a version below
         * five — so this is a belt kept for the race, not for a case anybody
         * has seen.
         *
         * Both directions, and for the same reason.
         *
         * A HIGHER number is a file a later window wrote, and a window must
         * never rewrite a file it cannot read. A LOWER one is a store from
         * before the ledger became tables: schema four kept a whole ledger as
         * one blob, and the ruling on it is that there is nothing to carry
         * across — that shape was never released, so no person's work is
         * inside it, and an importer for it would be dead weight plus one more
         * parser for input nobody trusts. The user's real ledgers are JSON
         * files, and their road in is `import_legacy` at cutover, not here.
         */
        if schema != 0 && schema != STORE_SCHEMA_VERSION {
            return Err(WorkflowStoreError::UnsupportedSchema { found: schema });
        }
        if schema < STORE_SCHEMA_VERSION {
            /* Only a file with nothing in it reaches here now.
             *
             * The guards that used to stand at the top of this branch asked
             * whether a schema-two or schema-three store had lost tables
             * before an idempotent `CREATE` could hide the damage. With the
             * older schemas refused above, the only version left below five is
             * zero — a database with no version and no tables — so those
             * questions have nothing to ask about, and a check that cannot
             * fail is a check that is only read as reassurance.
             *
             * Two notes the DDL itself may not carry — no comments inside the
             * SQL string, because SQLite keeps the CREATE text verbatim in
             * sqlite_schema and `normalized_schema_sql` is comment-blind: one
             * apostrophe in a SQL comment flips its quote tracking and every
             * fresh open reads its own schema as Corrupt.
             *
             * One: the effect table's parent is the head row — the ledger IS
             * the typed rows, and the blob its FK used to point at is gone.
             * The move waited for exactly that: done earlier, it would have
             * made every effect against a blob-only ledger unwritable.
             *
             * Two: the `ordinal < 16` in `host_effect_steps` is
             * `effect_journal::MAX_STRIDES` — the journal door refuses the
             * same number first, and the schema is the net behind it.
             */
            transaction
                .execute_batch(FRESH_TABLES_SQL)
                .map_err(|_| WorkflowStoreError::Database)?;
            transaction
                .execute_batch(AUTHORITY_TABLE_SQL)
                .map_err(|_| WorkflowStoreError::Database)?;
            /* The ledger's own tables, in the same transaction as the version
             * that names them. A store stamped five without them would be a
             * file whose number promises something it does not hold. */
            transaction
                .execute_batch(crate::ledger_store::LEDGER_TABLES_SQL)
                .map_err(|_| WorkflowStoreError::Database)?;
            if !column_exists(
                &transaction,
                "host_effect_operations",
                "operation_authority_epoch",
            )? {
                transaction
                    .execute_batch(
                        "ALTER TABLE host_effect_operations
                             ADD COLUMN operation_authority_epoch INTEGER NOT NULL DEFAULT 0
                                 CHECK (operation_authority_epoch >= 0);",
                    )
                    .map_err(|_| WorkflowStoreError::Database)?;
            }
            transaction
                .execute_batch(AUTHORITY_RECOVERY_INDEX_SQL)
                .map_err(|_| WorkflowStoreError::Database)?;
            require_single_in_flight_effect(&transaction)?;
            transaction
                .execute_batch(SINGLE_IN_FLIGHT_INDEX_SQL)
                .map_err(|_| WorkflowStoreError::Database)?;
            require_effect_schema_indexes(&transaction)?;
            require_authority_schema(&transaction)?;
            transaction
                .pragma_update(None, "user_version", STORE_SCHEMA_VERSION)
                .map_err(|_| WorkflowStoreError::Database)?;
        } else {
            /* A store already at the current version may still predate a
             * table the ledger DDL has since grown. The version number names
             * the shape of the workflow tables; the ledger's own tables grow
             * additively, the way the projection grows serde fields — and an
             * upgraded window walks in through THIS branch, so this is where
             * an older file has to be handed the tables the new build will
             * unconditionally address. Every ledger statement is `CREATE …
             * IF NOT EXISTS`, so the replay is a no-op on a current file.
             *
             * A replayed table is then read through the ledger's stored table
             * digests and independent head root below. A table genuinely new
             * to this digest format has no witness yet; a populated table that
             * disappeared does, even if its per-table metadata disappeared
             * with it. */
            transaction
                .execute_batch(crate::ledger_store::LEDGER_TABLES_SQL)
                .map_err(|_| WorkflowStoreError::Database)?;
            crate::ledger_store::ensure_ledger_columns(&transaction)
                .map_err(|_| WorkflowStoreError::Database)?;
            crate::ledger_store::require_current_table_integrity(
                &transaction,
                zerocode_core::orchestration::PROJECTION_SCHEMA,
            )
            .map_err(|error| match error {
                crate::effect_journal::EffectJournalError::Corrupt => WorkflowStoreError::Corrupt,
                _ => WorkflowStoreError::Database,
            })?;
            require_schema_tables(&transaction)?;
            require_effect_schema_indexes(&transaction)?;
            require_authority_schema(&transaction)?;
        }
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        tighten_store_files(&store.path)?;
        Ok(store)
    }

    /// Assign exactly once. A byte-for-byte equivalent retry returns the
    /// original store-minted generation; a changed retry is rejected.
    pub fn assign(&self, assignment: &NewAssignment) -> Result<WorkflowRecord, WorkflowStoreError> {
        validate_assignment(assignment)?;
        let (policy_json, policy_digest) =
            encoded_policy(&assignment.policy).map_err(|_| WorkflowStoreError::Corrupt)?;
        if policy_json.len() > MAX_WORKFLOW_POLICY_BYTES {
            return Err(WorkflowStoreError::InvalidInput {
                field: "publish_policy",
            });
        }
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        if let Some(existing) = select_row(&transaction, &assignment.workflow_id)? {
            let same = existing.assignee_id == assignment.assignee_id
                && existing.repository_id == assignment.repository_id
                && existing.worktree_id == assignment.worktree_id
                && existing.publisher_scope == assignment.publisher_scope
                && existing.base_oid == assignment.base_oid
                && existing.source_ref == assignment.source_ref
                && existing.target_ref == assignment.target_ref
                && existing.target_oid == assignment.base_oid
                && existing.policy_json.as_deref() == Some(policy_json.as_slice())
                && existing.policy_digest.as_deref() == Some(policy_digest.as_str());
            if same {
                let record = existing.record()?;
                transaction
                    .commit()
                    .map_err(|_| WorkflowStoreError::Database)?;
                return Ok(record);
            }
            return Err(WorkflowStoreError::ImmutableAssignment {
                workflow_id: assignment.workflow_id.clone(),
            });
        }
        transaction
            .execute("INSERT INTO workflow_generations DEFAULT VALUES", [])
            .map_err(|_| WorkflowStoreError::Database)?;
        let generation = transaction.last_insert_rowid();
        transaction
            .execute(
                "INSERT INTO workflows (
                    workflow_id, generation, assignee_id, repository_id, worktree_id,
                    publisher_scope, base_oid, source_ref, target_ref, target_oid, state,
                    policy_json, policy_digest
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?7, 'assigned', ?10, ?11)",
                params![
                    assignment.workflow_id,
                    generation,
                    assignment.assignee_id,
                    assignment.repository_id,
                    assignment.worktree_id,
                    assignment.publisher_scope,
                    assignment.base_oid,
                    assignment.source_ref,
                    assignment.target_ref,
                    policy_json,
                    policy_digest,
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        let record = select_required(&transaction, &assignment.workflow_id)?.record()?;
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(record)
    }

    /// Bind a handoff and policy to an assignment. Neither can be replaced by
    /// a later retry, even before review.
    pub fn submit(
        &self,
        workflow_id: &str,
        generation: u64,
        manifest: &HandoffManifestV1,
        policy: &PublishPolicy,
    ) -> Result<WorkflowRecord, WorkflowStoreError> {
        let manifest_json =
            serde_json::to_vec(manifest).map_err(|_| WorkflowStoreError::Corrupt)?;
        let (policy_json, policy_digest) =
            encoded_policy(policy).map_err(|_| WorkflowStoreError::Corrupt)?;
        if manifest_json.len() > MAX_WORKFLOW_MANIFEST_BYTES {
            return Err(WorkflowStoreError::InvalidInput {
                field: "handoff_manifest",
            });
        }
        if policy_json.len() > MAX_WORKFLOW_POLICY_BYTES
            || policy.required_tests.len() > MAX_WORKFLOW_REQUIRED_TESTS
        {
            return Err(WorkflowStoreError::InvalidInput {
                field: "publish_policy",
            });
        }
        let source_oid = manifest.worktree.head_oid.clone();
        let source_ref = manifest
            .worktree
            .branch
            .as_deref()
            .and_then(source_ref)
            .ok_or_else(|| WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "source branch",
            })?;
        if !valid_oid(&source_oid) {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "source oid",
            });
        }

        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let existing = select_required(&transaction, workflow_id)?;
        require_generation(&existing, workflow_id, generation)?;
        if manifest.worktree.repository_id != existing.repository_id {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "repository identity",
            });
        }
        if manifest.worktree.worktree_id != existing.worktree_id {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "worktree identity",
            });
        }
        if source_ref != existing.source_ref {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "source branch",
            });
        }
        if existing.policy_json.as_deref() != Some(policy_json.as_slice())
            || existing.policy_digest.as_deref() != Some(policy_digest.as_str())
        {
            return Err(WorkflowStoreError::ImmutableSubmission {
                workflow_id: workflow_id.to_string(),
            });
        }
        if manifest.generation != generation {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "store generation",
            });
        }
        let expected_parent = expected_parent_manifest(&transaction, &existing)?;
        if manifest.lineage.parent_manifest_id != expected_parent {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "parent manifest",
            });
        }
        if manifest.lineage.task_id != workflow_id {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "task identity",
            });
        }
        if manifest.lineage.worker_id != existing.assignee_id {
            return Err(WorkflowStoreError::SubmissionBinding {
                workflow_id: workflow_id.to_string(),
                field: "assignee identity",
            });
        }

        if existing.state()? != WorkflowState::Assigned {
            let same = existing.manifest_id.as_deref() == Some(manifest.manifest_id.as_str())
                && existing.manifest_json.as_deref() == Some(manifest_json.as_slice())
                && existing.source_ref == source_ref
                && existing.source_oid.as_deref() == Some(source_oid.as_str())
                && existing.policy_json.as_deref() == Some(policy_json.as_slice())
                && existing.policy_digest.as_deref() == Some(policy_digest.as_str());
            if same {
                let record = existing.record()?;
                transaction
                    .commit()
                    .map_err(|_| WorkflowStoreError::Database)?;
                return Ok(record);
            }
            return Err(WorkflowStoreError::ImmutableSubmission {
                workflow_id: workflow_id.to_string(),
            });
        }

        let changed = transaction
            .execute(
                "UPDATE workflows SET
                    state = 'submitted', manifest_id = ?1, manifest_json = ?2,
                    source_oid = ?3, revision = revision + 1
                 WHERE workflow_id = ?4 AND generation = ?5 AND state = 'assigned'
                       AND revision = ?6 AND repository_id = ?7 AND worktree_id = ?8
                       AND source_ref = ?9 AND policy_json = ?10 AND policy_digest = ?11",
                params![
                    manifest.manifest_id,
                    manifest_json,
                    source_oid,
                    workflow_id,
                    to_sql_u64(generation)?,
                    existing.revision,
                    manifest.worktree.repository_id,
                    manifest.worktree.worktree_id,
                    source_ref,
                    policy_json,
                    policy_digest,
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        require_one(changed, workflow_id)?;
        let record = select_required(&transaction, workflow_id)?.record()?;
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(record)
    }

    /// Record a host-verified independent review. Equivalent repeats are
    /// idempotent; an unverified caller spelling never reaches the raw CAS.
    pub fn review<V: TrustedReviewVerifier>(
        &self,
        receipt: &ReviewReceipt,
        verifier: &V,
    ) -> Result<WorkflowRecord, WorkflowStoreError> {
        if !valid_identity(&receipt.attestation_id) || !valid_identity(&receipt.reviewer_id) {
            return Err(WorkflowStoreError::InvalidInput {
                field: "review_receipt",
            });
        }
        if !verifier.verifies(receipt) {
            return Err(WorkflowStoreError::UntrustedReview);
        }
        self.record_review(receipt)
    }

    fn record_review(&self, receipt: &ReviewReceipt) -> Result<WorkflowRecord, WorkflowStoreError> {
        let workflow_id = &receipt.binding.workflow_id;
        let generation = receipt.binding.generation;
        let reviewer_id = &receipt.reviewer_id;
        let decision = receipt.decision;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let existing = select_required(&transaction, workflow_id)?;
        require_generation(&existing, workflow_id, generation)?;
        require_expectation(&existing, &receipt.binding)?;
        if reviewer_id.as_str() == existing.assignee_id {
            return Err(WorkflowStoreError::SelfReview {
                workflow_id: workflow_id.clone(),
            });
        }
        let next = match decision {
            ReviewDecision::Approve => WorkflowState::Approved,
            ReviewDecision::RequestChanges => WorkflowState::ChangesRequested,
        };
        let current = existing.state()?;
        if current != WorkflowState::Submitted {
            let progressed_approval = decision == ReviewDecision::Approve
                && matches!(
                    current,
                    WorkflowState::Approved
                        | WorkflowState::Publishing
                        | WorkflowState::PublishUnknown
                        | WorkflowState::Published
                        | WorkflowState::PublishFailed
                );
            let same_review = (current == next || progressed_approval)
                && existing.reviewer_id.as_deref() == Some(reviewer_id.as_str())
                && existing
                    .review_decision
                    .as_deref()
                    .and_then(ReviewDecision::from_db)
                    == Some(decision);
            if same_review {
                let record = existing.record()?;
                transaction
                    .commit()
                    .map_err(|_| WorkflowStoreError::Database)?;
                return Ok(record);
            }
            return Err(WorkflowStoreError::InvalidState {
                workflow_id: workflow_id.clone(),
                expected: WorkflowState::Submitted,
                actual: current,
            });
        }
        let changed = transaction
            .execute(
                "UPDATE workflows SET state = ?1, reviewer_id = ?2, review_decision = ?3,
                                      revision = revision + 1
                 WHERE workflow_id = ?4 AND generation = ?5 AND state = 'submitted'
                       AND revision = ?6",
                params![
                    next.as_db(),
                    reviewer_id,
                    decision.as_db(),
                    workflow_id,
                    to_sql_u64(generation)?,
                    existing.revision,
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        require_one(changed, workflow_id)?;
        let record = select_required(&transaction, workflow_id)?.record()?;
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(record)
    }

    /// Start the next correction generation after an authenticated reviewer
    /// requested changes. Assignment, repository, refs, and policy stay fixed.
    pub fn reopen(
        &self,
        workflow_id: &str,
        generation: u64,
    ) -> Result<WorkflowRecord, WorkflowStoreError> {
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let existing = select_required(&transaction, workflow_id)?;
        require_generation(&existing, workflow_id, generation)?;
        let current = existing.state()?;
        if current != WorkflowState::ChangesRequested {
            return Err(WorkflowStoreError::InvalidState {
                workflow_id: workflow_id.to_string(),
                expected: WorkflowState::ChangesRequested,
                actual: current,
            });
        }
        transaction
            .execute("INSERT INTO workflow_generations DEFAULT VALUES", [])
            .map_err(|_| WorkflowStoreError::Database)?;
        let next_generation = transaction.last_insert_rowid();
        let archived = transaction
            .execute(
                "INSERT INTO workflow_history (
                    workflow_id, generation, parent_generation, manifest_id, manifest_json,
                    reviewer_id, review_decision
                 ) SELECT workflow_id, generation, parent_generation, manifest_id, manifest_json,
                          reviewer_id, review_decision
                   FROM workflows
                  WHERE workflow_id = ?1 AND generation = ?2 AND state = 'changes_requested'
                        AND revision = ?3 AND manifest_id IS NOT NULL AND manifest_json IS NOT NULL
                        AND reviewer_id IS NOT NULL AND review_decision = 'request_changes'",
                params![workflow_id, to_sql_u64(generation)?, existing.revision],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        require_one(archived, workflow_id)?;
        let changed = transaction
            .execute(
                "UPDATE workflows SET generation = ?1, parent_generation = ?2,
                    state = 'assigned', manifest_id = NULL, manifest_json = NULL,
                    source_oid = NULL, reviewer_id = NULL, review_decision = NULL,
                    publish_attempt = 0, publish_nonce = NULL, published_oid = NULL,
                    revision = revision + 1
                 WHERE workflow_id = ?3 AND generation = ?2 AND state = 'changes_requested'
                       AND revision = ?4",
                params![
                    next_generation,
                    to_sql_u64(generation)?,
                    workflow_id,
                    existing.revision,
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        require_one(changed, workflow_id)?;
        let record = select_required(&transaction, workflow_id)?.record()?;
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(record)
    }

    #[must_use = "store reads can fail"]
    pub fn get(&self, workflow_id: &str) -> Result<WorkflowRecord, WorkflowStoreError> {
        let connection = self.connection()?;
        select_required(&connection, workflow_id)?.record()
    }

    /// Read immutable evidence from a superseded generation.
    pub fn archived_generation(
        &self,
        workflow_id: &str,
        generation: u64,
    ) -> Result<Option<ArchivedGeneration>, WorkflowStoreError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT parent_generation, manifest_json, reviewer_id, review_decision
                   FROM workflow_history WHERE workflow_id = ?1 AND generation = ?2",
                params![workflow_id, to_sql_u64(generation)?],
                |row| {
                    let parent: Option<i64> = row.get(0)?;
                    let manifest: Vec<u8> = row.get(1)?;
                    let reviewer_id: String = row.get(2)?;
                    let decision: String = row.get(3)?;
                    Ok((parent, manifest, reviewer_id, decision))
                },
            )
            .optional()
            .map_err(|_| WorkflowStoreError::Database)?
            .map(|(parent, manifest, reviewer_id, decision)| {
                Ok(ArchivedGeneration {
                    workflow_id: workflow_id.to_string(),
                    generation,
                    parent_generation: parent
                        .map(u64::try_from)
                        .transpose()
                        .map_err(|_| WorkflowStoreError::Corrupt)?,
                    manifest: serde_json::from_slice(&manifest)
                        .map_err(|_| WorkflowStoreError::Corrupt)?,
                    reviewer_id,
                    decision: ReviewDecision::from_db(&decision)
                        .ok_or(WorkflowStoreError::Corrupt)?,
                })
            })
            .transpose()
    }

    pub(crate) fn publish_material(
        &self,
        workflow_id: &str,
        generation: u64,
    ) -> Result<PublishMaterial, WorkflowStoreError> {
        let connection = self.connection()?;
        let row = select_required(&connection, workflow_id)?;
        require_generation(&row, workflow_id, generation)?;
        row.material()
    }

    pub(crate) fn claim_publish(
        &self,
        expected: &PublishExpectation,
    ) -> Result<PublishPermit, WorkflowStoreError> {
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let row = select_required(&transaction, &expected.workflow_id)?;
        require_generation(&row, &expected.workflow_id, expected.generation)?;
        require_expectation(&row, expected)?;
        let state = row.state()?;
        if state != WorkflowState::Approved {
            return Err(WorkflowStoreError::InvalidState {
                workflow_id: expected.workflow_id.clone(),
                expected: WorkflowState::Approved,
                actual: state,
            });
        }
        let next_attempt = row
            .attempt()?
            .checked_add(1)
            .ok_or(WorkflowStoreError::Corrupt)?;
        let next_revision = row
            .revision()?
            .checked_add(1)
            .ok_or(WorkflowStoreError::Corrupt)?;
        let publish_nonce: String = transaction
            .query_row("SELECT lower(hex(randomblob(32)))", [], |row| row.get(0))
            .map_err(|_| WorkflowStoreError::Database)?;
        let changed = transaction
            .execute(
                "UPDATE workflows SET state = 'publishing', publish_attempt = ?1,
                                      revision = ?2, publish_nonce = ?3
                 WHERE workflow_id = ?4 AND generation = ?5 AND state = 'approved'
                       AND revision = ?6 AND manifest_id = ?7 AND repository_id = ?8
                       AND worktree_id = ?9 AND publisher_scope = ?10 AND source_ref = ?11
                       AND source_oid = ?12 AND target_ref = ?13 AND target_oid = ?14
                       AND policy_digest = ?15",
                params![
                    to_sql_u64(next_attempt)?,
                    to_sql_u64(next_revision)?,
                    publish_nonce,
                    expected.workflow_id,
                    to_sql_u64(expected.generation)?,
                    row.revision,
                    expected.manifest_id,
                    expected.repository_id,
                    expected.worktree_id,
                    expected.publisher_scope,
                    expected.source.name,
                    expected.source.oid,
                    expected.target.name,
                    expected.target.oid,
                    expected.policy_digest,
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        require_one(changed, &expected.workflow_id)?;
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(PublishPermit::new(
            expected.clone(),
            next_attempt,
            next_revision,
            publish_nonce,
        ))
    }

    pub(crate) fn settle_permit(
        &self,
        permit: &PublishPermit,
        state: WorkflowState,
    ) -> Result<WorkflowRecord, WorkflowStoreError> {
        debug_assert!(matches!(
            state,
            WorkflowState::PublishUnknown | WorkflowState::Published | WorkflowState::PublishFailed
        ));
        let expected = permit.expectation();
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let published_oid = (state == WorkflowState::Published).then_some(&expected.source.oid);
        let changed = transaction
            .execute(
                "UPDATE workflows SET state = ?1, published_oid = ?2, revision = revision + 1
                 WHERE workflow_id = ?3 AND generation = ?4 AND state = 'publishing'
                       AND publish_attempt = ?5 AND revision = ?6 AND manifest_id = ?7
                       AND source_ref = ?8 AND source_oid = ?9 AND target_ref = ?10
                       AND target_oid = ?11 AND policy_digest = ?12 AND publish_nonce = ?13
                       AND repository_id = ?14 AND worktree_id = ?15 AND publisher_scope = ?16",
                params![
                    state.as_db(),
                    published_oid,
                    expected.workflow_id,
                    to_sql_u64(expected.generation)?,
                    to_sql_u64(permit.attempt())?,
                    to_sql_u64(permit.revision())?,
                    expected.manifest_id,
                    expected.source.name,
                    expected.source.oid,
                    expected.target.name,
                    expected.target.oid,
                    expected.policy_digest,
                    permit.operation_id(),
                    expected.repository_id,
                    expected.worktree_id,
                    expected.publisher_scope,
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        require_one(changed, &expected.workflow_id)?;
        let record = select_required(&transaction, &expected.workflow_id)?.record()?;
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(record)
    }

    pub(crate) fn settle_recovery(
        &self,
        material: &PublishMaterial,
        state: WorkflowState,
    ) -> Result<WorkflowRecord, WorkflowStoreError> {
        debug_assert!(matches!(
            state,
            WorkflowState::Approved | WorkflowState::Published
        ));
        let expected = material.expectation()?;
        let mut connection = self.connection()?;
        let transaction = immediate(&mut connection)?;
        let row = select_required(&transaction, &expected.workflow_id)?;
        require_generation(&row, &expected.workflow_id, expected.generation)?;
        require_expectation(&row, &expected)?;
        let current = row.state()?;
        if !matches!(
            current,
            WorkflowState::Publishing | WorkflowState::PublishUnknown
        ) {
            return Err(WorkflowStoreError::InvalidState {
                workflow_id: expected.workflow_id,
                expected: WorkflowState::PublishUnknown,
                actual: current,
            });
        }
        let published_oid = (state == WorkflowState::Published).then_some(&expected.source.oid);
        let changed = transaction
            .execute(
                "UPDATE workflows SET state = ?1, published_oid = ?2, revision = revision + 1
                 WHERE workflow_id = ?3 AND generation = ?4
                       AND state IN ('publishing', 'publish_unknown') AND revision = ?5
                       AND publish_attempt = ?6 AND manifest_id = ?7 AND source_ref = ?8
                       AND source_oid = ?9 AND target_ref = ?10 AND target_oid = ?11
                       AND policy_digest = ?12 AND publish_nonce = ?13
                       AND repository_id = ?14 AND worktree_id = ?15 AND publisher_scope = ?16",
                params![
                    state.as_db(),
                    published_oid,
                    expected.workflow_id,
                    to_sql_u64(expected.generation)?,
                    to_sql_u64(material.revision)?,
                    to_sql_u64(material.attempt)?,
                    expected.manifest_id,
                    expected.source.name,
                    expected.source.oid,
                    expected.target.name,
                    expected.target.oid,
                    expected.policy_digest,
                    material
                        .publish_nonce
                        .as_deref()
                        .ok_or(WorkflowStoreError::Corrupt)?,
                    expected.repository_id,
                    expected.worktree_id,
                    expected.publisher_scope,
                ],
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        require_one(changed, &expected.workflow_id)?;
        let record = select_required(&transaction, &expected.workflow_id)?.record()?;
        transaction
            .commit()
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(record)
    }

    /// A raw connection for FAULT INJECTION in another crate's tests.
    ///
    /// Hidden and named for what it is: the only supported use is a test
    /// standing between this store and its own database — a trigger that
    /// makes a write refuse, a dropped table that makes a read fail — which
    /// is the same seam this crate's own disk-fault battery injects at.
    /// Production code has no business here; the store's own roads use the
    /// private [`connection`].
    ///
    /// [`connection`]: WorkflowStore::connection
    #[doc(hidden)]
    pub fn fault_connection_for_tests(&self) -> Result<Connection, WorkflowStoreError> {
        self.connection()
    }

    pub(crate) fn connection(&self) -> Result<Connection, WorkflowStoreError> {
        // The Unix parent gate excludes other principals. A hostile same-UID
        // process (and platforms without equivalent ACL validation) remains
        // outside this foundation's boundary until a custom VFS owns all opens.
        preflight_store_sidecars(&self.path)?;
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection = Connection::open_with_flags(&self.path, flags)
            .map_err(|_| WorkflowStoreError::Database)?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|_| WorkflowStoreError::Database)?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|_| WorkflowStoreError::Database)?;
        connection
            .pragma_update(None, "synchronous", "FULL")
            .map_err(|_| WorkflowStoreError::Database)?;
        Ok(connection)
    }
}

/// A database already claiming the current schema must actually have every
/// authority table. Re-creating a missing effect table would turn durable
/// `Prepared`/`Unknown` operations into silence, so current-schema damage fails
/// closed; only an older declared schema is migrated.
fn require_schema_tables(connection: &Connection) -> Result<(), WorkflowStoreError> {
    const TABLES: [&str; 7] = [
        "workflow_generations",
        "workflows",
        "workflow_history",
        "trusted_test_evidence",
        "orchestration_ledger_heads",
        "host_effect_operations",
        "host_effect_steps",
    ];
    for table in TABLES {
        let present: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name = ?1",
                [table],
                |row| row.get(0),
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        if present != 1 {
            return Err(WorkflowStoreError::Corrupt);
        }
    }
    Ok(())
}

fn column_exists(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, WorkflowStoreError> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column],
            |row| row.get::<_, i64>(0),
        )
        .map(|count| count == 1)
        .map_err(|_| WorkflowStoreError::Database)
}
fn require_authority_schema(connection: &Connection) -> Result<(), WorkflowStoreError> {
    let owner_table: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
              WHERE type = 'table' AND name = 'orchestration_authorities'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)?;
    let index: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
              WHERE type = 'index' AND name = 'host_effect_authority_recovery'
                AND tbl_name = 'host_effect_operations'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)?;
    let expected_table = normalized_creation_sql(AUTHORITY_TABLE_SQL)?;
    let expected_index = normalized_creation_sql(AUTHORITY_RECOVERY_INDEX_SQL)?;
    if owner_table
        .as_deref()
        .and_then(normalized_schema_sql)
        .is_some_and(|sql| sql == expected_table)
        && index
            .as_deref()
            .and_then(normalized_schema_sql)
            .is_some_and(|sql| sql == expected_index)
        && valid_operation_authority_column(connection)?
    {
        Ok(())
    } else {
        Err(WorkflowStoreError::Corrupt)
    }
}

fn valid_operation_authority_column(connection: &Connection) -> Result<bool, WorkflowStoreError> {
    let column: Option<(String, i64, Option<String>, i64)> = connection
        .query_row(
            "SELECT type, \"notnull\", dflt_value, pk
               FROM pragma_table_info('host_effect_operations')
              WHERE name = 'operation_authority_epoch'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)?;
    let table_sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema
              WHERE type = 'table' AND name = 'host_effect_operations'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)?;
    let exact_check = table_sql
        .as_deref()
        .and_then(normalized_schema_sql)
        .is_some_and(|sql| {
            sql.contains(
                "operation_authority_epochintegernotnulldefault0check(operation_authority_epoch>=0)",
            )
        });
    Ok(exact_check
        && column.is_some_and(|(kind, not_null, default, primary_key)| {
            kind.eq_ignore_ascii_case("INTEGER")
                && not_null == 1
                && default.as_deref() == Some("0")
                && primary_key == 0
        }))
}

fn normalized_creation_sql(sql: &str) -> Result<String, WorkflowStoreError> {
    normalized_schema_sql(sql)
        .map(|sql| {
            sql.replacen("ifnotexists", "", 1)
                .trim_end_matches(';')
                .to_string()
        })
        .ok_or(WorkflowStoreError::Corrupt)
}

/// Refuse a v2 -> v3 upgrade if legacy rows already violate the authority the
/// partial unique index is about to make structural. The migration transaction
/// then rolls back without advancing `user_version` or discarding either row.
fn require_single_in_flight_effect(connection: &Connection) -> Result<(), WorkflowStoreError> {
    let duplicate: Option<i64> = connection
        .query_row(
            "SELECT 1
               FROM host_effect_operations
              WHERE effect_state IN ('prepared', 'unknown')
              GROUP BY ledger_id
             HAVING COUNT(*) > 1
              LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)?;
    if duplicate.is_some() {
        Err(WorkflowStoreError::Corrupt)
    } else {
        Ok(())
    }
}

/// A database claiming schema three must have the exact unique partial index,
/// not merely an index with the expected name. A missing `unknown` predicate,
/// a non-unique replacement, or an index on another column all fail closed.
fn require_effect_schema_indexes(connection: &Connection) -> Result<(), WorkflowStoreError> {
    const EXPECTED: &str = "createuniqueindexhost_effect_single_in_flightonhost_effect_operations(ledger_id)whereeffect_statein('prepared','unknown')";
    let sql: Option<String> = connection
        .query_row(
            "SELECT sql
               FROM sqlite_schema
              WHERE type = 'index'
                AND name = 'host_effect_single_in_flight'
                AND tbl_name = 'host_effect_operations'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)?;
    let Some(sql) = sql else {
        return Err(WorkflowStoreError::Corrupt);
    };
    if normalized_schema_sql(&sql).as_deref() == Some(EXPECTED) {
        Ok(())
    } else {
        Err(WorkflowStoreError::Corrupt)
    }
}

/// Normalize SQL syntax without changing the bytes SQLite compares inside
/// string literals. In particular, `'PREPARED'` must never be mistaken for the
/// lowercase state value the partial index is required to cover.
fn normalized_schema_sql(sql: &str) -> Option<String> {
    let mut normalized = String::with_capacity(sql.len());
    let mut quoted = false;
    let mut characters = sql.chars().peekable();
    while let Some(one) = characters.next() {
        if one == '\'' {
            normalized.push(one);
            if quoted && characters.peek() == Some(&'\'') {
                normalized.push(characters.next()?);
            } else {
                quoted = !quoted;
            }
        } else if quoted {
            normalized.push(one);
        } else if !one.is_ascii_whitespace() {
            normalized.push(one.to_ascii_lowercase());
        }
    }
    (!quoted).then_some(normalized)
}

/* ---- reading a store nobody is allowed to change ---------------------- */

/// One stored workflow, as a READ sees it: what it is bound to, what was
/// decided about it, and the receipts its manifest carried.
///
/// Deliberately not [`WorkflowRecord`]: that type is what a transition works
/// on and carries the whole manifest and policy. This is what a reader needs,
/// and a reader gets no manifest bytes and no policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredReview {
    pub workflow_id: String,
    pub generation: i64,
    pub manifest_id: Option<String>,
    /// The store's own word (`assigned`, `approved`, `published`, …), or
    /// `archived` for a row read out of the history table.
    pub state: String,
    pub reviewer_id: Option<String>,
    pub review_decision: Option<String>,
    /// The receipts inside the stored manifest. Empty when the row had no
    /// manifest, and when the manifest was larger than the read's bound —
    /// [`Self::manifest_unread`] tells those two apart.
    pub manifest_tests: Vec<crate::handoff::TestReceipt>,
    /// A manifest the read declined to parse because of its size.
    pub manifest_unread: bool,
}

/// One row of `trusted_test_evidence`, without the digests and artifact ids
/// that only the verifier itself has any use for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedReceiptRow {
    pub manifest_id: String,
    pub test_name: String,
    pub command_id: String,
    pub snapshot_digest: String,
    pub source_oid: String,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub exit_code: i32,
    pub succeeded: bool,
    pub output_truncated: bool,
}

/// A connection to an EXISTING authority store that cannot change it.
///
/// Three things are deliberately different from the store's own private
/// connection:
///
/// - **`CREATE` is not among the flags.** A reader that made the file would
///   answer "this window has no authority" by inventing one.
/// - **The migration is not run.** [`WorkflowStore::open`] may write tables
///   and a version; a read refuses an unexpected version instead.
/// - **The file is not chmod-ed.** The writer's preflight opens for writing
///   and tightens the mode, which is right for a store about to be written
///   and wrong for one only being read. The symlink and regular-file refusals
///   are kept, on the store and on both WAL sidecars.
///
/// The flags are `READ_WRITE` without `CREATE` rather than `READ_ONLY`, and
/// `PRAGMA query_only` is what makes it a read: a WAL database opened
/// read-only needs its `-shm` to exist, and a store whose last writer checked
/// out and removed the sidecars would be unreadable for that reason alone.
/// `query_only` makes SQLite refuse every statement that would change the
/// database, which is the property this needs.
pub struct ReadOnlyWorkflows {
    connection: Connection,
}

impl std::fmt::Debug for ReadOnlyWorkflows {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReadOnlyWorkflows")
            .finish_non_exhaustive()
    }
}

impl ReadOnlyWorkflows {
    /// Open an existing store for reading. `Ok(None)` is "there is no store
    /// here", which is an answer and not a failure.
    pub fn open(path: &Path) -> Result<Option<Self>, WorkflowStoreError> {
        // The symlink refusal comes FIRST and on the path as given: resolving
        // it would follow exactly the link this is here to turn away.
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(WorkflowStoreError::UnsafeStorePath);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(WorkflowStoreError::UnsafeStorePath),
        }
        /* Resolved before SQLite sees it, the way [`WorkflowStore::open`]
         * resolves its own. `SQLITE_OPEN_NOFOLLOW` refuses a path with a
         * symlinked COMPONENT, and on macOS the system temporary directory is
         * reached through one (`/var` → `/private/var`) — so an unresolved
         * path turns every store under it into "cannot open". */
        let path = fs::canonicalize(path).map_err(|_| WorkflowStoreError::UnsafeStorePath)?;
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            match fs::symlink_metadata(PathBuf::from(sidecar)) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                    return Err(WorkflowStoreError::UnsafeStorePath);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(WorkflowStoreError::UnsafeStorePath),
            }
        }
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW;
        let connection =
            Connection::open_with_flags(&path, flags).map_err(|_| WorkflowStoreError::Database)?;
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|_| WorkflowStoreError::Database)?;
        /* Spelled as a statement rather than through `pragma_update`, which
         * quotes its value: SQLite reads `query_only = 'ON'` as a string where
         * this pragma wants a boolean. */
        connection
            .execute_batch("PRAGMA query_only = ON;")
            .map_err(|_| WorkflowStoreError::Database)?;
        let found: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|_| WorkflowStoreError::Database)?;
        if found != STORE_SCHEMA_VERSION {
            return Err(WorkflowStoreError::UnsupportedSchema { found });
        }
        Ok(Some(Self { connection }))
    }

    /// Every workflow bound to one worktree, live rows and archived
    /// generations together, newest generation first.
    pub fn reviews_for(
        &self,
        repository_id: &str,
        worktree_id: &str,
        limit: usize,
    ) -> Result<Vec<StoredReview>, WorkflowStoreError> {
        let manifest_bytes =
            i64::try_from(crate::worktree_evidence::MAX_MANIFEST_BYTES).unwrap_or(i64::MAX);
        let mut statement = self
            .connection
            .prepare(
                "SELECT workflow_id, generation, manifest_id,
                        CASE WHEN manifest_json IS NULL THEN 0 ELSE 1 END,
                        CASE WHEN length(manifest_json) <= ?3 THEN manifest_json END,
                        reviewer_id, review_decision, state
                   FROM workflows
                  WHERE repository_id = ?1 AND worktree_id = ?2
                  UNION ALL
                 SELECT history.workflow_id, history.generation, history.manifest_id,
                        1,
                        CASE WHEN length(history.manifest_json) <= ?3
                             THEN history.manifest_json END,
                        history.reviewer_id, history.review_decision, 'archived'
                   FROM workflow_history AS history
                   JOIN workflows AS held ON held.workflow_id = history.workflow_id
                  WHERE held.repository_id = ?1 AND held.worktree_id = ?2
                  ORDER BY generation DESC
                  LIMIT ?4",
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        let rows = statement
            .query_map(
                params![
                    repository_id,
                    worktree_id,
                    manifest_bytes,
                    to_sql_usize(limit)?
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, i64>(3)? == 1,
                        row.get::<_, Option<Vec<u8>>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .map_err(|_| WorkflowStoreError::Database)?;
        let mut reviews = Vec::new();
        for row in rows {
            let (
                workflow_id,
                generation,
                manifest_id,
                has_manifest,
                manifest,
                reviewer,
                decision,
                state,
            ) = row.map_err(|_| WorkflowStoreError::Database)?;
            let parsed = manifest
                .as_deref()
                .map(serde_json::from_slice::<HandoffManifestV1>);
            // A manifest the store holds and this read could not parse is a
            // corrupt row, not an empty one: answering "no tests" for it
            // would be inventing an absence.
            let manifest = match parsed {
                Some(Ok(held)) => Some(held),
                Some(Err(_)) => return Err(WorkflowStoreError::Corrupt),
                None => None,
            };
            reviews.push(StoredReview {
                workflow_id,
                generation,
                manifest_id,
                state,
                reviewer_id: reviewer,
                review_decision: decision,
                manifest_unread: has_manifest && manifest.is_none(),
                manifest_tests: manifest.map(|held| held.tests).unwrap_or_default(),
            });
        }
        Ok(reviews)
    }

    /// The host-trusted receipts filed against any of these manifests.
    ///
    /// The ids are bound by the caller's own manifest list, so the `IN` set is
    /// as bounded as that list is. Newest ending first.
    pub fn trusted_receipts_for(
        &self,
        manifest_ids: &[String],
        limit: usize,
    ) -> Result<Vec<TrustedReceiptRow>, WorkflowStoreError> {
        if manifest_ids.is_empty() {
            return Ok(Vec::new());
        }
        let places = (1..=manifest_ids.len())
            .map(|at| format!("?{at}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT manifest_id, test_name, command_id, snapshot_digest, source_oid,
                    started_at_ms, ended_at_ms, exit_code, succeeded, output_truncated
               FROM trusted_test_evidence
              WHERE manifest_id IN ({places})
              ORDER BY ended_at_ms DESC
              LIMIT ?{}",
            manifest_ids.len() + 1
        );
        let mut statement = self
            .connection
            .prepare(&sql)
            .map_err(|_| WorkflowStoreError::Database)?;
        let mut bound: Vec<&dyn rusqlite::ToSql> = manifest_ids
            .iter()
            .map(|id| id as &dyn rusqlite::ToSql)
            .collect();
        let limit = to_sql_usize(limit)?;
        bound.push(&limit);
        let rows = statement
            .query_map(bound.as_slice(), |row| {
                Ok(TrustedReceiptRow {
                    manifest_id: row.get(0)?,
                    test_name: row.get(1)?,
                    command_id: row.get(2)?,
                    snapshot_digest: row.get(3)?,
                    source_oid: row.get(4)?,
                    started_at_ms: row.get(5)?,
                    ended_at_ms: row.get(6)?,
                    exit_code: row
                        .get::<_, i64>(7)?
                        .clamp(i64::from(i32::MIN), i64::from(i32::MAX))
                        as i32,
                    succeeded: row.get::<_, i64>(8)? == 1,
                    output_truncated: row.get::<_, i64>(9)? == 1,
                })
            })
            .map_err(|_| WorkflowStoreError::Database)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|_| WorkflowStoreError::Database)
    }
}

fn to_sql_usize(value: usize) -> Result<i64, WorkflowStoreError> {
    i64::try_from(value).map_err(|_| WorkflowStoreError::InvalidInput { field: "limit" })
}

fn prepare_store_path(path: &Path) -> Result<(), WorkflowStoreError> {
    let parent = path.parent().ok_or(WorkflowStoreError::UnsafeStorePath)?;
    let parent_metadata =
        fs::symlink_metadata(parent).map_err(|_| WorkflowStoreError::UnsafeStorePath)?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(WorkflowStoreError::UnsafeStorePath);
    }
    private_parent(&parent_metadata)?;

    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(WorkflowStoreError::UnsafeStorePath);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt as _;
                options.mode(0o600);
            }
            match options.open(path) {
                Ok(_) => {}
                // Another opener may have created it after our metadata
                // read. The descriptor-based check below still refuses a
                // symlink or non-file; existence alone never grants trust.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(WorkflowStoreError::UnsafeStorePath),
            }
        }
        Err(_) => return Err(WorkflowStoreError::UnsafeStorePath),
    }
    private_file(path).map_err(|_| WorkflowStoreError::UnsafeStorePath)
}

#[cfg(unix)]
fn private_parent(metadata: &fs::Metadata) -> Result<(), WorkflowStoreError> {
    use std::os::unix::fs::PermissionsExt as _;

    if metadata.permissions().mode() & 0o077 == 0 {
        Ok(())
    } else {
        Err(WorkflowStoreError::UnsafeStorePath)
    }
}

#[cfg(not(unix))]
fn private_parent(_metadata: &fs::Metadata) -> Result<(), WorkflowStoreError> {
    Ok(())
}

#[cfg(unix)]
fn private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "workflow storage is not a regular file",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn private_file(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "workflow storage is not a regular file",
        ));
    }
    Ok(())
}

fn tighten_store_files(path: &Path) -> Result<(), WorkflowStoreError> {
    private_file(path).map_err(|_| WorkflowStoreError::UnsafeStorePath)?;
    preflight_store_sidecars(path)
}

fn preflight_store_sidecars(path: &Path) -> Result<(), WorkflowStoreError> {
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        match private_file(&sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(WorkflowStoreError::UnsafeStorePath),
        }
    }
    Ok(())
}

fn immediate(connection: &mut Connection) -> Result<Transaction<'_>, WorkflowStoreError> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| WorkflowStoreError::Database)
}

/// Only the idempotent connection setup is retried. Every attempt shares the
/// existing busy budget, and migration/assignment transactions are unchanged.
fn configure_wal(connection: &Connection) -> rusqlite::Result<()> {
    let began = std::time::Instant::now();
    // SQLite's busy timeout counts its own sleep intervals, which can run
    // longer in wall time under load. This loop owns the setup deadline;
    // let each pragma return BUSY immediately instead of nesting two waits.
    connection.busy_timeout(Duration::ZERO)?;
    let configure =
        || connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;");
    let mut answer = configure();
    while matches!(&answer, Err(rusqlite::Error::SqliteFailure(error, _))
        if error.code == rusqlite::ErrorCode::DatabaseBusy)
    {
        let remaining = BUSY_TIMEOUT.saturating_sub(began.elapsed());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(WAL_RETRY_DELAY.min(remaining));
        let remaining = BUSY_TIMEOUT.saturating_sub(began.elapsed());
        if remaining.is_zero() {
            break;
        }
        answer = configure();
    }
    connection.busy_timeout(BUSY_TIMEOUT)?;
    answer
}

fn select_required(
    connection: &Connection,
    workflow_id: &str,
) -> Result<DatabaseRow, WorkflowStoreError> {
    select_row(connection, workflow_id)?.ok_or_else(|| WorkflowStoreError::NotFound {
        workflow_id: workflow_id.to_string(),
    })
}

fn select_row(
    connection: &Connection,
    workflow_id: &str,
) -> Result<Option<DatabaseRow>, WorkflowStoreError> {
    connection
        .query_row(
            "SELECT workflow_id, generation, parent_generation, assignee_id, repository_id, worktree_id,
                    publisher_scope, base_oid, target_ref, target_oid, state, manifest_id,
                    manifest_json, source_ref, source_oid, policy_json, policy_digest,
                    reviewer_id, review_decision, publish_attempt, publish_nonce,
                    published_oid, revision
             FROM workflows WHERE workflow_id = ?1",
            [workflow_id],
            |row| {
                Ok(DatabaseRow {
                    workflow_id: row.get(0)?,
                    generation: row.get(1)?,
                    parent_generation: row.get(2)?,
                    assignee_id: row.get(3)?,
                    repository_id: row.get(4)?,
                    worktree_id: row.get(5)?,
                    publisher_scope: row.get(6)?,
                    base_oid: row.get(7)?,
                    target_ref: row.get(8)?,
                    target_oid: row.get(9)?,
                    state: row.get(10)?,
                    manifest_id: row.get(11)?,
                    manifest_json: row.get(12)?,
                    source_ref: row.get(13)?,
                    source_oid: row.get(14)?,
                    policy_json: row.get(15)?,
                    policy_digest: row.get(16)?,
                    reviewer_id: row.get(17)?,
                    review_decision: row.get(18)?,
                    publish_attempt: row.get(19)?,
                    publish_nonce: row.get(20)?,
                    published_oid: row.get(21)?,
                    revision: row.get(22)?,
                })
            },
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)
}

fn expected_parent_manifest(
    connection: &Connection,
    row: &DatabaseRow,
) -> Result<Option<String>, WorkflowStoreError> {
    let Some(parent_generation) = row.parent_generation else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT manifest_id FROM workflow_history
              WHERE workflow_id = ?1 AND generation = ?2",
            params![row.workflow_id, parent_generation],
            |result| result.get(0),
        )
        .optional()
        .map_err(|_| WorkflowStoreError::Database)?
        .map(Some)
        .ok_or(WorkflowStoreError::Corrupt)
}

fn validate_assignment(assignment: &NewAssignment) -> Result<(), WorkflowStoreError> {
    for (field, value) in [
        ("workflow_id", assignment.workflow_id.as_str()),
        ("assignee_id", assignment.assignee_id.as_str()),
        ("repository_id", assignment.repository_id.as_str()),
        ("worktree_id", assignment.worktree_id.as_str()),
        ("publisher_scope", assignment.publisher_scope.as_str()),
    ] {
        if !valid_identity(value) {
            return Err(WorkflowStoreError::InvalidInput { field });
        }
    }
    if !valid_oid(&assignment.base_oid) {
        return Err(WorkflowStoreError::InvalidInput { field: "base_oid" });
    }
    if !valid_publish_target_ref(&assignment.target_ref) {
        return Err(WorkflowStoreError::InvalidInput {
            field: "target_ref",
        });
    }
    if !valid_branch_ref(&assignment.source_ref) || assignment.source_ref == assignment.target_ref {
        return Err(WorkflowStoreError::InvalidInput {
            field: "source_ref",
        });
    }
    if assignment.policy.required_tests.len() > MAX_WORKFLOW_REQUIRED_TESTS
        || assignment.policy.required_tests.iter().any(|requirement| {
            !valid_identity(&requirement.name) || !valid_identity(&requirement.command_id)
        })
    {
        return Err(WorkflowStoreError::InvalidInput {
            field: "publish_policy",
        });
    }
    Ok(())
}

fn require_generation(
    row: &DatabaseRow,
    workflow_id: &str,
    expected: u64,
) -> Result<(), WorkflowStoreError> {
    let actual = row.generation()?;
    if actual == expected {
        Ok(())
    } else {
        Err(WorkflowStoreError::GenerationMismatch {
            workflow_id: workflow_id.to_string(),
            expected,
            actual,
        })
    }
}

fn require_expectation(
    row: &DatabaseRow,
    expected: &PublishExpectation,
) -> Result<(), WorkflowStoreError> {
    let matches = row.manifest_id.as_deref() == Some(expected.manifest_id.as_str())
        && row.repository_id == expected.repository_id
        && row.worktree_id == expected.worktree_id
        && row.publisher_scope == expected.publisher_scope
        && row.source_ref == expected.source.name
        && row.source_oid.as_deref() == Some(expected.source.oid.as_str())
        && row.target_ref == expected.target.name
        && row.target_oid == expected.target.oid
        && row.policy_digest.as_deref() == Some(expected.policy_digest.as_str());
    if matches {
        Ok(())
    } else {
        Err(WorkflowStoreError::ImmutableSubmission {
            workflow_id: expected.workflow_id.clone(),
        })
    }
}

fn require_one(changed: usize, workflow_id: &str) -> Result<(), WorkflowStoreError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(WorkflowStoreError::ConcurrentTransition {
            workflow_id: workflow_id.to_string(),
        })
    }
}

fn to_sql_u64(value: u64) -> Result<i64, WorkflowStoreError> {
    i64::try_from(value).map_err(|_| WorkflowStoreError::Corrupt)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory a store will agree to live in.
    ///
    /// `prepare_store_path` refuses a world-readable parent, which is the
    /// right answer and also the reason every one of these tests has to say
    /// so first.
    fn a_private_root() -> tempfile::TempDir {
        let root = tempfile::tempdir().expect("store root");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;

            fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
                .expect("private root");
        }
        root
    }

    #[test]
    fn concurrent_first_openers_share_one_initialized_store() {
        let root = a_private_root();
        for wave in 0..8 {
            let path = root.path().join(format!("first-open-{wave}.sqlite"));
            let barrier = std::sync::Barrier::new(8);
            let opened = std::thread::scope(|scope| {
                let threads: Vec<_> = (0..8)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            WorkflowStore::open(&path)
                        })
                    })
                    .collect();
                threads
                    .into_iter()
                    .map(|thread| thread.join().expect("opener thread"))
                    .collect::<Vec<_>>()
            });
            let errors: Vec<_> = opened
                .iter()
                .filter_map(|result| result.as_ref().err())
                .collect();
            assert!(errors.is_empty(), "cold-open wave {wave}: {errors:?}");
            for store in opened.into_iter().map(Result::unwrap) {
                assert!(matches!(
                    store.get("not-created"),
                    Err(WorkflowStoreError::NotFound { .. })
                ));
            }
        }
    }

    #[test]
    fn wal_setup_stays_bounded_and_restores_the_connection_busy_timeout() {
        static NESTED_WAITS: std::sync::atomic::AtomicUsize =
            std::sync::atomic::AtomicUsize::new(0);
        fn count_nested_wait(_: i32) -> bool {
            NESTED_WAITS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        }

        let root = a_private_root();
        let path = root.path().join("locked.sqlite");
        let reader = Connection::open(&path).expect("reader");
        let connection = Connection::open(&path).expect("writer");
        connection.busy_timeout(BUSY_TIMEOUT).expect("busy budget");
        connection
            .busy_handler(Some(count_nested_wait))
            .expect("witness any nested SQLite wait");
        reader
            .execute_batch(
                "CREATE TABLE held(value); INSERT INTO held VALUES (1); BEGIN; SELECT * FROM held;",
            )
            .expect("hold a read lock during WAL conversion");
        let (send, receive) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            let result = configure_wal(&connection).map_err(|error| error.sqlite_error_code());
            let timeout: i64 = connection
                .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
                .unwrap();
            let _ = send.send((result, timeout));
        });
        let result = receive.recv_timeout(BUSY_TIMEOUT * 2);
        // Always free the fixture's lock before joining, including a failed
        // deadline assertion, so the regression cannot strand a test thread.
        reader.execute_batch("ROLLBACK").expect("release reader");
        writer.join().expect("writer thread");
        let (outcome, timeout) =
            result.expect("WAL setup must return while the reader still holds its lock");
        assert_eq!(outcome, Err(Some(rusqlite::ErrorCode::DatabaseBusy)));
        assert_eq!(NESTED_WAITS.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(timeout, i64::try_from(BUSY_TIMEOUT.as_millis()).unwrap());
    }

    #[test]
    fn every_connection_reasserts_the_durability_and_integrity_pragmas() {
        let root = a_private_root();
        let store = WorkflowStore::open(root.path().join("workflow.sqlite")).expect("store");
        let connection = store.connection().expect("fresh connection");
        let synchronous: i64 = connection
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous pragma");
        let foreign_keys: i64 = connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("foreign key pragma");
        let busy_timeout: i64 = connection
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy timeout pragma");
        assert_eq!(synchronous, 2, "SQLite FULL is numeric level 2");
        assert_eq!(foreign_keys, 1);
        assert_eq!(busy_timeout, 5_000);
    }

    /// A store this window will not read is refused by name, and the file is
    /// left exactly as it was found.
    ///
    /// Both directions of "not five". A LOWER number is a store from before
    /// the ledger became tables, and the ruling on those is that there is
    /// nothing to carry across — refusing is the whole behaviour, so the file
    /// must come out of the attempt byte for byte, not silently re-initialised
    /// under a person. A HIGHER one is a file a LATER window wrote, and a
    /// window that rewrote a file it cannot read would be destroying work it
    /// never understood.
    ///
    /// The bytes are compared, not the tables: a refusal that left the tables
    /// alone but rewrote the header would still be a refusal that edited
    /// somebody's file.
    #[test]
    fn a_store_from_another_window_is_refused_by_name_and_left_untouched() {
        for found in [1_i64, 2, 3, 4, 5, 6, 8] {
            let root = a_private_root();
            let path = root.path().join("workflow.sqlite");
            {
                let store = WorkflowStore::open(&path).expect("a store this window wrote");
                let connection = store.connection().expect("connection");
                connection
                    .pragma_update(None, "user_version", found)
                    .expect("stamp the other window's version");
                /* And leave it in the journal mode another window might have
                 * chosen. This is what makes the ordering inside `open` a
                 * fact rather than a preference: asking for WAL rewrites the
                 * header of a database that is not already in it, so a version
                 * read that happened AFTER that pragma would have edited the
                 * file it then refused. */
                connection
                    .pragma_update(None, "journal_mode", "DELETE")
                    .expect("another window's journal mode");
            }
            let before = fs::read(&path).expect("the file as it was");

            let refused = WorkflowStore::open(&path).expect_err("this must not open");
            assert!(
                matches!(refused, WorkflowStoreError::UnsupportedSchema { found: seen } if seen == found),
                "schema {found} was refused with {refused:?}"
            );

            let after = fs::read(&path).expect("the file afterwards");
            assert_eq!(
                before, after,
                "opening a schema {found} store rewrote the file it refused"
            );
        }
    }

    /// A store this window wrote opens again, which is what makes the refusal
    /// above a judgement rather than a wall.
    #[test]
    fn a_store_this_window_wrote_opens_again() {
        let root = a_private_root();
        let path = root.path().join("workflow.sqlite");
        let first = WorkflowStore::open(&path).expect("a fresh store");
        let version: i64 = first
            .connection()
            .expect("connection")
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("the version it stamped");
        assert_eq!(version, STORE_SCHEMA_VERSION);
        drop(first);
        WorkflowStore::open(&path).expect("it opens again");
    }

    /// The ledger's tables grow additively, and a store already at the
    /// current version gains the ones it predates on its next open — the
    /// gates tables landed without a version bump, and a file from before
    /// them would otherwise answer every ledger read and write with "no
    /// such table" for the rest of its life. The drops below are this test
    /// wearing the older build's hat.
    #[test]
    fn an_already_current_store_gains_the_tables_the_ledger_grew() {
        let root = a_private_root();
        let path = root.path().join("workflow.sqlite");
        let store = WorkflowStore::open(&path).expect("a fresh store");
        store
            .connection()
            .expect("a connection to age the file with")
            .execute_batch(
                "INSERT INTO orchestration_ledger_heads
                    (ledger_id, revision, next_id, bytes_held, updated_at_ms)
                 VALUES ('main-ledger', 3, 9, 7, 100);
                 INSERT INTO ledger_runs (ledger_id, ordinal, id, name, created_ms)
                 VALUES ('main-ledger', 0, 'run-1', 'nightly', 1);
                 DROP TABLE ledger_gate_options;
                 DROP TABLE ledger_gates;",
            )
            .expect("the populated shape the older build left behind");
        drop(store);

        let reopened = WorkflowStore::open(&path).expect("an upgraded window opens it");
        let connection = reopened.connection().expect("a connection to look with");
        let held: i64 = connection
            .query_row("SELECT COUNT(*) FROM ledger_gates", [], |row| row.get(0))
            .expect("the table the ledger grew since this store was written");
        let name: String = connection
            .query_row(
                "SELECT name FROM ledger_runs WHERE ledger_id = 'main-ledger'",
                [],
                |row| row.get(0),
            )
            .expect("the run the older store already held");
        assert_eq!(held, 0, "a table new to this build holds nothing yet");
        assert_eq!(name, "nightly", "the additive table cost an existing run");
        drop(connection);

        // And the same door hands an old store its new COLUMNS — the message
        // fields landed without a version bump too, and `IF NOT EXISTS` says
        // nothing about columns.
        reopened
            .connection()
            .expect("a connection to age the file with")
            .execute_batch(
                "ALTER TABLE ledger_messages DROP COLUMN payload;
                 ALTER TABLE ledger_workers DROP COLUMN session;",
            )
            .expect("the shape the older build left behind");
        drop(reopened);
        let grown = WorkflowStore::open(&path).expect("an upgraded window opens it again");
        let connection = grown.connection().expect("a connection to look with");
        for (table, column) in [
            ("ledger_messages", "payload"),
            ("ledger_workers", "session"),
        ] {
            assert!(
                column_exists(&connection, table, column).expect("a column question"),
                "an old store did not gain {table}.{column}"
            );
        }
    }

    /// Retention arrives the same way, and an old head row comes back with
    /// the DEFAULT policy rather than with nothing.
    ///
    /// The distinction is the whole migration: a head with no policy column
    /// belonged to a window that never swept, so the number it gains has to
    /// be the one that keeps a month — a zero would read as "compact every
    /// finished run at once", which is the one answer nobody chose.
    #[test]
    fn an_old_store_gains_a_retention_policy_and_keeps_the_rows_it_had() {
        let root = a_private_root();
        let path = root.path().join("workflow.sqlite");
        let store = WorkflowStore::open(&path).expect("a fresh store");
        {
            let connection = store.connection().expect("a connection to age the file");
            connection
                .execute_batch(
                    "ALTER TABLE orchestration_ledger_heads DROP COLUMN retention_days;
                     ALTER TABLE orchestration_ledger_heads DROP COLUMN swept_at_ms;
                     ALTER TABLE ledger_runs DROP COLUMN summary;
                     ALTER TABLE ledger_served DROP COLUMN filed_ms;
                     ALTER TABLE ledger_served DROP COLUMN expired;",
                )
                .expect("the shape the older build left behind");
            connection
                .execute_batch(
                    "INSERT INTO orchestration_ledger_heads
                        (ledger_id, revision, next_id, bytes_held, updated_at_ms)
                     VALUES ('main-ledger', 3, 9, 7, 100);
                     INSERT INTO ledger_runs (ledger_id, ordinal, id, name, created_ms)
                     VALUES ('main-ledger', 0, 'run-1', 'nightly', 1);",
                )
                .expect("a ledger the older build wrote");
        }
        drop(store);

        let grown = WorkflowStore::open(&path).expect("an upgraded window opens it");
        let connection = grown.connection().expect("a connection to look with");
        let (days, swept): (i64, i64) = connection
            .query_row(
                "SELECT retention_days, swept_at_ms FROM orchestration_ledger_heads
                  WHERE ledger_id = 'main-ledger'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("the columns the ledger grew");
        assert_eq!(days, 30, "an old head did not gain the default policy");
        assert_eq!(swept, 0, "an old head claims it has already been swept");
        // The row it already held is still there, unchanged — an additive
        // column must not cost a person their history.
        let name: String = connection
            .query_row(
                "SELECT name FROM ledger_runs WHERE ledger_id = 'main-ledger'",
                [],
                |row| row.get(0),
            )
            .expect("the run the older build wrote");
        assert_eq!(name, "nightly");
        for (table, column) in [
            ("ledger_runs", "summary"),
            ("ledger_served", "filed_ms"),
            ("ledger_served", "expired"),
        ] {
            assert!(
                column_exists(&connection, table, column).expect("a column question"),
                "{table} did not gain {column}"
            );
        }
    }

    /// No DDL string may carry a comment, because `normalized_schema_sql` is
    /// comment-blind: one apostrophe inside a SQL comment flips its quote
    /// tracking and every fresh open reads its own schema as Corrupt. The
    /// rule stood as prose; this is its net — the named constants swept for
    /// comment openers, and every row SQLite actually stored normalizing to
    /// `Some`, whatever statement put it there.
    #[test]
    fn the_stored_schema_stays_readable_to_its_own_normalizer() {
        for (name, sql) in [
            ("FRESH_TABLES_SQL", FRESH_TABLES_SQL),
            ("SINGLE_IN_FLIGHT_INDEX_SQL", SINGLE_IN_FLIGHT_INDEX_SQL),
            ("AUTHORITY_TABLE_SQL", AUTHORITY_TABLE_SQL),
            ("AUTHORITY_RECOVERY_INDEX_SQL", AUTHORITY_RECOVERY_INDEX_SQL),
            ("LEDGER_TABLES_SQL", crate::ledger_store::LEDGER_TABLES_SQL),
        ] {
            assert!(!sql.contains("/*"), "{name} carries a block comment");
            assert!(!sql.contains("--"), "{name} carries a line comment");
        }

        let root = a_private_root();
        let store =
            WorkflowStore::open(root.path().join("workflow.sqlite")).expect("a fresh store");
        let connection = store.connection().expect("schema connection");
        let mut statement = connection
            .prepare("SELECT name, sql FROM sqlite_schema WHERE sql IS NOT NULL")
            .expect("schema rows");
        let rows: Vec<(String, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("schema query")
            .collect::<Result<_, _>>()
            .expect("schema row");
        assert!(!rows.is_empty(), "a fresh store has a schema to sweep");
        for (name, sql) in rows {
            assert!(
                normalized_schema_sql(&sql).is_some(),
                "the stored text of `{name}` does not normalize — an unbalanced \
                 quote reached sqlite_schema"
            );
        }
    }
}
