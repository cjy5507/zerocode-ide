//! What one checkout can actually prove about itself, read in one place.
//!
//! A person looking at a worktree asks four questions at once: what changed
//! here, who ran in it, was any of that tested, and did anybody decide
//! anything. Each answer already exists in this window — Git holds the
//! changes, the orchestration ledger holds the dispatches and the
//! coordinator's own words, and the authority store holds review decisions
//! and host-trusted test receipts. Nothing here creates a new record; this
//! module is the one place those three are read TOGETHER and labelled, so a
//! reader is never left to assume that an old green test still describes the
//! bytes on disk.
//!
//! Three rules give this module its shape.
//!
//! **Nothing is executed.** [`crate::test_evidence::VerificationRunner`] is
//! the road that runs a test and mints a receipt; this road only reads what
//! that one already wrote. A query that ran tests would be a query nobody
//! could afford to open.
//!
//! **Absence is never success.** `empty`, `unsupported`, `missing` and
//! `error` are four different answers and stay four ([`SourceState`]). What
//! no road in this window records at all — tool approvals, CI checks — is
//! named in every answer's `summary.unrecorded`, so its absence is never read
//! as an approval.
//!
//! **A receipt is judged against the checkout as it is NOW.** The head alone
//! is not enough: two checkouts on the same commit with different uncommitted
//! bytes are different work, so currency compares the content digest and
//! refuses to claim `current` at all while the observation has coverage gaps
//! ([`Currency`]).
//!
//! The caller resolves WHICH checkout before asking. A path never arrives
//! here as an authority: the window resolves the person's click against its
//! own catalog and the CLI resolves the caller's seated checkout out of the
//! registered runtime state, and both hand the same [`EvidenceTarget`] in.

use std::path::{Path, PathBuf};

use serde::Serialize;
use zerocode_core::orchestration::{Ledger, ReviewFacts};

use crate::Orchestrator;
use crate::handoff::{CoverageGap, GitOperation, HandoffError, WorktreeSnapshot};
use crate::workflow_store::{
    ReadOnlyWorkflows, StoredReview, TrustedReceiptRow, WorkflowStoreError,
};

/// The external shape of one evidence read. Increment before changing meaning.
pub const EVIDENCE_SCHEMA_VERSION: u16 = 1;

/// Changed paths one answer carries. The snapshot's own reader already bounds
/// itself far higher; this is the bound for what crosses to a reader, and a
/// count above it is reported rather than hidden.
pub const MAX_CHANGED_PATHS: usize = 200;
/// Coverage gaps carried beside the digest.
pub const MAX_COVERAGE_GAPS: usize = 16;
/// Dispatches, newest first.
pub const MAX_EXECUTIONS: usize = 60;
/// Coordinator report rows, newest first.
pub const MAX_REPORTS: usize = 60;
/// Receipts of either kind, newest first.
pub const MAX_RECEIPTS: usize = 60;
/// Review and gate rows, newest first.
pub const MAX_DECISIONS: usize = 60;
/// Workflow rows read for one worktree, newest generation first.
pub const MAX_WORKFLOWS: usize = 16;
/// How much display prose one row carries. A question or a resolution is a
/// person's own sentence and can be a page long.
pub const MAX_TEXT_CHARS: usize = 240;
/// A stored manifest larger than this is counted and not parsed: its receipts
/// are reported as unread rather than read at an unbounded cost.
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/* ---- what a source has to say about itself ---------------------------- */

/// Why a section holds what it holds. Four answers, kept apart on purpose: a
/// reader that cannot tell "nothing happened" from "nobody looked" will read
/// silence as approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// Read, and it had rows.
    Ok,
    /// Read, and there were none. A fact about this checkout.
    Empty,
    /// This window does not hold the source — no orchestration runtime, no
    /// authority store, no repository at this path. A fact about the window.
    Missing,
    /// The question cannot be asked of this checkout by this read — a tree on
    /// another host, a tree Git cannot identify, a service this read never
    /// calls. A fact about the product, not about the checkout.
    Unsupported,
    /// It was asked and refused. [`Source::error`] says why.
    Error,
}

/// Whether the answer was observed at this read, or not taken at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    /// Read during this query, at the query's own observation time.
    Observed,
    /// Nothing was read, so nothing is fresh.
    Unobserved,
}

/// How much of what exists is in the answer. `truncated` is never hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceCoverage {
    /// Rows the source had, before the bound.
    pub counted: usize,
    /// Rows this answer carries.
    pub returned: usize,
    /// Whether the bound took any.
    pub truncated: bool,
}

impl SourceCoverage {
    fn of(counted: usize, returned: usize) -> Self {
        Self {
            counted,
            returned,
            truncated: returned < counted,
        }
    }
}

/// A refusal in the caller's terms. Never a raw Git or SQLite sentence: those
/// carry local paths and store internals, and this crosses a command boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceError {
    /// A stable tag a caller can branch on.
    pub code: &'static str,
    /// One sentence for a person, with nothing local in it.
    pub message: &'static str,
    /// Whether asking again could answer differently.
    pub retryable: bool,
}

/// One section of the answer: its state, when it was read, what it covers,
/// why it refused, and the rows themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Source<T> {
    pub state: SourceState,
    pub freshness: Freshness,
    pub coverage: SourceCoverage,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<EvidenceError>,
    pub data: T,
}

impl<T: Default> Source<T> {
    fn unread(state: SourceState, error: Option<EvidenceError>) -> Self {
        Self {
            state,
            freshness: Freshness::Unobserved,
            coverage: SourceCoverage::default(),
            error,
            data: T::default(),
        }
    }

    fn missing() -> Self {
        Self::unread(SourceState::Missing, None)
    }

    fn unsupported() -> Self {
        Self::unread(SourceState::Unsupported, None)
    }

    fn failed(error: EvidenceError) -> Self {
        Self::unread(SourceState::Error, Some(error))
    }

    fn observed(data: T, coverage: SourceCoverage) -> Self {
        Self {
            state: match coverage.counted {
                0 => SourceState::Empty,
                _ => SourceState::Ok,
            },
            freshness: Freshness::Observed,
            coverage,
            error: None,
            data,
        }
    }
}

/* ---- the checkout being asked about ----------------------------------- */

/// Where the checkout is. Nothing here reaches across a network: a checkout
/// on another host is named and declared unsupported rather than mistaken for
/// a directory on this disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceLocation {
    /// A directory on this machine, already resolved by the caller.
    Local(PathBuf),
    /// A checkout that lives on another host.
    Remote,
}

/// One checkout, resolved by its caller before anything is read.
///
/// The path inside is LOCAL ROUTING: it reaches Git and stops there. What
/// crosses to a reader is [`Self::workspace_id`], an opaque digest, plus the
/// Git-proved repository and worktree ids when Git could prove them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceTarget {
    location: EvidenceLocation,
    workspace_id: String,
    /// The same path with a trailing separator shed, for ledger matching.
    ledger_key: Option<String>,
    /// The canonical spelling, when the filesystem had one, for the same.
    canonical_key: Option<String>,
}

impl EvidenceTarget {
    /// A checkout on this disk. The caller has already proved it is one this
    /// window may read.
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let canonical = std::fs::canonicalize(&path).ok();
        Self {
            workspace_id: zerocode_core::git_dir::opaque_path_id(
                "workspace",
                canonical.as_deref().unwrap_or(&path),
            ),
            ledger_key: Some(shed_separator(&path.to_string_lossy())),
            canonical_key: canonical
                .as_deref()
                .map(|held| shed_separator(&held.to_string_lossy())),
            location: EvidenceLocation::Local(path),
        }
    }

    /// A checkout on another host, named by the key the window's catalog
    /// holds for it. Nothing about it is read from this disk.
    #[must_use]
    pub fn remote(key: &str) -> Self {
        Self {
            location: EvidenceLocation::Remote,
            workspace_id: zerocode_core::git_dir::opaque_path_id("workspace", Path::new(key)),
            ledger_key: None,
            canonical_key: None,
        }
    }

    #[must_use]
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self.location, EvidenceLocation::Local(_))
    }

    fn path(&self) -> Option<&Path> {
        match &self.location {
            EvidenceLocation::Local(path) => Some(path),
            EvidenceLocation::Remote => None,
        }
    }

    /// Whether a ledger row's own word for its checkout names THIS one.
    ///
    /// Compared as strings against both spellings held here rather than by
    /// canonicalizing each row: a run holds dozens of workers and a syscall
    /// per row would put the filesystem in the middle of a read.
    fn holds(&self, checkout: Option<&str>) -> bool {
        let Some(said) = checkout.map(shed_separator) else {
            return false;
        };
        self.ledger_key.as_deref() == Some(said.as_str())
            || self.canonical_key.as_deref() == Some(said.as_str())
    }
}

fn shed_separator(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.is_empty() {
        true => path.to_string(),
        false => trimmed.to_string(),
    }
}

/* ---- the answer -------------------------------------------------------- */

/// What was observed about one checkout, in one schema, for both callers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeEvidenceV1 {
    pub schema_version: u16,
    pub observed_at_ms: i64,
    /// Opaque and machine-local. Never a path.
    pub workspace_id: String,
    /// Present when Git proved it at this read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_id: Option<String>,
    /// A display fact, never an identity key: two checkouts can wear one
    /// branch name and they are not the same work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub summary: EvidenceSummary,
    pub snapshot: Source<Option<SnapshotFacts>>,
    pub executions: Source<Vec<ExecutionRow>>,
    pub reports: Source<Vec<ReportRow>>,
    pub verification: Source<Vec<ReceiptRow>>,
    pub decisions: Source<Vec<DecisionRow>>,
    /// Always unsupported in this local read: asking GitHub costs a network
    /// call per open, and no road here makes one.
    pub ci: Source<Vec<String>>,
}

/// The first thing a reader sees, counted out of the sections below it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceSummary {
    /// Whether the checkout has uncommitted or ignored bytes right now.
    /// `None` where Git was not read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    pub changed_paths: usize,
    pub executions: usize,
    pub open_executions: usize,
    pub receipts_current: usize,
    pub receipts_stale: usize,
    pub receipts_unknown: usize,
    /// Receipts that are current AND passed. The only count that may be read
    /// as "this checkout is tested", and it is zero unless both are true.
    pub receipts_current_passing: usize,
    pub coordinator_reports: usize,
    pub decisions: usize,
    /// What nothing in this window records, named so a reader does not take
    /// its absence for a negative answer.
    pub unrecorded: Vec<&'static str>,
}

/// Git's word about the checkout, bounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotFacts {
    pub head_oid: String,
    pub content_digest: String,
    pub detached: bool,
    pub locked: bool,
    pub dirty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<u64>,
    /// Whether the digest covers every byte in the checkout. A `false` here
    /// is why no receipt may be called current.
    pub complete: bool,
    pub coverage_gaps: Vec<String>,
    pub changes: Vec<ChangedPath>,
}

/// One changed path, with the two Git columns kept apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedPath {
    /// Repository-relative, as Git wrote it.
    pub path: String,
    pub code: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
}

/// One dispatch that ran in THIS checkout, as the ledger holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionRow {
    pub run_id: String,
    pub dispatch_id: String,
    pub worker_id: String,
    pub agent: String,
    pub task_id: String,
    pub task: String,
    pub started_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<i64>,
    pub open: bool,
    /// The worker's own claim, and only that. `None` while it is open.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_reported_success: Option<bool>,
    /// The attempt this one replaces, kept so a retry chain stays readable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// What a COORDINATOR wrote about a task carried out here.
///
/// Its `kind` is the label, and it is not a test result: `verified: true` in
/// a task result is a person or an agent saying they looked, which is a
/// different fact from a command that exited zero and left a receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRow {
    pub kind: &'static str,
    pub run_id: String,
    pub task_id: String,
    pub task: String,
    pub verified: bool,
    pub merged: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_head: Option<String>,
    pub deployed: bool,
    /// Whether the coordinator wrote any of those keys at all. `false` is
    /// "nobody has said", which is not "no".
    pub written: bool,
}

/// Whether a receipt still describes the checkout as it is now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    /// The digest it was taken against is the digest observed now, and the
    /// observation covered everything.
    Current,
    /// The checkout has moved since.
    Stale,
    /// Nothing was observed to compare against, or the observation had gaps.
    Unknown,
}

/// Where a receipt came from. The two are not interchangeable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptSource {
    /// Minted by the host-trusted runner and stored in its own table.
    HostTrusted,
    /// Carried inside a stored handoff manifest.
    Manifest,
}

/// One test receipt, judged against this observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceiptRow {
    pub source: ReceiptSource,
    pub name: String,
    /// A configured, non-secret id. Never an argv.
    pub command_id: String,
    pub exit_code: i32,
    pub passed: bool,
    pub currency: Currency,
    /// Why the currency is what it is, in one tag.
    pub currency_reason: &'static str,
    pub started_at_ms: i64,
    pub ended_at_ms: i64,
    pub output_truncated: bool,
}

/// What kind of decision a row records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// A durable review of a submitted workflow.
    WorkflowReview,
    /// A question the ledger held a task behind.
    LedgerGate,
}

/// One decision somebody actually recorded for this checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRow {
    pub kind: DecisionKind,
    /// `approved`, `changes_requested`, `pending`, `resolved`, … — the
    /// store's or the ledger's own word, never a translation of one into the
    /// other's vocabulary.
    pub decision: String,
    /// The workflow state or gate status the decision left behind.
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at_ms: Option<i64>,
}

/* ---- the three sources ------------------------------------------------- */

/// Git's bounded observation of the checkout.
///
/// Wraps the existing [`Orchestrator::handoff_snapshot`] rather than shelling
/// out again: that reader already observes twice, refuses a checkout that
/// changed underneath it, and bounds its own output and time.
pub struct GitSnapshotSource<'a> {
    orchestrator: Option<&'a Orchestrator>,
}

impl<'a> GitSnapshotSource<'a> {
    #[must_use]
    pub fn new(orchestrator: Option<&'a Orchestrator>) -> Self {
        Self { orchestrator }
    }

    fn read(
        &self,
        target: &EvidenceTarget,
        observed_at_ms: i64,
    ) -> (Source<Option<SnapshotFacts>>, Option<WorktreeSnapshot>) {
        let Some(path) = target.path() else {
            return (Source::unsupported(), None);
        };
        let Some(orchestrator) = self.orchestrator else {
            return (Source::missing(), None);
        };
        match orchestrator.handoff_snapshot(path, observed_at_ms) {
            Ok(snapshot) => {
                let facts = snapshot_facts(&snapshot);
                let counted = snapshot.changes.len();
                let returned = facts.changes.len();
                (
                    Source {
                        // A clean checkout is still an observation, not an
                        // absence: `Ok` here says Git answered, and the
                        // emptiness is `dirty: false` inside it.
                        state: SourceState::Ok,
                        freshness: Freshness::Observed,
                        coverage: SourceCoverage::of(counted, returned),
                        error: None,
                        data: Some(facts),
                    },
                    Some(snapshot),
                )
            }
            Err(why) => (Source::failed(git_error(&why)), None),
        }
    }
}

/// The ledger's own rows for this checkout: who ran here, and what a
/// coordinator wrote about it afterwards.
pub struct LedgerSource<'a> {
    ledger: Option<&'a Ledger>,
}

impl<'a> LedgerSource<'a> {
    #[must_use]
    pub fn new(ledger: Option<&'a Ledger>) -> Self {
        Self { ledger }
    }

    fn read(
        &self,
        target: &EvidenceTarget,
    ) -> (
        Source<Vec<ExecutionRow>>,
        Source<Vec<ReportRow>>,
        Vec<DecisionRow>,
    ) {
        /* A seat's checkout is the path the window reported on ITS disk. A
         * tree on another host has no such key, so no row here can be said to
         * belong to it — and "empty" would claim the ledger looked and found
         * nothing, which is not what happened. Asked first, because it stays
         * true whether or not this window holds a ledger. */
        if !target.is_local() {
            return (Source::unsupported(), Source::unsupported(), Vec::new());
        }
        let Some(ledger) = self.ledger else {
            return (Source::missing(), Source::missing(), Vec::new());
        };
        let mut executions: Vec<ExecutionRow> = Vec::new();
        let mut reports: Vec<ReportRow> = Vec::new();
        let mut gates: Vec<DecisionRow> = Vec::new();
        let mut reported_tasks: Vec<(String, String)> = Vec::new();
        for run in ledger.runs() {
            for worker in &run.workers {
                if !target.holds(worker.checkout.as_deref()) {
                    continue;
                }
                /* Every attempt this worker carried, not only the one still
                 * linked to its row: a closed dispatch is unlinked from the
                 * worker, and a checkout's story is its attempts. */
                for dispatch in run.dispatches.iter().filter(|one| one.worker == worker.id) {
                    let task = run.task(&dispatch.task);
                    executions.push(ExecutionRow {
                        run_id: run.id.clone(),
                        dispatch_id: dispatch.id.clone(),
                        worker_id: worker.id.clone(),
                        agent: worker.agent.clone(),
                        task_id: dispatch.task.clone(),
                        task: task.map(roster_name).unwrap_or_default(),
                        started_ms: dispatch.started_ms,
                        ended_ms: dispatch.ended_ms,
                        open: dispatch.is_open(),
                        worker_reported_success: dispatch.succeeded,
                        retry_of: dispatch.retry_of.clone(),
                        model: worker.model.clone(),
                        effort: worker.effort.clone(),
                    });
                    let Some(task) = task else { continue };
                    let key = (run.id.clone(), task.id.clone());
                    if reported_tasks.contains(&key) {
                        continue;
                    }
                    reported_tasks.push(key);
                    // The coordinator's facts as they stand for the task's
                    // newest attempt; a worker's claims are not a
                    // coordinator report (t-6815).
                    let review: ReviewFacts = run.review_of(task);
                    reports.push(ReportRow {
                        kind: COORDINATOR_REPORT,
                        run_id: run.id.clone(),
                        task_id: task.id.clone(),
                        task: roster_name(task),
                        verified: review.verified,
                        merged: review.merged,
                        merge_head: review.merge_head.clone(),
                        deployed: review.deployed,
                        written: review.written,
                    });
                    for gate in run.gates.iter().filter(|gate| gate.task == task.id) {
                        gates.push(DecisionRow {
                            kind: DecisionKind::LedgerGate,
                            decision: gate.status.as_str().to_string(),
                            state: gate.status.as_str().to_string(),
                            subject: Some(clipped(gate.question.as_str())),
                            detail: match gate.resolution.is_empty() {
                                true => None,
                                false => Some(clipped(gate.resolution.as_str())),
                            },
                            at_ms: Some(gate.resolved_ms.unwrap_or(gate.created_ms)),
                        });
                    }
                }
            }
        }
        // Newest first, everywhere: a reader opens this to find out what
        // happened last.
        executions.sort_by(|left, right| right.started_ms.cmp(&left.started_ms));
        reports.sort_by(|left, right| right.task_id.cmp(&left.task_id));
        (
            bounded(executions, MAX_EXECUTIONS),
            bounded(reports, MAX_REPORTS),
            gates,
        )
    }
}

/// The authority store's own rows: durable review decisions, and the receipts
/// the host-trusted runner left.
///
/// Opened read-only and never created: a window that has no authority store
/// answers `missing`, and this query is not the road that would make one.
pub struct WorkflowSource<'a> {
    store: Option<&'a Path>,
}

impl<'a> WorkflowSource<'a> {
    #[must_use]
    pub fn new(store: Option<&'a Path>) -> Self {
        Self { store }
    }

    /// What the store had to say, and — first — whether it was there to ask.
    ///
    /// The state travels with the rows because both sections that read it
    /// need it: a window with no authority store has not found zero receipts,
    /// and answering `empty` for it would be the exact confusion this whole
    /// read exists to prevent.
    fn read(&self, identity: Result<(&str, &str), Unidentified>) -> WorkflowFacts {
        /* Which checkout comes before whether there is a store. The store's
         * rows are keyed by Git's proof of the worktree, so a tree Git could
         * not identify is a tree nothing stored can be looked up for — and
         * that stays true whether or not this window has a store at all. */
        let (repository_id, worktree_id) = match identity {
            Ok(held) => held,
            Err(Unidentified::NotAGitCheckout) => {
                return WorkflowFacts::unread(SourceState::Unsupported, None);
            }
            Err(Unidentified::GitRefused(retryable)) => {
                return WorkflowFacts::unread(
                    SourceState::Error,
                    Some(EvidenceError {
                        code: "checkout_unidentified",
                        message: "the checkout could not be identified, so nothing stored \
                                  against it could be looked up",
                        retryable,
                    }),
                );
            }
        };
        let Some(path) = self.store else {
            return WorkflowFacts::unread(SourceState::Missing, None);
        };
        let opened = match ReadOnlyWorkflows::open(path) {
            Ok(Some(store)) => store,
            Ok(None) => return WorkflowFacts::unread(SourceState::Missing, None),
            Err(why) => {
                return WorkflowFacts::unread(SourceState::Error, Some(store_error(&why)));
            }
        };
        let reviews = match opened.reviews_for(repository_id, worktree_id, MAX_WORKFLOWS) {
            Ok(rows) => rows,
            Err(why) => {
                return WorkflowFacts::unread(SourceState::Error, Some(store_error(&why)));
            }
        };
        let manifests: Vec<String> = reviews
            .iter()
            .filter_map(|review| review.manifest_id.clone())
            .collect();
        /* A receipt read that refuses AFTER the reviews came back keeps the
         * reviews: they were read, and throwing them away to report one
         * refusal would lose evidence that is already in hand. */
        let (trusted, receipts_error) = match opened.trusted_receipts_for(&manifests, MAX_RECEIPTS)
        {
            Ok(trusted) => (trusted, None),
            Err(why) => (Vec::new(), Some(store_error(&why))),
        };
        WorkflowFacts {
            state: SourceState::Ok,
            error: None,
            receipts_error,
            reviews,
            trusted,
        }
    }
}

/// Why Git could not say which worktree this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unidentified {
    /// Another host's tree, or a folder with no repository: there is no
    /// identity to ask for.
    NotAGitCheckout,
    /// Git was asked and refused; whether asking again could help.
    GitRefused(bool),
}

/// The store's answer: whether it was readable at all, and what it held.
struct WorkflowFacts {
    /// `Ok` once the store answered — the rows below may still be empty, and
    /// that emptiness is then a fact about this checkout.
    state: SourceState,
    /// Why the store could not be asked at all.
    error: Option<EvidenceError>,
    /// Why the receipts could not be read after the reviews were. Kept apart
    /// from [`Self::error`] because it is news about ONE section: the reviews
    /// in hand are still good decisions.
    receipts_error: Option<EvidenceError>,
    reviews: Vec<StoredReview>,
    trusted: Vec<TrustedReceiptRow>,
}

impl WorkflowFacts {
    fn unread(state: SourceState, error: Option<EvidenceError>) -> Self {
        Self {
            state,
            error,
            receipts_error: None,
            reviews: Vec::new(),
            trusted: Vec::new(),
        }
    }

    /// Whether the store was read at all. `false` is the case where every
    /// section that depends on it must repeat its absence rather than report
    /// a count of zero.
    fn answered(&self) -> bool {
        self.state == SourceState::Ok
    }
}

const COORDINATOR_REPORT: &str = "coordinator_report";

/// What no road in this window records, named in the answer so its absence is
/// never read as a decision.
const UNRECORDED: [&str; 2] = ["tool_approvals", "ci_checks"];

/* ---- the one read ------------------------------------------------------ */

/// Everything three sources can say about one checkout, at one moment.
///
/// Asking twice changes nothing; nothing here writes, creates or executes.
#[must_use]
pub fn read(
    target: &EvidenceTarget,
    git: &GitSnapshotSource<'_>,
    ledger: &LedgerSource<'_>,
    workflows: &WorkflowSource<'_>,
    observed_at_ms: i64,
) -> WorktreeEvidenceV1 {
    let (snapshot_source, snapshot) = git.read(target, observed_at_ms);
    let (executions, reports, gates) = ledger.read(target);
    let identity = match (&snapshot, &snapshot_source.error) {
        (Some(held), _) => Ok((held.repository_id.as_str(), held.worktree_id.as_str())),
        (None, Some(refusal)) => Err(Unidentified::GitRefused(refusal.retryable)),
        (None, None) => Err(Unidentified::NotAGitCheckout),
    };
    let store = workflows.read(identity);

    let mut receipts: Vec<ReceiptRow> = Vec::new();
    let mut decisions: Vec<DecisionRow> = gates;
    for review in &store.reviews {
        if let Some(decision) = review_decision(review) {
            decisions.push(decision);
        }
        receipts.extend(manifest_receipts(review, snapshot.as_ref()));
    }
    receipts.extend(
        store
            .trusted
            .iter()
            .map(|row| trusted_receipt(row, snapshot.as_ref())),
    );
    // Newest first: a reader opens this to find out what happened last.
    receipts.sort_by(|left, right| right.ended_at_ms.cmp(&left.ended_at_ms));
    decisions.sort_by(|left, right| right.at_ms.cmp(&left.at_ms));

    /* Every receipt this window holds comes from the store, so a store that
     * could not be read leaves this section saying exactly that. */
    let verification = match (store.answered(), store.receipts_error.clone()) {
        (false, _) => Source::unread(store.state, store.error.clone()),
        (true, None) => bounded(receipts, MAX_RECEIPTS),
        /* The host-trusted rows could not be read. The manifest receipts that
         * WERE read still travel with the refusal beside them; with none in
         * hand the section is an error, never an empty that looks like "no
         * test ever ran here". */
        (true, Some(refusal)) if receipts.is_empty() => Source::failed(refusal),
        (true, Some(refusal)) => {
            let mut source = bounded(receipts, MAX_RECEIPTS);
            source.error = Some(refusal);
            source
        }
    };
    /* Decisions have TWO sources, so a refused store does not silence the
     * ledger's own gates: the rows that were read travel, and the store's
     * refusal travels beside them. A receipt refusal is not news here. */
    let decisions_source = match (store.answered(), decisions.is_empty()) {
        (true, _) => bounded(decisions, MAX_DECISIONS),
        (false, false) => {
            let mut source = bounded(decisions, MAX_DECISIONS);
            source.error = store.error;
            source
        }
        (false, true) => Source::unread(store.state, store.error),
    };

    let summary = summarize(
        &snapshot_source,
        &executions,
        &reports,
        &verification,
        &decisions_source,
    );
    WorktreeEvidenceV1 {
        schema_version: EVIDENCE_SCHEMA_VERSION,
        observed_at_ms,
        workspace_id: target.workspace_id.clone(),
        repository_id: snapshot.as_ref().map(|held| held.repository_id.clone()),
        worktree_id: snapshot.as_ref().map(|held| held.worktree_id.clone()),
        branch: snapshot.as_ref().and_then(|held| held.branch.clone()),
        summary,
        snapshot: snapshot_source,
        executions,
        reports,
        verification,
        decisions: decisions_source,
        ci: Source::unsupported(),
    }
}

fn summarize(
    snapshot: &Source<Option<SnapshotFacts>>,
    executions: &Source<Vec<ExecutionRow>>,
    reports: &Source<Vec<ReportRow>>,
    receipts: &Source<Vec<ReceiptRow>>,
    decisions: &Source<Vec<DecisionRow>>,
) -> EvidenceSummary {
    let facts = snapshot.data.as_ref();
    EvidenceSummary {
        dirty: facts.map(|held| held.dirty),
        changed_paths: snapshot.coverage.counted,
        executions: executions.coverage.counted,
        open_executions: executions.data.iter().filter(|row| row.open).count(),
        receipts_current: receipts
            .data
            .iter()
            .filter(|row| row.currency == Currency::Current)
            .count(),
        receipts_stale: receipts
            .data
            .iter()
            .filter(|row| row.currency == Currency::Stale)
            .count(),
        receipts_unknown: receipts
            .data
            .iter()
            .filter(|row| row.currency == Currency::Unknown)
            .count(),
        receipts_current_passing: receipts
            .data
            .iter()
            .filter(|row| row.currency == Currency::Current && row.passed)
            .count(),
        coordinator_reports: reports.coverage.counted,
        decisions: decisions.coverage.counted,
        unrecorded: UNRECORDED.to_vec(),
    }
}

fn bounded<T>(mut rows: Vec<T>, limit: usize) -> Source<Vec<T>> {
    let counted = rows.len();
    rows.truncate(limit);
    let returned = rows.len();
    Source::observed(rows, SourceCoverage::of(counted, returned))
}

fn snapshot_facts(snapshot: &WorktreeSnapshot) -> SnapshotFacts {
    SnapshotFacts {
        head_oid: snapshot.head_oid.clone(),
        content_digest: snapshot.content_digest.clone(),
        detached: snapshot.detached,
        locked: snapshot.locked,
        dirty: snapshot.is_dirty(),
        operation: snapshot.operation.map(|held| {
            match held {
                GitOperation::Merge => "merge",
                GitOperation::Rebase => "rebase",
                GitOperation::CherryPick => "cherry_pick",
                GitOperation::Other => "other",
            }
            .to_string()
        }),
        ahead: snapshot.ahead,
        behind: snapshot.behind,
        complete: snapshot.coverage.is_complete(),
        coverage_gaps: snapshot
            .coverage
            .gaps
            .iter()
            .take(MAX_COVERAGE_GAPS)
            .map(gap_tag)
            .collect(),
        changes: snapshot
            .changes
            .iter()
            .take(MAX_CHANGED_PATHS)
            .map(|change| ChangedPath {
                path: clipped(&change.path),
                code: change.code.clone(),
                staged: change.staged,
                unstaged: change.unstaged,
                untracked: change.untracked,
                conflicted: change.conflicted,
            })
            .collect(),
    }
}

/// A gap's KIND, never its path: `IndexHiddenContent` and `DirtySubmodule`
/// each name a file inside the checkout, and a repository-relative path is
/// fine — but the tag is what a reader acts on, and one word per gap keeps
/// the answer bounded whatever a repository holds.
fn gap_tag(gap: &CoverageGap) -> String {
    match gap {
        CoverageGap::IgnoredContent { .. } => "ignored_content",
        CoverageGap::UntrackedContent { .. } => "untracked_content",
        CoverageGap::ChangedPathLimit { .. } => "changed_path_limit",
        CoverageGap::ContentByteLimit { .. } => "content_byte_limit",
        CoverageGap::DirtySubmodule { .. } => "dirty_submodule",
        CoverageGap::IndexHiddenContent { .. } => "index_hidden_content",
    }
    .to_string()
}

/// Whether a receipt taken against `digest`/`oid` still describes what was
/// just observed.
///
/// The head is checked as well as the digest, and an incomplete observation
/// can never answer `Current`: a digest that did not cover the untracked file
/// somebody is working in says nothing about that file.
fn currency(
    snapshot: Option<&WorktreeSnapshot>,
    digest: &str,
    source_oid: Option<&str>,
) -> (Currency, &'static str) {
    let Some(snapshot) = snapshot else {
        return (Currency::Unknown, "not_observed");
    };
    if !snapshot.coverage.is_complete() {
        return (Currency::Unknown, "incomplete_observation");
    }
    if let Some(oid) = source_oid
        && oid != snapshot.head_oid
    {
        return (Currency::Stale, "head_moved");
    }
    match digest == snapshot.content_digest {
        true => (Currency::Current, "digest_matches"),
        false => (Currency::Stale, "content_changed"),
    }
}

fn trusted_receipt(row: &TrustedReceiptRow, snapshot: Option<&WorktreeSnapshot>) -> ReceiptRow {
    let (currency, reason) = currency(snapshot, &row.snapshot_digest, Some(&row.source_oid));
    ReceiptRow {
        source: ReceiptSource::HostTrusted,
        name: clipped(&row.test_name),
        command_id: clipped(&row.command_id),
        exit_code: row.exit_code,
        passed: row.succeeded,
        currency,
        currency_reason: reason,
        started_at_ms: row.started_at_ms,
        ended_at_ms: row.ended_at_ms,
        output_truncated: row.output_truncated,
    }
}

fn manifest_receipts(
    review: &StoredReview,
    snapshot: Option<&WorktreeSnapshot>,
) -> Vec<ReceiptRow> {
    review
        .manifest_tests
        .iter()
        .map(|test| {
            let (currency, reason) = currency(snapshot, &test.snapshot_digest, None);
            ReceiptRow {
                source: ReceiptSource::Manifest,
                name: clipped(&test.name),
                command_id: clipped(&test.command_id),
                exit_code: test.exit_code,
                passed: test.exit_code == 0,
                currency,
                currency_reason: reason,
                started_at_ms: test.started_at_ms,
                ended_at_ms: test.ended_at_ms,
                output_truncated: test.output_truncated,
            }
        })
        .collect()
}

/// A stored workflow becomes a decision row only when somebody decided
/// something. A workflow merely assigned is work in progress, not a verdict,
/// and inventing "pending approval" for it would be this module writing a
/// decision nobody made.
fn review_decision(review: &StoredReview) -> Option<DecisionRow> {
    let decision = review.review_decision.as_deref()?;
    Some(DecisionRow {
        kind: DecisionKind::WorkflowReview,
        decision: clipped(decision),
        state: clipped(&review.state),
        subject: review.reviewer_id.as_deref().map(clipped),
        detail: None,
        at_ms: None,
    })
}

/// The short name a roster shows for a task, and never its spec.
///
/// `Task::display_name` falls back to the spec's first line, which is the
/// coordinator's own instruction — a prompt. This answer crosses a command
/// boundary, so the fallback here is the id: a name nobody wrote is better
/// than an instruction nobody meant to publish.
fn roster_name(task: &zerocode_core::orchestration::Task) -> String {
    match task.title.is_empty() {
        true => task.id.clone(),
        false => clipped(task.title.as_str()),
    }
}

fn clipped(text: &str) -> String {
    match text.chars().count() > MAX_TEXT_CHARS {
        true => text.chars().take(MAX_TEXT_CHARS).collect::<String>() + "…",
        false => text.to_string(),
    }
}

/// Git's refusals, turned into the four things a caller can do about them.
/// The Git sentence itself never travels: it carries the checkout's path.
fn git_error(why: &HandoffError) -> EvidenceError {
    match why {
        HandoffError::NoHead { .. } => EvidenceError {
            code: "no_head",
            message: "this checkout has no commit yet",
            retryable: false,
        },
        HandoffError::ChangedDuringCapture => EvidenceError {
            code: "changed_while_read",
            message: "the checkout changed while it was being read",
            retryable: true,
        },
        HandoffError::GitTimeout { .. } => EvidenceError {
            code: "git_timeout",
            message: "git did not answer in time",
            retryable: true,
        },
        HandoffError::GitOutputTooLarge { .. } => EvidenceError {
            code: "git_output_too_large",
            message: "git answered with more than this read accepts",
            retryable: false,
        },
        _ => EvidenceError {
            code: "git_unreadable",
            message: "git could not be read here",
            retryable: true,
        },
    }
}

/// The store's refusals, in the same terms. A `Database`/`Corrupt` sentence
/// names store internals, so only the class travels.
fn store_error(why: &WorkflowStoreError) -> EvidenceError {
    match why {
        WorkflowStoreError::UnsupportedSchema { .. } => EvidenceError {
            code: "store_schema_unsupported",
            message: "the authority store was written by another version",
            retryable: false,
        },
        WorkflowStoreError::UnsafeStorePath => EvidenceError {
            code: "store_path_unsafe",
            message: "the authority store is not a private regular file",
            retryable: false,
        },
        WorkflowStoreError::Corrupt => EvidenceError {
            code: "store_corrupt",
            message: "the authority store holds a row this read cannot trust",
            retryable: false,
        },
        _ => EvidenceError {
            code: "store_unavailable",
            message: "the authority store could not be read",
            retryable: true,
        },
    }
}
