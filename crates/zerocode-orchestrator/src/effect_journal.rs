//! Dormant durable authority for orchestration host effects.
//!
//! This module owns no terminal and executes no effect. It only provides the
//! compare-and-swap boundary a future orchestration actor will need: the
//! ledger projection that reserves an effect and its `Prepared` row land in
//! one SQLite transaction — head CAS and typed rows through `ledger_store`,
//! in the same `BEGIN IMMEDIATE` as the operation row — and the projection
//! that settles it lands with `Applied`, `NotStarted`, or `Unknown` in
//! another. There is no serialized ledger anywhere in between: the blob and
//! its 16 MiB wall are gone, and the ledger IS the rows. Until that actor is
//! wired, these tables are intentionally unused; dual-writing the current
//! JSON ledger and this journal would create the very split authority this
//! is meant to remove. The import API accepts an already-validated
//! projection whose identity is the digest of the person's file; this module
//! neither reads legacy JSON nor wires a production caller in this stage.

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use zerocode_core::orchestration::{LedgerProjectionV1, PROJECTION_SCHEMA};

use crate::authority_lock::{OwnerLock, OwnerLockError, stable_path_bytes};
use crate::workflow_store::WorkflowStore;

const DIGEST_HEX_BYTES: usize = 64;

/// Which externally visible host action an operation reserves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEffectKind {
    Split,
    Release,
    Close,
    Respawn,
}

impl HostEffectKind {
    const fn as_db(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::Release => "release",
            Self::Close => "close",
            Self::Respawn => "respawn",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "split" => Some(Self::Split),
            "release" => Some(Self::Release),
            "close" => Some(Self::Close),
            "respawn" => Some(Self::Respawn),
            _ => None,
        }
    }
}

/// Durable state of one store-minted host operation id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEffectState {
    Prepared,
    Applied,
    NotStarted,
    Unknown,
}

impl HostEffectState {
    const fn as_db(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Applied => "applied",
            Self::NotStarted => "not_started",
            Self::Unknown => "unknown",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "prepared" => Some(Self::Prepared),
            "applied" => Some(Self::Applied),
            "not_started" => Some(Self::NotStarted),
            "unknown" => Some(Self::Unknown),
            _ => None,
        }
    }
}

/// Bounded failure categories. Provider prose, paths, and panic payloads never
/// cross the durable boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEffectFailure {
    Refused,
    Timeout,
    Disconnected,
    Unavailable,
    Panicked,
}

impl HostEffectFailure {
    const fn as_db(self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::Timeout => "timeout",
            Self::Disconnected => "disconnected",
            Self::Unavailable => "unavailable",
            Self::Panicked => "panicked",
        }
    }

    fn from_db(value: &str) -> Option<Self> {
        match value {
            "refused" => Some(Self::Refused),
            "timeout" => Some(Self::Timeout),
            "disconnected" => Some(Self::Disconnected),
            "unavailable" => Some(Self::Unavailable),
            "panicked" => Some(Self::Panicked),
            _ => None,
        }
    }
}

/// A ledger as the journal found it: the typed rows, which revision they
/// were at, and — when it was imported from legacy JSON — the digest the
/// import stamped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedLedger {
    pub revision: u64,
    pub projection: LedgerProjectionV1,
    pub legacy_digest: Option<String>,
}

/// Digested request and live-host binding for a new attempt.
///
/// Every string except `ledger_id` must be lowercase SHA-256 hex. Raw actor or
/// session ids, argv, prompts, paths, capabilities, and commands have no field
/// they can enter through.
pub struct EffectRequest {
    ledger_id: String,
    request_slot: String,
    request_fingerprint: String,
    binding_digest: String,
    host_epoch: String,
    kind: HostEffectKind,
}

impl std::fmt::Debug for EffectRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EffectRequest")
            .field("ledger_id", &"<opaque-ledger-id>")
            .field("request", &"<opaque-request-digests>")
            .field("binding", &"<opaque-host-binding>")
            .field("kind", &self.kind)
            .finish()
    }
}

impl EffectRequest {
    pub fn new(
        ledger_id: impl Into<String>,
        request_slot: impl Into<String>,
        request_fingerprint: impl Into<String>,
        binding_digest: impl Into<String>,
        host_epoch: impl Into<String>,
        kind: HostEffectKind,
    ) -> Result<Self, EffectJournalError> {
        let request = Self {
            ledger_id: ledger_id.into(),
            request_slot: request_slot.into(),
            request_fingerprint: request_fingerprint.into(),
            binding_digest: binding_digest.into(),
            host_epoch: host_epoch.into(),
            kind,
        };
        if !valid_ledger_id(&request.ledger_id) {
            return Err(invalid("ledger_id"));
        }
        for (field, value) in [
            ("request_slot", request.request_slot.as_str()),
            ("request_fingerprint", request.request_fingerprint.as_str()),
            ("binding_digest", request.binding_digest.as_str()),
            ("host_epoch", request.host_epoch.as_str()),
        ] {
            if !valid_digest(value) {
                return Err(invalid(field));
            }
        }
        Ok(request)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum EffectJournalError {
    #[error("the effect authority is unavailable")]
    Database,
    #[error("the effect authority contains an invalid row")]
    Corrupt,
    #[error("{field} is not a valid effect-authority value")]
    InvalidInput { field: &'static str },
    #[error("the orchestration ledger revision changed")]
    RevisionMismatch,
    #[error("the legacy ledger was already imported with different bytes or identity")]
    ImportConflict,
    #[error("the fresh orchestration ledger already has different state")]
    InitializationConflict,
    #[error("a prepared or unknown host effect blocks a non-host ledger mutation")]
    EffectInFlight,
    #[error("the orchestration ledger timestamp would move backwards")]
    TimestampRegression,
    #[error("a retry name was reused for a different request or effect")]
    RequestConflict,
    #[error("the host effect is no longer in the state this permit names")]
    InvalidTransition,
    #[error("the host effect changed while its transition was being committed")]
    ConcurrentTransition,
    #[error("another runtime process owns this orchestration ledger")]
    OwnerBusy,
    #[error("this platform cannot provide a durable runtime owner lock")]
    OwnerLockUnsupported,
    #[error("the runtime authority lease is stale or belongs to another store")]
    StaleAuthority,
    #[error("unresolved host effects prevent clean authority release")]
    ReleaseInFlight,
}

/// Opaque, process-bound authority for one ledger epoch.
pub struct AuthorityLease {
    ledger_id: String,
    authority_epoch: u64,
    token: [u8; 32],
    token_digest: String,
    store_digest: String,
    lock: Option<OwnerLock>,
}

impl std::fmt::Debug for AuthorityLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorityLease")
            .field("ledger", &"<opaque-ledger>")
            .field("authority_epoch", &self.authority_epoch)
            .field("token", &"<redacted>")
            .field("active", &self.lock.is_some())
            .finish()
    }
}

impl AuthorityLease {
    #[must_use]
    pub const fn authority_epoch(&self) -> u64 {
        self.authority_epoch
    }
}

impl Drop for AuthorityLease {
    fn drop(&mut self) {
        self.token.fill(0);
    }
}

/// Result of the one-time JSON-ledger cutover boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportResult {
    Imported,
    AlreadyImported,
}

/// Result of initializing an orchestration ledger that has no legacy source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshLedgerResult {
    Initialized,
    AlreadyInitialized,
}

/// Store access sharing the exact private SQLite file used by workflow and test
/// evidence authority.
#[derive(Clone)]
pub struct EffectJournal {
    store: WorkflowStore,
}

impl std::fmt::Debug for EffectJournal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EffectJournal(<private-authority-store>)")
    }
}

impl EffectJournal {
    #[must_use]
    pub fn new(store: &WorkflowStore) -> Self {
        Self {
            store: store.clone(),
        }
    }

    pub fn claim_authority(
        &self,
        ledger_id: &str,
        owner_instance_digest: &str,
        now_ms: i64,
    ) -> Result<AuthorityLease, EffectJournalError> {
        if !valid_ledger_id(ledger_id) {
            return Err(invalid("ledger_id"));
        }
        if !valid_digest(owner_instance_digest) {
            return Err(invalid("owner_instance_digest"));
        }
        if now_ms < 0 {
            return Err(invalid("now_ms"));
        }
        let lock = OwnerLock::try_acquire(self.store.private_path(), ledger_id)
            .map_err(owner_lock_error)?;
        let mut token = [0_u8; 32];
        rand::fill(&mut token);
        let token_digest = secret_digest(&token);
        let store_digest = store_digest(self.store.private_path());
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        let prior: Option<i64> = transaction
            .query_row(
                "SELECT authority_epoch FROM orchestration_authorities WHERE ledger_id = ?1",
                [ledger_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| EffectJournalError::Database)?;
        let authority_epoch = prior
            .map(from_sql_u64)
            .transpose()?
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(EffectJournalError::Corrupt)?;
        transaction
            .execute(
                "INSERT INTO orchestration_authorities (
                    ledger_id, authority_epoch, owner_state, token_digest,
                    owner_instance_digest, owner_revision, claimed_at_ms, updated_at_ms,
                    released_at_ms
                 ) VALUES (?1, ?2, 'claimed', ?3, ?4, 0, ?5, ?5, NULL)
                 ON CONFLICT(ledger_id) DO UPDATE SET
                    authority_epoch = excluded.authority_epoch,
                    owner_state = 'claimed', token_digest = excluded.token_digest,
                    owner_instance_digest = excluded.owner_instance_digest,
                    owner_revision = orchestration_authorities.owner_revision + 1,
                    claimed_at_ms = excluded.claimed_at_ms,
                    updated_at_ms = excluded.updated_at_ms, released_at_ms = NULL",
                params![
                    ledger_id,
                    to_sql_u64(authority_epoch)?,
                    token_digest,
                    owner_instance_digest,
                    now_ms,
                ],
            )
            .map_err(|_| EffectJournalError::Database)?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(AuthorityLease {
            ledger_id: ledger_id.to_string(),
            authority_epoch,
            token,
            token_digest,
            store_digest,
            lock: Some(lock),
        })
    }

    pub fn relinquish_authority(
        &self,
        lease: &mut AuthorityLease,
        now_ms: i64,
    ) -> Result<(), EffectJournalError> {
        if now_ms < 0 {
            return Err(invalid("now_ms"));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        if has_in_flight_effect(&transaction, &lease.ledger_id)? {
            return Err(EffectJournalError::ReleaseInFlight);
        }
        let changed = transaction
            .execute(
                "UPDATE orchestration_authorities
                    SET owner_state = 'released', token_digest = NULL,
                        owner_instance_digest = NULL, released_at_ms = ?1,
                        updated_at_ms = ?1, owner_revision = owner_revision + 1
                  WHERE ledger_id = ?2 AND authority_epoch = ?3
                    AND token_digest = ?4 AND owner_state = 'claimed'",
                params![
                    now_ms,
                    lease.ledger_id,
                    to_sql_u64(lease.authority_epoch)?,
                    lease.token_digest,
                ],
            )
            .map_err(|_| EffectJournalError::Database)?;
        require_one(changed)?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        let lock = lease
            .lock
            .take()
            .ok_or(EffectJournalError::StaleAuthority)?;
        lock.release().map_err(owner_lock_error)
    }

    pub fn load_ledger(
        &self,
        lease: &AuthorityLease,
    ) -> Result<Option<LoadedLedger>, EffectJournalError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        let held = crate::ledger_store::read(&transaction, &lease.ledger_id, PROJECTION_SCHEMA)?;
        let loaded = match held {
            None => None,
            Some(held) => Some(LoadedLedger {
                revision: held.revision,
                legacy_digest: crate::ledger_store::legacy_digest(&transaction, &lease.ledger_id)?,
                projection: held.projection,
            }),
        };
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(loaded)
    }

    /// Import one already-validated legacy ledger exactly once.
    ///
    /// The identity of an import is ONE thing: the digest of the bytes of the
    /// person's file. The projection plays no part in the question "did this
    /// file already come in" — so the two ways the sources could fork each
    /// have a single answer:
    ///
    /// - Same digest, different projection: `AlreadyImported`, and the STORED
    ///   interpretation stays. The file did not change — our reading of it
    ///   did — and a durable store takes the disk's word over a newer parser.
    /// - Different digest, same projection: `ImportConflict`. "Did I already
    ///   import this file" is honestly no; a reformatted file surfacing to
    ///   the operator beats a silent second identity for one ledger.
    pub fn import_legacy(
        &self,
        lease: &AuthorityLease,
        initial: &LedgerProjectionV1,
        legacy_digest: &str,
        now_ms: i64,
    ) -> Result<ImportResult, EffectJournalError> {
        if !valid_digest(legacy_digest) {
            return Err(invalid("legacy_digest"));
        }
        if now_ms < 0 {
            return Err(invalid("now_ms"));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        /* The head alone, not the rows: this branch only asks "is my file
         * already in", and the digest answers that by itself. The revision
         * deliberately plays no part — a ledger imported from this file and
         * ADVANCED since is still this file's ledger, and two windows racing
         * a shared store would otherwise have the loser arrive after the
         * winner's first verb and be told, wrongly, that its own file
         * conflicts. Walking every row back for a question one column
         * answers would also price this check at the size of the ledger. */
        if crate::ledger_store::head_revision(&transaction, &lease.ledger_id)?.is_some() {
            let stamped = crate::ledger_store::legacy_digest(&transaction, &lease.ledger_id)?;
            if stamped.as_deref() != Some(legacy_digest) {
                return Err(EffectJournalError::ImportConflict);
            }
            transaction
                .commit()
                .map_err(|_| EffectJournalError::Database)?;
            return Ok(ImportResult::AlreadyImported);
        }
        crate::ledger_store::write(&transaction, &lease.ledger_id, 0, 0, initial, now_ms)?;
        crate::ledger_store::stamp_legacy_digest(&transaction, &lease.ledger_id, legacy_digest)?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(ImportResult::Imported)
    }

    /// Initialize a new ledger that has no legacy JSON source.
    ///
    /// The first image is revision zero and carries no legacy digest. An exact
    /// retry is read-only and idempotent, but a different image or any existing
    /// host-effect row proves this is not a fresh boot and fails closed.
    pub fn initialize_fresh(
        &self,
        lease: &AuthorityLease,
        initial: &LedgerProjectionV1,
        now_ms: i64,
    ) -> Result<FreshLedgerResult, EffectJournalError> {
        if now_ms < 0 {
            return Err(invalid("now_ms"));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        let has_effect = has_any_effect(&transaction, &lease.ledger_id)?;
        if let Some(existing) =
            crate::ledger_store::read(&transaction, &lease.ledger_id, PROJECTION_SCHEMA)?
        {
            let stamped = crate::ledger_store::legacy_digest(&transaction, &lease.ledger_id)?;
            if has_effect
                || existing.revision != 0
                || stamped.is_some()
                || existing.projection != *initial
            {
                return Err(EffectJournalError::InitializationConflict);
            }
            transaction
                .commit()
                .map_err(|_| EffectJournalError::Database)?;
            return Ok(FreshLedgerResult::AlreadyInitialized);
        }
        if has_effect {
            return Err(EffectJournalError::InitializationConflict);
        }
        crate::ledger_store::write(&transaction, &lease.ledger_id, 0, 0, initial, now_ms)?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(FreshLedgerResult::Initialized)
    }

    /// Advance a ledger mutation that performs no host effect.
    ///
    /// Host-effect reservations must use [`Self::prepare`]. This path refuses
    /// while any prepared or ambiguous effect exists for the same ledger, so a
    /// plain mutation cannot race ahead of an effect that still needs provider
    /// observation.
    pub fn advance_ledger(
        &self,
        lease: &AuthorityLease,
        expected_revision: u64,
        next: &LedgerProjectionV1,
        now_ms: i64,
    ) -> Result<(), EffectJournalError> {
        if now_ms < 0 {
            return Err(invalid("now_ms"));
        }
        let next_revision = expected_revision
            .checked_add(1)
            .ok_or(EffectJournalError::RevisionMismatch)?;
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        if crate::ledger_store::head_revision(&transaction, &lease.ledger_id)?.is_none() {
            return Err(EffectJournalError::RevisionMismatch);
        }
        if has_in_flight_effect(&transaction, &lease.ledger_id)? {
            return Err(EffectJournalError::EffectInFlight);
        }
        crate::ledger_store::write(
            &transaction,
            &lease.ledger_id,
            expected_revision,
            next_revision,
            next,
            now_ms,
        )?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(())
    }

    /// Atomically reserve a fresh attempt and publish the ledger image that
    /// names it. Existing `Prepared`/`Unknown` attempts are reconciled, an
    /// `Applied` one is replayed, and `NotStarted` can advance to a new attempt
    /// only after its durable tombstone and explicit backoff allow it.
    pub fn prepare(
        &self,
        lease: &AuthorityLease,
        expected_revision: u64,
        prepared: &LedgerProjectionV1,
        request: &EffectRequest,
        now_ms: i64,
    ) -> Result<BeginEffect, EffectJournalError> {
        if now_ms < 0 {
            return Err(invalid("now_ms"));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        if request.ledger_id != lease.ledger_id {
            return Err(EffectJournalError::StaleAuthority);
        }
        let standing = latest_operation(&transaction, &request.ledger_id, &request.request_slot)?;

        let attempt = match standing {
            Some(row) => {
                if row.request_fingerprint != request.request_fingerprint
                    || row.kind != request.kind
                {
                    return Err(EffectJournalError::RequestConflict);
                }
                match row.state {
                    HostEffectState::Prepared | HostEffectState::Unknown => {
                        transaction
                            .commit()
                            .map_err(|_| EffectJournalError::Database)?;
                        return Ok(BeginEffect::Reconcile(EffectPermit::from(row, lease)));
                    }
                    HostEffectState::Applied => {
                        let replay = AppliedEffect::from_row(&row)?;
                        transaction
                            .commit()
                            .map_err(|_| EffectJournalError::Database)?;
                        return Ok(BeginEffect::Replay(replay));
                    }
                    HostEffectState::NotStarted => {
                        let refused = NotStartedEffect::from_row(&row)?;
                        let due = row.retryable
                            && row.tombstone_digest.is_some()
                            && row.retry_not_before_ms.is_some_and(|at| at <= now_ms);
                        if !due {
                            transaction
                                .commit()
                                .map_err(|_| EffectJournalError::Database)?;
                            return Ok(BeginEffect::Refused(refused));
                        }
                        row.attempt
                            .checked_add(1)
                            .ok_or(EffectJournalError::Corrupt)?
                    }
                }
            }
            None => 1,
        };

        // The request-slot lookup above owns idempotence for the standing
        // operation. Once it finds no such answer, a different slot may not
        // reserve another host effect for this ledger. BEGIN IMMEDIATE makes
        // this deterministic for API callers; the partial unique index remains
        // the structural backstop for every SQLite writer.
        if has_in_flight_effect(&transaction, &request.ledger_id)? {
            return Err(EffectJournalError::EffectInFlight);
        }

        let prepared_revision = expected_revision
            .checked_add(1)
            .ok_or(EffectJournalError::RevisionMismatch)?;
        crate::ledger_store::write(
            &transaction,
            &request.ledger_id,
            expected_revision,
            prepared_revision,
            prepared,
            now_ms,
        )?;
        let operation_id = operation_id(
            &request.ledger_id,
            &request.request_slot,
            &request.request_fingerprint,
            request.kind,
            attempt,
        );
        transaction
            .execute(
                "INSERT INTO host_effect_operations (
                    ledger_id, request_slot, request_fingerprint, attempt, operation_id,
                    effect_kind, effect_state, binding_digest, host_epoch,
                    operation_authority_epoch,
                    prepared_revision, prepared_at_ms, updated_at_ms
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'prepared', ?7, ?8, ?9, ?10, ?11, ?11)",
                params![
                    request.ledger_id,
                    request.request_slot,
                    request.request_fingerprint,
                    to_sql_u64(attempt)?,
                    operation_id,
                    request.kind.as_db(),
                    request.binding_digest,
                    request.host_epoch,
                    to_sql_u64(lease.authority_epoch)?,
                    to_sql_u64(prepared_revision)?,
                    now_ms,
                ],
            )
            .map_err(|_| EffectJournalError::Database)?;
        let row = operation_by_id(&transaction, &operation_id)?
            .ok_or(EffectJournalError::ConcurrentTransition)?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(BeginEffect::Execute(EffectPermit::from(row, lease)))
    }

    /// Make an ambiguous provider call durable with the ledger state that says
    /// it is ambiguous. This consumes the one permit for the prepared row.
    ///
    /// On refusal the permit comes BACK beside the error — the same shape
    /// [`SplitLease::finish`] uses one layer up, and for the same reason: a
    /// door that swallows a permit while refusing leaves an operation that
    /// nothing in this window can move again. It matters most for the
    /// refusals that happen BEFORE the row is touched — a wrong state, an
    /// unavailable connection, a lease that has moved — because the permit
    /// handed back there is still exactly as good as the one handed in.
    ///
    /// [`SplitLease::finish`]: ../../zerocode_shell/durable_split/struct.SplitLease.html
    pub fn mark_unknown(
        &self,
        lease: &AuthorityLease,
        mut permit: EffectPermit,
        expected_revision: u64,
        unknown: &LedgerProjectionV1,
        failure: HostEffectFailure,
        at_ms: i64,
    ) -> Result<EffectPermit, Box<(EffectPermit, EffectJournalError)>> {
        match self.mark_unknown_held(lease, &permit, expected_revision, unknown, failure, at_ms) {
            Ok(next) => {
                permit.spend();
                Ok(next)
            }
            Err(why) => Err(Box::new((permit, why))),
        }
    }

    fn mark_unknown_held(
        &self,
        lease: &AuthorityLease,
        permit: &EffectPermit,
        expected_revision: u64,
        unknown: &LedgerProjectionV1,
        failure: HostEffectFailure,
        at_ms: i64,
    ) -> Result<EffectPermit, EffectJournalError> {
        if permit.state != HostEffectState::Prepared || at_ms < 0 {
            return Err(EffectJournalError::InvalidTransition);
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        require_permit(&transaction, lease, permit)?;
        crate::ledger_store::write(
            &transaction,
            &permit.ledger_id,
            expected_revision,
            expected_revision
                .checked_add(1)
                .ok_or(EffectJournalError::RevisionMismatch)?,
            unknown,
            at_ms,
        )?;
        let changed = transaction
            .execute(
                "UPDATE host_effect_operations
                    SET effect_state = 'unknown', failure_kind = ?1,
                        updated_at_ms = ?2, row_revision = row_revision + 1
                  WHERE operation_id = ?3 AND ledger_id = ?4
                    AND effect_state = 'prepared' AND row_revision = ?5",
                params![
                    failure.as_db(),
                    at_ms,
                    permit.operation_id,
                    permit.ledger_id,
                    to_sql_u64(permit.row_revision)?,
                ],
            )
            .map_err(|_| EffectJournalError::Database)?;
        require_one(changed)?;
        let row = operation_by_id(&transaction, &permit.operation_id)?
            .ok_or(EffectJournalError::ConcurrentTransition)?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(EffectPermit::from(row, lease))
    }

    /// Atomically settle a prepared or unknown operation with the ledger image
    /// that records the same answer.
    ///
    /// On refusal the permit comes BACK beside the error — the same shape
    /// [`SplitLease::finish`] uses one layer up, and for the same reason: a
    /// door that swallows a permit while refusing leaves an operation that
    /// nothing in this window can move again. It matters most for the
    /// refusals that happen BEFORE the row is touched — a wrong state, an
    /// unavailable connection, a lease that has moved — because the permit
    /// handed back there is still exactly as good as the one handed in.
    ///
    /// [`SplitLease::finish`]: ../../zerocode_shell/durable_split/struct.SplitLease.html
    pub fn settle(
        &self,
        lease: &AuthorityLease,
        mut permit: EffectPermit,
        expected_revision: u64,
        settled: &LedgerProjectionV1,
        outcome: EffectSettlement,
    ) -> Result<SettledEffect, Box<(EffectPermit, EffectJournalError)>> {
        match self.settle_held(lease, &permit, expected_revision, settled, outcome) {
            Ok(answer) => {
                permit.spend();
                Ok(answer)
            }
            Err(why) => Err(Box::new((permit, why))),
        }
    }

    fn settle_held(
        &self,
        lease: &AuthorityLease,
        permit: &EffectPermit,
        expected_revision: u64,
        settled: &LedgerProjectionV1,
        outcome: EffectSettlement,
    ) -> Result<SettledEffect, EffectJournalError> {
        if !matches!(
            permit.state,
            HostEffectState::Prepared | HostEffectState::Unknown
        ) {
            return Err(EffectJournalError::InvalidTransition);
        }
        outcome.validate()?;
        let at_ms = outcome.settled_at_ms();
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        require_permit(&transaction, lease, permit)?;
        crate::ledger_store::write(
            &transaction,
            &permit.ledger_id,
            expected_revision,
            expected_revision
                .checked_add(1)
                .ok_or(EffectJournalError::RevisionMismatch)?,
            settled,
            at_ms,
        )?;

        let changed = match &outcome {
            EffectSettlement::Applied {
                result_digest,
                settled_at_ms,
            } => transaction.execute(
                "UPDATE host_effect_operations
                    SET effect_state = 'applied', result_digest = ?1,
                        failure_kind = NULL, tombstone_digest = NULL,
                        retryable = 0, retry_not_before_ms = NULL,
                        updated_at_ms = ?2, settled_at_ms = ?2,
                        row_revision = row_revision + 1
                  WHERE operation_id = ?3 AND ledger_id = ?4
                    AND effect_state = ?5 AND row_revision = ?6",
                params![
                    result_digest,
                    settled_at_ms,
                    permit.operation_id,
                    permit.ledger_id,
                    permit.state.as_db(),
                    to_sql_u64(permit.row_revision)?,
                ],
            ),
            EffectSettlement::NotStarted {
                failure,
                tombstone_digest,
                retryable,
                retry_not_before_ms,
                settled_at_ms,
            } => transaction.execute(
                "UPDATE host_effect_operations
                    SET effect_state = 'not_started', result_digest = NULL,
                        failure_kind = ?1, tombstone_digest = ?2,
                        retryable = ?3, retry_not_before_ms = ?4,
                        updated_at_ms = ?5, settled_at_ms = ?5,
                        row_revision = row_revision + 1
                  WHERE operation_id = ?6 AND ledger_id = ?7
                    AND effect_state = ?8 AND row_revision = ?9",
                params![
                    failure.as_db(),
                    tombstone_digest,
                    if *retryable { 1_i64 } else { 0_i64 },
                    retry_not_before_ms,
                    settled_at_ms,
                    permit.operation_id,
                    permit.ledger_id,
                    permit.state.as_db(),
                    to_sql_u64(permit.row_revision)?,
                ],
            ),
        }
        .map_err(|_| EffectJournalError::Database)?;
        require_one(changed)?;
        let row = operation_by_id(&transaction, &permit.operation_id)?
            .ok_or(EffectJournalError::ConcurrentTransition)?;
        let answer = match row.state {
            HostEffectState::Applied => SettledEffect::Applied(AppliedEffect::from_row(&row)?),
            HostEffectState::NotStarted => {
                SettledEffect::NotStarted(NotStartedEffect::from_row(&row)?)
            }
            _ => return Err(EffectJournalError::Corrupt),
        };
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        Ok(answer)
    }

    /// One durable stride inside a host effect that takes more than one.
    ///
    /// A split is one host action, but a lifecycle effect is several — a
    /// respawn cuts the new pane, settles the seat, re-points it, and only
    /// then ends the old shell. A crash between any two of those leaves a
    /// window the ledger no longer describes, and the permit row alone
    /// cannot say how far the host got. Each completed stride is appended
    /// here, and recovery reads them back to ask the one question it has:
    /// what already happened?
    ///
    /// The journal checks the SHAPE of a stride's name and its place in
    /// line. The vocabulary belongs to the lane that performs the effect —
    /// the same division that keeps enum spellings out of the schema.
    ///
    /// `ordinal` must be exactly the number of strides already taken: a
    /// checkpoint is a sequence, and a gap or a repeat in one is not a
    /// record of what happened, it is two records disagreeing.
    pub fn step(
        &self,
        lease: &AuthorityLease,
        permit: &EffectPermit,
        ordinal: u32,
        stride: &str,
        now_ms: i64,
    ) -> Result<(), EffectJournalError> {
        if now_ms < 0 {
            return Err(invalid("now_ms"));
        }
        if !valid_stride(stride) {
            return Err(invalid("stride"));
        }
        /* The vocabulary belongs to the lane, but the COUNT is a shape this
         * door can know — the same reason the actor door counts command
         * words. A lane gone wrong must not be able to grow one operation
         * without bound. */
        if ordinal >= MAX_STRIDES {
            return Err(invalid("ordinal"));
        }
        if !matches!(
            permit.state,
            HostEffectState::Prepared | HostEffectState::Unknown
        ) {
            return Err(EffectJournalError::InvalidTransition);
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        require_permit(&transaction, lease, permit)?;
        let taken: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM host_effect_steps WHERE operation_id = ?1",
                [&permit.operation_id],
                |row| row.get(0),
            )
            .map_err(|_| EffectJournalError::Database)?;
        if i64::from(ordinal) != taken {
            return Err(invalid("ordinal"));
        }
        let changed = transaction
            .execute(
                "INSERT INTO host_effect_steps (operation_id, ordinal, step, at_ms)
                 VALUES (?1, ?2, ?3, ?4)",
                params![&permit.operation_id, ordinal, stride, now_ms],
            )
            .map_err(|_| EffectJournalError::Database)?;
        require_one(changed)?;
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)
    }

    /// Every stride one operation has durably taken, in the order it took
    /// them. Empty for an operation that crashed before its first.
    pub fn strides_taken(
        &self,
        lease: &AuthorityLease,
        operation_id: &str,
    ) -> Result<Vec<StrideTaken>, EffectJournalError> {
        if !valid_digest(operation_id) {
            return Err(invalid("operation_id"));
        }
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        /* The `ORDER BY` cannot be caught failing today: the primary key is
         * (operation_id, ordinal), so the index already returns this order
         * and a mutation that removes the clause survives every test. It
         * stays because the CONTRACT is the order, not the planner's current
         * habit — the same reason ledger_store keeps its child `ORDER BY`. */
        let mut statement = transaction
            .prepare(
                "SELECT ordinal, step, at_ms FROM host_effect_steps
                  WHERE operation_id = ?1 ORDER BY ordinal",
            )
            .map_err(|_| EffectJournalError::Database)?;
        let rows = statement
            .query_map([operation_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|_| EffectJournalError::Database)?;
        let mut taken = Vec::new();
        for row in rows {
            let (ordinal, stride, at_ms) = row.map_err(|_| EffectJournalError::Database)?;
            let ordinal = u32::try_from(ordinal).map_err(|_| EffectJournalError::Corrupt)?;
            if !valid_stride(&stride) || at_ms < 0 {
                return Err(EffectJournalError::Corrupt);
            }
            taken.push(StrideTaken {
                ordinal,
                stride,
                at_ms,
            });
        }
        Ok(taken)
    }

    /// Prepared and ambiguous operations that must be observed before any new
    /// attempt carrying their request slot may execute, as PERMITS — one
    /// spendable capability each.
    ///
    /// Ask this only where the permits will be spent. A caller that merely
    /// wants to KNOW what is open wants [`Self::open_operations`]: minting a
    /// capability to read a field is how a one-use permit quietly becomes two
    /// live permits for one row.
    pub fn recoverable(
        &self,
        lease: &AuthorityLease,
    ) -> Result<Vec<EffectPermit>, EffectJournalError> {
        Ok(self
            .open_rows(lease)?
            .into_iter()
            .map(|row| EffectPermit::from(row, lease))
            .collect())
    }

    /// The same operations, DESCRIBED rather than granted.
    ///
    /// `verify_authority` runs before every request and only ever compares
    /// fields; before this door existed it asked [`Self::recoverable`] and
    /// threw a full permit away per open row, per request. The permit's own
    /// doc says one-use, and this is the door that makes that true.
    pub fn open_operations(
        &self,
        lease: &AuthorityLease,
    ) -> Result<Vec<OpenOperation>, EffectJournalError> {
        Ok(self
            .open_rows(lease)?
            .iter()
            .map(OpenOperation::from_row)
            .collect())
    }

    fn open_rows(&self, lease: &AuthorityLease) -> Result<Vec<OperationRow>, EffectJournalError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| EffectJournalError::Database)?;
        require_authority(&transaction, self, lease)?;
        let mut statement = transaction
            .prepare(
                "SELECT ledger_id, request_slot, request_fingerprint, attempt,
                        operation_id, effect_kind, effect_state, binding_digest,
                        host_epoch, operation_authority_epoch, prepared_revision,
                        result_digest, failure_kind,
                        tombstone_digest, retryable, retry_not_before_ms, row_revision,
                        prepared_at_ms, updated_at_ms, settled_at_ms
                   FROM host_effect_operations
                  WHERE ledger_id = ?1 AND effect_state IN ('prepared', 'unknown')
                  ORDER BY prepared_at_ms, attempt",
            )
            .map_err(|_| EffectJournalError::Database)?;
        let rows = statement
            .query_map([&lease.ledger_id], raw_operation)
            .map_err(|_| EffectJournalError::Database)?;
        let open: Result<Vec<OperationRow>, EffectJournalError> = rows
            .map(|row| {
                row.map_err(|_| EffectJournalError::Database)
                    .and_then(OperationRow::try_from)
            })
            .collect();
        drop(statement);
        transaction
            .commit()
            .map_err(|_| EffectJournalError::Database)?;
        open
    }

    fn connection(&self) -> Result<Connection, EffectJournalError> {
        self.store
            .connection()
            .map_err(|_| EffectJournalError::Database)
    }
}

/// Outcome of consulting a retry slot.
#[derive(Debug)]
pub enum BeginEffect {
    Execute(EffectPermit),
    Reconcile(EffectPermit),
    Replay(AppliedEffect),
    Refused(NotStartedEffect),
}

/// What is open, without the right to move it.
///
/// The six fields an owner compares to ask "has anything but me touched this
/// ledger" — the same six [`WalkMark`] carries on the actor side. Reading is
/// not spending, so this type is `Clone` and the permit is not.
///
/// [`WalkMark`]: crate::runtime_actor
#[derive(Clone, PartialEq, Eq)]
pub struct OpenOperation {
    operation_id: String,
    kind: HostEffectKind,
    state: HostEffectState,
    attempt: u64,
    operation_authority_epoch: u64,
    prepared_revision: u64,
}

impl std::fmt::Debug for OpenOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        /* The same redaction the permit uses: an operation id is a name
         * somebody chose, and it is opaque in one row or in none. */
        formatter
            .debug_struct("OpenOperation")
            .field("operation_id", &"<opaque-operation-id>")
            .field("attempt", &self.attempt)
            .field("kind", &self.kind)
            .field("state", &self.state)
            .finish()
    }
}

impl OpenOperation {
    fn from_row(row: &OperationRow) -> Self {
        Self {
            operation_id: row.operation_id.clone(),
            kind: row.kind,
            state: row.state,
            attempt: row.attempt,
            operation_authority_epoch: row.operation_authority_epoch,
            prepared_revision: row.prepared_revision,
        }
    }

    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub const fn kind(&self) -> HostEffectKind {
        self.kind
    }

    #[must_use]
    pub const fn state(&self) -> HostEffectState {
        self.state
    }

    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    #[must_use]
    pub const fn operation_authority_epoch(&self) -> u64 {
        self.operation_authority_epoch
    }

    #[must_use]
    pub const fn prepared_revision(&self) -> u64 {
        self.prepared_revision
    }
}

/// A capability to move one row, at one revision, once.
///
/// It implements neither `Clone` nor serialization, and since the read path
/// moved to [`OpenOperation`] the journal mints one only where it is meant to
/// be spent. Neither of those makes "once" true by itself: two permits for a
/// single row are still reachable — a boot holds what it recovered while a
/// caller reopens the same ledger — and what refuses the later one is
/// `require_permit`, which compares the WHOLE row the permit remembers
/// against the row as it now is. `a_row_that_bore_two_permits_moves_for_only_the_first`
/// asks that directly, because until it was asked, "one-use" was a wish this
/// type had no way to keep.
///
/// Which of those comparisons does the refusing was measured rather than
/// assumed: today it is the effect state, because every transition that
/// moves the row moves its state and its revision in one statement. See the
/// note on `require_permit`.
pub struct EffectPermit {
    ledger_id: String,
    operation_id: String,
    attempt: u64,
    kind: HostEffectKind,
    state: HostEffectState,
    binding_digest: String,
    host_epoch: String,
    operation_authority_epoch: u64,
    issued_authority_epoch: u64,
    issued_token_digest: String,
    row_revision: u64,
    prepared_revision: u64,
    /* Armed at issue; disarmed by the one door that actually spends it.
     * A permit that reaches `Drop` still armed is a fence nothing in this
     * window will ever lift — the rule has lived in comments since the
     * split lane was written, and comments do not go red. */
    armed: bool,
}

impl std::fmt::Debug for EffectPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EffectPermit")
            .field("operation_id", &"<opaque-operation-id>")
            .field("attempt", &self.attempt)
            .field("kind", &self.kind)
            .field("state", &self.state)
            .field("binding", &"<opaque-host-binding>")
            .finish()
    }
}

impl EffectPermit {
    fn from(row: OperationRow, lease: &AuthorityLease) -> Self {
        Self {
            ledger_id: row.ledger_id,
            operation_id: row.operation_id,
            attempt: row.attempt,
            kind: row.kind,
            state: row.state,
            binding_digest: row.binding_digest,
            host_epoch: row.host_epoch,
            operation_authority_epoch: row.operation_authority_epoch,
            issued_authority_epoch: lease.authority_epoch,
            issued_token_digest: lease.token_digest.clone(),
            row_revision: row.row_revision,
            prepared_revision: row.prepared_revision,
            armed: true,
        }
    }

    /// The permit was spent: its row moved, durably, in a committed
    /// transaction. Only the two doors that write that transition call this.
    fn spend(&mut self) {
        self.armed = false;
    }

    /// The one honest way to let a permit go unspent.
    ///
    /// Some windows genuinely cannot settle what they hold — the settlement
    /// result itself will not pair with the permit, and nothing further can
    /// be said from here. That case is real, and the next boot's recovery
    /// road is its answer. It has to be SPELLED, though: an abandonment that
    /// looks exactly like a forgotten `?` is the defect this guard exists to
    /// find.
    pub fn abandon(mut self, _why: &'static str) {
        self.spend();
    }

    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    #[must_use]
    pub const fn state(&self) -> HostEffectState {
        self.state
    }

    #[must_use]
    pub const fn kind(&self) -> HostEffectKind {
        self.kind
    }

    #[must_use]
    pub fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    #[must_use]
    pub fn host_epoch(&self) -> &str {
        &self.host_epoch
    }

    #[must_use]
    pub const fn prepared_revision(&self) -> u64 {
        self.prepared_revision
    }

    #[must_use]
    pub const fn operation_authority_epoch(&self) -> u64 {
        self.operation_authority_epoch
    }
}

impl Drop for EffectPermit {
    fn drop(&mut self) {
        /* Debug-only, and never while another panic is already unwinding —
         * a second panic there aborts the process and takes the first
         * panic's message with it. In release this whole body compiles
         * away, so the guard costs a bool nobody reads. */
        if self.armed && !std::thread::panicking() {
            debug_assert!(
                false,
                "a permit was dropped without settling: {self:?} — settle it, \
                 or say `abandon(why)` if this window truly cannot"
            );
        }
    }
}

/// Replayable terminal proof for an applied attempt.
///
/// The exact reply/result lives once, in the ledger rows settled in the same
/// transaction. `result_digest` is its bounded correlation and integrity
/// value rather than a second copy that could drift from those rows. The
/// future actor is responsible for hashing the exact encoded result it
/// places in the ledger; this journal does not interpret the projection to
/// do that on its behalf.
#[derive(PartialEq, Eq)]
pub struct AppliedEffect {
    operation_id: String,
    attempt: u64,
    result_digest: String,
}

impl std::fmt::Debug for AppliedEffect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppliedEffect")
            .field("operation_id", &"<opaque-operation-id>")
            .field("attempt", &self.attempt)
            .field("result", &"<opaque-result-digest>")
            .finish()
    }
}

impl AppliedEffect {
    fn from_row(row: &OperationRow) -> Result<Self, EffectJournalError> {
        if row.state != HostEffectState::Applied {
            return Err(EffectJournalError::Corrupt);
        }
        Ok(Self {
            operation_id: row.operation_id.clone(),
            attempt: row.attempt,
            result_digest: row
                .result_digest
                .clone()
                .ok_or(EffectJournalError::Corrupt)?,
        })
    }

    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    #[must_use]
    pub fn result_digest(&self) -> &str {
        &self.result_digest
    }
}

/// Terminal proof that this provider attempt was fenced before application.
#[derive(PartialEq, Eq)]
pub struct NotStartedEffect {
    operation_id: String,
    attempt: u64,
    failure: HostEffectFailure,
    tombstone_digest: String,
    retryable: bool,
    retry_not_before_ms: Option<i64>,
}

impl std::fmt::Debug for NotStartedEffect {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NotStartedEffect")
            .field("operation_id", &"<opaque-operation-id>")
            .field("attempt", &self.attempt)
            .field("failure", &self.failure)
            .field("tombstone", &"<opaque-tombstone-digest>")
            .field("retryable", &self.retryable)
            .field("retry_not_before_ms", &self.retry_not_before_ms)
            .finish()
    }
}

impl NotStartedEffect {
    fn from_row(row: &OperationRow) -> Result<Self, EffectJournalError> {
        if row.state != HostEffectState::NotStarted || row.tombstone_digest.is_none() {
            return Err(EffectJournalError::Corrupt);
        }
        Ok(Self {
            operation_id: row.operation_id.clone(),
            attempt: row.attempt,
            failure: row.failure.ok_or(EffectJournalError::Corrupt)?,
            tombstone_digest: row
                .tombstone_digest
                .clone()
                .ok_or(EffectJournalError::Corrupt)?,
            retryable: row.retryable,
            retry_not_before_ms: row.retry_not_before_ms,
        })
    }

    #[must_use]
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    #[must_use]
    pub const fn failure(&self) -> HostEffectFailure {
        self.failure
    }

    #[must_use]
    pub fn tombstone_digest(&self) -> &str {
        &self.tombstone_digest
    }

    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    #[must_use]
    pub const fn retry_not_before_ms(&self) -> Option<i64> {
        self.retry_not_before_ms
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SettledEffect {
    Applied(AppliedEffect),
    NotStarted(NotStartedEffect),
}

#[derive(Clone, PartialEq, Eq)]
pub enum EffectSettlement {
    Applied {
        /// Digest of the exact result encoded in the accompanying ledger
        /// snapshot; the result bytes themselves are deliberately not stored
        /// twice.
        result_digest: String,
        settled_at_ms: i64,
    },
    NotStarted {
        failure: HostEffectFailure,
        tombstone_digest: String,
        retryable: bool,
        retry_not_before_ms: Option<i64>,
        settled_at_ms: i64,
    },
}

impl std::fmt::Debug for EffectSettlement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Applied { settled_at_ms, .. } => formatter
                .debug_struct("AppliedSettlement")
                .field("result", &"<opaque-result-digest>")
                .field("settled_at_ms", settled_at_ms)
                .finish(),
            Self::NotStarted {
                failure,
                retryable,
                retry_not_before_ms,
                settled_at_ms,
                ..
            } => formatter
                .debug_struct("NotStartedSettlement")
                .field("failure", failure)
                .field("tombstone", &"<opaque-tombstone-digest>")
                .field("retryable", retryable)
                .field("retry_not_before_ms", retry_not_before_ms)
                .field("settled_at_ms", settled_at_ms)
                .finish(),
        }
    }
}

impl EffectSettlement {
    fn validate(&self) -> Result<(), EffectJournalError> {
        match self {
            Self::Applied {
                result_digest,
                settled_at_ms,
            } => {
                if !valid_digest(result_digest) {
                    return Err(invalid("result_digest"));
                }
                if *settled_at_ms < 0 {
                    return Err(invalid("settled_at_ms"));
                }
            }
            Self::NotStarted {
                tombstone_digest,
                retryable,
                retry_not_before_ms,
                settled_at_ms,
                ..
            } => {
                if !valid_digest(tombstone_digest) {
                    return Err(invalid("tombstone_digest"));
                }
                if *settled_at_ms < 0 {
                    return Err(invalid("settled_at_ms"));
                }
                match (*retryable, *retry_not_before_ms) {
                    (true, Some(at)) if at > *settled_at_ms => {}
                    (false, None) => {}
                    _ => return Err(invalid("retry_not_before_ms")),
                }
            }
        }
        Ok(())
    }

    const fn settled_at_ms(&self) -> i64 {
        match self {
            Self::Applied { settled_at_ms, .. } | Self::NotStarted { settled_at_ms, .. } => {
                *settled_at_ms
            }
        }
    }
}

struct RawOperationRow {
    ledger_id: String,
    request_slot: String,
    request_fingerprint: String,
    attempt: i64,
    operation_id: String,
    effect_kind: String,
    effect_state: String,
    binding_digest: String,
    host_epoch: String,
    operation_authority_epoch: i64,
    prepared_revision: i64,
    result_digest: Option<String>,
    failure_kind: Option<String>,
    tombstone_digest: Option<String>,
    retryable: i64,
    retry_not_before_ms: Option<i64>,
    row_revision: i64,
    prepared_at_ms: i64,
    updated_at_ms: i64,
    settled_at_ms: Option<i64>,
}

struct OperationRow {
    ledger_id: String,
    request_fingerprint: String,
    attempt: u64,
    operation_id: String,
    kind: HostEffectKind,
    state: HostEffectState,
    binding_digest: String,
    host_epoch: String,
    operation_authority_epoch: u64,
    prepared_revision: u64,
    result_digest: Option<String>,
    failure: Option<HostEffectFailure>,
    tombstone_digest: Option<String>,
    retryable: bool,
    retry_not_before_ms: Option<i64>,
    row_revision: u64,
}

impl TryFrom<RawOperationRow> for OperationRow {
    type Error = EffectJournalError;

    fn try_from(raw: RawOperationRow) -> Result<Self, Self::Error> {
        if !valid_ledger_id(&raw.ledger_id)
            || !valid_digest(&raw.request_slot)
            || !valid_digest(&raw.request_fingerprint)
            || !valid_digest(&raw.operation_id)
            || !valid_digest(&raw.binding_digest)
            || !valid_digest(&raw.host_epoch)
            || raw
                .result_digest
                .as_deref()
                .is_some_and(|one| !valid_digest(one))
            || raw
                .tombstone_digest
                .as_deref()
                .is_some_and(|one| !valid_digest(one))
            || raw.prepared_at_ms < 0
            || raw.updated_at_ms < raw.prepared_at_ms
            || raw.settled_at_ms.is_some_and(|at| at < raw.prepared_at_ms)
        {
            return Err(EffectJournalError::Corrupt);
        }
        let attempt = from_sql_u64(raw.attempt)?;
        let prepared_revision = from_sql_u64(raw.prepared_revision)?;
        let operation_authority_epoch = from_sql_u64(raw.operation_authority_epoch)?;
        if attempt == 0 || prepared_revision == 0 {
            return Err(EffectJournalError::Corrupt);
        }
        let kind = HostEffectKind::from_db(&raw.effect_kind).ok_or(EffectJournalError::Corrupt)?;
        let state =
            HostEffectState::from_db(&raw.effect_state).ok_or(EffectJournalError::Corrupt)?;
        if operation_authority_epoch == 0
            && matches!(state, HostEffectState::Prepared | HostEffectState::Unknown)
        {
            return Err(EffectJournalError::Corrupt);
        }
        let failure = match raw.failure_kind.as_deref() {
            Some(value) => {
                Some(HostEffectFailure::from_db(value).ok_or(EffectJournalError::Corrupt)?)
            }
            None => None,
        };
        let retryable = match raw.retryable {
            0 => false,
            1 => true,
            _ => return Err(EffectJournalError::Corrupt),
        };
        let expected_operation = operation_id(
            &raw.ledger_id,
            &raw.request_slot,
            &raw.request_fingerprint,
            kind,
            attempt,
        );
        if raw.operation_id != expected_operation
            || !valid_state_fields(
                state,
                raw.result_digest.as_deref(),
                failure,
                raw.tombstone_digest.as_deref(),
                retryable,
                raw.retry_not_before_ms,
                raw.settled_at_ms,
            )
        {
            return Err(EffectJournalError::Corrupt);
        }
        Ok(Self {
            ledger_id: raw.ledger_id,
            request_fingerprint: raw.request_fingerprint,
            attempt,
            operation_id: raw.operation_id,
            kind,
            state,
            binding_digest: raw.binding_digest,
            host_epoch: raw.host_epoch,
            operation_authority_epoch,
            prepared_revision,
            result_digest: raw.result_digest,
            failure,
            tombstone_digest: raw.tombstone_digest,
            retryable,
            retry_not_before_ms: raw.retry_not_before_ms,
            row_revision: from_sql_u64(raw.row_revision)?,
        })
    }
}

fn valid_state_fields(
    state: HostEffectState,
    result: Option<&str>,
    failure: Option<HostEffectFailure>,
    tombstone: Option<&str>,
    retryable: bool,
    retry_not_before_ms: Option<i64>,
    settled_at_ms: Option<i64>,
) -> bool {
    match state {
        HostEffectState::Prepared => {
            result.is_none()
                && failure.is_none()
                && tombstone.is_none()
                && !retryable
                && retry_not_before_ms.is_none()
                && settled_at_ms.is_none()
        }
        HostEffectState::Unknown => {
            result.is_none()
                && failure.is_some()
                && tombstone.is_none()
                && !retryable
                && retry_not_before_ms.is_none()
                && settled_at_ms.is_none()
        }
        HostEffectState::Applied => {
            result.is_some()
                && failure.is_none()
                && tombstone.is_none()
                && !retryable
                && retry_not_before_ms.is_none()
                && settled_at_ms.is_some()
        }
        HostEffectState::NotStarted => {
            result.is_none()
                && failure.is_some()
                && tombstone.is_some()
                && settled_at_ms.is_some()
                && match (retryable, retry_not_before_ms) {
                    (true, Some(at)) => settled_at_ms.is_some_and(|settled| at > settled),
                    (false, None) => true,
                    _ => false,
                }
        }
    }
}

fn raw_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawOperationRow> {
    Ok(RawOperationRow {
        ledger_id: row.get(0)?,
        request_slot: row.get(1)?,
        request_fingerprint: row.get(2)?,
        attempt: row.get(3)?,
        operation_id: row.get(4)?,
        effect_kind: row.get(5)?,
        effect_state: row.get(6)?,
        binding_digest: row.get(7)?,
        host_epoch: row.get(8)?,
        operation_authority_epoch: row.get(9)?,
        prepared_revision: row.get(10)?,
        result_digest: row.get(11)?,
        failure_kind: row.get(12)?,
        tombstone_digest: row.get(13)?,
        retryable: row.get(14)?,
        retry_not_before_ms: row.get(15)?,
        row_revision: row.get(16)?,
        prepared_at_ms: row.get(17)?,
        updated_at_ms: row.get(18)?,
        settled_at_ms: row.get(19)?,
    })
}

const OPERATION_COLUMNS: &str =
    "ledger_id, request_slot, request_fingerprint, attempt, operation_id,
     effect_kind, effect_state, binding_digest, host_epoch, operation_authority_epoch,
     prepared_revision,
     result_digest, failure_kind, tombstone_digest, retryable,
     retry_not_before_ms, row_revision, prepared_at_ms, updated_at_ms, settled_at_ms";

fn latest_operation(
    connection: &Connection,
    ledger_id: &str,
    request_slot: &str,
) -> Result<Option<OperationRow>, EffectJournalError> {
    connection
        .query_row(
            &format!(
                "SELECT {OPERATION_COLUMNS}
                   FROM host_effect_operations
                  WHERE ledger_id = ?1 AND request_slot = ?2
                  ORDER BY attempt DESC LIMIT 1"
            ),
            params![ledger_id, request_slot],
            raw_operation,
        )
        .optional()
        .map_err(|_| EffectJournalError::Database)?
        .map(OperationRow::try_from)
        .transpose()
}

fn operation_by_id(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<OperationRow>, EffectJournalError> {
    connection
        .query_row(
            &format!(
                "SELECT {OPERATION_COLUMNS}
                   FROM host_effect_operations WHERE operation_id = ?1"
            ),
            [operation_id],
            raw_operation,
        )
        .optional()
        .map_err(|_| EffectJournalError::Database)?
        .map(OperationRow::try_from)
        .transpose()
}

fn require_permit(
    connection: &Connection,
    lease: &AuthorityLease,
    permit: &EffectPermit,
) -> Result<(), EffectJournalError> {
    let row = operation_by_id(connection, &permit.operation_id)?
        .ok_or(EffectJournalError::InvalidTransition)?;
    /* The whole row, and two of these fields COVER FOR EACH OTHER.
     *
     * Measured, 2026-08: deleting `row_revision` alone kills nothing, and
     * deleting `state` alone kills nothing either — every UPDATE in this
     * file moves both in one statement, so whichever comparison is left
     * refuses the stale permit by itself. A mutation battery that plants one
     * cut at a time therefore sees a working guard no matter which one it
     * deletes, and a person reading that green run would conclude the field
     * they deleted was dead weight. It is not: it is the other half of a
     * pair. `a_row_that_bore_two_permits_moves_for_only_the_first` is what
     * actually holds this, and only removing BOTH turns it red. */
    if row.ledger_id == permit.ledger_id
        && row.attempt == permit.attempt
        && row.kind == permit.kind
        && row.state == permit.state
        && row.binding_digest == permit.binding_digest
        && row.host_epoch == permit.host_epoch
        && row.operation_authority_epoch == permit.operation_authority_epoch
        && row.row_revision == permit.row_revision
        && row.prepared_revision == permit.prepared_revision
        && lease.authority_epoch == permit.issued_authority_epoch
        && lease.token_digest == permit.issued_token_digest
    {
        Ok(())
    } else {
        Err(EffectJournalError::InvalidTransition)
    }
}

fn require_authority(
    connection: &Connection,
    journal: &EffectJournal,
    lease: &AuthorityLease,
) -> Result<(), EffectJournalError> {
    if lease.lock.is_none()
        || lease.store_digest != store_digest(journal.store.private_path())
        || secret_digest(&lease.token) != lease.token_digest
    {
        return Err(EffectJournalError::StaleAuthority);
    }
    let matches: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM orchestration_authorities
              WHERE ledger_id = ?1 AND authority_epoch = ?2
                AND token_digest = ?3 AND owner_state = 'claimed'",
            params![
                lease.ledger_id,
                to_sql_u64(lease.authority_epoch)?,
                lease.token_digest,
            ],
            |row| row.get(0),
        )
        .map_err(|_| EffectJournalError::Database)?;
    if matches == 1 {
        Ok(())
    } else {
        Err(EffectJournalError::StaleAuthority)
    }
}

fn has_any_effect(connection: &Connection, ledger_id: &str) -> Result<bool, EffectJournalError> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM host_effect_operations WHERE ledger_id = ?1
             )",
            [ledger_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value == 1)
        .map_err(|_| EffectJournalError::Database)
}

fn has_in_flight_effect(
    connection: &Connection,
    ledger_id: &str,
) -> Result<bool, EffectJournalError> {
    connection
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                   FROM host_effect_operations
                  WHERE ledger_id = ?1
                    AND effect_state IN ('prepared', 'unknown')
             )",
            [ledger_id],
            |row| row.get::<_, i64>(0),
        )
        .map(|value| value == 1)
        .map_err(|_| EffectJournalError::Database)
}

fn operation_id(
    ledger_id: &str,
    slot: &str,
    fingerprint: &str,
    kind: HostEffectKind,
    attempt: u64,
) -> String {
    const DOMAIN: &[u8] = b"zerocode.orchestration.host-effect.v1";
    let mut digest = Sha256::new();
    for part in [
        DOMAIN,
        ledger_id.as_bytes(),
        slot.as_bytes(),
        fingerprint.as_bytes(),
        kind.as_db().as_bytes(),
    ] {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    digest.update((std::mem::size_of::<u64>() as u64).to_le_bytes());
    digest.update(attempt.to_le_bytes());
    format!("{:x}", digest.finalize())
}

fn valid_digest(value: &str) -> bool {
    value.len() == DIGEST_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn secret_digest(bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"zerocode.runtime-authority-token.v1");
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn store_digest(path: &std::path::Path) -> String {
    let mut digest = Sha256::new();
    digest.update(b"zerocode.runtime-authority-store.v1");
    let bytes = stable_path_bytes(path.as_os_str());
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
    format!("{:x}", digest.finalize())
}

fn owner_lock_error(error: OwnerLockError) -> EffectJournalError {
    match error {
        OwnerLockError::Busy => EffectJournalError::OwnerBusy,
        OwnerLockError::Unavailable => EffectJournalError::Database,
        #[cfg(not(any(unix, windows)))]
        OwnerLockError::Unsupported => EffectJournalError::OwnerLockUnsupported,
    }
}

fn valid_ledger_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn invalid(field: &'static str) -> EffectJournalError {
    EffectJournalError::InvalidInput { field }
}

/// The most strides one operation may record — five times the longest lane
/// table today, so a new stride is a design decision long before it is a
/// squeeze. The schema holds the same number in `host_effect_steps`'s
/// `ordinal` CHECK, so a writer that skips this door meets it again in the
/// database.
pub const MAX_STRIDES: u32 = 16;

/// One recorded stride of a multi-stride host effect, read back for recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrideTaken {
    pub ordinal: u32,
    pub stride: String,
    pub at_ms: i64,
}

/// The shape a stride's name must have. Its VOCABULARY belongs to the lane
/// that performs the effect; the journal only refuses names no lane writes.
fn valid_stride(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
}

fn require_one(changed: usize) -> Result<(), EffectJournalError> {
    if changed == 1 {
        Ok(())
    } else {
        Err(EffectJournalError::ConcurrentTransition)
    }
}

pub(crate) fn to_sql_u64(value: u64) -> Result<i64, EffectJournalError> {
    i64::try_from(value).map_err(|_| EffectJournalError::Corrupt)
}

pub(crate) fn from_sql_u64(value: i64) -> Result<u64, EffectJournalError> {
    u64::try_from(value).map_err(|_| EffectJournalError::Corrupt)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use rusqlite::params;
    use tempfile::TempDir;

    use super::*;

    struct Fixture {
        _root: TempDir,
        path: std::path::PathBuf,
        store: WorkflowStore,
        journal: EffectJournal,
        lease: Arc<AuthorityLease>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("private authority root");
            let private = root.path().join("authority");
            std::fs::create_dir(&private).expect("private authority directory");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;

                std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
                    .expect("private authority permissions");
            }
            let path = private.join("authority.sqlite");
            let store = WorkflowStore::open(&path).expect("workflow authority");
            let journal = EffectJournal::new(&store);
            let lease = Arc::new(
                journal
                    .claim_authority("main-ledger", &digest(0xf0), 0)
                    .expect("main ledger authority"),
            );
            Self {
                _root: root,
                path,
                store,
                journal,
                lease,
            }
        }
    }

    fn digest(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    /// A distinct, valid projection per (revision, word) — the same shape the
    /// blob fixtures had, with `next_id` carrying the distinctness the bytes
    /// used to. The revision argument is only a distinctness input now: the
    /// journal derives the written revision from `expected + 1` itself.
    fn snapshot(revision: u64, word: &str) -> LedgerProjectionV1 {
        let mut held = zerocode_core::orchestration::Ledger::new().export();
        held.next_id =
            revision * 1009 + word.bytes().fold(0_u64, |sum, byte| sum + u64::from(byte));
        held
    }

    fn fresh_snapshot(revision: u64, word: &str) -> LedgerProjectionV1 {
        snapshot(revision.wrapping_add(499), word)
    }

    fn claim(fixture: &Fixture, ledger_id: &str) -> AuthorityLease {
        fixture
            .journal
            .claim_authority(ledger_id, &digest(0xf1), 0)
            .expect("ledger authority")
    }

    fn insert_raw_prepared(
        connection: &Connection,
        ledger_id: &str,
        slot: u8,
        fingerprint: u8,
        kind: HostEffectKind,
        at_ms: i64,
    ) -> rusqlite::Result<usize> {
        let request_slot = digest(slot);
        let request_fingerprint = digest(fingerprint);
        let operation_id = operation_id(ledger_id, &request_slot, &request_fingerprint, kind, 1);
        connection.execute(
            "INSERT INTO host_effect_operations (
                ledger_id, request_slot, request_fingerprint, attempt, operation_id,
                effect_kind, effect_state, binding_digest, host_epoch,
                operation_authority_epoch, prepared_revision, prepared_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, 1, ?4, ?5, 'prepared', ?6, ?7, 1, 1, ?8, ?8)",
            params![
                ledger_id,
                request_slot,
                request_fingerprint,
                operation_id,
                kind.as_db(),
                digest(0xb1),
                digest(0xe1),
                at_ms,
            ],
        )
    }

    fn request(slot: u8, fingerprint: u8, kind: HostEffectKind) -> EffectRequest {
        EffectRequest::new(
            "main-ledger",
            digest(slot),
            digest(fingerprint),
            digest(0xb1),
            digest(0xe1),
            kind,
        )
        .expect("effect request")
    }

    /// The reason a door refused, with the permit it handed back SAID.
    ///
    /// These doors return the permit beside the refusal now, so a test that
    /// only wants the reason still has to account for the capability. That
    /// is not ceremony: a fixture that silently drops one is indisputable
    /// evidence the guard cannot tell fixtures from lanes, and this is the
    /// one line that keeps the guard sharp.
    fn refusal<T>(
        result: Result<T, Box<(EffectPermit, EffectJournalError)>>,
    ) -> EffectJournalError {
        match result {
            Ok(_) => panic!("the door did not refuse"),
            Err(refused) => {
                let (permit, why) = *refused;
                permit.abandon("fixture: this test wanted the refusal, not the row");
                why
            }
        }
    }

    fn execute(answer: BeginEffect) -> EffectPermit {
        match answer {
            BeginEffect::Execute(permit) => permit,
            other => panic!("expected an executable permit, got {other:?}"),
        }
    }

    #[test]
    fn current_schema_with_a_missing_or_replaced_effect_index_fails_closed() {
        for replacement in [
            None,
            Some("CREATE INDEX host_effect_single_in_flight ON host_effect_operations (ledger_id)"),
            Some(
                "CREATE UNIQUE INDEX host_effect_single_in_flight
                     ON host_effect_operations (ledger_id)
                  WHERE effect_state IN ('PREPARED', 'UNKNOWN')",
            ),
            Some(
                "CREATE UNIQUE INDEX host_effect_single_in_flight
                     ON host_effect_operations (ledger_id)
                  WHERE effect_state IN ('prepared ', 'unknown ')",
            ),
        ] {
            let fixture = Fixture::new();
            let connection = fixture.store.connection().expect("authority connection");
            connection
                .execute_batch("DROP INDEX host_effect_single_in_flight;")
                .expect("damage current index");
            if let Some(sql) = replacement {
                connection
                    .execute_batch(sql)
                    .expect("replace current index");
            }
            drop(connection);
            drop(fixture.journal);
            drop(fixture.store);
            assert!(matches!(
                WorkflowStore::open(&fixture.path),
                Err(crate::workflow_store::WorkflowStoreError::Corrupt)
            ));
        }
    }

    #[test]
    fn current_schema_with_a_missing_effect_table_fails_closed() {
        let fixture = Fixture::new();
        let connection = fixture.store.connection().expect("authority connection");
        connection
            .execute_batch("DROP TABLE host_effect_operations;")
            .expect("damage current schema");
        drop(connection);
        drop(fixture.journal);
        drop(fixture.store);
        assert!(matches!(
            WorkflowStore::open(&fixture.path),
            Err(crate::workflow_store::WorkflowStoreError::Corrupt)
        ));
    }

    #[test]
    fn current_schema_with_missing_or_replaced_owner_authority_fails_closed() {
        for replacement in [
            None,
            Some(
                "CREATE TABLE orchestration_authorities (
                    ledger_id TEXT PRIMARY KEY,
                    authority_epoch INTEGER NOT NULL,
                    owner_state TEXT NOT NULL,
                    token_digest TEXT,
                    owner_instance_digest TEXT,
                    owner_revision INTEGER NOT NULL,
                    claimed_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL,
                    released_at_ms INTEGER
                );",
            ),
        ] {
            let fixture = Fixture::new();
            let connection = fixture.store.connection().expect("authority connection");
            connection
                .execute_batch("DROP TABLE orchestration_authorities;")
                .expect("damage owner authority");
            if let Some(sql) = replacement {
                connection.execute_batch(sql).expect("replace owner table");
            }
            drop(connection);
            assert!(matches!(
                WorkflowStore::open(&fixture.path),
                Err(crate::workflow_store::WorkflowStoreError::Corrupt)
            ));
        }

        let fixture = Fixture::new();
        let connection = fixture.store.connection().expect("authority connection");
        connection
            .execute_batch(
                "DROP INDEX host_effect_authority_recovery;
                 CREATE INDEX host_effect_authority_recovery
                     ON host_effect_operations (ledger_id);",
            )
            .expect("replace authority recovery index");
        drop(connection);
        assert!(matches!(
            WorkflowStore::open(&fixture.path),
            Err(crate::workflow_store::WorkflowStoreError::Corrupt)
        ));
    }

    #[test]
    fn authority_claim_release_and_crash_takeover_are_monotonic() {
        let fixture = Fixture::new();
        let mut first = fixture
            .journal
            .claim_authority("owner-ledger", &digest(0xa1), 10)
            .expect("first owner");
        assert_eq!(first.authority_epoch(), 1);
        let raw_token = first
            .token
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let stored_token: String = fixture
            .store
            .connection()
            .expect("owner row")
            .query_row(
                "SELECT token_digest FROM orchestration_authorities
                  WHERE ledger_id = 'owner-ledger'",
                [],
                |row| row.get(0),
            )
            .expect("stored token digest");
        assert_ne!(stored_token, raw_token);
        assert!(!format!("{first:?}").contains(&raw_token));
        assert!(matches!(
            fixture
                .journal
                .claim_authority("owner-ledger", &digest(0xa2), 999_999),
            Err(EffectJournalError::OwnerBusy)
        ));
        fixture
            .journal
            .initialize_fresh(&first, &fresh_snapshot(0, "owner"), 10)
            .expect("owner ledger");
        fixture
            .journal
            .relinquish_authority(&mut first, 20)
            .expect("clean release");
        assert_eq!(
            fixture.journal.load_ledger(&first),
            Err(EffectJournalError::StaleAuthority)
        );
        let second = fixture
            .journal
            .claim_authority("owner-ledger", &digest(0xa2), 1)
            .expect("owner after release");
        assert_eq!(second.authority_epoch(), 2);
        drop(second);
        let mut third = fixture
            .journal
            .claim_authority("owner-ledger", &digest(0xa3), 1)
            .expect("crash takeover");
        assert_eq!(third.authority_epoch(), 3);
        let debug = format!("{third:?}");
        assert!(!debug.contains(&digest(0xa3)));
        fixture
            .journal
            .relinquish_authority(&mut third, 30)
            .expect("release takeover owner");
    }

    #[test]
    fn concurrent_claims_have_one_owner_and_the_next_epoch_waits_for_drop() {
        let fixture = Fixture::new();
        let ready = Arc::new(Barrier::new(9));
        let attempted = Arc::new(Barrier::new(9));
        let mut threads = Vec::new();
        for index in 0_u8..8 {
            let journal = fixture.journal.clone();
            let ready = Arc::clone(&ready);
            let attempted = Arc::clone(&attempted);
            threads.push(thread::spawn(move || {
                ready.wait();
                let answer = journal.claim_authority(
                    "concurrent-owner-ledger",
                    &digest(index),
                    i64::from(index),
                );
                attempted.wait();
                answer
            }));
        }
        ready.wait();
        attempted.wait();

        let mut winner = None;
        let mut busy = 0;
        for thread in threads {
            match thread.join().expect("claim thread") {
                Ok(lease) => {
                    assert!(winner.replace(lease).is_none(), "two authorities won");
                }
                Err(EffectJournalError::OwnerBusy) => busy += 1,
                Err(error) => panic!("unexpected claim result: {error:?}"),
            }
        }
        let first = winner.expect("one authority winner");
        assert_eq!(busy, 7);
        assert_eq!(first.authority_epoch(), 1);
        drop(first);

        let mut second = fixture
            .journal
            .claim_authority("concurrent-owner-ledger", &digest(0xf2), 10)
            .expect("takeover after winner drop");
        assert_eq!(second.authority_epoch(), 2);
        fixture
            .journal
            .relinquish_authority(&mut second, 11)
            .expect("release takeover");
    }

    #[test]
    fn authority_claim_and_release_faults_roll_back_without_dropping_the_fence() {
        let fixture = Fixture::new();
        let connection = fixture.store.connection().expect("fault connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_authority_claim
                 BEFORE INSERT ON orchestration_authorities
                 BEGIN SELECT RAISE(ABORT, 'injected claim failure'); END;",
            )
            .expect("claim fault");
        assert!(matches!(
            fixture
                .journal
                .claim_authority("fault-owner-ledger", &digest(0xd1), 1),
            Err(EffectJournalError::Database)
        ));
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM orchestration_authorities
                      WHERE ledger_id = 'fault-owner-ledger'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .expect("rolled back owner row"),
            0
        );
        connection
            .execute_batch("DROP TRIGGER fail_authority_claim;")
            .expect("remove claim fault");

        let mut authority = fixture
            .journal
            .claim_authority("fault-owner-ledger", &digest(0xd2), 2)
            .expect("claim after rollback");
        assert_eq!(authority.authority_epoch(), 1);
        fixture
            .journal
            .initialize_fresh(&authority, &fresh_snapshot(0, "fault-owner"), 2)
            .expect("fault owner ledger");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_authority_release
                 BEFORE UPDATE ON orchestration_authorities
                 WHEN NEW.owner_state = 'released'
                 BEGIN SELECT RAISE(ABORT, 'injected release failure'); END;",
            )
            .expect("release fault");
        assert_eq!(
            fixture.journal.relinquish_authority(&mut authority, 3),
            Err(EffectJournalError::Database)
        );
        assert!(
            fixture
                .journal
                .load_ledger(&authority)
                .expect("failed release retained authority")
                .is_some()
        );
        assert!(matches!(
            fixture
                .journal
                .claim_authority("fault-owner-ledger", &digest(0xd3), 4),
            Err(EffectJournalError::OwnerBusy)
        ));
        connection
            .execute_batch("DROP TRIGGER fail_authority_release;")
            .expect("remove release fault");
        fixture
            .journal
            .relinquish_authority(&mut authority, 5)
            .expect("release after rollback");
    }

    #[test]
    fn unresolved_effect_blocks_release_and_stale_permit_cannot_settle() {
        let fixture = Fixture::new();
        let mut first = fixture
            .journal
            .claim_authority("fenced-ledger", &digest(0xb1), 1)
            .expect("first effect owner");
        fixture
            .journal
            .import_legacy(&first, &snapshot(0, "legacy"), &digest(0xaa), 1)
            .expect("effect ledger");
        let request = EffectRequest::new(
            "fenced-ledger",
            digest(0x61),
            digest(0x71),
            digest(0x81),
            digest(0x91),
            HostEffectKind::Split,
        )
        .expect("fenced request");
        let permit = execute(
            fixture
                .journal
                .prepare(&first, 0, &snapshot(1, "prepared"), &request, 2)
                .expect("prepared effect"),
        );
        assert_eq!(permit.operation_authority_epoch(), 1);
        assert_eq!(
            fixture.journal.relinquish_authority(&mut first, 3),
            Err(EffectJournalError::ReleaseInFlight)
        );
        assert!(matches!(
            fixture
                .journal
                .claim_authority("fenced-ledger", &digest(0xb2), 3),
            Err(EffectJournalError::OwnerBusy)
        ));
        drop(first);
        let mut second = fixture
            .journal
            .claim_authority("fenced-ledger", &digest(0xb2), 4)
            .expect("takeover effect owner");
        assert_eq!(second.authority_epoch(), 2);
        assert!(matches!(
            refusal(fixture.journal.mark_unknown(
                &second,
                permit,
                1,
                &snapshot(2, "stale-must-not-land"),
                HostEffectFailure::Timeout,
                5,
            )),
            EffectJournalError::InvalidTransition
        ));
        let recovered = fixture
            .journal
            .recoverable(&second)
            .expect("re-mint recovered permit")
            .pop()
            .expect("one recovered permit");
        let unknown = fixture
            .journal
            .mark_unknown(
                &second,
                recovered,
                1,
                &snapshot(2, "unknown"),
                HostEffectFailure::Timeout,
                5,
            )
            .expect("current owner marks unknown");
        fixture
            .journal
            .settle(
                &second,
                unknown,
                2,
                &snapshot(3, "not-started"),
                EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Refused,
                    tombstone_digest: digest(0xc1),
                    retryable: false,
                    retry_not_before_ms: None,
                    settled_at_ms: 6,
                },
            )
            .expect("settle recovered effect");
        fixture
            .journal
            .relinquish_authority(&mut second, 7)
            .expect("release settled owner");
    }

    #[test]
    fn eight_concurrent_fresh_initializers_choose_one_exact_image() {
        let fixture = Fixture::new();
        let journal = Arc::new(fixture.journal.clone());
        let lease = Arc::clone(&fixture.lease);
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0_usize..8)
            .map(|index| {
                let journal = Arc::clone(&journal);
                let lease = Arc::clone(&lease);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let word = if index.is_multiple_of(2) {
                        "left"
                    } else {
                        "right"
                    };
                    let initial = fresh_snapshot(0, word);
                    barrier.wait();
                    (word, journal.initialize_fresh(&lease, &initial, 10))
                })
            })
            .collect();
        let answers: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("initialize thread"))
            .collect();
        assert_eq!(
            answers
                .iter()
                .filter(|(_, answer)| *answer == Ok(FreshLedgerResult::Initialized))
                .count(),
            1
        );
        let stored = journal
            .load_ledger(&fixture.lease)
            .expect("load initialized ledger")
            .expect("initialized row");
        let winner = if stored.projection == fresh_snapshot(0, "left") {
            "left"
        } else {
            assert_eq!(stored.projection, fresh_snapshot(0, "right"));
            "right"
        };
        for (word, answer) in answers {
            if word == winner {
                assert!(matches!(
                    answer,
                    Ok(FreshLedgerResult::Initialized | FreshLedgerResult::AlreadyInitialized)
                ));
            } else {
                assert_eq!(answer, Err(EffectJournalError::InitializationConflict));
            }
        }
    }

    #[test]
    fn fresh_initialization_is_exact_read_only_and_refuses_existing_effects() {
        let fixture = Fixture::new();
        let initial = fresh_snapshot(0, "initial");
        assert_eq!(
            fixture
                .journal
                .initialize_fresh(&fixture.lease, &initial, 100),
            Ok(FreshLedgerResult::Initialized)
        );
        assert_eq!(
            fixture
                .journal
                .initialize_fresh(&fixture.lease, &initial, 50),
            Ok(FreshLedgerResult::AlreadyInitialized)
        );
        let mut rebound = initial.clone();
        rebound.next_id += 1;
        assert_eq!(
            fixture
                .journal
                .initialize_fresh(&fixture.lease, &rebound, 101),
            Err(EffectJournalError::InitializationConflict)
        );
        let updated_at_ms: i64 = fixture
            .store
            .connection()
            .expect("timestamp connection")
            .query_row(
                "SELECT updated_at_ms FROM orchestration_ledger_heads
                  WHERE ledger_id = 'main-ledger'",
                [],
                |row| row.get(0),
            )
            .expect("initialization timestamp");
        assert_eq!(updated_at_ms, 100, "an idempotent retry rewrote time");

        let effect_initial = fresh_snapshot(0, "effect-ledger");
        let effect_lease = claim(&fixture, "effect-ledger");
        fixture
            .journal
            .initialize_fresh(&effect_lease, &effect_initial, 1)
            .expect("second fresh ledger");
        let connection = fixture.store.connection().expect("effect connection");
        insert_raw_prepared(
            &connection,
            "effect-ledger",
            0x33,
            0x43,
            HostEffectKind::Respawn,
            2,
        )
        .expect("existing effect row");
        assert_eq!(
            fixture
                .journal
                .initialize_fresh(&effect_lease, &effect_initial, 3),
            Err(EffectJournalError::InitializationConflict)
        );
    }

    #[test]
    fn fresh_initialization_refuses_terminal_or_legacy_history_with_identical_bytes() {
        let fixture = Fixture::new();
        let terminal_initial = fresh_snapshot(0, "terminal-history");
        let terminal_lease = claim(&fixture, "terminal-ledger");
        fixture
            .journal
            .initialize_fresh(&terminal_lease, &terminal_initial, 1)
            .expect("fresh terminal ledger");
        let connection = fixture.store.connection().expect("terminal connection");
        insert_raw_prepared(
            &connection,
            "terminal-ledger",
            0x34,
            0x44,
            HostEffectKind::Split,
            2,
        )
        .expect("prepared applied row");
        connection
            .execute(
                "UPDATE host_effect_operations
                    SET effect_state = 'applied', result_digest = ?1,
                        updated_at_ms = 3, settled_at_ms = 3, row_revision = 1
                  WHERE ledger_id = 'terminal-ledger' AND request_slot = ?2",
                params![digest(0x54), digest(0x34)],
            )
            .expect("applied history");
        insert_raw_prepared(
            &connection,
            "terminal-ledger",
            0x35,
            0x45,
            HostEffectKind::Close,
            4,
        )
        .expect("prepared not-started row");
        connection
            .execute(
                "UPDATE host_effect_operations
                    SET effect_state = 'not_started', failure_kind = 'refused',
                        tombstone_digest = ?1, updated_at_ms = 5,
                        settled_at_ms = 5, row_revision = 1
                  WHERE ledger_id = 'terminal-ledger' AND request_slot = ?2",
                params![digest(0x55), digest(0x35)],
            )
            .expect("not-started history");
        assert_eq!(
            fixture
                .journal
                .initialize_fresh(&terminal_lease, &terminal_initial, 6),
            Err(EffectJournalError::InitializationConflict)
        );

        let fresh_equivalent = fresh_snapshot(0, "legacy-equivalent");
        let legacy_lease = claim(&fixture, "legacy-ledger");
        fixture
            .journal
            .import_legacy(&legacy_lease, &fresh_equivalent, &digest(0xaa), 7)
            .expect("legacy import");
        assert_eq!(
            fixture
                .journal
                .initialize_fresh(&legacy_lease, &fresh_equivalent, 8),
            Err(EffectJournalError::InitializationConflict)
        );
    }

    #[test]
    fn fresh_initialization_fault_rolls_back_the_whole_image() {
        let fixture = Fixture::new();
        let connection = fixture.store.connection().expect("fault connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_fresh_initialization
                    AFTER INSERT ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected fresh initialization failure'); END;",
            )
            .expect("fresh initialization fault");
        assert_eq!(
            fixture.journal.initialize_fresh(
                &fixture.lease,
                &fresh_snapshot(0, "must-not-land"),
                10,
            ),
            Err(EffectJournalError::Database)
        );
        assert!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger after initialization fault")
                .is_none()
        );
    }

    #[test]
    fn cross_slot_prepares_have_one_api_winner_and_one_sqlite_row() {
        let fixture = Fixture::new();
        fixture
            .journal
            .import_legacy(&fixture.lease, &snapshot(0, "legacy"), &digest(0xaa), 1)
            .expect("legacy import");
        let journal = Arc::new(fixture.journal.clone());
        let lease = Arc::clone(&fixture.lease);
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0_u8..8)
            .map(|index| {
                let journal = Arc::clone(&journal);
                let lease = Arc::clone(&lease);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let request = request(0x40 + index, 0x50 + index, HostEffectKind::Split);
                    let prepared = snapshot(1, &format!("slot-{index}"));
                    barrier.wait();
                    journal.prepare(&lease, 0, &prepared, &request, 10)
                })
            })
            .collect();
        let answers: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("prepare thread"))
            .collect();
        assert_eq!(
            answers
                .iter()
                .filter(|answer| matches!(answer, Ok(BeginEffect::Execute(_))))
                .count(),
            1
        );
        assert_eq!(
            answers
                .iter()
                .filter(|answer| matches!(answer, Err(EffectJournalError::EffectInFlight)))
                .count(),
            7
        );
        let winner = answers
            .into_iter()
            .find_map(|answer| match answer {
                Ok(BeginEffect::Execute(permit)) => Some(permit),
                _ => None,
            })
            .expect("one executable permit");
        let unknown = journal
            .mark_unknown(
                &lease,
                winner,
                1,
                &snapshot(2, "cross-slot-unknown"),
                HostEffectFailure::Timeout,
                11,
            )
            .expect("mark cross-slot winner unknown");
        assert_eq!(unknown.state(), HostEffectState::Unknown);
        let connection = fixture.store.connection().expect("effect connection");
        let active: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM host_effect_operations
                  WHERE ledger_id = 'main-ledger'
                    AND effect_state IN ('prepared', 'unknown')",
                [],
                |row| row.get(0),
            )
            .expect("active effect count");
        assert_eq!(active, 1);
        let error = insert_raw_prepared(
            &connection,
            "main-ledger",
            0x70,
            0x71,
            HostEffectKind::Close,
            12,
        )
        .expect_err("partial unique index accepted a second active slot");
        assert!(matches!(
            error,
            rusqlite::Error::SqliteFailure(ref failure, _)
                if failure.code == rusqlite::ErrorCode::ConstraintViolation
        ));
        unknown.abandon("fixture: this test wanted the row read, not the row moved");
    }

    #[test]
    fn terminal_history_does_not_block_a_different_request_slot() {
        let fixture = Fixture::new();
        fixture
            .journal
            .import_legacy(&fixture.lease, &snapshot(0, "legacy"), &digest(0xaa), 1)
            .expect("legacy import");
        let first = execute(
            fixture
                .journal
                .prepare(
                    &fixture.lease,
                    0,
                    &snapshot(1, "first-prepared"),
                    &request(0x72, 0x73, HostEffectKind::Split),
                    2,
                )
                .expect("first slot"),
        );
        fixture
            .journal
            .settle(
                &fixture.lease,
                first,
                1,
                &snapshot(2, "first-applied"),
                EffectSettlement::Applied {
                    result_digest: digest(0x74),
                    settled_at_ms: 3,
                },
            )
            .expect("settle first slot");
        let second = execute(
            fixture
                .journal
                .prepare(
                    &fixture.lease,
                    2,
                    &snapshot(3, "second-prepared"),
                    &request(0x75, 0x76, HostEffectKind::Close),
                    4,
                )
                .expect("second slot"),
        );
        assert_eq!(second.state(), HostEffectState::Prepared);
        let connection = fixture.store.connection().expect("effect connection");
        let (all, active): (i64, i64) = connection
            .query_row(
                "SELECT COUNT(*),
                        SUM(effect_state IN ('prepared', 'unknown'))
                   FROM host_effect_operations
                  WHERE ledger_id = 'main-ledger'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("operation counts");
        assert_eq!((all, active), (2, 1));
        second.abandon("fixture: this test wanted the row read, not the row moved");
    }

    #[test]
    fn eight_concurrent_legacy_imports_choose_one_exact_immutable_image() {
        let fixture = Fixture::new();
        let journal = Arc::new(fixture.journal.clone());
        let lease = Arc::clone(&fixture.lease);
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0_usize..8)
            .map(|index| {
                let journal = Arc::clone(&journal);
                let lease = Arc::clone(&lease);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let word = if index.is_multiple_of(2) {
                        "left"
                    } else {
                        "right"
                    };
                    let initial = snapshot(0, word);
                    barrier.wait();
                    (
                        word,
                        journal.import_legacy(&lease, &initial, &digest(0xaa), 10),
                    )
                })
            })
            .collect();
        let answers: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("import thread"))
            .collect();
        assert_eq!(
            answers
                .iter()
                .filter(|(_, answer)| *answer == Ok(ImportResult::Imported))
                .count(),
            1
        );
        let stored = journal
            .load_ledger(&fixture.lease)
            .expect("load imported ledger")
            .expect("imported row");
        assert!(
            stored.projection == snapshot(0, "left") || stored.projection == snapshot(0, "right"),
            "the stored image is one thread's exact projection"
        );
        /* The identity of an import is the digest of the person's file, and
         * every thread carried the same file — so every thread is IN, and
         * the first projection to land is the interpretation that stays. */
        assert_eq!(
            answers
                .iter()
                .filter(|(_, answer)| *answer == Ok(ImportResult::AlreadyImported))
                .count(),
            7
        );
        assert_eq!(
            journal.import_legacy(&fixture.lease, &stored.projection, &digest(0xaa), 5),
            Ok(ImportResult::AlreadyImported),
            "an older retry rewrote the import timestamp"
        );
        // Same digest, different projection: the file did not change — our
        // reading of it did — and the disk's word beats a newer parser.
        let mut reparsed = stored.projection.clone();
        reparsed.next_id += 1;
        assert_eq!(
            journal.import_legacy(&fixture.lease, &reparsed, &digest(0xaa), 11),
            Ok(ImportResult::AlreadyImported),
            "the same file under a newer parser is still the same import"
        );
        assert_eq!(
            journal
                .load_ledger(&fixture.lease)
                .expect("load after the reparsed retry")
                .expect("imported row")
                .projection,
            stored.projection,
            "the stored interpretation survived the reparsed retry"
        );
        // Different digest, same projection: honestly not the same file, and
        // a second identity for one ledger fails closed to the operator.
        assert_eq!(
            journal.import_legacy(&fixture.lease, &stored.projection, &digest(0xab), 12),
            Err(EffectJournalError::ImportConflict),
            "the same rows under a different source digest are not the same import"
        );

        /* And the revision plays NO part in the identity: a ledger imported
         * from this file and advanced since is still this file's ledger. Two
         * windows racing a shared store make this reachable in production —
         * the loser arrives after the winner's first verb, and an answer of
         * "conflict" would stand a degraded window on a perfectly healthy
         * store. */
        let moved = snapshot(7, "moved-on");
        journal
            .advance_ledger(&fixture.lease, 0, &moved, 20)
            .expect("one verb after the import");
        assert_eq!(
            journal.import_legacy(&fixture.lease, &stored.projection, &digest(0xaa), 21),
            Ok(ImportResult::AlreadyImported),
            "an advanced ledger no longer answers its own file"
        );
        let advanced = journal
            .load_ledger(&fixture.lease)
            .expect("load after the late retry")
            .expect("advanced row");
        assert_eq!(advanced.revision, 1, "the late retry moved the revision");
        assert_eq!(
            advanced.projection, moved,
            "the late retry rewrote rows a verb had advanced"
        );
        assert_eq!(
            journal.import_legacy(&fixture.lease, &stored.projection, &digest(0xab), 22),
            Err(EffectJournalError::ImportConflict),
            "a different file still conflicts after the ledger moved"
        );
    }

    /// A ledger past the old snapshot wall round-trips through the rows.
    ///
    /// `MAX_ORCHESTRATION_LEDGER_BYTES` — the 16 MiB wall — died with the
    /// blob it measured. This is the existence proof at the store: a ledger
    /// whose rows hold more than that goes in through `import_legacy` and
    /// comes back equal, through the same doors the cutover will use. The
    /// bodies stay inside `MAX_PROSE` each, because the wall this proves
    /// down is the one on the WHOLE ledger, not the one on a field.
    #[test]
    fn a_ledger_past_the_old_wall_round_trips_through_the_rows() {
        use zerocode_core::orchestration::{MAX_PROSE, Message, MessageKind};
        let mut big = zerocode_core::orchestration::Ledger::new();
        let run_id = big.create_run("nightly", 5);
        let to = big.run(&run_id).expect("the run").address();
        /* A repeating pattern rather than one character, with the awkward
         * bytes in it, so a round trip that collapsed or re-encoded a body
         * could not pass by accident. */
        let body = "0123456789abcdef\"\\\n".repeat(MAX_PROSE / 19);
        assert!(body.len() <= MAX_PROSE);
        let mut held = 0_usize;
        let mut at = 0_u32;
        while held <= 16 * 1024 * 1024 {
            held += body.len();
            at += 1;
            big.send(
                &run_id,
                Message {
                    id: format!("wall-{at}"),
                    from: to.clone(),
                    to: to.clone(),
                    kind: MessageKind::Status,
                    body: body.clone().into(),
                    subject: Default::default(),
                    priority: Default::default(),
                    payload: Default::default(),
                    thread: None,
                    task: None,
                    dispatch: None,
                    author_seat: None,
                    created_ms: 6,
                },
            )
            .expect("a message the wall must carry");
        }
        let initial = big.export();

        let fixture = Fixture::new();
        assert_eq!(
            fixture
                .journal
                .import_legacy(&fixture.lease, &initial, &digest(0xaa), 10),
            Ok(ImportResult::Imported)
        );
        let loaded = fixture
            .journal
            .load_ledger(&fixture.lease)
            .expect("load the big ledger")
            .expect("imported row");
        assert_eq!(loaded.revision, 0);
        assert!(
            loaded.projection == initial,
            "the rows changed a ledger bigger than the old wall"
        );
    }

    #[test]
    fn eight_concurrent_plain_advances_have_one_revision_winner() {
        let fixture = Fixture::new();
        fixture
            .journal
            .import_legacy(&fixture.lease, &snapshot(0, "legacy"), &digest(0xaa), 10)
            .expect("legacy import");
        let journal = Arc::new(fixture.journal.clone());
        let lease = Arc::clone(&fixture.lease);
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let journal = Arc::clone(&journal);
                let lease = Arc::clone(&lease);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let next = snapshot(1, "plain");
                    barrier.wait();
                    journal.advance_ledger(&lease, 0, &next, 20)
                })
            })
            .collect();
        let answers: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("advance thread"))
            .collect();
        assert_eq!(answers.iter().filter(|answer| answer.is_ok()).count(), 1);
        assert_eq!(
            answers
                .iter()
                .filter(|answer| **answer == Err(EffectJournalError::RevisionMismatch))
                .count(),
            7
        );
        let advanced = journal
            .load_ledger(&fixture.lease)
            .expect("load advanced ledger")
            .expect("advanced row");
        assert_eq!(advanced.revision, 1);
        assert_eq!(advanced.projection, snapshot(1, "plain"));
    }

    #[test]
    fn prepared_and_unknown_effects_block_plain_ledger_advances() {
        let fixture = Fixture::new();
        fixture
            .journal
            .import_legacy(&fixture.lease, &snapshot(0, "legacy"), &digest(0xaa), 1)
            .expect("legacy import");
        let request = request(0x10, 0x20, HostEffectKind::Split);
        let prepared = execute(
            fixture
                .journal
                .prepare(&fixture.lease, 0, &snapshot(1, "prepared"), &request, 2)
                .expect("prepare effect"),
        );
        assert_eq!(
            fixture
                .journal
                .advance_ledger(&fixture.lease, 1, &snapshot(2, "plain"), 3),
            Err(EffectJournalError::EffectInFlight)
        );
        let unknown = fixture
            .journal
            .mark_unknown(
                &fixture.lease,
                prepared,
                1,
                &snapshot(2, "unknown"),
                HostEffectFailure::Timeout,
                3,
            )
            .expect("mark unknown");
        assert_eq!(
            fixture
                .journal
                .advance_ledger(&fixture.lease, 2, &snapshot(3, "plain"), 4),
            Err(EffectJournalError::EffectInFlight)
        );
        fixture
            .journal
            .settle(
                &fixture.lease,
                unknown,
                2,
                &snapshot(3, "applied"),
                EffectSettlement::Applied {
                    result_digest: digest(0x70),
                    settled_at_ms: 4,
                },
            )
            .expect("settle effect");
        fixture
            .journal
            .advance_ledger(&fixture.lease, 3, &snapshot(4, "plain"), 5)
            .expect("plain advance after settlement");
        assert_eq!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger")
                .expect("row")
                .revision,
            4
        );
    }

    #[test]
    fn ledger_timestamps_never_move_backwards() {
        let fixture = Fixture::new();
        let initial = snapshot(0, "legacy");
        assert_eq!(
            fixture
                .journal
                .import_legacy(&fixture.lease, &initial, &digest(0xaa), 100),
            Ok(ImportResult::Imported)
        );
        assert_eq!(
            fixture
                .journal
                .import_legacy(&fixture.lease, &initial, &digest(0xaa), 50),
            Ok(ImportResult::AlreadyImported)
        );
        assert_eq!(
            fixture
                .journal
                .advance_ledger(&fixture.lease, 0, &snapshot(1, "older"), 99),
            Err(EffectJournalError::TimestampRegression)
        );
        fixture
            .journal
            .advance_ledger(&fixture.lease, 0, &snapshot(1, "same-time"), 100)
            .expect("equal timestamp is monotonic");
        assert!(matches!(
            fixture.journal.prepare(
                &fixture.lease,
                1,
                &snapshot(2, "effect-in-the-past"),
                &request(0x19, 0x29, HostEffectKind::Close),
                99,
            ),
            Err(EffectJournalError::TimestampRegression)
        ));
        let connection = fixture.store.connection().expect("timestamp connection");
        let (revision, updated_at_ms, effects): (i64, i64, i64) = connection
            .query_row(
                "SELECT revision, updated_at_ms,
                        (SELECT COUNT(*) FROM host_effect_operations)
                   FROM orchestration_ledger_heads WHERE ledger_id = 'main-ledger'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("timestamp row");
        assert_eq!((revision, updated_at_ms, effects), (1, 100, 0));
    }

    #[test]
    fn import_and_plain_advance_faults_roll_back_the_whole_snapshot() {
        let fixture = Fixture::new();
        let connection = fixture.store.connection().expect("fault connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_legacy_import
                    AFTER INSERT ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected import failure'); END;",
            )
            .expect("import fault");
        assert_eq!(
            fixture.journal.import_legacy(
                &fixture.lease,
                &snapshot(0, "legacy"),
                &digest(0xaa),
                10
            ),
            Err(EffectJournalError::Database)
        );
        assert!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger after import fault")
                .is_none()
        );
        connection
            .execute_batch("DROP TRIGGER fail_legacy_import;")
            .expect("remove import fault");
        fixture
            .journal
            .import_legacy(&fixture.lease, &snapshot(0, "legacy"), &digest(0xaa), 10)
            .expect("import after fault");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_plain_advance
                    AFTER UPDATE OF revision ON orchestration_ledger_heads
                    BEGIN SELECT RAISE(ABORT, 'injected advance failure'); END;",
            )
            .expect("advance fault");
        assert_eq!(
            fixture
                .journal
                .advance_ledger(&fixture.lease, 0, &snapshot(1, "plain"), 20),
            Err(EffectJournalError::Database)
        );
        assert_eq!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger after advance fault")
                .expect("imported snapshot")
                .projection,
            snapshot(0, "legacy")
        );
        connection
            .execute_batch("DROP TRIGGER fail_plain_advance;")
            .expect("remove advance fault");
        fixture
            .journal
            .advance_ledger(&fixture.lease, 0, &snapshot(1, "plain"), 20)
            .expect("advance after fault");
    }

    #[test]
    fn eight_concurrent_prepares_mint_one_attempt_and_one_operation_id() {
        let fixture = Fixture::new();
        let journal = Arc::new(fixture.journal.clone());
        let lease = Arc::clone(&fixture.lease);
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let journal = Arc::clone(&journal);
                let lease = Arc::clone(&lease);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let request = request(0x11, 0x21, HostEffectKind::Split);
                    let prepared = snapshot(1, "prepared");
                    barrier.wait();
                    journal
                        .prepare(&lease, 0, &prepared, &request, 10)
                        .map(|answer| match answer {
                            BeginEffect::Execute(permit) => {
                                let seen =
                                    (true, permit.attempt(), permit.operation_id().to_string());
                                permit.abandon(
                                    "fixture: this test wanted the row read, not the row moved",
                                );
                                seen
                            }
                            BeginEffect::Reconcile(permit) => {
                                let seen =
                                    (false, permit.attempt(), permit.operation_id().to_string());
                                permit.abandon(
                                    "fixture: this test wanted the row read, not the row moved",
                                );
                                seen
                            }
                            other => panic!("concurrent prepare returned {other:?}"),
                        })
                })
            })
            .collect();
        let answers: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("prepare thread").expect("prepare"))
            .collect();
        assert_eq!(answers.iter().filter(|(execute, _, _)| *execute).count(), 1);
        assert!(answers.iter().all(|(_, attempt, _)| *attempt == 1));
        assert!(
            answers
                .windows(2)
                .all(|pair| pair[0].2.as_str() == pair[1].2.as_str()),
            "one attempt produced more than one operation id: {answers:?}"
        );
        assert_eq!(
            journal
                .load_ledger(&fixture.lease)
                .expect("ledger")
                .expect("prepared ledger")
                .revision,
            1
        );
    }

    #[test]
    fn applied_attempt_reopens_and_replays_without_rewriting_the_ledger() {
        let fixture = Fixture::new();
        let request = request(0x12, 0x22, HostEffectKind::Split);
        let permit = execute(
            fixture
                .journal
                .prepare(&fixture.lease, 0, &snapshot(1, "prepared"), &request, 10)
                .expect("prepare"),
        );
        let operation = permit.operation_id().to_string();
        let result = digest(0x72);
        let settled = fixture
            .journal
            .settle(
                &fixture.lease,
                permit,
                1,
                &snapshot(2, "applied"),
                EffectSettlement::Applied {
                    result_digest: result.clone(),
                    settled_at_ms: 20,
                },
            )
            .expect("settle applied");
        assert!(matches!(settled, SettledEffect::Applied(_)));
        let Fixture {
            _root,
            path,
            store,
            journal: held_journal,
            lease: held_lease,
        } = fixture;
        drop(held_lease);
        drop(held_journal);
        drop(store);

        let reopened = WorkflowStore::open(&path).expect("reopen authority");
        let journal = EffectJournal::new(&reopened);
        let lease = journal
            .claim_authority("main-ledger", &digest(0xf3), 30)
            .expect("reopened authority");
        let replay = journal
            .prepare(&lease, 2, &snapshot(3, "must-not-land"), &request, 30)
            .expect("replay");
        let BeginEffect::Replay(replay) = replay else {
            panic!("applied operation was not replayed");
        };
        assert_eq!(replay.operation_id(), operation);
        assert_eq!(replay.result_digest(), result);
        let ledger = journal
            .load_ledger(&lease)
            .expect("load ledger")
            .expect("ledger row");
        assert_eq!(ledger.revision, 2);
        assert_eq!(ledger.projection, snapshot(2, "applied"));
    }

    #[test]
    fn retry_slot_conflicts_and_cross_slot_races_fail_closed() {
        let fixture = Fixture::new();
        execute(
            fixture
                .journal
                .prepare(
                    &fixture.lease,
                    0,
                    &snapshot(1, "first"),
                    &request(0x13, 0x23, HostEffectKind::Split),
                    10,
                )
                .expect("first prepare"),
        )
        .abandon("fixture: this test wanted the row read, not the row moved");
        assert!(matches!(
            fixture.journal.prepare(
                &fixture.lease,
                1,
                &snapshot(2, "conflict"),
                &request(0x13, 0x24, HostEffectKind::Split),
                11,
            ),
            Err(EffectJournalError::RequestConflict)
        ));
        assert!(matches!(
            fixture.journal.prepare(
                &fixture.lease,
                0,
                &snapshot(1, "stale"),
                &request(0x14, 0x25, HostEffectKind::Close),
                12,
            ),
            Err(EffectJournalError::EffectInFlight)
        ));
        assert_eq!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger")
                .expect("row")
                .revision,
            1
        );
    }

    /// One row can still bear two permits — and the second one moves nothing.
    ///
    /// [`EffectJournal::open_operations`] took the READING half of this away:
    /// nothing mints a capability just to compare fields any more. The
    /// GRANTING half is still legitimately re-entrant — a boot holds the
    /// recovery permits it was handed, and a caller that reopens gets its own
    /// — so two live permits for one row remain reachable, and the type has
    /// no idea. What actually keeps the later one from settling a row twice
    /// is the row revision each permit names, checked in `require_permit`.
    ///
    /// That is a fact about a `WHERE` clause, and facts about `WHERE` clauses
    /// are exactly what stops being true quietly. So it is asked here rather
    /// than asserted in the doc comment that calls this type one-use.
    #[test]
    fn a_row_that_bore_two_permits_moves_for_only_the_first() {
        let fixture = Fixture::new();
        let request = request(0x1a, 0x2b, HostEffectKind::Split);
        let first = execute(
            fixture
                .journal
                .prepare(&fixture.lease, 0, &snapshot(1, "prepared"), &request, 10)
                .expect("prepare"),
        );
        let mut again = fixture
            .journal
            .recoverable(&fixture.lease)
            .expect("the same open row, minted a second time");
        assert_eq!(again.len(), 1, "one open row");
        let second = again.pop().expect("a second permit for that row");
        assert_eq!(
            second.operation_id(),
            first.operation_id(),
            "the two permits name one row"
        );

        fixture
            .journal
            .settle(
                &fixture.lease,
                first,
                1,
                &snapshot(2, "applied"),
                EffectSettlement::Applied {
                    result_digest: digest(0x5c),
                    settled_at_ms: 20,
                },
            )
            .expect("the first permit moves the row");

        /* The second names a revision the row no longer has. It comes back
         * beside the refusal — which is the other half of the same rule:
         * being refused is not being spent. */
        assert_eq!(
            refusal(fixture.journal.settle(
                &fixture.lease,
                second,
                2,
                &snapshot(3, "applied-twice"),
                EffectSettlement::Applied {
                    result_digest: digest(0x5d),
                    settled_at_ms: 21,
                },
            )),
            EffectJournalError::InvalidTransition,
            "the later permit for a settled row must not move it"
        );
    }

    #[test]
    fn not_started_requires_a_tombstone_and_backoff_before_attempt_two() {
        let fixture = Fixture::new();
        let request = request(0x15, 0x26, HostEffectKind::Split);
        let first = execute(
            fixture
                .journal
                .prepare(&fixture.lease, 0, &snapshot(1, "first"), &request, 10)
                .expect("first prepare"),
        );
        let first_id = first.operation_id().to_string();
        fixture
            .journal
            .settle(
                &fixture.lease,
                first,
                1,
                &snapshot(2, "not-started"),
                EffectSettlement::NotStarted {
                    failure: HostEffectFailure::Unavailable,
                    tombstone_digest: digest(0x90),
                    retryable: true,
                    retry_not_before_ms: Some(50),
                    settled_at_ms: 20,
                },
            )
            .expect("not-started tombstone");
        let refused = fixture
            .journal
            .prepare(&fixture.lease, 2, &snapshot(3, "too-soon"), &request, 49)
            .expect("backoff answer");
        let BeginEffect::Refused(refused) = refused else {
            panic!("backoff did not refuse a new attempt");
        };
        assert_eq!(refused.attempt(), 1);
        assert_eq!(refused.tombstone_digest(), digest(0x90));
        assert_eq!(refused.retry_not_before_ms(), Some(50));

        let second = execute(
            fixture
                .journal
                .prepare(&fixture.lease, 2, &snapshot(3, "second"), &request, 50)
                .expect("second attempt"),
        );
        assert_eq!(second.attempt(), 2);
        assert_ne!(second.operation_id(), first_id);
        let prepared_at: i64 = fixture
            .store
            .connection()
            .expect("attempt time connection")
            .query_row(
                "SELECT prepared_at_ms FROM host_effect_operations
                  WHERE ledger_id = 'main-ledger' AND request_slot = ?1 AND attempt = 2",
                [digest(0x15)],
                |row| row.get(0),
            )
            .expect("attempt two time");
        assert_eq!(prepared_at, 50, "attempt two inherited attempt one's clock");
        assert_eq!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger")
                .expect("row")
                .revision,
            3
        );
        second.abandon("fixture: this test wanted the row read, not the row moved");
    }

    #[test]
    fn prepare_unknown_and_settle_faults_roll_back_their_ledger_transition() {
        let fixture = Fixture::new();
        let request = request(0x16, 0x27, HostEffectKind::Release);
        let connection = fixture.store.connection().expect("fault connection");
        connection
            .execute_batch(
                "CREATE TRIGGER fail_effect_prepare
                    BEFORE INSERT ON host_effect_operations
                    BEGIN SELECT RAISE(ABORT, 'injected prepare failure'); END;",
            )
            .expect("prepare fault");
        assert!(matches!(
            fixture
                .journal
                .prepare(&fixture.lease, 0, &snapshot(1, "prepared"), &request, 10),
            Err(EffectJournalError::Database)
        ));
        assert!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger after failed prepare")
                .is_none(),
            "the ledger half committed without Prepared"
        );
        connection
            .execute_batch("DROP TRIGGER fail_effect_prepare;")
            .expect("remove prepare fault");
        let permit = execute(
            fixture
                .journal
                .prepare(&fixture.lease, 0, &snapshot(1, "prepared"), &request, 10)
                .expect("prepare after fault"),
        );

        connection
            .execute_batch(
                "CREATE TRIGGER fail_effect_unknown
                    BEFORE UPDATE OF effect_state ON host_effect_operations
                    WHEN NEW.effect_state = 'unknown'
                    BEGIN SELECT RAISE(ABORT, 'injected unknown failure'); END;",
            )
            .expect("unknown fault");
        assert!(matches!(
            refusal(fixture.journal.mark_unknown(
                &fixture.lease,
                permit,
                1,
                &snapshot(2, "unknown"),
                HostEffectFailure::Timeout,
                20,
            )),
            EffectJournalError::Database
        ));
        assert_eq!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger after unknown fault")
                .expect("prepared snapshot")
                .revision,
            1
        );
        connection
            .execute_batch("DROP TRIGGER fail_effect_unknown;")
            .expect("remove unknown fault");
        let prepared = fixture
            .journal
            .recoverable(&fixture.lease)
            .expect("recover prepared")
            .pop()
            .expect("prepared permit");
        assert_eq!(prepared.state(), HostEffectState::Prepared);
        let unknown = fixture
            .journal
            .mark_unknown(
                &fixture.lease,
                prepared,
                1,
                &snapshot(2, "unknown"),
                HostEffectFailure::Timeout,
                20,
            )
            .expect("mark unknown");

        connection
            .execute_batch(
                "CREATE TRIGGER fail_effect_settle
                    BEFORE UPDATE OF effect_state ON host_effect_operations
                    WHEN NEW.effect_state = 'applied'
                    BEGIN SELECT RAISE(ABORT, 'injected settle failure'); END;",
            )
            .expect("settle fault");
        assert_eq!(
            refusal(fixture.journal.settle(
                &fixture.lease,
                unknown,
                2,
                &snapshot(3, "applied"),
                EffectSettlement::Applied {
                    result_digest: digest(0x77),
                    settled_at_ms: 30,
                },
            )),
            EffectJournalError::Database
        );
        assert_eq!(
            fixture
                .journal
                .load_ledger(&fixture.lease)
                .expect("ledger after settle fault")
                .expect("unknown snapshot")
                .revision,
            2
        );
        connection
            .execute_batch("DROP TRIGGER fail_effect_settle;")
            .expect("remove settle fault");
        let unknown = fixture
            .journal
            .recoverable(&fixture.lease)
            .expect("recover unknown")
            .pop()
            .expect("unknown permit");
        assert_eq!(unknown.state(), HostEffectState::Unknown);
        assert!(matches!(
            fixture
                .journal
                .settle(
                    &fixture.lease,
                    unknown,
                    2,
                    &snapshot(3, "applied"),
                    EffectSettlement::Applied {
                        result_digest: digest(0x77),
                        settled_at_ms: 30,
                    },
                )
                .expect("settle recovered operation"),
            SettledEffect::Applied(_)
        ));
    }

    #[test]
    fn debug_and_effect_rows_disclose_no_private_input_channel() {
        let fixture = Fixture::new();
        let private = fixture.path.to_string_lossy().into_owned();
        let secret = "/private/project raw-session prompt --token";
        let snapshot = snapshot(1, "private");
        let fresh = fresh_snapshot(0, "private");
        let request = request(0x17, 0x28, HostEffectKind::Respawn);
        let permit = execute(
            fixture
                .journal
                .prepare(&fixture.lease, 0, &snapshot, &request, 10)
                .expect("prepare private snapshot"),
        );
        let result = digest(0x98);
        let tombstone = digest(0x99);
        let applied = EffectSettlement::Applied {
            result_digest: result.clone(),
            settled_at_ms: 20,
        };
        let not_started = EffectSettlement::NotStarted {
            failure: HostEffectFailure::Refused,
            tombstone_digest: tombstone.clone(),
            retryable: false,
            retry_not_before_ms: None,
            settled_at_ms: 20,
        };
        let debug = format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?} {:?}",
            fixture.journal,
            snapshot,
            fresh,
            request,
            permit,
            applied,
            not_started,
            ImportResult::Imported,
            FreshLedgerResult::Initialized,
            EffectJournalError::InitializationConflict,
        );
        assert!(!debug.contains(&private));
        assert!(!debug.contains(secret));
        assert!(!debug.contains(&digest(0x17)));
        assert!(!debug.contains(&result));
        assert!(!debug.contains(&tombstone));
        assert!(matches!(
            EffectRequest::new(
                "/private/project",
                digest(1),
                digest(2),
                digest(3),
                digest(4),
                HostEffectKind::Split,
            ),
            Err(EffectJournalError::InvalidInput { field: "ledger_id" })
        ));

        let connection = fixture.store.connection().expect("privacy connection");
        let stored: String = connection
            .query_row(
                "SELECT ledger_id || request_slot || request_fingerprint || operation_id ||
                        binding_digest || host_epoch || effect_kind || effect_state
                   FROM host_effect_operations",
                [],
                |row| row.get(0),
            )
            .expect("effect metadata");
        assert!(!stored.contains(&private));
        assert!(!stored.contains(secret));
        permit.abandon("fixture: this test wanted the row read, not the row moved");
    }

    #[test]
    fn public_inputs_are_bounded_and_digests_are_lowercase_sha256() {
        assert!(matches!(
            EffectRequest::new(
                "main-ledger",
                "A".repeat(64),
                digest(2),
                digest(3),
                digest(4),
                HostEffectKind::Split,
            ),
            Err(EffectJournalError::InvalidInput {
                field: "request_slot"
            })
        ));
        assert!(matches!(
            EffectSettlement::NotStarted {
                failure: HostEffectFailure::Refused,
                tombstone_digest: digest(5),
                retryable: true,
                retry_not_before_ms: None,
                settled_at_ms: 10,
            }
            .validate(),
            Err(EffectJournalError::InvalidInput {
                field: "retry_not_before_ms"
            })
        ));
        let fixture = Fixture::new();
        let fresh = fresh_snapshot(0, "fresh");
        assert_eq!(
            fixture
                .journal
                .import_legacy(&fixture.lease, &fresh, "not-a-digest", 1),
            Err(EffectJournalError::InvalidInput {
                field: "legacy_digest"
            })
        );
        assert!(matches!(
            fixture
                .journal
                .claim_authority("/private/ledger", &digest(0xf2), 1),
            Err(EffectJournalError::InvalidInput { field: "ledger_id" })
        ));
        assert_eq!(
            fixture.journal.import_legacy(
                &fixture.lease,
                &snapshot(0, "legacy"),
                &digest(0xaa),
                -1
            ),
            Err(EffectJournalError::InvalidInput { field: "now_ms" })
        );
        assert_eq!(
            fixture
                .journal
                .advance_ledger(&fixture.lease, 0, &snapshot(1, "plain"), 1),
            Err(EffectJournalError::RevisionMismatch),
            "plain advance created a ledger without the import boundary"
        );
        /* The old shape also refused a fresh snapshot that arrived already
         * claiming a revision or a legacy digest. Those refusals died with
         * the inputs: a projection carries neither field now — the journal
         * writes revision zero itself and only `import_legacy` stamps a
         * digest — so the door has nothing of that kind left to refuse. */
        let fresh_lease = claim(&fixture, "fresh-ledger");
        assert_eq!(
            fixture.journal.initialize_fresh(&fresh_lease, &fresh, -1),
            Err(EffectJournalError::InvalidInput { field: "now_ms" })
        );
    }

    #[test]
    fn operation_id_has_a_stable_golden_value_for_one_attempt() {
        assert_eq!(
            operation_id(
                "main-ledger",
                &digest(0x31),
                &digest(0x41),
                HostEffectKind::Split,
                1,
            ),
            "835ddb8ab486407888cdfa659d1d0d65a1fdd27f5474cff2a17c98df724a5b16"
        );
    }

    /* ---- v6: strides (the crash checkpoint of a multi-stride effect) ---- */

    fn a_prepared_lifecycle_permit(fixture: &Fixture) -> EffectPermit {
        let request = EffectRequest::new(
            "main-ledger",
            digest(0x61),
            digest(0x62),
            digest(0xb1),
            digest(0xe1),
            HostEffectKind::Respawn,
        )
        .expect("effect request");
        match fixture
            .journal
            .prepare(&fixture.lease, 0, &snapshot(1, "prepared"), &request, 20)
            .expect("prepare")
        {
            BeginEffect::Execute(permit) => permit,
            other => panic!("expected a fresh permit, got {other:?}"),
        }
    }

    /// The happy path and both ways of stepping out of line.
    #[test]
    fn strides_are_recorded_in_line_and_read_back_in_it() {
        let fixture = Fixture::new();
        let permit = a_prepared_lifecycle_permit(&fixture);

        fixture
            .journal
            .step(&fixture.lease, &permit, 0, "pane_cut", 21)
            .expect("first stride");
        fixture
            .journal
            .step(&fixture.lease, &permit, 1, "seat_settled", 22)
            .expect("second stride");

        // A repeat and a gap are both a second record disagreeing with the
        // first, and neither is written.
        assert_eq!(
            fixture
                .journal
                .step(&fixture.lease, &permit, 1, "seat_settled", 23)
                .expect_err("a repeated ordinal must be refused"),
            invalid("ordinal"),
        );
        assert_eq!(
            fixture
                .journal
                .step(&fixture.lease, &permit, 3, "seat_repointed", 23)
                .expect_err("a gap must be refused"),
            invalid("ordinal"),
        );

        let taken = fixture
            .journal
            .strides_taken(&fixture.lease, permit.operation_id())
            .expect("strides read back");
        assert_eq!(
            taken,
            vec![
                StrideTaken {
                    ordinal: 0,
                    stride: "pane_cut".to_string(),
                    at_ms: 21,
                },
                StrideTaken {
                    ordinal: 1,
                    stride: "seat_settled".to_string(),
                    at_ms: 22,
                },
            ],
        );
        permit.abandon("fixture: this test wanted the row read, not the row moved");
    }

    /// A name no lane writes is refused at the door, not stored.
    #[test]
    fn a_stride_named_outside_the_shape_is_refused() {
        let fixture = Fixture::new();
        let permit = a_prepared_lifecycle_permit(&fixture);
        for stride in ["", "Pane_Cut", "pane cut", "pane-cut", &"x".repeat(65)] {
            assert_eq!(
                fixture
                    .journal
                    .step(&fixture.lease, &permit, 0, stride, 21)
                    .expect_err("a malformed stride must be refused"),
                invalid("stride"),
            );
        }
        assert_eq!(
            fixture
                .journal
                .strides_taken(&fixture.lease, permit.operation_id())
                .expect("strides read back"),
            Vec::new(),
        );
        permit.abandon("fixture: this test wanted the row read, not the row moved");
    }

    /// Once the operation settles, the permit that stepped it is spent paper.
    #[test]
    fn a_settled_operation_takes_no_more_strides() {
        let fixture = Fixture::new();
        let stale = a_prepared_lifecycle_permit(&fixture);
        fixture
            .journal
            .step(&fixture.lease, &stale, 0, "pane_cut", 21)
            .expect("a stride while in flight");

        // The same row, borrowed again — settling through THIS permit spends
        // the row revision the first one still names.
        let request = EffectRequest::new(
            "main-ledger",
            digest(0x61),
            digest(0x62),
            digest(0xb1),
            digest(0xe1),
            HostEffectKind::Respawn,
        )
        .expect("effect request");
        let BeginEffect::Reconcile(current) = fixture
            .journal
            .prepare(&fixture.lease, 1, &snapshot(2, "reconcile"), &request, 22)
            .expect("reconcile")
        else {
            panic!("an in-flight operation reconciles");
        };
        fixture
            .journal
            .settle(
                &fixture.lease,
                current,
                1,
                &snapshot(2, "settled"),
                EffectSettlement::Applied {
                    result_digest: digest(0x63),
                    settled_at_ms: 23,
                },
            )
            .expect("settle");

        assert_eq!(
            fixture
                .journal
                .step(&fixture.lease, &stale, 1, "seat_settled", 24)
                .expect_err("a settled operation must take no more strides"),
            EffectJournalError::InvalidTransition,
        );
        let taken = fixture
            .journal
            .strides_taken(&fixture.lease, stale.operation_id())
            .expect("strides read back");
        assert_eq!(taken.len(), 1, "the stride taken in flight stays");
        stale.abandon("fixture: this test wanted the row read, not the row moved");
    }

    /// A stride is written under the authority that holds the ledger, or not
    /// at all — and read back under one, or not at all.
    #[test]
    fn a_stride_without_the_authority_is_refused() {
        let fixture = Fixture::new();
        let permit = a_prepared_lifecycle_permit(&fixture);
        // The row this lease names stops saying "claimed" — the shape a
        // crashed-and-superseded window finds when it wakes back up.
        fixture
            .store
            .connection()
            .expect("connection")
            .execute(
                "UPDATE orchestration_authorities
                    SET owner_state = 'released', token_digest = NULL,
                        owner_instance_digest = NULL, released_at_ms = 32
                  WHERE ledger_id = 'main-ledger'",
                [],
            )
            .expect("release the authority row");
        assert_eq!(
            fixture
                .journal
                .step(&fixture.lease, &permit, 0, "pane_cut", 31)
                .expect_err("a stale lease must not write a stride"),
            EffectJournalError::StaleAuthority,
        );
        assert_eq!(
            fixture
                .journal
                .strides_taken(&fixture.lease, permit.operation_id())
                .expect_err("a stale lease must not read strides either"),
            EffectJournalError::StaleAuthority,
        );
        permit.abandon("fixture: this test wanted the row read, not the row moved");
    }

    /// The table refuses a stride for an operation nobody prepared.
    #[test]
    fn a_stride_for_an_operation_nobody_prepared_is_refused_by_the_table() {
        let fixture = Fixture::new();
        let connection = fixture.store.connection().expect("connection");
        let refused = connection.execute(
            "INSERT INTO host_effect_steps (operation_id, ordinal, step, at_ms)
             VALUES (?1, 0, 'pane_cut', 21)",
            params![digest(0x77)],
        );
        assert!(
            refused.is_err(),
            "a stride without its operation must not insert"
        );
    }

    /// What the checkpoint is FOR: the strides survive the window that wrote
    /// them, and the next window reads how far the last one got.
    #[test]
    fn strides_survive_the_window_that_wrote_them() {
        let fixture = Fixture::new();
        let permit = a_prepared_lifecycle_permit(&fixture);
        fixture
            .journal
            .step(&fixture.lease, &permit, 0, "pane_cut", 21)
            .expect("first stride");
        fixture
            .journal
            .step(&fixture.lease, &permit, 1, "seat_settled", 22)
            .expect("second stride");
        let operation = permit.operation_id().to_string();
        permit.abandon("fixture: this test wanted the row read, not the row moved");
        drop(fixture.lease);
        drop(fixture.journal);
        drop(fixture.store);

        let store = WorkflowStore::open(&fixture.path).expect("the next window's store");
        let journal = EffectJournal::new(&store);
        let lease = journal
            .claim_authority("main-ledger", &digest(0xf3), 40)
            .expect("the next window's authority");
        let recovered = journal.recoverable(&lease).expect("recoverable rows");
        assert_eq!(recovered.len(), 1, "the in-flight operation is offered");
        let taken = journal
            .strides_taken(&lease, &operation)
            .expect("strides read back");
        assert_eq!(
            taken
                .iter()
                .map(|one| one.stride.as_str())
                .collect::<Vec<_>>(),
            vec!["pane_cut", "seat_settled"],
            "the next window sees how far the last one got"
        );
        for permit in recovered {
            permit.abandon("fixture: this test wanted the row read, not the row moved");
        }
    }

    /// One operation's record cannot outgrow `MAX_STRIDES` — the door
    /// refuses the seventeenth word, and a writer that skips the door meets
    /// the same number again in the schema.
    #[test]
    fn a_record_cannot_outgrow_the_stride_bound() {
        let fixture = Fixture::new();
        let permit = a_prepared_lifecycle_permit(&fixture);
        for at in 0..MAX_STRIDES {
            fixture
                .journal
                .step(&fixture.lease, &permit, at, "walking", i64::from(at) + 21)
                .expect("a stride under the bound");
        }
        assert_eq!(
            fixture
                .journal
                .step(&fixture.lease, &permit, MAX_STRIDES, "walking", 40)
                .expect_err("a stride past the bound must be refused"),
            invalid("ordinal"),
        );
        /* The raw INSERT tracks `MAX_STRIDES` rather than spelling 16, so a
         * door lowered without its net — or a net without its door — turns
         * this red instead of leaving the drift green. And the refusal must
         * BE the CHECK: an insert that failed for some other reason (a
         * mistyped table, a broken binding) would pass a bare `is_err`. */
        let connection = fixture.store.connection().expect("raw connection");
        let refused = connection
            .execute(
                &format!(
                    "INSERT INTO host_effect_steps (operation_id, ordinal, step, at_ms)
                     VALUES (?1, {MAX_STRIDES}, 'walking', 41)"
                ),
                [permit.operation_id()],
            )
            .expect_err("the schema accepted an ordinal past the bound");
        assert!(
            refused.to_string().contains("CHECK"),
            "the refusal was not the ordinal CHECK: {refused}"
        );
        permit.abandon("fixture: this test wanted the row read, not the row moved");
    }
}
