//! Durable, append-only turn trace — externalizing the agentic loop's state.
//!
//! The agentic-loop-program doc (§2-5, "Harness-1: state lives outside the
//! context window") argues that an agent's actions, observations, and outcomes
//! should be recorded in durable external state, not held only in the
//! conversation that auto-compaction will later trim. The workflow engine
//! already does this for `Workflow` runs (`workflow_tools::event_store`); the
//! ordinary turn loop did not — its per-turn record went only to the ephemeral
//! OTLP tracer, so once a turn scrolled out of context and was compacted, the
//! fact that it happened was gone.
//!
//! This module closes that gap for the general loop with the *same* shape the
//! rest of zo uses for external state: an append-only JSONL log under
//! `.zo/turns/`, one [`TurnRecord`] per line, best-effort and lossy on read
//! (a corrupt line is skipped, never fatal). It is deliberately tiny — what
//! happened, how it ended, what it touched — not a full transcript: the
//! transcript is the model's job, this is the *audit trail*.
//!
//! It also feeds the Dreamer (`crate::memory::dreamer`): [`read_all_digests`]
//! maps every session's turn log into the curation brain's `TurnDigest`, and
//! `TurnLogLessonSource` mines recurring tool failures from it. So the durable
//! turn log is a real cross-session signal source, and curation no longer
//! depends solely on the deep-gate producer.
//!
//! Following zo's pure-brain ↔ IO-seam convention, [`TurnRecord`] and
//! [`TurnRecord::from_summary`] are pure and unit-tested; only [`append`] and
//! [`read_session`] touch the filesystem.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use core_types::paths::ZO_DIR_NAME;

use serde::{Deserialize, Serialize};

use crate::conversation::TurnSummary;
use crate::jsonl_log::{jsonl_files_newest_first, jsonl_files_oldest_first, prune_jsonl_lines};
use crate::session::ContentBlock;

/// Directory under `.zo/` holding append-only per-session turn logs.
const TURNS_DIR: &str = "turns";

#[cfg(test)]
const MAX_TURN_LOG_LINES: usize = 4;
#[cfg(not(test))]
const MAX_TURN_LOG_LINES: usize = 20_000;

#[cfg(test)]
const MAX_TURN_LOG_FILES: usize = 4;
#[cfg(not(test))]
const MAX_TURN_LOG_FILES: usize = 256;

#[cfg(test)]
const MAX_TURN_DIGESTS: usize = 8;
#[cfg(not(test))]
const MAX_TURN_DIGESTS: usize = 10_000;

/// How a turn ended. Kept coarse on purpose: the trace records the *shape* of
/// the outcome (did it finish, fail, or get cancelled), not error specifics —
/// which belong in logs, not the durable audit trail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The turn ran to completion (the model stopped on its own terms).
    Completed,
    /// The turn ended in a runtime error.
    Failed,
    /// The turn was cancelled by the host/user (dropped receiver, abandoned
    /// permission prompt).
    Cancelled,
}

impl TurnOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One externalized turn: the durable, compaction-proof record of a single
/// runtime turn. Serialized as one JSON line in the session's turn log.
///
/// This is an *audit* record, not a transcript: it captures what the turn did
/// (tool names, counts, outcome, token cost) so later passes — the Dreamer, a
/// `/turns` review, a debugging session — can reconstruct what happened without
/// the original context, which compaction may have discarded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnRecord {
    /// The session this turn belongs to (groups a session's turns; matches the
    /// Dreamer's cross-session key).
    pub session_id: String,
    /// Monotonic turn index within the session, starting at 0.
    pub seq: u64,
    /// Unix-milliseconds when the record was written.
    pub ts_ms: u64,
    /// How the turn ended.
    pub outcome: TurnOutcome,
    /// Number of model round-trips inside the turn (the agentic sub-loop depth).
    pub iterations: usize,
    /// Distinct tool names invoked this turn, in first-seen order. The
    /// high-signal "what did the agent actually do" summary.
    pub tools_used: Vec<String>,
    /// Total tool results produced (including repeats of the same tool).
    pub tool_result_count: usize,
    /// How many tool results were errors — the turn's friction signal.
    pub tool_error_count: usize,
    /// Distinct tool names that produced an error this turn, in first-seen
    /// order. The attributable half of `tool_error_count`: *which* tools failed,
    /// so the Dreamer can promote a recurring-failure gotcha keyed on the tool.
    /// `#[serde(default)]` keeps older logs (written before this field existed)
    /// readable — they decode with an empty list, never failing the parse.
    #[serde(default)]
    pub error_tools: Vec<String>,
    /// Distinct file paths mutated by edit/write tools this turn, in first-seen
    /// order. The durable, compaction-proof record of *what this turn actually
    /// changed on disk* — the signal that lets a later pass (or the
    /// post-compaction reminder) know an edit was already applied, so the model
    /// does not re-apply or revert its own prior change once the diff scrolls
    /// out of the context window. `#[serde(default)]` keeps older logs (written
    /// before this field existed) readable: they decode with an empty list.
    #[serde(default)]
    pub files_edited: Vec<String>,
    /// Git HEAD transitions observed during this turn's bash tool calls — the
    /// durable, compaction-proof record of *which commits this session
    /// created* (see [`crate::commit_ledger`]). Observation-based (before/after
    /// OID snapshots around each foreground command), so rebases, merges, and
    /// resets register the same as plain commits. `#[serde(default)]` keeps
    /// older logs readable: they decode with an empty list.
    #[serde(default)]
    pub head_transitions: Vec<crate::commit_ledger::HeadTransition>,
    /// Cumulative output tokens at turn end (cost/throughput signal).
    pub output_tokens: u32,
    /// The session goal in effect, if one was set (`/goal`). Lets a later pass
    /// tie turns to the objective they served.
    pub goal: Option<String>,
}

impl TurnRecord {
    /// Build a durable record from a completed turn's [`TurnSummary`] (pure).
    ///
    /// Extracts only the audit-level signal: which tools ran (deduplicated,
    /// first-seen order), how many results and errors there were, and the token
    /// cost. `seq` and `goal` are supplied by the caller (the runtime owns the
    /// turn counter and the session goal); `ts_ms` is stamped here so the record
    /// is self-dating.
    #[must_use]
    pub fn from_summary(
        session_id: &str,
        seq: u64,
        summary: &TurnSummary,
        goal: Option<&str>,
    ) -> Self {
        let mut tools_used: Vec<String> = Vec::new();
        let mut error_tools: Vec<String> = Vec::new();
        let mut tool_error_count = 0;
        for message in &summary.tool_results {
            for block in &message.blocks {
                if let ContentBlock::ToolResult {
                    tool_name,
                    is_error,
                    ..
                } = block
                {
                    if !tools_used.iter().any(|name| name == tool_name) {
                        tools_used.push(tool_name.clone());
                    }
                    if *is_error {
                        tool_error_count += 1;
                        // Track *which* tools failed, deduplicated in first-seen
                        // order — the signal the Dreamer keys recurring-failure
                        // gotchas on.
                        if !error_tools.iter().any(|name| name == tool_name) {
                            error_tools.push(tool_name.clone());
                        }
                    }
                }
            }
        }
        Self {
            session_id: session_id.to_string(),
            seq,
            ts_ms: now_ms(),
            outcome: TurnOutcome::Completed,
            iterations: summary.iterations,
            tools_used,
            tool_result_count: summary.tool_results.len(),
            tool_error_count,
            error_tools,
            // Externalize *which* files this turn changed (Harness-1: state
            // lives outside the context window): parsed from the edit/write
            // result envelopes, deduped in first-seen order. Survives
            // compaction, which would otherwise erase the only record that the
            // edit was applied.
            files_edited: crate::edited_file_paths(&summary.tool_results),
            // Filled by `record_completed` from the commit ledger's pending
            // buffer — not here, which stays pure (and the buffer must only
            // shrink after the append that persisted it succeeded).
            head_transitions: Vec::new(),
            output_tokens: summary.usage.output_tokens,
            goal: goal.map(str::to_string),
        }
    }

    /// Build a minimal record for a turn that ended without a summary (a failure
    /// or cancellation), capturing only identity and outcome (pure).
    #[must_use]
    pub fn terminal(
        session_id: &str,
        seq: u64,
        outcome: TurnOutcome,
        iterations: usize,
        goal: Option<&str>,
    ) -> Self {
        Self {
            session_id: session_id.to_string(),
            seq,
            ts_ms: now_ms(),
            outcome,
            iterations,
            tools_used: Vec::new(),
            tool_result_count: 0,
            tool_error_count: 0,
            error_tools: Vec::new(),
            files_edited: Vec::new(),
            // Filled by `record_terminal` (a failed/cancelled turn may still
            // have committed before it died); constructor stays pure.
            head_transitions: Vec::new(),
            output_tokens: 0,
            goal: goal.map(str::to_string),
        }
    }
}

/// Unix-milliseconds now, saturating to 0 before the epoch.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// Resolve the append-only log path for a session under `<cwd>/.zo/turns/`.
///
/// `cwd` is already the authoritative trace root: callers pass
/// `DeepGate::trace_cwd()`, the single resolver that honors `ZO_TRACE_ROOT`
/// (and the stable workspace cwd for the TUI / `zo serve`). `turn_trace` must
/// not second-guess it with a competing project-state resolver, or traces and the
/// dream log (which also roots at `trace_cwd`) would split.
fn log_path(cwd: &Path, session_id: &str) -> PathBuf {
    let stem: String = session_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let stem = if stem.is_empty() { "session" } else { &stem };
    crate::traces_base(cwd).join(ZO_DIR_NAME)
        .join(TURNS_DIR)
        .join(format!("{stem}.jsonl"))
}

/// Append one turn record to its session log under `<cwd>/.zo/turns/`.
///
/// Append-only and crash-safe: exactly one JSON line per call, so a concurrent
/// or interrupted turn can never corrupt earlier records. Best-effort by
/// contract — recording the audit trail must never fail or slow a turn — so the
/// runtime calls this and ignores the result.
///
/// # Errors
/// Returns the underlying [`std::io::Error`] if the directory cannot be created
/// or the line cannot be appended.
pub fn append(cwd: &Path, record: &TurnRecord) -> std::io::Result<()> {
    let path = log_path(cwd, &record.session_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
        // Turn traces record the full prompt/response audit trail, so keep the
        // directory and file owner-only — best-effort, so this audit aid never
        // fails the write (the append itself is best-effort by contract).
        let _ = core_types::paths::restrict_permissions_owner_only(parent);
    }
    let mut line = serde_json::to_string(record).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    let _ = core_types::paths::restrict_permissions_owner_only(&path);
    file.write_all(line.as_bytes())?;
    // Retention is best-effort: the trace is an audit aid and must never make
    // the live turn fail after the append succeeded. Sequence assignment remains
    // monotonic because `next_seq` streams the retained records and uses the
    // highest persisted `seq` as well as physical line count.
    let _ = prune_jsonl_lines(&path, MAX_TURN_LOG_LINES);
    let _ = prune_turn_log_files(cwd, MAX_TURN_LOG_FILES);
    Ok(())
}

/// Read all turn records for one session, in log (append) order. Lossy: a line
/// that fails to parse is skipped, never fatal — the same tolerance the workflow
/// event-log reader uses. A missing log reads as empty.
#[must_use]
pub fn read_session(cwd: &Path, session_id: &str) -> Vec<TurnRecord> {
    let path = log_path(cwd, session_id);
    let Ok(file) = File::open(&path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .filter_map(|line| serde_json::from_str::<TurnRecord>(&line).ok())
        .collect()
}

/// Most-recently edited file paths the durable turn trace can re-assert after
/// compaction, newest-first and deduplicated.
const MAX_REINJECTED_EDITED_FILES: usize = 30;

/// Distinct file paths this session has already edited, read from the durable
/// turn trace, ordered most-recently-edited first and capped at
/// `MAX_REINJECTED_EDITED_FILES`. Best-effort: a missing or partially-pruned
/// log simply yields fewer (or no) paths.
///
/// This is the durable, compaction-proof answer to "what has this session
/// already changed on disk?" — sourced from the externalized trace rather than
/// the conversation, so it survives the very compaction that erases the inline
/// edit diffs. Retention (`MAX_TURN_LOG_LINES`) bounds how far back it sees,
/// which is the right trade: the reminder names recent edits, not the entire
/// session's history.
#[must_use]
pub fn session_edited_files(cwd: &Path, session_id: &str) -> Vec<String> {
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut ordered: Vec<String> = Vec::new();
    // Newest record first so the most-recently-edited files lead the list.
    for record in read_session(cwd, session_id).into_iter().rev() {
        for path in record.files_edited {
            if seen.insert(path.clone()) {
                ordered.push(path);
                if ordered.len() >= MAX_REINJECTED_EDITED_FILES {
                    return ordered;
                }
            }
        }
    }
    ordered
}

/// Post-compaction reminder that history the model no longer remembers is its
/// own work. Emitted unconditionally by every compaction round.
///
/// Compaction summarizes away the tool results that proved *this* session made a
/// commit. [`render_edited_files_reminder`] covers files, but nothing covered
/// history, so on resume `git log` shows commits the model has no context for
/// and it reasons about them as another session's — then plans around its own
/// changes instead of owning them. Observed: five commits from one session were
/// attributed to a phantom concurrent session across several turns.
///
/// Unconditional on purpose. Unlike the file list this has nothing to enumerate,
/// and the case it must cover — a session whose commits are the only trace left
/// — is exactly the case where a file list would be empty and a gated reminder
/// would stay silent. Names the check rather than urging caution, because the
/// data was never missing: `git log` was always available, the instruction to
/// consult it was not.
pub const HISTORY_ATTRIBUTION_REMINDER: &str = "[system: Commits and edits already in this session are YOUR OWN work (state preserved across compaction). History you do not remember was almost certainly made earlier in this same session and summarized away — confirm with `git log --format='%h %an %ar %s'` before treating it as another session's, and do not plan around it as foreign.]";

/// Stable opening shared by BOTH history-attribution forms (the enumerated
/// ledger and the no-ledger fallback [`HISTORY_ATTRIBUTION_REMINDER`]) — the
/// dedup key compaction uses to replace (not stack) the previous round's copy.
pub const HISTORY_ATTRIBUTION_REMINDER_PREFIX: &str = "[system: Commits";

/// True when `prompt` is a history-attribution reminder in either form.
#[must_use]
pub fn is_history_attribution_reminder(prompt: &str) -> bool {
    prompt
        .trim_start()
        .starts_with(HISTORY_ATTRIBUTION_REMINDER_PREFIX)
}

/// Render the post-compaction history-attribution reminder. With a non-empty
/// ledger this ENUMERATES the session's own commits (observed HEAD
/// transitions, see [`crate::commit_ledger`]) instead of the blanket "history
/// you do not remember is yours" claim — which mis-attributes in both
/// directions once several sessions share one repository: the model either
/// re-verifies its own commits as foreign (no enumeration to check against)
/// or claims another session's commits as its own (the blanket claim). Falls
/// back to [`HISTORY_ATTRIBUTION_REMINDER`] when nothing was observed
/// (non-git workspace, or a session that never moved HEAD).
#[must_use]
pub fn render_history_attribution_reminder(
    transitions: &[crate::commit_ledger::HeadTransition],
) -> String {
    if transitions.is_empty() {
        return HISTORY_ATTRIBUTION_REMINDER.to_string();
    }
    let mut out = String::from(
        "[system: Commits THIS session created — HEAD transitions observed during this \
         session's own tool calls (state preserved across compaction; oldest first):\n",
    );
    for transition in transitions {
        let old = transition
            .old
            .as_deref()
            .map_or("(root)", |oid| oid.get(..8).unwrap_or(oid));
        let new = transition.new.get(..8).unwrap_or(&transition.new);
        out.push_str("- ");
        out.push_str(old);
        out.push_str(" → ");
        out.push_str(new);
        if !transition.subject.is_empty() {
            out.push(' ');
            out.push_str(&transition.subject);
        }
        out.push('\n');
    }
    out.push_str(
        "These are your own work — do not re-verify or plan around them as another \
         session's. A commit NOT listed may still be yours (observation is best-effort) \
         or may be the user's or another session's: confirm with \
         `git log --format='%h %an %ar %s'` before assuming either way.]",
    );
    out
}

/// Stable opening of the edited-files reminder — the dedup key compaction uses
/// to replace (not stack) the previous round's list across repeated rounds,
/// since the list contents grow as the session edits more files.
pub const EDITED_FILES_REMINDER_PREFIX: &str = "[system: Files this session has ALREADY edited";

/// True when `prompt` is an edited-files reminder produced by
/// [`render_edited_files_reminder`].
#[must_use]
pub fn is_edited_files_reminder(prompt: &str) -> bool {
    prompt.trim_start().starts_with(EDITED_FILES_REMINDER_PREFIX)
}

/// Render the session's already-edited file list as a post-compaction system
/// reminder, or `None` when nothing has been edited yet. Pure (no IO): the
/// caller supplies the paths from [`session_edited_files`].
///
/// Compaction summarizes away the `edit_file`/`write_file` tool-results that
/// proved a change was applied; without this the model, on resume, re-reads a
/// file, sees an edit it no longer has context for, and "fixes" it back — the
/// self-revert failure on long sessions. Naming the files (not the diffs) is
/// enough to stop that: it tells the model "you already changed these; do not
/// redo or revert them — re-read to confirm before touching."
#[must_use]
pub fn render_edited_files_reminder(files: &[String]) -> Option<String> {
    if files.is_empty() {
        return None;
    }
    let mut out = format!(
        "{EDITED_FILES_REMINDER_PREFIX} (state preserved across compaction). \
These changes are applied on disk — do not redo or revert them; re-read a file to confirm \
its current contents before editing again.]\n# Files already edited this session\n",
    );
    for path in files {
        out.push_str("- ");
        out.push_str(path);
        out.push('\n');
    }
    Some(out.trim_end().to_string())
}

/// Next sequence number for a session: the count of records already in its log.
/// Turns within a session are strictly sequential (the loop never runs two at
/// once), so this is a race-free monotonic counter without any in-memory state —
/// which is the whole point of externalizing the turn trace.
///
/// Counts non-empty lines directly rather than deserializing every record, so a
/// long session's per-turn cost stays O(file size) instead of O(records · parse)
/// — the append path must not get quadratically slower as the log grows.
fn next_seq(cwd: &Path, session_id: &str) -> u64 {
    let path = log_path(cwd, session_id);
    let Ok(file) = File::open(&path) else {
        return 0;
    };
    let mut physical_records = 0u64;
    let mut next_after_max_seq = 0u64;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        physical_records = physical_records.saturating_add(1);
        if let Ok(record) = serde_json::from_str::<TurnRecord>(line) {
            next_after_max_seq = next_after_max_seq.max(record.seq.saturating_add(1));
        }
    }
    physical_records.max(next_after_max_seq)
}

/// Externalize one completed turn: assign the next session-local `seq`, build the
/// audit record from the turn summary, and append it. Best-effort and
/// side-effect-only — the runtime calls this from its existing
/// `record_turn_completed` seam and ignores the result, so a recording failure
/// can never affect the turn. Returns the appended record on success (handy for
/// callers that want to surface or test it).
#[must_use]
pub fn record_completed(
    cwd: &Path,
    session_id: &str,
    summary: &TurnSummary,
    goal: Option<&str>,
) -> Option<TurnRecord> {
    let mut record = TurnRecord::from_summary(session_id, next_seq(cwd, session_id), summary, goal);
    // Externalize the commits this turn created (same rationale as
    // `files_edited`: after compaction this record is the only proof the HEAD
    // movement was this session's own work). Snapshot-then-confirm, so a
    // failed append leaves the entries pending for the next turn's record
    // instead of losing them.
    record.head_transitions = crate::commit_ledger::pending_snapshot(session_id);
    let persisted = record.head_transitions.len();
    append(cwd, &record).ok().map(|()| {
        crate::commit_ledger::confirm_persisted(session_id, persisted);
        record
    })
}

/// Externalize a turn that ended without a summary (a failure or cancellation).
/// Best-effort, same contract as [`record_completed`].
#[must_use]
pub fn record_terminal(
    cwd: &Path,
    session_id: &str,
    outcome: TurnOutcome,
    iterations: usize,
    goal: Option<&str>,
) -> Option<TurnRecord> {
    let mut record = TurnRecord::terminal(
        session_id,
        next_seq(cwd, session_id),
        outcome,
        iterations,
        goal,
    );
    // Same snapshot-then-confirm as `record_completed`: commits made before
    // the turn failed/cancelled are no less this session's work.
    record.head_transitions = crate::commit_ledger::pending_snapshot(session_id);
    let persisted = record.head_transitions.len();
    append(cwd, &record).ok().map(|()| {
        crate::commit_ledger::confirm_persisted(session_id, persisted);
        record
    })
}

/// Read every session's turn records under `<cwd>/.zo/turns/`, mapping each
/// to the Dreamer's portable `TurnDigest`. Best-effort and lossy: an
/// unreadable directory or a corrupt line is skipped, never fatal. A turn with
/// no errored tools still yields a digest (with an empty `error_tools`), so the
/// brain — not this IO seam — owns the "is there a lesson here?" decision.
///
/// This is the bridge that lets the Dreamer mine the externalized turn trace
/// (recurring tool failures) alongside the deep-gate green-accept producer, so
/// curation no longer depends on a single signal source.
#[must_use]
pub fn read_all_digests(cwd: &Path) -> Vec<decision_core::dreamer::TurnDigest> {
    // Same root as the writer (`log_path`): the caller-resolved `trace_cwd`.
    let dir = crate::traces_base(cwd).join(ZO_DIR_NAME).join(TURNS_DIR);
    let files = jsonl_files_oldest_first(&dir, MAX_TURN_LOG_FILES);
    let mut digests = VecDeque::new();
    for path in files {
        let Ok(file) = File::open(&path) else {
            continue;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(record) = serde_json::from_str::<TurnRecord>(line) {
                push_capped(
                    &mut digests,
                    decision_core::dreamer::TurnDigest {
                        session_id: record.session_id,
                        error_tools: record.error_tools,
                    },
                    MAX_TURN_DIGESTS,
                );
            }
        }
    }
    digests.into_iter().collect()
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

fn prune_turn_log_files(cwd: &Path, keep_files: usize) -> std::io::Result<()> {
    let dir = crate::traces_base(cwd).join(ZO_DIR_NAME).join(TURNS_DIR);
    let files = jsonl_files_newest_first(&dir);
    for path in files.into_iter().skip(keep_files) {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{append, log_path, read_session, record_completed, TurnOutcome, TurnRecord};
    use crate::conversation::TurnSummary;
    use crate::session::{ContentBlock, ConversationMessage, MessageRole};
    use core_types::usage::TokenUsage;

    fn tool_result(tool_name: &str, is_error: bool) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "id".to_string(),
                tool_name: tool_name.to_string(),
                output: "out".to_string(),
                is_error,
                images: Vec::new(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        }
    }

    fn summary(tool_results: Vec<ConversationMessage>, output_tokens: u32) -> TurnSummary {
        TurnSummary {
            assistant_messages: Vec::new(),
            tool_results,
            prompt_cache_events: Vec::new(),
            iterations: 2,
            usage: TokenUsage {
                output_tokens,
                ..TokenUsage::default()
            },
            turn_output_tokens: output_tokens,
            auto_compaction: None,
            microcompact: None,
            deep_verification: None,
            verification_issues: Vec::new(),
            deep_verifier_parse: None,
            deep_verifier_model: None,
            budget_exhausted: None,
        }
    }

    /// A user message holding one `edit_file` result mutating `path`.
    fn edit_result(path: &str) -> ConversationMessage {
        ConversationMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "edit".to_string(),
                tool_name: "edit_file".to_string(),
                output: serde_json::json!({ "filePath": path }).to_string(),
                is_error: false,
                images: Vec::new(),
            }],
            usage: None,
            thought_signature: None,
            reasoning_replay: None,
                    model: None,
        }
    }

    #[test]
    fn from_summary_extracts_distinct_tools_and_error_count() {
        let s = summary(
            vec![
                tool_result("bash", false),
                tool_result("read_file", true),
                tool_result("bash", false), // repeat → deduped in tools_used
            ],
            128,
        );
        let record = TurnRecord::from_summary("sess-1", 0, &s, Some("ship the feature"));

        assert_eq!(record.session_id, "sess-1");
        assert_eq!(record.outcome, TurnOutcome::Completed);
        assert_eq!(record.iterations, 2);
        // Distinct, first-seen order.
        assert_eq!(record.tools_used, vec!["bash", "read_file"]);
        assert_eq!(record.tool_result_count, 3);
        assert_eq!(record.tool_error_count, 1);
        // Only the tool that errored is attributed, deduped in first-seen order.
        assert_eq!(record.error_tools, vec!["read_file"]);
        // No edit/write tool ran, so nothing is recorded as edited.
        assert!(record.files_edited.is_empty());
        assert_eq!(record.output_tokens, 128);
        assert_eq!(record.goal.as_deref(), Some("ship the feature"));
    }

    #[test]
    fn terminal_record_is_minimal() {
        let record = TurnRecord::terminal("s", 4, TurnOutcome::Cancelled, 1, None);
        assert_eq!(record.outcome, TurnOutcome::Cancelled);
        assert_eq!(record.seq, 4);
        assert!(record.tools_used.is_empty());
        assert_eq!(record.tool_result_count, 0);
        assert!(record.files_edited.is_empty());
        assert!(record.goal.is_none());
    }

    #[test]
    fn from_summary_records_distinct_edited_files_in_first_seen_order() {
        let s = summary(
            vec![
                edit_result("src/a.rs"),
                tool_result("read_file", false), // not a mutation → ignored
                edit_result("src/b.rs"),
                edit_result("src/a.rs"), // duplicate → deduped
            ],
            10,
        );
        let record = TurnRecord::from_summary("sess", 0, &s, None);
        assert_eq!(record.files_edited, vec!["src/a.rs", "src/b.rs"]);
    }

    #[test]
    fn files_edited_survives_jsonl_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let s = summary(vec![edit_result("crates/x/src/lib.rs")], 5);
        record_completed(cwd, "sess", &s, None).unwrap();

        let records = read_session(cwd, "sess");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].files_edited, vec!["crates/x/src/lib.rs"]);
    }

    #[test]
    fn old_log_without_files_edited_field_still_parses() {
        // A record written before `files_edited` existed (the field is absent)
        // must decode with an empty list, never failing the parse.
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let path = log_path(cwd, "sess");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "{\"session_id\":\"sess\",\"seq\":0,\"ts_ms\":1,\"outcome\":\"completed\",\
\"iterations\":1,\"tools_used\":[\"edit_file\"],\"tool_result_count\":1,\
\"tool_error_count\":0,\"output_tokens\":7}\n",
        )
        .unwrap();

        let records = read_session(cwd, "sess");
        assert_eq!(records.len(), 1);
        assert!(records[0].files_edited.is_empty());
    }

    #[test]
    fn session_edited_files_dedupes_newest_first_across_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        // Turn 0 edits a.rs; turn 1 edits b.rs then re-edits a.rs.
        record_completed(cwd, "sess", &summary(vec![edit_result("a.rs")], 1), None).unwrap();
        record_completed(
            cwd,
            "sess",
            &summary(vec![edit_result("b.rs"), edit_result("a.rs")], 1),
            None,
        )
        .unwrap();

        // Newest-first, deduplicated: turn 1's b.rs and a.rs lead; turn 0's
        // a.rs is already seen and not repeated.
        assert_eq!(
            super::session_edited_files(cwd, "sess"),
            vec!["b.rs".to_string(), "a.rs".to_string()]
        );
    }

    #[test]
    fn render_edited_files_reminder_lists_files_or_none() {
        assert!(super::render_edited_files_reminder(&[]).is_none());

        let reminder =
            super::render_edited_files_reminder(&["src/a.rs".to_string(), "src/b.rs".to_string()])
                .expect("non-empty list renders");
        assert!(reminder.contains("# Files already edited this session"));
        assert!(reminder.contains("do not redo or revert them"));
        assert!(reminder.contains("- src/a.rs"));
        assert!(reminder.contains("- src/b.rs"));
    }

    #[test]
    fn history_attribution_reminder_names_the_authorship_check() {
        let reminder = super::HISTORY_ATTRIBUTION_REMINDER;
        // Both halves are load-bearing. The ownership claim is what stops the
        // model treating its own summarized-away commits as another session's;
        // the concrete command is what settles it. A caution without the command
        // is what failed before — `git log` was always available, the
        // instruction to consult it was not.
        assert!(reminder.contains("YOUR OWN work"));
        assert!(reminder.contains("git log --format="));
        assert!(reminder.contains("preserved across compaction"));
    }

    #[test]
    fn head_transitions_survive_jsonl_round_trip_and_old_logs_parse_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let mut record = TurnRecord::terminal("sess", 0, TurnOutcome::Completed, 1, None);
        record.head_transitions = vec![crate::commit_ledger::HeadTransition {
            old: Some("a".repeat(40)),
            new: "b".repeat(40),
            subject: "bbbbbbbb feat: thing".into(),
        }];
        super::append(cwd, &record).unwrap();

        let records = read_session(cwd, "sess");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].head_transitions, record.head_transitions);

        // Old-log tolerance rides the same `#[serde(default)]` contract as
        // `files_edited`: `old_log_without_files_edited_field_still_parses`
        // exercises a record with neither field present.
    }

    #[test]
    fn render_history_attribution_reminder_enumerates_or_falls_back() {
        // Empty ledger → the blanket fallback, verbatim.
        assert_eq!(
            super::render_history_attribution_reminder(&[]),
            super::HISTORY_ATTRIBUTION_REMINDER
        );

        let rendered = super::render_history_attribution_reminder(&[
            crate::commit_ledger::HeadTransition {
                old: None,
                new: "c0ffee00".repeat(5),
                subject: "c0ffee00 root: init".into(),
            },
            crate::commit_ledger::HeadTransition {
                old: Some("c0ffee00".repeat(5)),
                new: "deadbeef".repeat(5),
                subject: "deadbeef fix: thing".into(),
            },
        ]);
        // Enumerated form: short OIDs, root marker, subjects, and the
        // both-directions caution (own work vs possibly-foreign commits).
        assert!(rendered.contains("(root) → c0ffee00"));
        assert!(rendered.contains("c0ffee00 → deadbeef"));
        assert!(rendered.contains("fix: thing"));
        assert!(rendered.contains("your own work"));
        assert!(rendered.contains("git log --format="));
        // Both forms share the dedup prefix compaction replaces on.
        assert!(super::is_history_attribution_reminder(&rendered));
        assert!(super::is_history_attribution_reminder(
            super::HISTORY_ATTRIBUTION_REMINDER
        ));
        assert!(!super::is_history_attribution_reminder(
            "[system: Files this session has ALREADY edited]"
        ));
    }

    #[test]
    fn log_path_uses_caller_supplied_trace_root_without_project_state_rewrite() {
        let _env_lock = crate::test_env_lock(); // Writes process env: hold the crate env lock so a parallel test never sees it mid-change.

        let root = tempfile::tempdir().unwrap();
        let trace_root = root.path().join("explicit-trace-root");
        let state_root = root.path().join("state-root");
        std::env::set_var("ZO_STATE_DIR", &state_root);

        let path = log_path(&trace_root, "session-1");

        std::env::remove_var("ZO_STATE_DIR");
        assert!(path.starts_with(&trace_root));
        assert!(!path.starts_with(&state_root));
        assert!(path.ends_with(".zo/turns/session-1.jsonl"));
    }

    #[test]
    fn append_then_read_round_trips_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        for seq in 0..3 {
            let record = TurnRecord::terminal("sess-x", seq, TurnOutcome::Completed, 1, None);
            append(cwd, &record).unwrap();
        }
        // A different session is isolated.
        append(
            cwd,
            &TurnRecord::terminal("other", 0, TurnOutcome::Failed, 1, None),
        )
        .unwrap();

        let records = read_session(cwd, "sess-x");
        assert_eq!(records.len(), 3);
        assert_eq!(
            records.iter().map(|r| r.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(read_session(cwd, "other").len(), 1);
        assert!(read_session(cwd, "missing").is_empty());
    }

    #[test]
    fn record_completed_auto_increments_seq_from_the_log() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let s = summary(vec![tool_result("bash", false)], 10);

        // No in-memory counter: seq is derived from the durable log length, so
        // three calls produce 0,1,2 even across independent invocations.
        let r0 = super::record_completed(cwd, "sess", &s, None).unwrap();
        let r1 = super::record_completed(cwd, "sess", &s, None).unwrap();
        let r2 = super::record_completed(cwd, "sess", &s, None).unwrap();
        assert_eq!((r0.seq, r1.seq, r2.seq), (0, 1, 2));

        let records = read_session(cwd, "sess");
        assert_eq!(records.len(), 3);
        assert_eq!(records[2].tools_used, vec!["bash"]);
    }

    #[test]
    fn next_seq_counts_physical_lines_including_unparseable_ones() {
        // next_seq must count every physical record line, even one that
        // read_session would skip as corrupt: seq is the *append position*, and a
        // corrupt line still occupies a slot. Counting only parseable lines would
        // hand out a duplicate seq and overwrite history in spirit.
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        append(
            cwd,
            &TurnRecord::terminal("s", 0, TurnOutcome::Completed, 1, None),
        )
        .unwrap();
        let path = super::log_path(cwd, "s");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("{ corrupt\n\n"); // a corrupt line + a blank line
        std::fs::write(&path, content).unwrap();

        // read_session sees 1 valid record; next_seq counts 2 physical lines
        // (valid + corrupt), ignoring the blank — so the next append is seq 2.
        assert_eq!(read_session(cwd, "s").len(), 1);
        let next = super::record_completed(
            cwd,
            "s",
            &summary(vec![tool_result("bash", false)], 1),
            None,
        )
        .unwrap();
        assert_eq!(
            next.seq, 2,
            "seq skips past the corrupt line, never reuses it"
        );
    }

    #[test]
    fn read_skips_corrupt_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        append(
            cwd,
            &TurnRecord::terminal("s", 0, TurnOutcome::Completed, 1, None),
        )
        .unwrap();
        // Append a corrupt line directly.
        let path = super::log_path(cwd, "s");
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("{ not valid json\n");
        std::fs::write(&path, content).unwrap();

        let records = read_session(cwd, "s");
        assert_eq!(records.len(), 1, "the corrupt line is skipped, not fatal");
    }

    #[test]
    fn record_terminal_persists_failed_and_cancelled_outcomes() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        let failed = super::record_terminal(cwd, "sess", TurnOutcome::Failed, 3, Some("goal"))
            .expect("failed terminal record appended");
        let cancelled = super::record_terminal(cwd, "sess", TurnOutcome::Cancelled, 4, None)
            .expect("cancelled terminal record appended");

        assert_eq!(failed.seq, 0);
        assert_eq!(cancelled.seq, 1);
        let records = read_session(cwd, "sess");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].outcome, TurnOutcome::Failed);
        assert_eq!(records[0].goal.as_deref(), Some("goal"));
        assert_eq!(records[1].outcome, TurnOutcome::Cancelled);
    }

    #[test]
    fn retention_keeps_recent_lines_without_reusing_seq() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();

        for _ in 0..(super::MAX_TURN_LOG_LINES + 2) {
            super::record_terminal(cwd, "sess", TurnOutcome::Completed, 1, None).unwrap();
        }

        let records = read_session(cwd, "sess");
        assert_eq!(records.len(), super::MAX_TURN_LOG_LINES);
        assert_eq!(records.first().unwrap().seq, 2);
        assert_eq!(records.last().unwrap().seq, 5);

        let next = super::record_terminal(cwd, "sess", TurnOutcome::Failed, 1, None).unwrap();
        assert_eq!(
            next.seq, 6,
            "retention must not reset the monotonic sequence"
        );
    }
}
