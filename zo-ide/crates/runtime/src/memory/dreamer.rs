//! Dreaming memory: the runtime IO seam around the pure curation brain.
//!
//! This is the "hands" half of the Dreamer (doc §10 / §8-8). The "brain" —
//! *which* lessons to promote and *why* — lives in [`decision_core::dreamer`]
//! and is pure and unit-tested. This module owns only the IO that genuinely
//! differs by environment, behind dependency-inverted traits so the
//! orchestrator stays testable without a real filesystem:
//!
//! - [`LessonSource`] — where candidate observations come from. The production
//!   impl, [`JsonlLessonSource`], reads append-only `*.jsonl` candidate logs
//!   that sessions write under `.zo/dream/`.
//! - [`MemoryStore`] — where promoted lessons go. The production impl,
//!   [`FsMemoryStore`], writes `<global-project-memory>/<slug>.md` and upserts
//!   the `MEMORY.md` pointer with the *same* byte layout as the `MemoryWrite`
//!   tool, so a dreamed entry is indistinguishable from a hand-written one.
//!
//! [`Dreamer::run`] is the whole loop: read existing index summaries (so re-runs
//! are idempotent and template families stay bounded), read observations,
//! curate them through the policy gate, then apply only the approved promotions
//! and report the full audit trail ([`DreamReport`]).

use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use core_types::paths::ZO_DIR_NAME;
use crate::jsonl_log::{
    jsonl_files_newest_first, jsonl_files_oldest_first, prune_jsonl_lines, read_jsonl_lines,
};
#[cfg(unix)]
use crate::jsonl_log::{
    append_jsonl_line_retained_durable, jsonl_files_oldest_first_retained,
    prune_jsonl_files_retained, prune_jsonl_lines_retained, read_jsonl_lines_retained,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::memory::{
    MemoryClassification, MemoryKind, MemoryModelTag, MemorySource, classify_memory_body,
    memory_body_has_classification_metadata,
};
#[cfg(unix)]
use crate::secure_fs::RetainedDir;
use decision_core::dreamer::{
    curate_with_existing_summaries, decide_apply_gate, synthesize_dream_fusion, AdvisorFinding,
    AdvisorRole, ApplyGateDecision, ApplyGateInput, AutomationDigest, CandidateEvidence,
    CandidateKind, CandidateStatus, CurationPlan, DreamFusionReport, LessonKind,
    LessonObservation, PatchCheckResult, PatchRisk, PromotionPolicy, QuarantinePatchRun,
    SelfImproveCandidate,
};

/// Directory under `.zo/` where sessions drop candidate-lesson logs and the
/// Dreamer reads them. Append-only JSONL, one [`LessonObservation`] per line —
/// the same "external event log" shape the workflow engine already uses.
const DREAM_DIR: &str = "dream";
const AUTOMATION_DIR: &str = "automation";
const USER_PATTERN_DIR: &str = "user-patterns";
const USER_PATTERN_FILE: &str = "patterns.jsonl";
const USER_PATTERN_SUMMARY_MAX_BYTES: usize = 512;
const SELF_IMPROVE_CANDIDATES_DIR: &str = "candidates";
const DREAM_FUSION_DIR: &str = "fusion";
const DREAM_QUARANTINE_DIR: &str = "quarantine";
#[cfg(test)]
const MAX_QUARANTINE_RUNS: usize = 3;
#[cfg(not(test))]
const MAX_QUARANTINE_RUNS: usize = 32;
/// Where the decay pass moves expired dreamer entries (under the memory store,
/// not `.zo/`). Archiving rather than deleting keeps a wrongly-decayed lesson
/// recoverable.
const DECAY_ARCHIVE_DIR: &str = "archive";
const AUTO_DREAM_ERROR_FILE: &str = ".last_auto_dream_error.json";
const SELF_IMPROVE_ATTEMPT_MARKER: &str = ".last_self_improve_attempt";
const SELF_IMPROVE_ERROR_FILE: &str = ".last_self_improve_error.json";
const SELF_IMPROVE_LOCK_FILE: &str = ".self_improve.lock";
pub(crate) const MEMORY_STORE_LOCK_FILE: &str = ".memory-store.lock";
const MEMORY_WRITE_JOURNAL_FILE: &str = ".memory-write-journal.json";
const MEMORY_DECAY_JOURNAL_FILE: &str = ".memory-decay-journal.json";
const MEMORY_STORE_LOCK_RETRY_COUNT: usize = 40;
const MEMORY_STORE_LOCK_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(10);
static MEMORY_STORE_PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
const MAX_DREAM_JSONL_LINES: usize = 4;
#[cfg(not(test))]
const MAX_DREAM_JSONL_LINES: usize = 10_000;

#[cfg(test)]
const MAX_DREAM_JSONL_FILES: usize = 4;
#[cfg(not(test))]
const MAX_DREAM_JSONL_FILES: usize = 256;

#[cfg(test)]
const MAX_DREAM_OBSERVATIONS: usize = 8;
#[cfg(not(test))]
const MAX_DREAM_OBSERVATIONS: usize = 10_000;

#[cfg(test)]
const MAX_AUTOMATION_DIGESTS: usize = 8;
#[cfg(not(test))]
const MAX_AUTOMATION_DIGESTS: usize = 10_000;

#[cfg(test)]
const MAX_SELF_IMPROVE_CANDIDATE_LINES: usize = 4;
#[cfg(not(test))]
const MAX_SELF_IMPROVE_CANDIDATE_LINES: usize = 2_000;

#[cfg(test)]
const MAX_SELF_IMPROVE_CANDIDATE_FILES: usize = 4;
#[cfg(not(test))]
const MAX_SELF_IMPROVE_CANDIDATE_FILES: usize = 256;

#[cfg(test)]
const MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE: usize = 4;
#[cfg(not(test))]
const MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE: usize = 64;

/// The filesystem-backed memory store used by Dreamer promotion. Automatic
/// passes use the local project overlay; an explicit global store remains
/// available to callers that intentionally promote durable knowledge.
///
/// Errors from a dreaming run. Only genuine IO failures surface here; a parse
/// failure on a single candidate line is skipped, never fatal (mirroring the
/// workflow event-log reader's lossy tolerance), so one corrupt line cannot
/// stop the whole pass.
#[derive(Debug, thiserror::Error)]
pub enum DreamError {
    #[error("dreamer IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Source of candidate lessons (DIP seam). Implementors decide *where* the
/// observations live; the orchestrator only consumes the merged list.
pub trait LessonSource {
    /// All candidate observations to consider this run, in any order (the brain
    /// groups and sorts deterministically). Best-effort: unreadable or
    /// malformed records are dropped, not surfaced as errors.
    fn observations(&self) -> Vec<LessonObservation>;
}

/// Destination for promoted lessons plus the read side the gate needs for
/// idempotency and template coalescing (DIP seam). Splitting reads from
/// [`MemoryStore::write_entry`] keeps each method single-purpose.
pub trait MemoryStore {
    /// Slugs already present in long-term memory, so the gate can skip
    /// already-known lessons instead of rewriting them.
    fn existing_slugs(&self) -> Vec<String>;

    /// Existing index summaries paired with their slugs. Stores that only
    /// expose slugs retain exact-slug idempotency; filesystem stores override
    /// this so curation can also coalesce summary-template families.
    fn existing_summaries(&self) -> Vec<(String, String)> {
        self.existing_slugs()
            .into_iter()
            .map(|slug| (slug, String::new()))
            .collect()
    }

    /// Persist one promoted lesson: write its entry file and upsert its index
    /// pointer. Returns whether the entry was newly `created` or `updated`.
    fn write_entry(&self, entry: &MemoryWriteRequest) -> Result<WriteOutcome, DreamError>;
}

/// Whether a [`MemoryStore::write_entry`] created a new entry, updated one, or
/// found everything already in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Created,
    Updated,
    /// The entry, its ownership marker and its index pointer already hold
    /// exactly these bytes — nothing was written. The steady-state outcome of
    /// the `MemoryWrite` tool: a model re-records the lesson it considers
    /// important in session after session, byte-identically.
    Unchanged,
}

impl WriteOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Unchanged => "unchanged",
        }
    }
}

/// A fully-resolved write request the store persists verbatim. Built by the
/// orchestrator from a `PromotedLesson` so the store stays a dumb sink.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryWriteRequest {
    pub slug: String,
    pub summary: String,
    pub body: String,
}

/// One promotion that actually hit the store, for the run report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedPromotion {
    pub slug: String,
    pub outcome: WriteOutcome,
}

/// The auditable result of one dreaming run (doc §13: return evidence + what
/// was skipped, not just successes). `skipped` carries every rejected candidate
/// and its reason straight from the [`CurationPlan`].
#[derive(Debug, Clone, Default)]
pub struct DreamReport {
    /// Lessons written to memory this run.
    pub applied: Vec<AppliedPromotion>,
    /// The full curation plan (promotions + skip reasons) for the audit trail.
    pub plan: CurationPlan,
}

impl DreamReport {
    /// True when nothing was written — the caller can stay silent.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.applied.is_empty()
    }

    /// One-line human summary for logs/notifications.
    #[must_use]
    pub fn summary_line(&self) -> String {
        let unchanged = self
            .applied
            .iter()
            .filter(|applied| applied.outcome == WriteOutcome::Unchanged)
            .count();
        let promoted = self.applied.len() - unchanged;
        if unchanged == 0 {
            format!(
                "dreamer: promoted {promoted}, skipped {}",
                self.plan.skipped.len()
            )
        } else {
            // A lesson that needed no write is its own count — "promoted 5"
            // for five untouched entries reads as churn that did not happen.
            format!(
                "dreamer: promoted {promoted}, {unchanged} already current, skipped {}",
                self.plan.skipped.len()
            )
        }
    }
}

/// The Dreamer orchestrator: binds a [`LessonSource`], a [`MemoryStore`], and a
/// [`PromotionPolicy`] into one between-sessions curation pass.
pub struct Dreamer<S, M> {
    source: S,
    store: M,
    policy: PromotionPolicy,
    author: Option<MemoryModelTag>,
}

impl<S: LessonSource, M: MemoryStore> Dreamer<S, M> {
    /// Build a Dreamer over an explicit source, store, and policy (the
    /// dependency-injected form used by tests and custom hosts).
    pub fn new(source: S, store: M, policy: PromotionPolicy) -> Self {
        Self {
            source,
            store,
            policy,
            author: None,
        }
    }

    /// Name the model running this pass, so every entry it promotes records who
    /// promoted it and recall can prefer that model's own lessons later.
    ///
    /// The observations themselves carry no model (they are mined from turn
    /// logs written before this existed), so the honest claim an entry can make
    /// is "this model's pass promoted me" — which is exactly the identity the
    /// session that reads it back will be comparing against. A pass with no
    /// model to name leaves its entries untagged rather than guessing.
    #[must_use]
    pub fn authored_by(mut self, model: Option<&str>) -> Self {
        self.author = model.and_then(MemoryModelTag::new);
        self
    }

    /// Run one curation pass end to end: gather observations, gate them against
    /// existing memory + policy, then apply only the approved promotions.
    ///
    /// Ordering matters for the anti-pollution guarantee: existing summaries
    /// are read *before* any write, so within a run the gate never promotes a
    /// slug it is about to create twice, and across runs an already-written
    /// lesson or summary template is recognised and skipped.
    pub fn run(&self) -> Result<DreamReport, DreamError> {
        let observations = self.source.observations();
        let existing = self.store.existing_summaries();
        let plan = curate_with_existing_summaries(&observations, &existing, self.policy);

        let now_secs = now_secs();
        let mut applied = Vec::with_capacity(plan.promote.len());
        for lesson in &plan.promote {
            let request = MemoryWriteRequest {
                slug: lesson.slug.clone(),
                summary: lesson.summary.clone(),
                body: render_entry_body(lesson, now_secs, self.author.clone()),
            };
            let outcome = self.store.write_entry(&request)?;
            applied.push(AppliedPromotion {
                slug: lesson.slug.clone(),
                outcome,
            });
        }

        Ok(DreamReport { applied, plan })
    }
}

/// Provenance trailer line that marks an entry as dreamer-promoted. The decay
/// pass scopes itself to entries carrying this exact prefix so it never archives
/// a hand-written `MemoryWrite` entry (which writes its body verbatim, without
/// any provenance trailer).
const DREAMER_SOURCE_PREFIX: &str = "- source: dreamer";

/// Entry-body line carrying the unix-seconds write time the decay pass ages off.
const WRITTEN_FIELD_PREFIX: &str = "- written: ";

/// Entry-body line carrying the policy expiry the decay pass enforces.
const REVISIT_FIELD_PREFIX: &str = "- revisit_after_days: ";

pub(crate) const DREAMER_OWNED_DIR: &str = ".dreamer-owned";
const DREAMER_OWNED_MARKER_VERSION: &str = "v=1";

fn memory_kind_for_lesson(kind: LessonKind) -> MemoryKind {
    MemoryKind::from_lesson(kind)
}

#[cfg(any(not(unix), test))]
fn dreamer_owned_marker_dir(memory_dir: &Path) -> PathBuf {
    memory_dir.join(DREAMER_OWNED_DIR)
}

#[cfg(any(not(unix), test))]
fn dreamer_owned_marker_path(memory_dir: &Path, slug: &str) -> PathBuf {
    dreamer_owned_marker_dir(memory_dir).join(format!("{slug}.marker"))
}

fn dreamer_owned_body_hash(body: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in body.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(any(not(unix), test))]
fn ensure_dreamer_owned_marker_dir_durable(memory_dir: &Path) -> Result<(), DreamError> {
    ensure_child_dir_no_symlink(memory_dir, DREAMER_OWNED_DIR)?;
    crate::secure_fs::sync_parent_directory(memory_dir, Path::new(DREAMER_OWNED_DIR))?;
    Ok(())
}

/// The exact marker file contents that stamp a Dreamer-owned entry whose body
/// on disk is `body`. One producer: the ownership guard and the idempotence
/// check compare against this string, so the format cannot drift from what the
/// writers put down.
fn dreamer_owned_marker_contents(body: &str) -> String {
    format!(
        "{DREAMER_OWNED_MARKER_VERSION}\nhash={:016x}\n",
        dreamer_owned_body_hash(body)
    )
}

#[cfg(any(not(unix), test))]
fn write_dreamer_owned_marker(memory_dir: &Path, slug: &str, body: &str) -> Result<(), DreamError> {
    ensure_dreamer_owned_marker_dir_durable(memory_dir)?;
    write_text_atomic(
        memory_dir,
        &dreamer_owned_marker_path(memory_dir, slug),
        &dreamer_owned_marker_contents(body),
    )?;
    Ok(())
}

/// Where a slug's marker sits **relative to the memory dir**.
///
/// Both recovery paths need this shape in different forms: the retained (unix)
/// path opens it through a directory handle, the fallback path joins it onto
/// `memory_dir`. One producer keeps a marker from being written at one place
/// and restored from another — the fallback path called a name that did not
/// exist, which only a Windows cross-check could catch.
fn dreamer_owned_marker_relative_path(slug: &str) -> PathBuf {
    Path::new(DREAMER_OWNED_DIR).join(format!("{slug}.marker"))
}

#[cfg(unix)]
fn has_matching_dreamer_owned_marker(
    memory: &crate::secure_fs::RetainedDir,
    slug: &str,
    body: &str,
) -> bool {
    let expected = format!(
        "{DREAMER_OWNED_MARKER_VERSION}
hash={:016x}
",
        dreamer_owned_body_hash(body)
    );
    memory
        .open_regular_file(&dreamer_owned_marker_relative_path(slug))
        .and_then(|mut marker| marker.read_to_string())
        .map(|marker| marker == expected)
        .unwrap_or(false)
}

/// Render a promoted lesson into its entry-file body, stamping the provenance
/// the doc requires for promoted memory (§5-7: source sessions, confidence,
/// expiry; §10-2: every promoted lesson is traceable and decayable).
///
/// `now_secs` is the unix-seconds write time, injected so the stamp (and the
/// decay pass that reads it back) is deterministic under test. It is recorded
/// only alongside an expiry, since without an expiry there is nothing to age.
fn render_entry_body(
    lesson: &decision_core::dreamer::PromotedLesson,
    now_secs: u64,
    author: Option<MemoryModelTag>,
) -> String {
    use std::fmt::Write as _;
    let mut body = String::new();
    body.push_str(&lesson.lesson);
    body.push_str("\n\n---\n");
    // Writing into a String is infallible, so the `write!` results are ignored.
    let _ = writeln!(body, "- kind: {}", lesson.kind);
    let _ = writeln!(
        body,
        "{}",
        MemoryClassification::dreamer(
            memory_kind_for_lesson(lesson.kind),
            false,
            Some(now_secs),
            author
        )
        .with_scope(crate::memory::MemoryScope::Local)
        .metadata_line()
    );
    let _ = writeln!(
        body,
        "- evidence: {} distinct session(s), verified: {}",
        lesson.distinct_sessions, lesson.verified
    );
    let _ = writeln!(body, "- confidence: {:.2}", lesson.confidence);
    if lesson.expiry_days > 0 {
        let _ = writeln!(body, "{WRITTEN_FIELD_PREFIX}{now_secs}");
        let _ = writeln!(body, "{REVISIT_FIELD_PREFIX}{}", lesson.expiry_days);
    }
    body.push_str(DREAMER_SOURCE_PREFIX);
    body.push_str(" (auto-promoted from repeated, verified sessions)\n");
    body
}

/// What a [`render_entry_body`]-shaped entry tells the decay pass: whether it is
/// dreamer-owned, and (when stamped) its write time and revisit window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct EntryDecayFields {
    is_dreamer: bool,
    written_secs: Option<u64>,
    revisit_after_days: Option<u32>,
}

/// Parse the decay-relevant trailer lines out of an entry-file body. Pure and
/// tolerant: any missing or unparseable field reads as absent, so a malformed
/// entry simply never decays rather than being wrongly archived.
fn parse_entry_decay_fields(body: &str) -> EntryDecayFields {
    let mut fields = EntryDecayFields::default();
    let Some(trailer) = body.rsplit_once("
---
").map(|(_, trailer)| trailer) else {
        return fields;
    };
    for line in trailer.lines() {
        let line = line.trim();
        if line == DREAMER_SOURCE_PREFIX || line.starts_with("- source: dreamer (") {
            fields.is_dreamer = true;
        } else if let Some(rest) = line.strip_prefix(WRITTEN_FIELD_PREFIX) {
            fields.written_secs = rest.trim().parse().ok();
        } else if let Some(rest) = line.strip_prefix(REVISIT_FIELD_PREFIX) {
            fields.revisit_after_days = rest.trim().parse().ok();
        }
    }
    fields
}

/// Deterministic, best-effort decay decision for a single entry.
///
/// An entry is archived only when it is dreamer-owned, carries both a write time
/// and a positive revisit window, and `now_secs` is strictly past
/// `written + revisit_after_days`. Anything else — hand-written entries, entries
/// missing a stamp, future-dated writes (clock skew) — is kept. This is the
/// age-threshold guard the spec requires: a verified high-value lesson is never
/// lost until it is genuinely past its revisit window.
fn entry_is_expired(fields: EntryDecayFields, now_secs: u64) -> bool {
    if !fields.is_dreamer {
        return false;
    }
    let (Some(written), Some(days)) = (fields.written_secs, fields.revisit_after_days) else {
        return false;
    };
    if days == 0 {
        return false;
    }
    let ttl_secs = u64::from(days).saturating_mul(86_400);
    let deadline = written.saturating_add(ttl_secs);
    now_secs > deadline
}

#[cfg(unix)]
fn entry_is_decay_archive_candidate(
    memory: &crate::secure_fs::RetainedDir,
    slug: &str,
    body: &str,
    now_secs: u64,
) -> bool {
    if !has_matching_dreamer_owned_marker(memory, slug, body) {
        return false;
    }
    if !entry_is_expired(parse_entry_decay_fields(body), now_secs) {
        return false;
    }
    if !memory_body_has_classification_metadata(body) {
        return false;
    }
    let classification = classify_memory_body(body);
    classification.source == MemorySource::Dreamer
        && !classification.protected
        && !classification.resolved_task_log
}

// ---------------------------------------------------------------------------
// Production IO impls
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AutomationEventRecord {
    session_id: String,
    kind: String,
    event: String,
    verified: bool,
    ts_ms: u64,
}

fn safe_stem(input: &str) -> String {
    let stem: String = input
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    if stem.is_empty() {
        "session".to_string()
    } else {
        stem
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Wall-clock unix seconds, used to stamp entry write times and to age them off
/// in the decay pass. Saturates rather than panicking on a clock before the
/// epoch (treated as `0`), mirroring [`now_ms`].
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Record one goal/loop automation event for later Dreamer curation.
pub fn record_automation_event(
    cwd: &Path,
    session_id: &str,
    kind: &str,
    event: &str,
    verified: bool,
) -> Result<(), DreamError> {
    let kind = kind.trim();
    let event = event.trim();
    if kind.is_empty() || event.is_empty() {
        return Ok(());
    }
    let dir = ensure_repo_dream_child_dir_no_symlink(cwd, AUTOMATION_DIR)?;
    let record = AutomationEventRecord {
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        event: event.to_string(),
        verified,
        ts_ms: now_ms(),
    };
    let path = dir.join(format!("{}.jsonl", safe_stem(session_id)));
    let mut line =
        serde_json::to_string(&record).map_err(|e| DreamError::Io(std::io::Error::other(e)))?;
    line.push('\n');
    append_jsonl_line_no_symlink(cwd, &path, &line)?;
    prune_jsonl_lines(&path, MAX_DREAM_JSONL_LINES)?;
    prune_jsonl_files(&dir, MAX_DREAM_JSONL_FILES)?;
    Ok(())
}

/// Best-effort observability for automatic Dreamer failures.
pub fn record_auto_dream_failure(cwd: &Path, error: &DreamError) -> Result<(), DreamError> {
    let dir = ensure_repo_dream_dir_no_symlink(cwd)?;
    let payload = serde_json::json!({
        "tsMs": now_ms(),
        "error": dream_error_signature(error),
    });
    let bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|e| DreamError::Io(std::io::Error::other(e)))?;
    write_text_atomic(
        &trace_base(cwd),
        &dir.join(AUTO_DREAM_ERROR_FILE),
        &String::from_utf8_lossy(&bytes),
    )?;
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelfImproveCandidateRecord {
    ts_ms: u64,
    candidate: SelfImproveCandidate,
}

/// Where this workspace's `.zo/dream` and `.zo/turns` are rooted — the tree
/// itself until the host relocates traces (`crate::traces_base`). Every path
/// below that names a trace file goes through here; the workspace `cwd`
/// stays the identity for memory and slugs.
fn trace_base(cwd: &Path) -> PathBuf {
    crate::traces_base(cwd)
}

fn dream_dir(cwd: &Path) -> PathBuf {
    trace_base(cwd).join(ZO_DIR_NAME).join(DREAM_DIR)
}

fn self_improve_candidates_dir(cwd: &Path) -> PathBuf {
    dream_dir(cwd).join(SELF_IMPROVE_CANDIDATES_DIR)
}

fn push_capped<T>(items: &mut VecDeque<T>, item: T, cap: usize) {
    if cap == 0 {
        return;
    }
    items.push_back(item);
    while items.len() > cap {
        items.pop_front();
    }
}

fn dream_error_signature(_error: &DreamError) -> &'static str {
    "dreamer_io_error"
}

fn write_text_atomic(root: &Path, path: &Path, content: &str) -> Result<(), DreamError> {
    let relative = path.strip_prefix(root).map_err(|_| {
        DreamError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "artifact path escaped its trusted root",
        ))
    })?;
    crate::secure_fs::write_atomic_owner_only(root, relative, content.as_bytes())
        .map_err(DreamError::Io)
}

fn prune_jsonl_files(dir: &Path, keep_files: usize) -> std::io::Result<()> {
    let files = jsonl_files_newest_first(dir);
    for path in files.into_iter().skip(keep_files) {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn ensure_child_dir_no_symlink(parent: &Path, child: &str) -> std::io::Result<PathBuf> {
    crate::secure_fs::ensure_private_dir(parent, Path::new(child))
}

fn existing_child_dir_no_symlink(parent: &Path, child: &str) -> Result<Option<PathBuf>, DreamError> {
    let path = parent.join(child);
    match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            let file_type = metadata.file_type();
            if file_type.is_symlink() || !file_type.is_dir() {
                return Err(DreamError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "dream path component is not a real directory",
                )));
            }
            Ok(Some(path))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DreamError::Io(error)),
    }
}

fn existing_repo_dream_dir_no_symlink(cwd: &Path) -> Result<Option<PathBuf>, DreamError> {
    let Some(zo_dir) = existing_child_dir_no_symlink(&trace_base(cwd), ZO_DIR_NAME)? else {
        return Ok(None);
    };
    existing_child_dir_no_symlink(&zo_dir, DREAM_DIR)
}

fn ensure_repo_dream_dir_no_symlink(cwd: &Path) -> std::io::Result<PathBuf> {
    let zo_dir = ensure_child_dir_no_symlink(&trace_base(cwd), ZO_DIR_NAME)?;
    ensure_child_dir_no_symlink(&zo_dir, DREAM_DIR)
}

fn ensure_repo_dream_child_dir_no_symlink(cwd: &Path, child: &str) -> std::io::Result<PathBuf> {
    let dream_dir = ensure_repo_dream_dir_no_symlink(cwd)?;
    ensure_child_dir_no_symlink(&dream_dir, child)
}

fn append_jsonl_line_no_symlink(
    cwd: &Path,
    path: &Path,
    line: &str,
) -> std::io::Result<()> {
    let relative = path.strip_prefix(cwd).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "jsonl path escaped its trusted root",
        )
    })?;
    crate::secure_fs::append_owner_only(cwd, relative, line.as_bytes())
}

const CANDIDATE_STORE_LOCK_ATTEMPTS: usize = 20;
const CANDIDATE_STORE_LOCK_RETRY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(5);
static CANDIDATE_STORE_PROCESS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct CandidateStoreLock {
    _process: std::sync::MutexGuard<'static, ()>,
    _file: crate::secure_fs::ExclusiveFileLock,
    #[cfg(unix)]
    dir: crate::secure_fs::RetainedDir,
}

fn lock_candidate_store(cwd: &Path, dir: &Path) -> std::io::Result<CandidateStoreLock> {
    let expected = trace_base(cwd)
        .join(ZO_DIR_NAME)
        .join(DREAM_DIR)
        .join(SELF_IMPROVE_CANDIDATES_DIR);
    if dir != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "candidate store path does not match its trusted root",
        ));
    }
    let process = CANDIDATE_STORE_PROCESS_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = ensure_repo_dream_child_dir_no_symlink(
        cwd,
        SELF_IMPROVE_CANDIDATES_DIR,
    )?;
    #[cfg(unix)]
    let dir = crate::secure_fs::RetainedDir::open(&dir)?;

    for attempt in 0..CANDIDATE_STORE_LOCK_ATTEMPTS {
        #[cfg(unix)]
        let file = dir.try_lock_owner_only(Path::new(".candidate-store.lock"))?;
        #[cfg(not(unix))]
        let file = {
            let relative = Path::new(ZO_DIR_NAME)
                .join(DREAM_DIR)
                .join(SELF_IMPROVE_CANDIDATES_DIR)
                .join(".candidate-store.lock");
            crate::secure_fs::try_lock_owner_only(&trace_base(cwd), &relative)?
        };
        if let Some(file) = file {
            return Ok(CandidateStoreLock {
                _process: process,
                _file: file,
                #[cfg(unix)]
                dir,
            });
        }
        if attempt + 1 < CANDIDATE_STORE_LOCK_ATTEMPTS {
            std::thread::sleep(CANDIDATE_STORE_LOCK_RETRY_DELAY);
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "candidate store lock acquisition timed out",
            ));
        }
    }
    unreachable!("candidate store lock attempts are non-zero")
}

fn truncate_detail(input: &str) -> String {
    const MAX_CHARS: usize = 240;
    let trimmed = input.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let mut out = trimmed.chars().take(MAX_CHARS).collect::<String>();
    out.push('…');
    out
}

fn candidate_status_precedence(status: CandidateStatus) -> u8 {
    match status {
        CandidateStatus::Proposed => 0,
        CandidateStatus::Planned => 1,
        CandidateStatus::Rejected => 2,
        CandidateStatus::Quarantined => 3,
        CandidateStatus::Applied => 4,
    }
}

fn advance_candidate_status(
    existing: CandidateStatus,
    incoming: CandidateStatus,
) -> CandidateStatus {
    if existing.is_terminal() {
        return existing;
    }
    if incoming.is_terminal() {
        return incoming;
    }
    if candidate_status_precedence(incoming) >= candidate_status_precedence(existing) {
        incoming
    } else {
        existing
    }
}

fn evidence_bucket(evidence: &CandidateEvidence) -> String {
    let session = evidence.session_id.trim();
    if session.is_empty() {
        format!("host:{}", evidence.source.trim())
    } else {
        format!("session:{session}")
    }
}

fn retain_diverse_evidence(evidence: &mut Vec<CandidateEvidence>) {
    if evidence.len() <= MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE {
        return;
    }
    let mut representatives = std::collections::BTreeMap::<String, usize>::new();
    for (index, item) in evidence.iter().enumerate() {
        representatives
            .entry(evidence_bucket(item))
            .and_modify(|current| {
                let previous = &evidence[*current];
                if (item.verified && !previous.verified)
                    || (item.verified == previous.verified && index > *current)
                {
                    *current = index;
                }
            })
            .or_insert(index);
    }
    let mut preferred: Vec<usize> = representatives.into_values().collect();
    preferred.sort_by(|a, b| {
        evidence[*b]
            .verified
            .cmp(&evidence[*a].verified)
            .then_with(|| b.cmp(a))
    });
    preferred.truncate(MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE);
    let mut retained: std::collections::BTreeSet<usize> = preferred.into_iter().collect();
    for index in (0..evidence.len()).rev() {
        if retained.len() == MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE {
            break;
        }
        retained.insert(index);
    }
    let mut index = 0usize;
    evidence.retain(|_| {
        let keep = retained.contains(&index);
        index = index.saturating_add(1);
        keep
    });
}

fn merge_evidence(
    existing: &mut Vec<CandidateEvidence>,
    incoming: impl IntoIterator<Item = CandidateEvidence>,
) {
    let mut seen: std::collections::BTreeSet<(String, String, String, bool)> = existing
        .iter()
        .map(|evidence| {
            (
                evidence.session_id.clone(),
                evidence.source.clone(),
                evidence.detail.clone(),
                evidence.verified,
            )
        })
        .collect();
    for evidence in incoming {
        let key = (
            evidence.session_id.clone(),
            evidence.source.clone(),
            evidence.detail.clone(),
            evidence.verified,
        );
        if seen.insert(key) {
            existing.push(evidence);
        }
    }
    retain_diverse_evidence(existing);
}

fn merge_candidate(existing: &mut SelfImproveCandidate, incoming: SelfImproveCandidate) {
    existing.summary = incoming.summary;
    existing.kind = incoming.kind;
    existing.status = advance_candidate_status(existing.status, incoming.status);
    existing.first_observed_at_ms = match (
        existing.first_observed_at_ms,
        incoming.first_observed_at_ms,
    ) {
        (0, incoming) => incoming,
        (existing, 0) => existing,
        (existing, incoming) => existing.min(incoming),
    };
    existing.last_observed_at_ms = existing
        .last_observed_at_ms
        .max(incoming.last_observed_at_ms);
    merge_evidence(&mut existing.evidence, incoming.evidence);
}

fn merge_candidate_snapshot_lines(
    lines: Vec<String>,
    incoming: &mut SelfImproveCandidate,
) {
    let mut snapshot: Option<SelfImproveCandidate> = None;
    for line in lines {
        let Ok(record) = serde_json::from_str::<SelfImproveCandidateRecord>(&line) else {
            continue;
        };
        let mut candidate = normalize_legacy_candidate(record.candidate);
        if candidate.first_observed_at_ms == 0 {
            candidate.first_observed_at_ms = record.ts_ms;
        }
        candidate.last_observed_at_ms = candidate.last_observed_at_ms.max(record.ts_ms);
        if candidate.id != incoming.id {
            continue;
        }
        if let Some(existing) = &mut snapshot {
            merge_candidate(existing, candidate);
        } else {
            snapshot = Some(candidate);
        }
    }
    if let Some(mut existing) = snapshot {
        merge_candidate(&mut existing, incoming.clone());
        *incoming = existing;
    }
}

#[cfg(unix)]
fn record_self_improve_candidate_retained(
    store: &CandidateStoreLock,
    candidate: &SelfImproveCandidate,
) -> Result<(), DreamError> {
    let file_name = PathBuf::from(format!("{}.jsonl", safe_stem(&candidate.id)));
    let observed_at_ms = now_ms();
    let mut candidate = candidate.clone();
    candidate.first_observed_at_ms = if candidate.first_observed_at_ms == 0 {
        observed_at_ms
    } else {
        candidate.first_observed_at_ms.min(observed_at_ms)
    };
    candidate.last_observed_at_ms = observed_at_ms;
    retain_diverse_evidence(&mut candidate.evidence);
    let lines = match read_jsonl_lines_retained(&store.dir, &file_name) {
        Ok(lines) => lines,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(DreamError::Io(error)),
    };
    merge_candidate_snapshot_lines(lines, &mut candidate);
    let record = SelfImproveCandidateRecord {
        ts_ms: observed_at_ms,
        candidate,
    };
    let mut line =
        serde_json::to_string(&record).map_err(|e| DreamError::Io(std::io::Error::other(e)))?;
    line.push('\n');
    // The Applied status transition must be durable before callers clean up
    // their proposal receipts, so sync the JSONL file and its directory.
    append_jsonl_line_retained_durable(&store.dir, &file_name, &line)?;
    prune_jsonl_lines_retained(
        &store.dir,
        &file_name,
        MAX_SELF_IMPROVE_CANDIDATE_LINES,
    )?;
    prune_jsonl_files_retained(&store.dir, MAX_SELF_IMPROVE_CANDIDATE_FILES)?;
    Ok(())
}

/// Append one self-improvement candidate event under `.zo/dream/candidates/`.
///
/// This is the candidate-store sibling of [`record_observation`]. It does not
/// trigger planning, patching, or applying; it only records natural runtime
/// signals in an append-only log so a later, separately-gated runner can inspect
/// them. A candidate id maps to one JSONL file, so repeated signals accumulate
/// without overwriting earlier evidence.
pub fn record_self_improve_candidate(
    cwd: &Path,
    candidate: &SelfImproveCandidate,
) -> Result<(), DreamError> {
    if candidate.id.trim().is_empty() {
        return Ok(());
    }
    let dir = trace_base(cwd)
        .join(ZO_DIR_NAME)
        .join(DREAM_DIR)
        .join(SELF_IMPROVE_CANDIDATES_DIR);
    let store = lock_candidate_store(cwd, &dir)?;
    #[cfg(unix)]
    {
        record_self_improve_candidate_retained(&store, candidate)
    }
    #[cfg(not(unix))]
    {
        let _ = (store, candidate);
        Err(DreamError::Io(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "secure candidate mutation requires Unix retained directory handles",
        )))
    }
}

/// Mark a self-improve candidate `Applied` before removing its proposal, so a
/// persisted proposal remains as a recovery receipt if the terminal event cannot
/// be recorded. A successful patch must never silently leave its candidate
/// eligible for regeneration.
pub fn mark_self_improve_candidate_applied(
    cwd: &Path,
    candidate_id: &str,
) -> Result<(), DreamError> {
    mark_self_improve_candidate_terminal(cwd, candidate_id, CandidateStatus::Applied)
}

/// Retire a reviewed proposal's candidate after an explicit human rejection so
/// the same evidence does not immediately regenerate the declined patch.
pub fn mark_self_improve_candidate_rejected(
    cwd: &Path,
    candidate_id: &str,
) -> Result<(), DreamError> {
    mark_self_improve_candidate_terminal(cwd, candidate_id, CandidateStatus::Rejected)
}

fn mark_self_improve_candidate_terminal(
    cwd: &Path,
    candidate_id: &str,
    status: CandidateStatus,
) -> Result<(), DreamError> {
    let Some(mut candidate) = read_self_improve_candidates(cwd)
        .into_iter()
        .find(|c| c.id == candidate_id)
    else {
        return Err(io_error("self-improve candidate disappeared before completion"));
    };
    if !candidate.kind.is_actionable() {
        return Err(io_error("self-improve candidate is not actionable"));
    }
    candidate.status = status;
    record_self_improve_candidate(cwd, &candidate)
}

fn dream_automation_enabled_for_cwd(cwd: &Path) -> bool {
    crate::config::ConfigLoader::default_for(cwd)
        .load()
        .map(|config| config.dream_automation_enabled())
        .unwrap_or(false)
}

/// Convenience producer for natural self-improvement pulses. Returns `false`
/// when disabled, empty, or when the best-effort append fails.
// One pulse record carries this many distinct, unrelated metadata fields by
// design (provenance + classification + payload); bundling them into a struct
// would only move the argument list to the call sites across three crates.
#[allow(clippy::too_many_arguments)]
#[must_use]
pub fn record_self_improve_pulse_if_enabled(
    enabled: bool,
    cwd: &Path,
    kind: CandidateKind,
    session_id: &str,
    source: &str,
    summary: &str,
    detail: &str,
    verified: bool,
) -> bool {
    if !enabled || summary.trim().is_empty() {
        return false;
    }
    let candidate = SelfImproveCandidate::new(
        kind,
        summary.trim(),
        vec![CandidateEvidence {
            session_id: session_id.trim().to_string(),
            source: source.trim().to_string(),
            detail: truncate_detail(detail),
            verified,
        }],
    );
    record_self_improve_candidate(cwd, &candidate).is_ok()
}

/// Convenience producer for runtime components that do not own an already-loaded
/// feature config. It loads the workspace config rooted at `cwd` for each pulse,
/// so `autoDreamEnabled: false` remains a kill switch without process-global
/// mutable state that concurrent sessions could clobber.
#[must_use]
pub fn record_self_improve_pulse(
    cwd: &Path,
    kind: CandidateKind,
    session_id: &str,
    source: &str,
    summary: &str,
    detail: &str,
    verified: bool,
) -> bool {
    let enabled = dream_automation_enabled_for_cwd(cwd);
    record_self_improve_pulse_if_enabled(
        enabled, cwd, kind, session_id, source, summary, detail, verified,
    )
}

const LEGACY_CANCELLATION_SUMMARY: &str = "turn cancelled before normal completion";
const USER_CANCELLATION_SUMMARY: &str = "turn cancelled by user or host";
/// Fixed summary the pre-segmentation turn-failure recorder used for every
/// failure. Records carrying it are demoted to terminal on read (see
/// [`normalize_legacy_candidate`]) now that failures record per error
/// signature.
const LEGACY_GENERIC_FAILURE_SUMMARY: &str = "turn failed before normal completion";

fn has_explicit_legacy_user_cancel_origin(candidate: &SelfImproveCandidate) -> bool {
    candidate.evidence.iter().any(|evidence| {
        let source = evidence.source.to_ascii_lowercase();
        let detail = evidence.detail.to_ascii_lowercase();
        source == "user"
            || source == "user_cancel"
            || detail.contains("by user")
            || detail.contains("user abort")
            || detail.contains("ctrl+c")
            || detail.contains("cancelturn")
            || detail.contains("session.cancel_turn")
    })
}

fn normalize_legacy_candidate(mut candidate: SelfImproveCandidate) -> SelfImproveCandidate {
    let legacy_id = decision_core::dreamer::self_improve_candidate_id(
        CandidateKind::TurnFailure,
        LEGACY_CANCELLATION_SUMMARY,
    );
    if candidate.kind == CandidateKind::TurnFailure
        && (candidate.summary == LEGACY_CANCELLATION_SUMMARY || candidate.id == legacy_id)
        && has_explicit_legacy_user_cancel_origin(&candidate)
    {
        candidate.kind = CandidateKind::UserCancelled;
        candidate.status = CandidateStatus::Rejected;
        candidate.summary = USER_CANCELLATION_SUMMARY.to_string();
        candidate.id = decision_core::dreamer::self_improve_candidate_id(
            CandidateKind::UserCancelled,
            USER_CANCELLATION_SUMMARY,
        );
    }
    if candidate.kind == CandidateKind::GoalTerminal
        && candidate.evidence.iter().any(|evidence| {
            evidence.source.eq_ignore_ascii_case("goal")
                && evidence.detail.to_ascii_lowercase().contains("failed")
        })
    {
        // Old stores represented both successful and failed goals as
        // `GoalTerminal`. Preserve the lifecycle status while giving the failed
        // signal its actionable identity during read/coalescing.
        candidate.kind = CandidateKind::GoalFailure;
        candidate.id = decision_core::dreamer::self_improve_candidate_id(
            CandidateKind::GoalFailure,
            &candidate.summary,
        );
    }
    if candidate.kind == CandidateKind::TurnFailure
        && candidate.status == CandidateStatus::Proposed
        && (candidate.summary == LEGACY_GENERIC_FAILURE_SUMMARY
            || candidate.summary == LEGACY_CANCELLATION_SUMMARY
            || candidate.id
                == decision_core::dreamer::self_improve_candidate_id(
                    CandidateKind::TurnFailure,
                    LEGACY_GENERIC_FAILURE_SUMMARY,
                )
            || candidate.id
                == decision_core::dreamer::self_improve_candidate_id(
                    CandidateKind::TurnFailure,
                    LEGACY_CANCELLATION_SUMMARY,
                ))
    {
        // Pre-segmentation stores aggregated EVERY turn failure (and every
        // non-user cancellation) into one generic candidate per fixed summary.
        // Those blobs' mountains of mixed-cause evidence hit the session-count
        // score cap and would permanently outrank the per-signature candidates
        // that replaced them — fusion would keep proposing against an
        // unactionably vague aggregate forever. Demote them to terminal on
        // read; live producers now record segmented summaries ("turn failure:
        // <signature>", "turn failed because the host stopped consuming"), so
        // no current signal is lost. Only `Proposed` records demote, keeping
        // an `Applied` history entry's outcome. (Supersedes the earlier
        // "ambiguous legacy cancellation stays actionable" contract, which
        // predates signature segmentation.)
        candidate.status = CandidateStatus::Rejected;
    }
    candidate
}

fn coalesce_candidate_files(
    files: impl IntoIterator<Item = Vec<String>>,
) -> Vec<SelfImproveCandidate> {
    let mut by_id: std::collections::BTreeMap<String, SelfImproveCandidate> =
        std::collections::BTreeMap::new();
    for lines in files {
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Ok(record) = serde_json::from_str::<SelfImproveCandidateRecord>(line) else {
                continue;
            };
            let mut candidate = normalize_legacy_candidate(record.candidate);
            if candidate.first_observed_at_ms == 0 {
                candidate.first_observed_at_ms = record.ts_ms;
            }
            candidate.last_observed_at_ms = candidate.last_observed_at_ms.max(record.ts_ms);
            let id = candidate.id.clone();
            by_id
                .entry(id)
                .and_modify(|existing| merge_candidate(existing, candidate.clone()))
                .or_insert(candidate);
        }
    }
    decision_core::dreamer::rank_self_improve_candidates_at(
        &by_id.into_values().collect::<Vec<_>>(),
        now_ms(),
    )
}

/// Read and coalesce self-improvement candidates from append-only JSONL logs.
/// Malformed lines are skipped, matching the Dreamer lesson-source tolerance.
#[must_use]
pub fn read_self_improve_candidates(cwd: &Path) -> Vec<SelfImproveCandidate> {
    let dir = self_improve_candidates_dir(cwd);
    #[cfg(unix)]
    {
        let Ok(dir) = crate::secure_fs::RetainedDir::open(&dir) else {
            return Vec::new();
        };
        let Ok(paths) =
            jsonl_files_oldest_first_retained(&dir, MAX_SELF_IMPROVE_CANDIDATE_FILES)
        else {
            return Vec::new();
        };
        coalesce_candidate_files(
            paths
                .into_iter()
                .filter_map(|path| read_jsonl_lines_retained(&dir, &path).ok()),
        )
    }
    #[cfg(not(unix))]
    {
        coalesce_candidate_files(
            jsonl_files_oldest_first(&dir, MAX_SELF_IMPROVE_CANDIDATE_FILES)
                .into_iter()
                .filter_map(|path| read_jsonl_lines(&path).ok()),
        )
    }
}

fn default_advisor_findings(candidate: &SelfImproveCandidate) -> Vec<AdvisorFinding> {
    let risk = match candidate.kind {
        CandidateKind::PostTurn | CandidateKind::VerifiedAccept | CandidateKind::UserCancelled => {
            PatchRisk::Low
        }
        CandidateKind::GoalTerminal
        | CandidateKind::GoalFailure
        | CandidateKind::TurnFailure
        // A failing harness feature is repaired inside the harness plumbing
        // it names — the same blast radius a turn-failure repair has.
        | CandidateKind::HarnessDefect => PatchRisk::Medium,
    };
    let representative = decision_core::dreamer::representative_candidate_evidence(candidate, 3);
    let detail = if representative.is_empty() {
        String::from("no evidence detail")
    } else {
        representative
            .iter()
            .map(|evidence| evidence.detail.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    };
    vec![
        AdvisorFinding {
            role: AdvisorRole::RootCause,
            candidate_id: candidate.id.clone(),
            summary: format!(
                "Representative root cause signals: {}",
                truncate_detail(&detail)
            ),
            confidence: 0.6,
            risk,
            recommended_checks: Vec::new(),
            accepts_quarantine: true,
        },
        AdvisorFinding {
            role: AdvisorRole::Risk,
            candidate_id: candidate.id.clone(),
            summary: format!("Estimated patch risk is {risk}"),
            confidence: 0.6,
            risk,
            recommended_checks: Vec::new(),
            accepts_quarantine: risk != PatchRisk::High,
        },
        AdvisorFinding {
            role: AdvisorRole::TestPlan,
            candidate_id: candidate.id.clone(),
            summary: "Use the focused regression command attached to the candidate or run the Dreamer/runtime tests.".to_string(),
            confidence: 0.7,
            risk,
            recommended_checks: vec!["cargo test -p runtime dreamer --lib".to_string()],
            accepts_quarantine: true,
        },
        AdvisorFinding {
            role: AdvisorRole::AlternativeHypothesis,
            candidate_id: candidate.id.clone(),
            summary: "The signal may be environmental or flaky; require objective checks before apply.".to_string(),
            confidence: 0.5,
            risk,
            recommended_checks: Vec::new(),
            accepts_quarantine: true,
        },
    ]
}

/// Run native `DreamFusion` v0 over the highest-ranked self-improvement candidate.
///
/// This phase is intentionally read-only: it synthesizes advisor findings and
/// writes a JSON report under `.zo/dream/fusion/`, but never edits source,
/// creates patches, or applies changes.
pub fn run_dream_fusion_v0(
    cwd: &Path,
    run_id: &str,
) -> Result<Option<DreamFusionReport>, DreamError> {
    // Skip resolved candidates (Applied/Rejected) so `/improve` never regenerates
    // a patch it just acted on. Ranking still returns them (status inspection /
    // store semantics are unchanged); only proposal selection skips them.
    let Some(candidate) = read_self_improve_candidates(cwd)
        .into_iter()
        .find(|candidate| candidate.kind.is_actionable() && !candidate.status.is_terminal())
    else {
        return Ok(None);
    };
    let report = synthesize_dream_fusion(run_id, &candidate, default_advisor_findings(&candidate));
    write_dream_fusion_report(cwd, &report)?;
    Ok(Some(report))
}

pub fn write_dream_fusion_report(cwd: &Path, report: &DreamFusionReport) -> Result<(), DreamError> {
    let dir = ensure_repo_dream_child_dir_no_symlink(cwd, DREAM_FUSION_DIR)?;
    let payload =
        serde_json::to_string_pretty(report).map_err(|e| DreamError::Io(std::io::Error::other(e)))?;
    write_text_atomic(
        &trace_base(cwd),
        &dir.join(format!("{}.json", safe_stem(&report.run_id))),
        &payload,
    )?;
    Ok(())
}

/// Newest persisted `DreamFusion` report (by file mtime) with its timestamp,
/// or `None` when no report exists. Read-only — never creates `.zo/dream`
/// — so `/improve status` can surface the report the startup preflight (or a
/// prior `/improve`) generated without arming anything. Unreadable or
/// malformed report files are skipped, matching the Dreamer's tolerance for
/// its own append-only stores.
#[must_use]
pub fn latest_dream_fusion_report(
    cwd: &Path,
) -> Option<(DreamFusionReport, std::time::SystemTime)> {
    let dream_dir = existing_repo_dream_dir_no_symlink(cwd).ok()??;
    let fusion_dir = dream_dir.join(DREAM_FUSION_DIR);
    let metadata = std::fs::symlink_metadata(&fusion_dir).ok()?;
    if !metadata.file_type().is_dir() {
        return None;
    }
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&fusion_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        if newest
            .as_ref()
            .is_none_or(|(newest_modified, _)| modified > *newest_modified)
        {
            newest = Some((modified, path));
        }
    }
    let (modified, path) = newest?;
    let contents = std::fs::read_to_string(path).ok()?;
    let report = serde_json::from_str::<DreamFusionReport>(&contents).ok()?;
    Some((report, modified))
}

#[derive(Debug, Clone)]
pub struct QuarantineCheckCommand {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct QuarantinePatchRequest {
    pub run_id: String,
    pub candidate_id: String,
    pub patch_diff: String,
    pub allowed_paths: Vec<String>,
    /// Explicit host authorization to execute check commands after the patch's
    /// path/symlink gate passes. Generated code is otherwise never executed.
    pub checks_authorized: bool,
    pub checks: Vec<QuarantineCheckCommand>,
    pub risk: PatchRisk,
}

#[derive(Debug, Clone)]
pub struct ManualApplyGateRequest {
    pub approved_by_user: bool,
    pub run: QuarantinePatchRun,
    pub allowed_paths: Vec<String>,
    pub reviewer_accepted: bool,
}

fn io_error(message: impl Into<String>) -> DreamError {
    DreamError::Io(std::io::Error::other(message.into()))
}

/// Resolve Git from a canonical platform-owned location for every quarantine
/// and final-apply operation. This deliberately never consults ambient `PATH`.
pub fn trusted_git_binary(cwd: &Path) -> Result<PathBuf, DreamError> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        strict_check::trusted_git_binary(cwd).map_err(io_error)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = cwd;
        Err(io_error("trusted Git is unavailable on this platform"))
    }
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<std::process::Output, DreamError> {
    let git = trusted_git_binary(cwd)?;
    std::process::Command::new(git)
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map_err(DreamError::Io)
}

fn require_success(context: &str, output: &std::process::Output) -> Result<(), DreamError> {
    if output.status.success() {
        Ok(())
    } else {
        Err(io_error(format!(
            "{context} failed: {}",
            truncate_detail(&String::from_utf8_lossy(&output.stderr))
        )))
    }
}

fn current_head(cwd: &Path) -> Result<String, DreamError> {
    let output = run_git(cwd, &["rev-parse", "HEAD"])?;
    if !output.status.success() {
        return Err(io_error("git rev-parse HEAD failed"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_status_clean(cwd: &Path) -> bool {
    run_git(cwd, &["status", "--porcelain"])
        .map(|output| output.status.success() && output.stdout.is_empty())
        .unwrap_or(false)
}

fn safe_relative_path(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn path_matches_allowlist(path: &str, allowlist: &[String]) -> bool {
    safe_relative_path(path)
        && allowlist.iter().any(|allowed| {
            safe_relative_path(allowed)
                && (path == allowed
                    || path
                        .strip_prefix(allowed.as_str())
                        .is_some_and(|rest| rest.starts_with('/')))
        })
}

fn all_paths_allowed(paths: &[String], allowlist: &[String]) -> bool {
    !paths.is_empty()
        && paths
            .iter()
            .all(|path| path_matches_allowlist(path, allowlist))
}

fn path_has_existing_symlink(cwd: &Path, path: &str) -> bool {
    let mut current = cwd.to_path_buf();
    for component in Path::new(path).components() {
        let std::path::Component::Normal(part) = component else {
            return true;
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(_) => return true,
        }
    }
    false
}

fn no_changed_path_hits_symlink(cwd: &Path, paths: &[String]) -> bool {
    paths
        .iter()
        .all(|path| !path_has_existing_symlink(cwd, path))
}

fn changed_paths_from_nul(output: &[u8]) -> Result<Vec<String>, DreamError> {
    if !output.is_empty() && !output.ends_with(&[0]) {
        return Err(io_error("git changed-path output was not NUL terminated"));
    }
    let mut paths = Vec::new();
    for raw in output.split(|byte| *byte == 0).filter(|raw| !raw.is_empty()) {
        let path = std::str::from_utf8(raw)
            .map_err(|_| io_error("git changed path is not valid UTF-8"))?;
        if !safe_relative_path(path) {
            return Err(io_error("git reported an unsafe changed path"));
        }
        if !paths.iter().any(|existing| existing == path) {
            paths.push(path.to_string());
        }
    }
    Ok(paths)
}

fn run_check_command(
    worktree: &Path,
    check_state: &Path,
    check: &QuarantineCheckCommand,
) -> PatchCheckResult {
    strict_check::run(worktree, check_state, check)
}

fn verify_quarantine_after_check(
    worktree: &Path,
    expected_diff: &str,
    expected_paths: &[String],
) -> Result<(), DreamError> {
    let cached = run_git(worktree, &["diff", "--cached", "--binary", "HEAD"])?;
    require_success("post-check git diff --cached", &cached)?;
    let unstaged = run_git(worktree, &["diff", "--binary"])?;
    require_success("post-check git diff", &unstaged)?;
    let untracked = run_git(
        worktree,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    require_success("post-check git ls-files", &untracked)?;
    let paths = run_git(
        worktree,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "-z",
            "HEAD",
        ],
    )?;
    require_success("post-check git diff --name-only", &paths)?;
    if cached.stdout != expected_diff.as_bytes()
        || !unstaged.stdout.is_empty()
        || !untracked.stdout.is_empty()
        || changed_paths_from_nul(&paths.stdout)? != expected_paths
    {
        return Err(io_error(
            "quarantine source or index changed while checks were running",
        ));
    }
    Ok(())
}

fn remove_dir_all_if_present(path: &Path) -> Result<(), DreamError> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DreamError::Io(error)),
    }
}

fn finish_with_cleanup<T>(
    result: Result<T, DreamError>,
    cleanup: Result<(), DreamError>,
    cleanup_context: &str,
) -> Result<T, DreamError> {
    match (result, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup_error)) => Err(io_error(format!(
            "{error}; {cleanup_context} also failed: {cleanup_error}"
        ))),
    }
}

fn create_quarantine_worktree_parent(cwd: &Path) -> Result<tempfile::TempDir, DreamError> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("zo-quarantine-worktree-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    let parent = builder.tempdir().map_err(DreamError::Io)?;
    let cwd = fs::canonicalize(cwd)?;
    let parent_path = fs::canonicalize(parent.path())?;
    if parent_path.starts_with(&cwd) {
        return finish_with_cleanup(
            Err(io_error(
                "quarantine worktree directory must be outside the repository",
            )),
            parent.close().map_err(DreamError::Io),
            "external quarantine worktree directory cleanup",
        );
    }
    Ok(parent)
}

fn verify_quarantine_worktree_unregistered(cwd: &Path, worktree: &Path) -> Result<(), DreamError> {
    if is_quarantine_worktree_registered(cwd, worktree)? {
        Err(io_error("quarantine worktree remains registered after cleanup"))
    } else {
        Ok(())
    }
}

fn prune_quarantine_worktree_registration(cwd: &Path, worktree: &Path) -> Result<(), DreamError> {
    let output = run_git(cwd, &["worktree", "prune"])?;
    require_success("git worktree prune", &output)?;
    verify_quarantine_worktree_unregistered(cwd, worktree)
}

fn prune_stale_quarantine_worktree_registrations(cwd: &Path) {
    let result = run_git(cwd, &["worktree", "prune"])
        .and_then(|output| require_success("git worktree prune", &output));
    if let Err(error) = result {
        eprintln!("[zo] stale quarantine worktree prune failed: {error}");
    }
}

fn remove_quarantine_worktree(cwd: &Path, worktree: &Path) -> Result<(), DreamError> {
    let remove_result = run_git(
        cwd,
        &[
            "worktree",
            "remove",
            "--force",
            worktree.to_string_lossy().as_ref(),
        ],
    );
    match remove_result {
        Ok(output) if output.status.success() => verify_quarantine_worktree_unregistered(cwd, worktree),
        Ok(output) => {
            let primary = io_error(format!(
                "git worktree remove failed: {}",
                truncate_detail(&String::from_utf8_lossy(&output.stderr))
            ));
            let cleanup = finish_with_cleanup(
                remove_dir_all_if_present(worktree),
                prune_quarantine_worktree_registration(cwd, worktree),
                "quarantine worktree registration cleanup",
            );
            finish_with_cleanup(Err(primary), cleanup, "quarantine worktree fallback cleanup")
        }
        Err(error) => {
            let cleanup = finish_with_cleanup(
                remove_dir_all_if_present(worktree),
                prune_quarantine_worktree_registration(cwd, worktree),
                "quarantine worktree registration cleanup",
            );
            finish_with_cleanup(Err(error), cleanup, "quarantine worktree fallback cleanup")
        }
    }
}

fn finish_quarantine_run(
    run_result: Result<QuarantinePatchRun, DreamError>,
    cleanup_result: Result<(), DreamError>,
) -> Result<QuarantinePatchRun, DreamError> {
    finish_with_cleanup(
        run_result,
        cleanup_result,
        "quarantine worktree cleanup",
    )
}

fn prune_quarantine_runs(
    quarantine_root: &Path,
    current_run: &str,
) -> std::io::Result<()> {
    let mut runs = Vec::new();
    for entry in fs::read_dir(quarantine_root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "quarantine run path is a symlink",
            ));
        }
        if !file_type.is_dir() || entry.file_name() == current_run {
            continue;
        }
        let modified = entry.metadata()?.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        runs.push((modified, entry.path()));
    }
    runs.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    for (_, path) in runs
        .into_iter()
        .skip(MAX_QUARANTINE_RUNS.saturating_sub(1))
    {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[cfg(test)]
fn quarantine_dir(cwd: &Path) -> PathBuf {
    dream_dir(cwd).join(DREAM_QUARANTINE_DIR)
}

fn quarantine_storage_id(run_id: &str) -> String {
    const STEM_MAX_CHARS: usize = 64;
    let stem = safe_stem(run_id)
        .to_ascii_lowercase()
        .chars()
        .take(STEM_MAX_CHARS)
        .collect::<String>();
    format!("{stem}-{:x}", Sha256::digest(run_id.as_bytes()))
}

#[cfg(test)]
fn quarantine_run_dir(cwd: &Path, run_id: &str) -> PathBuf {
    quarantine_dir(cwd).join(run_id)
}

struct QuarantineRunDir {
    path: PathBuf,
}

impl QuarantineRunDir {
    fn create(quarantine_root: &Path, run_id: &str) -> Result<Self, DreamError> {
        let path = quarantine_root.join(run_id);
        #[cfg(unix)]
        {
            let root = crate::secure_fs::RetainedDir::open(quarantine_root)?;
            root.create_private_subdir_new(Path::new(run_id))?;
        }
        #[cfg(not(unix))]
        {
            fs::create_dir(&path)?;
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
                return Err(io_error("quarantine run directory is not a real directory"));
            }
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn cleanup(&self) -> Result<(), DreamError> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata.file_type().is_dir() => remove_dir_all_if_present(&self.path),
            Ok(_) => Err(io_error("owned quarantine run directory is not a real directory")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DreamError::Io(error)),
        }
    }
}

fn is_quarantine_worktree_registered(cwd: &Path, worktree: &Path) -> Result<bool, DreamError> {
    let output = run_git(cwd, &["worktree", "list", "--porcelain"])?;
    require_success("git worktree list", &output)?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .any(|registered| Path::new(registered) == worktree))
}

fn remove_quarantine_worktree_if_registered(
    cwd: &Path,
    worktree: &Path,
) -> Result<(), DreamError> {
    if is_quarantine_worktree_registered(cwd, worktree)? {
        remove_quarantine_worktree(cwd, worktree)
    } else {
        Ok(())
    }
}

fn execute_quarantine_patch(
    cwd: &Path,
    request: &QuarantinePatchRequest,
    run_id: String,
    run_dir: &Path,
    base_commit: String,
    worktree: &Path,
) -> Result<QuarantinePatchRun, DreamError> {
    let patch_input = run_dir.join("proposal.diff");
    write_text_atomic(&trace_base(cwd), &patch_input, &request.patch_diff)?;
    if !request.patch_diff.trim().is_empty() {
        require_success(
            "git apply",
            &run_git(
                worktree,
                &[
                    "apply",
                    "--index",
                    "--whitespace=nowarn",
                    patch_input.to_string_lossy().as_ref(),
                ],
            )?,
        )?;
    }

    let diff_output = run_git(worktree, &["diff", "--cached", "--binary", "HEAD"])?;
    require_success("git diff --cached --binary", &diff_output)?;
    let final_diff = String::from_utf8_lossy(&diff_output.stdout).to_string();
    let path_output = run_git(
        worktree,
        &[
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "-z",
            "HEAD",
        ],
    )?;
    require_success("git diff --cached --name-only", &path_output)?;
    let changed_paths = changed_paths_from_nul(&path_output.stdout)?;
    if !all_paths_allowed(&changed_paths, &request.allowed_paths)
        || !no_changed_path_hits_symlink(cwd, &changed_paths)
        || !no_changed_path_hits_symlink(worktree, &changed_paths)
    {
        return Err(io_error(
            "quarantine patch touches a disallowed or symlinked path",
        ));
    }
    if !request.checks.is_empty() && !request.checks_authorized {
        return Err(io_error(
            "quarantine checks require explicit execution authorization",
        ));
    }
    let check_state_root = ensure_child_dir_no_symlink(run_dir, "check-state")?;
    let mut check_results = Vec::with_capacity(request.checks.len());
    for (index, check) in request.checks.iter().enumerate() {
        let check_state = check_state_root.join(format!("check-{index}"));
        let result = run_check_command(worktree, &check_state, check);
        verify_quarantine_after_check(worktree, &final_diff, &changed_paths)?;
        check_results.push(result);
    }
    fs::remove_dir(&check_state_root)?;
    write_text_atomic(&trace_base(cwd), &run_dir.join("patch.diff"), &final_diff)?;
    write_text_atomic(
        &trace_base(cwd),
        &run_dir.join("checks.json"),
        &serde_json::to_string_pretty(&check_results)
            .map_err(|e| DreamError::Io(std::io::Error::other(e)))?,
    )?;
    let run = QuarantinePatchRun {
        run_id,
        candidate_id: request.candidate_id.clone(),
        base_commit,
        patch_digest: format!("{:x}", Sha256::digest(request.patch_diff.as_bytes())),
        changed_paths,
        check_results,
        risk: request.risk,
    };
    write_text_atomic(
        &trace_base(cwd),
        &run_dir.join("metadata.json"),
        &serde_json::to_string_pretty(&run)
            .map_err(|e| DreamError::Io(std::io::Error::other(e)))?,
    )?;
    Ok(run)
}

/// Create a temporary git worktree, apply a proposed diff there, run bounded
/// check commands, and save all artifacts under `.zo/dream/quarantine`.
/// The main worktree is never patched or applied to by this function.
pub fn run_quarantine_patch(
    cwd: &Path,
    request: &QuarantinePatchRequest,
) -> Result<QuarantinePatchRun, DreamError> {
    prune_stale_quarantine_worktree_registrations(cwd);
    let base_commit = current_head(cwd)?;
    let quarantine_root =
        ensure_repo_dream_child_dir_no_symlink(cwd, DREAM_QUARANTINE_DIR)?;
    let run_id = quarantine_storage_id(&request.run_id);
    let run_dir = QuarantineRunDir::create(&quarantine_root, &run_id)?;
    let worktree_parent = match create_quarantine_worktree_parent(cwd) {
        Ok(parent) => parent,
        Err(error) => {
            return finish_with_cleanup(
                Err(error),
                run_dir.cleanup(),
                "quarantine run artifact cleanup",
            );
        }
    };
    let worktree = worktree_parent.path().join("worktree");
    let add_result = run_git(
        cwd,
        &[
            "worktree",
            "add",
            "--detach",
            worktree.to_string_lossy().as_ref(),
            &base_commit,
        ],
    )
    .and_then(|output| {
        require_success("git worktree add", &output)?;
        Ok(())
    });
    if let Err(error) = add_result {
        let setup_cleanup = finish_with_cleanup(
            remove_quarantine_worktree_if_registered(cwd, &worktree),
            worktree_parent.close().map_err(DreamError::Io),
            "external quarantine worktree directory cleanup",
        );
        let setup_cleanup = finish_with_cleanup(
            setup_cleanup,
            run_dir.cleanup(),
            "quarantine setup artifact cleanup",
        );
        return finish_with_cleanup(Err(error), setup_cleanup, "quarantine setup cleanup");
    }

    let run_result = execute_quarantine_patch(
        cwd,
        request,
        run_id,
        run_dir.path(),
        base_commit,
        &worktree,
    );
    let cleanup_result = finish_with_cleanup(
        remove_quarantine_worktree(cwd, &worktree),
        worktree_parent.close().map_err(DreamError::Io),
        "external quarantine worktree directory cleanup",
    );
    let result = finish_quarantine_run(run_result, cleanup_result);
    match result {
        Ok(run) => {
            prune_quarantine_runs(&quarantine_root, &run.run_id)?;
            Ok(run)
        }
        Err(error) => finish_with_cleanup(
            Err(error),
            run_dir.cleanup(),
            "quarantine run artifact cleanup",
        ),
    }
}

#[must_use]
pub fn evaluate_manual_apply_gate(
    cwd: &Path,
    request: &ManualApplyGateRequest,
) -> ApplyGateDecision {
    let current = current_head(cwd).ok();
    let input = ApplyGateInput {
        approved_by_user: request.approved_by_user,
        clean_tree: git_status_clean(cwd),
        base_commit_matches: current.as_deref() == Some(request.run.base_commit.as_str()),
        paths_allowed: all_paths_allowed(&request.run.changed_paths, &request.allowed_paths)
            && no_changed_path_hits_symlink(cwd, &request.run.changed_paths),
        focused_checks_green: !request.run.check_results.is_empty()
            && request.run.check_results.iter().all(|check| check.success),
        reviewer_accepted: request.reviewer_accepted,
        risk: request.run.risk,
    };
    decide_apply_gate(&input)
}

/// Record that the self-improve scheduler attempted a pass. Attempt markers are
/// separate from success markers so repeated failures back off instead of
/// storming every turn/startup.
pub fn record_self_improve_attempt(cwd: &Path) -> Result<(), DreamError> {
    let dir = ensure_repo_dream_dir_no_symlink(cwd)?;
    write_marker(cwd, &dir.join(SELF_IMPROVE_ATTEMPT_MARKER))
}

/// Best-effort observability for automatic self-improve failures.
pub fn record_self_improve_failure(cwd: &Path, error: &DreamError) -> Result<(), DreamError> {
    let dir = ensure_repo_dream_dir_no_symlink(cwd)?;
    let payload = serde_json::json!({
        "tsMs": now_ms(),
        "error": dream_error_signature(error),
    });
    let bytes = serde_json::to_vec_pretty(&payload)
        .map_err(|e| DreamError::Io(std::io::Error::other(e)))?;
    write_text_atomic(
        &trace_base(cwd),
        &dir.join(SELF_IMPROVE_ERROR_FILE),
        &String::from_utf8_lossy(&bytes),
    )?;
    Ok(())
}

/// Last observed self-improve scheduler markers. Missing markers mean the
/// scheduler has not attempted/faulted yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SelfImproveScheduleState {
    pub last_attempt: Option<SystemTime>,
    pub last_failure: Option<SystemTime>,
}

fn marker_mtime(path: &Path) -> Result<Option<SystemTime>, DreamError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                return Err(DreamError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "self-improve marker is not a regular file",
                )));
            }
            metadata.modified().map(Some).map_err(DreamError::Io)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DreamError::Io(error)),
    }
}

/// Read marker mtimes used by the automatic self-improve preflight scheduler.
pub fn read_self_improve_schedule_state(cwd: &Path) -> Result<SelfImproveScheduleState, DreamError> {
    let dream_dir = ensure_repo_dream_dir_no_symlink(cwd)?;
    Ok(SelfImproveScheduleState {
        last_attempt: marker_mtime(&dream_dir.join(SELF_IMPROVE_ATTEMPT_MARKER))?,
        last_failure: marker_mtime(&dream_dir.join(SELF_IMPROVE_ERROR_FILE))?,
    })
}

/// Read marker mtimes for display/status without creating `.zo/dream`.
pub fn read_self_improve_schedule_state_readonly(
    cwd: &Path,
) -> Result<SelfImproveScheduleState, DreamError> {
    let Some(dream_dir) = existing_repo_dream_dir_no_symlink(cwd)? else {
        return Ok(SelfImproveScheduleState::default());
    };
    Ok(SelfImproveScheduleState {
        last_attempt: marker_mtime(&dream_dir.join(SELF_IMPROVE_ATTEMPT_MARKER))?,
        last_failure: marker_mtime(&dream_dir.join(SELF_IMPROVE_ERROR_FILE))?,
    })
}

/// Pure scheduler/backoff gate for the automatic self-improve preflight runner.
#[must_use]
pub fn should_run_self_improve(
    last_attempt: Option<std::time::SystemTime>,
    last_failure: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
    min_interval: std::time::Duration,
    failure_backoff: std::time::Duration,
) -> bool {
    if last_attempt.is_some_and(|last| {
        now.duration_since(last)
            .map_or(true, |gap| gap < min_interval)
    }) {
        return false;
    }
    if last_failure.is_some_and(|last| {
        now.duration_since(last)
            .map_or(true, |gap| gap < failure_backoff)
    }) {
        return false;
    }
    true
}

/// RAII lock for one workspace's self-improve transaction. The underlying
/// capability retains the no-follow parent directory handle through release.
pub struct SelfImproveLock {
    _inner: crate::secure_fs::ExclusiveFileLock,
}

pub fn try_acquire_self_improve_lock(cwd: &Path) -> Result<Option<SelfImproveLock>, DreamError> {
    ensure_repo_dream_dir_no_symlink(cwd)?;
    let relative = Path::new(ZO_DIR_NAME)
        .join(DREAM_DIR)
        .join(SELF_IMPROVE_LOCK_FILE);
    crate::secure_fs::try_lock_owner_only(&trace_base(cwd), &relative)
        .map(|lock| lock.map(|inner| SelfImproveLock { _inner: inner }))
        .map_err(DreamError::Io)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserPatternRecord {
    ts_ms: u64,
    observation: LessonObservation,
}

fn user_pattern_dir(cwd: &Path) -> PathBuf {
    crate::memory::paths::global_project_memory_dir(cwd, USER_PATTERN_DIR)
}

fn ensure_user_pattern_dir(cwd: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let config_home = crate::default_config_home();
    let parent = config_home.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config home has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let name = config_home.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config home has no directory name",
        )
    })?;
    crate::secure_fs::ensure_private_dir(parent, Path::new(name))?;
    let dir = user_pattern_dir(cwd);
    let relative = dir.strip_prefix(&config_home).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "user-pattern path escaped config home",
        )
    })?;
    crate::secure_fs::ensure_private_dir(&config_home, relative)?;
    Ok((config_home, dir))
}

fn user_pattern_signature(summary: &str) -> String {
    format!("user-pattern:{:016x}", dreamer_owned_body_hash(summary))
}

fn user_pattern_text_allowed(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && text.len() <= USER_PATTERN_SUMMARY_MAX_BYTES
        && !text.contains('\n')
        && !text.contains('\r')
        && !text.chars().any(|ch| ch.is_control() && !ch.is_whitespace())
}

fn user_pattern_record_is_valid(record: &UserPatternRecord) -> bool {
    let obs = &record.observation;
    obs.verified
        && obs.kind == LessonKind::Preference
        && !obs.session_id.trim().is_empty()
        && user_pattern_text_allowed(&obs.session_id)
        && user_pattern_text_allowed(&obs.summary)
        && obs.lesson == obs.summary
        && obs.signature == user_pattern_signature(obs.summary.trim())
}

#[must_use]
pub fn record_user_pattern_observation(
    cwd: &Path,
    session_id: &str,
    summary: &str,
    verified: bool,
) -> bool {
    let summary = summary.trim();
    if !verified || !user_pattern_text_allowed(summary) {
        return false;
    }
    let observation = LessonObservation {
        session_id: session_id.trim().to_string(),
        signature: user_pattern_signature(summary),
        lesson: summary.to_string(),
        summary: summary.to_string(),
        kind: LessonKind::Preference,
        verified: true,
    };
    let record = UserPatternRecord { ts_ms: now_ms(), observation };
    let Ok((config_home, dir)) = ensure_user_pattern_dir(cwd) else {
        return false;
    };
    let path = dir.join(USER_PATTERN_FILE);
    let Ok(mut line) = serde_json::to_string(&record) else {
        return false;
    };
    line.push('\n');
    append_jsonl_line_no_symlink(&config_home, &path, &line).is_ok()
}

/// Reads candidate observations from append-only JSONL logs under
/// `.zo/dream/`. Each non-empty line is one JSON [`LessonObservation`];
/// unparseable lines are skipped so a single corrupt write never blocks a run.
pub struct JsonlLessonSource {
    dir: PathBuf,
}

impl JsonlLessonSource {
    /// Source rooted at `<cwd>/.zo/dream/`.
    #[must_use]
    pub fn at_cwd(cwd: &Path) -> Self {
        Self {
            dir: trace_base(cwd).join(ZO_DIR_NAME).join(DREAM_DIR),
        }
    }
}

impl LessonSource for JsonlLessonSource {
    fn observations(&self) -> Vec<LessonObservation> {
        let mut out = VecDeque::new();
        for path in jsonl_files_oldest_first(&self.dir, MAX_DREAM_JSONL_FILES) {
            let Ok(lines) = read_jsonl_lines(&path) else {
                continue;
            };
            for line in lines {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(obs) = serde_json::from_str::<LessonObservation>(line) {
                    push_capped(&mut out, obs, MAX_DREAM_OBSERVATIONS);
                }
            }
        }
        out.into_iter().collect()
    }
}

/// Reads candidate observations from the externalized turn trace under
/// `.zo/turns/` (written by `crate::turn_trace`), distilling recurring
/// tool-failure gotchas via the pure [`lessons_from_turns`] brain. This is the
/// second producer the doc's §12-2 calls for ("a coding agent remembers
/// recurring failures"): it complements [`JsonlLessonSource`]'s green-accept
/// workflow lessons, so the Dreamer mines friction *and* success signals.
///
/// [`lessons_from_turns`]: decision_core::dreamer::lessons_from_turns
pub struct TurnLogLessonSource {
    cwd: PathBuf,
}

impl TurnLogLessonSource {
    /// Source rooted at `<cwd>/.zo/turns/`.
    #[must_use]
    pub fn at_cwd(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }
}

impl LessonSource for TurnLogLessonSource {
    fn observations(&self) -> Vec<LessonObservation> {
        let digests = crate::turn_trace::read_all_digests(&self.cwd);
        decision_core::dreamer::lessons_from_turns(&digests)
    }
}

/// Reads verified, distilled user-pattern observations. This source never mines
/// raw prompts; only callers that already distilled and verified a stable
/// preference can append records via [`record_user_pattern_observation`].
pub struct UserPatternLessonSource {
    cwd: PathBuf,
}

impl UserPatternLessonSource {
    #[must_use]
    pub fn at_cwd(cwd: &Path) -> Self {
        Self { cwd: cwd.to_path_buf() }
    }
}

impl LessonSource for UserPatternLessonSource {
    fn observations(&self) -> Vec<LessonObservation> {
        let dir = user_pattern_dir(&self.cwd);
        let mut out = VecDeque::new();
        for path in jsonl_files_oldest_first(&dir, MAX_DREAM_JSONL_FILES) {
            let Ok(lines) = read_jsonl_lines(&path) else {
                continue;
            };
            for line in lines {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(record) = serde_json::from_str::<UserPatternRecord>(line) {
                    if user_pattern_record_is_valid(&record) {
                        push_capped(&mut out, record.observation, MAX_DREAM_OBSERVATIONS);
                    }
                }
            }
        }
        out.into_iter().collect()
    }
}

/// Reads goal/loop automation events and distills them into Dreamer observations.
pub struct AutomationLessonSource {
    cwd: PathBuf,
}

impl AutomationLessonSource {
    #[must_use]
    pub fn at_cwd(cwd: &Path) -> Self {
        Self {
            cwd: cwd.to_path_buf(),
        }
    }
}

impl LessonSource for AutomationLessonSource {
    fn observations(&self) -> Vec<LessonObservation> {
        let dir = trace_base(&self.cwd)
            .join(ZO_DIR_NAME)
            .join(DREAM_DIR)
            .join(AUTOMATION_DIR);
        let mut digests = VecDeque::new();
        for path in jsonl_files_oldest_first(&dir, MAX_DREAM_JSONL_FILES) {
            let Ok(lines) = read_jsonl_lines(&path) else {
                continue;
            };
            for line in lines {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(record) = serde_json::from_str::<AutomationEventRecord>(line) {
                    push_capped(
                        &mut digests,
                        AutomationDigest {
                            session_id: record.session_id,
                            kind: record.kind,
                            event: record.event,
                            verified: record.verified,
                        },
                        MAX_AUTOMATION_DIGESTS,
                    );
                }
            }
        }
        decision_core::dreamer::lessons_from_automation(&digests.into_iter().collect::<Vec<_>>())
    }
}

/// Merges several [`LessonSource`]s into one, concatenating their observations.
/// Lets the Dreamer mine every signal stream (green-accept workflow lessons +
/// turn-trace failure gotchas + any future producer) through a single
/// [`Dreamer`] without the orchestrator knowing how many sources exist — the
/// curation gate dedups and ranks the merged pool regardless of origin.
pub struct CompositeLessonSource {
    sources: Vec<Box<dyn LessonSource>>,
}

impl CompositeLessonSource {
    /// Build a composite over a set of boxed sources.
    #[must_use]
    pub fn new(sources: Vec<Box<dyn LessonSource>>) -> Self {
        Self { sources }
    }
}

impl LessonSource for CompositeLessonSource {
    fn observations(&self) -> Vec<LessonObservation> {
        self.sources
            .iter()
            .flat_map(|source| source.observations())
            .collect()
    }
}

/// Append one candidate observation to a session's JSONL log under
/// `.zo/dream/`. This is the *producer* side of the loop, symmetric to
/// [`JsonlLessonSource`]: sessions call it during a run to record a noteworthy
/// outcome (a preference confirmed, a gotcha hit, a verified workflow), and the
/// Dreamer later decides — across sessions — which of those become memory.
///
/// Append-only and crash-safe: each call writes exactly one JSON line, so a
/// concurrent or interrupted session can never corrupt earlier records. The log
/// is named per `session_id` so distinct sessions stay distinguishable (which
/// is what the cross-session repetition count relies on).
///
/// # Errors
/// Returns [`DreamError::Io`] if the `.zo/dream/` directory cannot be created
/// or the line cannot be appended.
pub fn record_observation(cwd: &Path, observation: &LessonObservation) -> Result<(), DreamError> {
    let dir = ensure_repo_dream_dir_no_symlink(cwd)?;
    // Sanitise the session id into a safe filename stem; the id also lives
    // inside the record, so this only affects the file name.
    let stem = safe_stem(&observation.session_id);
    let path = dir.join(format!("{stem}.jsonl"));
    let mut line =
        serde_json::to_string(observation).map_err(|e| DreamError::Io(std::io::Error::other(e)))?;
    line.push('\n');
    append_jsonl_line_no_symlink(cwd, &path, &line)?;
    prune_jsonl_lines(&path, MAX_DREAM_JSONL_LINES)?;
    prune_jsonl_files(&dir, MAX_DREAM_JSONL_FILES)?;
    Ok(())
}

/// Build the candidate lesson for a change that was *accepted only after the
/// project's objective check command ran green* (the deep-lane verifier gate).
///
/// This is the cleanest automatic signal in the loop: it is grounded in a real
/// green run, so it can never be a hallucinated "lesson", and the canonical
/// verification command for a project naturally recurs across sessions — which
/// is exactly the cross-session repetition the promotion gate counts. The lesson
/// kind is [`LessonKind::Workflow`] and `verified` is always `true`, so a single
/// green accept never promotes alone: it must be seen in
/// [`PromotionPolicy::min_distinct_sessions`] distinct sessions first.
///
/// Returns `None` when there is no objective command to anchor the lesson (an
/// accept with no check command is not a *verified* outcome, so it is not
/// recorded — honesty over volume).
#[must_use]
pub fn verified_check_observation(
    session_id: &str,
    check_command: Option<&str>,
) -> Option<LessonObservation> {
    let command = check_command.map(str::trim).filter(|c| !c.is_empty())?;
    Some(LessonObservation {
        // Signature is the command alone, so the same project check dedups across
        // sessions regardless of which task triggered it.
        signature: format!("verified check command: {command}"),
        session_id: session_id.to_string(),
        lesson: format!(
            "`{command}` is this project's objective verification gate: deliberate \
             changes were accepted only after it ran green. Run it to verify edits \
             before considering a change complete."
        ),
        summary: format!("Verify changes in this project with `{command}`"),
        kind: LessonKind::Workflow,
        verified: true,
    })
}

/// Record a deep-lane *green accept* as a candidate workflow lesson, if it
/// carries an objective check command. Best-effort and side-effect-only: this is
/// the single call the live verifier gate makes when a change is accepted after
/// its check ran green. Combines [`verified_check_observation`] (pure, decides
/// *whether* there is a lesson) with [`record_observation`] (the append), so the
/// gate stays a one-liner and the decision logic stays unit-tested.
///
/// Errors are intentionally swallowed: recording a candidate must never fail or
/// slow a turn. A `false` return means nothing was recorded (no check command or
/// the append failed); callers ignore it.
#[must_use]
pub fn record_verified_check(cwd: &Path, session_id: &str, check_command: Option<&str>) -> bool {
    let Some(observation) = verified_check_observation(session_id, check_command) else {
        return false;
    };
    record_observation(cwd, &observation).is_ok()
}

/// Writes promoted lessons into the global per-project memory store, byte-for-byte
/// compatible with the `MemoryWrite` tool (same entry-file body trailing
/// newline, same `MEMORY.md` header and pointer format) so dreamed and
/// hand-written memory are interchangeable on recall.
pub struct FsMemoryStore {
    memory_dir: PathBuf,
}

impl FsMemoryStore {
    /// Store rooted at the global per-project memory directory for `cwd`.
    #[must_use]
    pub fn at_cwd(cwd: &Path) -> Self {
        Self {
            memory_dir: crate::memory::paths::memory_write_dir(cwd, false),
        }
    }

    /// Store for automatic curation. It persists in `memory.local/` so a
    /// compaction survives the current session and a restart, while the global
    /// store remains an explicit promotion target. Global slugs are exposed as
    /// existing so the Dreamer cannot produce a local override for one.
    #[must_use]
    pub fn local_at_cwd(cwd: &Path) -> Self {
        Self {
            memory_dir: crate::memory::paths::memory_write_dir(cwd, true),
        }
    }

    /// Archive every dreamer-promoted entry whose revisit window has elapsed as
    /// of `now_secs`, removing its `MEMORY.md` pointer so recall stops surfacing
    /// it. Hand-written entries and un-stamped dreamer entries are untouched.
    fn decay_expired(&self, now_secs: u64) -> Result<Vec<String>, DreamError> {
        self.decay_expired_with_before_marker_validation(now_secs, || {})
    }

    fn decay_expired_with_before_marker_validation<F>(
        &self,
        now_secs: u64,
        before_marker_validation: F,
    ) -> Result<Vec<String>, DreamError>
    where
        F: FnMut(),
    {
        self.decay_expired_with_lock_hooks(now_secs, || {}, before_marker_validation)
    }

    fn decay_expired_with_lock_hooks<P, F>(
        &self,
        now_secs: u64,
        on_process_contention: P,
        mut before_marker_validation: F,
    ) -> Result<Vec<String>, DreamError>
    where
        P: FnMut(),
        F: FnMut(),
    {
        let lock = acquire_memory_store_lock_with_retry(
            &self.memory_dir,
            MEMORY_STORE_LOCK_RETRY_COUNT,
            MEMORY_STORE_LOCK_RETRY_DELAY,
            on_process_contention,
            |_| {},
        )?;
        #[cfg(unix)]
        {
            recover_memory_write_journal_retained(&lock.dir)?;
            recover_memory_decay_journal_retained(&lock.dir)?;
            let Ok(entry_names) = lock.dir.entry_names() else {
                return Ok(Vec::new());
            };
            let mut archived = Vec::new();
            for file_name in entry_names {
                let path = Path::new(&file_name);
                if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                    continue;
                }
                let Some(slug) = path.file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                if !crate::memory::curation::is_safe_memory_slug(slug) {
                    continue;
                }
                let Ok(mut entry) = lock.dir.open_regular_file(path) else {
                    continue;
                };
                let Ok(body) = entry.read_to_string() else {
                    continue;
                };
                before_marker_validation();
                if !entry_is_decay_archive_candidate(&lock.dir, slug, &body, now_secs) {
                    continue;
                }
                archive_decay_entry_retained(&lock.dir, slug, &body)?;
                archived.push(slug.to_string());
            }
            Ok(archived)
        }
        #[cfg(not(unix))]
        {
            let _ = (&lock, &mut before_marker_validation, now_secs);
            recover_memory_write_journal(&self.memory_dir)?;
            Ok(Vec::new())
        }
    }
}

/// A shared lock for all writers of one global memory store. Both `MemoryWrite`
/// and Dreamer use it, so entry, ownership marker, and index updates cannot
/// interleave with another writer in this process or another Zo process.
pub struct MemoryStoreLock {
    _process: std::sync::MutexGuard<'static, ()>,
    _file: crate::secure_fs::ExclusiveFileLock,
    #[cfg(unix)]
    dir: RetainedDir,
}

fn acquire_memory_store_lock(memory_dir: &Path) -> Result<MemoryStoreLock, DreamError> {
    acquire_memory_store_lock_with_retry(
        memory_dir,
        MEMORY_STORE_LOCK_RETRY_COUNT,
        MEMORY_STORE_LOCK_RETRY_DELAY,
        || {},
        |_| {},
    )
}

fn acquire_memory_store_lock_with_retry<P, F>(
    memory_dir: &Path,
    retry_count: usize,
    retry_delay: std::time::Duration,
    mut on_process_contention: P,
    mut on_file_contention: F,
) -> Result<MemoryStoreLock, DreamError>
where
    P: FnMut(),
    F: FnMut(usize),
{
    // Directory creation is part of the same critical section as the advisory
    // lock. Otherwise two local writers can both initialize the directory before
    // either reaches the process mutex.
    let process = match MEMORY_STORE_PROCESS_LOCK.try_lock() {
        Ok(process) => process,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            on_process_contention();
            MEMORY_STORE_PROCESS_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    };
    let parent = memory_dir
        .parent()
        .ok_or_else(|| io_error("memory store has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let name = memory_dir
        .file_name()
        .ok_or_else(|| io_error("memory store has no directory name"))?;
    crate::secure_fs::ensure_private_dir(parent, Path::new(name))?;
    crate::secure_fs::sync_parent_directory(parent, Path::new(name))?;
    #[cfg(unix)]
    let dir = RetainedDir::open(memory_dir)?;

    for attempt in 0..=retry_count {
        #[cfg(unix)]
        let acquired = dir.try_lock_owner_only(Path::new(MEMORY_STORE_LOCK_FILE))?;
        #[cfg(not(unix))]
        let acquired = crate::secure_fs::try_lock_owner_only(
            memory_dir,
            Path::new(MEMORY_STORE_LOCK_FILE),
        )?;
        if let Some(file) = acquired {
            return Ok(MemoryStoreLock {
                _process: process,
                _file: file,
                #[cfg(unix)]
                dir,
            });
        }
        on_file_contention(attempt);
        if attempt < retry_count {
            std::thread::sleep(retry_delay);
        }
    }
    Err(DreamError::Io(std::io::Error::new(
        std::io::ErrorKind::WouldBlock,
        format!(
            "memory store lock acquisition timed out after {} attempts",
            retry_count.saturating_add(1)
        ),
    )))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MemoryWriteJournalState {
    Prepared,
    Committed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryWriteJournal {
    state: MemoryWriteJournalState,
    slug: String,
    entry_before: Option<String>,
    marker_before: Option<String>,
    index_before: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum MemoryWriteOwner {
    Dreamer,
    HandWritten,
}

fn optional_memory_file(memory_dir: &Path, relative: &Path) -> Result<Option<String>, DreamError> {
    match crate::secure_fs::read_to_string_no_symlink(memory_dir, relative) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DreamError::Io(error)),
    }
}

#[cfg(any(not(unix), test))]
fn remove_optional_memory_file(memory_dir: &Path, relative: &Path) -> Result<(), DreamError> {
    match crate::secure_fs::remove_file_no_symlink(memory_dir, relative) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DreamError::Io(error)),
    }
}

#[cfg(unix)]
fn optional_memory_file_retained(
    memory: &RetainedDir,
    relative: &Path,
) -> Result<Option<String>, DreamError> {
    match memory.open_regular_file(relative) {
        Ok(mut file) => file.read_to_string().map(Some).map_err(DreamError::Io),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(DreamError::Io(error)),
    }
}

#[cfg(unix)]
fn remove_optional_memory_file_retained(
    memory: &RetainedDir,
    relative: &Path,
) -> Result<(), DreamError> {
    match memory.remove_regular_file(relative) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DreamError::Io(error)),
    }
}

#[cfg(unix)]
fn restore_memory_file_retained(
    memory: &RetainedDir,
    relative: &Path,
    before: Option<&str>,
) -> Result<(), DreamError> {
    match before {
        Some(contents) => crate::secure_fs::write_atomic_owner_only_retained(
            memory,
            relative,
            contents.as_bytes(),
        )
        .map_err(DreamError::Io),
        None => remove_optional_memory_file_retained(memory, relative),
    }
}

#[cfg(unix)]
fn dreamer_owned_marker_file_name(slug: &str) -> PathBuf {
    PathBuf::from(format!("{slug}.marker"))
}

#[cfg(unix)]
fn dreamer_owned_marker_dir_retained(memory: &RetainedDir) -> Result<RetainedDir, DreamError> {
    memory
        .ensure_private_subdir(Path::new(DREAMER_OWNED_DIR))
        .map_err(DreamError::Io)
}

#[cfg(unix)]
fn optional_dreamer_owned_marker_retained(
    memory: &RetainedDir,
    slug: &str,
) -> Result<Option<String>, DreamError> {
    let marker_dir = dreamer_owned_marker_dir_retained(memory)?;
    optional_memory_file_retained(&marker_dir, &dreamer_owned_marker_file_name(slug))
}

#[cfg(unix)]
fn remove_dreamer_owned_marker_retained(
    memory: &RetainedDir,
    slug: &str,
) -> Result<(), DreamError> {
    let marker_dir = dreamer_owned_marker_dir_retained(memory)?;
    remove_optional_memory_file_retained(&marker_dir, &dreamer_owned_marker_file_name(slug))
}

#[cfg(unix)]
fn restore_dreamer_owned_marker_retained(
    memory: &RetainedDir,
    slug: &str,
    before: Option<&str>,
) -> Result<(), DreamError> {
    let marker_dir = dreamer_owned_marker_dir_retained(memory)?;
    restore_memory_file_retained(
        &marker_dir,
        &dreamer_owned_marker_file_name(slug),
        before,
    )
}

#[cfg(unix)]
fn write_dreamer_owned_marker_retained(
    memory: &RetainedDir,
    slug: &str,
    body: &str,
) -> Result<(), DreamError> {
    let marker_dir = dreamer_owned_marker_dir_retained(memory)?;
    let contents = dreamer_owned_marker_contents(body);
    crate::secure_fs::write_atomic_owner_only_retained(
        &marker_dir,
        &dreamer_owned_marker_file_name(slug),
        contents.as_bytes(),
    )
    .map_err(DreamError::Io)
}

#[cfg(any(not(unix), test))]
fn restore_memory_file(
    memory_dir: &Path,
    relative: &Path,
    before: Option<&str>,
) -> Result<(), DreamError> {
    match before {
        Some(contents) => crate::secure_fs::write_atomic_owner_only(
            memory_dir,
            relative,
            contents.as_bytes(),
        )
        .map_err(DreamError::Io),
        None => remove_optional_memory_file(memory_dir, relative),
    }
}

fn journal_path() -> &'static Path {
    Path::new(MEMORY_WRITE_JOURNAL_FILE)
}

#[cfg(any(not(unix), test))]
fn write_memory_journal(memory_dir: &Path, journal: &MemoryWriteJournal) -> Result<(), DreamError> {
    let contents = serde_json::to_vec(journal)
        .map_err(|error| DreamError::Io(std::io::Error::other(error)))?;
    crate::secure_fs::write_atomic_owner_only(memory_dir, journal_path(), &contents)
        .and_then(|()| crate::secure_fs::sync_parent_directory(memory_dir, journal_path()))
        .map_err(DreamError::Io)
}

#[cfg(any(not(unix), test))]
fn remove_memory_journal(memory_dir: &Path) -> Result<(), DreamError> {
    crate::secure_fs::remove_file_no_symlink(memory_dir, journal_path())
        .and_then(|()| crate::secure_fs::sync_parent_directory(memory_dir, journal_path()))
        .map_err(DreamError::Io)
}

#[cfg(any(not(unix), test))]
fn recover_memory_write_journal(memory_dir: &Path) -> Result<(), DreamError> {
    let Some(contents) = optional_memory_file(memory_dir, journal_path())? else {
        return Ok(());
    };
    let journal: MemoryWriteJournal = serde_json::from_str(&contents)
        .map_err(|error| io_error(format!("memory write journal is invalid: {error}")))?;
    if !crate::memory::curation::is_safe_memory_slug(&journal.slug) {
        return Err(io_error("memory write journal has an unsafe slug"));
    }
    if journal.state == MemoryWriteJournalState::Prepared {
        let entry = PathBuf::from(format!("{}.md", journal.slug));
        let marker = dreamer_owned_marker_relative_path(&journal.slug);
        if journal.marker_before.is_some() {
            ensure_dreamer_owned_marker_dir_durable(memory_dir)?;
        }
        restore_memory_file(memory_dir, &entry, journal.entry_before.as_deref())?;
        restore_memory_file(memory_dir, &marker, journal.marker_before.as_deref())?;
        restore_memory_file(
            memory_dir,
            Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
            journal.index_before.as_deref(),
        )?;
    }
    remove_memory_journal(memory_dir)
}

#[cfg(unix)]
fn write_memory_journal_retained(
    memory: &RetainedDir,
    journal: &MemoryWriteJournal,
) -> Result<(), DreamError> {
    let contents = serde_json::to_vec(journal)
        .map_err(|error| DreamError::Io(std::io::Error::other(error)))?;
    crate::secure_fs::write_atomic_owner_only_retained(memory, journal_path(), &contents)
        .map_err(DreamError::Io)
}

#[cfg(unix)]
fn remove_memory_journal_retained(memory: &RetainedDir) -> Result<(), DreamError> {
    remove_optional_memory_file_retained(memory, journal_path())
}

#[cfg(unix)]
fn recover_memory_write_journal_retained(memory: &RetainedDir) -> Result<(), DreamError> {
    let Some(contents) = optional_memory_file_retained(memory, journal_path())? else {
        return Ok(());
    };
    let journal: MemoryWriteJournal = serde_json::from_str(&contents)
        .map_err(|error| io_error(format!("memory write journal is invalid: {error}")))?;
    if !crate::memory::curation::is_safe_memory_slug(&journal.slug) {
        return Err(io_error("memory write journal has an unsafe slug"));
    }
    if journal.state == MemoryWriteJournalState::Prepared {
        let entry = PathBuf::from(format!("{}.md", journal.slug));
        restore_memory_file_retained(memory, &entry, journal.entry_before.as_deref())?;
        restore_dreamer_owned_marker_retained(
            memory,
            &journal.slug,
            journal.marker_before.as_deref(),
        )?;
        restore_memory_file_retained(
            memory,
            Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
            journal.index_before.as_deref(),
        )?;
    }
    remove_memory_journal_retained(memory)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum MemoryDecayJournalState {
    Prepared,
    EntryArchived,
    MarkerRemoved,
    IndexUpdated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemoryDecayJournal {
    state: MemoryDecayJournalState,
    slug: String,
    entry_body_hash: u64,
}

fn decay_journal_path() -> &'static Path {
    Path::new(MEMORY_DECAY_JOURNAL_FILE)
}

#[cfg(unix)]
fn write_memory_decay_journal_retained(
    memory: &RetainedDir,
    journal: &MemoryDecayJournal,
) -> Result<(), DreamError> {
    let contents = serde_json::to_vec(journal)
        .map_err(|error| DreamError::Io(std::io::Error::other(error)))?;
    crate::secure_fs::write_atomic_owner_only_retained(memory, decay_journal_path(), &contents)
        .map_err(DreamError::Io)
}

#[cfg(unix)]
fn remove_memory_decay_journal_retained(memory: &RetainedDir) -> Result<(), DreamError> {
    remove_optional_memory_file_retained(memory, decay_journal_path())
}

#[cfg(unix)]
fn remove_memory_index_pointer_retained(
    memory: &RetainedDir,
    slug: &str,
) -> Result<(), DreamError> {
    let index = Path::new(crate::memory::paths::MEMORY_INDEX_FILE);
    let Some(contents) = optional_memory_file_retained(memory, index)? else {
        return Ok(());
    };
    let needle = format!("]({slug}.md)");
    let retained = contents
        .lines()
        .filter(|line| !line.contains(&needle))
        .collect::<Vec<_>>();
    if retained.len() == contents.lines().count() {
        return Ok(());
    }
    crate::secure_fs::write_atomic_owner_only_retained(
        memory,
        index,
        format!("{}\n", retained.join("\n")).as_bytes(),
    )
    .map_err(DreamError::Io)
}

#[cfg(unix)]
fn recover_memory_decay_journal_retained(memory: &RetainedDir) -> Result<(), DreamError> {
    let Some(contents) = optional_memory_file_retained(memory, decay_journal_path())? else {
        return Ok(());
    };
    let mut journal: MemoryDecayJournal = serde_json::from_str(&contents)
        .map_err(|error| io_error(format!("memory decay journal is invalid: {error}")))?;
    if !crate::memory::curation::is_safe_memory_slug(&journal.slug) {
        return Err(io_error("memory decay journal has an unsafe slug"));
    }

    let entry_relative = PathBuf::from(format!("{}.md", journal.slug));
    let archive = memory
        .ensure_private_subdir(Path::new(DECAY_ARCHIVE_DIR))
        .map_err(DreamError::Io)?;
    let entry_body = optional_memory_file_retained(memory, &entry_relative)?;
    let archive_body = optional_memory_file_retained(&archive, &entry_relative)?;

    match (entry_body.as_deref(), archive_body.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(io_error(format!(
                "decay recovery found both live and archived copies for {}",
                journal.slug
            )));
        }
        (Some(body), None) => {
            if dreamer_owned_body_hash(body) != journal.entry_body_hash {
                return Err(io_error(format!(
                    "decay recovery refused a changed live entry for {}",
                    journal.slug
                )));
            }
            let entry = memory.open_regular_file(&entry_relative)?;
            if !memory.rename_file_no_replace(
                &entry_relative,
                &entry,
                &archive,
                &entry_relative,
            )? {
                return Err(io_error(format!(
                    "decay recovery could not archive {} without replacement",
                    journal.slug
                )));
            }
            journal.state = MemoryDecayJournalState::EntryArchived;
            write_memory_decay_journal_retained(memory, &journal)?;
        }
        (None, Some(body)) => {
            if dreamer_owned_body_hash(body) != journal.entry_body_hash {
                return Err(io_error(format!(
                    "decay recovery refused a changed archived entry for {}",
                    journal.slug
                )));
            }
            if journal.state == MemoryDecayJournalState::Prepared {
                journal.state = MemoryDecayJournalState::EntryArchived;
                write_memory_decay_journal_retained(memory, &journal)?;
            }
        }
        (None, None) => {
            if journal.state == MemoryDecayJournalState::Prepared {
                journal.state = MemoryDecayJournalState::EntryArchived;
                write_memory_decay_journal_retained(memory, &journal)?;
            }
        }
    }

    remove_dreamer_owned_marker_retained(memory, &journal.slug)?;
    journal.state = MemoryDecayJournalState::MarkerRemoved;
    write_memory_decay_journal_retained(memory, &journal)?;
    remove_memory_index_pointer_retained(memory, &journal.slug)?;
    journal.state = MemoryDecayJournalState::IndexUpdated;
    write_memory_decay_journal_retained(memory, &journal)?;
    remove_memory_decay_journal_retained(memory)
}

#[cfg(unix)]
fn archive_decay_entry_retained(
    memory: &RetainedDir,
    slug: &str,
    body: &str,
) -> Result<(), DreamError> {
    let journal = MemoryDecayJournal {
        state: MemoryDecayJournalState::Prepared,
        slug: slug.to_string(),
        entry_body_hash: dreamer_owned_body_hash(body),
    };
    write_memory_decay_journal_retained(memory, &journal)?;
    recover_memory_decay_journal_retained(memory)
}

fn default_memory_index_lines() -> Vec<String> {
    vec![
        "# Zo — Persistent Memory Index".to_string(),
        String::new(),
        "One pointer line per entry. Read an entry's file only when its summary is relevant to the task at hand.".to_string(),
        String::new(),
    ]
}

fn memory_index_lines(contents: Option<&str>) -> Vec<String> {
    contents.map_or_else(default_memory_index_lines, |contents| {
        contents.lines().map(ToOwned::to_owned).collect()
    })
}

/// The exact `MEMORY.md` pointer line for one entry — shared by the index
/// writers and the idempotence check so the two can never disagree about what
/// "already indexed" means.
fn memory_index_pointer(slug: &str, summary: &str) -> String {
    format!("- [{slug}]({slug}.md) — {summary}")
}

/// The index contents after upserting `slug`, plus whether the pointer was new.
fn next_memory_index(contents: Option<&str>, slug: &str, summary: &str) -> (String, WriteOutcome) {
    let pointer = memory_index_pointer(slug, summary);
    let mut lines = memory_index_lines(contents);
    let needle = format!("]({slug}.md)");
    let outcome = if let Some(line) = lines.iter_mut().find(|line| line.contains(&needle)) {
        *line = pointer;
        WriteOutcome::Updated
    } else {
        lines.push(pointer);
        WriteOutcome::Created
    };
    (format!("{}\n", lines.join("\n")), outcome)
}

/// True when this write would change nothing on disk: the entry already holds
/// exactly `body_on_disk`, the ownership marker already says what `owner`
/// would stamp, and the index already carries this exact pointer.
///
/// The Dreamer itself rarely gets here (curation skips `AlreadyKnown` slugs
/// before the store sees them), but the `MemoryWrite` tool has no such gate:
/// a model re-records the lesson it considers important session after
/// session, usually byte-identically. Each of those re-saves ran the full
/// transaction — a journal write, the entry, the marker removal and the index,
/// four fsynced files of pure mtime churn carrying bytes that were already
/// there — and mtime churn is not cosmetic: the retention sweep ages stores by
/// it, so a store that is merely re-saved reads as freshly authored.
fn memory_write_is_noop(
    owner: MemoryWriteOwner,
    entry_before: Option<&str>,
    marker_before: Option<&str>,
    index_before: Option<&str>,
    slug: &str,
    summary: &str,
    body_on_disk: &str,
) -> bool {
    let entry_settled = match owner {
        // Dreamer bodies are compared byte-exactly: curation's `AlreadyKnown`
        // gate means a Dreamer write reaching the store is meant to change it.
        MemoryWriteOwner::Dreamer => entry_before == Some(body_on_disk),
        // Hand-written bodies carry a fresh `written_at` stamp on every save,
        // so byte equality would never fire for exactly the writer that
        // re-saves the most.
        MemoryWriteOwner::HandWritten => entry_before.is_some_and(|before| {
            crate::memory::classification::bodies_equal_ignoring_written_at(
                before,
                body_on_disk,
            )
        }),
    };
    if !entry_settled {
        return false;
    }
    let marker_settled = match owner {
        MemoryWriteOwner::Dreamer => {
            marker_before == Some(dreamer_owned_marker_contents(body_on_disk).as_str())
        }
        // A hand-written entry must carry no Dreamer marker; one still present
        // means ownership is being transferred, which is a real write.
        MemoryWriteOwner::HandWritten => marker_before.is_none(),
    };
    if !marker_settled {
        return false;
    }
    let pointer = memory_index_pointer(slug, summary);
    memory_index_lines(index_before)
        .iter()
        .any(|line| line == &pointer)
}

#[cfg(any(not(unix), test))]
fn write_index_from_contents(
    memory_dir: &Path,
    contents: Option<&str>,
    slug: &str,
    summary: &str,
) -> Result<WriteOutcome, DreamError> {
    let (next, outcome) = next_memory_index(contents, slug, summary);
    if contents == Some(next.as_str()) {
        return Ok(outcome);
    }
    crate::secure_fs::write_atomic_owner_only(
        memory_dir,
        Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
        next.as_bytes(),
    )
    .map_err(DreamError::Io)?;
    Ok(outcome)
}

#[cfg(unix)]
fn write_index_from_contents_retained(
    memory: &RetainedDir,
    contents: Option<&str>,
    slug: &str,
    summary: &str,
) -> Result<WriteOutcome, DreamError> {
    let (next, outcome) = next_memory_index(contents, slug, summary);
    // Byte-identical index -> skip the write. The index is rewritten on every
    // entry write, so on a settled store this was the hottest of the four
    // redundant writes.
    if contents == Some(next.as_str()) {
        return Ok(outcome);
    }
    crate::secure_fs::write_atomic_owner_only_retained(
        memory,
        Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
        next.as_bytes(),
    )
    .map_err(DreamError::Io)?;
    Ok(outcome)
}

#[cfg(test)]
fn upsert_index(index_path: &Path, slug: &str, summary: &str) -> Result<WriteOutcome, DreamError> {
    let memory_dir = index_path
        .parent()
        .ok_or_else(|| io_error("memory index has no parent directory"))?;
    let index_relative = Path::new(crate::memory::paths::MEMORY_INDEX_FILE);
    let contents = optional_memory_file(memory_dir, index_relative)?;
    write_index_from_contents(memory_dir, contents.as_deref(), slug, summary)
}

fn write_memory_entry_transaction(
    memory_dir: &Path,
    entry: &MemoryWriteRequest,
    owner: MemoryWriteOwner,
) -> Result<WriteOutcome, DreamError> {
    write_memory_entry_transaction_with_before_prepare(memory_dir, entry, owner, || {})
}

fn write_memory_entry_transaction_with_before_prepare<F>(
    memory_dir: &Path,
    entry: &MemoryWriteRequest,
    owner: MemoryWriteOwner,
    before_prepare: F,
) -> Result<WriteOutcome, DreamError>
where
    F: FnOnce(),
{
    if !crate::memory::curation::is_safe_memory_slug(&entry.slug) {
        return Err(io_error("memory entry has an unsafe slug"));
    }
    let lock = acquire_memory_store_lock(memory_dir)?;
    #[cfg(unix)]
    {
        write_memory_entry_transaction_locked_with_before_prepare(
            &lock,
            entry,
            owner,
            before_prepare,
        )
    }
    #[cfg(not(unix))]
    {
        recover_memory_write_journal(memory_dir)?;
        let entry_relative = PathBuf::from(format!("{}.md", entry.slug));
        let marker_relative = Path::new(DREAMER_OWNED_DIR).join(format!("{}.marker", entry.slug));
        let entry_before = optional_memory_file(memory_dir, &entry_relative)?;
        let marker_before = optional_memory_file(memory_dir, &marker_relative)?;
        let index_before = optional_memory_file(
            memory_dir,
            Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
        )?;
        let body_on_disk = format!("{}\n", entry.body);
        if matches!(owner, MemoryWriteOwner::Dreamer)
            && entry_before.is_some()
            && marker_before.as_deref()
                != Some(
                    dreamer_owned_marker_contents(entry_before.as_deref().unwrap_or_default())
                        .as_str(),
                )
        {
            return Err(io_error(
                "refusing to overwrite a non-Dreamer memory entry with a Dreamer promotion",
            ));
        }
        if memory_write_is_noop(
            owner,
            entry_before.as_deref(),
            marker_before.as_deref(),
            index_before.as_deref(),
            &entry.slug,
            &entry.summary,
            &body_on_disk,
        ) {
            return Ok(WriteOutcome::Unchanged);
        }

        before_prepare();
        let mut journal = MemoryWriteJournal {
            state: MemoryWriteJournalState::Prepared,
            slug: entry.slug.clone(),
            entry_before: entry_before.clone(),
            marker_before: marker_before.clone(),
            index_before: index_before.clone(),
        };
        write_memory_journal(memory_dir, &journal)?;
        let write_result = (|| {
            crate::secure_fs::write_atomic_owner_only(
                memory_dir,
                &entry_relative,
                body_on_disk.as_bytes(),
            )?;
            match owner {
                MemoryWriteOwner::Dreamer => {
                    write_dreamer_owned_marker(memory_dir, &entry.slug, &body_on_disk)?;
                }
                MemoryWriteOwner::HandWritten => {
                    remove_optional_memory_file(memory_dir, &marker_relative)?;
                }
            }
            write_index_from_contents(
                memory_dir,
                index_before.as_deref(),
                &entry.slug,
                &entry.summary,
            )
        })();
        let outcome = match write_result {
            Ok(outcome) => outcome,
            Err(error) => match recover_memory_write_journal(memory_dir) {
                Ok(()) => return Err(error),
                Err(recovery) => {
                    return Err(io_error(format!(
                        "memory write failed: {error}; recovery also failed: {recovery}"
                    )));
                }
            },
        };
        journal.state = MemoryWriteJournalState::Committed;
        write_memory_journal(memory_dir, &journal)?;
        remove_memory_journal(memory_dir)?;
        drop(lock);
        Ok(if entry_before.is_some() {
            WriteOutcome::Updated
        } else {
            outcome
        })
    }
}

#[cfg(all(unix, test))]
fn write_memory_entry_transaction_locked(
    lock: &MemoryStoreLock,
    entry: &MemoryWriteRequest,
    owner: MemoryWriteOwner,
) -> Result<WriteOutcome, DreamError> {
    write_memory_entry_transaction_locked_with_before_prepare(lock, entry, owner, || {})
}

#[cfg(unix)]
fn write_memory_entry_transaction_locked_with_before_prepare<F>(
    lock: &MemoryStoreLock,
    entry: &MemoryWriteRequest,
    owner: MemoryWriteOwner,
    before_prepare: F,
) -> Result<WriteOutcome, DreamError>
where
    F: FnOnce(),
{
    if !crate::memory::curation::is_safe_memory_slug(&entry.slug) {
        return Err(io_error("memory entry has an unsafe slug"));
    }
    recover_memory_write_journal_retained(&lock.dir)?;
    recover_memory_decay_journal_retained(&lock.dir)?;

    let entry_relative = PathBuf::from(format!("{}.md", entry.slug));
    let entry_before = match optional_memory_file_retained(&lock.dir, &entry_relative) {
        Ok(entry_before) => entry_before,
        Err(DreamError::Io(error))
            if matches!(owner, MemoryWriteOwner::HandWritten)
                && error.raw_os_error() == Some(nix::libc::ELOOP) =>
        {
            // A manual write may safely replace a leaf symlink atomically. It
            // is not a restorable regular-file preimage and is never followed.
            None
        }
        Err(error) => return Err(error),
    };
    let marker_before = optional_dreamer_owned_marker_retained(&lock.dir, &entry.slug)?;
    let index_before = optional_memory_file_retained(
        &lock.dir,
        Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
    )?;
    let body_on_disk = format!("{}\n", entry.body);

    if matches!(owner, MemoryWriteOwner::Dreamer)
        && entry_before.is_some()
        && marker_before.as_deref()
            != Some(
                dreamer_owned_marker_contents(entry_before.as_deref().unwrap_or_default())
                    .as_str(),
            )
    {
        return Err(io_error(
            "refusing to overwrite a non-Dreamer memory entry with a Dreamer promotion",
        ));
    }

    if memory_write_is_noop(
        owner,
        entry_before.as_deref(),
        marker_before.as_deref(),
        index_before.as_deref(),
        &entry.slug,
        &entry.summary,
        &body_on_disk,
    ) {
        return Ok(WriteOutcome::Unchanged);
    }

    before_prepare();
    let mut journal = MemoryWriteJournal {
        state: MemoryWriteJournalState::Prepared,
        slug: entry.slug.clone(),
        entry_before: entry_before.clone(),
        marker_before: marker_before.clone(),
        index_before: index_before.clone(),
    };
    write_memory_journal_retained(&lock.dir, &journal)?;

    let write_result: Result<WriteOutcome, DreamError> = (|| {
        crate::secure_fs::write_atomic_owner_only_retained(
            &lock.dir,
            &entry_relative,
            body_on_disk.as_bytes(),
        )
        .map_err(DreamError::Io)?;
        match owner {
            MemoryWriteOwner::Dreamer => {
                write_dreamer_owned_marker_retained(&lock.dir, &entry.slug, &body_on_disk)?;
            }
            MemoryWriteOwner::HandWritten => {
                remove_dreamer_owned_marker_retained(&lock.dir, &entry.slug)?;
            }
        }
        write_index_from_contents_retained(
            &lock.dir,
            index_before.as_deref(),
            &entry.slug,
            &entry.summary,
        )
    })();

    let outcome = match write_result {
        Ok(outcome) => outcome,
        Err(error) => match recover_memory_write_journal_retained(&lock.dir) {
            Ok(()) => return Err(error),
            Err(recovery) => {
                return Err(io_error(format!(
                    "memory write failed: {error}; recovery also failed: {recovery}"
                )));
            }
        },
    };
    journal.state = MemoryWriteJournalState::Committed;
    write_memory_journal_retained(&lock.dir, &journal)?;
    remove_memory_journal_retained(&lock.dir)?;
    Ok(if entry_before.is_some() {
        WriteOutcome::Updated
    } else {
        outcome
    })
}

/// Persist a hand-written memory entry through the same lock and crash-recovery
/// journal as Dreamer promotions. This prevents either writer from losing the
/// other's index pointer or leaving an ownership marker on a manual override.
pub fn write_hand_written_memory_entry(
    cwd: &Path,
    local: bool,
    entry: &MemoryWriteRequest,
) -> Result<WriteOutcome, DreamError> {
    if local && global_memory_has_slug(cwd, &entry.slug) {
        return Err(io_error(format!(
            "local memory `{}` cannot modify the read-only global entry with the same slug",
            entry.slug
        )));
    }
    let mut entry = entry.clone();
    entry.body = crate::memory::with_memory_scope_metadata(
        &entry.body,
        crate::memory::MemoryScope::from_local(local),
    );
    write_memory_entry_transaction(
        &crate::memory::paths::memory_write_dir(cwd, local),
        &entry,
        MemoryWriteOwner::HandWritten,
    )
}

fn global_memory_has_slug(cwd: &Path, slug: &str) -> bool {
    optional_memory_file(
        &crate::memory::paths::memory_write_dir(cwd, false),
        Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
    )
    .ok()
    .flatten()
    .is_some_and(|contents| contents.lines().filter_map(parse_index_slug).any(|known| known == slug))
}

impl MemoryStore for FsMemoryStore {
    fn existing_slugs(&self) -> Vec<String> {
        let mut slugs = optional_memory_file(
            &self.memory_dir,
            Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
        )
        .ok()
        .flatten()
        .map_or_else(Vec::new, |contents| {
            contents.lines().filter_map(parse_index_slug).collect()
        });
        if let Some(global_dir) = self.read_only_global_dir() {
            if let Ok(Some(contents)) = optional_memory_file(
                &global_dir,
                Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
            ) {
                slugs.extend(contents.lines().filter_map(parse_index_slug));
            }
        }
        slugs.sort();
        slugs.dedup();
        slugs
    }

    fn existing_summaries(&self) -> Vec<(String, String)> {
        let mut by_slug = BTreeMap::new();
        if let Some(global_dir) = self.read_only_global_dir() {
            by_slug.extend(memory_index_summaries(&global_dir));
        }
        by_slug.extend(memory_index_summaries(&self.memory_dir));
        by_slug.into_iter().collect()
    }

    fn write_entry(&self, entry: &MemoryWriteRequest) -> Result<WriteOutcome, DreamError> {
        write_memory_entry_transaction(&self.memory_dir, entry, MemoryWriteOwner::Dreamer)
    }
}

impl FsMemoryStore {
    /// The durable sibling of a local overlay, if this store is local. The
    /// layout is fixed by `memory::paths`, so deriving it from the private
    /// directory keeps `FsMemoryStore`'s test-only literal construction stable.
    fn read_only_global_dir(&self) -> Option<PathBuf> {
        (self.memory_dir.file_name()? == crate::memory::paths::MEMORY_LOCAL_STORE).then(|| {
            self.memory_dir
                .parent()
                .map(|parent| parent.join(crate::memory::paths::MEMORY_STORE))
        })?
    }
}

/// Extract the slug from a `- [slug](slug.md) — summary` index pointer line.
fn parse_index_slug(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("- [")?;
    let (slug, rest) = rest.split_once("](")?;
    rest.strip_prefix(slug)?.strip_prefix(".md)")?;
    crate::memory::curation::is_safe_memory_slug(slug).then(|| slug.to_string())
}

fn memory_index_summaries(memory_dir: &Path) -> Vec<(String, String)> {
    optional_memory_file(
        memory_dir,
        Path::new(crate::memory::paths::MEMORY_INDEX_FILE),
    )
    .ok()
    .flatten()
    .map_or_else(Vec::new, |contents| {
        crate::memory::parse_memory_index(&contents)
            .into_iter()
            .filter(|entry| crate::memory::curation::is_safe_memory_slug(&entry.slug))
            .map(|entry| (entry.slug, entry.summary))
            .collect()
    })
}

/// Run a dreaming pass against the real filesystem rooted at `cwd`, with the
/// production [`PromotionPolicy::default`] gate. The one-call entry point a
/// scheduler (cron) or `/dream` command invokes.
///
/// Mines *both* externalized signal streams through a [`CompositeLessonSource`]:
/// the `.zo/dream/` candidate logs (green-accept workflow lessons) and the
/// `.zo/turns/` turn trace (recurring tool-failure gotchas). The curation
/// gate dedups and ranks the merged pool, so adding a producer never changes
/// the promotion contract.
pub fn dream_at_cwd(cwd: &Path, active_model: Option<&str>) -> Result<DreamReport, DreamError> {
    let source = CompositeLessonSource::new(vec![
        Box::new(JsonlLessonSource::at_cwd(cwd)),
        Box::new(TurnLogLessonSource::at_cwd(cwd)),
        Box::new(UserPatternLessonSource::at_cwd(cwd)),
        Box::new(AutomationLessonSource::at_cwd(cwd)),
    ]);
    let dreamer = Dreamer::new(
        source,
        FsMemoryStore::local_at_cwd(cwd),
        PromotionPolicy::default(),
    )
    .authored_by(active_model);
    dreamer.run()
}

// ---------------------------------------------------------------------------
// Automatic between-sessions trigger
// ---------------------------------------------------------------------------

/// Default minimum gap between *automatic* dreaming passes. The Dreamer is
/// meant to run "between sessions" (doc §3), not on every process launch, so
/// rapid relaunches coalesce into at most one pass per window.
pub const DEFAULT_AUTO_DREAM_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(6 * 60 * 60);

/// A full compaction already occurs only after a large context segment, so it
/// is a much sparser and more meaningful trigger than an arbitrary turn count.
/// Fifteen minutes prevents repeated overflow/retry rounds from scheduling a
/// fleet of identical passes while still moving curation into a long session.
pub const DEFAULT_COMPACTION_DREAM_INTERVAL: std::time::Duration =
    std::time::Duration::from_secs(15 * 60);

/// Throttle marker file: stores the unix-seconds timestamp of the last
/// automatic pass under `.zo/dream/`, alongside the candidate logs.
const THROTTLE_MARKER: &str = ".last_auto_dream";
const COMPACTION_THROTTLE_MARKER: &str = ".last_compaction_dream";

/// Pure throttle decision: should an automatic pass run, given the last-run
/// timestamp, the current time, and the minimum interval? Kept separate from
/// the marker IO so the policy is unit-tested without a clock or filesystem.
///
/// `None` for `last_run` means "never run" → always dream. A `last_run` in the
/// future (clock skew / edited marker) is treated as "just ran" → skip, so a
/// bad timestamp can never cause a dream storm.
#[must_use]
pub fn should_auto_dream(
    last_run: Option<std::time::SystemTime>,
    now: std::time::SystemTime,
    min_interval: std::time::Duration,
) -> bool {
    match last_run {
        None => true,
        Some(last) => now
            .duration_since(last)
            .is_ok_and(|gap| gap >= min_interval),
    }
}

/// Fire an automatic dreaming pass if the throttle window has elapsed, updating
/// the marker on success. Returns `Ok(None)` when throttled (the common case),
/// `Ok(Some(report))` when a pass ran. Designed to be called fire-and-forget at
/// session startup; it does no work and no IO beyond a single marker read when
/// throttled.
pub fn maybe_auto_dream(
    cwd: &Path,
    min_interval: std::time::Duration,
    active_model: Option<&str>,
) -> Result<Option<DreamReport>, DreamError> {
    let marker = trace_base(cwd)
        .join(ZO_DIR_NAME)
        .join(DREAM_DIR)
        .join(THROTTLE_MARKER);
    let last_run = read_marker(&marker);
    if !should_auto_dream(last_run, std::time::SystemTime::now(), min_interval) {
        return Ok(None);
    }
    let report = dream_at_cwd(cwd, active_model)?;
    // Best-effort hygiene: age off entries past their revisit window so memory
    // does not creep unboundedly. A decay failure must not sink the dream pass,
    // so its error is swallowed (the next pass retries).
    let _ = FsMemoryStore::at_cwd(cwd).decay_expired(now_secs());
    write_marker(&trace_base(cwd), &marker)?;
    Ok(Some(report))
}

/// Schedule curation after a successful conversation compaction without ever
/// borrowing or blocking the render/stream loop. The cheap gate runs on the
/// detached worker before the full Dreamer pass: no evidence files, or a pass
/// inside the compact-trigger throttle window, means no index load, no model
/// work, and no memory write.
///
/// The worker intentionally has no UI callback. A compaction must be usable
/// even when its initiating render stream has ended; successful promotions are
/// visible through ordinary recall on the next turn, whose fingerprint watcher
/// reloads the local/global stores.
#[must_use]
pub fn spawn_auto_dream_on_compaction(cwd: PathBuf, active_model: Option<String>) -> bool {
    std::thread::Builder::new()
        .name("dreamer-compaction".to_string())
        .spawn(move || {
            let _ = maybe_auto_dream_on_compaction(
                &cwd,
                DEFAULT_COMPACTION_DREAM_INTERVAL,
                active_model.as_deref(),
            );
        })
        .is_ok()
}

/// Detached-worker half of [`spawn_auto_dream_on_compaction`]. Public for
/// hermetic tests and the IDE session hook; callers on a render path must use
/// the spawning entry point above.
pub fn maybe_auto_dream_on_compaction(
    cwd: &Path,
    min_interval: std::time::Duration,
    active_model: Option<&str>,
) -> Result<Option<DreamReport>, DreamError> {
    let marker = trace_base(cwd)
        .join(ZO_DIR_NAME)
        .join(DREAM_DIR)
        .join(COMPACTION_THROTTLE_MARKER);
    if !should_auto_dream(read_marker(&marker), std::time::SystemTime::now(), min_interval)
        || !has_compaction_curation_evidence(cwd)
    {
        return Ok(None);
    }

    // Claim the interval before doing the full scan. A simultaneous compaction
    // therefore loses only a marker read/write, not a duplicate curation pass.
    // If this worker fails, the next compact retries after the short interval.
    write_marker(&trace_base(cwd), &marker)?;
    let report = dream_at_cwd(cwd, active_model)?;
    let _ = FsMemoryStore::local_at_cwd(cwd).decay_expired(now_secs());
    Ok(Some(report))
}

/// Constant-time-ish prefilter for compaction-triggered curation. The actual
/// promotion gate still rejects one-off noise, unsupported hypotheses, and
/// transient tool output; this merely avoids opening every candidate/index
/// file when neither producer has written any evidence at all.
fn has_compaction_curation_evidence(cwd: &Path) -> bool {
    [trace_base(cwd).join(ZO_DIR_NAME).join(DREAM_DIR), trace_base(cwd).join(ZO_DIR_NAME).join("turns")]
        .into_iter()
        .any(|dir| fs::read_dir(dir).ok().is_some_and(|mut entries| entries.next().is_some()))
}

/// Read the unix-seconds timestamp from the throttle marker, if present and
/// parseable. Any failure (missing, corrupt) reads as "never run".
fn read_marker(marker: &Path) -> Option<std::time::SystemTime> {
    let secs: u64 = fs::read_to_string(marker).ok()?.trim().parse().ok()?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
}

/// Stamp the throttle marker with the current unix-seconds time without ever
/// following an existing marker symlink. Atomic rename replaces a racing symlink
/// at `marker` instead of opening its target, and preflight rejects non-file
/// markers before writing so broken/corrupt state stays observable as an error.
fn write_marker(root: &Path, marker: &Path) -> Result<(), DreamError> {
    let parent = marker
        .parent()
        .ok_or_else(|| io_error("marker path has no parent directory"))?;
    let relative_parent = parent
        .strip_prefix(root)
        .map_err(|_| io_error("marker path escaped its trusted root"))?;
    crate::secure_fs::ensure_private_dir(root, relative_parent)?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    write_text_atomic(root, marker, &secs.to_string())
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    fn observation(index: usize) -> LessonObservation {
        LessonObservation {
            signature: format!("sig-{index}"),
            session_id: "session".to_string(),
            lesson: format!("lesson {index}"),
            summary: format!("summary {index}"),
            kind: LessonKind::Workflow,
            verified: true,
        }
    }

    fn evidence(index: usize) -> CandidateEvidence {
        CandidateEvidence {
            session_id: format!("s-{index}"),
            source: "test".to_string(),
            detail: format!("detail {index}"),
            verified: true,
        }
    }

    #[test]
    fn record_observation_retains_latest_jsonl_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        for index in 0..(MAX_DREAM_JSONL_LINES + 2) {
            record_observation(cwd, &observation(index)).unwrap();
        }

        let observed = JsonlLessonSource::at_cwd(cwd).observations();
        assert_eq!(observed.len(), MAX_DREAM_JSONL_LINES);
        assert_eq!(observed.first().unwrap().signature, "sig-2");
        assert_eq!(observed.last().unwrap().signature, "sig-5");
    }

    #[test]
    fn self_improve_candidate_evidence_is_capped() {
        let mut existing = Vec::new();
        merge_evidence(
            &mut existing,
            (0..(MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE + 2)).map(evidence),
        );

        assert_eq!(existing.len(), MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE);
        assert_eq!(existing.first().unwrap().session_id, "s-2");
        assert_eq!(existing.last().unwrap().session_id, "s-5");
    }

    fn promoted(slug: &str) -> decision_core::dreamer::PromotedLesson {
        decision_core::dreamer::PromotedLesson {
            slug: slug.to_string(),
            summary: format!("summary for {slug}"),
            lesson: format!("lesson body for {slug}"),
            kind: LessonKind::Workflow,
            distinct_sessions: 2,
            verified: true,
            confidence: 0.9,
            expiry_days: 90,
        }
    }

    #[cfg(unix)]
    #[test]
    fn decay_uses_retained_memory_for_marker_validation_and_removal() {
        let tmp = tempfile::tempdir().unwrap();
        // Explicit `memory_dir` shared by store and assertions, so no
        // `ZO_CONFIG_HOME` resolution to race parallel tests.
        let memory_dir = tmp.path().join("memory");
        let store = FsMemoryStore {
            memory_dir: memory_dir.clone(),
        };
        let day = 86_400;
        let now = 200 * day;
        let stale = render_entry_body(&promoted("stale-lesson"), now - 100 * day, None);
        store
            .write_entry(&MemoryWriteRequest {
                slug: "stale-lesson".to_string(),
                summary: "stale summary".to_string(),
                body: stale,
            })
            .unwrap();

        let retained_dir = tmp.path().join("retained-memory");
        let stale_body = fs::read_to_string(memory_dir.join("stale-lesson.md")).unwrap();
        let attacker_marker = memory_dir
            .join(DREAMER_OWNED_DIR)
            .join("stale-lesson.marker");

        let archived = store
            .decay_expired_with_before_marker_validation(now, || {
                fs::rename(&memory_dir, &retained_dir).unwrap();
                fs::create_dir(&memory_dir).unwrap();
                fs::write(memory_dir.join("MEMORY.md"), "attacker index\n").unwrap();
                write_dreamer_owned_marker(&memory_dir, "stale-lesson", &stale_body).unwrap();
            })
            .unwrap();

        assert_eq!(archived, vec!["stale-lesson"]);
        assert!(retained_dir.join("archive/stale-lesson.md").exists());
        assert!(!retained_dir
            .join(DREAMER_OWNED_DIR)
            .join("stale-lesson.marker")
            .exists());
        assert!(attacker_marker.exists());
        assert_eq!(
            fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap(),
            "attacker index\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn decay_rejects_symlinked_and_hardlinked_ownership_markers() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
        // Tempdir-local store: `at_cwd` resolves through the real global config
        // home when `ZO_CONFIG_HOME` is unset — which is exactly what `cargo
        // test` looks like — so every run of this test deposited a slug under
        // the developer's actual `~/.zo/projects`.
        let memory_dir = tmp.path().join("memory");
        let store = FsMemoryStore {
            memory_dir: memory_dir.clone(),
        };
        let day = 86_400;
        let now = 200 * day;
        for slug in ["symlink-marker", "hardlink-marker"] {
            store
                .write_entry(&MemoryWriteRequest {
                    slug: slug.to_string(),
                    summary: slug.to_string(),
                    body: render_entry_body(&promoted(slug), now - 100 * day, None),
                })
                .unwrap();
        }

        let marker_dir = memory_dir.join(DREAMER_OWNED_DIR);
        let symlink_target = tmp.path().join("symlink-marker-target");
        let hardlink_target = tmp.path().join("hardlink-marker-target");
        fs::write(&symlink_target, "symlink sentinel").unwrap();
        fs::write(&hardlink_target, "hardlink sentinel").unwrap();
        fs::remove_file(marker_dir.join("symlink-marker.marker")).unwrap();
        fs::remove_file(marker_dir.join("hardlink-marker.marker")).unwrap();
        symlink(&symlink_target, marker_dir.join("symlink-marker.marker")).unwrap();
        fs::hard_link(&hardlink_target, marker_dir.join("hardlink-marker.marker")).unwrap();

        assert!(store.decay_expired(now).unwrap().is_empty());
        assert!(memory_dir.join("symlink-marker.md").exists());
        assert!(memory_dir.join("hardlink-marker.md").exists());
        assert_eq!(fs::read_to_string(symlink_target).unwrap(), "symlink sentinel");
        assert_eq!(fs::read_to_string(hardlink_target).unwrap(), "hardlink sentinel");
    }

    #[test]
    fn decay_archives_expired_entry_and_keeps_fresh_one() {
        let tmp = tempfile::tempdir().unwrap();
        // Tempdir-local store: `at_cwd` resolves through the real global config
        // home when `ZO_CONFIG_HOME` is unset — which is exactly what `cargo
        // test` looks like — so every run of this test deposited a slug under
        // the developer's actual `~/.zo/projects`.
        let memory_dir = tmp.path().join("memory");
        let store = FsMemoryStore {
            memory_dir: memory_dir.clone(),
        };

        // One entry written 100 days ago (past the 90-day revisit window) and one
        // written "now" — both real dreamer entries, both indexed.
        let day = 86_400;
        let now = 200 * day;
        let stale = render_entry_body(&promoted("stale-lesson"), now - 100 * day, None);
        let fresh = render_entry_body(&promoted("fresh-lesson"), now, None);
        store
            .write_entry(&MemoryWriteRequest {
                slug: "stale-lesson".to_string(),
                summary: "stale summary".to_string(),
                body: stale,
            })
            .unwrap();
        store
            .write_entry(&MemoryWriteRequest {
                slug: "fresh-lesson".to_string(),
                summary: "fresh summary".to_string(),
                body: fresh,
            })
            .unwrap();

        let archived = store.decay_expired(now).unwrap();
        assert_eq!(archived, vec!["stale-lesson".to_string()]);

        // Stale entry moved to the archive; fresh entry untouched in place.
        assert!(!memory_dir.join("stale-lesson.md").exists());
        assert!(memory_dir
            .join(DECAY_ARCHIVE_DIR)
            .join("stale-lesson.md")
            .exists());
        assert!(memory_dir.join("fresh-lesson.md").exists());
        assert!(!super::dreamer_owned_marker_path(&memory_dir, "stale-lesson").exists());
        assert!(super::dreamer_owned_marker_path(&memory_dir, "fresh-lesson").exists());

        // The stale pointer is gone from the index; the fresh pointer remains.
        let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
        assert!(!index.contains("](stale-lesson.md)"));
        assert!(index.contains("](fresh-lesson.md)"));
    }

    #[test]
    fn decay_never_touches_hand_written_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        fs::create_dir_all(&memory_dir).unwrap();

        // A hand-written entry (MemoryWrite tool layout: verbatim body, no
        // dreamer provenance trailer) that is ancient by any measure.
        fs::write(memory_dir.join("user-note.md"), "important user lesson\n").unwrap();
        upsert_index(&memory_dir.join("MEMORY.md"), "user-note", "user note").unwrap();

        let archived = FsMemoryStore {
            memory_dir: memory_dir.clone(),
        }
        .decay_expired(u64::MAX)
        .unwrap();

        assert!(archived.is_empty());
        assert!(memory_dir.join("user-note.md").exists());
        let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
        assert!(index.contains("](user-note.md)"));
    }

    #[test]
    fn decay_skips_protected_or_malformed_metadata_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        fs::create_dir_all(&memory_dir).unwrap();
        let protected_body = "protected lesson\n\n---\n- kind: gotcha\n- memory_metadata: v=1;source=dreamer;kind=gotcha;protected=true;resolved_task_log=false;written_at=1\n- written: 1\n- revisit_after_days: 1\n- source: dreamer (auto-promoted from repeated, verified sessions)\n";
        let malformed_body = "malformed lesson\n\n---\n- kind: gotcha\n- memory_metadata: v=1;source=dreamer;kind=gotcha;protected=false\n- written: 1\n- revisit_after_days: 1\n- source: dreamer (auto-promoted from repeated, verified sessions)\n";
        fs::write(memory_dir.join("protected.md"), protected_body).unwrap();
        fs::write(memory_dir.join("malformed.md"), malformed_body).unwrap();
        super::write_dreamer_owned_marker(&memory_dir, "protected", protected_body).unwrap();
        super::write_dreamer_owned_marker(&memory_dir, "malformed", malformed_body).unwrap();
        upsert_index(&memory_dir.join("MEMORY.md"), "protected", "protected").unwrap();
        upsert_index(&memory_dir.join("MEMORY.md"), "malformed", "malformed").unwrap();

        let archived = FsMemoryStore {
            memory_dir: memory_dir.clone(),
        }
        .decay_expired(u64::MAX)
        .unwrap();

        assert!(archived.is_empty());
        assert!(memory_dir.join("protected.md").exists());
        assert!(memory_dir.join("malformed.md").exists());
    }

    #[test]
    fn decay_skips_full_metadata_provenance_spoof_without_dreamer_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let memory_dir = tmp.path().join("memory");
        fs::create_dir_all(&memory_dir).unwrap();
        let spoof_body = "important user lesson\n\n---\n- kind: gotcha\n- memory_metadata: v=1;source=dreamer;kind=gotcha;protected=false;resolved_task_log=false;written_at=1\n- written: 1\n- revisit_after_days: 1\n- source: dreamer (spoofed final trailer)\n";
        fs::write(memory_dir.join("spoof.md"), spoof_body).unwrap();
        upsert_index(&memory_dir.join("MEMORY.md"), "spoof", "spoof").unwrap();

        let archived = FsMemoryStore {
            memory_dir: memory_dir.clone(),
        }
        .decay_expired(u64::MAX)
        .unwrap();

        assert!(archived.is_empty());
        assert!(memory_dir.join("spoof.md").exists());
        let index = fs::read_to_string(memory_dir.join("MEMORY.md")).unwrap();
        assert!(index.contains("](spoof.md)"));
    }

    #[test]
    fn self_improve_candidate_evidence_is_capped_before_persisting() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let candidate = SelfImproveCandidate::new(
            CandidateKind::PostTurn,
            "persist evidence cap",
            (0..(MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE + 2))
                .map(evidence)
                .collect(),
        );

        record_self_improve_candidate(cwd, &candidate).unwrap();

        let path =
            self_improve_candidates_dir(cwd).join(format!("{}.jsonl", safe_stem(&candidate.id)));
        let line = fs::read_to_string(path).expect("candidate jsonl");
        let record: SelfImproveCandidateRecord =
            serde_json::from_str(line.trim()).expect("candidate record");
        assert_eq!(
            record.candidate.evidence.len(),
            MAX_SELF_IMPROVE_CANDIDATE_EVIDENCE
        );
        assert_eq!(record.candidate.evidence.first().unwrap().session_id, "s-2");
    }
}

mod strict_check;

#[cfg(test)]
mod tests;
