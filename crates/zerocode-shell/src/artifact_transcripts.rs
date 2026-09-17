//! The transcript road into the artifact store (t-3233 §2): which files are
//! read, when, and how much of them.
//!
//! The pure half — what a line SAYS — is `zerocode_core::artifact_transcript`.
//! This half owns the two moments a transcript is read, and never a third:
//!
//! - **Boot backfill** ([`backfill`]): the roots the vendors write under
//!   (`~/.claude/projects`, the window home's `.claude/projects`,
//!   `~/.zo/sessions`), newest files first, bounded by the table
//!   (`transcript_files_max`, `transcript_bytes_max`) and incremental on a
//!   stamp file (`<store>/transcripts.json`: path → bytes already read), so
//!   the second boot reads only what was appended since the first. The
//!   backfill rides the reconcile thread beside the store's first scan — no
//!   thread, no poll of its own.
//! - **The hook road** ([`note_hook`]): a Claude or zo pane's `Stop` (and its
//!   `PostToolUse` of `Write` or `Artifact`) names `transcript_path`, and the
//!   tail past the remembered stamp is read then — bounded by
//!   `transcript_tail_bytes` when the file was never met. This IS the
//!   existing event; the transcript is not watched.
//!
//! What it registers: `Artifact` results as `Web` rows (one per url), and a
//! table extension under the project root as a page only when the writer's
//! own result says it CREATED the file (t-3952) — an edit of the project's
//! existing source is not an artifact. Claude's lines carry their own
//! `cwd`; zo's carry none, so the pane's `SessionStart` cwd is remembered per
//! session and stands in.
//!
//! The rule before t-3952 registered every `Write`/`Edit` call. The rows it
//! left are judged again on the boot road ([`rejudge_pass`]), a bounded pass
//! at a time, and only a row conclusively born of an edit is forgotten.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use zerocode_core::agent::AgentKind;
use zerocode_core::artifact::{Artifact, Limits};
use zerocode_core::artifact_transcript::{ExtractionState, Fact, Speaker, extract_incremental};
use zerocode_core::civil::epoch_ms_of_iso;
use zerocode_core::hook::{HookEnvelope, Phase, Tool};

use crate::artifact_runtime::Store;

/// The stamp file's name under the store root.
pub(crate) const STAMPS_FILE: &str = "transcripts.json";
/// The hook events that make this road read a transcript: a turn ending, or a
/// writing tool finishing. Spelled as `normalized_event` spells them.
const READING_EVENTS: &[&str] = &["stop", "posttooluse"];
/// Sessions whose `cwd` is remembered for zo's rootless lines.
const SESSION_ROOTS_MAX: usize = 256;
/// Rule 1 required `create`; rule 2 also checks other sessions before legacy
/// cleanup. Older session-local reviews restart with the complete history.
const PAGE_RULE: u32 = 2;
/// Claude's writer calls, whose `file_path` a legacy judgement follows.
const CLAUDE_WRITERS: &[&str] = &["Write", "Edit", "MultiEdit"];

/// Where one vendor's transcripts live, and which grammar they speak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Root {
    pub(crate) dir: PathBuf,
    pub(crate) speaker: Speaker,
}

/// The roots this machine's agents write under: Claude's under the person's
/// home AND under the window's agent home (the window launches Claude with
/// its own HOME), zo's sessions under the person's home.
pub(crate) fn roots(home: &Path, window_home: &Path) -> Vec<Root> {
    let mut roots = vec![Root {
        dir: home.join(".claude").join("projects"),
        speaker: Speaker::Claude,
    }];
    let window_claude = window_home.join(".claude").join("projects");
    if window_claude != roots[0].dir {
        roots.push(Root {
            dir: window_claude,
            speaker: Speaker::Claude,
        });
    }
    roots.push(Root {
        dir: home.join(".zo").join("sessions"),
        speaker: Speaker::Zo,
    });
    roots
}

/// How far into each transcript this store has read. `at_ms` is when, so the
/// map can shed its oldest entries past the table.
#[derive(Debug, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub(crate) struct Stamps {
    #[serde(default)]
    read: BTreeMap<PathBuf, ReadMark>,
    /// The [`PAGE_RULE`] the catalog's transcript rows were last judged by.
    #[serde(default)]
    page_rule: u32,
    /// Creation facts outlive read cursors, which may be evicted or skip a
    /// consumed transcript. Only paths still in the catalog are retained.
    #[serde(default)]
    created_pages: BTreeSet<PathBuf>,
    /// Rule 1's session-local judgements are deliberately not resumed: they
    /// did not inspect other sessions before removing a row.
    #[serde(default)]
    creation_review: Option<Rejudging>,
}

/// The catalog's legacy judgement in progress.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
struct Rejudging {
    /// All retained transcripts, including already consumed ones.
    unread: BTreeMap<PathBuf, u64>,
    /// Detect a rewrite, append, disappearance or new transcript during review.
    history: BTreeMap<PathBuf, HistoryStamp>,
    rows: BTreeMap<String, Evidence>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
struct HistoryStamp {
    bytes: u64,
    modified: std::time::SystemTime,
}

fn history_stamp(path: &Path) -> Option<HistoryStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(HistoryStamp {
        bytes: meta.len(),
        modified: meta.modified().ok()?,
    })
}

/// What retained transcripts say about one legacy row's file.
#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
struct Evidence {
    /// When the session first named the file in a writer call.
    first_touch_ms: Option<i64>,
    /// A result changed the file as it already was — an edit, or a `Write`
    /// that answered `update`.
    edited: bool,
    /// Unanswered writer calls cannot establish edit-only history. The next
    /// transcript is not read until these are resolved or the row is kept.
    #[serde(default)]
    pending: BTreeSet<String>,
    /// Something keeps the row whatever else is read: its creation, a touch
    /// this reader cannot classify, a transcript gone or unreadable.
    keep: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
struct ReadMark {
    bytes: u64,
    at_ms: i64,
    #[serde(default)]
    extraction: ExtractionState,
    #[serde(default)]
    project: Option<PathBuf>,
}

impl Stamps {
    pub(crate) fn load(store_root: &Path) -> Self {
        std::fs::read_to_string(store_root.join(STAMPS_FILE))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub(crate) fn save(&self, store_root: &Path) -> Result<(), String> {
        std::fs::create_dir_all(store_root).map_err(|error| error.to_string())?;
        let text = serde_json::to_string(self).map_err(|error| error.to_string())?;
        let tmp = store_root.join(format!("{STAMPS_FILE}.tmp"));
        std::fs::write(&tmp, text).map_err(|error| error.to_string())?;
        std::fs::rename(&tmp, store_root.join(STAMPS_FILE)).map_err(|error| error.to_string())
    }

    #[cfg(test)]
    fn read_to(&self, path: &Path) -> Option<u64> {
        self.read.get(path).map(|mark| mark.bytes)
    }

    fn retain_creations(&mut self, rows: &[Artifact]) {
        self.created_pages
            .retain(|path| rows.iter().any(|row| &row.path == path));
    }

    fn mark(&mut self, path: &Path, mark: ReadMark, cap: usize) {
        self.read.insert(path.to_path_buf(), mark);
        while self.read.len() > cap.max(1) {
            let Some(oldest) = self
                .read
                .iter()
                .min_by_key(|(_, mark)| mark.at_ms)
                .map(|(path, _)| path.clone())
            else {
                break;
            };
            self.read.remove(&oldest);
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.read.len()
    }
}

/// What one backfill pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BackfillReport {
    pub(crate) files: usize,
    pub(crate) bytes: u64,
    pub(crate) remote: usize,
    pub(crate) pages: usize,
    /// A bound of the table was reached before every transcript was read.
    pub(crate) truncated: bool,
    /// Legacy rows this pass found born of an edit and forgot (t-3952).
    pub(crate) forgotten: usize,
    /// Whether a history review is scheduled for another bounded pass (0/1).
    pub(crate) rejudging: usize,
}

impl BackfillReport {
    pub(crate) fn changed(&self) -> bool {
        self.remote + self.pages + self.forgotten > 0
    }
}

/// One read of a transcript's unread stretch.
struct Stretch {
    bytes: Vec<u8>,
    /// Where the read ended — the next stamp.
    read_to: u64,
    /// The read began past the mark (the bound cut it), so its first line
    /// may be a line's tail rather than a line.
    cut: bool,
    /// Earlier bytes no longer join this stretch (rewrite or bounded skip).
    reset: bool,
    read_bytes: u64,
}

/// The bytes of `path` from `from` to its end, bounded by `most`. Reads the
/// LAST `most` bytes when the unread stretch is longer — the newest lines are
/// the ones a gallery wants first — and says so.
fn read_since(path: &Path, from: u64, most: u64) -> Option<Stretch> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    // A file shorter than the mark was rewritten: read it as new.
    let rewritten = from > len;
    let from = if rewritten { 0 } else { from };
    let start = from.max(len.saturating_sub(most));
    if start >= len {
        return Some(Stretch {
            bytes: Vec::new(),
            read_to: len,
            cut: false,
            reset: rewritten,
            read_bytes: 0,
        });
    }
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut bytes = Vec::new();
    file.take(len - start).read_to_end(&mut bytes).ok()?;
    let read_bytes = bytes.len() as u64;
    // The producer may still be writing the last JSONL record. Its first
    // bytes must be read again with the remainder on the next hook.
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    bytes.truncate(complete);
    Some(Stretch {
        bytes,
        read_to: start + complete as u64,
        cut: start > from,
        reset: rewritten || start > from,
        read_bytes,
    })
}

/// Whole lines out of a chunk: the first is dropped when the read was cut
/// into the file, because a line's tail is not a line.
fn whole_lines(bytes: &[u8], cut: bool) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if cut && !lines.is_empty() {
        lines.remove(0);
    }
    lines
}

/// The session a transcript file belongs to: the file stem, which is how both
/// vendors name them (Claude's `<uuid>.jsonl`, zo's `session-<ms>-<n>.jsonl`).
fn session_of(path: &Path) -> Option<String> {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
}

/// Register the facts of one transcript's unread stretch. `project` is the
/// root zo's rootless lines are judged against; Claude's lines carry their
/// own. Counts actual bytes read separately from the committed cursor.
pub(crate) fn note_transcript(
    store: &Store,
    stamps: &mut Stamps,
    path: &Path,
    speaker: Speaker,
    project: Option<&Path>,
    most: u64,
    now_ms: i64,
) -> BackfillReport {
    let limits = store.limits();
    let mut report = BackfillReport {
        files: 1,
        ..BackfillReport::default()
    };
    let mut mark = stamps.read.get(path).cloned().unwrap_or_default();
    let project = project
        .map(Path::to_path_buf)
        .or_else(|| mark.project.clone());
    // zo's metadata contains no cwd (Session::meta_record). A rootless
    // backfill cannot judge its writes; leave the cursor for SessionStart.
    if speaker == Speaker::Zo && project.is_none() {
        return report;
    }
    if speaker == Speaker::Zo && mark.project.is_none() {
        mark.bytes = 0;
    }
    let from = mark.bytes;
    let Some(stretch) = read_since(path, from, most) else {
        return report;
    };
    report.bytes = stretch.read_bytes;
    report.truncated = stretch.cut;
    let lines = whole_lines(&stretch.bytes, stretch.cut);
    if stretch.reset {
        mark.extraction = ExtractionState::default();
    }
    let facts = extract_incremental(
        speaker,
        lines.iter(),
        project.as_deref(),
        &mut mark.extraction,
        limits.transcript_pending_max,
    );
    mark.bytes = stretch.read_to;
    mark.at_ms = now_ms;
    mark.project = project;
    stamps.mark(path, mark, limits.transcript_files_max * 2);
    let agent = match speaker {
        Speaker::Claude => AgentKind::Claude.slug(),
        Speaker::Zo => AgentKind::Zo.slug(),
    };
    let session = session_of(path);
    for fact in facts {
        match fact {
            Fact::Remote(mut held) => {
                if held.session.is_none() {
                    held.session = session.clone();
                }
                if store.register_remote(&held, agent, now_ms).is_ok() {
                    report.remote += 1;
                }
            }
            Fact::Page(mut held) => {
                if held.session.is_none() {
                    held.session = session.clone();
                }
                if let Ok(Some(row)) = store.register_page(&held, agent, now_ms) {
                    stamps.created_pages.insert(row.path);
                    report.pages += 1;
                }
            }
        }
    }
    stamps.retain_creations(&store.agent_pages());
    report
}

/// One bounded pass over every root, newest files first, incremental on the
/// stamps. The stamps are saved afterwards so the next pass starts where this
/// one stopped.
pub(crate) fn backfill(store: &Store, roots: &[Root], now_ms: i64) -> BackfillReport {
    let _reading = store
        .transcript_reads
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let limits = store.limits();
    let mut stamps = Stamps::load(store.root());
    let mut report = BackfillReport::default();
    // The earlier rule's rows are judged before this pass can add rows by
    // the new one, so only what that rule registered is ever judged.
    let judged = rejudge_pass(store, &mut stamps, roots, &limits);
    report.files = judged.files;
    report.bytes = judged.bytes;
    report.forgotten = judged.forgotten;
    report.rejudging = judged.pending;
    if judged.forgotten > 0 {
        crate::system_runtime::note_window_event(
            store.local_data_root(),
            &format!(
                "artifacts: forgot {} transcript pages born of an edit rather than a creation",
                judged.forgotten
            ),
        );
    }
    let mut files: Vec<(std::time::SystemTime, PathBuf, Speaker)> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root.dir) else {
            continue;
        };
        // Claude nests `<slug>/<session>.jsonl`; zo keeps the files flat.
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(inner) = std::fs::read_dir(&path) {
                    for file in inner.flatten() {
                        push_transcript(&mut files, file.path(), root.speaker);
                    }
                }
            } else {
                push_transcript(&mut files, path, root.speaker);
            }
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let files_left = limits.transcript_files_max.saturating_sub(judged.files);
    if files.len() > files_left {
        report.truncated = true;
        files.truncate(files_left);
    }
    let mut budget = limits.transcript_bytes_max.saturating_sub(judged.bytes);
    for (_, path, speaker) in files {
        if budget == 0 {
            report.truncated = true;
            break;
        }
        let read = note_transcript(store, &mut stamps, &path, speaker, None, budget, now_ms);
        budget = budget.saturating_sub(read.bytes);
        report.files += read.files;
        report.bytes += read.bytes;
        report.remote += read.remote;
        report.pages += read.pages;
        report.truncated |= read.truncated;
    }
    if let Err(error) = stamps.save(store.root()) {
        // Without a durable cursor, another automatic pass would repeat
        // forever. Retain history and let the next boot retry the review.
        eprintln!("artifacts: could not save transcript review: {error}");
        report.rejudging = 0;
        report.truncated = true;
    }
    report
}

fn push_transcript(
    files: &mut Vec<(std::time::SystemTime, PathBuf, Speaker)>,
    path: PathBuf,
    speaker: Speaker,
) {
    if path.extension().is_none_or(|held| held != "jsonl") {
        return;
    }
    let Ok(meta) = std::fs::metadata(&path) else {
        return;
    };
    if !meta.is_file() {
        return;
    }
    let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
    files.push((modified, path, speaker));
}

/// What one legacy judgement pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Judged {
    files: usize,
    bytes: u64,
    forgotten: usize,
    pending: usize,
}

/// Inspect all retained history before forgetting an edit-only legacy row.
/// A row's current origin can name a later editor, and its file time is not
/// creation provenance. Positive or unknown evidence from ANY session wins;
/// only its own session can supply the edit/birth evidence for removal.
/// The pending review and creation facts share the existing stamp file.
fn rejudge_pass(store: &Store, stamps: &mut Stamps, roots: &[Root], limits: &Limits) -> Judged {
    let rows = store.agent_pages();
    stamps.retain_creations(&rows);
    if stamps.page_rule >= PAGE_RULE {
        return Judged::default();
    }
    if limits.transcript_files_max == 0 || limits.transcript_bytes_max == 0 {
        // No readable history is not evidence of an edit-only origin.
        stamps.creation_review = None;
        stamps.page_rule = PAGE_RULE;
        return Judged::default();
    }
    let mut judging = stamps.creation_review.take().unwrap_or_else(|| {
        let (unread, mut unknown) = retained_transcripts(roots, stamps);
        let history = unread
            .keys()
            .filter_map(|path| {
                let stamp = history_stamp(path);
                unknown |= stamp.is_none();
                stamp.map(|stamp| (path.clone(), stamp))
            })
            .collect();
        Rejudging {
            unread,
            history,
            rows: rows
                .iter()
                .filter(|row| row.origin.agent.as_deref() == Some(AgentKind::Claude.slug()))
                .filter(|row| row.origin.session.is_some())
                .map(|row| {
                    (
                        row.id.clone(),
                        Evidence {
                            keep: unknown,
                            ..Evidence::default()
                        },
                    )
                })
                .collect(),
        }
    });
    judging
        .rows
        .retain(|id, _| rows.iter().any(|row| &row.id == id));
    for row in &rows {
        if stamps.created_pages.contains(&row.path)
            && let Some(evidence) = judging.rows.get_mut(&row.id)
        {
            evidence.keep = true;
        }
    }
    let mut files_left = limits.transcript_files_max;
    let mut bytes_left = limits.transcript_bytes_max;
    let mut report = Judged::default();
    while judging.rows.values().any(|evidence| !evidence.keep) {
        let Some((file, from)) = judging
            .unread
            .iter()
            .next()
            .map(|(file, from)| (file.clone(), *from))
        else {
            break;
        };
        if files_left == 0 || bytes_left == 0 {
            stamps.creation_review = Some(judging);
            return Judged {
                pending: 1,
                ..report
            };
        }
        files_left -= 1;
        report.files += 1;
        let Some(chunk) = read_forward(&file, from, bytes_left) else {
            for evidence in judging.rows.values_mut() {
                evidence.keep = true;
            }
            break;
        };
        bytes_left = bytes_left.saturating_sub(chunk.read);
        report.bytes += chunk.read;
        weigh(
            &chunk.bytes,
            &rows,
            &mut judging.rows,
            session_of(&file).as_deref(),
            limits.transcript_pending_max,
        );
        if chunk.incomplete {
            // The tail could contain a creation, even if it does not yet
            // contain the file name. Never treat a partial record as EOF.
            for evidence in judging.rows.values_mut() {
                evidence.keep = true;
            }
        }
        if chunk.at_end {
            for evidence in judging.rows.values_mut() {
                evidence.keep |= !evidence.pending.is_empty();
            }
            judging.unread.remove(&file);
        } else if chunk.next > from {
            judging.unread.insert(file, chunk.next);
        } else if chunk.read >= limits.transcript_bytes_max {
            for evidence in judging.rows.values_mut() {
                evidence.keep = true;
            }
        } else {
            stamps.creation_review = Some(judging);
            return Judged {
                pending: 1,
                ..report
            };
        }
    }
    let (current, unknown) = retained_transcripts(roots, stamps);
    let unchanged = !unknown
        && current.keys().eq(judging.history.keys())
        && judging
            .history
            .iter()
            .all(|(path, stamp)| history_stamp(path).as_ref() == Some(stamp));
    let forget: Vec<String> = rows
        .iter()
        .filter(|row| {
            judging.rows.get(&row.id).is_some_and(|evidence| {
                unchanged
                    && judging.unread.is_empty()
                    && !evidence.keep
                    && evidence.edited
                    && evidence
                        .first_touch_ms
                        .is_some_and(|first| row.created_ms >= first)
            })
        })
        .map(|row| row.id.clone())
        .collect();
    let forgotten = store.forget_agent_pages(&forget);
    stamps.page_rule = PAGE_RULE;
    Judged {
        forgotten,
        ..report
    }
}

/// Enumerate history independently of the normal newest-first backfill cap.
/// Reading still spends that cap each pass. Remembered but missing paths
/// are included, so a failed read preserves the rows instead of proving absence.
fn retained_transcripts(roots: &[Root], stamps: &Stamps) -> (BTreeMap<PathBuf, u64>, bool) {
    let mut paths: BTreeMap<PathBuf, u64> =
        stamps.read.keys().cloned().map(|path| (path, 0)).collect();
    let mut unknown = false;
    for root in roots {
        let entries = match std::fs::read_dir(&root.dir) {
            Ok(entries) => entries,
            Err(error) => {
                unknown |= error.kind() != std::io::ErrorKind::NotFound;
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                unknown = true;
                continue;
            };
            let path = entry.path();
            if path.is_dir() {
                let Ok(inner) = std::fs::read_dir(&path) else {
                    unknown = true;
                    continue;
                };
                for file in inner {
                    match file {
                        Ok(file) if file.path().extension().is_some_and(|ext| ext == "jsonl") => {
                            paths.insert(file.path(), 0);
                        }
                        Ok(_) => {}
                        Err(_) => unknown = true,
                    }
                }
            } else if path.extension().is_some_and(|ext| ext == "jsonl") {
                paths.insert(path, 0);
            }
        }
    }
    (paths, unknown)
}

/// A forward read of a transcript's next whole lines.
struct Forward {
    bytes: Vec<u8>,
    /// Where the next read starts: just past the last whole line.
    next: u64,
    /// The read reached the file's end; a last line still being written is
    /// left unread.
    at_end: bool,
    incomplete: bool,
    read: u64,
}

/// The whole lines of `path` from `from`, at most `most` bytes of them.
/// `None` when the file cannot be read, or is shorter than `from` — it was
/// rewritten, and what the judgement read no longer holds.
fn read_forward(path: &Path, from: u64, most: u64) -> Option<Forward> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if from > len {
        return None;
    }
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take((len - from).min(most))
        .read_to_end(&mut bytes)
        .ok()?;
    let read = bytes.len() as u64;
    let complete = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |end| end + 1);
    let bytes_len = bytes.len();
    bytes.truncate(complete);
    Some(Forward {
        bytes,
        next: from + complete as u64,
        at_end: from + complete as u64 >= len,
        incomplete: from + read >= len && complete < bytes_len,
        read,
    })
}

/// Parse each record once. Incomplete/unknown records are not evidence of
/// absence; an unnamed tool result can still answer a named pending call.
fn weigh(
    bytes: &[u8],
    rows: &[Artifact],
    evidence: &mut BTreeMap<String, Evidence>,
    session: Option<&str>,
    pending_max: usize,
) {
    for line in String::from_utf8_lossy(bytes).lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            for held in evidence.values_mut() {
                held.keep = true;
            }
            continue;
        };
        for row in rows {
            let Some(held) = evidence.get_mut(&row.id).filter(|held| !held.keep) else {
                continue;
            };
            // Other grammars (including zo) are unknown to the deletion
            // classifier. A mention can protect a row but never condemn it.
            if value.pointer("/message/content").is_none() {
                if row
                    .path
                    .file_name()
                    .is_some_and(|name| line.contains(name.to_string_lossy().as_ref()))
                {
                    held.keep = true;
                }
                continue;
            }
            weigh_line(
                &value,
                &row.path,
                held,
                row.origin.session.as_deref() == session,
            );
            if held.pending.len() > pending_max {
                held.keep = true;
                held.pending.clear();
            }
        }
    }
}

/// What one Claude line says about one file: a writer call naming it places
/// the session's first touch; a result naming it says how it was written.
fn weigh_line(value: &serde_json::Value, path: &Path, evidence: &mut Evidence, own_session: bool) {
    let cwd = value
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    let names_file = |file: &str| {
        let file = cwd
            .as_deref()
            .map_or_else(|| PathBuf::from(file), |cwd| cwd.join(file));
        file == path || file.canonicalize().is_ok_and(|held| held == path)
    };
    let Some(content) = value
        .pointer("/message/content")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    let mut answered = false;
    let mut matched_answer = false;
    for block in content {
        match block.get("type").and_then(serde_json::Value::as_str) {
            Some("tool_use") => {
                let writer = block
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|name| CLAUDE_WRITERS.contains(&name));
                let file = block
                    .pointer("/input/file_path")
                    .and_then(serde_json::Value::as_str);
                if !writer || !file.is_some_and(names_file) {
                    continue;
                }
                if let Some(id) = block.get("id").and_then(serde_json::Value::as_str) {
                    evidence.pending.insert(id.to_string());
                } else {
                    evidence.keep = true;
                }
                if !own_session {
                    continue;
                }
                match value
                    .get("timestamp")
                    .and_then(serde_json::Value::as_str)
                    .and_then(epoch_ms_of_iso)
                {
                    Some(at) => {
                        evidence.first_touch_ms =
                            Some(evidence.first_touch_ms.map_or(at, |first| first.min(at)));
                    }
                    None => evidence.keep = true,
                }
            }
            Some("tool_result") => {
                answered = true;
                if let Some(id) = block.get("tool_use_id").and_then(serde_json::Value::as_str) {
                    matched_answer |= evidence.pending.remove(id);
                }
            }
            _ => {}
        }
    }
    let Some(result) = value
        .get("toolUseResult")
        .filter(|_| answered)
        .and_then(serde_json::Value::as_object)
    else {
        evidence.keep |= matched_answer;
        return;
    };
    if !result
        .get("filePath")
        .and_then(serde_json::Value::as_str)
        .is_some_and(names_file)
    {
        evidence.keep |= matched_answer;
        return;
    }
    // An edit whose old text is empty may have made the file.
    let from_nothing = result.get("oldString").and_then(serde_json::Value::as_str) == Some("")
        || result
            .get("edits")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|edits| {
                edits.iter().any(|edit| {
                    edit.get("old_string").and_then(serde_json::Value::as_str) == Some("")
                })
            });
    let edit_shaped = ["oldString", "edits", "structuredPatch"]
        .iter()
        .any(|key| result.contains_key(*key));
    match result.get("type").and_then(serde_json::Value::as_str) {
        Some("update") => evidence.edited |= own_session,
        None if edit_shaped && !from_nothing => evidence.edited |= own_session,
        // Its creation, or a word this reader does not know: the row stays.
        _ => evidence.keep = true,
    }
}

/// The `cwd` each session announced at `SessionStart`, for zo's lines that
/// carry none. Bounded; the window's whole life is a few hundred sessions.
fn session_roots() -> &'static Mutex<BTreeMap<String, (PathBuf, i64)>> {
    static CELL: std::sync::OnceLock<Mutex<BTreeMap<String, (PathBuf, i64)>>> =
        std::sync::OnceLock::new();
    CELL.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn remember_session_root(session: &str, cwd: &Path, now_ms: i64) {
    let mut held = session_roots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    held.insert(session.to_string(), (cwd.to_path_buf(), now_ms));
    while held.len() > SESSION_ROOTS_MAX {
        let Some(oldest) = held
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        held.remove(&oldest);
    }
}

fn session_root(session: &str) -> Option<PathBuf> {
    session_roots()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(session)
        .map(|(cwd, _)| cwd.clone())
}

/// An event name reduced to its letters and digits, lower-cased — the
/// spelling `READING_EVENTS` is written in.
fn normalized(event: &str) -> String {
    event
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// What a hook envelope asks of this road, judged without the store: the
/// transcript to read and the root to judge pages by, or nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reading {
    pub(crate) path: PathBuf,
    pub(crate) speaker: Speaker,
    pub(crate) project: Option<PathBuf>,
}

/// The hook road's judgement (pure but for the session-root memory): only
/// Claude and zo speak a transcript this reader knows; `SessionStart`
/// remembers the cwd and reads nothing; `Stop` and a writing tool's
/// `PostToolUse` read the transcript the payload names.
pub(crate) fn reading_of(envelope: &HookEnvelope, now_ms: i64) -> Option<Reading> {
    let speaker = match envelope.agent {
        AgentKind::Claude => Speaker::Claude,
        AgentKind::Zo => Speaker::Zo,
        _ => return None,
    };
    let payload = zerocode_core::payload::HookPayload::of(&envelope.payload);
    let event = zerocode_core::hook::envelope_event_name_parsed(envelope, &payload)?;
    let word = normalized(&event);
    let tree = payload.tree()?;
    let session = zerocode_core::provider_session::session_in_parsed(envelope.agent, &payload)?;
    let cwd = tree
        .get("cwd")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    if word == "sessionstart" {
        if let Some(cwd) = &cwd {
            remember_session_root(&session.id, cwd, now_ms);
        }
        return None;
    }
    if !READING_EVENTS.contains(&word.as_str()) {
        return None;
    }
    if word == "posttooluse" {
        let activity = zerocode_core::hook::activity_of_parsed(&event, &payload)?;
        // Only a `Write` can create a page; an edit changes what was there.
        let writes = matches!(activity.verb, Tool::Write)
            || matches!(&activity.verb, Tool::Other(name) if name.eq_ignore_ascii_case("artifact"));
        if !writes || activity.phase != Phase::Finished {
            return None;
        }
    }
    let path = PathBuf::from(session.transcript_path?);
    let project = cwd.or_else(|| session_root(&session.id));
    Some(Reading {
        path,
        speaker,
        project,
    })
}

/// The hook road: read what the envelope points at and register it. Answers
/// whether anything landed, so the caller can tell the window once.
pub(crate) fn note_hook(store: &Store, envelope: &HookEnvelope, now_ms: i64) -> bool {
    let Some(reading) = reading_of(envelope, now_ms) else {
        return false;
    };
    let limits = store.limits();
    let _reading = store
        .transcript_reads
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut stamps = Stamps::load(store.root());
    let report = note_transcript(
        store,
        &mut stamps,
        &reading.path,
        reading.speaker,
        reading.project.as_deref(),
        limits.transcript_tail_bytes,
        now_ms,
    );
    let _ = stamps.save(store.root());
    report.changed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_runtime::Filter;
    use zerocode_core::artifact::{ArtifactKind, Limits};

    const CLAUDE: &str =
        include_str!("../../zerocode-core/fixtures/artifact-transcript/claude.jsonl");
    const ZO: &str = include_str!("../../zerocode-core/fixtures/artifact-transcript/zo.jsonl");

    #[test]
    fn pending_artifact_pairs_survive_saved_cursors_and_partial_result_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let path = dir.path().join("session.jsonl");
        let use_line = CLAUDE.lines().next().unwrap();
        let result = CLAUDE.lines().nth(1).unwrap();
        let split = result.len() / 2;
        touch(&path, &format!("{use_line}\n{}", &result[..split]));
        let mut stamps = Stamps::default();
        assert_eq!(
            note_transcript(
                &store,
                &mut stamps,
                &path,
                Speaker::Claude,
                None,
                u64::MAX,
                1
            )
            .remote,
            0
        );
        stamps.save(store.root()).unwrap();
        touch(&path, &format!("{use_line}\n{result}\n"));
        let mut reopened = Stamps::load(store.root());
        assert_eq!(
            note_transcript(
                &store,
                &mut reopened,
                &path,
                Speaker::Claude,
                None,
                u64::MAX,
                2
            )
            .remote,
            1
        );
        let row = store.list(&Filter::default()).rows.remove(0);
        assert_eq!(
            row.favicon.as_deref(),
            Some("🧱"),
            "input metadata survives the cursor"
        );
        assert_eq!(
            note_transcript(
                &store,
                &mut reopened,
                &path,
                Speaker::Claude,
                None,
                u64::MAX,
                3
            )
            .remote,
            0
        );
    }

    #[test]
    fn a_rootless_zo_backfill_does_not_consume_pages_before_the_session_root_is_known() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let project = dir.path().join("project");
        touch(&project.join("docs/report.html"), "page");
        let path = dir.path().join("session.jsonl");
        let line = ZO
            .lines()
            .find(|line| line.contains("tool_result") && line.contains("docs/report.html"))
            .expect("the created page's result")
            .replace(
                "/Users/someone/project/docs/report.html",
                "docs/report.html",
            );
        touch(&path, &format!("{line}\n"));
        let mut stamps = Stamps::default();
        assert_eq!(
            note_transcript(&store, &mut stamps, &path, Speaker::Zo, None, u64::MAX, 1).pages,
            0
        );
        assert_eq!(
            note_transcript(
                &store,
                &mut stamps,
                &path,
                Speaker::Zo,
                Some(&project),
                u64::MAX,
                2
            )
            .pages,
            1
        );
        stamps.save(store.root()).unwrap();
        touch(&project.join("second.html"), "second");
        touch(
            &path,
            &format!(
                "{line}\n{}\n",
                line.replace("docs/report.html", "second.html")
            ),
        );
        let mut reopened = Stamps::load(store.root());
        assert_eq!(
            note_transcript(&store, &mut reopened, &path, Speaker::Zo, None, u64::MAX, 3).pages,
            1
        );
    }

    fn touch(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    /// A Claude transcript in the shape the vendor writes it, with the page
    /// it created present on disk under a project root the lines name — and
    /// the two files it only edited or overwrote present too, so a road that
    /// counted those would register them (t-3952).
    fn claude_fixture(dir: &Path) -> (PathBuf, PathBuf) {
        let project = dir.join("project");
        let page = project.join("DECISIONS.md");
        touch(&page, "# Decisions\n\nfixture body\n");
        touch(&project.join("ui/index.html"), "<title>Project</title>");
        touch(&project.join("README.md"), "# Project\n");
        let text = CLAUDE.replace("/Users/someone/project", &project.display().to_string());
        let transcript = dir
            .join("home/.claude/projects/-Users-someone-project/33333333-4444-4555-8666-777777777777.jsonl");
        touch(&transcript, &text);
        (transcript, page)
    }

    /// The backfill walks both vendors' roots, registers one web row per url
    /// and one page per file under its root, saves where it stopped, and the
    /// second pass reads nothing until a line is appended.
    #[test]
    fn a_backfill_registers_web_rows_and_pages_and_is_incremental_on_its_stamps() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let (transcript, page) = claude_fixture(dir.path());
        let zo_project = dir.path().join("project");
        touch(&zo_project.join("docs/report.html"), "<!doctype html>");
        let zo = home.join(".zo/sessions/session-1780878278884-0.jsonl");
        touch(
            &zo,
            &ZO.replace("/Users/someone/project", &zo_project.display().to_string()),
        );
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let roots = roots(&home, &dir.path().join("window-home"));
        assert_eq!(roots.len(), 3, "{roots:?}");
        let first = backfill(&store, &roots, 1_000);
        assert_eq!(first.files, 2, "{first:?}");
        assert_eq!(first.remote, 1, "{first:?}");
        // The Claude page under its cwd; zo's lines have no root at backfill.
        assert_eq!(first.pages, 1, "{first:?}");
        assert!(first.changed());
        let listing = store.list(&Filter::default());
        let web = listing
            .rows
            .iter()
            .find(|row| row.kind == ArtifactKind::Web)
            .expect("a web row");
        assert_eq!(
            web.url.as_deref(),
            Some("https://claude.ai/code/artifact/aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee")
        );
        assert_eq!(web.title, "왜 MSA가 아니라 모듈러 모놀리스인가");
        assert_eq!(web.favicon.as_deref(), Some("🧱"));
        assert_eq!(web.origin.agent.as_deref(), Some("claude"));
        assert_eq!(
            web.origin.session.as_deref(),
            Some("11111111-2222-4333-8444-555555555555")
        );
        let document = listing
            .rows
            .iter()
            .find(|row| row.kind == ArtifactKind::Document)
            .expect("the page under its root");
        assert_eq!(document.path, page.canonicalize().unwrap());
        assert_eq!(
            document.origin.session.as_deref(),
            Some("33333333-4444-4555-8666-777777777777")
        );
        let remote_only = store.list(&Filter {
            remote: Some(true),
            ..Filter::default()
        });
        assert_eq!(remote_only.rows.len(), 1);
        let local_only = store.list(&Filter {
            remote: Some(false),
            ..Filter::default()
        });
        assert_eq!(local_only.rows.len(), 1);

        let stamps = Stamps::load(store.root());
        assert_eq!(stamps.len(), 1, "the rootless zo transcript stays unread");
        let second = backfill(&store, &roots, 2_000);
        assert_eq!(
            (second.bytes, second.remote, second.pages),
            (0, 0, 0),
            "{second:?}"
        );
        assert!(!second.changed());
        assert_eq!(store.len(), 2, "a second pass added rows");

        // A line appended later is the only thing the third pass reads.
        let mut appended = std::fs::read_to_string(&transcript).expect("read");
        let use_line = CLAUDE.lines().next().expect("use");
        let result_line = CLAUDE
            .lines()
            .nth(1)
            .expect("result")
            .replace(
                "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            )
            .replace("2026-09-04T21:27:04.061Z", "2026-09-05T00:00:00.000Z");
        appended.push_str(use_line);
        appended.push('\n');
        appended.push_str(&result_line);
        appended.push('\n');
        std::fs::write(&transcript, appended).expect("append");
        let third = backfill(&store, &roots, 3_000);
        assert_eq!(third.remote, 1, "{third:?}");
        assert!(third.bytes < 8_000, "the whole file was re-read: {third:?}");
        assert_eq!(store.len(), 3);
    }

    /// The table bounds the pass: files past `transcript_files_max` and bytes
    /// past `transcript_bytes_max` are reported as truncation, not read.
    #[test]
    fn a_backfill_past_the_table_reports_truncation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        for n in 0..3 {
            touch(&home.join(format!(".claude/projects/-p/{n}.jsonl")), CLAUDE);
        }
        let store = Store::open(
            &dir.path().join("data"),
            Limits {
                transcript_files_max: 2,
                ..Limits::default()
            },
        );
        let report = backfill(&store, &roots(&home, &home), 1_000);
        assert!(report.truncated, "{report:?}");
        assert_eq!(report.files, 2);
        let tight = Store::open(
            &dir.path().join("data2"),
            Limits {
                transcript_bytes_max: 100,
                ..Limits::default()
            },
        );
        let report = backfill(&tight, &roots(&home, &home), 1_000);
        assert!(report.truncated, "{report:?}");
        assert!(report.bytes <= 100, "{report:?}");
    }

    /// The hook road: only Claude and zo, only `Stop` and a writing tool's
    /// `PostToolUse`, only with a `transcript_path`; `SessionStart` remembers
    /// the cwd zo's lines lack; and reading the tail registers the rows.
    #[test]
    fn the_hook_road_reads_the_tail_a_stop_names_and_remembers_the_session_root() {
        let envelope = |agent: AgentKind, payload: &str| HookEnvelope {
            agent,
            pane_key: "term-1".into(),
            tab_id: String::new(),
            launch_token: String::new(),
            worktree_id: String::new(),
            env: String::new(),
            version: String::new(),
            hook_event_name: String::new(),
            payload: payload.to_string(),
        };
        let stop = r#"{"hook_event_name":"Stop","session_id":"s-1","transcript_path":"/t/s-1.jsonl","cwd":"/p"}"#;
        let read = reading_of(&envelope(AgentKind::Claude, stop), 1).expect("a stop reads");
        assert_eq!(read.path, PathBuf::from("/t/s-1.jsonl"));
        assert_eq!(read.speaker, Speaker::Claude);
        assert_eq!(read.project.as_deref(), Some(Path::new("/p")));
        assert!(
            reading_of(&envelope(AgentKind::Codex, stop), 1).is_none(),
            "codex writes no transcript this reader knows"
        );
        let no_path = r#"{"hook_event_name":"Stop","session_id":"s-1"}"#;
        assert!(reading_of(&envelope(AgentKind::Claude, no_path), 1).is_none());
        let read_tool = r#"{"hook_event_name":"PostToolUse","tool_name":"Read","tool_input":{"file_path":"/p/a.md"},"session_id":"s-1","transcript_path":"/t/s-1.jsonl"}"#;
        assert!(reading_of(&envelope(AgentKind::Claude, read_tool), 1).is_none());
        let write_tool = r#"{"hook_event_name":"PostToolUse","tool_name":"Write","tool_input":{"file_path":"/p/a.md"},"session_id":"s-1","transcript_path":"/t/s-1.jsonl"}"#;
        assert!(reading_of(&envelope(AgentKind::Claude, write_tool), 1).is_some());
        let edit_tool = write_tool.replace("\"Write\"", "\"Edit\"");
        assert!(
            reading_of(&envelope(AgentKind::Claude, &edit_tool), 1).is_none(),
            "an edit creates no page, so it reads no transcript"
        );
        let artifact_tool = r#"{"hook_event_name":"PostToolUse","tool_name":"Artifact","tool_input":{"file_path":"/tmp/x.html"},"session_id":"s-1","transcript_path":"/t/s-1.jsonl"}"#;
        assert!(reading_of(&envelope(AgentKind::Claude, artifact_tool), 1).is_some());

        // zo: SessionStart remembers the cwd, Stop reads with it.
        let start = r#"{"hook_event_name":"SessionStart","session_id":"session-9","transcript_path":"/t/session-9.jsonl","cwd":"/zo-project"}"#;
        assert!(reading_of(&envelope(AgentKind::Zo, start), 1).is_none());
        let zo_stop = r#"{"hook_event_name":"Stop","session_id":"session-9","transcript_path":"/t/session-9.jsonl","last_assistant_message":"done"}"#;
        let read = reading_of(&envelope(AgentKind::Zo, zo_stop), 2).expect("zo stop reads");
        assert_eq!(read.speaker, Speaker::Zo);
        assert_eq!(read.project.as_deref(), Some(Path::new("/zo-project")));

        // And the read itself, against a real transcript on disk.
        let dir = tempfile::tempdir().expect("tempdir");
        let (transcript, _) = claude_fixture(dir.path());
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let payload = format!(
            r#"{{"hook_event_name":"Stop","session_id":"s-1","transcript_path":"{}"}}"#,
            transcript.display()
        );
        assert!(note_hook(
            &store,
            &envelope(AgentKind::Claude, &payload),
            1_000
        ));
        assert_eq!(store.len(), 2);
        assert!(
            !note_hook(&store, &envelope(AgentKind::Claude, &payload), 2_000),
            "nothing new was appended, nothing new landed"
        );
        assert_eq!(Stamps::load(store.root()).len(), 1);
    }

    /// A file the stamps say was read further than it is long was rewritten:
    /// it is read as new. A stretch longer than the bound is read from its
    /// end, and the cut first line is dropped rather than parsed.
    #[test]
    fn a_rewritten_transcript_is_read_again_and_a_long_stretch_is_read_from_its_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("t.jsonl");
        touch(&path, "line-one\nline-two\nline-three\n");
        let whole = read_since(&path, 1_000, u64::MAX).expect("reads");
        assert_eq!(whole.read_to, 29);
        assert!(!whole.cut);
        assert_eq!(
            whole_lines(&whole.bytes, whole.cut),
            vec!["line-one", "line-two", "line-three"]
        );
        let tail = read_since(&path, 0, 12).expect("reads");
        assert!(tail.cut);
        assert_eq!(
            whole_lines(&tail.bytes, tail.cut),
            vec!["line-three"],
            "the cut first line was kept"
        );
        let continued = read_since(&path, 9, u64::MAX).expect("reads");
        assert!(!continued.cut, "a read from the mark is not a cut");
        assert_eq!(
            whole_lines(&continued.bytes, continued.cut),
            vec!["line-two", "line-three"]
        );
        let nothing = read_since(&path, 29, u64::MAX).expect("reads");
        assert!(nothing.bytes.is_empty());
        assert_eq!(nothing.read_to, 29);
        let mut stamps = Stamps::default();
        for n in 0..5 {
            stamps.mark(
                Path::new(&format!("/t/{n}")),
                ReadMark {
                    bytes: 1,
                    at_ms: n,
                    ..ReadMark::default()
                },
                3,
            );
        }
        assert_eq!(stamps.len(), 3, "the stamp map is bounded");
        assert_eq!(
            stamps.read_to(Path::new("/t/0")),
            None,
            "the oldest went first"
        );
    }

    /// A legacy transcript row, as the rule before t-3952 left it: put in
    /// the catalog straight through the store, which never judged creation.
    fn legacy_row(store: &Store, project: &Path, name: &str, agent: &str, session: &str) -> String {
        touch(&project.join(name), "<title>legacy</title>");
        store
            .register_page(
                &zerocode_core::artifact_transcript::PageFact {
                    path: project.join(name),
                    at_ms: None,
                    session: Some(session.into()),
                    project: Some(project.to_path_buf()),
                },
                agent,
                1,
            )
            .expect("register")
            .expect("a legacy row")
            .id
    }

    /// One writer call and its result, in the shape Claude writes them.
    fn claude_call(
        id: &str,
        tool: &str,
        file: &Path,
        result: serde_json::Value,
        at: &str,
        cwd: &Path,
        session: &str,
    ) -> String {
        let call = serde_json::json!({
            "type": "assistant", "timestamp": at, "cwd": cwd, "sessionId": session,
            "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": id, "name": tool, "input": {"file_path": file}}
            ]}
        });
        let answer = serde_json::json!({
            "type": "user", "timestamp": at, "cwd": cwd, "sessionId": session,
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": id, "content": "ok"}
            ]},
            "toolUseResult": result
        });
        format!("{call}\n{answer}\n")
    }

    /// The legacy judgement forgets only rows its own session shows were
    /// born of an edit or an overwrite; a creation, a born-earlier row, a
    /// session with no transcript, a zo row, a result it cannot classify and
    /// an edit from nothing all stay — and no project file is touched.
    #[test]
    fn legacy_rows_born_of_an_edit_are_forgotten_and_every_other_row_stays() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let project = dir.path().join("project");
        let store = Store::open(&dir.path().join("data"), Limits::default());
        let edited = legacy_row(&store, &project, "edited.html", "claude", "sess-a");
        let overwritten = legacy_row(&store, &project, "overwritten.md", "claude", "sess-a");
        let created = legacy_row(&store, &project, "created.md", "claude", "sess-a");
        let unknown = legacy_row(&store, &project, "unknown.html", "claude", "sess-a");
        let from_nothing = legacy_row(&store, &project, "nothing.md", "claude", "sess-a");
        let zo = legacy_row(&store, &project, "zo.html", "zo", "sess-a");
        let earlier = legacy_row(&store, &project, "earlier.html", "claude", "sess-b");
        let orphan = legacy_row(&store, &project, "orphan.md", "claude", "sess-gone");
        let past = "2026-01-01T00:00:00.000Z";
        let edit = |file: &str| {
            serde_json::json!({
                "filePath": project.join(file), "oldString": "a", "newString": "b",
                "originalFile": null, "structuredPatch": [], "userModified": false, "replaceAll": false
            })
        };
        let mut session_a = String::new();
        session_a += &claude_call(
            "t1",
            "Edit",
            &project.join("edited.html"),
            edit("edited.html"),
            past,
            &project,
            "sess-a",
        );
        session_a += &claude_call(
            "t2",
            "Write",
            &project.join("overwritten.md"),
            serde_json::json!({"type": "update", "filePath": project.join("overwritten.md"), "content": "x"}),
            past,
            &project,
            "sess-a",
        );
        session_a += &claude_call(
            "t3",
            "Write",
            &project.join("created.md"),
            serde_json::json!({"type": "create", "filePath": project.join("created.md"), "content": "x"}),
            past,
            &project,
            "sess-a",
        );
        session_a += &claude_call(
            "t4",
            "Edit",
            &project.join("created.md"),
            edit("created.md"),
            past,
            &project,
            "sess-a",
        );
        session_a += &claude_call(
            "t5",
            "Edit",
            &project.join("unknown.html"),
            serde_json::json!({"filePath": project.join("unknown.html"), "type": "patch"}),
            past,
            &project,
            "sess-a",
        );
        let mut nothing = edit("nothing.md");
        nothing["oldString"] = serde_json::json!("");
        session_a += &claude_call(
            "t6",
            "Edit",
            &project.join("nothing.md"),
            nothing,
            past,
            &project,
            "sess-a",
        );
        session_a += &claude_call(
            "t7",
            "Edit",
            &project.join("zo.html"),
            edit("zo.html"),
            past,
            &project,
            "sess-a",
        );
        touch(
            &home.join(".claude/projects/-project/sess-a.jsonl"),
            &session_a,
        );
        // A session whose first touch comes after the row was catalogued:
        // the row was born of some other session's call.
        touch(
            &home.join(".claude/projects/-project/sess-b.jsonl"),
            &claude_call(
                "t8",
                "Edit",
                &project.join("earlier.html"),
                edit("earlier.html"),
                "2099-01-01T00:00:00.000Z",
                &project,
                "sess-b",
            ),
        );
        let roots = roots(&home, &dir.path().join("window-home"));
        let report = backfill(&store, &roots, 1_000);
        assert_eq!((report.forgotten, report.rejudging), (2, 0), "{report:?}");
        assert!(report.changed());
        for gone in [&edited, &overwritten] {
            assert!(store.get(gone).is_none(), "{gone} stayed");
            assert!(
                !store
                    .root()
                    .join(crate::artifact_runtime::VERSIONS_DIR_NAME)
                    .join(gone)
                    .exists()
            );
        }
        for kept in [&created, &unknown, &from_nothing, &zo, &earlier, &orphan] {
            assert!(store.get(kept).is_some(), "{kept} was forgotten");
        }
        for name in ["edited.html", "overwritten.md"] {
            assert!(
                project.join(name).is_file(),
                "the project's {name} was touched"
            );
        }
        assert_eq!(Stamps::load(store.root()).page_rule, PAGE_RULE);
        // The rule is judged once: a later pass forgets nothing more.
        let again = backfill(&store, &roots, 2_000);
        assert_eq!((again.forgotten, again.rejudging), (0, 0), "{again:?}");
        assert!(store.get(&created).is_some());
    }

    /// A session longer than one pass may read is judged across passes, its
    /// place kept in the stamps; its row goes only when the end is read.
    #[test]
    fn a_long_session_is_judged_across_passes_under_the_tables_bound() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let project = dir.path().join("project");
        let store = Store::open(
            &dir.path().join("data"),
            Limits {
                transcript_bytes_max: 1_024,
                ..Limits::default()
            },
        );
        let edited = legacy_row(&store, &project, "edited.html", "claude", "sess-long");
        let mut text = String::new();
        for n in 0..40 {
            text +=
                &format!("{{\"type\":\"user\",\"message\":{{\"content\":\"filler line {n}\"}}}}\n");
        }
        text += &claude_call(
            "t1",
            "Edit",
            &project.join("edited.html"),
            serde_json::json!({"filePath": project.join("edited.html"), "oldString": "a", "structuredPatch": []}),
            "2026-01-01T00:00:00.000Z",
            &project,
            "sess-long",
        );
        assert!(text.len() > 2_048, "the fixture must outgrow two passes");
        touch(
            &home.join(".claude/projects/-project/sess-long.jsonl"),
            &text,
        );
        let roots = roots(&home, &dir.path().join("window-home"));
        let first = backfill(&store, &roots, 1_000);
        assert_eq!((first.forgotten, first.rejudging), (0, 1), "{first:?}");
        assert!(
            store.get(&edited).is_some(),
            "judged before its end was read"
        );
        assert!(Stamps::load(store.root()).creation_review.is_some());
        let mut passes = 1;
        let mut forgotten = 0;
        while passes < 20 {
            passes += 1;
            let report = backfill(&store, &roots, 1_000 + passes);
            forgotten += report.forgotten;
            if report.rejudging == 0 {
                break;
            }
        }
        assert_eq!(forgotten, 1, "after {passes} passes");
        assert!(passes > 2, "one pass read more than its bound");
        assert!(store.get(&edited).is_none());
        assert_eq!(Stamps::load(store.root()).page_rule, PAGE_RULE);
    }

    fn cross_session_creation_survives(creator: &str, editor: &str, limits: Limits) {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("home");
        let project = dir.path().join("project");
        let store = Store::open(&dir.path().join("data"), limits);
        let id = legacy_row(&store, &project, "page.html", "claude", editor);
        let source = project.join("page.html");
        let versions = store.versions(&id);
        let snapshot = std::fs::read(&versions[0].path).unwrap();
        // Recreating a deleted catalog row from current source would lose
        // these original snapshot bytes even if its ID were unchanged.
        touch(
            &source,
            "<title>later content with a different length</title>",
        );
        let mut stamps = Stamps::default();
        for (session, kind) in [(creator, "create"), (editor, "update")] {
            let text = claude_call(
                "write",
                "Write",
                &source,
                serde_json::json!({"type": kind, "filePath": source}),
                "2026-01-01T00:00:00.000Z",
                &project,
                session,
            );
            let path = home
                .join(".claude/projects/project")
                .join(format!("{session}.jsonl"));
            touch(&path, &text);
            // Pre-migration cursors have consumed both sessions, but the
            // row's origin names only the editor. Normal backfill cannot repair it.
            stamps.read.insert(
                path,
                ReadMark {
                    bytes: text.len() as u64,
                    ..ReadMark::default()
                },
            );
        }
        stamps.save(store.root()).unwrap();
        let roots = roots(&home, &dir.path().join("window-home"));
        let mut passes = 0;
        loop {
            passes += 1;
            let report = backfill(&store, &roots, passes);
            assert_eq!(report.forgotten, 0, "{report:?}");
            assert!(report.files <= limits.transcript_files_max, "{report:?}");
            assert!(report.bytes <= limits.transcript_bytes_max, "{report:?}");
            assert!(store.get(&id).is_some());
            assert_eq!(std::fs::read(&versions[0].path).unwrap(), snapshot);
            assert_eq!(store.versions(&id), versions);
            assert!(source.is_file());
            if report.rejudging == 0 {
                break;
            }
            assert!(passes < 20, "review made no progress");
            assert!(Stamps::load(store.root()).creation_review.is_some());
        }
        if limits.transcript_files_max == 1 {
            assert!(passes > 1);
        }
        assert_eq!(Stamps::load(store.root()).page_rule, PAGE_RULE);
    }

    #[test]
    fn consumed_creation_before_edit_preserves_original_snapshots() {
        cross_session_creation_survives("a-creator", "z-editor", Limits::default());
    }

    #[test]
    fn consumed_creation_after_edit_preserves_original_snapshots() {
        cross_session_creation_survives("z-creator", "a-editor", Limits::default());
    }

    #[test]
    fn creation_in_a_later_bounded_pass_preserves_rows_across_saved_cursors() {
        cross_session_creation_survives(
            "z-creator",
            "a-editor",
            Limits {
                transcript_files_max: 1,
                transcript_bytes_max: 1_024,
                ..Limits::default()
            },
        );
    }

    fn edit_only_history(dir: &Path) -> (Store, Vec<Root>, String, PathBuf) {
        let home = dir.join("home");
        let project = dir.join("project");
        let store = Store::open(&dir.join("data"), Limits::default());
        let id = legacy_row(&store, &project, "page.html", "claude", "editor");
        let path = home.join(".claude/projects/project/editor.jsonl");
        touch(
            &path,
            &claude_call(
                "edit",
                "Edit",
                &project.join("page.html"),
                serde_json::json!({"filePath": project.join("page.html"), "oldString": "a", "newString": "b"}),
                "2026-01-01T00:00:00.000Z",
                &project,
                "editor",
            ),
        );
        (store, roots(&home, &dir.join("window-home")), id, path)
    }

    #[test]
    fn an_incomplete_final_record_cannot_prove_absence_of_creation() {
        let dir = tempfile::tempdir().unwrap();
        let (store, roots, id, path) = edit_only_history(dir.path());
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"type\":\"user\",\"toolUseResult\":");
        touch(&path, &text);
        let report = backfill(&store, &roots, 1);
        assert_eq!(report.forgotten, 0);
        assert!(store.get(&id).is_some());
        assert!(!store.versions(&id).is_empty());
    }

    #[test]
    fn a_missing_consumed_transcript_keeps_legacy_history() {
        let dir = tempfile::tempdir().unwrap();
        let (store, roots, id, path) = edit_only_history(dir.path());
        let mut stamps = Stamps::default();
        stamps
            .read
            .insert(path.with_file_name("missing.jsonl"), ReadMark::default());
        let report = rejudge_pass(&store, &mut stamps, &roots, &store.limits());
        assert_eq!(report.forgotten, 0);
        assert!(store.get(&id).is_some());
    }

    #[test]
    fn an_unanswered_writer_in_another_session_keeps_legacy_history() {
        let dir = tempfile::tempdir().unwrap();
        let (store, roots, id, path) = edit_only_history(dir.path());
        let row = store.get(&id).unwrap();
        let text = claude_call(
            "unknown",
            "Write",
            &row.path,
            serde_json::Value::Null,
            "2026-01-01T00:00:00.000Z",
            row.origin.project.as_deref().unwrap(),
            "other",
        );
        touch(
            &path.with_file_name("other.jsonl"),
            &format!("{}\n", text.lines().next().unwrap()),
        );
        let report = backfill(&store, &roots, 1);
        assert_eq!(report.forgotten, 0);
        assert!(store.get(&id).is_some());
    }

    #[test]
    fn a_creation_observed_between_passes_survives_a_missing_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let (store, roots, id, path) = edit_only_history(dir.path());
        let row = store.get(&id).unwrap();
        let mut limits = store.limits();
        // A second history file leaves the review pending after the editor.
        let creator = path.with_file_name("z-creator.jsonl");
        touch(
            &creator,
            &claude_call(
                "create",
                "Write",
                &row.path,
                serde_json::json!({"type": "create", "filePath": row.path}),
                "2026-01-01T00:00:00.000Z",
                row.origin.project.as_deref().unwrap(),
                "creator",
            ),
        );
        limits.transcript_files_max = 1;
        let mut stamps = Stamps::default();
        assert_eq!(
            rejudge_pass(&store, &mut stamps, &roots, &limits).pending,
            1
        );
        note_transcript(
            &store,
            &mut stamps,
            &creator,
            Speaker::Claude,
            None,
            limits.transcript_bytes_max,
            1,
        );
        assert!(stamps.created_pages.contains(&row.path));
        stamps.save(store.root()).unwrap();
        std::fs::remove_file(&creator).unwrap();
        let mut restored = Stamps::load(store.root());
        let report = rejudge_pass(&store, &mut restored, &roots, &limits);
        assert_eq!(report.forgotten, 0);
        assert!(store.get(&id).is_some());
        assert!(!store.versions(&id).is_empty());
    }

    #[test]
    fn unreadable_retained_history_keeps_rows_and_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let (store, roots, id, path) = edit_only_history(dir.path());
        let unreadable = path.with_file_name("unreadable.jsonl");
        // A directory can be statted but cannot be read as transcript bytes,
        // including when the test process has permission to read every file.
        std::fs::create_dir(&unreadable).unwrap();
        let mut stamps = Stamps::default();
        stamps.read.insert(unreadable, ReadMark::default());
        let report = rejudge_pass(&store, &mut stamps, &roots, &store.limits());
        assert_eq!(report.forgotten, 0);
        assert!(store.get(&id).is_some());
        assert!(!store.versions(&id).is_empty());
    }

    #[test]
    fn history_changed_after_an_earlier_pass_cannot_authorize_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let (store, roots, id, path) = edit_only_history(dir.path());
        touch(&path.with_file_name("z-other.jsonl"), "{}\n");
        let mut limits = store.limits();
        limits.transcript_files_max = 1;
        let mut stamps = Stamps::default();
        assert_eq!(
            rejudge_pass(&store, &mut stamps, &roots, &limits).pending,
            1
        );
        touch(&path, "{}\n");
        stamps.save(store.root()).unwrap();
        let mut restored = Stamps::load(store.root());
        let report = rejudge_pass(&store, &mut restored, &roots, &limits);
        assert_eq!(report.forgotten, 0);
        assert!(store.get(&id).is_some());
    }

    #[test]
    fn removing_an_edit_only_row_leaves_source_and_publication_bytes_intact() {
        let dir = tempfile::tempdir().unwrap();
        let (store, roots, id, _) = edit_only_history(dir.path());
        let source = store.get(&id).unwrap().path;
        let source_bytes = std::fs::read(&source).unwrap();
        let publication = store
            .publish_page(&zerocode_core::artifact_publish::PublishInput {
                file_path: source.clone(),
                ..Default::default()
            })
            .unwrap();
        let versions = store.versions(&publication.id);
        let published_bytes = std::fs::read(&versions[0].path).unwrap();
        let report = backfill(&store, &roots, 1);
        assert_eq!(report.forgotten, 1);
        assert!(store.get(&id).is_none());
        assert_eq!(std::fs::read(source).unwrap(), source_bytes);
        assert!(store.get(&publication.id).is_some());
        assert_eq!(store.versions(&publication.id), versions);
        assert_eq!(std::fs::read(&versions[0].path).unwrap(), published_bytes);
    }
}
