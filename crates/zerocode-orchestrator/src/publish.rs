//! One-shot publication after durable assignment, submission, and review.
//!
//! This is the authority kernel, not a permissive fallback: production stays
//! unwired until its Git host supplies an atomic [`Publisher`], its review
//! service supplies a trusted verifier, and its test runner supplies
//! [`TrustedTestReceiptResolver`]. The fake adapters live only in tests.

use crate::handoff::{
    HandoffManifestV1, PublishBlocker, PublishPolicy, TestReceipt, TestRequirement,
};
use crate::workflow::{PublishExpectation, WorkflowRecord, WorkflowState};
use crate::workflow_store::{WorkflowStore, WorkflowStoreError};

/// The publisher's current view of the two refs in an expectation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishObservation {
    pub repository_id: String,
    pub publisher_scope: String,
    pub source_oid: String,
    pub target_oid: String,
    pub source_descends_from_target: bool,
}

/// Immutable result for one operation id plus the publisher's current view.
/// If the target advanced after this operation, `target_contains_applied`
/// proves the current target still descends from `applied_oid`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedPublication {
    pub applied_oid: String,
    pub repository_id: String,
    pub publisher_scope: String,
    pub current_target_oid: String,
    pub target_contains_applied: bool,
}

/// Terminal status of one store-minted publisher operation id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishVerification {
    Applied(AppliedPublication),
    /// The provider guarantees this operation id is terminal and can no longer
    /// mutate the target; every later commit carrying it must be refused. An
    /// unchanged observation alone is not sufficient.
    NotAppliedFinal(PublishObservation),
    Pending,
}

/// Failures have bounded categories so a tool cannot copy a private checkout
/// path into a durable workflow error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PublisherFailure {
    #[error("the publisher refused the compare-and-swap")]
    Refused,
    #[error("the publisher timed out")]
    Timeout,
    #[error("the publisher disconnected")]
    Disconnected,
    #[error("the publisher is unavailable")]
    Unavailable,
}

/// A target adapter. `commit` must compare both refs, prove ancestry, and may
/// only fast-forward the exact target. The operation id must be idempotent and
/// queryable after timeout or disconnect.
pub trait Publisher: Send + Sync {
    fn observe(
        &self,
        expected: &PublishExpectation,
    ) -> Result<PublishObservation, PublisherFailure>;

    fn commit(&self, attempt: &PublishAttempt<'_>) -> Result<(), PublisherFailure>;

    fn verify(&self, attempt: &PublishAttempt<'_>)
    -> Result<PublishVerification, PublisherFailure>;
}

/// Borrowed provider command. Its operation id is deliberately redacted.
pub struct PublishAttempt<'a> {
    expected: &'a PublishExpectation,
    operation_id: &'a str,
}

impl std::fmt::Debug for PublishAttempt<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<opaque-publish-attempt>")
    }
}

impl<'a> PublishAttempt<'a> {
    #[must_use]
    pub const fn expected(&self) -> &'a PublishExpectation {
        self.expected
    }

    #[must_use]
    pub const fn operation_id(&self) -> &'a str {
        self.operation_id
    }

    pub(crate) fn new(expected: &'a PublishExpectation, operation_id: &'a str) -> Self {
        Self {
            expected,
            operation_id,
        }
    }
}

/// Required test receipts come from a trusted execution store, never from the
/// submitting manifest alone. Implementations receive the complete binding
/// they must resolve and the coordinator validates the returned receipt again.
pub trait TrustedTestReceiptResolver: Send + Sync {
    fn resolve(
        &self,
        manifest_id: &str,
        requirement: &TestRequirement,
        snapshot_digest: &str,
    ) -> Result<Option<TestReceipt>, EvidenceFailure>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum EvidenceFailure {
    #[error("trusted test evidence storage is unavailable")]
    Unavailable,
    #[error("trusted test evidence is corrupt")]
    Corrupt,
    #[error("the trusted test command is absent or changed under its command id")]
    CommandBinding,
}

/// A capability minted only by the store's `Approved -> Publishing` CAS.
///
/// It intentionally implements neither `Clone` nor serialization, and all
/// fields are private. [`PublishCoordinator::publish`] consumes it.
pub struct PublishPermit {
    expectation: PublishExpectation,
    attempt: u64,
    revision: u64,
    operation_id: String,
}

impl std::fmt::Debug for PublishPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<opaque-publish-permit>")
    }
}

impl PublishPermit {
    pub(crate) fn new(
        expectation: PublishExpectation,
        attempt: u64,
        revision: u64,
        operation_id: String,
    ) -> Self {
        Self {
            expectation,
            attempt,
            revision,
            operation_id,
        }
    }

    pub(crate) fn expectation(&self) -> &PublishExpectation {
        &self.expectation
    }

    pub(crate) const fn attempt(&self) -> u64 {
        self.attempt
    }

    pub(crate) const fn revision(&self) -> u64 {
        self.revision
    }

    pub(crate) fn operation_id(&self) -> &str {
        &self.operation_id
    }

    fn attempt_descriptor(&self) -> PublishAttempt<'_> {
        PublishAttempt::new(&self.expectation, &self.operation_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherOperation {
    Observe,
    Commit,
    Verify,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PublishError {
    #[error(transparent)]
    Store(#[from] WorkflowStoreError),
    #[error("workflow publication is blocked by its evidence")]
    Blocked(Vec<PublishBlocker>),
    #[error(transparent)]
    Evidence(#[from] EvidenceFailure),
    #[error("the source ref is stale")]
    StaleSource,
    #[error("the target ref is stale")]
    StaleTarget,
    #[error("the publisher observed a different repository")]
    WrongRepository,
    #[error("the publisher observed a different host/account scope")]
    WrongPublisherScope,
    #[error("the source is not a descendant of the assigned target base")]
    NonFastForward,
    #[error("publisher {operation:?} failed: {failure}")]
    Publisher {
        operation: PublisherOperation,
        failure: PublisherFailure,
    },
    #[error("publication outcome is unknown after {failure}")]
    PublishUnknown { failure: PublisherFailure },
    #[error("publication completed but verification did not prove the exact target")]
    VerificationUnknown,
    #[error("the prior publisher operation has not reached a terminal outcome")]
    RecoveryPending,
}

/// Coordinates the only path from reviewed evidence to a target mutation.
pub struct PublishCoordinator<'a, P, R> {
    store: &'a WorkflowStore,
    publisher: &'a P,
    receipts: &'a R,
}

impl<'a, P, R> PublishCoordinator<'a, P, R>
where
    P: Publisher,
    R: TrustedTestReceiptResolver,
{
    #[must_use]
    pub const fn new(store: &'a WorkflowStore, publisher: &'a P, receipts: &'a R) -> Self {
        Self {
            store,
            publisher,
            receipts,
        }
    }

    /// Validate evidence and current refs, then atomically claim the one permit
    /// for this attempt. External mutation has not happened when this returns.
    pub fn prepare(
        &self,
        workflow_id: &str,
        generation: u64,
    ) -> Result<PublishPermit, PublishError> {
        let material = self.store.publish_material(workflow_id, generation)?;
        if material.record.state != WorkflowState::Approved {
            return Err(WorkflowStoreError::InvalidState {
                workflow_id: workflow_id.to_string(),
                expected: WorkflowState::Approved,
                actual: material.record.state,
            }
            .into());
        }
        let blockers = publication_blockers(&material.manifest, &material.policy, self.receipts)?;
        if !blockers.is_empty() {
            return Err(PublishError::Blocked(blockers));
        }
        let expectation = material.expectation()?;
        let observation =
            self.publisher
                .observe(&expectation)
                .map_err(|failure| PublishError::Publisher {
                    operation: PublisherOperation::Observe,
                    failure,
                })?;
        require_current_refs(&expectation, &observation)?;
        self.store.claim_publish(&expectation).map_err(Into::into)
    }

    /// Consume a one-shot permit. Any ambiguous call or verification result is
    /// made durable as `PublishUnknown`; it can only continue through recover.
    pub fn publish(&self, permit: PublishPermit) -> Result<WorkflowRecord, PublishError> {
        match self.publisher.commit(&permit.attempt_descriptor()) {
            Ok(()) => self.verify_committed(permit),
            Err(PublisherFailure::Refused) => self.verify_refused(permit),
            Err(
                failure @ (PublisherFailure::Timeout
                | PublisherFailure::Disconnected
                | PublisherFailure::Unavailable),
            ) => {
                let _ = self
                    .store
                    .settle_permit(&permit, WorkflowState::PublishUnknown);
                Err(PublishError::PublishUnknown { failure })
            }
        }
    }

    /// Resolve an unknown or stranded operation by its stable provider id.
    /// `NotAppliedFinal` also tombstones that id at the provider, so a live old
    /// permit cannot mutate the target after recovery approves a retry.
    pub fn recover(
        &self,
        workflow_id: &str,
        generation: u64,
    ) -> Result<WorkflowRecord, PublishError> {
        let material = self.store.publish_material(workflow_id, generation)?;
        if !matches!(
            material.record.state,
            WorkflowState::Publishing | WorkflowState::PublishUnknown
        ) {
            return Err(WorkflowStoreError::InvalidState {
                workflow_id: workflow_id.to_string(),
                expected: WorkflowState::PublishUnknown,
                actual: material.record.state,
            }
            .into());
        }
        let expectation = material.expectation()?;
        let operation_id = material
            .publish_nonce
            .as_deref()
            .ok_or(WorkflowStoreError::Corrupt)?;
        let attempt = PublishAttempt::new(&expectation, operation_id);
        match self
            .publisher
            .verify(&attempt)
            .map_err(|failure| PublishError::Publisher {
                operation: PublisherOperation::Verify,
                failure,
            })? {
            PublishVerification::Applied(applied) => {
                require_published(&expectation, &applied)?;
                self.store
                    .settle_recovery(&material, WorkflowState::Published)
                    .map_err(Into::into)
            }
            PublishVerification::NotAppliedFinal(observation) => {
                require_current_refs(&expectation, &observation)?;
                self.store
                    .settle_recovery(&material, WorkflowState::Approved)
                    .map_err(Into::into)
            }
            PublishVerification::Pending => Err(PublishError::RecoveryPending),
        }
    }

    fn verify_committed(&self, permit: PublishPermit) -> Result<WorkflowRecord, PublishError> {
        let verification = self.publisher.verify(&permit.attempt_descriptor());
        if let Ok(PublishVerification::Applied(applied)) = verification
            && require_published(permit.expectation(), &applied).is_ok()
        {
            return self
                .store
                .settle_permit(&permit, WorkflowState::Published)
                .map_err(Into::into);
        }
        let _ = self
            .store
            .settle_permit(&permit, WorkflowState::PublishUnknown);
        Err(PublishError::VerificationUnknown)
    }

    fn verify_refused(&self, permit: PublishPermit) -> Result<WorkflowRecord, PublishError> {
        match self.publisher.verify(&permit.attempt_descriptor()) {
            Ok(PublishVerification::Applied(applied))
                if require_published(permit.expectation(), &applied).is_ok() =>
            {
                self.store
                    .settle_permit(&permit, WorkflowState::Published)
                    .map_err(Into::into)
            }
            Ok(PublishVerification::NotAppliedFinal(_)) => {
                let _ = self
                    .store
                    .settle_permit(&permit, WorkflowState::PublishUnknown);
                Err(PublishError::Publisher {
                    operation: PublisherOperation::Commit,
                    failure: PublisherFailure::Refused,
                })
            }
            Ok(PublishVerification::Applied(_) | PublishVerification::Pending) | Err(_) => {
                let _ = self
                    .store
                    .settle_permit(&permit, WorkflowState::PublishUnknown);
                Err(PublishError::VerificationUnknown)
            }
        }
    }
}

fn publication_blockers<R: TrustedTestReceiptResolver>(
    manifest: &HandoffManifestV1,
    policy: &PublishPolicy,
    receipts: &R,
) -> Result<Vec<PublishBlocker>, EvidenceFailure> {
    // Passing an empty policy checks schema, manifest identity, coverage, Git
    // operation, conflicts, and materialization without trusting submitted
    // test rows. Required tests are resolved below from the trusted store.
    let mut blockers = manifest.structural_publish_blockers(&PublishPolicy::default());
    for required in &policy.required_tests {
        let Some(receipt) = receipts.resolve(
            &manifest.manifest_id,
            required,
            &manifest.worktree.content_digest,
        )?
        else {
            blockers.push(PublishBlocker::RequiredTestMissing {
                name: required.name.clone(),
            });
            continue;
        };
        if receipt.name != required.name || receipt.command_id != required.command_id {
            blockers.push(PublishBlocker::UnexpectedTestCommand {
                name: required.name.clone(),
            });
        } else if receipt.snapshot_digest != manifest.worktree.content_digest {
            blockers.push(PublishBlocker::StaleTest {
                name: required.name.clone(),
            });
        } else if receipt.exit_code != 0 {
            blockers.push(PublishBlocker::FailedTest {
                name: required.name.clone(),
                exit_code: receipt.exit_code,
            });
        }
    }
    Ok(blockers)
}

fn require_current_refs(
    expected: &PublishExpectation,
    observed: &PublishObservation,
) -> Result<(), PublishError> {
    require_scope(expected, observed)?;
    if observed.source_oid != expected.source.oid {
        return Err(PublishError::StaleSource);
    }
    if observed.target_oid != expected.target.oid {
        return Err(PublishError::StaleTarget);
    }
    if !observed.source_descends_from_target {
        return Err(PublishError::NonFastForward);
    }
    Ok(())
}

fn require_published(
    expected: &PublishExpectation,
    applied: &AppliedPublication,
) -> Result<(), PublishError> {
    if applied.repository_id != expected.repository_id {
        return Err(PublishError::WrongRepository);
    }
    if applied.publisher_scope != expected.publisher_scope {
        return Err(PublishError::WrongPublisherScope);
    }
    if applied.applied_oid == expected.source.oid
        && (applied.current_target_oid == applied.applied_oid || applied.target_contains_applied)
    {
        Ok(())
    } else {
        Err(PublishError::VerificationUnknown)
    }
}

fn require_scope(
    expected: &PublishExpectation,
    observed: &PublishObservation,
) -> Result<(), PublishError> {
    if observed.repository_id != expected.repository_id {
        return Err(PublishError::WrongRepository);
    }
    if observed.publisher_scope != expected.publisher_scope {
        return Err(PublishError::WrongPublisherScope);
    }
    Ok(())
}
