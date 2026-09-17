//! A bounded, stable worktree observation for handing work to another agent.
//!
//! The orchestration ledger knows who owns a task. Git knows what that owner
//! actually changed. This module is the seam between them: it records Git facts
//! without touching the user's branch or index, and refuses to call a changing
//! or only-partly-read checkout publishable.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zerocode_core::conflict::ConflictOperation;

use crate::{Orchestrator, OrchestratorError, PendingLoss, StatusEntry, Worktree};

/// The external handoff representation. Increment before changing its meaning.
pub const HANDOFF_SCHEMA_VERSION: u16 = 1;

/// A normal snapshot reads at most this many changed files.
pub const SNAPSHOT_CHANGED_PATH_LIMIT: usize = 4_096;

/// Changed-file bytes hashed by one observation. Larger work remains visible,
/// but incomplete and therefore ineligible for automatic publication.
pub const SNAPSHOT_CONTENT_BYTE_LIMIT: u64 = 64 * 1024 * 1024;

/// Bound Git metadata such as the index listing before it reaches a manifest.
pub const SNAPSHOT_GIT_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;

/// Error text is bounded separately from Git data.
pub const SNAPSHOT_GIT_STDERR_LIMIT: usize = 1024 * 1024;

/// A read-only Git observation must not hold an orchestration worker forever.
pub const SNAPSHOT_GIT_TIMEOUT: Duration = Duration::from_secs(8);

const SNAPSHOT_GIT_POLL: Duration = Duration::from_millis(10);

/// Admission limits for one worktree observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotLimits {
    pub changed_paths: usize,
    pub content_bytes: u64,
    pub git_output_bytes: usize,
    pub git_stderr_bytes: usize,
    pub git_timeout: Duration,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            changed_paths: SNAPSHOT_CHANGED_PATH_LIMIT,
            content_bytes: SNAPSHOT_CONTENT_BYTE_LIMIT,
            git_output_bytes: SNAPSHOT_GIT_OUTPUT_LIMIT,
            git_stderr_bytes: SNAPSHOT_GIT_STDERR_LIMIT,
            git_timeout: SNAPSHOT_GIT_TIMEOUT,
        }
    }
}

impl SnapshotLimits {
    fn clamped(self) -> Self {
        Self {
            changed_paths: self.changed_paths.min(SNAPSHOT_CHANGED_PATH_LIMIT),
            content_bytes: self.content_bytes.min(SNAPSHOT_CONTENT_BYTE_LIMIT),
            git_output_bytes: self.git_output_bytes.min(SNAPSHOT_GIT_OUTPUT_LIMIT),
            git_stderr_bytes: self.git_stderr_bytes.min(SNAPSHOT_GIT_STDERR_LIMIT),
            git_timeout: self.git_timeout.min(SNAPSHOT_GIT_TIMEOUT),
        }
    }
}

/// Why an observation cannot claim to cover all worktree content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CoverageGap {
    IgnoredContent {
        count: usize,
    },
    UntrackedContent {
        count: usize,
    },
    ChangedPathLimit {
        found: usize,
        limit: usize,
    },
    ContentByteLimit {
        source: String,
        bytes: u64,
        limit: u64,
    },
    DirtySubmodule {
        path: String,
    },
    IndexHiddenContent {
        path: String,
        flag: HiddenIndexFlag,
    },
}

/// Index flags that ask normal status/diff commands to hide worktree bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HiddenIndexFlag {
    AssumeUnchanged,
    SkipWorktree,
    Both,
}

/// What was and was not covered by the content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCoverage {
    pub changed_paths: usize,
    pub content_bytes: u64,
    pub gaps: Vec<CoverageGap>,
}

impl SnapshotCoverage {
    /// Only a gap-free observation may be treated as complete evidence.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.gaps.is_empty()
    }
}

/// A ref resolved at one named observation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefObservation {
    pub name: String,
    pub oid: String,
    pub observed_at_ms: i64,
}

/// A Git operation known to own the checkout, if one does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GitOperation {
    Merge,
    Rebase,
    CherryPick,
    /// Unmerged paths exist, but none of the three known operation markers do.
    Other,
}

/// The submodule portion of one changed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffSubmodule {
    pub commit_changed: bool,
    pub tracked_changes: bool,
    pub untracked_changes: bool,
}

/// One path in a handoff, with the index and worktree columns kept separate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffChange {
    pub code: String,
    pub path: String,
    pub origin: Option<String>,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
    pub submodule: Option<HandoffSubmodule>,
}

impl From<&StatusEntry> for HandoffChange {
    fn from(entry: &StatusEntry) -> Self {
        let untracked = entry.code == "??";
        let mut columns = entry.code.chars();
        let index = columns.next().unwrap_or(' ');
        let worktree = columns.next().unwrap_or(' ');
        Self {
            code: entry.code.clone(),
            path: entry.path.clone(),
            origin: entry.origin.clone(),
            staged: !untracked && index != ' ',
            unstaged: !untracked && worktree != ' ',
            untracked,
            conflicted: is_conflict_code(&entry.code),
            submodule: entry.submodule.map(|submodule| HandoffSubmodule {
                commit_changed: submodule.commit_changed,
                tracked_changes: submodule.tracked_changes,
                untracked_changes: submodule.untracked_changes,
            }),
        }
    }
}

/// Local routing metadata deliberately redacted from serialization and Debug.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PrivateWorktreePath(PathBuf);

impl PrivateWorktreePath {
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl std::fmt::Debug for PrivateWorktreePath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<private-worktree-path>")
    }
}

impl From<PathBuf> for PrivateWorktreePath {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

/// Facts safe to carry between agents. `private_path` remains local-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeSnapshot {
    pub schema_version: u16,
    pub observed_at_ms: i64,
    /// Machine-local opaque id derived from the repository administration dir.
    pub repository_id: String,
    /// Stable across `git worktree move`, but intentionally machine-local.
    pub worktree_id: String,
    pub worktree_name: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub locked: bool,
    pub head_oid: String,
    pub upstream: Option<RefObservation>,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub operation: Option<GitOperation>,
    pub changes: Vec<HandoffChange>,
    /// Names only. Ignored contents are never copied or hashed automatically.
    pub ignored: Vec<String>,
    pub content_digest: String,
    pub coverage: SnapshotCoverage,
    /// Used by the local executor, never serialized into an agent message.
    #[serde(skip)]
    pub private_path: Option<PrivateWorktreePath>,
}

impl WorktreeSnapshot {
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        !self.changes.is_empty() || !self.ignored.is_empty()
    }

    #[must_use]
    pub fn has_unmerged_paths(&self) -> bool {
        self.changes.iter().any(|change| change.conflicted)
    }
}

/// Orchestration lineage that gives the Git facts an owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffLineage {
    pub run_id: String,
    pub task_id: String,
    pub dispatch_id: String,
    pub worker_id: String,
    pub parent_manifest_id: Option<String>,
}

/// A test result tied to exactly one content digest. The command is represented
/// by a configured, non-secret id; arbitrary argv never enters the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestReceipt {
    pub name: String,
    /// Stable, non-secret id of a configured test command.
    pub command_id: String,
    pub snapshot_digest: String,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub exit_code: i32,
    /// Opaque private-store id, never a local filesystem path.
    pub output_artifact_id: Option<String>,
    pub output_truncated: bool,
}

impl TestReceipt {
    pub fn new(
        name: impl Into<String>,
        command_id: impl Into<String>,
        snapshot_digest: impl Into<String>,
        started_at_ms: i64,
        ended_at_ms: i64,
        exit_code: i32,
    ) -> Self {
        Self {
            name: name.into(),
            command_id: command_id.into(),
            snapshot_digest: snapshot_digest.into(),
            started_at_ms,
            ended_at_ms,
            exit_code,
            output_artifact_id: None,
            output_truncated: false,
        }
    }
}

/// Evidence assembled for one ownership transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffManifestV1 {
    pub schema_version: u16,
    pub manifest_id: String,
    pub generation: u64,
    pub created_at_ms: i64,
    pub lineage: HandoffLineage,
    pub worktree: WorktreeSnapshot,
    pub tests: Vec<TestReceipt>,
}

impl HandoffManifestV1 {
    pub fn new(
        generation: u64,
        created_at_ms: i64,
        lineage: HandoffLineage,
        worktree: WorktreeSnapshot,
        tests: Vec<TestReceipt>,
    ) -> Result<Self, HandoffError> {
        let manifest_id = manifest_id(generation, created_at_ms, &lineage, &worktree, &tests)?;
        Ok(Self {
            schema_version: HANDOFF_SCHEMA_VERSION,
            manifest_id,
            generation,
            created_at_ms,
            lineage,
            worktree,
            tests,
        })
    }

    /// Structural facts missing before an integration worktree may publish.
    ///
    /// This does not authenticate a test receipt; the future durable runtime
    /// must construct receipts itself. This read-only slice therefore always
    /// blocks dirty live work until an immutable recovery commit exists.
    #[must_use]
    pub fn structural_publish_blockers(&self, policy: &PublishPolicy) -> Vec<PublishBlocker> {
        let mut blockers = Vec::new();
        if self.schema_version != HANDOFF_SCHEMA_VERSION
            || self.worktree.schema_version != HANDOFF_SCHEMA_VERSION
        {
            blockers.push(PublishBlocker::UnsupportedSchema {
                manifest: self.schema_version,
                snapshot: self.worktree.schema_version,
            });
        }
        let expected_id = manifest_id(
            self.generation,
            self.created_at_ms,
            &self.lineage,
            &self.worktree,
            &self.tests,
        )
        .ok();
        if expected_id.as_deref() != Some(self.manifest_id.as_str()) {
            blockers.push(PublishBlocker::ManifestIdMismatch);
        }
        blockers.extend(
            self.worktree
                .coverage
                .gaps
                .iter()
                .cloned()
                .map(PublishBlocker::from),
        );
        if let Some(operation) = self.worktree.operation {
            blockers.push(PublishBlocker::GitOperation { operation });
        }
        if self.worktree.has_unmerged_paths() {
            blockers.push(PublishBlocker::UnmergedPaths);
        }
        if self.worktree.detached {
            blockers.push(PublishBlocker::DetachedHead);
        }
        if self.worktree.locked {
            blockers.push(PublishBlocker::LockedWorktree);
        }
        // This slice only observes. Until a later mutation creates and verifies
        // an immutable recovery commit, dirty live bytes are never publishable.
        if self.worktree.is_dirty() {
            blockers.push(PublishBlocker::DirtySnapshotNotMaterialized);
        }
        for required in &policy.required_tests {
            let Some(receipt) = self
                .tests
                .iter()
                .rev()
                .find(|test| test.name == required.name)
            else {
                blockers.push(PublishBlocker::RequiredTestMissing {
                    name: required.name.clone(),
                });
                continue;
            };
            if receipt.command_id != required.command_id {
                blockers.push(PublishBlocker::UnexpectedTestCommand {
                    name: required.name.clone(),
                });
            } else if receipt.snapshot_digest != self.worktree.content_digest {
                blockers.push(PublishBlocker::StaleTest {
                    name: required.name.clone(),
                });
            } else if receipt.exit_code != 0 {
                blockers.push(PublishBlocker::FailedTest {
                    name: required.name.clone(),
                    exit_code: receipt.exit_code,
                });
            } else if required.must_fail_first && !self.fails_on_another_tree(required, receipt) {
                blockers.push(PublishBlocker::NoBaselineFailure {
                    name: required.name.clone(),
                });
            }
        }
        blockers
    }

    /// Whether some receipt for `required` failed on a tree that is not the
    /// one being published.
    ///
    /// The same command id, so another check's failure cannot stand in for
    /// this one's; a different snapshot digest, because a receipt for THIS
    /// tree can never be its own baseline; and a non-zero exit, which is the
    /// failure itself.
    fn fails_on_another_tree(&self, required: &TestRequirement, passing: &TestReceipt) -> bool {
        self.tests.iter().any(|test| {
            test.name == required.name
                && test.command_id == required.command_id
                && test.exit_code != 0
                && test.snapshot_digest != passing.snapshot_digest
        })
    }
}

/// Publication policy belongs to the coordinator, not to the snapshotter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishPolicy {
    pub required_tests: Vec<TestRequirement>,
}

/// One configured test the publisher requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRequirement {
    pub name: String,
    pub command_id: String,
    /// Whether this check must also be shown FAILING on another tree before
    /// its pass is believed.
    ///
    /// A passing receipt says the check is green on the commit it names. It
    /// does not say the work made it green: a tree that already satisfied the
    /// check, and a check that cannot fail at all, hand up the same receipt a
    /// fix does. The window's GUI bench has asked this question since
    /// 2026-09-12 — its scenarios refuse to start when the answer is already
    /// on screen (`display_lacks`, `read_lacks` in
    /// `tools/computer-bench/scenarios.py`), after a calculator that restored
    /// its last value let a run that did nothing pass both its checks. This is
    /// that precondition, asked of a commit rather than of a screen.
    pub must_fail_first: bool,
}

/// A typed reason automatic integration must stop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PublishBlocker {
    UnsupportedSchema {
        manifest: u16,
        snapshot: u16,
    },
    ManifestIdMismatch,
    IncompleteSnapshot {
        gap: CoverageGap,
    },
    GitOperation {
        operation: GitOperation,
    },
    UnmergedPaths,
    DetachedHead,
    LockedWorktree,
    DirtySnapshotNotMaterialized,
    RequiredTestMissing {
        name: String,
    },
    UnexpectedTestCommand {
        name: String,
    },
    StaleTest {
        name: String,
    },
    FailedTest {
        name: String,
        exit_code: i32,
    },
    /// The check passed, and nothing shows that it can fail: no receipt for it
    /// names another tree it failed on.
    NoBaselineFailure {
        name: String,
    },
}

impl PublishBlocker {
    fn incomplete(gap: CoverageGap) -> Self {
        Self::IncompleteSnapshot { gap }
    }
}

impl From<CoverageGap> for PublishBlocker {
    fn from(gap: CoverageGap) -> Self {
        Self::incomplete(gap)
    }
}

/// Snapshot collection failed without changing the checkout.
#[derive(Debug, thiserror::Error)]
pub enum HandoffError {
    #[error(transparent)]
    Orchestrator(#[from] OrchestratorError),
    #[error("worktree {path} has no committed HEAD to hand off")]
    NoHead { path: PathBuf },
    #[error("`git {command}` returned malformed output: {output}")]
    MalformedGit { command: String, output: String },
    #[error("`git {command}` returned more than {limit} bytes")]
    GitOutputTooLarge { command: String, limit: usize },
    #[error("`git {command}` did not finish within {milliseconds} ms")]
    GitTimeout { command: String, milliseconds: u128 },
    #[error("`git {command}` returned a path that is not valid UTF-8")]
    NonUtf8GitOutput { command: String },
    #[error("could not read `git {command}` output: {source}")]
    ReadGitOutput {
        command: String,
        #[source]
        source: io::Error,
    },
    #[error("worktree changed while its handoff snapshot was being captured")]
    ChangedDuringCapture,
    #[error("could not serialize a handoff manifest: {0}")]
    Serialize(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Observation {
    known: Worktree,
    repository_id: String,
    worktree_id: String,
    head_oid: String,
    upstream: Option<RefObservation>,
    ahead: Option<u64>,
    behind: Option<u64>,
    operation: Option<GitOperation>,
    loss: PendingLoss,
    changes: Vec<HandoffChange>,
    content_digest: String,
    coverage: SnapshotCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UpstreamState {
    observation: Option<RefObservation>,
    ahead: Option<u64>,
    behind: Option<u64>,
}

#[derive(Debug)]
struct CapturedStream {
    bytes: Vec<u8>,
    overflowed: bool,
}

#[derive(Debug)]
struct GitCapture {
    stdout: CapturedStream,
}

impl Orchestrator {
    /// Opaque identity of the repository administration directory shared by
    /// all of its linked worktrees. This reads Git metadata only.
    pub fn repository_id(&self, worktree: impl AsRef<Path>) -> Result<String, HandoffError> {
        self.repository_id_with_limits(worktree.as_ref(), SnapshotLimits::default())
    }

    /// Observe a checkout twice and accept it only if both observations agree.
    /// No Git ref, index entry, or worktree file is modified.
    pub fn handoff_snapshot(
        &self,
        worktree: impl AsRef<Path>,
        observed_at_ms: i64,
    ) -> Result<WorktreeSnapshot, HandoffError> {
        self.handoff_snapshot_with_limits(worktree, observed_at_ms, SnapshotLimits::default())
    }

    /// [`Self::handoff_snapshot`] with explicit admission limits.
    pub fn handoff_snapshot_with_limits(
        &self,
        worktree: impl AsRef<Path>,
        observed_at_ms: i64,
        limits: SnapshotLimits,
    ) -> Result<WorktreeSnapshot, HandoffError> {
        self.handoff_snapshot_around(worktree.as_ref(), observed_at_ms, limits.clamped(), || {})
    }

    fn handoff_snapshot_around(
        &self,
        worktree: &Path,
        observed_at_ms: i64,
        limits: SnapshotLimits,
        between: impl FnOnce(),
    ) -> Result<WorktreeSnapshot, HandoffError> {
        let first = self.observe_worktree(worktree, observed_at_ms, limits)?;
        between();
        let second = self.observe_worktree(worktree, observed_at_ms, limits)?;
        if first != second {
            return Err(HandoffError::ChangedDuringCapture);
        }
        let worktree_name = first
            .known
            .path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        Ok(WorktreeSnapshot {
            schema_version: HANDOFF_SCHEMA_VERSION,
            observed_at_ms,
            repository_id: first.repository_id,
            worktree_id: first.worktree_id,
            worktree_name,
            branch: first.known.branch,
            detached: first.known.detached,
            locked: first.known.locked,
            head_oid: first.head_oid,
            upstream: first.upstream,
            ahead: first.ahead,
            behind: first.behind,
            operation: first.operation,
            changes: first.changes,
            ignored: first.loss.ignored,
            content_digest: first.content_digest,
            coverage: first.coverage,
            private_path: Some(first.known.path.into()),
        })
    }

    fn observe_worktree(
        &self,
        worktree: &Path,
        observed_at_ms: i64,
        limits: SnapshotLimits,
    ) -> Result<Observation, HandoffError> {
        let known = bounded_worktree_at(self, worktree, limits)?;
        if known
            .head
            .as_deref()
            .is_none_or(|head| head.bytes().all(|byte| byte == b'0'))
        {
            return Err(HandoffError::NoHead {
                path: known.path.clone(),
            });
        }
        let head = required_git_text(
            self,
            &known.path,
            ["rev-parse", "--verify", "HEAD"],
            "rev-parse --verify HEAD",
            limits,
        )?;
        let head_oid = head.trim().to_string();
        if head_oid.is_empty() {
            return Err(HandoffError::NoHead {
                path: known.path.clone(),
            });
        }
        let repository_id = self.repository_id_with_limits(&known.path, limits)?;
        let worktree_git_dir = required_git_text(
            self,
            &known.path,
            ["rev-parse", "--absolute-git-dir"],
            "rev-parse --absolute-git-dir",
            limits,
        )?;
        let worktree_git_dir = resolve_git_path(&known.path, worktree_git_dir.trim());
        let worktree_id = zerocode_core::git_dir::opaque_path_id("worktree", &worktree_git_dir);
        let upstream = self.upstream_of(&known, &head_oid, observed_at_ms, limits)?;
        let loss = bounded_pending_loss(self, &known.path, limits)?;
        let changes: Vec<HandoffChange> = loss.uncommitted.iter().map(Into::into).collect();
        let conflicted = changes.iter().any(|change| change.conflicted);
        let operation = match bounded_conflict_operation(self, &known.path, limits)? {
            ConflictOperation::Merge => Some(GitOperation::Merge),
            ConflictOperation::Rebase => Some(GitOperation::Rebase),
            ConflictOperation::CherryPick => Some(GitOperation::CherryPick),
            ConflictOperation::Unknown if conflicted => Some(GitOperation::Other),
            ConflictOperation::Unknown => None,
        };
        let (content_digest, coverage) =
            content_digest(self, &known.path, &head_oid, &loss, limits)?;
        Ok(Observation {
            known,
            repository_id,
            worktree_id,
            head_oid,
            upstream: upstream.observation,
            ahead: upstream.ahead,
            behind: upstream.behind,
            operation,
            loss,
            changes,
            content_digest,
            coverage,
        })
    }

    fn repository_id_with_limits(
        &self,
        worktree: &Path,
        limits: SnapshotLimits,
    ) -> Result<String, HandoffError> {
        let common = required_git_text(
            self,
            worktree,
            ["rev-parse", "--git-common-dir"],
            "rev-parse --git-common-dir",
            limits,
        )?;
        Ok(zerocode_core::git_dir::opaque_path_id(
            "repo",
            &resolve_git_path(worktree, common.trim()),
        ))
    }

    fn upstream_of(
        &self,
        worktree: &Worktree,
        head_oid: &str,
        observed_at_ms: i64,
        limits: SnapshotLimits,
    ) -> Result<UpstreamState, HandoffError> {
        let Some(branch) = worktree.branch.as_deref() else {
            return Ok(UpstreamState {
                observation: None,
                ahead: None,
                behind: None,
            });
        };
        let branch_ref = format!("refs/heads/{branch}");
        let output = required_git_text(
            self,
            &worktree.path,
            [
                "for-each-ref",
                "--format=%(upstream:short)%00%(upstream)",
                branch_ref.as_str(),
            ],
            "for-each-ref upstream",
            limits,
        )?;
        let Some((name, full_ref)) = output.trim_end().split_once('\0') else {
            return Ok(UpstreamState {
                observation: None,
                ahead: None,
                behind: None,
            });
        };
        if name.is_empty() || full_ref.is_empty() {
            return Ok(UpstreamState {
                observation: None,
                ahead: None,
                behind: None,
            });
        }
        let oid = required_git_text(
            self,
            &worktree.path,
            ["rev-parse", "--verify", full_ref],
            "rev-parse upstream",
            limits,
        )?
        .trim()
        .to_string();
        let range = format!("{head_oid}...{oid}");
        let counts = required_git_text(
            self,
            &worktree.path,
            ["rev-list", "--left-right", "--count", range.as_str()],
            "rev-list --left-right --count",
            limits,
        )?;
        let counts_output = counts.trim().to_string();
        let mut counts = counts_output.split_ascii_whitespace();
        let ahead = counts.next().and_then(|value| value.parse::<u64>().ok());
        let behind = counts.next().and_then(|value| value.parse::<u64>().ok());
        if ahead.is_none() || behind.is_none() {
            return Err(HandoffError::MalformedGit {
                command: "rev-list --left-right --count".to_string(),
                output: counts_output,
            });
        }
        Ok(UpstreamState {
            observation: Some(RefObservation {
                name: name.to_string(),
                oid,
                observed_at_ms,
            }),
            ahead,
            behind,
        })
    }
}

fn capture_stream(mut reader: impl Read, limit: usize) -> io::Result<CapturedStream> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    let mut overflowed = false;
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let room = limit.saturating_sub(bytes.len());
        let kept = room.min(read);
        bytes.extend_from_slice(&chunk[..kept]);
        overflowed |= kept < read;
    }
    Ok(CapturedStream { bytes, overflowed })
}

fn run_git_capture<I, S>(
    orchestrator: &Orchestrator,
    cwd: &Path,
    args: I,
    command_name: &str,
    stdout_limit: usize,
    limits: SnapshotLimits,
) -> Result<GitCapture, HandoffError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = crate::git_command(&orchestrator.git, cwd);
    command
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("status.renames=false")
        .arg("-c")
        .arg("diff.algorithm=myers")
        .arg("-c")
        .arg("diff.renames=false")
        .arg("-c")
        .arg("diff.mnemonicPrefix=false")
        .arg("-c")
        .arg("diff.noprefix=false")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(OrchestratorError::GitUnavailable)?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(HandoffError::ReadGitOutput {
            command: command_name.to_string(),
            source: io::Error::new(io::ErrorKind::BrokenPipe, "stdout pipe was not created"),
        });
    };
    let Some(stderr) = child.stderr.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(HandoffError::ReadGitOutput {
            command: command_name.to_string(),
            source: io::Error::new(io::ErrorKind::BrokenPipe, "stderr pipe was not created"),
        });
    };
    let stdout_reader = spawn_stream_reader(stdout, stdout_limit);
    let stderr_limit = limits.git_stderr_bytes;
    let stderr_reader = spawn_stream_reader(stderr, stderr_limit);
    let started = Instant::now();
    let deadline = started.checked_add(limits.git_timeout).unwrap_or(started);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < limits.git_timeout => {
                std::thread::sleep(SNAPSHOT_GIT_POLL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HandoffError::GitTimeout {
                    command: command_name.to_string(),
                    milliseconds: limits.git_timeout.as_millis(),
                });
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(OrchestratorError::GitUnavailable(error).into());
            }
        }
    };
    let stdout = receive_stream(stdout_reader, deadline, command_name, limits)?;
    let stderr = receive_stream(stderr_reader, deadline, command_name, limits)?;
    if !status.success() {
        let mut said = String::from_utf8_lossy(&stderr.bytes).into_owned();
        if stderr.overflowed {
            said.push_str("\n[stderr truncated]");
        }
        return Err(OrchestratorError::Git {
            command: command_name.to_string(),
            status: status.to_string(),
            stderr: if said.trim().is_empty() {
                "no output".to_string()
            } else {
                said
            },
        }
        .into());
    }
    Ok(GitCapture { stdout })
}

fn spawn_stream_reader(
    reader: impl Read + Send + 'static,
    limit: usize,
) -> Receiver<io::Result<CapturedStream>> {
    let (send, receive) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = send.send(capture_stream(reader, limit));
    });
    receive
}

fn receive_stream(
    reader: Receiver<io::Result<CapturedStream>>,
    deadline: Instant,
    command: &str,
    limits: SnapshotLimits,
) -> Result<CapturedStream, HandoffError> {
    let left = deadline.saturating_duration_since(Instant::now());
    match reader.recv_timeout(left) {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(source)) => Err(HandoffError::ReadGitOutput {
            command: command.to_string(),
            source,
        }),
        Err(RecvTimeoutError::Timeout) => Err(HandoffError::GitTimeout {
            command: command.to_string(),
            milliseconds: limits.git_timeout.as_millis(),
        }),
        Err(RecvTimeoutError::Disconnected) => Err(HandoffError::ReadGitOutput {
            command: command.to_string(),
            source: io::Error::other("reader thread ended without output"),
        }),
    }
}

fn required_git_bytes<I, S>(
    orchestrator: &Orchestrator,
    cwd: &Path,
    args: I,
    command: &str,
    limits: SnapshotLimits,
) -> Result<Vec<u8>, HandoffError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let capture = run_git_capture(
        orchestrator,
        cwd,
        args,
        command,
        limits.git_output_bytes,
        limits,
    )?;
    if capture.stdout.overflowed {
        return Err(HandoffError::GitOutputTooLarge {
            command: command.to_string(),
            limit: limits.git_output_bytes,
        });
    }
    Ok(capture.stdout.bytes)
}

fn required_git_text<I, S>(
    orchestrator: &Orchestrator,
    cwd: &Path,
    args: I,
    command: &str,
    limits: SnapshotLimits,
) -> Result<String, HandoffError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    String::from_utf8(required_git_bytes(
        orchestrator,
        cwd,
        args,
        command,
        limits,
    )?)
    .map_err(|_| HandoffError::NonUtf8GitOutput {
        command: command.to_string(),
    })
}

fn bounded_worktree_at(
    orchestrator: &Orchestrator,
    path: &Path,
    limits: SnapshotLimits,
) -> Result<Worktree, HandoffError> {
    let listed = required_git_text(
        orchestrator,
        orchestrator.repo_root(),
        ["worktree", "list", "--porcelain"],
        "worktree list --porcelain",
        limits,
    )?;
    crate::parse_worktree_list(&listed)
        .into_iter()
        .find(|candidate| crate::same_worktree_path(&candidate.path, path))
        .ok_or_else(|| {
            OrchestratorError::UnknownWorktree {
                path: path.to_path_buf(),
            }
            .into()
        })
}

fn bounded_pending_loss(
    orchestrator: &Orchestrator,
    worktree: &Path,
    limits: SnapshotLimits,
) -> Result<PendingLoss, HandoffError> {
    let status = required_git_text(
        orchestrator,
        worktree,
        [
            "status",
            "--porcelain=v2",
            "-z",
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
        "status --porcelain=v2 -z --ignored",
        limits,
    )?;
    Ok(crate::parse_status(&status))
}

fn bounded_conflict_operation(
    orchestrator: &Orchestrator,
    worktree: &Path,
    limits: SnapshotLimits,
) -> Result<ConflictOperation, HandoffError> {
    let git_dir = match zerocode_core::git_dir::of(worktree) {
        Some(path) => path,
        None => {
            let found = required_git_text(
                orchestrator,
                worktree,
                ["rev-parse", "--absolute-git-dir"],
                "rev-parse --absolute-git-dir",
                limits,
            )?;
            PathBuf::from(found.trim())
        }
    };
    if git_dir.join("MERGE_HEAD").exists() {
        return Ok(ConflictOperation::Merge);
    }
    if git_dir.join("rebase-merge").exists() || git_dir.join("rebase-apply").exists() {
        return Ok(ConflictOperation::Rebase);
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Ok(ConflictOperation::CherryPick);
    }
    Ok(ConflictOperation::Unknown)
}

fn hidden_index_gaps(listing: &str) -> Vec<CoverageGap> {
    listing
        .split('\0')
        .filter_map(|record| {
            let mut record = record.chars();
            let tag = record.next()?;
            if record.next()? != ' ' {
                return None;
            }
            let path = record.as_str().to_string();
            let flag = match tag {
                'S' => HiddenIndexFlag::SkipWorktree,
                's' => HiddenIndexFlag::Both,
                tag if tag.is_ascii_lowercase() => HiddenIndexFlag::AssumeUnchanged,
                _ => return None,
            };
            Some(CoverageGap::IndexHiddenContent { path, flag })
        })
        .collect()
}

fn content_digest(
    orchestrator: &Orchestrator,
    worktree: &Path,
    head_oid: &str,
    loss: &PendingLoss,
    limits: SnapshotLimits,
) -> Result<(String, SnapshotCoverage), HandoffError> {
    let index = required_git_bytes(
        orchestrator,
        worktree,
        ["ls-files", "--stage", "-z"],
        "ls-files --stage",
        limits,
    )?;
    let visibility = required_git_text(
        orchestrator,
        worktree,
        ["ls-files", "-v", "-z"],
        "ls-files -v",
        limits,
    )?;
    let untracked = required_git_bytes(
        orchestrator,
        worktree,
        ["ls-files", "--others", "--exclude-standard", "-z"],
        "ls-files --others --exclude-standard",
        limits,
    )?;
    let untracked_count = untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .count();
    let tracked_count = loss
        .uncommitted
        .iter()
        .filter(|entry| entry.code != "??")
        .count();
    let found = tracked_count.saturating_add(untracked_count);

    let mut digest = Sha256::new();
    hash_frame(&mut digest, b"head", head_oid.as_bytes());
    hash_frame(&mut digest, b"index", &index);
    hash_frame(&mut digest, b"index_visibility", visibility.as_bytes());
    hash_frame(&mut digest, b"status", loss.fingerprint().as_bytes());
    hash_frame(&mut digest, b"untracked_paths", &untracked);

    let mut gaps = Vec::new();
    if !loss.ignored.is_empty() {
        gaps.push(CoverageGap::IgnoredContent {
            count: loss.ignored.len(),
        });
    }
    if untracked_count > 0 {
        gaps.push(CoverageGap::UntrackedContent {
            count: untracked_count,
        });
    }
    if found > limits.changed_paths {
        gaps.push(CoverageGap::ChangedPathLimit {
            found,
            limit: limits.changed_paths,
        });
    }
    gaps.extend(hidden_index_gaps(&visibility));
    for entry in &loss.uncommitted {
        if entry
            .submodule
            .is_some_and(|submodule| submodule.tracked_changes || submodule.untracked_changes)
        {
            gaps.push(CoverageGap::DirtySubmodule {
                path: entry.path.clone(),
            });
        }
    }

    let mut content_bytes = 0_u64;
    let diffs: [(&str, &[&str]); 2] = [
        (
            "unstaged_diff",
            &[
                "diff",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--ignore-submodules=none",
                "--",
            ],
        ),
        (
            "staged_diff",
            &[
                "diff",
                "--cached",
                "--binary",
                "--full-index",
                "--no-ext-diff",
                "--no-textconv",
                "--no-color",
                "--ignore-submodules=none",
                "--",
            ],
        ),
    ];
    for (label, args) in diffs {
        let remaining = limits.content_bytes.saturating_sub(content_bytes);
        let stored_limit = usize::try_from(remaining).unwrap_or(usize::MAX);
        let capture = run_git_capture(
            orchestrator,
            worktree,
            args.iter().copied(),
            label,
            stored_limit,
            limits,
        )?;
        hash_frame(&mut digest, label.as_bytes(), &capture.stdout.bytes);
        content_bytes = content_bytes.saturating_add(capture.stdout.bytes.len() as u64);
        if capture.stdout.overflowed {
            gaps.push(CoverageGap::ContentByteLimit {
                source: label.to_string(),
                bytes: remaining.saturating_add(1),
                limit: limits.content_bytes,
            });
        }
    }
    Ok((
        finish_digest(digest),
        SnapshotCoverage {
            changed_paths: found,
            content_bytes,
            gaps,
        },
    ))
}

fn is_conflict_code(code: &str) -> bool {
    matches!(code, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU")
}

fn resolve_git_path(worktree: &Path, reported: &str) -> PathBuf {
    let path = PathBuf::from(reported);
    let joined = if path.is_absolute() {
        path
    } else {
        worktree.join(path)
    };
    joined.canonicalize().unwrap_or(joined)
}

fn manifest_id(
    generation: u64,
    created_at_ms: i64,
    lineage: &HandoffLineage,
    worktree: &WorktreeSnapshot,
    tests: &[TestReceipt],
) -> Result<String, HandoffError> {
    let mut digest = Sha256::new();
    let evidence = serde_json::to_vec(&(
        HANDOFF_SCHEMA_VERSION,
        generation,
        created_at_ms,
        lineage,
        worktree,
        tests,
    ))?;
    hash_frame(&mut digest, b"manifest", &evidence);
    Ok(format!("handoff-{}", finish_digest(digest)))
}

fn hash_frame(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u64).to_le_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

fn finish_digest(digest: Sha256) -> String {
    format!("{:x}", digest.finalize())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new(crate::GIT_EXECUTABLE)
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn repository() -> TempDir {
        let root = tempfile::tempdir().expect("temp repository");
        git(root.path(), &["init", "-b", "main"]);
        git(root.path(), &["config", "user.name", "ZeroCode Test"]);
        git(
            root.path(),
            &["config", "user.email", "test@zerocode.local"],
        );
        std::fs::write(root.path().join("tracked.txt"), "one\n").expect("seed file");
        git(root.path(), &["add", "tracked.txt"]);
        git(root.path(), &["commit", "-m", "seed"]);
        root
    }

    #[test]
    fn a_write_between_the_two_observations_is_not_called_a_snapshot() {
        let root = repository();
        let orchestrator = Orchestrator::open(root.path()).expect("open repository");
        let result =
            orchestrator.handoff_snapshot_around(root.path(), 1, SnapshotLimits::default(), || {
                std::fs::write(root.path().join("tracked.txt"), "two\n").expect("concurrent edit");
            });
        assert!(matches!(result, Err(HandoffError::ChangedDuringCapture)));
    }

    #[test]
    fn public_limits_can_only_make_a_snapshot_stricter() {
        let clamped = SnapshotLimits {
            changed_paths: usize::MAX,
            content_bytes: u64::MAX,
            git_output_bytes: usize::MAX,
            git_stderr_bytes: usize::MAX,
            git_timeout: Duration::MAX,
        }
        .clamped();
        assert_eq!(clamped, SnapshotLimits::default());
    }
}
