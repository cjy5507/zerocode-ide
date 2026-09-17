//! Durable test evidence and the host-owned disposable-worktree runner.
//!
//! Commands enter through [`VerificationCatalog`], never through a manifest or
//! repository file. Each command runs without a shell in a disposable detached
//! worktree at the manifest's exact source OID. Only bounded digests and the
//! typed receipt reach SQLite; local paths and command output do not.
//! The catalog is trusted host policy, not a daemon-containment sandbox: Unix
//! process groups stop ordinary descendants while nonblocking pipes keep the
//! deadline bounded if test code deliberately leaves that group.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension as _, TransactionBehavior, params};
use sha2::{Digest, Sha256};

use crate::bounded_process::{self, ProcessError, ProcessLimits, ProcessOutput};
use crate::handoff::{HANDOFF_SCHEMA_VERSION, HandoffManifestV1, TestReceipt, TestRequirement};
use crate::publish::{EvidenceFailure, TrustedTestReceiptResolver};
use crate::workflow::{valid_identity, valid_oid};
use crate::workflow_store::{WorkflowStore, WorkflowStoreError};
use crate::{Orchestrator, PendingLoss, git_command};

pub const MAX_VERIFICATION_COMMANDS: usize = 256;
pub const MAX_VERIFICATION_ARGS: usize = 128;
pub const MAX_VERIFICATION_WORD_BYTES: usize = 16 * 1024;
pub const MAX_VERIFICATION_TIMEOUT: Duration = Duration::from_secs(10 * 60);
pub const TEST_STDOUT_LIMIT: usize = 1024 * 1024;
pub const TEST_STDERR_LIMIT: usize = 1024 * 1024;
pub const VERIFICATION_GIT_TIMEOUT: Duration = Duration::from_secs(30);

const GIT_OUTPUT_LIMIT: usize = 1024 * 1024;
const WORKTREE_ATTEMPTS: usize = 100;
#[cfg(windows)]
const EMPTY_GRAFT_FILE: &str = "NUL";
#[cfg(not(windows))]
const EMPTY_GRAFT_FILE: &str = "/dev/null";

static NEXT_WORKTREE: AtomicU64 = AtomicU64::new(0);

/// One executable and argv chosen by the host application.
///
/// A command id is an immutable semantic version. Change it whenever the
/// program or argv changes; the durable resolver rejects a rebound id.
#[derive(Clone)]
pub struct VerificationCommand {
    command_id: String,
    program: OsString,
    args: Vec<OsString>,
    timeout: Duration,
}

impl std::fmt::Debug for VerificationCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerificationCommand")
            .field("command_id", &self.command_id)
            .field("program", &"<host-owned-program>")
            .field("arg_count", &self.args.len())
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl VerificationCommand {
    pub fn new(
        command_id: impl Into<String>,
        program: impl Into<OsString>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
        timeout: Duration,
    ) -> Result<Self, VerificationConfigError> {
        let command_id = command_id.into();
        let program = program.into();
        let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
        if !valid_identity(&command_id)
            || os_bytes(program.as_os_str()).is_empty()
            || !valid_program(program.as_os_str())
            || os_bytes(program.as_os_str()).len() > MAX_VERIFICATION_WORD_BYTES
            || args.len() > MAX_VERIFICATION_ARGS
            || args
                .iter()
                .any(|arg| os_bytes(arg.as_os_str()).len() > MAX_VERIFICATION_WORD_BYTES)
            || timeout.is_zero()
            || timeout > MAX_VERIFICATION_TIMEOUT
        {
            return Err(VerificationConfigError::InvalidCommand);
        }
        Ok(Self {
            command_id,
            program,
            args,
            timeout,
        })
    }

    #[must_use]
    pub fn command_id(&self) -> &str {
        &self.command_id
    }
}

/// The complete allowlist. Duplicate ids are refused rather than shadowed.
#[derive(Debug, Clone)]
pub struct VerificationCatalog {
    commands: Vec<VerificationCommand>,
}

impl VerificationCatalog {
    pub fn new(
        commands: impl IntoIterator<Item = VerificationCommand>,
    ) -> Result<Self, VerificationConfigError> {
        let commands: Vec<VerificationCommand> = commands.into_iter().collect();
        if commands.len() > MAX_VERIFICATION_COMMANDS {
            return Err(VerificationConfigError::TooManyCommands);
        }
        let mut ids = HashSet::with_capacity(commands.len());
        if commands
            .iter()
            .any(|command| !ids.insert(command.command_id.clone()))
        {
            return Err(VerificationConfigError::DuplicateCommand);
        }
        Ok(Self { commands })
    }

    fn command(&self, command_id: &str) -> Option<&VerificationCommand> {
        self.commands
            .iter()
            .find(|command| command.command_id == command_id)
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerificationConfigError {
    #[error("a verification command is invalid or exceeds its bounds")]
    InvalidCommand,
    #[error("the verification catalog has too many commands")]
    TooManyCommands,
    #[error("the verification catalog repeats a command id")]
    DuplicateCommand,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum VerificationError {
    #[error("the verification runner is not available on this platform")]
    UnsupportedPlatform,
    #[error("the verification root is not a private plain directory")]
    UnsafeRoot,
    #[error("the requested verification command is not in the host allowlist")]
    UnknownCommand,
    #[error("the verification command changed without a new command id")]
    CommandBinding,
    #[error("the manifest belongs to another repository")]
    RepositoryMismatch,
    #[error("the manifest identity does not match its immutable contents")]
    ManifestMismatch,
    #[error("the manifest source OID is invalid or unavailable")]
    InvalidSource,
    #[error("the source requires a repository-controlled filter or submodule materializer")]
    UnsupportedSource,
    #[error("a disposable verification worktree could not be created")]
    WorktreeCreate,
    #[error("the disposable verification worktree escaped its private root")]
    WorktreeEscaped,
    #[error("the verification command timed out")]
    Timeout,
    #[error("the verification command could not be run")]
    Unavailable,
    #[error("the disposable verification worktree could not be removed")]
    Cleanup,
    #[error(transparent)]
    Evidence(#[from] EvidenceFailure),
}

#[derive(Clone)]
struct PrivateVerificationRoot(PathBuf);

impl std::fmt::Debug for PrivateVerificationRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<private-verification-root>")
    }
}

struct DisposableCheckout {
    root: PathBuf,
    path: PathBuf,
    armed: bool,
}

impl DisposableCheckout {
    fn new(root: &Path, path: PathBuf) -> Self {
        Self {
            root: root.to_path_buf(),
            path,
            armed: true,
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(&mut self) -> Result<(), VerificationError> {
        if !self.armed {
            return Ok(());
        }
        remove_disposable(&self.root, &self.path)?;
        self.armed = false;
        Ok(())
    }

    fn fail(mut self, error: VerificationError) -> VerificationError {
        match self.cleanup() {
            Ok(()) => error,
            Err(cleanup) => cleanup,
        }
    }
}

impl Drop for DisposableCheckout {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Runs host-owned commands against immutable source commits.
#[derive(Clone)]
pub struct VerificationRunner {
    root: PrivateVerificationRoot,
    catalog: VerificationCatalog,
    git: OsString,
    git_timeout: Duration,
    git_output_limit: usize,
    stdout_limit: usize,
    stderr_limit: usize,
}

impl std::fmt::Debug for VerificationRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerificationRunner")
            .field("root", &self.root)
            .field("catalog", &self.catalog)
            .finish()
    }
}

impl VerificationRunner {
    pub fn open(
        root: impl AsRef<Path>,
        catalog: VerificationCatalog,
    ) -> Result<Self, VerificationError> {
        Self::open_with_git(root, catalog, crate::GIT_EXECUTABLE)
    }

    pub fn open_with_git(
        root: impl AsRef<Path>,
        catalog: VerificationCatalog,
        git: impl Into<OsString>,
    ) -> Result<Self, VerificationError> {
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (root, catalog, git);
            return Err(VerificationError::UnsupportedPlatform);
        }
        #[cfg(any(unix, windows))]
        {
            let root = private_root(root.as_ref())?;
            let git = resolve_host_executable(git.into()).ok_or(VerificationError::Unavailable)?;
            Ok(Self {
                root: PrivateVerificationRoot(root),
                catalog,
                git: git.into_os_string(),
                git_timeout: VERIFICATION_GIT_TIMEOUT,
                git_output_limit: GIT_OUTPUT_LIMIT,
                stdout_limit: TEST_STDOUT_LIMIT,
                stderr_limit: TEST_STDERR_LIMIT,
            })
        }
    }

    pub fn run(
        &self,
        orchestrator: &Orchestrator,
        store: &WorkflowStore,
        manifest: &HandoffManifestV1,
        requirement: &TestRequirement,
    ) -> Result<TrustedTestEvidence, VerificationError> {
        if !valid_manifest_binding(manifest) {
            return Err(VerificationError::ManifestMismatch);
        }
        let command = self
            .catalog
            .command(&requirement.command_id)
            .ok_or(VerificationError::UnknownCommand)?;
        let expected_command_digest = command_digest(command, self.stdout_limit, self.stderr_limit);
        let key = EvidenceKey::new(
            &manifest.manifest_id,
            &requirement.name,
            &requirement.command_id,
            &manifest.worktree.content_digest,
            &expected_command_digest,
        )?;
        if !valid_lower_oid(&manifest.worktree.head_oid) {
            return Err(VerificationError::InvalidSource);
        }
        let repository_id = orchestrator
            .repository_id(orchestrator.repo_root())
            .map_err(|_| VerificationError::RepositoryMismatch)?;
        if repository_id != manifest.worktree.repository_id {
            return Err(VerificationError::RepositoryMismatch);
        }
        if let Some(existing) = load_evidence(store, &key).map_err(|error| match error {
            EvidenceFailure::CommandBinding => VerificationError::CommandBinding,
            other => VerificationError::Evidence(other),
        })? {
            return Ok(existing);
        }

        let mut worktree = self.create_worktree(orchestrator, &manifest.worktree.head_oid)?;
        if let Err(error) = self.verify_source_content(
            worktree.path(),
            &manifest.worktree.head_oid,
            &manifest.worktree.content_digest,
        ) {
            return Err(worktree.fail(error));
        }
        let started_at_ms = now_ms();
        let executed = self.execute(command, worktree.path());
        let ended_at_ms = now_ms().max(started_at_ms);
        if worktree.cleanup().is_err() {
            return Err(VerificationError::Cleanup);
        }
        let output = executed?;
        let exit_code = output.status.code().unwrap_or(-1);
        let command_digest = expected_command_digest;
        let stdout_digest = bytes_digest(b"stdout", &output.stdout);
        let stderr_digest = bytes_digest(b"stderr", &output.stderr);
        let output_truncated = output.stdout_truncated || output.stderr_truncated;
        let mut receipt = TestReceipt::new(
            requirement.name.clone(),
            requirement.command_id.clone(),
            manifest.worktree.content_digest.clone(),
            started_at_ms,
            ended_at_ms,
            exit_code,
        );
        receipt.output_truncated = output_truncated;
        let mut evidence = TrustedTestEvidence {
            manifest_id: manifest.manifest_id.clone(),
            receipt,
            source_oid: manifest.worktree.head_oid.clone(),
            command_digest,
            stdout_digest,
            stderr_digest,
            evidence_digest: String::new(),
        };
        evidence.evidence_digest = evidence_digest(&evidence)?;
        evidence.receipt.output_artifact_id =
            Some(format!("test-evidence-{}", evidence.evidence_digest));
        record_evidence(store, &evidence)
    }

    /// Bind durable rows to this runner's current host-owned command catalog.
    #[must_use]
    pub fn resolver<'a>(&'a self, store: &'a WorkflowStore) -> TrustedEvidenceResolver<'a> {
        TrustedEvidenceResolver {
            store,
            catalog: &self.catalog,
            stdout_limit: self.stdout_limit,
            stderr_limit: self.stderr_limit,
        }
    }

    fn execute(
        &self,
        configured: &VerificationCommand,
        worktree: &Path,
    ) -> Result<ProcessOutput, VerificationError> {
        let mut command = Command::new(&configured.program);
        command.args(&configured.args).current_dir(worktree);
        sanitize_git_environment(&mut command);
        bounded_process::run(
            &mut command,
            None,
            ProcessLimits {
                timeout: configured.timeout,
                stdout_bytes: self.stdout_limit,
                stderr_bytes: self.stderr_limit,
            },
        )
        .map_err(process_error)
    }

    fn create_worktree(
        &self,
        orchestrator: &Orchestrator,
        source_oid: &str,
    ) -> Result<DisposableCheckout, VerificationError> {
        for _ in 0..WORKTREE_ATTEMPTS {
            let sequence = NEXT_WORKTREE.fetch_add(1, Ordering::Relaxed);
            let path = self
                .root
                .0
                .join(format!("verify-{}-{sequence}", std::process::id()));
            if fs::symlink_metadata(&path).is_ok() {
                continue;
            }
            let candidate = DisposableCheckout::new(&self.root.0, path.clone());
            let words = vec![
                OsString::from("clone"),
                OsString::from("--no-checkout"),
                OsString::from("--no-local"),
                OsString::from("--no-tags"),
                OsString::from("--"),
                orchestrator.repo_root().as_os_str().to_owned(),
                path.as_os_str().to_owned(),
            ];
            let output = match self.run_git(&self.root.0, &words) {
                Ok(output) => output,
                Err(error) => return Err(candidate.fail(error)),
            };
            if !output.status.success() {
                return Err(candidate.fail(VerificationError::WorktreeCreate));
            }
            match self.materialize_clone(&path, source_oid) {
                Ok(canonical) if canonical == path => return Ok(candidate),
                Ok(_) => return Err(candidate.fail(VerificationError::WorktreeEscaped)),
                Err(error) => return Err(candidate.fail(error)),
            }
        }
        Err(VerificationError::WorktreeCreate)
    }

    fn materialize_clone(
        &self,
        path: &Path,
        source_oid: &str,
    ) -> Result<PathBuf, VerificationError> {
        let canonical = path
            .canonicalize()
            .map_err(|_| VerificationError::WorktreeCreate)?;
        if canonical.parent() != Some(self.root.0.as_path()) {
            return Err(VerificationError::WorktreeEscaped);
        }
        let checkout = self.run_git(
            &canonical,
            &[
                OsString::from("checkout"),
                OsString::from("--detach"),
                OsString::from("--force"),
                OsString::from(source_oid),
                OsString::from("--"),
            ],
        )?;
        if !checkout.status.success() {
            return Err(VerificationError::InvalidSource);
        }
        let head = self.run_git(
            &canonical,
            &[
                OsString::from("rev-parse"),
                OsString::from("--verify"),
                OsString::from("HEAD^{commit}"),
            ],
        )?;
        if !head.status.success()
            || std::str::from_utf8(&head.stdout).ok().map(str::trim) != Some(source_oid)
        {
            return Err(VerificationError::InvalidSource);
        }
        let symbolic = self.run_git(
            &canonical,
            &[
                OsString::from("symbolic-ref"),
                OsString::from("--quiet"),
                OsString::from("HEAD"),
            ],
        )?;
        if symbolic.status.code() != Some(1) {
            return Err(VerificationError::InvalidSource);
        }
        Ok(canonical)
    }

    fn verify_source_content(
        &self,
        worktree: &Path,
        source_oid: &str,
        expected_digest: &str,
    ) -> Result<(), VerificationError> {
        let status = self.required_git(
            worktree,
            &[
                OsString::from("status"),
                OsString::from("--porcelain=v2"),
                OsString::from("-z"),
                OsString::from("--ignored=matching"),
                OsString::from("--ignore-submodules=none"),
            ],
        )?;
        if !status.is_empty() {
            return Err(VerificationError::InvalidSource);
        }
        let index = self.required_git(
            worktree,
            &[
                OsString::from("ls-files"),
                OsString::from("--stage"),
                OsString::from("-z"),
            ],
        )?;
        if index
            .split(|byte| *byte == 0)
            .any(|entry| entry.starts_with(b"160000 "))
        {
            return Err(VerificationError::UnsupportedSource);
        }
        let visibility = self.required_git(
            worktree,
            &[
                OsString::from("ls-files"),
                OsString::from("-v"),
                OsString::from("-z"),
            ],
        )?;
        std::str::from_utf8(&visibility).map_err(|_| VerificationError::InvalidSource)?;
        let untracked = self.required_git(
            worktree,
            &[
                OsString::from("ls-files"),
                OsString::from("--others"),
                OsString::from("--exclude-standard"),
                OsString::from("-z"),
            ],
        )?;
        if !untracked.is_empty() {
            return Err(VerificationError::InvalidSource);
        }
        let paths = self.required_git(
            worktree,
            &[OsString::from("ls-files"), OsString::from("-z")],
        )?;
        let attributes = self.run_git_with_input(
            worktree,
            &[
                OsString::from("check-attr"),
                OsString::from("--cached"),
                OsString::from("-z"),
                OsString::from("--stdin"),
                OsString::from("filter"),
            ],
            Some(&paths),
        )?;
        if !attributes.status.success() || selected_filter(&attributes.stdout)? {
            return Err(VerificationError::UnsupportedSource);
        }
        let unstaged = self.required_git(worktree, &diff_words(false))?;
        let staged = self.required_git(worktree, &diff_words(true))?;
        if !unstaged.is_empty() || !staged.is_empty() {
            return Err(VerificationError::InvalidSource);
        }

        let mut digest = Sha256::new();
        hash_frame(&mut digest, b"head", source_oid.as_bytes());
        hash_frame(&mut digest, b"index", &index);
        hash_frame(&mut digest, b"index_visibility", &visibility);
        hash_frame(
            &mut digest,
            b"status",
            PendingLoss::default().fingerprint().as_bytes(),
        );
        hash_frame(&mut digest, b"untracked_paths", &untracked);
        hash_frame(&mut digest, b"unstaged_diff", &unstaged);
        hash_frame(&mut digest, b"staged_diff", &staged);
        if format!("{:x}", digest.finalize()) != expected_digest {
            return Err(VerificationError::InvalidSource);
        }
        Ok(())
    }

    fn required_git(&self, cwd: &Path, words: &[OsString]) -> Result<Vec<u8>, VerificationError> {
        let output = self.run_git(cwd, words)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(VerificationError::Unavailable)
        }
    }

    fn run_git(&self, cwd: &Path, words: &[OsString]) -> Result<ProcessOutput, VerificationError> {
        self.run_git_with_input(cwd, words, None)
    }

    fn run_git_with_input(
        &self,
        cwd: &Path,
        words: &[OsString],
        input: Option<&[u8]>,
    ) -> Result<ProcessOutput, VerificationError> {
        let mut command = git_command(&self.git, cwd);
        command
            .arg("-c")
            .arg(format!("core.hooksPath={EMPTY_GRAFT_FILE}"))
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-c")
            .arg(format!("init.templateDir={EMPTY_GRAFT_FILE}"));
        sanitize_git_environment(&mut command);
        command.args(words);
        let output = bounded_process::run(
            &mut command,
            input,
            ProcessLimits {
                timeout: self.git_timeout,
                stdout_bytes: self.git_output_limit,
                stderr_bytes: self.git_output_limit,
            },
        )
        .map_err(process_error)?;
        if output.stdout_truncated || output.stderr_truncated {
            return Err(VerificationError::Unavailable);
        }
        Ok(output)
    }
}

/// Immutable row returned by concurrent duplicate runs and after restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedTestEvidence {
    manifest_id: String,
    receipt: TestReceipt,
    source_oid: String,
    command_digest: String,
    stdout_digest: String,
    stderr_digest: String,
    evidence_digest: String,
}

impl TrustedTestEvidence {
    #[must_use]
    pub fn manifest_id(&self) -> &str {
        &self.manifest_id
    }

    #[must_use]
    pub fn receipt(&self) -> &TestReceipt {
        &self.receipt
    }

    #[must_use]
    pub fn source_oid(&self) -> &str {
        &self.source_oid
    }

    #[must_use]
    pub fn evidence_digest(&self) -> &str {
        &self.evidence_digest
    }

    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.receipt.exit_code == 0
    }
}

#[derive(Debug, Clone)]
struct EvidenceKey {
    manifest_id: String,
    test_name: String,
    command_id: String,
    snapshot_digest: String,
    command_digest: String,
}

impl EvidenceKey {
    fn new(
        manifest_id: &str,
        test_name: &str,
        command_id: &str,
        snapshot_digest: &str,
        command_digest: &str,
    ) -> Result<Self, EvidenceFailure> {
        if !valid_identity(manifest_id)
            || !valid_identity(test_name)
            || !valid_identity(command_id)
            || !valid_digest(snapshot_digest)
            || !valid_digest(command_digest)
        {
            return Err(EvidenceFailure::Corrupt);
        }
        Ok(Self {
            manifest_id: manifest_id.to_string(),
            test_name: test_name.to_string(),
            command_id: command_id.to_string(),
            snapshot_digest: snapshot_digest.to_string(),
            command_digest: command_digest.to_string(),
        })
    }
}

/// Trusted receipt lookup paired with the exact host command catalog.
pub struct TrustedEvidenceResolver<'a> {
    store: &'a WorkflowStore,
    catalog: &'a VerificationCatalog,
    stdout_limit: usize,
    stderr_limit: usize,
}

impl TrustedTestReceiptResolver for TrustedEvidenceResolver<'_> {
    fn resolve(
        &self,
        manifest_id: &str,
        requirement: &TestRequirement,
        snapshot_digest: &str,
    ) -> Result<Option<TestReceipt>, EvidenceFailure> {
        let command = self
            .catalog
            .command(&requirement.command_id)
            .ok_or(EvidenceFailure::CommandBinding)?;
        let expected_digest = command_digest(command, self.stdout_limit, self.stderr_limit);
        let key = EvidenceKey::new(
            manifest_id,
            &requirement.name,
            &requirement.command_id,
            snapshot_digest,
            &expected_digest,
        )?;
        load_evidence(self.store, &key).map(|found| found.map(|evidence| evidence.receipt))
    }
}

fn load_evidence(
    store: &WorkflowStore,
    key: &EvidenceKey,
) -> Result<Option<TrustedTestEvidence>, EvidenceFailure> {
    let connection = store.connection().map_err(store_error)?;
    load_from(&connection, key)
}

fn load_from(
    connection: &Connection,
    key: &EvidenceKey,
) -> Result<Option<TrustedTestEvidence>, EvidenceFailure> {
    let row = connection
        .query_row(
            "SELECT source_oid, started_at_ms, ended_at_ms, exit_code, succeeded, command_digest,
                    stdout_digest, stderr_digest, evidence_digest, output_artifact_id,
                    output_truncated
               FROM trusted_test_evidence
              WHERE manifest_id = ?1 AND test_name = ?2 AND command_id = ?3
                    AND snapshot_digest = ?4",
            params![
                key.manifest_id,
                key.test_name,
                key.command_id,
                key.snapshot_digest,
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, i64>(10)?,
                ))
            },
        )
        .optional()
        .map_err(|_| EvidenceFailure::Unavailable)?;
    let Some((
        source_oid,
        started_at_ms,
        ended_at_ms,
        exit_code,
        succeeded,
        command_digest,
        stdout_digest,
        stderr_digest,
        stored_digest,
        output_artifact_id,
        output_truncated,
    )) = row
    else {
        return Ok(None);
    };
    if command_digest != key.command_digest {
        return Err(EvidenceFailure::CommandBinding);
    }
    let exit_code = i32::try_from(exit_code).map_err(|_| EvidenceFailure::Corrupt)?;
    if succeeded != i64::from(exit_code == 0) {
        return Err(EvidenceFailure::Corrupt);
    }
    let mut receipt = TestReceipt::new(
        key.test_name.clone(),
        key.command_id.clone(),
        key.snapshot_digest.clone(),
        started_at_ms,
        ended_at_ms,
        exit_code,
    );
    receipt.output_artifact_id = Some(output_artifact_id);
    receipt.output_truncated = match output_truncated {
        0 => false,
        1 => true,
        _ => return Err(EvidenceFailure::Corrupt),
    };
    let evidence = TrustedTestEvidence {
        manifest_id: key.manifest_id.clone(),
        receipt,
        source_oid,
        command_digest,
        stdout_digest,
        stderr_digest,
        evidence_digest: stored_digest.clone(),
    };
    let expected_artifact = format!("test-evidence-{stored_digest}");
    if !valid_lower_oid(&evidence.source_oid)
        || !valid_digest(&evidence.command_digest)
        || !valid_digest(&evidence.stdout_digest)
        || !valid_digest(&evidence.stderr_digest)
        || !valid_digest(&stored_digest)
        || evidence_digest(&evidence)? != stored_digest
        || evidence.receipt.output_artifact_id.as_deref() != Some(expected_artifact.as_str())
    {
        return Err(EvidenceFailure::Corrupt);
    }
    Ok(Some(evidence))
}

fn record_evidence(
    store: &WorkflowStore,
    evidence: &TrustedTestEvidence,
) -> Result<TrustedTestEvidence, VerificationError> {
    validate_evidence(evidence)?;
    let key = EvidenceKey::new(
        &evidence.manifest_id,
        &evidence.receipt.name,
        &evidence.receipt.command_id,
        &evidence.receipt.snapshot_digest,
        &evidence.command_digest,
    )?;
    let artifact = evidence
        .receipt
        .output_artifact_id
        .as_deref()
        .ok_or(EvidenceFailure::Corrupt)?;
    let mut connection = store.connection().map_err(store_error)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| EvidenceFailure::Unavailable)?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO trusted_test_evidence (
                manifest_id, test_name, command_id, snapshot_digest, source_oid,
                started_at_ms, ended_at_ms, exit_code, succeeded, command_digest,
                stdout_digest, stderr_digest, evidence_digest, output_artifact_id,
                output_truncated
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                evidence.manifest_id,
                evidence.receipt.name,
                evidence.receipt.command_id,
                evidence.receipt.snapshot_digest,
                evidence.source_oid,
                evidence.receipt.started_at_ms,
                evidence.receipt.ended_at_ms,
                i64::from(evidence.receipt.exit_code),
                i64::from(evidence.succeeded()),
                evidence.command_digest,
                evidence.stdout_digest,
                evidence.stderr_digest,
                evidence.evidence_digest,
                artifact,
                i64::from(evidence.receipt.output_truncated),
            ],
        )
        .map_err(|_| EvidenceFailure::Unavailable)?;
    let canonical = load_from(&transaction, &key)?
        .ok_or(VerificationError::Evidence(EvidenceFailure::Corrupt))?;
    transaction
        .commit()
        .map_err(|_| EvidenceFailure::Unavailable)?;
    Ok(canonical)
}

fn validate_evidence(evidence: &TrustedTestEvidence) -> Result<(), VerificationError> {
    let expected_artifact = format!("test-evidence-{}", evidence.evidence_digest);
    if !valid_identity(&evidence.manifest_id)
        || !valid_identity(&evidence.receipt.name)
        || !valid_identity(&evidence.receipt.command_id)
        || !valid_digest(&evidence.receipt.snapshot_digest)
        || !valid_lower_oid(&evidence.source_oid)
        || !valid_digest(&evidence.command_digest)
        || !valid_digest(&evidence.stdout_digest)
        || !valid_digest(&evidence.stderr_digest)
        || !valid_digest(&evidence.evidence_digest)
        || evidence.receipt.started_at_ms > evidence.receipt.ended_at_ms
        || evidence_digest(evidence)? != evidence.evidence_digest
        || evidence.receipt.output_artifact_id.as_deref() != Some(expected_artifact.as_str())
    {
        return Err(VerificationError::Evidence(EvidenceFailure::Corrupt));
    }
    Ok(())
}

fn evidence_digest(evidence: &TrustedTestEvidence) -> Result<String, EvidenceFailure> {
    let mut digest = Sha256::new();
    for (label, value) in [
        (b"manifest_id".as_slice(), evidence.manifest_id.as_bytes()),
        (b"name".as_slice(), evidence.receipt.name.as_bytes()),
        (
            b"command_id".as_slice(),
            evidence.receipt.command_id.as_bytes(),
        ),
        (
            b"snapshot_digest".as_slice(),
            evidence.receipt.snapshot_digest.as_bytes(),
        ),
        (b"source_oid".as_slice(), evidence.source_oid.as_bytes()),
        (
            b"command_digest".as_slice(),
            evidence.command_digest.as_bytes(),
        ),
        (
            b"stdout_digest".as_slice(),
            evidence.stdout_digest.as_bytes(),
        ),
        (
            b"stderr_digest".as_slice(),
            evidence.stderr_digest.as_bytes(),
        ),
    ] {
        hash_frame(&mut digest, label, value);
    }
    hash_frame(
        &mut digest,
        b"started_at_ms",
        &evidence.receipt.started_at_ms.to_le_bytes(),
    );
    hash_frame(
        &mut digest,
        b"ended_at_ms",
        &evidence.receipt.ended_at_ms.to_le_bytes(),
    );
    hash_frame(
        &mut digest,
        b"exit_code",
        &evidence.receipt.exit_code.to_le_bytes(),
    );
    hash_frame(
        &mut digest,
        b"output_truncated",
        &[u8::from(evidence.receipt.output_truncated)],
    );
    Ok(format!("{:x}", digest.finalize()))
}

fn command_digest(
    command: &VerificationCommand,
    stdout_limit: usize,
    stderr_limit: usize,
) -> String {
    let mut digest = Sha256::new();
    hash_frame(&mut digest, b"program", &os_bytes(&command.program));
    for argument in &command.args {
        hash_frame(&mut digest, b"argument", &os_bytes(argument));
    }
    hash_frame(
        &mut digest,
        b"timeout_secs",
        &command.timeout.as_secs().to_le_bytes(),
    );
    hash_frame(
        &mut digest,
        b"timeout_nanos",
        &command.timeout.subsec_nanos().to_le_bytes(),
    );
    hash_frame(
        &mut digest,
        b"stdout_limit",
        &u64::try_from(stdout_limit)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    hash_frame(
        &mut digest,
        b"stderr_limit",
        &u64::try_from(stderr_limit)
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    format!("{:x}", digest.finalize())
}

fn bytes_digest(label: &[u8], bytes: &[u8]) -> String {
    let mut digest = Sha256::new();
    hash_frame(&mut digest, label, bytes);
    format!("{:x}", digest.finalize())
}

fn hash_frame(digest: &mut Sha256, label: &[u8], value: &[u8]) {
    digest.update((label.len() as u64).to_le_bytes());
    digest.update(label);
    digest.update((value.len() as u64).to_le_bytes());
    digest.update(value);
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_lower_oid(value: &str) -> bool {
    valid_oid(value)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_manifest_binding(manifest: &HandoffManifestV1) -> bool {
    if manifest.schema_version != HANDOFF_SCHEMA_VERSION
        || manifest.worktree.schema_version != HANDOFF_SCHEMA_VERSION
    {
        return false;
    }
    HandoffManifestV1::new(
        manifest.generation,
        manifest.created_at_ms,
        manifest.lineage.clone(),
        manifest.worktree.clone(),
        manifest.tests.clone(),
    )
    .is_ok_and(|expected| expected.manifest_id == manifest.manifest_id)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

fn process_error(error: ProcessError) -> VerificationError {
    match error {
        ProcessError::Timeout => VerificationError::Timeout,
        ProcessError::Unavailable => VerificationError::Unavailable,
        #[cfg(not(any(unix, windows)))]
        ProcessError::UnsupportedPlatform => VerificationError::UnsupportedPlatform,
    }
}

fn sanitize_git_environment(command: &mut Command) {
    for name in crate::INHERITED_GIT_VARS {
        command.env_remove(name);
    }
    for name in [
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_CONFIG",
        "GIT_CONFIG_PARAMETERS",
        "GIT_EXEC_PATH",
        "GIT_TEMPLATE_DIR",
    ] {
        command.env_remove(name);
    }
    command.env("GIT_CONFIG_COUNT", "0");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command.env("GIT_CONFIG_SYSTEM", EMPTY_GRAFT_FILE);
    command.env("GIT_CONFIG_GLOBAL", EMPTY_GRAFT_FILE);
    command.env("GIT_ATTR_NOSYSTEM", "1");
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    command.env("GIT_GRAFT_FILE", EMPTY_GRAFT_FILE);
    command.env("GIT_ALLOW_PROTOCOL", "file");
}

fn store_error(error: WorkflowStoreError) -> EvidenceFailure {
    match error {
        WorkflowStoreError::Corrupt => EvidenceFailure::Corrupt,
        _ => EvidenceFailure::Unavailable,
    }
}

fn private_root(root: &Path) -> Result<PathBuf, VerificationError> {
    let metadata = fs::symlink_metadata(root).map_err(|_| VerificationError::UnsafeRoot)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(VerificationError::UnsafeRoot);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(VerificationError::UnsafeRoot);
        }
    }
    // The same question asked of the DACL: a root any other account can
    // reach is refused, and one whose DACL cannot be read is refused too.
    #[cfg(windows)]
    if windows_root::is_private(root) != Some(true) {
        return Err(VerificationError::UnsafeRoot);
    }
    root.canonicalize()
        .map_err(|_| VerificationError::UnsafeRoot)
}

fn remove_disposable(root: &Path, worktree: &Path) -> Result<(), VerificationError> {
    if worktree.parent() != Some(root) {
        return Err(VerificationError::WorktreeEscaped);
    }
    match fs::symlink_metadata(worktree) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            fs::remove_file(worktree).map_err(|_| VerificationError::Cleanup)?;
        }
        Ok(_) => fs::remove_dir_all(worktree).map_err(|_| VerificationError::Cleanup)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(VerificationError::Cleanup),
    }
    if fs::symlink_metadata(worktree)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        Ok(())
    } else {
        Err(VerificationError::Cleanup)
    }
}

fn valid_program(program: &OsStr) -> bool {
    let path = Path::new(program);
    path.is_absolute()
        && !path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
}

fn resolve_host_executable(program: OsString) -> Option<PathBuf> {
    let program = PathBuf::from(program);
    let candidate = if program.is_absolute() {
        program
    } else {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .map(|directory| directory.join(&program))
            .find(|candidate| candidate.is_file())?
    };
    candidate.canonicalize().ok().filter(|path| path.is_file())
}

fn diff_words(staged: bool) -> Vec<OsString> {
    let mut words = vec![OsString::from("diff")];
    if staged {
        words.push(OsString::from("--cached"));
    }
    words.extend([
        OsString::from("--binary"),
        OsString::from("--full-index"),
        OsString::from("--no-ext-diff"),
        OsString::from("--no-textconv"),
        OsString::from("--no-color"),
        OsString::from("--ignore-submodules=none"),
        OsString::from("--"),
    ]);
    words
}

fn selected_filter(output: &[u8]) -> Result<bool, VerificationError> {
    let mut fields: Vec<&[u8]> = output.split(|byte| *byte == 0).collect();
    if fields.last() == Some(&&[][..]) {
        fields.pop();
    }
    if !fields.len().is_multiple_of(3) {
        return Err(VerificationError::InvalidSource);
    }
    Ok(fields
        .chunks_exact(3)
        .any(|triple| triple[2] != b"unspecified" && triple[2] != b"unset"))
}

#[cfg(unix)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt as _;

    value.as_bytes().to_vec()
}

#[cfg(windows)]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt as _;

    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}

#[cfg(not(any(unix, windows)))]
fn os_bytes(value: &OsStr) -> Vec<u8> {
    value.to_string_lossy().into_owned().into_bytes()
}

/// One entry of a directory's DACL, as the root privacy rule reads it.
#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum AccessEntry {
    /// Grants `mask` to the SID spelled `trustee` (`S-1-5-11`, …).
    Allows { trustee: String, mask: u32 },
    /// Denies something; a deny only narrows access.
    Denies,
    /// A type this rule does not read (object, callback, conditional): it
    /// may grant anyone anything.
    Unread,
}

/// The machine's own principals a verification root may grant besides its
/// owner and this process's account: SYSTEM and Administrators (root's
/// place on Windows), OWNER RIGHTS and CREATOR OWNER (which resolve to the
/// owner and the creator).
#[cfg(any(windows, test))]
const TRUSTED_MACHINE_SIDS: [&str; 4] = ["S-1-5-18", "S-1-5-32-544", "S-1-3-4", "S-1-3-0"];

/// The owners a verification root may have: the account running this process,
/// SYSTEM, and Administrators (an elevated creation's owner).
#[cfg(any(windows, test))]
const TRUSTED_OWNER_SIDS: [&str; 2] = ["S-1-5-18", "S-1-5-32-544"];

/// Windows' `mode & 0o077 == 0`: whether a directory's DACL reaches nobody
/// but its `owner`, the account running this process (`user`) and
/// [`TRUSTED_MACHINE_SIDS`]. Any access at all counts, read included, as a
/// group or other bit does on unix, and inherit-only entries count too —
/// they are what the disposable clone created inside will carry. No DACL
/// (`None`) grants everyone everything.
///
/// The owner itself must be this account, SYSTEM or Administrators: an owner
/// keeps WRITE_DAC whatever its DACL says, so a root another local user
/// pre-created — granting us Full to lure us in — stays theirs to rewrite.
#[cfg(any(windows, test))]
fn dacl_is_private(dacl: Option<&[AccessEntry]>, owner: &str, user: &str) -> bool {
    if owner != user && !TRUSTED_OWNER_SIDS.contains(&owner) {
        return false;
    }
    let Some(entries) = dacl else {
        return false;
    };
    entries.iter().all(|entry| match entry {
        AccessEntry::Denies => true,
        AccessEntry::Unread => false,
        AccessEntry::Allows { trustee, mask } => {
            *mask == 0
                || trustee == owner
                || trustee == user
                || TRUSTED_MACHINE_SIDS.contains(&trustee.as_str())
        }
    })
}

#[cfg(windows)]
mod windows_root {
    //! A directory's owner and DACL, and this process's account, read as
    //! SID strings for [`super::dacl_is_private`].

    use super::AccessEntry;
    use std::os::windows::ffi::OsStrExt as _;
    use std::path::Path;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GetAce,
        GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY,
        TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    /// Whether `root` is private by [`super::dacl_is_private`]; `None` when
    /// its security descriptor or this process's account cannot be read,
    /// which the caller refuses like an open root.
    pub(super) fn is_private(root: &Path) -> Option<bool> {
        let user = current_user()?;
        let wide: Vec<u16> = root.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut owner: PSID = null_mut();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
        // SAFETY: `wide` is NUL-terminated and outlives the call; the four
        // out-pointers are live locals; the descriptor is freed below.
        let status = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let owner = sid_string(owner);
        let entries = if dacl.is_null() {
            Some(None)
        } else {
            entries_of(dacl).map(Some)
        };
        // SAFETY: the descriptor GetNamedSecurityInfoW allocated; `owner`
        // and `dacl` point into it and are not used past this line.
        unsafe {
            LocalFree(descriptor);
        }
        Some(super::dacl_is_private(entries?.as_deref(), &owner?, &user))
    }

    fn entries_of(dacl: *mut ACL) -> Option<Vec<AccessEntry>> {
        // SAFETY: a DACL GetNamedSecurityInfoW returned, alive with its
        // descriptor for the whole walk.
        let count = unsafe { (*dacl).AceCount };
        let mut entries = Vec::with_capacity(usize::from(count));
        for index in 0..u32::from(count) {
            let mut ace: *mut core::ffi::c_void = null_mut();
            // SAFETY: `index` is below the ACL's own count.
            if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
                return None;
            }
            // SAFETY: every ACE begins with an ACE_HEADER.
            let header = unsafe { *ace.cast::<ACE_HEADER>() };
            let entry = match u32::from(header.AceType) {
                ACCESS_ALLOWED_ACE_TYPE => {
                    let allowed = ace.cast::<ACCESS_ALLOWED_ACE>();
                    // SAFETY: an ACCESS_ALLOWED ACE: header, mask, then its
                    // SID starting at `SidStart`.
                    let mask = unsafe { (*allowed).Mask };
                    // SAFETY: as above; the SID runs on from `SidStart`.
                    let sid = unsafe { std::ptr::addr_of_mut!((*allowed).SidStart) };
                    AccessEntry::Allows {
                        trustee: sid_string(sid.cast())?,
                        mask,
                    }
                }
                ACCESS_DENIED_ACE_TYPE => AccessEntry::Denies,
                _ => AccessEntry::Unread,
            };
            entries.push(entry);
        }
        Some(entries)
    }

    fn sid_string(sid: PSID) -> Option<String> {
        if sid.is_null() {
            return None;
        }
        let mut text: windows_sys::core::PWSTR = null_mut();
        // SAFETY: a valid SID in; the string it allocates is freed below.
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
            return None;
        }
        let mut length = 0;
        // SAFETY: ConvertSidToStringSidW returns a NUL-terminated string.
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        // SAFETY: `length` UTF-16 units precede the NUL just found.
        let spelled = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, length) });
        // SAFETY: freeing the string ConvertSidToStringSidW allocated.
        unsafe {
            LocalFree(text.cast());
        }
        Some(spelled)
    }

    /// The SID of the account this process runs as.
    fn current_user() -> Option<String> {
        let mut token: HANDLE = null_mut();
        // SAFETY: querying our own process token; closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return None;
        }
        let mut needed = 0_u32;
        // SAFETY: a size query — no buffer, the length comes back in `needed`.
        unsafe {
            GetTokenInformation(token, TokenUser, null_mut(), 0, &mut needed);
        }
        // Whole u64 words, so the TOKEN_USER at the front is aligned.
        let words = usize::try_from(needed).ok()?.div_ceil(8).max(1);
        let mut buffer = vec![0_u64; words];
        // SAFETY: the buffer holds at least `needed` bytes.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        };
        // SAFETY: closing the token we opened.
        unsafe {
            CloseHandle(token);
        }
        if ok == 0 {
            return None;
        }
        // SAFETY: GetTokenInformation(TokenUser) filled a TOKEN_USER, whose
        // SID points into the same buffer, alive for the conversion.
        let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
        sid_string(user.User.Sid)
    }
}

#[cfg(test)]
mod root_privacy_tests {
    //! Windows' `mode & 0o077 != 0`, pinned on every platform: the decision
    //! is pure, and only the DACL reader behind it is Windows-only.

    use super::{AccessEntry, dacl_is_private};

    const ME: &str = "S-1-5-21-1111-2222-3333-1001";
    const SYSTEM: &str = "S-1-5-18";
    const ADMINISTRATORS: &str = "S-1-5-32-544";
    const USERS: &str = "S-1-5-32-545";
    const AUTHENTICATED_USERS: &str = "S-1-5-11";
    const EVERYONE: &str = "S-1-1-0";
    const FULL: u32 = 0x001F_01FF;
    const MODIFY: u32 = 0x0013_01BF;
    const READ: u32 = 0x0012_00A9;

    fn allows(trustee: &str, mask: u32) -> AccessEntry {
        AccessEntry::Allows {
            trustee: trustee.to_string(),
            mask,
        }
    }

    /// A folder made straight under `C:\` inherits Authenticated Users
    /// Modify; another local user could then edit the disposable clone
    /// between its content check and its run and earn a trusted receipt.
    /// Unix refuses the same root (`UnsafeRoot`); Windows took it.
    #[test]
    fn a_root_that_other_accounts_can_reach_is_refused_on_windows_too() {
        let profile = vec![
            allows(SYSTEM, FULL),
            allows(ADMINISTRATORS, FULL),
            allows(ME, FULL),
        ];
        assert!(dacl_is_private(Some(&profile), ME, ME));

        let mut under_c = profile.clone();
        under_c.push(allows(AUTHENTICATED_USERS, MODIFY));
        under_c.push(allows(USERS, READ));
        assert!(!dacl_is_private(Some(&under_c), ME, ME));

        // Read alone is reach, as a group or other read bit is on unix.
        let readable = [allows(ME, FULL), allows(EVERYONE, READ)];
        assert!(!dacl_is_private(Some(&readable), ME, ME));

        // No DACL at all grants everyone everything.
        assert!(!dacl_is_private(None, ME, ME));

        // A deny only narrows; an entry this rule cannot read might grant
        // anything.
        assert!(dacl_is_private(
            Some(&[allows(ME, FULL), AccessEntry::Denies]),
            ME,
            ME
        ));
        assert!(!dacl_is_private(
            Some(&[allows(ME, FULL), AccessEntry::Unread]),
            ME,
            ME
        ));

        // An elevated creation leaves Administrators as the owner; the
        // account running this process is still trusted.
        assert!(dacl_is_private(
            Some(&[allows(ME, FULL)]),
            ADMINISTRATORS,
            ME
        ));
        // Another account's root is never private to this one, whatever it
        // grants: an owner keeps WRITE_DAC over its directory, so a local
        // user who pre-created the path can rewrite the clone inside at will.
        // (Unix refuses the same by the mode: that user's 0700 directory is
        // unusable to us and 0707 is refused.)
        const ANOTHER: &str = "S-1-5-21-9-9-9-500";
        assert!(!dacl_is_private(
            Some(&[allows(ANOTHER, FULL)]),
            ANOTHER,
            ME
        ));
        assert!(!dacl_is_private(
            Some(&[allows(ME, FULL), allows(ANOTHER, FULL)]),
            ANOTHER,
            ME
        ));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _, symlink};
    use std::process::{Command, Output};
    use std::sync::{Arc, Barrier};

    use tempfile::TempDir;

    use super::*;
    use crate::handoff::HandoffLineage;

    struct Repository {
        root: TempDir,
        orchestrator: Orchestrator,
        manifest: HandoffManifestV1,
    }

    fn git_output(root: &Path, args: &[&str]) -> Output {
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

    fn repository(tests: Vec<TestReceipt>) -> Repository {
        let root = tempfile::tempdir().expect("repository");
        git(root.path(), &["init", "-qb", "main"]);
        git(root.path(), &["config", "user.name", "Evidence Test"]);
        git(
            root.path(),
            &["config", "user.email", "evidence@example.invalid"],
        );
        fs::write(root.path().join("tracked.txt"), "committed\n").expect("tracked file");
        git(root.path(), &["add", "tracked.txt"]);
        git(root.path(), &["commit", "-qm", "source"]);
        let orchestrator = Orchestrator::open(root.path()).expect("orchestrator");
        let snapshot = orchestrator
            .handoff_snapshot(root.path(), 1_000)
            .expect("snapshot");
        let tests = tests
            .into_iter()
            .map(|mut receipt| {
                receipt.snapshot_digest.clone_from(&snapshot.content_digest);
                receipt
            })
            .collect();
        let manifest = HandoffManifestV1::new(
            1,
            1_001,
            HandoffLineage {
                run_id: "run-1".to_string(),
                task_id: "task-1".to_string(),
                dispatch_id: "dispatch-1".to_string(),
                worker_id: "worker-1".to_string(),
                parent_manifest_id: None,
            },
            snapshot,
            tests,
        )
        .expect("manifest");
        Repository {
            root,
            orchestrator,
            manifest,
        }
    }

    /// A ceiling on a fixture script that must FINISH — never a measurement.
    /// What these tests assert is the exit, the cap, the receipt; the script
    /// itself prints a line or two kilobytes. Two seconds was not enough on
    /// the release lane's gate, where these run beside the whole workspace's
    /// tests, twice on 2026-09-17: `Timeout` on the noisy and the failing
    /// fixtures (load 6–9, the window restarting, `syspolicyd` scanning the
    /// freshly installed build) while the same tests passed in 1.3 s alone.
    const SCRIPT_PATIENCE: Duration = Duration::from_secs(10);

    /// A deadline a fixture must OVERRUN: the runner's timeout is what is
    /// under test, and the shell only has to get as far as writing a marker
    /// before it fires. The test waits this out once, so the margin costs
    /// seconds of wall clock, not a red lane — the same gate saw the shell
    /// not reach its marker inside two.
    const FAULT_DEADLINE: Duration = Duration::from_secs(5);

    fn private_dir(parent: &Path, name: &str) -> PathBuf {
        let path = parent.join(name);
        fs::create_dir(&path).expect("private directory");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).expect("private permissions");
        path
    }

    fn store(parent: &TempDir) -> (PathBuf, WorkflowStore) {
        let root = private_dir(parent.path(), "authority");
        let path = root.join("workflow.sqlite");
        let store = WorkflowStore::open(&path).expect("workflow store");
        (path, store)
    }

    fn runner(
        parent: &TempDir,
        commands: Vec<VerificationCommand>,
    ) -> (PathBuf, VerificationRunner) {
        let root = private_dir(parent.path(), "verification");
        let catalog = VerificationCatalog::new(commands).expect("catalog");
        let runner = VerificationRunner::open(&root, catalog).expect("runner");
        (root, runner)
    }

    fn exact_command(_source_oid: &str) -> VerificationCommand {
        VerificationCommand::new(
            "exact-source-v1",
            absolute_git(),
            [
                OsString::from("diff"),
                OsString::from("--quiet"),
                OsString::from("HEAD"),
                OsString::from("--"),
            ],
            Duration::from_secs(2),
        )
        .expect("exact command")
    }

    fn absolute_git() -> PathBuf {
        let path = std::env::var_os("PATH").expect("test PATH");
        std::env::split_paths(&path)
            .map(|directory| directory.join(crate::GIT_EXECUTABLE))
            .find(|candidate| candidate.is_file())
            .and_then(|candidate| candidate.canonicalize().ok())
            .expect("absolute Git executable")
    }

    fn requirement(command_id: &str) -> TestRequirement {
        TestRequirement {
            name: "rust".to_string(),
            command_id: command_id.to_string(),
            must_fail_first: false,
        }
    }

    fn executable(parent: &Path, name: &str, body: &str) -> PathBuf {
        let path = parent.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("executable");
        path
    }

    fn assert_empty(directory: &Path) {
        assert_eq!(
            fs::read_dir(directory).expect("verification root").count(),
            0,
            "disposable worktree leaked"
        );
    }

    #[test]
    fn exact_source_runs_in_a_disposable_tree_and_restart_resolves_it() {
        let repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (store_path, store) = store(&home);
        let commands = vec![exact_command(&repository.manifest.worktree.head_oid)];
        let (verification_root, runner) = runner(&home, commands);
        let hook_marker = home.path().join("repo-hook-ran");
        let hook = repository.root.path().join(".git/hooks/post-checkout");
        fs::write(
            &hook,
            format!("#!/bin/sh\ntouch '{}'\n", hook_marker.display()),
        )
        .expect("hostile repository hook");
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).expect("hook executable");
        fs::write(
            repository.root.path().join("tracked.txt"),
            "dirty worker bytes\n",
        )
        .expect("dirty original");
        let before_status = git(repository.root.path(), &["status", "--porcelain=v1"]);
        let before_head = git(repository.root.path(), &["rev-parse", "HEAD"]);
        let before_bytes = fs::read(repository.root.path().join("tracked.txt")).expect("before");
        let required = requirement("exact-source-v1");

        let evidence = runner
            .run(
                &repository.orchestrator,
                &store,
                &repository.manifest,
                &required,
            )
            .expect("verification");
        assert!(evidence.succeeded());
        assert_eq!(evidence.source_oid(), repository.manifest.worktree.head_oid);
        assert_eq!(
            git(repository.root.path(), &["rev-parse", "HEAD"]),
            before_head
        );
        assert_eq!(
            git(repository.root.path(), &["status", "--porcelain=v1"]),
            before_status
        );
        assert_eq!(
            fs::read(repository.root.path().join("tracked.txt")).expect("after"),
            before_bytes
        );
        assert_empty(&verification_root);
        assert!(
            !hook_marker.exists(),
            "repository hook executed outside allowlist"
        );
        let mut rebound = repository.manifest.clone();
        rebound.worktree.head_oid = "a".repeat(40);
        assert_eq!(
            runner.run(&repository.orchestrator, &store, &rebound, &required),
            Err(VerificationError::ManifestMismatch),
            "a cached receipt must not authenticate a rebound manifest"
        );

        drop(store);
        let reopened = WorkflowStore::open(store_path).expect("reopen store");
        let restarted_runner = VerificationRunner::open(
            &verification_root,
            VerificationCatalog::new([exact_command(&repository.manifest.worktree.head_oid)])
                .expect("restarted catalog"),
        )
        .expect("restarted runner");
        assert_eq!(
            restarted_runner
                .resolver(&reopened)
                .resolve(
                    &repository.manifest.manifest_id,
                    &required,
                    &repository.manifest.worktree.content_digest,
                )
                .expect("resolve after restart"),
            Some(evidence.receipt().clone())
        );
        let rebound_program = executable(home.path(), "rebound", "exit 99");
        let rebound_command = VerificationCommand::new(
            "exact-source-v1",
            rebound_program,
            Vec::<OsString>::new(),
            Duration::from_secs(2),
        )
        .expect("rebound command");
        let rebound_runner = VerificationRunner::open(
            &verification_root,
            VerificationCatalog::new([rebound_command]).expect("rebound catalog"),
        )
        .expect("rebound runner");
        assert_eq!(
            rebound_runner.run(
                &repository.orchestrator,
                &reopened,
                &repository.manifest,
                &required,
            ),
            Err(VerificationError::CommandBinding)
        );
        assert_eq!(
            rebound_runner.resolver(&reopened).resolve(
                &repository.manifest.manifest_id,
                &required,
                &repository.manifest.worktree.content_digest,
            ),
            Err(EvidenceFailure::CommandBinding)
        );
        let removed_runner = VerificationRunner::open(
            &verification_root,
            VerificationCatalog::new(Vec::<VerificationCommand>::new()).expect("empty catalog"),
        )
        .expect("removed-command runner");
        assert_eq!(
            removed_runner.resolver(&reopened).resolve(
                &repository.manifest.manifest_id,
                &required,
                &repository.manifest.worktree.content_digest,
            ),
            Err(EvidenceFailure::CommandBinding)
        );
        let mut rebound_limits = restarted_runner.clone();
        rebound_limits.stdout_limit -= 1;
        assert_eq!(
            rebound_limits.resolver(&reopened).resolve(
                &repository.manifest.manifest_id,
                &required,
                &repository.manifest.worktree.content_digest,
            ),
            Err(EvidenceFailure::CommandBinding)
        );
        let connection = reopened.connection().expect("corruptible connection");
        connection
            .execute(
                "UPDATE trusted_test_evidence SET evidence_digest = ?1",
                ["0".repeat(64)],
            )
            .expect("corrupt evidence digest");
        assert_eq!(
            restarted_runner.resolver(&reopened).resolve(
                &repository.manifest.manifest_id,
                &required,
                &repository.manifest.worktree.content_digest,
            ),
            Err(EvidenceFailure::Corrupt)
        );
    }

    #[test]
    fn repository_checkout_filters_never_execute_outside_the_host_allowlist() {
        let mut repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let marker = home.path().join("smudge-ran");
        let upload_marker = home.path().join("upload-hook-ran");
        let smudge = executable(
            home.path(),
            "repo-smudge",
            &format!("touch '{}'\nprintf 'smudged\\n'", marker.display()),
        );
        git(
            repository.root.path(),
            &["config", "filter.evil.clean", "cat"],
        );
        git(
            repository.root.path(),
            &[
                "config",
                "filter.evil.smudge",
                smudge.to_str().expect("utf8 test path"),
            ],
        );
        git(
            repository.root.path(),
            &["config", "filter.evil.required", "true"],
        );
        let upload_hook = executable(
            home.path(),
            "repo-upload-hook",
            &format!("touch '{}'\nexit 99", upload_marker.display()),
        );
        git(
            repository.root.path(),
            &[
                "config",
                "uploadpack.packObjectsHook",
                upload_hook.to_str().expect("utf8 test path"),
            ],
        );
        fs::write(
            repository.root.path().join(".gitattributes"),
            "tracked.txt filter=evil\n",
        )
        .expect("filter attributes");
        git(repository.root.path(), &["add", ".gitattributes"]);
        git(repository.root.path(), &["commit", "-qm", "filter fixture"]);
        let snapshot = repository
            .orchestrator
            .handoff_snapshot(repository.root.path(), 2_000)
            .expect("filtered source snapshot");
        repository.manifest = HandoffManifestV1::new(
            1,
            2_001,
            HandoffLineage {
                run_id: "run-1".to_string(),
                task_id: "task-1".to_string(),
                dispatch_id: "dispatch-1".to_string(),
                worker_id: "worker-1".to_string(),
                parent_manifest_id: None,
            },
            snapshot,
            Vec::new(),
        )
        .expect("filtered manifest");
        let command = exact_command(&repository.manifest.worktree.head_oid);
        let (verification_root, runner) = runner(&home, vec![command]);
        let required = requirement("exact-source-v1");

        assert_eq!(
            runner.run(
                &repository.orchestrator,
                &store,
                &repository.manifest,
                &required,
            ),
            Err(VerificationError::UnsupportedSource)
        );
        assert!(!marker.exists(), "repository smudge command executed");
        assert!(
            !upload_marker.exists(),
            "repository upload-pack hook executed"
        );
        assert_empty(&verification_root);
        assert_eq!(
            runner
                .resolver(&store)
                .resolve(
                    &repository.manifest.manifest_id,
                    &required,
                    &repository.manifest.worktree.content_digest,
                )
                .expect("missing filtered evidence"),
            None
        );
    }

    #[test]
    fn unverified_submodule_materialization_is_refused_and_cleaned() {
        let mut repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let gitlink = format!(
            "160000,{},vendor/sub",
            repository.manifest.worktree.head_oid
        );
        git(
            repository.root.path(),
            &["update-index", "--add", "--cacheinfo", &gitlink],
        );
        git(
            repository.root.path(),
            &["commit", "-qm", "gitlink fixture"],
        );
        let snapshot = repository
            .orchestrator
            .handoff_snapshot(repository.root.path(), 3_000)
            .expect("gitlink source snapshot");
        repository.manifest = HandoffManifestV1::new(
            1,
            3_001,
            HandoffLineage {
                run_id: "run-1".to_string(),
                task_id: "task-1".to_string(),
                dispatch_id: "dispatch-1".to_string(),
                worker_id: "worker-1".to_string(),
                parent_manifest_id: None,
            },
            snapshot,
            Vec::new(),
        )
        .expect("gitlink manifest");
        let command = exact_command(&repository.manifest.worktree.head_oid);
        let (verification_root, runner) = runner(&home, vec![command]);

        assert_eq!(
            runner.run(
                &repository.orchestrator,
                &store,
                &repository.manifest,
                &requirement("exact-source-v1"),
            ),
            Err(VerificationError::UnsupportedSource)
        );
        assert_empty(&verification_root);
    }

    #[test]
    fn failure_and_forged_manifest_success_resolve_to_trusted_failure() {
        let forged = TestReceipt::new("rust", "failure-v1", "placeholder", 1, 2, 0);
        let repository = repository(vec![forged]);
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let script = executable(home.path(), "fail", "echo refused >&2\nexit 7");
        let command = VerificationCommand::new(
            "failure-v1",
            script,
            Vec::<OsString>::new(),
            SCRIPT_PATIENCE,
        )
        .expect("failure command");
        let (_, runner) = runner(&home, vec![command]);
        let required = requirement("failure-v1");
        let evidence = runner
            .run(
                &repository.orchestrator,
                &store,
                &repository.manifest,
                &required,
            )
            .expect("failed evidence");
        assert!(!evidence.succeeded());
        assert_eq!(evidence.receipt().exit_code, 7);
        assert_eq!(repository.manifest.tests[0].exit_code, 0);
        assert_eq!(
            runner
                .resolver(&store)
                .resolve(
                    &repository.manifest.manifest_id,
                    &required,
                    &repository.manifest.worktree.content_digest,
                )
                .expect("trusted receipt")
                .expect("stored receipt")
                .exit_code,
            7
        );
    }

    #[test]
    fn output_is_capped_but_still_records_a_successful_exit() {
        let repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let stdout_script = executable(
            home.path(),
            "noisy-stdout",
            "i=0\nwhile [ $i -lt 200 ]; do printf 1234567890; i=$((i + 1)); done\nexit 0",
        );
        let stderr_script = executable(
            home.path(),
            "noisy-stderr",
            "i=0\nwhile [ $i -lt 200 ]; do printf 0987654321 >&2; i=$((i + 1)); done\nexit 0",
        );
        let stdout_command = VerificationCommand::new(
            "noisy-stdout-v1",
            stdout_script,
            Vec::<OsString>::new(),
            SCRIPT_PATIENCE,
        )
        .expect("noisy stdout command");
        let stderr_command = VerificationCommand::new(
            "noisy-stderr-v1",
            stderr_script,
            Vec::<OsString>::new(),
            SCRIPT_PATIENCE,
        )
        .expect("noisy stderr command");
        let (_, mut runner) = runner(&home, vec![stdout_command, stderr_command]);
        runner.stdout_limit = 32;
        runner.stderr_limit = 32;
        for command_id in ["noisy-stdout-v1", "noisy-stderr-v1"] {
            let evidence = runner
                .run(
                    &repository.orchestrator,
                    &store,
                    &repository.manifest,
                    &requirement(command_id),
                )
                .expect("noisy evidence");
            assert!(evidence.succeeded());
            assert!(evidence.receipt().output_truncated);
            assert!(valid_digest(evidence.evidence_digest()));
        }
    }

    #[test]
    fn timeout_kills_persistent_descendants_and_cleans_the_worktree() {
        let repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let marker = home.path().join("descendant.pid");
        let script = executable(
            home.path(),
            "timeout",
            &format!("sleep 30 &\necho $! > '{}'\nwait", marker.display()),
        );
        let command = VerificationCommand::new(
            "timeout-v1",
            script,
            Vec::<OsString>::new(),
            // The full package runs many real-Git tests in parallel. Give the
            // shell enough scheduling margin to publish its child pid before
            // testing the runner's deadline and process-group kill.
            FAULT_DEADLINE,
        )
        .expect("timeout command");
        let (verification_root, runner) = runner(&home, vec![command]);
        let required = requirement("timeout-v1");
        assert_eq!(
            runner.run(
                &repository.orchestrator,
                &store,
                &repository.manifest,
                &required,
            ),
            Err(VerificationError::Timeout)
        );
        assert_empty(&verification_root);
        let descendant: i32 = fs::read_to_string(marker)
            .expect("descendant pid")
            .trim()
            .parse()
            .expect("numeric pid");
        for _ in 0..50 {
            // SAFETY: signal 0 only checks the child process allocated above.
            let exists = unsafe { libc::kill(descendant, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
            if !exists {
                assert_eq!(
                    runner
                        .resolver(&store)
                        .resolve(
                            &repository.manifest.manifest_id,
                            &required,
                            &repository.manifest.worktree.content_digest,
                        )
                        .expect("missing timeout evidence"),
                    None
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("verification descendant survived timeout");
    }

    #[test]
    fn a_daemonized_pipe_holder_cannot_extend_the_deadline_or_leave_evidence() {
        let repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let python =
            resolve_host_executable(OsString::from("python3")).expect("absolute Python executable");
        let before_fds = open_fd_count();
        let mut attempt = 0;
        let mut proven = 0;
        while proven < 3 {
            attempt += 1;
            assert!(
                attempt <= 6,
                "the host never managed to start the command in six tries"
            );
            let marker = home.path().join(format!("daemon-{attempt}.pid"));
            let started_marker = home.path().join(format!("daemon-{attempt}.started"));
            let script = executable(
                home.path(),
                &format!("daemonize-{attempt}"),
                &format!(
                    "touch '{}'\n'{}' -c 'import os,time; os.fork() and os._exit(0); os.setsid(); open(\"{}\", \"w\").write(str(os.getpid())); time.sleep(30)' &\nwhile [ ! -s '{}' ]; do sleep 0.01; done\nexit 0",
                    started_marker.display(),
                    python.display(),
                    marker.display(),
                    marker.display(),
                ),
            );
            let command = VerificationCommand::new(
                "daemon-v1",
                script,
                Vec::<OsString>::new(),
                Duration::from_secs(2),
            )
            .expect("daemon command");
            let root_name = format!("verification-daemon-{attempt}");
            let verification_root = private_dir(home.path(), &root_name);
            let runner = VerificationRunner::open(
                &verification_root,
                VerificationCatalog::new([command]).expect("daemon catalog"),
            )
            .expect("daemon runner");
            let required = requirement("daemon-v1");
            // The command's budget starts after clone/content verification.
            // Observe its first line so unrelated Git preparation cannot be
            // mistaken for an escaped writer extending that budget.
            let (result, command_elapsed) = std::thread::scope(|scope| {
                let (finished, pending) = std::sync::mpsc::channel::<()>();
                let started_path = &started_marker;
                let observer = scope.spawn(move || {
                    while matches!(
                        pending.recv_timeout(Duration::from_millis(10)),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    ) {
                        if started_path.exists() {
                            return Some(std::time::Instant::now());
                        }
                    }
                    None
                });
                let result = runner.run(
                    &repository.orchestrator,
                    &store,
                    &repository.manifest,
                    &required,
                );
                let ended = std::time::Instant::now();
                drop(finished);
                let elapsed = observer
                    .join()
                    .expect("command observer")
                    .map(|started| ended.saturating_duration_since(started));
                (result, elapsed)
            });
            // Reap the deliberately escaped process before any assertion can
            // unwind. A failing test must not leave a pipe holder behind.
            let escaped = fs::read_to_string(&marker)
                .ok()
                .and_then(|pid| pid.trim().parse::<i32>().ok());
            if let Some(escaped) = escaped {
                // SAFETY: the pid was written by this attempt's own child.
                unsafe { libc::kill(escaped, libc::SIGKILL) };
                wait_until_gone(escaped);
            }
            assert_eq!(result, Err(VerificationError::Timeout));
            assert_empty(&verification_root);
            let Some(command_elapsed) = command_elapsed else {
                /* A DID-NOT-RUN, not a red. Under load the two-second
                 * command deadline can fall before the shell has executed
                 * its first line, and an attempt whose command never
                 * started proves nothing about pipe holders either way —
                 * the timeout and clean root above were still exacted.
                 * Witnessed 2026-08 at one-in-eight on a machine running
                 * three review batteries; the assert that stood here read
                 * that spawn latency as a defect. Three attempts must
                 * PROVE; a miss is asked again, and six misses is its own
                 * honest failure. */
                continue;
            };
            assert!(
                command_elapsed < Duration::from_secs(5),
                "escaped pipe holder extended the deadline: {command_elapsed:?}"
            );
            assert!(
                escaped.is_some(),
                "the command never created its pipe holder"
            );
            assert_eq!(
                runner
                    .resolver(&store)
                    .resolve(
                        &repository.manifest.manifest_id,
                        &required,
                        &repository.manifest.worktree.content_digest,
                    )
                    .expect("missing escaped evidence"),
                None
            );
            proven += 1;
        }
        assert!(
            open_fd_count() <= before_fds + 2,
            "escaped writers leaked process pipe descriptors"
        );
    }

    #[test]
    fn a_timeout_after_clone_creation_removes_the_disposable_checkout() {
        let repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let marker = home.path().join("checkout-entered");
        // Inject a successful clone allocation, then stall materialization.
        // Real clone/source correctness has its own tests. Putting real clone
        // latency under this short fault deadline can time out before the
        // cleanup path this test is supposed to exercise is ever reached.
        let wrapper = executable(
            home.path(),
            "git-wrapper",
            &format!(
                "clone=false\nfor arg in \"$@\"; do\n  if [ \"$arg\" = clone ]; then clone=true; fi\n  if [ \"$arg\" = checkout ]; then\n    touch '{}'\n    sleep 30\n  fi\n  destination=$arg\ndone\nif [ \"$clone\" = true ]; then mkdir -- \"$destination\"; exit $?; fi\nexec git \"$@\"",
                marker.display()
            ),
        );
        let verification_root = private_dir(home.path(), "verification");
        let catalog =
            VerificationCatalog::new([exact_command(&repository.manifest.worktree.head_oid)])
                .expect("catalog");
        let mut runner = VerificationRunner::open_with_git(&verification_root, catalog, wrapper)
            .expect("wrapper runner");
        runner.git_timeout = FAULT_DEADLINE;
        let required = requirement("exact-source-v1");

        assert_eq!(
            runner.run(
                &repository.orchestrator,
                &store,
                &repository.manifest,
                &required,
            ),
            Err(VerificationError::Timeout)
        );
        assert!(marker.exists(), "fault did not occur after clone creation");
        assert_empty(&verification_root);
        assert_eq!(
            runner
                .resolver(&store)
                .resolve(
                    &repository.manifest.manifest_id,
                    &required,
                    &repository.manifest.worktree.content_digest,
                )
                .expect("missing timeout evidence"),
            None
        );
    }

    #[test]
    fn concurrent_duplicate_runs_return_one_immutable_row() {
        let repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (_, store) = store(&home);
        let command = exact_command(&repository.manifest.worktree.head_oid);
        let (_, runner) = runner(&home, vec![command]);
        let runner = Arc::new(runner);
        let store = Arc::new(store);
        let orchestrator = Arc::new(repository.orchestrator.clone());
        let manifest = Arc::new(repository.manifest.clone());
        let required = Arc::new(requirement("exact-source-v1"));
        let barrier = Arc::new(Barrier::new(3));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let runner = Arc::clone(&runner);
                let store = Arc::clone(&store);
                let orchestrator = Arc::clone(&orchestrator);
                let manifest = Arc::clone(&manifest);
                let required = Arc::clone(&required);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    runner.run(&orchestrator, &store, &manifest, &required)
                })
            })
            .collect();
        barrier.wait();
        let evidence: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().expect("runner thread").expect("evidence"))
            .collect();
        assert_eq!(evidence[0], evidence[1]);
        let connection = store.connection().expect("evidence connection");
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM trusted_test_evidence", [], |row| {
                row.get(0)
            })
            .expect("evidence count");
        assert_eq!(count, 1);
    }

    #[test]
    fn missing_and_unavailable_evidence_are_distinct() {
        let repository = repository(Vec::new());
        let home = tempfile::tempdir().expect("authority home");
        let (store_path, store) = store(&home);
        let command = exact_command(&repository.manifest.worktree.head_oid);
        let (_, runner) = runner(&home, vec![command]);
        let required = requirement("exact-source-v1");
        assert_eq!(
            runner
                .resolver(&store)
                .resolve(
                    &repository.manifest.manifest_id,
                    &required,
                    &repository.manifest.worktree.content_digest,
                )
                .expect("missing is readable"),
            None
        );
        fs::remove_file(&store_path).expect("remove database");
        assert_eq!(
            runner.resolver(&store).resolve(
                &repository.manifest.manifest_id,
                &required,
                &repository.manifest.worktree.content_digest,
            ),
            Err(EvidenceFailure::Unavailable)
        );
    }

    #[test]
    fn symlinked_roots_and_relative_program_escapes_are_refused() {
        let home = tempfile::tempdir().expect("runner home");
        let real = private_dir(home.path(), "real");
        let linked = home.path().join("linked");
        symlink(&real, &linked).expect("root symlink");
        let catalog = VerificationCatalog::new(Vec::<VerificationCommand>::new()).expect("catalog");
        assert!(matches!(
            VerificationRunner::open(&linked, catalog),
            Err(VerificationError::UnsafeRoot)
        ));
        assert!(matches!(
            VerificationCommand::new(
                "escape-v1",
                "../outside",
                Vec::<OsString>::new(),
                Duration::from_secs(1),
            ),
            Err(VerificationConfigError::InvalidCommand)
        ));
        assert!(matches!(
            VerificationCommand::new(
                "path-search-v1",
                "cargo",
                Vec::<OsString>::new(),
                Duration::from_secs(1),
            ),
            Err(VerificationConfigError::InvalidCommand)
        ));
        let private_program = home.path().join("private-program");
        let private_argument = home.path().join("private-argument");
        let command = VerificationCommand::new(
            "redacted-v1",
            &private_program,
            [private_argument.as_os_str()],
            Duration::from_secs(1),
        )
        .expect("redacted command");
        let rendered = format!("{command:?}");
        assert!(!rendered.contains(&private_program.to_string_lossy().into_owned()));
        assert!(!rendered.contains(&private_argument.to_string_lossy().into_owned()));
        assert_eq!(fs::metadata(real).expect("real root").mode() & 0o777, 0o700);
    }

    fn open_fd_count() -> usize {
        fs::read_dir("/dev/fd")
            .expect("open descriptor directory")
            .count()
    }

    fn wait_until_gone(process: i32) {
        for _ in 0..100 {
            // SAFETY: signal 0 only checks the process created by this test.
            let exists = unsafe { libc::kill(process, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
            if !exists {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("escaped verification process survived test cleanup");
    }
}
