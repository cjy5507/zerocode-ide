//! Local Git implementation of the workflow publication contract.
//!
//! This publisher never infers a target. It accepts a source branch and only a
//! target under `refs/zerocode/integration/`, refuses that target when any
//! worktree has explicitly put its HEAD there, and moves it together with an
//! immutable operation receipt in one `git update-ref --stdin` transaction.
//! Recovery creates the cancellation tombstone through the same transaction
//! mechanism, so commit and cancellation cannot both win.
//!
//! This first adapter is Unix-only. Git and its ordinary descendants share a
//! process group; nonblocking pipes keep the call deadline bounded even if a
//! deliberately daemonized process leaves that group. The configured Git is a
//! trusted host executable, not an adversarial-code sandbox. Windows needs a
//! Job Object before construction can be enabled there.
//! A hostile same-UID process that manually rewrites a worktree's `HEAD` during
//! publication remains outside this foundation's boundary; ordinary Git
//! worktree/checkout flows cannot attach this non-branch target namespace.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::bounded_process::{self, ProcessError, ProcessLimits, ProcessOutput};
use crate::publish::{
    AppliedPublication, PublishAttempt, PublishObservation, PublishVerification, Publisher,
    PublisherFailure,
};
use crate::workflow::{PublishExpectation, valid_branch_ref, valid_oid, valid_publish_target_ref};
use crate::{Orchestrator, git_command};

pub const LOCAL_GIT_SCOPE_PREFIX: &str = "local-git:";
pub const APPLIED_REF_PREFIX: &str = "refs/zerocode/publish/applied/";
pub const CANCELLED_REF_PREFIX: &str = "refs/zerocode/publish/cancelled/";
pub const PUBLISH_GIT_TIMEOUT: Duration = Duration::from_secs(8);
pub const PUBLISH_GIT_OUTPUT_LIMIT: usize = 1024 * 1024;

const VERIFY_RETRIES: usize = 4;
#[cfg(windows)]
const EMPTY_GRAFT_FILE: &str = "NUL";
#[cfg(not(windows))]
const EMPTY_GRAFT_FILE: &str = "/dev/null";

/// A path is kept only for routing commands and is redacted from Debug.
#[derive(Clone)]
struct PrivateRepositoryPath(PathBuf);

impl std::fmt::Debug for PrivateRepositoryPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<private-repository-path>")
    }
}

/// Atomic publisher for refs in one local repository.
#[derive(Clone)]
pub struct LocalGitRefPublisher {
    git: OsString,
    repository: PrivateRepositoryPath,
    repository_id: String,
    publisher_scope: String,
    timeout: Duration,
    output_limit: usize,
}

impl std::fmt::Debug for LocalGitRefPublisher {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LocalGitRefPublisher")
            .field("repository", &self.repository)
            .field("repository_id", &self.repository_id)
            .field("publisher_scope", &self.publisher_scope)
            .finish()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LocalGitPublisherError {
    #[error("the local Git publisher is not available on this platform")]
    UnsupportedPlatform,
    #[error("the local Git publisher could not open the repository")]
    Open,
    #[error("the local Git publisher could not establish repository identity")]
    Identity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitCallError {
    Timeout,
    Unavailable,
    UnsafeRef,
}

impl GitCallError {
    const fn publisher(self) -> PublisherFailure {
        match self {
            Self::Timeout => PublisherFailure::Timeout,
            Self::Unavailable => PublisherFailure::Unavailable,
            Self::UnsafeRef => PublisherFailure::Refused,
        }
    }
}

impl From<ProcessError> for GitCallError {
    fn from(error: ProcessError) -> Self {
        match error {
            ProcessError::Timeout => Self::Timeout,
            ProcessError::Unavailable => Self::Unavailable,
            #[cfg(not(any(unix, windows)))]
            ProcessError::UnsupportedPlatform => Self::Unavailable,
        }
    }
}

impl LocalGitRefPublisher {
    /// Open the repository with the `git` resolved on `PATH`.
    pub fn open(worktree: impl AsRef<Path>) -> Result<Self, LocalGitPublisherError> {
        Self::open_with_git(worktree, crate::GIT_EXECUTABLE)
    }

    /// [`Self::open`] with an explicit executable for managed installations and
    /// deterministic tests.
    pub fn open_with_git(
        worktree: impl AsRef<Path>,
        git: impl Into<OsString>,
    ) -> Result<Self, LocalGitPublisherError> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (worktree, git);
            return Err(LocalGitPublisherError::UnsupportedPlatform);
        }
        #[cfg(any(unix, windows))]
        {
            let worktree = worktree.as_ref();
            let git = git.into();
            let orchestrator = Orchestrator::open_with_git(worktree, git.clone())
                .map_err(|_| LocalGitPublisherError::Open)?;
            let repository_id = orchestrator
                .repository_id(worktree)
                .map_err(|_| LocalGitPublisherError::Identity)?;
            let publisher_scope = format!("{LOCAL_GIT_SCOPE_PREFIX}{repository_id}");
            Ok(Self {
                git,
                repository: PrivateRepositoryPath(orchestrator.repo_root().to_path_buf()),
                repository_id,
                publisher_scope,
                timeout: PUBLISH_GIT_TIMEOUT,
                output_limit: PUBLISH_GIT_OUTPUT_LIMIT,
            })
        }
    }

    #[must_use]
    pub fn repository_id(&self) -> &str {
        &self.repository_id
    }

    #[must_use]
    pub fn publisher_scope(&self) -> &str {
        &self.publisher_scope
    }

    fn run_git(
        &self,
        args: &[&OsStr],
        input: Option<&[u8]>,
    ) -> Result<ProcessOutput, GitCallError> {
        let mut command = git_command(&self.git, &self.repository.0);
        command.env("GIT_NO_REPLACE_OBJECTS", "1");
        command.env("GIT_GRAFT_FILE", EMPTY_GRAFT_FILE);
        command.args(args);
        let output = bounded_process::run(
            &mut command,
            input,
            ProcessLimits {
                timeout: self.timeout,
                stdout_bytes: self.output_limit,
                stderr_bytes: self.output_limit,
            },
        )?;
        if output.stdout_truncated || output.stderr_truncated {
            return Err(GitCallError::Unavailable);
        }
        Ok(output)
    }

    fn args<const N: usize>(args: [&str; N]) -> [&OsStr; N] {
        args.map(OsStr::new)
    }

    fn read_ref(&self, reference: &str) -> Result<Option<String>, GitCallError> {
        if self.is_symbolic_ref(reference)? {
            return Err(GitCallError::UnsafeRef);
        }
        let args = Self::args([
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            reference,
        ]);
        let answer = self.run_git(&args, None)?;
        match answer.status.code() {
            Some(0) => bounded_oid(&answer.stdout)
                .map(Some)
                .ok_or(GitCallError::Unavailable),
            Some(1) => Ok(None),
            _ => Err(GitCallError::Unavailable),
        }
    }

    fn is_symbolic_ref(&self, reference: &str) -> Result<bool, GitCallError> {
        let args = Self::args(["symbolic-ref", "--quiet", reference]);
        let answer = self.run_git(&args, None)?;
        match answer.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(GitCallError::Unavailable),
        }
    }

    fn commit_oid(&self, reference: &str) -> Result<String, GitCallError> {
        let peeled = format!("{reference}^{{commit}}");
        let args = Self::args(["rev-parse", "--verify", "--end-of-options", &peeled]);
        let answer = self.run_git(&args, None)?;
        if !answer.status.success() {
            return Err(GitCallError::Unavailable);
        }
        bounded_oid(&answer.stdout).ok_or(GitCallError::Unavailable)
    }

    fn is_ancestor(&self, ancestor: &str, descendant: &str) -> Result<bool, GitCallError> {
        let args = Self::args(["merge-base", "--is-ancestor", ancestor, descendant]);
        let answer = self.run_git(&args, None)?;
        match answer.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(GitCallError::Unavailable),
        }
    }

    fn target_is_checked_out(&self, target: &str) -> Result<bool, GitCallError> {
        let args = Self::args(["worktree", "list", "--porcelain"]);
        let answer = self.run_git(&args, None)?;
        if !answer.status.success() {
            return Err(GitCallError::Unavailable);
        }
        let listing = std::str::from_utf8(&answer.stdout).map_err(|_| GitCallError::Unavailable)?;
        Ok(listing
            .lines()
            .any(|line| line.strip_prefix("branch ") == Some(target)))
    }

    fn observe_refs(
        &self,
        expected: &PublishExpectation,
        reject_checked_out: bool,
    ) -> Result<PublishObservation, PublisherFailure> {
        validate_expectation(expected)?;
        if self
            .is_symbolic_ref(&expected.source.name)
            .map_err(GitCallError::publisher)?
            || self
                .is_symbolic_ref(&expected.target.name)
                .map_err(GitCallError::publisher)?
        {
            return Err(PublisherFailure::Refused);
        }
        if reject_checked_out
            && self
                .target_is_checked_out(&expected.target.name)
                .map_err(GitCallError::publisher)?
        {
            return Err(PublisherFailure::Refused);
        }
        let source_oid = self
            .commit_oid(&expected.source.name)
            .map_err(GitCallError::publisher)?;
        let target_oid = self
            .commit_oid(&expected.target.name)
            .map_err(GitCallError::publisher)?;
        let source_descends_from_target = self
            .is_ancestor(&target_oid, &source_oid)
            .map_err(GitCallError::publisher)?;
        Ok(PublishObservation {
            repository_id: self.repository_id.clone(),
            publisher_scope: self.publisher_scope.clone(),
            source_oid,
            target_oid,
            source_descends_from_target,
        })
    }

    fn operation_refs(operation_id: &str) -> Result<(String, String), PublisherFailure> {
        if !valid_operation_id(operation_id) {
            return Err(PublisherFailure::Refused);
        }
        Ok((
            format!("{APPLIED_REF_PREFIX}{operation_id}"),
            format!("{CANCELLED_REF_PREFIX}{operation_id}"),
        ))
    }

    fn update_refs(&self, transaction: &str) -> Result<bool, GitCallError> {
        let args = Self::args(["update-ref", "--no-deref", "--stdin"]);
        self.run_git(&args, Some(transaction.as_bytes()))
            .map(|answer| answer.status.success())
    }

    fn cancellation(
        &self,
        attempt: &PublishAttempt<'_>,
        applied_ref: &str,
        cancelled_ref: &str,
    ) -> Result<bool, PublisherFailure> {
        let expected = attempt.expected();
        let zero = zero_oid(&expected.source.oid)?;
        let transaction = format!(
            "start\nverify {applied_ref} {zero}\ncreate {cancelled_ref} {}\nprepare\ncommit\n",
            expected.target.oid
        );
        self.update_refs(&transaction)
            .map_err(GitCallError::publisher)
    }
}

impl Publisher for LocalGitRefPublisher {
    fn observe(
        &self,
        expected: &PublishExpectation,
    ) -> Result<PublishObservation, PublisherFailure> {
        self.observe_refs(expected, true)
    }

    fn commit(&self, attempt: &PublishAttempt<'_>) -> Result<(), PublisherFailure> {
        let expected = attempt.expected();
        validate_bound_expectation(self, expected)?;
        let (applied_ref, cancelled_ref) = Self::operation_refs(attempt.operation_id())?;
        if let Some(applied) = self
            .read_ref(&applied_ref)
            .map_err(GitCallError::publisher)?
        {
            return if applied == expected.source.oid {
                Ok(())
            } else {
                Err(PublisherFailure::Refused)
            };
        }
        if self
            .read_ref(&cancelled_ref)
            .map_err(GitCallError::publisher)?
            .is_some()
        {
            return Err(PublisherFailure::Refused);
        }
        let observed = self.observe_refs(expected, true)?;
        if observed.source_oid != expected.source.oid
            || observed.target_oid != expected.target.oid
            || !observed.source_descends_from_target
        {
            return Err(PublisherFailure::Refused);
        }

        let zero = zero_oid(&expected.source.oid)?;
        let transaction = format!(
            "start\nverify {} {}\nverify {cancelled_ref} {zero}\nupdate {} {} {}\ncreate {applied_ref} {}\nprepare\ncommit\n",
            expected.source.name,
            expected.source.oid,
            expected.target.name,
            expected.source.oid,
            expected.target.oid,
            expected.source.oid
        );
        if self
            .update_refs(&transaction)
            .map_err(GitCallError::publisher)?
        {
            return Ok(());
        }
        if self
            .read_ref(&applied_ref)
            .map_err(GitCallError::publisher)?
            .as_deref()
            == Some(expected.source.oid.as_str())
        {
            return Ok(());
        }
        Err(PublisherFailure::Refused)
    }

    fn verify(
        &self,
        attempt: &PublishAttempt<'_>,
    ) -> Result<PublishVerification, PublisherFailure> {
        let expected = attempt.expected();
        validate_bound_expectation(self, expected)?;
        let (applied_ref, cancelled_ref) = Self::operation_refs(attempt.operation_id())?;
        for _ in 0..VERIFY_RETRIES {
            if let Some(applied_oid) = self
                .read_ref(&applied_ref)
                .map_err(GitCallError::publisher)?
            {
                if applied_oid != expected.source.oid {
                    return Err(PublisherFailure::Unavailable);
                }
                if self
                    .is_symbolic_ref(&expected.target.name)
                    .map_err(GitCallError::publisher)?
                {
                    return Err(PublisherFailure::Unavailable);
                }
                let current_target_oid = self
                    .commit_oid(&expected.target.name)
                    .map_err(GitCallError::publisher)?;
                let target_contains_applied = current_target_oid == applied_oid
                    || self
                        .is_ancestor(&applied_oid, &current_target_oid)
                        .map_err(GitCallError::publisher)?;
                return Ok(PublishVerification::Applied(AppliedPublication {
                    applied_oid,
                    repository_id: self.repository_id.clone(),
                    publisher_scope: self.publisher_scope.clone(),
                    current_target_oid,
                    target_contains_applied,
                }));
            }
            if self
                .read_ref(&cancelled_ref)
                .map_err(GitCallError::publisher)?
                .is_some()
            {
                return self
                    .observe_refs(expected, false)
                    .map(PublishVerification::NotAppliedFinal);
            }
            if self.cancellation(attempt, &applied_ref, &cancelled_ref)? {
                return self
                    .observe_refs(expected, false)
                    .map(PublishVerification::NotAppliedFinal);
            }
        }
        Err(PublisherFailure::Unavailable)
    }
}

fn validate_expectation(expected: &PublishExpectation) -> Result<(), PublisherFailure> {
    if !valid_branch_ref(&expected.source.name)
        || !valid_publish_target_ref(&expected.target.name)
        || expected.source.name == expected.target.name
        || !valid_lower_oid(&expected.source.oid)
        || !valid_lower_oid(&expected.target.oid)
        || expected.source.oid.len() != expected.target.oid.len()
    {
        return Err(PublisherFailure::Refused);
    }
    Ok(())
}

fn validate_bound_expectation(
    publisher: &LocalGitRefPublisher,
    expected: &PublishExpectation,
) -> Result<(), PublisherFailure> {
    validate_expectation(expected)?;
    if expected.repository_id != publisher.repository_id
        || expected.publisher_scope != publisher.publisher_scope
    {
        return Err(PublisherFailure::Refused);
    }
    Ok(())
}

fn valid_lower_oid(value: &str) -> bool {
    valid_oid(value)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_operation_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn zero_oid(oid: &str) -> Result<String, PublisherFailure> {
    valid_lower_oid(oid)
        .then(|| "0".repeat(oid.len()))
        .ok_or(PublisherFailure::Refused)
}

fn bounded_oid(bytes: &[u8]) -> Option<String> {
    let value = std::str::from_utf8(bytes).ok()?.trim();
    valid_lower_oid(value).then(|| value.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::io;
    use std::process::Command;
    use std::sync::{Arc, Barrier};
    use std::time::Instant;

    use tempfile::TempDir;

    use super::*;
    use crate::workflow::{PublishExpectation, RefBinding};

    const TEST_TARGET: &str = "refs/zerocode/integration/main";

    struct Repository {
        root: TempDir,
        base_oid: String,
        source_oid: String,
        publisher: LocalGitRefPublisher,
    }

    fn git_output(root: &Path, args: &[&str]) -> std::process::Output {
        Command::new(crate::GIT_EXECUTABLE)
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git runs")
    }

    fn git(root: &Path, args: &[&str]) -> String {
        let output = git_output(root, args);
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

    fn repository() -> Repository {
        let root = tempfile::tempdir().expect("repository");
        git(root.path(), &["init", "-qb", "main"]);
        git(root.path(), &["config", "user.name", "Publisher Test"]);
        git(
            root.path(),
            &["config", "user.email", "publisher@example.invalid"],
        );
        fs::write(root.path().join("base.txt"), "base\n").expect("base file");
        git(root.path(), &["add", "base.txt"]);
        git(root.path(), &["commit", "-qm", "base"]);
        let base_oid = git(root.path(), &["rev-parse", "HEAD"]);
        git(root.path(), &["branch", "integration", &base_oid]);
        git(root.path(), &["update-ref", TEST_TARGET, &base_oid]);
        git(root.path(), &["switch", "-qc", "feature"]);
        fs::write(root.path().join("feature.txt"), "feature\n").expect("feature file");
        git(root.path(), &["add", "feature.txt"]);
        git(root.path(), &["commit", "-qm", "feature"]);
        let source_oid = git(root.path(), &["rev-parse", "HEAD"]);
        let publisher = LocalGitRefPublisher::open(root.path()).expect("publisher");
        Repository {
            root,
            base_oid,
            source_oid,
            publisher,
        }
    }

    fn expectation(repository: &Repository, target: &str, target_oid: &str) -> PublishExpectation {
        PublishExpectation {
            workflow_id: "workflow-1".to_string(),
            generation: 1,
            manifest_id: "manifest-1".to_string(),
            repository_id: repository.publisher.repository_id().to_string(),
            worktree_id: "worktree-1".to_string(),
            publisher_scope: repository.publisher.publisher_scope().to_string(),
            source: RefBinding {
                name: "refs/heads/feature".to_string(),
                oid: repository.source_oid.clone(),
            },
            target: RefBinding {
                name: target.to_string(),
                oid: target_oid.to_string(),
            },
            policy_digest: "policy-1".to_string(),
        }
    }

    fn operation(index: usize) -> String {
        format!("{index:064x}")
    }

    #[test]
    fn one_transaction_publishes_and_an_operation_retry_is_idempotent() {
        let repository = repository();
        assert!(
            !format!("{:?}", repository.publisher)
                .contains(&repository.root.path().to_string_lossy().to_string())
        );
        let expected = expectation(&repository, TEST_TARGET, &repository.base_oid);
        let operation = operation(1);
        let attempt = PublishAttempt::new(&expected, &operation);
        repository.publisher.commit(&attempt).expect("publish");
        repository
            .publisher
            .commit(&attempt)
            .expect("idempotent retry");
        assert_eq!(
            git(repository.root.path(), &["rev-parse", TEST_TARGET]),
            repository.source_oid
        );
        assert_eq!(
            git(
                repository.root.path(),
                &["rev-parse", &format!("{APPLIED_REF_PREFIX}{operation}")]
            ),
            repository.source_oid
        );
        assert!(matches!(
            repository.publisher.verify(&attempt),
            Ok(PublishVerification::Applied(AppliedPublication {
                ref applied_oid,
                target_contains_applied: true,
                ..
            })) if applied_oid == &repository.source_oid
        ));
    }

    #[test]
    fn applied_receipt_survives_a_later_target_advance() {
        let repository = repository();
        let expected = expectation(&repository, TEST_TARGET, &repository.base_oid);
        let operation = operation(2);
        let attempt = PublishAttempt::new(&expected, &operation);
        repository.publisher.commit(&attempt).expect("publish");
        let tree = git(
            repository.root.path(),
            &["rev-parse", &format!("{}^{{tree}}", repository.source_oid)],
        );
        let later = git(
            repository.root.path(),
            &[
                "commit-tree",
                &tree,
                "-p",
                &repository.source_oid,
                "-m",
                "later",
            ],
        );
        git(
            repository.root.path(),
            &["update-ref", TEST_TARGET, &later, &repository.source_oid],
        );
        git(
            repository.root.path(),
            &[
                "update-ref",
                "-d",
                "refs/heads/feature",
                &repository.source_oid,
            ],
        );
        assert!(matches!(
            repository.publisher.verify(&attempt),
            Ok(PublishVerification::Applied(AppliedPublication {
                ref applied_oid,
                ref current_target_oid,
                target_contains_applied: true,
                ..
            })) if applied_oid == &repository.source_oid && current_target_oid == &later
        ));
    }

    #[test]
    fn stale_and_non_fast_forward_targets_are_refused_without_receipts() {
        let repository = repository();
        let tree = git(
            repository.root.path(),
            &["rev-parse", &format!("{}^{{tree}}", repository.base_oid)],
        );
        let sibling = git(
            repository.root.path(),
            &[
                "commit-tree",
                &tree,
                "-p",
                &repository.base_oid,
                "-m",
                "sibling",
            ],
        );
        git(
            repository.root.path(),
            &["update-ref", TEST_TARGET, &sibling, &repository.base_oid],
        );

        let stale = expectation(&repository, TEST_TARGET, &repository.base_oid);
        let stale_operation = operation(3);
        assert_eq!(
            repository
                .publisher
                .commit(&PublishAttempt::new(&stale, &stale_operation)),
            Err(PublisherFailure::Refused)
        );

        let diverged = expectation(&repository, TEST_TARGET, &sibling);
        let observation = repository.publisher.observe(&diverged).expect("observe");
        assert!(!observation.source_descends_from_target);
        let source_tree = git(
            repository.root.path(),
            &["rev-parse", &format!("{}^{{tree}}", repository.source_oid)],
        );
        let replacement = git(
            repository.root.path(),
            &[
                "commit-tree",
                &source_tree,
                "-p",
                &sibling,
                "-m",
                "replacement-parent",
            ],
        );
        git(
            repository.root.path(),
            &["replace", &repository.source_oid, &replacement],
        );
        assert!(
            git_output(
                repository.root.path(),
                &[
                    "merge-base",
                    "--is-ancestor",
                    &sibling,
                    &repository.source_oid
                ]
            )
            .status
            .success(),
            "the fixture's replace ref changes ordinary Git ancestry"
        );
        assert!(
            !repository
                .publisher
                .observe(&diverged)
                .expect("replacement-disabled observation")
                .source_descends_from_target,
            "the publisher must ignore refs/replace"
        );
        git(
            repository.root.path(),
            &["replace", "-d", &repository.source_oid],
        );
        let graft = repository.root.path().join(".git/info/grafts");
        fs::write(&graft, format!("{} {}\n", repository.source_oid, sibling))
            .expect("legacy graft");
        let grafted = Command::new(crate::GIT_EXECUTABLE)
            .arg("-C")
            .arg(repository.root.path())
            .args([
                "merge-base",
                "--is-ancestor",
                &sibling,
                &repository.source_oid,
            ])
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .output()
            .expect("grafted git");
        assert!(
            grafted.status.success(),
            "GIT_NO_REPLACE_OBJECTS alone does not disable grafts"
        );
        assert!(
            !repository
                .publisher
                .observe(&diverged)
                .expect("graft-disabled observation")
                .source_descends_from_target,
            "the publisher must ignore legacy grafts"
        );
        let diverged_operation = operation(4);
        assert_eq!(
            repository
                .publisher
                .commit(&PublishAttempt::new(&diverged, &diverged_operation)),
            Err(PublisherFailure::Refused)
        );
        for operation in [stale_operation, diverged_operation] {
            assert!(
                repository
                    .publisher
                    .read_ref(&format!("{APPLIED_REF_PREFIX}{operation}"))
                    .expect("read applied")
                    .is_none()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn source_ref_movement_inside_the_commit_window_fails_the_transaction() {
        use std::os::unix::fs::PermissionsExt as _;

        let repository = repository();
        let expected = expectation(&repository, TEST_TARGET, &repository.base_oid);
        let tree = git(
            repository.root.path(),
            &["rev-parse", &format!("{}^{{tree}}", repository.source_oid)],
        );
        let later = git(
            repository.root.path(),
            &[
                "commit-tree",
                &tree,
                "-p",
                &repository.source_oid,
                "-m",
                "source-moved",
            ],
        );
        let wrapper = repository
            .root
            .path()
            .join("move-source-before-transaction");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\ncase \" $* \" in\n  *\" update-ref --no-deref --stdin \"*)\n    git -C '{}' update-ref refs/heads/feature {} {} || exit 91\n    ;;\nesac\nexec git \"$@\"\n",
                repository.root.path().display(),
                later,
                repository.source_oid
            ),
        )
        .expect("git wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("executable");
        let mut publisher = repository.publisher.clone();
        publisher.git = wrapper.into_os_string();
        let operation = operation(10);
        assert_eq!(
            publisher.commit(&PublishAttempt::new(&expected, &operation)),
            Err(PublisherFailure::Refused)
        );
        assert_eq!(
            git(repository.root.path(), &["rev-parse", TEST_TARGET]),
            repository.base_oid
        );
        assert_eq!(
            git(repository.root.path(), &["rev-parse", "refs/heads/feature"]),
            later
        );
        assert!(
            publisher
                .read_ref(&format!("{APPLIED_REF_PREFIX}{operation}"))
                .expect("read applied")
                .is_none()
        );
    }

    #[test]
    fn checked_out_and_unsafe_targets_are_refused() {
        let repository = repository();
        let holder = tempfile::tempdir().expect("worktree holder");
        let checked = holder.path().join("integration");
        git(
            repository.root.path(),
            &[
                "worktree",
                "add",
                checked.to_str().expect("utf8 path"),
                "integration",
            ],
        );
        let expected = expectation(&repository, "refs/heads/integration", &repository.base_oid);
        assert_eq!(
            repository.publisher.observe(&expected),
            Err(PublisherFailure::Refused)
        );
        assert_eq!(
            git(
                repository.root.path(),
                &["rev-parse", "refs/heads/integration"]
            ),
            repository.base_oid
        );
        let integration_holder = holder.path().join("integration-ref");
        git(
            repository.root.path(),
            &[
                "worktree",
                "add",
                "--detach",
                integration_holder.to_str().expect("utf8 path"),
                &repository.base_oid,
            ],
        );
        git(&integration_holder, &["symbolic-ref", "HEAD", TEST_TARGET]);
        let integration_target = expectation(&repository, TEST_TARGET, &repository.base_oid);
        assert_eq!(
            repository.publisher.observe(&integration_target),
            Err(PublisherFailure::Refused)
        );

        for target in [
            "refs/tags/release",
            "refs/heads/a..b",
            "refs/zerocode/integration/../escape",
        ] {
            let unsafe_expected = expectation(&repository, target, &repository.base_oid);
            assert_eq!(
                repository.publisher.observe(&unsafe_expected),
                Err(PublisherFailure::Refused)
            );
        }
    }

    #[test]
    fn symbolic_target_and_operation_refs_are_never_dereferenced() {
        let repository = repository();
        let alias = "refs/zerocode/integration/alias";
        git(
            repository.root.path(),
            &["symbolic-ref", alias, "refs/heads/feature"],
        );
        let symbolic_target = expectation(&repository, alias, &repository.source_oid);
        assert_eq!(
            repository.publisher.observe(&symbolic_target),
            Err(PublisherFailure::Refused)
        );
        assert_eq!(
            git(repository.root.path(), &["rev-parse", "refs/heads/feature"]),
            repository.source_oid
        );

        for (index, prefix, victim) in [
            (8, APPLIED_REF_PREFIX, "refs/heads/applied-victim"),
            (9, CANCELLED_REF_PREFIX, "refs/heads/cancelled-victim"),
        ] {
            let target = format!("refs/zerocode/integration/symbolic-{index}");
            git(
                repository.root.path(),
                &["update-ref", &target, &repository.base_oid],
            );
            let operation = operation(index);
            git(
                repository.root.path(),
                &["symbolic-ref", &format!("{prefix}{operation}"), victim],
            );
            let expected = expectation(&repository, &target, &repository.base_oid);
            assert_eq!(
                repository
                    .publisher
                    .commit(&PublishAttempt::new(&expected, &operation)),
                Err(PublisherFailure::Refused)
            );
            assert_eq!(
                git(repository.root.path(), &["rev-parse", &target]),
                repository.base_oid
            );
            assert!(
                !git_output(repository.root.path(), &["show-ref", "--verify", victim])
                    .status
                    .success()
            );
        }
    }

    #[test]
    fn designated_integration_ref_is_publishable() {
        let repository = repository();
        let target = "refs/zerocode/integration/run-1";
        git(
            repository.root.path(),
            &["update-ref", target, &repository.base_oid],
        );
        let expected = expectation(&repository, target, &repository.base_oid);
        let operation = operation(5);
        repository
            .publisher
            .commit(&PublishAttempt::new(&expected, &operation))
            .expect("integration publish");
        assert_eq!(
            git(repository.root.path(), &["rev-parse", target]),
            repository.source_oid
        );
    }

    #[test]
    fn shell_shaped_ref_text_remains_literal_data() {
        let repository = repository();
        let target = "refs/zerocode/integration/$(touch-owned)";
        git(
            repository.root.path(),
            &["update-ref", target, &repository.base_oid],
        );
        let expected = expectation(&repository, target, &repository.base_oid);
        let operation = operation(6);
        repository
            .publisher
            .commit(&PublishAttempt::new(&expected, &operation))
            .expect("literal ref publish");
        assert_eq!(
            git(repository.root.path(), &["rev-parse", target]),
            repository.source_oid
        );
        assert!(!repository.root.path().join("touch-owned").exists());
    }

    #[test]
    fn a_cancelled_operation_can_never_move_the_target() {
        let repository = repository();
        let target = "refs/zerocode/integration/cancelled";
        git(
            repository.root.path(),
            &["update-ref", target, &repository.base_oid],
        );
        let expected = expectation(&repository, target, &repository.base_oid);
        let operation = operation(7);
        let attempt = PublishAttempt::new(&expected, &operation);
        assert!(matches!(
            repository.publisher.verify(&attempt),
            Ok(PublishVerification::NotAppliedFinal(_))
        ));
        assert_eq!(
            repository.publisher.commit(&attempt),
            Err(PublisherFailure::Refused)
        );
        assert_eq!(
            git(repository.root.path(), &["rev-parse", target]),
            repository.base_oid
        );
        assert!(
            repository
                .publisher
                .read_ref(&format!("{CANCELLED_REF_PREFIX}{operation}"))
                .expect("read cancellation")
                .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_descendant_holding_git_pipes_is_killed_without_extending_the_deadline() {
        use std::os::unix::fs::PermissionsExt as _;

        let repository = repository();
        let wrapper = repository.root.path().join("git-wrapper");
        let marker = repository.root.path().join("descendant.pid");
        fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nsleep 30 &\necho $! > '{}'\nexit 0\n",
                marker.display()
            ),
        )
        .expect("wrapper");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("executable");
        let mut publisher = repository.publisher.clone();
        publisher.git = wrapper.into_os_string();
        publisher.timeout = Duration::from_millis(500);
        let started = Instant::now();
        let args = LocalGitRefPublisher::args(["--version"]);
        let timed_out = match publisher.run_git(&args, None) {
            Ok(answer) => {
                assert!(answer.status.success());
                false
            }
            Err(GitCallError::Timeout) => true,
            Err(other) => panic!("unexpected wrapper result: {other:?}"),
        };
        assert!(started.elapsed() < Duration::from_secs(2));
        let Ok(marker) = fs::read_to_string(marker) else {
            assert!(
                timed_out,
                "a successful wrapper returned without recording the descendant it started"
            );
            // Under heavy parallel-test load the deadline can expire before
            // the wrapper is scheduled at all. In that case there is no
            // descendant to inspect; the timeout itself is the expected
            // bounded result.
            return;
        };
        let descendant: i32 = marker.trim().parse().expect("numeric pid");
        for _ in 0..50 {
            // SAFETY: signal 0 only asks whether this child pid still exists.
            let exists = unsafe { libc::kill(descendant, 0) } == 0
                || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
            if !exists {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("the Git descendant survived its process group");
    }

    #[test]
    fn commit_and_cancellation_race_has_exactly_one_winner() {
        let repository = repository();
        for index in 16..32 {
            let target = format!("refs/zerocode/integration/race-{index}");
            git(
                repository.root.path(),
                &["update-ref", &target, &repository.base_oid],
            );
            let expected = Arc::new(expectation(&repository, &target, &repository.base_oid));
            let operation = Arc::new(operation(index));
            let barrier = Arc::new(Barrier::new(3));
            let commit_publisher = repository.publisher.clone();
            let commit_expected = Arc::clone(&expected);
            let commit_operation = Arc::clone(&operation);
            let commit_barrier = Arc::clone(&barrier);
            let committing = std::thread::spawn(move || {
                commit_barrier.wait();
                commit_publisher.commit(&PublishAttempt::new(
                    commit_expected.as_ref(),
                    commit_operation.as_str(),
                ))
            });
            let cancel_publisher = repository.publisher.clone();
            let cancel_expected = Arc::clone(&expected);
            let cancel_operation = Arc::clone(&operation);
            let cancel_barrier = Arc::clone(&barrier);
            let cancelling = std::thread::spawn(move || {
                cancel_barrier.wait();
                cancel_publisher.verify(&PublishAttempt::new(
                    cancel_expected.as_ref(),
                    cancel_operation.as_str(),
                ))
            });
            barrier.wait();
            let committed = committing.join().expect("commit thread");
            let cancelled = cancelling.join().expect("cancel thread");
            let applied_ref = format!("{APPLIED_REF_PREFIX}{operation}");
            let cancelled_ref = format!("{CANCELLED_REF_PREFIX}{operation}");
            let applied = repository
                .publisher
                .read_ref(&applied_ref)
                .expect("read applied");
            let tombstone = repository
                .publisher
                .read_ref(&cancelled_ref)
                .expect("read cancelled");
            assert_ne!(applied.is_some(), tombstone.is_some());
            if applied.is_some() {
                assert_eq!(committed, Ok(()));
                assert!(matches!(cancelled, Ok(PublishVerification::Applied(_))));
            } else {
                assert_eq!(committed, Err(PublisherFailure::Refused));
                assert!(matches!(
                    cancelled,
                    Ok(PublishVerification::NotAppliedFinal(_))
                ));
            }
        }
    }
}
