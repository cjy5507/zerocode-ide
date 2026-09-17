//! Host boundary for journaled pane splits.
//!
//! The orchestration road's `Split` lane drives this module. It binds a host
//! effect to the exact pane term and token generation captured before
//! `Prepared`, and gives apply/recovery three truthful answers: Applied,
//! NotStarted, Unknown.

/* The LANE half of this module is live. The reconciliation half — the
 * `DurableSplitHost` re-walk, `Unknown`, `recover` — stays dormant: the boot
 * road today settles a dead window's operation as NotStarted without
 * re-walking it, and the re-walk goes live at the lifecycle cutover slice. */
#![allow(dead_code)]

use std::collections::HashMap;

use sha2::{Digest as _, Sha256};
use zerocode_core::agent_teams::{Direction, Team};
use zerocode_orchestrator::effect_journal::{
    EffectPermit, HostEffectFailure, HostEffectKind, HostEffectState,
};
use zeroize::Zeroizing;

use crate::agent_teams::{current_pane_capability, teams};

const SHA256_HEX_BYTES: usize = 64;
const MAX_COMMAND_BYTES: usize = 256 * 1024;
const MAX_TOKEN_BYTES: usize = 4 * 1024;
pub(crate) const MAX_PANE_BYTES: usize = 512;

/// Exact occupant of one pane name. A respawn may keep the name, but cannot
/// keep both this term and the generation derived from its raw capability.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PaneIncarnation {
    team: String,
    pane: String,
    term: u32,
    generation: String,
}

impl std::fmt::Debug for PaneIncarnation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PaneIncarnation")
            .field("seat", &"<opaque>")
            .field("term", &self.term)
            .field("generation", &"<opaque>")
            .finish()
    }
}

impl PaneIncarnation {
    pub(crate) fn capture(team: &str, pane: &str) -> Option<Self> {
        let held = teams();
        Self::capture_from(&held, team, pane)
    }

    fn capture_from(held: &HashMap<String, Team>, team: &str, pane: &str) -> Option<Self> {
        let term = held.get(team)?.term_of(pane)?;
        let token = Zeroizing::new(current_pane_capability(team, pane)?);
        Some(Self {
            team: team.to_string(),
            pane: pane.to_string(),
            term,
            generation: generation(team, pane, &token),
        })
    }

    /// Execute a lifecycle mutation only while this exact incarnation remains
    /// current. The callback runs under the team-table lock and must not
    /// re-enter `teams`.
    pub(crate) fn with_current<R>(
        &self,
        apply: impl FnOnce(&mut Team) -> R,
    ) -> Result<R, SplitLeaseError> {
        let mut held = teams();
        if Self::capture_from(&held, &self.team, &self.pane).as_ref() != Some(self) {
            return Err(SplitLeaseError::StaleIncarnation);
        }
        let team = held
            .get_mut(&self.team)
            .ok_or(SplitLeaseError::StaleIncarnation)?;
        Ok(apply(team))
    }

    pub(crate) fn team(&self) -> &str {
        &self.team
    }

    pub(crate) fn pane(&self) -> &str {
        &self.pane
    }

    pub(crate) const fn term(&self) -> u32 {
        self.term
    }

    pub(crate) fn generation(&self) -> &str {
        &self.generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SplitLeaseError {
    InvalidInput { field: &'static str },
    MissingIncarnation,
    DestinationOccupied,
    StaleIncarnation,
    PermitMismatch,
    HostEpochMismatch,
}

/// Captured before the effect journal mints its operation id. It is non-Clone
/// because it already owns the raw capability destined for the child.
pub(crate) struct SplitLeaseDraft {
    binding: SplitLeaseBinding,
    destination_token: Zeroizing<String>,
    command: Zeroizing<String>,
}

impl std::fmt::Debug for SplitLeaseDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitLeaseDraft")
            .field("binding", &self.binding)
            .field("destination_token", &"<redacted>")
            .field("command_bytes", &self.command.len())
            .finish()
    }
}

impl SplitLeaseDraft {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn capture(
        team_id: &str,
        from_pane: &str,
        destination_pane: &str,
        destination_token: String,
        host_epoch: String,
        direction: Direction,
        command: String,
    ) -> Result<Self, SplitLeaseError> {
        let destination_token = Zeroizing::new(destination_token);
        let command = Zeroizing::new(command);
        if destination_pane.is_empty() || destination_pane.len() > MAX_PANE_BYTES {
            return Err(invalid("destination_pane"));
        }
        if destination_token.is_empty() || destination_token.len() > MAX_TOKEN_BYTES {
            return Err(invalid("destination_token"));
        }
        if !valid_digest(&host_epoch) {
            return Err(invalid("host_epoch"));
        }
        if command.len() > MAX_COMMAND_BYTES {
            return Err(invalid("command"));
        }

        // Team first, token second: the same lock order as `forget_term`.
        let held = teams();
        let team = held
            .get(team_id)
            .ok_or(SplitLeaseError::MissingIncarnation)?;
        if team.pane(destination_pane).is_some() {
            return Err(SplitLeaseError::DestinationOccupied);
        }
        let source = PaneIncarnation::capture_from(&held, team_id, from_pane)
            .ok_or(SplitLeaseError::MissingIncarnation)?;
        let destination_generation = generation(team_id, destination_pane, &destination_token);
        let binding_digest = split_binding_digest(
            &source,
            team.leader_term,
            destination_pane,
            &destination_generation,
            &host_epoch,
            direction,
            &command,
        );
        let binding = SplitLeaseBinding {
            operation_id: String::new(),
            binding_digest,
            source,
            leader_term: team.leader_term,
            destination_pane: destination_pane.to_string(),
            destination_generation,
            host_epoch,
            direction,
        };
        Ok(Self {
            binding,
            destination_token,
            command,
        })
    }

    /// Digest passed to `EffectRequest` before `prepare` returns an operation id.
    pub(crate) fn binding_digest(&self) -> &str {
        &self.binding.binding_digest
    }

    pub(crate) fn host_epoch(&self) -> &str {
        &self.binding.host_epoch
    }

    /// On refusal the permit comes BACK beside the error, the same shape
    /// [`SplitLease::finish`] uses at the other end of this lane. It matters
    /// more HERE than anywhere in the actor: the shell cannot ask the journal
    /// to mint a row again, so a permit dropped on this side is a fence
    /// nothing in this window will ever lift.
    pub(crate) fn finalize(
        self,
        permit: EffectPermit,
    ) -> Result<SplitLease, Box<(EffectPermit, SplitLeaseError)>> {
        let Self {
            mut binding,
            destination_token,
            command,
        } = self;
        if permit.kind() != HostEffectKind::Split
            || permit.state() != HostEffectState::Prepared
            || permit.binding_digest() != binding.binding_digest
            || permit.host_epoch() != binding.host_epoch
            || !valid_digest(permit.operation_id())
        {
            return Err(Box::new((permit, SplitLeaseError::PermitMismatch)));
        }
        binding.operation_id = permit.operation_id().to_string();
        Ok(SplitLease {
            authority: SplitAuthority { binding, permit },
            destination_token,
            command,
        })
    }
}

/// Cloneable observation key; it contains no raw capability or command.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SplitLeaseBinding {
    operation_id: String,
    binding_digest: String,
    source: PaneIncarnation,
    leader_term: u32,
    destination_pane: String,
    destination_generation: String,
    host_epoch: String,
    direction: Direction,
}

impl std::fmt::Debug for SplitLeaseBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitLeaseBinding")
            .field("operation", &"<opaque>")
            .field("binding", &"<opaque>")
            .field("source", &self.source)
            .field("destination", &"<opaque>")
            .field("host_epoch", &"<opaque>")
            .field("direction", &self.direction)
            .finish()
    }
}

impl SplitLeaseBinding {
    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub(crate) fn binding_digest(&self) -> &str {
        &self.binding_digest
    }

    pub(crate) fn source(&self) -> &PaneIncarnation {
        &self.source
    }

    pub(crate) const fn leader_term(&self) -> u32 {
        self.leader_term
    }

    pub(crate) fn destination_pane(&self) -> &str {
        &self.destination_pane
    }

    pub(crate) fn host_epoch(&self) -> &str {
        &self.host_epoch
    }

    pub(crate) const fn direction(&self) -> Direction {
        self.direction
    }

    fn may_apply_in(&self, held: &HashMap<String, Team>) -> bool {
        PaneIncarnation::capture_from(held, self.source.team(), self.source.pane()).as_ref()
            == Some(&self.source)
            && held
                .get(self.source.team())
                .is_some_and(|team| team.pane(&self.destination_pane).is_none())
    }

    pub(crate) fn applied_is_current(&self, term: u32) -> bool {
        PaneIncarnation::capture(self.source.team(), &self.destination_pane)
            .is_some_and(|now| now.term == term && now.generation == self.destination_generation)
    }
}

/// One-use capability consumed by the host that spawns the pane.
pub(crate) struct SplitLease {
    authority: SplitAuthority,
    destination_token: Zeroizing<String>,
    command: Zeroizing<String>,
}

impl std::fmt::Debug for SplitLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitLease")
            .field("binding", self.authority().binding())
            .field("destination_token", &"<redacted>")
            .field("command_bytes", &self.command.len())
            .finish()
    }
}

impl SplitLease {
    pub(crate) fn binding(&self) -> &SplitLeaseBinding {
        self.authority().binding()
    }

    pub(crate) fn destination_token(&self) -> &str {
        &self.destination_token
    }

    pub(crate) fn command(&self) -> &str {
        &self.command
    }

    /// Run the backend while the source incarnation and empty destination are
    /// held under the team-table lock. The closure must not re-enter `teams`.
    pub(crate) fn apply_fenced<R>(
        &self,
        current_host_epoch: &str,
        apply: impl FnOnce(&mut Team, &SplitLeaseBinding, &str, &str) -> R,
    ) -> Result<R, SplitLeaseError> {
        let binding = self.binding();
        if current_host_epoch != binding.host_epoch {
            return Err(SplitLeaseError::HostEpochMismatch);
        }
        let mut held = teams();
        if !binding.may_apply_in(&held) {
            return Err(SplitLeaseError::StaleIncarnation);
        }
        let team = held
            .get_mut(binding.source.team())
            .ok_or(SplitLeaseError::StaleIncarnation)?;
        Ok(apply(team, binding, &self.destination_token, &self.command))
    }

    /// On refusal the lease comes BACK beside the error: a permit that
    /// cannot pair with its result must still be settled by the caller — a
    /// return type that could drop it silently was the last place the
    /// "dropped permit" fence could still be built.
    pub(crate) fn finish(
        self,
        result: SplitEffectResult,
    ) -> Result<SplitHostResult, Box<(Self, SplitLeaseError)>> {
        let Self {
            authority,
            destination_token,
            command,
        } = self;
        match authority.finish(result) {
            Ok(done) => Ok(done),
            Err(returned) => {
                let (authority, why) = *returned;
                Err(Box::new((
                    Self {
                        authority,
                        destination_token,
                        command,
                    },
                    why,
                )))
            }
        }
    }

    fn authority(&self) -> &SplitAuthority {
        &self.authority
    }

    /// This window cannot settle what it holds. Spelled, not dropped —
    /// see [`EffectPermit::abandon`].
    pub(crate) fn abandon(self, why: &'static str) {
        self.authority.abandon(why);
    }
}

/// Recovered journal permit paired back to the exact binding that prepared it.
pub(crate) struct SplitAuthority {
    binding: SplitLeaseBinding,
    permit: EffectPermit,
}

impl std::fmt::Debug for SplitAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SplitAuthority")
            .field("binding", &self.binding)
            .field("permit", &"<opaque>")
            .finish()
    }
}

impl SplitAuthority {
    pub(crate) fn recover(
        binding: SplitLeaseBinding,
        permit: EffectPermit,
    ) -> Result<Self, SplitLeaseError> {
        if permit.kind() != HostEffectKind::Split
            || !matches!(
                permit.state(),
                HostEffectState::Prepared | HostEffectState::Unknown
            )
            || permit.binding_digest() != binding.binding_digest
            || permit.host_epoch() != binding.host_epoch
            || permit.operation_id() != binding.operation_id
        {
            return Err(SplitLeaseError::PermitMismatch);
        }
        Ok(Self { binding, permit })
    }

    pub(crate) fn binding(&self) -> &SplitLeaseBinding {
        &self.binding
    }

    pub(crate) fn host_epoch_matches(&self, current: &str) -> bool {
        current == self.binding.host_epoch
    }

    pub(crate) fn finish(
        self,
        result: SplitEffectResult,
    ) -> Result<SplitHostResult, Box<(Self, SplitLeaseError)>> {
        if result.operation_id() != self.binding.operation_id
            || result
                .applied_generation()
                .is_some_and(|generation| generation != self.binding.destination_generation)
            || matches!(
                &result,
                SplitEffectResult::NotStarted {
                    tombstone_digest,
                    ..
                } if !valid_digest(tombstone_digest)
            )
        {
            return Err(Box::new((self, SplitLeaseError::PermitMismatch)));
        }
        Ok((self.permit, result))
    }

    /// Forward the journal's one honest escape. See [`EffectPermit::abandon`].
    pub(crate) fn abandon(self, why: &'static str) {
        self.permit.abandon(why);
    }
}

/// The exact non-Clone permit comes back beside the typed host observation.
pub(crate) type SplitHostResult = (EffectPermit, SplitEffectResult);

#[derive(Clone, PartialEq, Eq)]
pub(crate) enum SplitEffectResult {
    NotStarted {
        operation_id: String,
        failure: HostEffectFailure,
        tombstone_digest: String,
    },
    Applied {
        operation_id: String,
        term: u32,
        generation: String,
    },
    Unknown {
        operation_id: String,
        failure: HostEffectFailure,
    },
}

impl std::fmt::Debug for SplitEffectResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted { failure, .. } => f
                .debug_struct("SplitNotStarted")
                .field("operation", &"<opaque>")
                .field("failure", failure)
                .field("tombstone", &"<opaque>")
                .finish(),
            Self::Applied { term, .. } => f
                .debug_struct("SplitApplied")
                .field("operation", &"<opaque>")
                .field("term", term)
                .field("generation", &"<opaque>")
                .finish(),
            Self::Unknown { failure, .. } => f
                .debug_struct("SplitUnknown")
                .field("operation", &"<opaque>")
                .field("failure", failure)
                .finish(),
        }
    }
}

impl SplitEffectResult {
    pub(crate) fn not_started(
        binding: &SplitLeaseBinding,
        failure: HostEffectFailure,
        tombstone_digest: String,
    ) -> Result<Self, SplitLeaseError> {
        if !valid_digest(&tombstone_digest) {
            return Err(invalid("tombstone_digest"));
        }
        Ok(Self::NotStarted {
            operation_id: binding.operation_id().to_string(),
            failure,
            tombstone_digest,
        })
    }

    pub(crate) fn applied(binding: &SplitLeaseBinding, term: u32) -> Result<Self, SplitLeaseError> {
        if !binding.applied_is_current(term) {
            return Err(SplitLeaseError::StaleIncarnation);
        }
        Ok(Self::Applied {
            operation_id: binding.operation_id().to_string(),
            term,
            generation: binding.destination_generation.clone(),
        })
    }

    pub(crate) fn unknown(binding: &SplitLeaseBinding, failure: HostEffectFailure) -> Self {
        Self::Unknown {
            operation_id: binding.operation_id().to_string(),
            failure,
        }
    }

    pub(crate) fn operation_id(&self) -> &str {
        match self {
            Self::NotStarted { operation_id, .. }
            | Self::Applied { operation_id, .. }
            | Self::Unknown { operation_id, .. } => operation_id,
        }
    }

    pub(crate) fn applied_term(&self) -> Option<u32> {
        match self {
            Self::Applied { term, .. } => Some(*term),
            _ => None,
        }
    }

    fn applied_generation(&self) -> Option<&str> {
        match self {
            Self::Applied { generation, .. } => Some(generation),
            _ => None,
        }
    }
}

/// Missing evidence must be returned as Unknown; NotStarted requires a host
/// tombstone proving the backend was fenced before application. An Applied
/// result is historical proof bound to its original generation. Before using
/// it to touch a live pane, callers must also require `applied_is_current`.
pub(crate) trait DurableSplitHost: Send + Sync {
    fn host_epoch(&self) -> &str;
    fn apply_split(&self, lease: SplitLease) -> Result<SplitHostResult, SplitLeaseError>;
    fn verify_split(&self, authority: SplitAuthority) -> Result<SplitHostResult, SplitLeaseError>;
}

fn generation(team: &str, pane: &str, token: &str) -> String {
    digest(
        b"zerocode.orchestration.pane-generation.v1",
        &[team.as_bytes(), pane.as_bytes(), token.as_bytes()],
    )
}

fn split_binding_digest(
    source: &PaneIncarnation,
    leader_term: u32,
    destination_pane: &str,
    destination_generation: &str,
    host_epoch: &str,
    direction: Direction,
    command: &str,
) -> String {
    digest(
        b"zerocode.orchestration.split-binding.v1",
        &[
            host_epoch.as_bytes(),
            source.team.as_bytes(),
            source.pane.as_bytes(),
            &source.term.to_le_bytes(),
            source.generation.as_bytes(),
            destination_pane.as_bytes(),
            destination_generation.as_bytes(),
            &leader_term.to_le_bytes(),
            direction.as_str().as_bytes(),
            command.as_bytes(),
        ],
    )
}

pub(crate) fn digest(domain: &[u8], parts: &[&[u8]]) -> String {
    use std::fmt::Write as _;

    let mut digest = Sha256::new();
    for part in std::iter::once(domain).chain(parts.iter().copied()) {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part);
    }
    let mut encoded = String::with_capacity(SHA256_HEX_BYTES);
    for byte in digest.finalize() {
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn valid_digest(value: &str) -> bool {
    value.len() == SHA256_HEX_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

const fn invalid(field: &'static str) -> SplitLeaseError {
    SplitLeaseError::InvalidInput { field }
}

/// Test fixtures shared by the durable lanes.
///
/// One copy, because the rule these encode ("a distinct, valid projection
/// per (revision, word)") drifts the moment it is spelled twice.
#[cfg(test)]
pub(crate) mod lane_fixtures {
    /// Spreads revisions apart in `next_id` so two fixtures never collide.
    const REVISION_SPREAD: u64 = 1009;

    pub(crate) fn test_digest(byte: u8) -> String {
        format!("{byte:02x}").repeat(32)
    }

    pub(crate) fn snapshot(
        revision: u64,
        word: &str,
    ) -> zerocode_core::orchestration::LedgerProjectionV1 {
        let mut held = zerocode_core::orchestration::Ledger::new().export();
        held.next_id = revision * REVISION_SPREAD
            + word.bytes().fold(0_u64, |sum, byte| sum + u64::from(byte));
        held
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};

    use super::lane_fixtures::{snapshot, test_digest};
    use super::*;
    use crate::agent_teams::{
        forget_term, remember_pane_token, restore_pane_token, team_table_is_locked,
    };
    use zerocode_orchestrator::effect_journal::{
        AuthorityLease, BeginEffect, EffectJournal, EffectRequest,
    };
    use zerocode_orchestrator::workflow_store::WorkflowStore;

    struct PermitFactory {
        _root: tempfile::TempDir,
        journal: EffectJournal,
        authority: AuthorityLease,
    }

    impl PermitFactory {
        fn new() -> Self {
            let root = tempfile::tempdir().expect("authority root");
            let private = root.path().join("authority");
            std::fs::create_dir(&private).expect("authority directory");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700))
                    .expect("private authority directory");
            }
            let store = WorkflowStore::open(private.join("authority.sqlite")).expect("store");
            let journal = EffectJournal::new(&store);
            let authority = journal
                .claim_authority("main-ledger", &test_digest(0xe1), 0)
                .expect("claim authority");
            journal
                .import_legacy(&authority, &snapshot(0, "legacy"), &test_digest(0xaa), 1)
                .expect("legacy import");
            Self {
                _root: root,
                journal,
                authority,
            }
        }

        fn permit(&self, draft: &SplitLeaseDraft, slot: u8) -> EffectPermit {
            let request = EffectRequest::new(
                "main-ledger",
                test_digest(slot),
                test_digest(0x71),
                draft.binding_digest(),
                draft.host_epoch(),
                HostEffectKind::Split,
            )
            .expect("effect request");
            match self
                .journal
                .prepare(&self.authority, 0, &snapshot(1, "prepared"), &request, 2)
                .expect("prepare")
            {
                BeginEffect::Execute(permit) | BeginEffect::Reconcile(permit) => permit,
                other => panic!("unexpected prepare answer: {other:?}"),
            }
        }
    }

    struct Recorder {
        rows: Mutex<HashMap<String, (SplitLeaseBinding, SplitEffectResult)>>,
        calls: AtomicUsize,
        next_term: AtomicU32,
        epoch: String,
    }

    impl Recorder {
        fn new(term: u32, epoch: String) -> Self {
            Self {
                rows: Mutex::new(HashMap::new()),
                calls: AtomicUsize::new(0),
                next_term: AtomicU32::new(term),
                epoch,
            }
        }
    }

    impl DurableSplitHost for Recorder {
        fn host_epoch(&self) -> &str {
            &self.epoch
        }

        fn apply_split(&self, lease: SplitLease) -> Result<SplitHostResult, SplitLeaseError> {
            let binding = lease.binding().clone();
            let mut rows = self.rows.lock().expect("rows");
            if let Some((standing, result)) = rows.get(binding.operation_id()) {
                let result = if standing == &binding {
                    result.clone()
                } else {
                    SplitEffectResult::unknown(&binding, HostEffectFailure::Unavailable)
                };
                return lease.finish(result).map_err(|returned| returned.1);
            }
            if self.host_epoch() != binding.host_epoch() {
                let result = SplitEffectResult::unknown(&binding, HostEffectFailure::Unavailable);
                return lease.finish(result).map_err(|returned| returned.1);
            }

            let applied = lease.apply_fenced(self.host_epoch(), |team, binding, token, command| {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let term = self.next_term.fetch_add(1, Ordering::SeqCst);
                team.record_split(
                    binding.destination_pane(),
                    term,
                    binding.source().pane(),
                    binding.direction(),
                );
                let _ = remember_pane_token(
                    binding.source().team(),
                    binding.destination_pane(),
                    token.to_string(),
                );
                assert!(command.len() <= MAX_COMMAND_BYTES);
                term
            });
            let result = match applied {
                Ok(term) => SplitEffectResult::applied(&binding, term).unwrap_or_else(|_| {
                    SplitEffectResult::unknown(&binding, HostEffectFailure::Unavailable)
                }),
                Err(SplitLeaseError::StaleIncarnation) => SplitEffectResult::not_started(
                    &binding,
                    HostEffectFailure::Refused,
                    test_digest(0x91),
                )?,
                Err(SplitLeaseError::HostEpochMismatch) => {
                    SplitEffectResult::unknown(&binding, HostEffectFailure::Unavailable)
                }
                Err(error) => return Err(error),
            };
            rows.insert(
                binding.operation_id().to_string(),
                (binding.clone(), result.clone()),
            );
            lease.finish(result).map_err(|returned| returned.1)
        }

        fn verify_split(
            &self,
            authority: SplitAuthority,
        ) -> Result<SplitHostResult, SplitLeaseError> {
            let binding = authority.binding().clone();
            if !authority.host_epoch_matches(self.host_epoch()) {
                let result = SplitEffectResult::unknown(&binding, HostEffectFailure::Unavailable);
                return authority.finish(result).map_err(|returned| returned.1);
            }
            let rows = self.rows.lock().expect("rows");
            let result = match rows.get(binding.operation_id()) {
                Some((standing, result)) if standing == &binding => result.clone(),
                _ => SplitEffectResult::unknown(&binding, HostEffectFailure::Unavailable),
            };
            authority.finish(result).map_err(|returned| returned.1)
        }
    }

    fn open(base: u32) -> String {
        let id = format!("durable-split-team-{base}");
        teams().insert(id.clone(), Team::new(id.clone(), "unused", base));
        let _ = remember_pane_token(&id, "%1", format!("leader-token-{base}"));
        id
    }

    fn draft(team: &str, token: &str, command: &str) -> SplitLeaseDraft {
        SplitLeaseDraft::capture(
            team,
            "%1",
            "%2",
            token.to_string(),
            test_digest(0xe1),
            Direction::Vertical,
            command.to_string(),
        )
        .expect("draft")
    }

    #[test]
    fn term_and_token_rotation_invalidate_a_captured_incarnation() {
        let id = open(21_000);
        teams()
            .get_mut(&id)
            .expect("team")
            .record_split("%2", 21_001, "%1", Direction::Vertical);
        let _ = remember_pane_token(&id, "%2", "child-one".into());
        let first = PaneIncarnation::capture(&id, "%2").expect("incarnation");
        assert!(first.with_current(|_| ()).is_ok());

        let old = remember_pane_token(&id, "%2", "child-two".into());
        assert_eq!(
            first.with_current(|_| ()),
            Err(SplitLeaseError::StaleIncarnation)
        );
        restore_pane_token(&id, "%2", old);
        teams()
            .get_mut(&id)
            .expect("team")
            .respawn_pane("%2", 21_002);
        assert_eq!(
            first.with_current(|_| ()),
            Err(SplitLeaseError::StaleIncarnation)
        );
        assert_ne!(
            PaneIncarnation::capture(&id, "%2")
                .expect("replacement")
                .term(),
            first.term()
        );
        assert!(valid_digest(first.generation()));
        forget_term(21_000);
    }

    #[test]
    fn stale_lease_is_not_started_before_backend_application() {
        let id = open(22_000);
        let authority = PermitFactory::new();
        let captured = draft(&id, "child-token", "claude --safe");
        let binding_digest = captured.binding_digest().to_string();
        let permit = authority.permit(&captured, 0x31);
        let operation_id = permit.operation_id().to_string();

        // A permit prepared for another binding cannot be relabelled by giving
        // its operation id to this draft.
        let mismatched = draft(&id, "other-child-token", "codex --different");
        let mismatch_permit = authority.permit(&captured, 0x31);
        let refused = *mismatched
            .finalize(mismatch_permit)
            .expect_err("a mismatched permit must not bind a lease");
        assert_eq!(refused.1, SplitLeaseError::PermitMismatch);
        refused
            .0
            .abandon("fixture: this test wanted the refusal, not the row");

        let lease = captured.finalize(permit).expect("lease");
        let _ = remember_pane_token(&id, "%1", "rotated-leader".into());
        let host = Recorder::new(22_100, test_digest(0xe1));
        let result = host.apply_split(lease).expect("typed host result");
        assert!(matches!(
            &result.1,
            SplitEffectResult::NotStarted {
                failure: HostEffectFailure::Refused,
                ..
            }
        ));
        assert_eq!(host.calls.load(Ordering::SeqCst), 0);
        assert_eq!(result.1.operation_id(), operation_id);
        assert!(valid_digest(&binding_digest));
        forget_term(22_000);

        // A correctly paired lease from an older host epoch is Unknown, and
        // cannot reach the backend owned by the replacement host.
        let epoch_team = open(22_010);
        let epoch_authority = PermitFactory::new();
        let epoch_draft = draft(&epoch_team, "epoch-child", "claude");
        let epoch_permit = epoch_authority.permit(&epoch_draft, 0x32);
        let epoch_lease = epoch_draft.finalize(epoch_permit).expect("epoch lease");
        let replacement = Recorder::new(22_110, test_digest(0xe2));
        let unknown = replacement
            .apply_split(epoch_lease)
            .expect("unknown host result");
        assert!(matches!(&unknown.1, SplitEffectResult::Unknown { .. }));
        assert_eq!(replacement.calls.load(Ordering::SeqCst), 0);
        result
            .0
            .abandon("fixture: this test wanted the row read, not the row moved");
        unknown
            .0
            .abandon("fixture: this test wanted the row read, not the row moved");
        forget_term(22_010);
    }

    #[test]
    fn fenced_apply_holds_the_incarnation_lock_through_the_backend_effect() {
        let id = open(22_500);
        let authority = PermitFactory::new();
        let captured = draft(&id, "fenced-child", "claude");
        let permit = authority.permit(&captured, 0x35);
        let lease = captured.finalize(permit).expect("lease");
        let binding = lease.binding().clone();
        let term = lease
            .apply_fenced(&test_digest(0xe1), |team, binding, token, _command| {
                assert!(
                    team_table_is_locked(),
                    "source check and backend effect were separated by an unlocked window"
                );
                team.record_split(
                    binding.destination_pane(),
                    22_600,
                    binding.source().pane(),
                    binding.direction(),
                );
                let _ = remember_pane_token(
                    binding.source().team(),
                    binding.destination_pane(),
                    token.to_string(),
                );
                22_600
            })
            .expect("fenced application");
        assert!(SplitEffectResult::applied(&binding, term).is_ok());
        lease.abandon("fixture: this test wanted the row read, not the row moved");
        forget_term(22_500);
    }

    #[test]
    fn concurrent_equivalent_leases_apply_once_and_verify_exact_generation() {
        let id = open(23_000);
        let authority = PermitFactory::new();
        let leases: Vec<_> = (0..8)
            .map(|_| {
                let draft = draft(&id, "one-child-token", "codex --safe");
                let permit = authority.permit(&draft, 0x32);
                draft.finalize(permit).expect("lease")
            })
            .collect();
        let binding = leases[0].binding().clone();
        let verification_draft = draft(&id, "one-child-token", "codex --safe");
        let permits: Vec<_> = (0..4)
            .map(|_| authority.permit(&verification_draft, 0x32))
            .collect();
        let mut permits = permits.into_iter();
        let host = Arc::new(Recorder::new(23_100, test_digest(0xe1)));
        let before =
            SplitAuthority::recover(binding.clone(), permits.next().expect("pre-apply permit"))
                .expect("pre-apply authority");
        let observed = host.verify_split(before).expect("pre-apply observation");
        assert!(matches!(&observed.1, SplitEffectResult::Unknown { .. }));
        observed
            .0
            .abandon("fixture: this test wanted the refusal, not the row");
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = leases
            .into_iter()
            .map(|lease| {
                let host = Arc::clone(&host);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    host.apply_split(lease).expect("host result")
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("thread"))
            .collect();
        assert_eq!(host.calls.load(Ordering::SeqCst), 1);
        assert!(results.windows(2).all(|pair| pair[0].1 == pair[1].1));
        let expected = results[0].1.clone();
        let after =
            SplitAuthority::recover(binding.clone(), permits.next().expect("post-apply permit"))
                .expect("post-apply authority");
        let observed_after = host.verify_split(after).expect("post-apply observation");
        assert_eq!(observed_after.1, expected);
        observed_after
            .0
            .abandon("fixture: this test wanted the row read, not the row moved");

        let applied_term = expected.applied_term().expect("applied term");
        let old = remember_pane_token(&id, "%2", "replacement-child".into());
        let rotated =
            SplitAuthority::recover(binding.clone(), permits.next().expect("rotated permit"))
                .expect("rotated authority");
        let observed_rotated = host.verify_split(rotated).expect("rotated observation");
        assert_eq!(observed_rotated.1, expected);
        observed_rotated
            .0
            .abandon("fixture: this test wanted the row read, not the row moved");
        assert!(!binding.applied_is_current(applied_term));
        restore_pane_token(&id, "%2", old);
        teams()
            .get_mut(&id)
            .expect("team")
            .respawn_pane("%2", 23_101);
        let respawned =
            SplitAuthority::recover(binding.clone(), permits.next().expect("respawn permit"))
                .expect("respawn authority");
        let observed_respawn = host.verify_split(respawned).expect("respawn observation");
        assert_eq!(observed_respawn.1, expected);
        observed_respawn
            .0
            .abandon("fixture: this test wanted the row read, not the row moved");
        assert!(!binding.applied_is_current(applied_term));
        forget_term(23_000);
        /* Whatever this race never reached for is said, not dropped — the
         * leftovers of a fixture are still capabilities. */
        for permit in permits {
            permit.abandon("fixture: a spare permit this race never needed");
        }
        /* Eight threads each came back with a permit beside its result. The
         * race is what this test asks about; the capabilities are what it
         * has to account for. */
        for one in results {
            one.0
                .abandon("fixture: one racer's permit, kept only to compare results");
        }
    }

    #[test]
    fn lease_inputs_are_bounded_and_debug_is_redacted() {
        let id = open(24_000);
        let token = "private-child-token";
        let command = "/private/project claude --token secret";
        let authority = PermitFactory::new();
        let captured = draft(&id, token, command);
        let binding_digest = captured.binding_digest().to_string();
        let draft_debug = format!("{captured:?}");
        let permit = authority.permit(&captured, 0x33);
        let operation = permit.operation_id().to_string();
        let lease = captured.finalize(permit).expect("lease");
        let binding = lease.binding().clone();
        let lease_debug = format!("{lease:?}");
        let tombstone = test_digest(0x92);
        let applied = Recorder::new(24_100, test_digest(0xe1))
            .apply_split(lease)
            .expect("host result");
        let results = [
            applied.1.clone(),
            SplitEffectResult::unknown(&binding, HostEffectFailure::Panicked),
            SplitEffectResult::not_started(
                &binding,
                HostEffectFailure::Unavailable,
                tombstone.clone(),
            )
            .expect("not started"),
        ];
        let debug = format!("{draft_debug} {lease_debug} {binding:?} {applied:?} {results:?}");
        for private in [
            token,
            command,
            id.as_str(),
            operation.as_str(),
            tombstone.as_str(),
            binding_digest.as_str(),
        ] {
            assert!(!debug.contains(private), "Debug disclosed `{private}`");
        }
        assert!(valid_digest(binding.binding_digest()));
        assert_eq!(binding.leader_term(), 24_000);
        assert!(valid_digest(binding.host_epoch()));
        let (returned, returned_result) = applied;
        assert_eq!(returned.operation_id(), operation);
        assert_eq!(returned_result, results[0]);
        returned.abandon("fixture: this test wanted the row read, not the row moved");
        assert!(matches!(
            SplitLeaseDraft::capture(
                &id,
                "%1",
                "%3",
                token.into(),
                test_digest(0xe1),
                Direction::Vertical,
                "x".repeat(MAX_COMMAND_BYTES + 1),
            ),
            Err(SplitLeaseError::InvalidInput { field: "command" })
        ));
        forget_term(24_000);
    }
}
