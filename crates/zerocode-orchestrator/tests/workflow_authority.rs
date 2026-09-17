use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use tempfile::TempDir;
use zerocode_orchestrator::handoff::{
    CoverageGap, HandoffLineage, HandoffManifestV1, PublishBlocker, PublishPolicy, TestReceipt,
    TestRequirement,
};
use zerocode_orchestrator::publish::{
    AppliedPublication, EvidenceFailure, PublishAttempt, PublishCoordinator, PublishError,
    PublishObservation, PublishVerification, Publisher, PublisherFailure,
    TrustedTestReceiptResolver,
};
use zerocode_orchestrator::workflow::{
    NewAssignment, PublishExpectation, ReviewDecision, ReviewReceipt, TrustedReviewVerifier,
    WorkflowRecord, WorkflowState,
};
use zerocode_orchestrator::workflow_store::{
    MAX_WORKFLOW_MANIFEST_BYTES, MAX_WORKFLOW_REQUIRED_TESTS, WorkflowStore, WorkflowStoreError,
};
use zerocode_orchestrator::{GIT_EXECUTABLE, Orchestrator};

const ASSIGNEE: &str = "worker-alice";
const REVIEWER: &str = "reviewer-bob";
const PUBLISHER_SCOPE: &str = "origin:test-account/repository";
const TARGET_REF: &str = "refs/zerocode/integration/main";

struct SourceRepository {
    root: TempDir,
    base_oid: String,
    source_oid: String,
    repository_id: String,
    worktree_id: String,
    source_ref: String,
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new(GIT_EXECUTABLE)
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git output is utf8")
        .trim()
        .to_string()
}

fn source_repository() -> SourceRepository {
    let root = tempfile::tempdir().expect("source repository");
    git(root.path(), &["init", "-q"]);
    git(root.path(), &["config", "user.name", "Workflow Test"]);
    git(
        root.path(),
        &["config", "user.email", "workflow@example.invalid"],
    );
    fs::write(root.path().join("base.txt"), "base\n").expect("write base");
    git(root.path(), &["add", "base.txt"]);
    git(root.path(), &["commit", "-qm", "base"]);
    git(root.path(), &["branch", "-M", "main"]);
    let base_oid = git(root.path(), &["rev-parse", "HEAD"]);
    git(root.path(), &["switch", "-qc", "wt/workflow"]);
    fs::write(root.path().join("change.txt"), "candidate\n").expect("write candidate");
    git(root.path(), &["add", "change.txt"]);
    git(root.path(), &["commit", "-qm", "candidate"]);
    let source_oid = git(root.path(), &["rev-parse", "HEAD"]);
    let snapshot = Orchestrator::open(root.path())
        .expect("open source repository")
        .handoff_snapshot(root.path(), 900)
        .expect("source identity");
    let source_ref = format!(
        "refs/heads/{}",
        snapshot.branch.as_deref().expect("source branch")
    );
    SourceRepository {
        root,
        base_oid,
        source_oid,
        repository_id: snapshot.repository_id,
        worktree_id: snapshot.worktree_id,
        source_ref,
    }
}

fn store(root: &TempDir) -> WorkflowStore {
    let private = root.path().join("authority");
    fs::create_dir(&private).expect("private store directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(&private, fs::Permissions::from_mode(0o700))
            .expect("private store permissions");
    }
    WorkflowStore::open(private.join("workflow.sqlite")).expect("open workflow store")
}

fn assign(
    store: &WorkflowStore,
    workflow_id: &str,
    repository: &SourceRepository,
) -> WorkflowRecord {
    store
        .assign(&NewAssignment {
            workflow_id: workflow_id.to_string(),
            assignee_id: ASSIGNEE.to_string(),
            repository_id: repository.repository_id.clone(),
            worktree_id: repository.worktree_id.clone(),
            publisher_scope: PUBLISHER_SCOPE.to_string(),
            base_oid: repository.base_oid.clone(),
            source_ref: repository.source_ref.clone(),
            target_ref: TARGET_REF.to_string(),
            policy: policy(),
        })
        .expect("assign workflow")
}

fn manifest(
    workflow_id: &str,
    generation: u64,
    repository: &SourceRepository,
) -> HandoffManifestV1 {
    manifest_with_parent(workflow_id, generation, repository, None)
}

fn manifest_with_parent(
    workflow_id: &str,
    generation: u64,
    repository: &SourceRepository,
    parent_manifest_id: Option<String>,
) -> HandoffManifestV1 {
    let orchestrator = Orchestrator::open(repository.root.path()).expect("open repository");
    let snapshot = orchestrator
        .handoff_snapshot(repository.root.path(), 1_000)
        .expect("capture handoff");
    assert_eq!(snapshot.head_oid, repository.source_oid);
    HandoffManifestV1::new(
        generation,
        1_001,
        HandoffLineage {
            run_id: "run-1".to_string(),
            task_id: workflow_id.to_string(),
            dispatch_id: "dispatch-1".to_string(),
            worker_id: ASSIGNEE.to_string(),
            parent_manifest_id,
        },
        snapshot,
        Vec::new(),
    )
    .expect("build manifest")
}

fn policy() -> PublishPolicy {
    PublishPolicy {
        required_tests: vec![TestRequirement {
            name: "rust".to_string(),
            command_id: "cargo-test-orchestrator-v1".to_string(),
            must_fail_first: false,
        }],
    }
}

fn ready_workflow(
    store: &WorkflowStore,
    workflow_id: &str,
    repository: &SourceRepository,
) -> (WorkflowRecord, HandoffManifestV1) {
    let assigned = assign(store, workflow_id, repository);
    let manifest = manifest(workflow_id, assigned.generation, repository);
    let submitted = store
        .submit(workflow_id, assigned.generation, &manifest, &policy())
        .expect("submit workflow");
    let review = review_receipt(&submitted, REVIEWER, ReviewDecision::Approve);
    let reviewed = store
        .review(&review, &ReviewVerifier::new(REVIEWER))
        .expect("approve workflow");
    (reviewed, manifest)
}

fn review_receipt(
    record: &WorkflowRecord,
    reviewer_id: &str,
    decision: ReviewDecision,
) -> ReviewReceipt {
    ReviewReceipt {
        attestation_id: format!("attestation-{reviewer_id}"),
        reviewer_id: reviewer_id.to_string(),
        decision,
        binding: PublishExpectation {
            workflow_id: record.workflow_id.clone(),
            generation: record.generation,
            manifest_id: record.manifest_id.clone().expect("submitted manifest"),
            repository_id: record.repository_id.clone(),
            worktree_id: record.worktree_id.clone(),
            publisher_scope: record.publisher_scope.clone(),
            source: record.source.clone().expect("submitted source"),
            target: record.target.clone(),
            policy_digest: record.policy_digest.clone().expect("assigned policy"),
        },
    }
}

struct ReviewVerifier {
    authenticated_principal: String,
}

impl ReviewVerifier {
    fn new(principal: &str) -> Self {
        Self {
            authenticated_principal: principal.to_string(),
        }
    }
}

impl TrustedReviewVerifier for ReviewVerifier {
    fn verifies(&self, receipt: &ReviewReceipt) -> bool {
        receipt.reviewer_id == self.authenticated_principal
            && receipt.attestation_id == format!("attestation-{}", self.authenticated_principal)
    }
}

#[derive(Default)]
struct PassingReceipts;

impl TrustedTestReceiptResolver for PassingReceipts {
    fn resolve(
        &self,
        _manifest_id: &str,
        requirement: &TestRequirement,
        snapshot_digest: &str,
    ) -> Result<Option<TestReceipt>, zerocode_orchestrator::publish::EvidenceFailure> {
        Ok(Some(TestReceipt::new(
            requirement.name.clone(),
            requirement.command_id.clone(),
            snapshot_digest,
            10,
            20,
            0,
        )))
    }
}

#[derive(Default)]
struct MissingReceipts;

impl TrustedTestReceiptResolver for MissingReceipts {
    fn resolve(
        &self,
        _manifest_id: &str,
        _requirement: &TestRequirement,
        _snapshot_digest: &str,
    ) -> Result<Option<TestReceipt>, zerocode_orchestrator::publish::EvidenceFailure> {
        Ok(None)
    }
}

struct FailedReceipts(EvidenceFailure);

impl TrustedTestReceiptResolver for FailedReceipts {
    fn resolve(
        &self,
        _manifest_id: &str,
        _requirement: &TestRequirement,
        _snapshot_digest: &str,
    ) -> Result<Option<TestReceipt>, EvidenceFailure> {
        Err(self.0)
    }
}

#[derive(Clone, Copy)]
enum BadReceipt {
    WrongCommand,
    Stale,
    Failed,
}

struct BadReceipts(BadReceipt);

impl TrustedTestReceiptResolver for BadReceipts {
    fn resolve(
        &self,
        _manifest_id: &str,
        requirement: &TestRequirement,
        snapshot_digest: &str,
    ) -> Result<Option<TestReceipt>, zerocode_orchestrator::publish::EvidenceFailure> {
        let command = if matches!(self.0, BadReceipt::WrongCommand) {
            "other-command"
        } else {
            requirement.command_id.as_str()
        };
        let digest = if matches!(self.0, BadReceipt::Stale) {
            "0".repeat(64)
        } else {
            snapshot_digest.to_string()
        };
        Ok(Some(TestReceipt::new(
            requirement.name.clone(),
            command,
            digest,
            10,
            20,
            i32::from(matches!(self.0, BadReceipt::Failed)),
        )))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommitMode {
    Succeed,
    ApplyThenTimeout,
    TimeoutWithoutApply,
    DisconnectWithoutApply,
    PendingWithoutApply,
}

struct FakePublisher {
    state: Mutex<FakePublisherState>,
    commit_gate: Mutex<Option<(Arc<Barrier>, Arc<Barrier>)>>,
    verify_gate: Mutex<Option<Arc<Barrier>>>,
    commits: AtomicUsize,
    verifies: AtomicUsize,
}

struct FakePublisherState {
    observation: PublishObservation,
    mode: CommitMode,
    committed_operations: HashSet<String>,
    applied_operations: HashMap<String, String>,
    tombstoned_operations: HashSet<String>,
    target_contains_applied: bool,
}

impl FakePublisher {
    fn new(repository: &SourceRepository) -> Self {
        Self {
            state: Mutex::new(FakePublisherState {
                observation: PublishObservation {
                    repository_id: repository.repository_id.clone(),
                    publisher_scope: PUBLISHER_SCOPE.to_string(),
                    source_oid: repository.source_oid.clone(),
                    target_oid: repository.base_oid.clone(),
                    source_descends_from_target: true,
                },
                mode: CommitMode::Succeed,
                committed_operations: HashSet::new(),
                applied_operations: HashMap::new(),
                tombstoned_operations: HashSet::new(),
                target_contains_applied: false,
            }),
            commit_gate: Mutex::new(None),
            verify_gate: Mutex::new(None),
            commits: AtomicUsize::new(0),
            verifies: AtomicUsize::new(0),
        }
    }

    fn set_source(&self, oid: &str) {
        self.state
            .lock()
            .expect("publisher state lock")
            .observation
            .source_oid = oid.to_string();
    }

    fn set_target(&self, oid: &str) {
        let mut state = self.state.lock().expect("publisher state lock");
        state.observation.target_oid = oid.to_string();
        state.target_contains_applied = false;
    }

    fn set_mode(&self, mode: CommitMode) {
        self.state.lock().expect("publisher state lock").mode = mode;
    }

    fn set_fast_forward(&self, fast_forward: bool) {
        self.state
            .lock()
            .expect("publisher state lock")
            .observation
            .source_descends_from_target = fast_forward;
    }

    fn set_repository(&self, repository_id: &str) {
        self.state
            .lock()
            .expect("publisher state lock")
            .observation
            .repository_id = repository_id.to_string();
    }

    fn set_scope(&self, publisher_scope: &str) {
        self.state
            .lock()
            .expect("publisher state lock")
            .observation
            .publisher_scope = publisher_scope.to_string();
    }

    fn target_oid(&self) -> String {
        self.state
            .lock()
            .expect("publisher state lock")
            .observation
            .target_oid
            .clone()
    }

    fn gate_commit(&self, entered: Arc<Barrier>, release: Arc<Barrier>) {
        *self.commit_gate.lock().expect("commit gate lock") = Some((entered, release));
    }

    fn gate_verify(&self, entered: Arc<Barrier>) {
        *self.verify_gate.lock().expect("verify gate lock") = Some(entered);
    }

    fn advance_target(&self, oid: &str, contains_applied: bool) {
        let mut state = self.state.lock().expect("publisher state lock");
        state.observation.target_oid = oid.to_string();
        state.target_contains_applied = contains_applied;
    }

    fn complete_pending(&self) {
        let mut state = self.state.lock().expect("publisher state lock");
        let applied_oid = state.observation.source_oid.clone();
        let operations: Vec<_> = state.committed_operations.iter().cloned().collect();
        for operation in operations {
            if !state.tombstoned_operations.contains(&operation) {
                state
                    .applied_operations
                    .insert(operation, applied_oid.clone());
            }
        }
        state.observation.target_oid = applied_oid;
        state.target_contains_applied = true;
        state.mode = CommitMode::Succeed;
    }
}

impl Publisher for FakePublisher {
    fn observe(
        &self,
        _expected: &PublishExpectation,
    ) -> Result<PublishObservation, PublisherFailure> {
        Ok(self
            .state
            .lock()
            .expect("publisher state lock")
            .observation
            .clone())
    }

    fn commit(&self, attempt: &PublishAttempt<'_>) -> Result<(), PublisherFailure> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        let mut state = self.state.lock().expect("publisher state lock");
        if state
            .applied_operations
            .contains_key(attempt.operation_id())
        {
            return Ok(());
        }
        if state.tombstoned_operations.contains(attempt.operation_id()) {
            return Err(PublisherFailure::Refused);
        }
        state
            .committed_operations
            .insert(attempt.operation_id().to_string());
        let expected = attempt.expected();
        if let Some((entered, release)) = self.commit_gate.lock().expect("commit gate lock").take()
        {
            entered.wait();
            release.wait();
        }
        if state.observation.repository_id != expected.repository_id
            || state.observation.publisher_scope != expected.publisher_scope
            || state.observation.source_oid != expected.source.oid
            || state.observation.target_oid != expected.target.oid
            || !state.observation.source_descends_from_target
        {
            return Err(PublisherFailure::Refused);
        }
        match state.mode {
            CommitMode::Succeed => {
                let applied_oid = expected.source.oid.clone();
                state.observation.target_oid.clone_from(&applied_oid);
                state
                    .applied_operations
                    .insert(attempt.operation_id().to_string(), applied_oid);
                state.target_contains_applied = true;
                Ok(())
            }
            CommitMode::ApplyThenTimeout => {
                let applied_oid = expected.source.oid.clone();
                state.observation.target_oid.clone_from(&applied_oid);
                state
                    .applied_operations
                    .insert(attempt.operation_id().to_string(), applied_oid);
                state.target_contains_applied = true;
                Err(PublisherFailure::Timeout)
            }
            CommitMode::TimeoutWithoutApply => Err(PublisherFailure::Timeout),
            CommitMode::DisconnectWithoutApply => Err(PublisherFailure::Disconnected),
            CommitMode::PendingWithoutApply => Err(PublisherFailure::Timeout),
        }
    }

    fn verify(
        &self,
        attempt: &PublishAttempt<'_>,
    ) -> Result<PublishVerification, PublisherFailure> {
        self.verifies.fetch_add(1, Ordering::SeqCst);
        if let Some(entered) = self.verify_gate.lock().expect("verify gate lock").take() {
            entered.wait();
        }
        let mut state = self.state.lock().expect("publisher state lock");
        let observation = state.observation.clone();
        if let Some(applied_oid) = state
            .applied_operations
            .get(attempt.operation_id())
            .cloned()
        {
            let target_contains_applied =
                observation.target_oid == applied_oid || state.target_contains_applied;
            return Ok(PublishVerification::Applied(AppliedPublication {
                applied_oid,
                repository_id: observation.repository_id,
                publisher_scope: observation.publisher_scope,
                current_target_oid: observation.target_oid,
                target_contains_applied,
            }));
        }
        if state.mode == CommitMode::PendingWithoutApply {
            Ok(PublishVerification::Pending)
        } else {
            state
                .tombstoned_operations
                .insert(attempt.operation_id().to_string());
            Ok(PublishVerification::NotAppliedFinal(observation))
        }
    }
}

#[test]
fn duplicate_and_concurrent_assignment_mint_one_generation() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = Arc::new(store(&store_root));
    let assignment = Arc::new(NewAssignment {
        workflow_id: "task-duplicate".to_string(),
        assignee_id: ASSIGNEE.to_string(),
        repository_id: repository.repository_id.clone(),
        worktree_id: repository.worktree_id.clone(),
        publisher_scope: PUBLISHER_SCOPE.to_string(),
        base_oid: repository.base_oid.clone(),
        source_ref: repository.source_ref.clone(),
        target_ref: TARGET_REF.to_string(),
        policy: policy(),
    });
    const CONCURRENT_ASSIGNMENT_WAVES: usize = 32;
    let mut generations = HashSet::new();
    for _ in 0..CONCURRENT_ASSIGNMENT_WAVES {
        let barrier = Arc::new(Barrier::new(8));
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let store = Arc::clone(&store);
                let assignment = Arc::clone(&assignment);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    store.assign(&assignment).expect("idempotent assignment")
                })
            })
            .collect();
        generations.extend(
            threads
                .into_iter()
                .map(|thread| thread.join().expect("assignment thread").generation),
        );
    }
    assert_eq!(generations.len(), 1);

    let changed = NewAssignment {
        assignee_id: "worker-eve".to_string(),
        ..assignment.as_ref().clone()
    };
    assert!(matches!(
        store.assign(&changed),
        Err(WorkflowStoreError::ImmutableAssignment { .. })
    ));
}

#[test]
fn assignment_accepts_only_the_designated_integration_namespace() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let template = NewAssignment {
        workflow_id: "task-ref".to_string(),
        assignee_id: ASSIGNEE.to_string(),
        repository_id: repository.repository_id.clone(),
        worktree_id: repository.worktree_id.clone(),
        publisher_scope: PUBLISHER_SCOPE.to_string(),
        base_oid: repository.base_oid.clone(),
        source_ref: repository.source_ref.clone(),
        target_ref: TARGET_REF.to_string(),
        policy: policy(),
    };
    for invalid in [
        "refs/heads/",
        "refs/heads/a..b",
        "refs/heads/a b",
        "refs/heads/a~b",
        "refs/heads/a^b",
        "refs/heads/a:b",
        "refs/heads/a?b",
        "refs/heads/a*b",
        "refs/heads/a[b",
        "refs/heads/a\\b",
        "refs/heads/a@{b",
        "refs/heads/.hidden",
        "refs/heads/a.lock",
        "refs/heads/a/",
        "refs/heads/a.",
        "refs/heads/a//b",
        "refs/heads/release/feature-a_1",
    ] {
        let assignment = NewAssignment {
            target_ref: invalid.to_string(),
            ..template.clone()
        };
        assert!(matches!(
            store.assign(&assignment),
            Err(WorkflowStoreError::InvalidInput {
                field: "target_ref"
            })
        ));
    }
    let integration = NewAssignment {
        workflow_id: "task-integration-ref".to_string(),
        target_ref: "refs/zerocode/integration/run-1".to_string(),
        ..template
    };
    assert_eq!(
        store
            .assign(&integration)
            .expect("designated integration ref")
            .state,
        WorkflowState::Assigned
    );
}

#[test]
fn workflow_store_bounds_policy_and_manifest_inputs() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let oversized_policy = PublishPolicy {
        required_tests: vec![
            TestRequirement {
                name: "rust".to_string(),
                command_id: "cargo-test-v1".to_string(),
                must_fail_first: false,
            };
            MAX_WORKFLOW_REQUIRED_TESTS + 1
        ],
    };
    let assignment = NewAssignment {
        workflow_id: "task-policy-limit".to_string(),
        assignee_id: ASSIGNEE.to_string(),
        repository_id: repository.repository_id.clone(),
        worktree_id: repository.worktree_id.clone(),
        publisher_scope: PUBLISHER_SCOPE.to_string(),
        base_oid: repository.base_oid.clone(),
        source_ref: repository.source_ref.clone(),
        target_ref: TARGET_REF.to_string(),
        policy: oversized_policy,
    };
    assert!(matches!(
        store.assign(&assignment),
        Err(WorkflowStoreError::InvalidInput {
            field: "publish_policy"
        })
    ));

    let assigned = assign(&store, "task-manifest-limit", &repository);
    let candidate = manifest("task-manifest-limit", assigned.generation, &repository);
    let mut snapshot = candidate.worktree;
    snapshot.worktree_name = "x".repeat(MAX_WORKFLOW_MANIFEST_BYTES + 1);
    let oversized_manifest = HandoffManifestV1::new(
        candidate.generation,
        candidate.created_at_ms,
        candidate.lineage,
        snapshot,
        candidate.tests,
    )
    .expect("oversized manifest can be represented in memory");
    assert!(matches!(
        store.submit(
            "task-manifest-limit",
            assigned.generation,
            &oversized_manifest,
            &policy()
        ),
        Err(WorkflowStoreError::InvalidInput {
            field: "handoff_manifest"
        })
    ));
}

#[test]
fn submission_cannot_switch_repository_worktree_or_source_namespace() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    for (workflow_id, field) in [
        ("task-wrong-repo", "repository identity"),
        ("task-wrong-worktree", "worktree identity"),
        ("task-wrong-source", "source branch"),
    ] {
        let assigned = assign(&store, workflow_id, &repository);
        let candidate = manifest(workflow_id, assigned.generation, &repository);
        let mut snapshot = candidate.worktree;
        match field {
            "repository identity" => snapshot.repository_id = "repo-from-fork".to_string(),
            "worktree identity" => snapshot.worktree_id = "worktree-other".to_string(),
            "source branch" => snapshot.branch = Some("wt/other".to_string()),
            _ => unreachable!(),
        }
        let rebound = HandoffManifestV1::new(
            candidate.generation,
            candidate.created_at_ms,
            candidate.lineage,
            snapshot,
            candidate.tests,
        )
        .expect("rebound manifest");
        assert!(matches!(
            store.submit(
                workflow_id,
                assigned.generation,
                &rebound,
                &policy()
            ),
            Err(WorkflowStoreError::SubmissionBinding {
                field: actual,
                ..
            }) if actual == field
        ));
    }
}

#[test]
fn submission_binding_is_immutable_and_review_must_be_independent() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let assigned = assign(&store, "task-review", &repository);
    let manifest = manifest("task-review", assigned.generation, &repository);
    let submitted = store
        .submit("task-review", assigned.generation, &manifest, &policy())
        .expect("submit");
    let self_review = review_receipt(&submitted, ASSIGNEE, ReviewDecision::Approve);
    assert!(matches!(
        store.review(&self_review, &ReviewVerifier::new(ASSIGNEE)),
        Err(WorkflowStoreError::SelfReview { .. })
    ));
    let forged = review_receipt(&submitted, "reviewer-eve", ReviewDecision::Approve);
    assert_eq!(
        store.review(&forged, &ReviewVerifier::new(REVIEWER)),
        Err(WorkflowStoreError::UntrustedReview)
    );
    let mut rebound = review_receipt(&submitted, REVIEWER, ReviewDecision::Approve);
    rebound.binding.target.oid = "c".repeat(40);
    assert!(matches!(
        store.review(&rebound, &ReviewVerifier::new(REVIEWER)),
        Err(WorkflowStoreError::ImmutableSubmission { .. })
    ));

    let changed_policy = PublishPolicy {
        required_tests: vec![TestRequirement {
            name: "different".to_string(),
            command_id: "different-v1".to_string(),
            must_fail_first: false,
        }],
    };
    assert!(matches!(
        store.submit(
            "task-review",
            assigned.generation,
            &manifest,
            &changed_policy
        ),
        Err(WorkflowStoreError::ImmutableSubmission { .. })
    ));
    let publisher = FakePublisher::new(&repository);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    assert!(matches!(
        coordinator.prepare("task-review", assigned.generation),
        Err(PublishError::Store(WorkflowStoreError::InvalidState {
            actual: WorkflowState::Submitted,
            ..
        }))
    ));
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 0);
}

#[test]
fn changes_requested_mints_a_new_generation_and_rejects_generation_one_authority() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let assigned = assign(&store, "task-reopen", &repository);
    let generation_one = assigned.generation;
    let handoff = manifest("task-reopen", generation_one, &repository);
    let submitted = store
        .submit("task-reopen", generation_one, &handoff, &policy())
        .expect("submit generation one");
    let changes = review_receipt(&submitted, REVIEWER, ReviewDecision::RequestChanges);
    assert_eq!(
        store
            .review(&changes, &ReviewVerifier::new(REVIEWER))
            .expect("request changes")
            .state,
        WorkflowState::ChangesRequested
    );
    let reopened = store
        .reopen("task-reopen", generation_one)
        .expect("mint correction generation");
    assert_ne!(reopened.generation, generation_one);
    assert_eq!(reopened.parent_generation, Some(generation_one));
    assert_eq!(reopened.state, WorkflowState::Assigned);
    assert_eq!(reopened.base_oid, assigned.base_oid);
    assert_eq!(reopened.target, assigned.target);
    assert_eq!(reopened.policy_digest, assigned.policy_digest);
    assert!(reopened.manifest_id.is_none());
    assert!(reopened.source.is_none());
    assert!(reopened.reviewer_id.is_none());
    let archived = store
        .archived_generation("task-reopen", generation_one)
        .expect("read generation history")
        .expect("generation one was archived");
    assert_eq!(archived.manifest.manifest_id, handoff.manifest_id);
    assert_eq!(archived.reviewer_id, REVIEWER);
    assert_eq!(archived.decision, ReviewDecision::RequestChanges);

    assert!(matches!(
        store.review(&changes, &ReviewVerifier::new(REVIEWER)),
        Err(WorkflowStoreError::GenerationMismatch { .. })
    ));
    let missing_parent = manifest("task-reopen", reopened.generation, &repository);
    assert!(matches!(
        store.submit(
            "task-reopen",
            reopened.generation,
            &missing_parent,
            &policy()
        ),
        Err(WorkflowStoreError::SubmissionBinding {
            field: "parent manifest",
            ..
        })
    ));
    let correction = manifest_with_parent(
        "task-reopen",
        reopened.generation,
        &repository,
        Some(handoff.manifest_id.clone()),
    );
    assert_eq!(
        store
            .submit("task-reopen", reopened.generation, &correction, &policy())
            .expect("submit correction")
            .state,
        WorkflowState::Submitted
    );
    let publisher = FakePublisher::new(&repository);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    assert!(matches!(
        coordinator.prepare("task-reopen", generation_one),
        Err(PublishError::Store(
            WorkflowStoreError::GenerationMismatch { .. }
        ))
    ));
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 0);
}

#[test]
fn stale_source_and_target_are_refused_before_a_permit() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-stale", &repository);
    let publisher = FakePublisher::new(&repository);
    let receipts = PassingReceipts;
    let coordinator = PublishCoordinator::new(&store, &publisher, &receipts);

    publisher.set_source(&"a".repeat(40));
    assert!(matches!(
        coordinator.prepare("task-stale", workflow.generation),
        Err(PublishError::StaleSource)
    ));
    publisher.set_source(&repository.source_oid);
    publisher.set_target(&"b".repeat(40));
    assert!(matches!(
        coordinator.prepare("task-stale", workflow.generation),
        Err(PublishError::StaleTarget)
    ));
    publisher.set_target(&repository.base_oid);
    publisher.set_repository("repo-from-another-fork");
    assert!(matches!(
        coordinator.prepare("task-stale", workflow.generation),
        Err(PublishError::WrongRepository)
    ));
    publisher.set_repository(&repository.repository_id);
    publisher.set_scope("origin:other-account/repository");
    assert!(matches!(
        coordinator.prepare("task-stale", workflow.generation),
        Err(PublishError::WrongPublisherScope)
    ));
    publisher.set_scope(PUBLISHER_SCOPE);
    publisher.set_fast_forward(false);
    assert!(matches!(
        coordinator.prepare("task-stale", workflow.generation),
        Err(PublishError::NonFastForward)
    ));
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 0);
}

#[test]
fn dirty_and_incomplete_handoffs_and_untrusted_tests_block_publish() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);

    let dirty_assignment = assign(&store, "task-dirty", &repository);
    fs::write(
        repository.root.path().join("change.txt"),
        "uncommitted bytes\n",
    )
    .expect("dirty worktree");
    let dirty = manifest("task-dirty", dirty_assignment.generation, &repository);
    let dirty_submitted = store
        .submit("task-dirty", dirty_assignment.generation, &dirty, &policy())
        .expect("submit dirty");
    let dirty_review = review_receipt(&dirty_submitted, REVIEWER, ReviewDecision::Approve);
    store
        .review(&dirty_review, &ReviewVerifier::new(REVIEWER))
        .expect("review dirty");
    let publisher = FakePublisher::new(&repository);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    let error = coordinator
        .prepare("task-dirty", dirty_assignment.generation)
        .expect_err("dirty handoff blocks");
    let PublishError::Blocked(blockers) = error else {
        panic!("unexpected error: {error:?}");
    };
    assert!(blockers.contains(&PublishBlocker::DirtySnapshotNotMaterialized));

    git(repository.root.path(), &["checkout", "--", "change.txt"]);
    let gap_assignment = assign(&store, "task-gap", &repository);
    let mut gap = manifest("task-gap", gap_assignment.generation, &repository);
    gap.worktree
        .coverage
        .gaps
        .push(CoverageGap::UntrackedContent { count: 1 });
    gap = HandoffManifestV1::new(
        gap.generation,
        gap.created_at_ms,
        gap.lineage,
        gap.worktree,
        gap.tests,
    )
    .expect("rebind gap manifest");
    let gap_submitted = store
        .submit("task-gap", gap_assignment.generation, &gap, &policy())
        .expect("submit gap");
    let gap_review = review_receipt(&gap_submitted, REVIEWER, ReviewDecision::Approve);
    store
        .review(&gap_review, &ReviewVerifier::new(REVIEWER))
        .expect("review gap");
    let error = coordinator
        .prepare("task-gap", gap_assignment.generation)
        .expect_err("gap blocks");
    let PublishError::Blocked(blockers) = error else {
        panic!("unexpected error: {error:?}");
    };
    assert!(blockers.iter().any(|blocker| matches!(
        blocker,
        PublishBlocker::IncompleteSnapshot {
            gap: CoverageGap::UntrackedContent { count: 1 }
        }
    )));

    let trusted_assignment = assign(&store, "task-trust", &repository);
    let trusted = manifest("task-trust", trusted_assignment.generation, &repository);
    let forged_receipt = TestReceipt::new(
        "rust",
        "cargo-test-orchestrator-v1",
        trusted.worktree.content_digest.clone(),
        10,
        20,
        0,
    );
    let trusted = HandoffManifestV1::new(
        trusted.generation,
        trusted.created_at_ms,
        trusted.lineage,
        trusted.worktree,
        vec![forged_receipt],
    )
    .expect("manifest with submitted receipt");
    let trust_submitted = store
        .submit(
            "task-trust",
            trusted_assignment.generation,
            &trusted,
            &policy(),
        )
        .expect("submit trust");
    let trust_review = review_receipt(&trust_submitted, REVIEWER, ReviewDecision::Approve);
    store
        .review(&trust_review, &ReviewVerifier::new(REVIEWER))
        .expect("review trust");
    let coordinator = PublishCoordinator::new(&store, &publisher, &MissingReceipts);
    assert!(matches!(
        coordinator.prepare("task-trust", trusted_assignment.generation),
        Err(PublishError::Blocked(blockers))
            if blockers.contains(&PublishBlocker::RequiredTestMissing {
                name: "rust".to_string()
            })
    ));
}

#[test]
fn trusted_receipts_must_match_command_snapshot_and_success() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-bad-receipts", &repository);
    let publisher = FakePublisher::new(&repository);
    for (kind, expected) in [
        (
            BadReceipt::WrongCommand,
            PublishBlocker::UnexpectedTestCommand {
                name: "rust".to_string(),
            },
        ),
        (
            BadReceipt::Stale,
            PublishBlocker::StaleTest {
                name: "rust".to_string(),
            },
        ),
        (
            BadReceipt::Failed,
            PublishBlocker::FailedTest {
                name: "rust".to_string(),
                exit_code: 1,
            },
        ),
    ] {
        let receipts = BadReceipts(kind);
        let coordinator = PublishCoordinator::new(&store, &publisher, &receipts);
        assert!(matches!(
            coordinator.prepare("task-bad-receipts", workflow.generation),
            Err(PublishError::Blocked(blockers)) if blockers.contains(&expected)
        ));
    }
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 0);
}

#[test]
fn evidence_storage_failure_is_not_downgraded_to_a_missing_test() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-evidence-down", &repository);
    let publisher = FakePublisher::new(&repository);
    let unavailable = FailedReceipts(EvidenceFailure::Unavailable);
    let coordinator = PublishCoordinator::new(&store, &publisher, &unavailable);

    assert!(matches!(
        coordinator.prepare("task-evidence-down", workflow.generation),
        Err(PublishError::Evidence(EvidenceFailure::Unavailable))
    ));
    let changed = FailedReceipts(EvidenceFailure::CommandBinding);
    let coordinator = PublishCoordinator::new(&store, &publisher, &changed);
    assert!(matches!(
        coordinator.prepare("task-evidence-down", workflow.generation),
        Err(PublishError::Evidence(EvidenceFailure::CommandBinding))
    ));
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 0);
}

#[test]
fn concurrent_publish_attempts_call_the_publisher_once() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = Arc::new(store(&store_root));
    let (workflow, _) = ready_workflow(&store, "task-concurrent", &repository);
    let publisher = Arc::new(FakePublisher::new(&repository));
    let receipts = Arc::new(PassingReceipts);
    let barrier = Arc::new(Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let store = Arc::clone(&store);
            let publisher = Arc::clone(&publisher);
            let receipts = Arc::clone(&receipts);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let coordinator =
                    PublishCoordinator::new(store.as_ref(), publisher.as_ref(), receipts.as_ref());
                coordinator
                    .prepare("task-concurrent", workflow.generation)
                    .and_then(|permit| coordinator.publish(permit))
            })
        })
        .collect();
    let published = threads
        .into_iter()
        .filter_map(|thread| thread.join().expect("publish thread").ok())
        .filter(|record| record.state == WorkflowState::Published)
        .count();
    assert_eq!(published, 1);
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 1);
    assert_eq!(
        store.get("task-concurrent").expect("stored workflow").state,
        WorkflowState::Published
    );
}

#[test]
fn recovery_tombstones_a_dropped_live_permit_before_approving_a_retry() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-dropped-permit", &repository);
    let publisher = FakePublisher::new(&repository);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    let old_permit = coordinator
        .prepare("task-dropped-permit", workflow.generation)
        .expect("claim first permit");

    let recovered = coordinator
        .recover("task-dropped-permit", workflow.generation)
        .expect("provider tombstones uncalled operation");
    assert_eq!(recovered.state, WorkflowState::Approved);
    assert!(matches!(
        coordinator.publish(old_permit),
        Err(PublishError::Publisher {
            operation: zerocode_orchestrator::publish::PublisherOperation::Commit,
            failure: PublisherFailure::Refused
        })
    ));
    assert_eq!(publisher.target_oid(), repository.base_oid);
    assert_eq!(
        store
            .get("task-dropped-permit")
            .expect("workflow after old permit")
            .state,
        WorkflowState::Approved
    );

    let retry = coordinator
        .prepare("task-dropped-permit", workflow.generation)
        .expect("claim retry permit");
    assert_eq!(
        coordinator.publish(retry).expect("publish retry").state,
        WorkflowState::Published
    );
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 2);
}

#[test]
fn commit_claim_and_recovery_tombstone_are_one_atomic_provider_state_machine() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = Arc::new(store(&store_root));
    let (workflow, _) = ready_workflow(&store, "task-atomic-fence", &repository);
    let publisher = Arc::new(FakePublisher::new(&repository));
    let receipts = Arc::new(PassingReceipts);
    let coordinator =
        PublishCoordinator::new(store.as_ref(), publisher.as_ref(), receipts.as_ref());
    let permit = coordinator
        .prepare("task-atomic-fence", workflow.generation)
        .expect("prepare fenced operation");
    let commit_entered = Arc::new(Barrier::new(2));
    let release_commit = Arc::new(Barrier::new(2));
    publisher.gate_commit(Arc::clone(&commit_entered), Arc::clone(&release_commit));

    let publish_store = Arc::clone(&store);
    let publish_publisher = Arc::clone(&publisher);
    let publish_receipts = Arc::clone(&receipts);
    let publish_thread = thread::spawn(move || {
        PublishCoordinator::new(
            publish_store.as_ref(),
            publish_publisher.as_ref(),
            publish_receipts.as_ref(),
        )
        .publish(permit)
    });
    commit_entered.wait();

    let verify_entered = Arc::new(Barrier::new(2));
    publisher.gate_verify(Arc::clone(&verify_entered));
    let recover_store = Arc::clone(&store);
    let recover_publisher = Arc::clone(&publisher);
    let recover_receipts = Arc::clone(&receipts);
    let generation = workflow.generation;
    let recover_thread = thread::spawn(move || {
        PublishCoordinator::new(
            recover_store.as_ref(),
            recover_publisher.as_ref(),
            recover_receipts.as_ref(),
        )
        .recover("task-atomic-fence", generation)
    });
    verify_entered.wait();
    release_commit.wait();

    let publish_result = publish_thread.join().expect("publish thread");
    let recover_result = recover_thread.join().expect("recover thread");
    assert!(publish_result.is_ok() || recover_result.is_ok());
    assert_eq!(publisher.target_oid(), repository.source_oid);
    assert_eq!(
        store
            .get("task-atomic-fence")
            .expect("fenced workflow")
            .state,
        WorkflowState::Published
    );
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 1);
}

#[test]
fn target_change_after_prepare_is_refused_by_the_commit_cas() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-target-race", &repository);
    let publisher = FakePublisher::new(&repository);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    let permit = coordinator
        .prepare("task-target-race", workflow.generation)
        .expect("prepare before race");
    let raced_target = "d".repeat(40);
    publisher.set_target(&raced_target);
    assert!(matches!(
        coordinator.publish(permit),
        Err(PublishError::Publisher {
            failure: PublisherFailure::Refused,
            ..
        })
    ));
    assert_eq!(publisher.target_oid(), raced_target);
    assert_eq!(
        store
            .get("task-target-race")
            .expect("unknown workflow")
            .state,
        WorkflowState::PublishUnknown
    );
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 1);
}

#[test]
fn timeout_and_disconnect_become_unknown_and_recovery_observes_before_resume() {
    for (workflow_id, mode, expected_failure) in [
        (
            "task-timeout-applied",
            CommitMode::ApplyThenTimeout,
            PublisherFailure::Timeout,
        ),
        (
            "task-disconnected",
            CommitMode::DisconnectWithoutApply,
            PublisherFailure::Disconnected,
        ),
    ] {
        let store_root = tempfile::tempdir().expect("store root");
        let repository = source_repository();
        let store = store(&store_root);
        let (workflow, _) = ready_workflow(&store, workflow_id, &repository);
        let publisher = FakePublisher::new(&repository);
        publisher.set_mode(mode);
        let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
        let permit = coordinator
            .prepare(workflow_id, workflow.generation)
            .expect("prepare publish");
        assert_eq!(
            coordinator.publish(permit),
            Err(PublishError::PublishUnknown {
                failure: expected_failure
            })
        );
        assert_eq!(
            store.get(workflow_id).expect("unknown state").state,
            WorkflowState::PublishUnknown
        );
        assert!(matches!(
            coordinator.prepare(workflow_id, workflow.generation),
            Err(PublishError::Store(WorkflowStoreError::InvalidState {
                actual: WorkflowState::PublishUnknown,
                ..
            }))
        ));

        let recovered = coordinator
            .recover(workflow_id, workflow.generation)
            .expect("recover after observation");
        assert_eq!(publisher.verifies.load(Ordering::SeqCst), 1);
        if mode == CommitMode::ApplyThenTimeout {
            assert_eq!(recovered.state, WorkflowState::Published);
            assert_eq!(publisher.commits.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(recovered.state, WorkflowState::Approved);
            publisher.set_mode(CommitMode::Succeed);
            let permit = coordinator
                .prepare(workflow_id, workflow.generation)
                .expect("prepare after observed non-application");
            let published = coordinator.publish(permit).expect("publish retry");
            assert_eq!(published.state, WorkflowState::Published);
            assert_eq!(publisher.commits.load(Ordering::SeqCst), 2);
        }
    }
}

#[test]
fn store_and_serialized_records_do_not_disclose_private_paths() {
    let store_root = tempfile::tempdir().expect("private store root");
    let repository = source_repository();
    let private = store_root.path().join("authority");
    fs::create_dir(&private).expect("private directory");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        fs::set_permissions(&private, fs::Permissions::from_mode(0o700))
            .expect("private permissions");
    }
    let store_path = private.join("secret-workflows.sqlite");
    let store = WorkflowStore::open(&store_path).expect("open store");
    let assigned = assign(&store, "task-private", &repository);
    let handoff = manifest("task-private", assigned.generation, &repository);
    let debug = format!("{store:?} {handoff:?}");
    let serialized = serde_json::to_string(&handoff).expect("serialize handoff");
    let record_json = serde_json::to_string(&assigned).expect("serialize record");
    for private in [
        store_path.to_string_lossy(),
        repository.root.path().to_string_lossy(),
    ] {
        assert!(!debug.contains(private.as_ref()));
        assert!(!serialized.contains(private.as_ref()));
        assert!(!record_json.contains(private.as_ref()));
    }
}

#[cfg(unix)]
#[test]
fn store_refuses_symlinks_and_keeps_database_files_owner_only() {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};

    let store_root = tempfile::tempdir().expect("private store root");
    fs::set_permissions(store_root.path(), fs::Permissions::from_mode(0o700))
        .expect("private parent permissions");
    let store_path = store_root.path().join("workflow.sqlite");
    let store = WorkflowStore::open(&store_path).expect("private database");
    drop(store);
    assert_eq!(
        fs::metadata(&store_path).expect("database metadata").mode() & 0o777,
        0o600
    );
    for suffix in ["-wal", "-shm"] {
        let sidecar = store_root.path().join(format!("workflow.sqlite{suffix}"));
        if let Ok(metadata) = fs::metadata(sidecar) {
            assert_eq!(metadata.mode() & 0o777, 0o600);
        }
    }

    let victim = store_root.path().join("victim.sqlite");
    fs::write(&victim, b"do not open through a link").expect("victim");
    let linked = store_root.path().join("linked.sqlite");
    symlink(&victim, &linked).expect("database symlink");
    assert!(matches!(
        WorkflowStore::open(&linked),
        Err(WorkflowStoreError::UnsafeStorePath)
    ));

    for suffix in ["-wal", "-shm"] {
        let stem = format!("blocked{}", suffix.trim_start_matches('-'));
        let database = store_root.path().join(format!("{stem}.sqlite"));
        let sidecar = store_root.path().join(format!("{stem}.sqlite{suffix}"));
        let sidecar_victim = store_root.path().join(format!("victim{suffix}"));
        let original = format!("must survive {suffix}");
        fs::write(&sidecar_victim, &original).expect("sidecar victim");
        symlink(&sidecar_victim, &sidecar).expect("sidecar symlink");
        assert!(matches!(
            WorkflowStore::open(&database),
            Err(WorkflowStoreError::UnsafeStorePath)
        ));
        assert_eq!(
            fs::read_to_string(&sidecar_victim).expect("untouched sidecar victim"),
            original
        );
    }

    let reopen_path = store_root.path().join("reopen.sqlite");
    let reopen_store = WorkflowStore::open(&reopen_path).expect("reopen database");
    let late_sidecar = store_root.path().join("reopen.sqlite-wal");
    if fs::symlink_metadata(&late_sidecar).is_ok() {
        fs::remove_file(&late_sidecar).expect("remove SQLite's closed sidecar");
    }
    let late_victim = store_root.path().join("late-sidecar-victim");
    fs::write(&late_victim, "must survive reopen").expect("late victim");
    symlink(&late_victim, &late_sidecar).expect("late sidecar symlink");
    assert!(matches!(
        reopen_store.get("missing"),
        Err(WorkflowStoreError::UnsafeStorePath)
    ));
    assert_eq!(
        fs::read_to_string(&late_victim).expect("untouched late victim"),
        "must survive reopen"
    );

    let public_parent = tempfile::tempdir().expect("public parent");
    fs::set_permissions(public_parent.path(), fs::Permissions::from_mode(0o755))
        .expect("make parent public");
    assert!(matches!(
        WorkflowStore::open(public_parent.path().join("workflow.sqlite")),
        Err(WorkflowStoreError::UnsafeStorePath)
    ));
}

#[test]
fn timeout_without_application_can_resume_only_after_verification() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-resume", &repository);
    let publisher = FakePublisher::new(&repository);
    publisher.set_mode(CommitMode::TimeoutWithoutApply);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    let permit = coordinator
        .prepare("task-resume", workflow.generation)
        .expect("prepare");
    assert!(matches!(
        coordinator.publish(permit),
        Err(PublishError::PublishUnknown {
            failure: PublisherFailure::Timeout
        })
    ));
    let recovered = coordinator
        .recover("task-resume", workflow.generation)
        .expect("observe non-application");
    assert_eq!(recovered.state, WorkflowState::Approved);
    assert_eq!(publisher.verifies.load(Ordering::SeqCst), 1);
}

#[test]
fn a_nonterminal_remote_operation_stays_unknown_until_it_finishes() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-pending", &repository);
    let publisher = FakePublisher::new(&repository);
    publisher.set_mode(CommitMode::PendingWithoutApply);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    let permit = coordinator
        .prepare("task-pending", workflow.generation)
        .expect("prepare pending operation");
    assert!(matches!(
        coordinator.publish(permit),
        Err(PublishError::PublishUnknown {
            failure: PublisherFailure::Timeout
        })
    ));
    assert_eq!(
        coordinator.recover("task-pending", workflow.generation),
        Err(PublishError::RecoveryPending)
    );
    assert_eq!(
        store.get("task-pending").expect("pending workflow").state,
        WorkflowState::PublishUnknown
    );

    publisher.complete_pending();
    assert_eq!(
        coordinator
            .recover("task-pending", workflow.generation)
            .expect("late operation applied")
            .state,
        WorkflowState::Published
    );
    assert_eq!(publisher.commits.load(Ordering::SeqCst), 1);
}

#[test]
fn applied_receipt_survives_a_later_verified_target_advance() {
    let store_root = tempfile::tempdir().expect("store root");
    let repository = source_repository();
    let store = store(&store_root);
    let (workflow, _) = ready_workflow(&store, "task-applied-advanced", &repository);
    let publisher = FakePublisher::new(&repository);
    publisher.set_mode(CommitMode::ApplyThenTimeout);
    let coordinator = PublishCoordinator::new(&store, &publisher, &PassingReceipts);
    let permit = coordinator
        .prepare("task-applied-advanced", workflow.generation)
        .expect("prepare applied operation");
    assert!(matches!(
        coordinator.publish(permit),
        Err(PublishError::PublishUnknown {
            failure: PublisherFailure::Timeout
        })
    ));

    let later_oid = "e".repeat(40);
    publisher.advance_target(&later_oid, false);
    assert_eq!(
        coordinator.recover("task-applied-advanced", workflow.generation),
        Err(PublishError::VerificationUnknown)
    );
    assert_eq!(
        store
            .get("task-applied-advanced")
            .expect("unknown without containment proof")
            .state,
        WorkflowState::PublishUnknown
    );

    publisher.advance_target(&later_oid, true);
    assert_eq!(
        coordinator
            .recover("task-applied-advanced", workflow.generation)
            .expect("verified descendant preserves applied receipt")
            .state,
        WorkflowState::Published
    );
    assert_eq!(publisher.target_oid(), later_oid);
}
