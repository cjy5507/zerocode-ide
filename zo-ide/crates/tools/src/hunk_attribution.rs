use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use runtime::{CompactDiffHunk, CompactDiffLineKind, compact_line_diff};

use crate::{WorkspaceCheckpoint, WorkspaceFileSnapshot};

static REVIEW_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributionOrigin {
    Agent { turn_index: usize },
    Human,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributionStatus {
    Pending,
    Accepted,
    Rejected,
    Stale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttributionLineKind {
    Context,
    Removed,
    Added,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributionLine {
    pub kind: AttributionLineKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LineEnding {
    Lf,
    CrLf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SnapshotFormat {
    exists: bool,
    trailing_newline: bool,
    line_ending: LineEnding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttributedHunk {
    pub path: PathBuf,
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<AttributionLine>,
    pub origin: AttributionOrigin,
    pub status: AttributionStatus,
    old_format: SnapshotFormat,
    new_format: SnapshotFormat,
}

impl AttributedHunk {
    #[must_use]
    pub fn new(
        path: impl Into<PathBuf>,
        old_start: usize,
        old_lines: usize,
        new_start: usize,
        new_lines: usize,
        lines: Vec<AttributionLine>,
        origin: AttributionOrigin,
    ) -> Self {
        Self {
            path: path.into(),
            old_start,
            old_lines,
            new_start,
            new_lines,
            lines,
            origin,
            status: AttributionStatus::Pending,
            old_format: SnapshotFormat {
                exists: true,
                trailing_newline: true,
                line_ending: LineEnding::Lf,
            },
            new_format: SnapshotFormat {
                exists: true,
                trailing_newline: true,
                line_ending: LineEnding::Lf,
            },
        }
    }

    fn from_compact(
        path: &Path,
        origin: AttributionOrigin,
        hunk: CompactDiffHunk,
        old: TextSnapshot<'_>,
        new: TextSnapshot<'_>,
    ) -> Self {
        Self {
            path: path.to_path_buf(),
            old_start: hunk.old_start,
            old_lines: hunk.old_lines,
            new_start: hunk.new_start,
            new_lines: hunk.new_lines,
            lines: hunk
                .lines
                .into_iter()
                .map(|line| AttributionLine {
                    kind: match line.kind {
                        CompactDiffLineKind::Context => AttributionLineKind::Context,
                        CompactDiffLineKind::Removed => AttributionLineKind::Removed,
                        CompactDiffLineKind::Added => AttributionLineKind::Added,
                    },
                    text: line.text,
                })
                .collect(),
            origin,
            status: AttributionStatus::Pending,
            old_format: old.format(),
            new_format: new.format(),
        }
    }

    fn same_change(&self, other: &Self) -> bool {
        self.path == other.path
            && self.old_start == other.old_start
            && self.old_lines == other.old_lines
            && self.new_start == other.new_start
            && self.new_lines == other.new_lines
            && self.lines == other.lines
            && self.origin == other.origin
            && self.old_format == other.old_format
            && self.new_format == other.new_format
    }

    fn is_reverse_of(&self, other: &Self) -> bool {
        self.path == other.path
            && self.old_start == other.new_start
            && self.old_lines == other.new_lines
            && self.new_start == other.old_start
            && self.new_lines == other.old_lines
            && self.old_format == other.new_format
            && self.new_format == other.old_format
            && side_refs(self, false) == side_refs(other, true)
            && side_refs(self, true) == side_refs(other, false)
    }

    fn old_side(&self) -> Vec<String> {
        self.lines
            .iter()
            .filter(|line| line.kind != AttributionLineKind::Added)
            .map(|line| line.text.clone())
            .collect()
    }

    fn new_side(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter(|line| line.kind != AttributionLineKind::Removed)
            .map(|line| line.text.as_str())
            .collect()
    }
}

fn side_refs(hunk: &AttributedHunk, new_side: bool) -> Vec<&str> {
    hunk.lines
        .iter()
        .filter(|line| {
            if new_side {
                line.kind != AttributionLineKind::Removed
            } else {
                line.kind != AttributionLineKind::Added
            }
        })
        .map(|line| line.text.as_str())
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HunkAttributionLedger {
    pub hunks: Vec<AttributedHunk>,
}

impl HunkAttributionLedger {
    pub fn build(checkpoints: &[WorkspaceCheckpoint]) -> io::Result<Self> {
        let mut ordered: Vec<&WorkspaceCheckpoint> = checkpoints.iter().collect();
        ordered.sort_by_key(|checkpoint| checkpoint.turn_index);

        let mut hunks = Vec::new();
        let mut last_after_by_path = BTreeMap::<PathBuf, WorkspaceFileSnapshot>::new();
        for checkpoint in ordered {
            for file in &checkpoint.files {
                if let Some(previous_after) = last_after_by_path.get(&file.path) {
                    append_snapshot_diff(
                        &mut hunks,
                        &file.path,
                        previous_after,
                        &file.before,
                        AttributionOrigin::Human,
                    );
                }
                append_snapshot_diff(
                    &mut hunks,
                    &file.path,
                    &file.before,
                    &file.after,
                    AttributionOrigin::Agent {
                        turn_index: checkpoint.turn_index,
                    },
                );
                last_after_by_path.insert(file.path.clone(), file.after.clone());
            }
        }

        for (path, last_after) in last_after_by_path {
            let current = crate::workspace_checkpoint::capture_snapshot(&path)?;
            append_snapshot_diff(
                &mut hunks,
                &path,
                &last_after,
                &current,
                AttributionOrigin::Human,
            );
        }
        Ok(Self { hunks })
    }

    #[must_use]
    pub fn from_hunks(hunks: Vec<AttributedHunk>) -> Self {
        Self { hunks }
    }

    pub fn reconcile(&mut self, previous: &Self) {
        self.hunks.retain(|fresh| {
            !(fresh.origin == AttributionOrigin::Human
                && previous.hunks.iter().any(|old| {
                    old.status == AttributionStatus::Rejected
                        && matches!(old.origin, AttributionOrigin::Agent { .. })
                        && fresh.is_reverse_of(old)
                }))
        });

        let mut matched_previous = vec![false; previous.hunks.len()];
        for fresh in &mut self.hunks {
            if let Some((index, old)) = previous
                .hunks
                .iter()
                .enumerate()
                .find(|(index, old)| !matched_previous[*index] && fresh.same_change(old))
            {
                fresh.status = old.status;
                matched_previous[index] = true;
            }
        }
        for (matched, old) in matched_previous.into_iter().zip(&previous.hunks) {
            if !matched && old.status != AttributionStatus::Pending {
                self.hunks.push(old.clone());
            }
        }
    }

    pub fn accept(&mut self, index: usize) -> Result<(), ReviewHunkError> {
        let hunk = self
            .hunks
            .get_mut(index)
            .ok_or(ReviewHunkError::NotFound(index))?;
        if hunk.status == AttributionStatus::Pending {
            hunk.status = AttributionStatus::Accepted;
        }
        Ok(())
    }

    pub fn accept_file(&mut self, path: &Path) {
        for hunk in &mut self.hunks {
            if hunk.path == path && hunk.status == AttributionStatus::Pending {
                hunk.status = AttributionStatus::Accepted;
            }
        }
    }

    pub fn reject(&mut self, index: usize) -> Result<(), ReviewHunkError> {
        let hunk = self
            .hunks
            .get_mut(index)
            .ok_or(ReviewHunkError::NotFound(index))?;
        apply_reverse_patch(hunk)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReviewHunkError {
    #[error("review hunk {0} was not found")]
    NotFound(usize),
    #[error("{} is no longer at the recorded hunk context", .path.display())]
    Stale { path: PathBuf },
    #[error("{}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

impl ReviewHunkError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

pub fn apply_reverse_patch(hunk: &mut AttributedHunk) -> Result<(), ReviewHunkError> {
    if hunk.status != AttributionStatus::Pending {
        return Ok(());
    }

    let current_bytes = match fs::read(&hunk.path) {
        Ok(bytes) => Some(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(ReviewHunkError::io(&hunk.path, error)),
    };
    let current = match current_bytes.as_deref() {
        Some(bytes) => match std::str::from_utf8(bytes) {
            Ok(text) => Some(text),
            Err(_) => return stale(hunk),
        },
        None => None,
    };
    if current.is_some() != hunk.new_format.exists {
        return stale(hunk);
    }

    let current_text = current.unwrap_or_default();
    let mut current_lines: Vec<String> = current_text.lines().map(str::to_string).collect();
    let expected = hunk.new_side();
    let preferred_start = hunk.new_start.saturating_sub(1);
    let Some((start, end)) = find_target_region(&current_lines, &expected, preferred_start) else {
        return stale(hunk);
    };
    let replaces_eof = end == current_lines.len();
    if replaces_eof && current_text.ends_with('\n') != hunk.new_format.trailing_newline {
        return stale(hunk);
    }

    current_lines.splice(start..end, hunk.old_side());
    if !hunk.old_format.exists && current_lines.is_empty() {
        fs::remove_file(&hunk.path).map_err(|error| ReviewHunkError::io(&hunk.path, error))?;
    } else {
        let line_ending = if current.is_some() {
            detect_line_ending(current_text)
        } else {
            hunk.old_format.line_ending
        };
        let separator = match line_ending {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        };
        let mut updated = current_lines.join(separator);
        let trailing_newline = if replaces_eof {
            hunk.old_format.trailing_newline
        } else {
            current_text.ends_with('\n')
        };
        if trailing_newline && !current_lines.is_empty() {
            updated.push_str(separator);
        }
        atomic_write(&hunk.path, updated.as_bytes())
            .map_err(|error| ReviewHunkError::io(&hunk.path, error))?;
    }
    hunk.status = AttributionStatus::Rejected;
    Ok(())
}

fn find_target_region(
    current_lines: &[String],
    expected: &[&str],
    preferred_start: usize,
) -> Option<(usize, usize)> {
    let preferred_end = preferred_start.checked_add(expected.len())?;
    if preferred_end <= current_lines.len()
        && current_lines[preferred_start..preferred_end]
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
    {
        return Some((preferred_start, preferred_end));
    }
    if expected.is_empty() || expected.len() > current_lines.len() {
        return None;
    }

    let mut matches = (0..=current_lines.len() - expected.len()).filter(|start| {
        current_lines[*start..*start + expected.len()]
            .iter()
            .map(String::as_str)
            .eq(expected.iter().copied())
    });
    let start = matches.next()?;
    matches.next().is_none().then_some((start, start + expected.len()))
}

fn stale(hunk: &mut AttributedHunk) -> Result<(), ReviewHunkError> {
    hunk.status = AttributionStatus::Stale;
    Err(ReviewHunkError::Stale {
        path: hunk.path.clone(),
    })
}

fn append_snapshot_diff(
    hunks: &mut Vec<AttributedHunk>,
    path: &Path,
    old: &WorkspaceFileSnapshot,
    new: &WorkspaceFileSnapshot,
    origin: AttributionOrigin,
) {
    let (Some(old), Some(new)) = (TextSnapshot::from_snapshot(old), TextSnapshot::from_snapshot(new))
    else {
        return;
    };
    if old.exists == new.exists && old.text == new.text {
        return;
    }
    hunks.extend(
        compact_line_diff(old.text, new.text)
            .into_iter()
            .map(|hunk| AttributedHunk::from_compact(path, origin, hunk, old, new)),
    );
}

#[derive(Clone, Copy)]
struct TextSnapshot<'a> {
    text: &'a str,
    exists: bool,
    trailing_newline: bool,
    line_ending: LineEnding,
}

impl<'a> TextSnapshot<'a> {
    fn from_snapshot(snapshot: &'a WorkspaceFileSnapshot) -> Option<Self> {
        if snapshot.skipped.is_some() {
            return None;
        }
        let (text, exists) = match snapshot.content.as_deref() {
            Some(bytes) => (std::str::from_utf8(bytes).ok()?, true),
            None => ("", false),
        };
        Some(Self {
            text,
            exists,
            trailing_newline: text.ends_with('\n'),
            line_ending: detect_line_ending(text),
        })
    }

    const fn format(self) -> SnapshotFormat {
        SnapshotFormat {
            exists: self.exists,
            trailing_newline: self.trailing_newline,
            line_ending: self.line_ending,
        }
    }
}

fn detect_line_ending(text: &str) -> LineEnding {
    if text.contains("\r\n") {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("review");
    let sequence = REVIEW_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{file_name}.zo-review-{}-{sequence}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(contents)?;
        if let Ok(metadata) = fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{
        AttributionLineKind, AttributionOrigin, AttributionStatus, HunkAttributionLedger,
        ReviewHunkError,
    };
    use crate::{WorkspaceCheckpoint, WorkspaceCheckpointFile, WorkspaceFileSnapshot};

    fn snapshot(text: &str) -> WorkspaceFileSnapshot {
        WorkspaceFileSnapshot {
            content: Some(text.as_bytes().to_vec()),
            skipped: None,
        }
    }

    fn checkpoint(
        turn_index: usize,
        path: PathBuf,
        before: &str,
        after: &str,
    ) -> WorkspaceCheckpoint {
        WorkspaceCheckpoint {
            turn_index,
            files: vec![WorkspaceCheckpointFile {
                path,
                before: snapshot(before),
                after: snapshot(after),
            }],
            ..WorkspaceCheckpoint::default()
        }
    }

    #[test]
    fn agent_only_turn_attributes_checkpoint_diff_to_agent() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("agent.txt");
        fs::write(&path, "one\nagent\nthree\n").unwrap();
        let ledger = HunkAttributionLedger::build(&[checkpoint(
            4,
            path.clone(),
            "one\ntwo\nthree\n",
            "one\nagent\nthree\n",
        )])
        .unwrap();

        assert_eq!(ledger.hunks.len(), 1);
        assert_eq!(
            ledger.hunks[0].origin,
            AttributionOrigin::Agent { turn_index: 4 }
        );
        assert_eq!(ledger.hunks[0].status, AttributionStatus::Pending);
    }

    #[test]
    fn human_edit_between_turns_is_attributed_to_human() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("between.txt");
        fs::write(&path, "agent two\n").unwrap();
        let checkpoints = vec![
            checkpoint(1, path.clone(), "base\n", "agent one\n"),
            checkpoint(2, path.clone(), "human\n", "agent two\n"),
        ];
        let ledger = HunkAttributionLedger::build(&checkpoints).unwrap();

        assert_eq!(ledger.hunks.len(), 3);
        assert_eq!(ledger.hunks[0].origin, AttributionOrigin::Agent { turn_index: 1 });
        assert_eq!(ledger.hunks[1].origin, AttributionOrigin::Human);
        assert_eq!(ledger.hunks[2].origin, AttributionOrigin::Agent { turn_index: 2 });
    }

    #[test]
    fn mixed_agent_and_current_human_edits_are_both_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mixed.txt");
        fs::write(&path, "agent\nhuman tail\n").unwrap();
        let ledger = HunkAttributionLedger::build(&[checkpoint(
            7,
            path,
            "base\n",
            "agent\n",
        )])
        .unwrap();

        assert_eq!(ledger.hunks.len(), 2);
        assert_eq!(ledger.hunks[0].origin, AttributionOrigin::Agent { turn_index: 7 });
        assert_eq!(ledger.hunks[1].origin, AttributionOrigin::Human);
    }

    #[test]
    fn reverse_patch_applies_and_marks_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("reject.txt");
        fs::write(&path, "one\nagent\nthree\n").unwrap();
        let mut ledger = HunkAttributionLedger::build(&[checkpoint(
            3,
            path.clone(),
            "one\ntwo\nthree\n",
            "one\nagent\nthree\n",
        )])
        .unwrap();

        ledger.reject(0).unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "one\ntwo\nthree\n");
        assert_eq!(ledger.hunks[0].status, AttributionStatus::Rejected);
    }

    #[test]
    fn reverse_patch_relocates_one_exact_target_after_earlier_lines_shift() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("shifted.txt");
        fs::write(&path, "human prefix\none\nagent\nthree\n").unwrap();
        let mut ledger = HunkAttributionLedger::build(&[checkpoint(
            3,
            path.clone(),
            "one\ntwo\nthree\n",
            "one\nagent\nthree\n",
        )])
        .unwrap();

        ledger.reject(0).unwrap();

        assert_eq!(
            fs::read_to_string(path).unwrap(),
            "human prefix\none\ntwo\nthree\n"
        );
        assert_eq!(ledger.hunks[0].status, AttributionStatus::Rejected);
    }

    #[test]
    fn rebuild_does_not_reclassify_a_rejected_agent_hunk_as_human() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rebuild.txt");
        fs::write(&path, "agent\n").unwrap();
        let checkpoints = vec![checkpoint(5, path, "before\n", "agent\n")];
        let mut previous = HunkAttributionLedger::build(&checkpoints).unwrap();
        previous.reject(0).unwrap();

        let mut rebuilt = HunkAttributionLedger::build(&checkpoints).unwrap();
        rebuilt.reconcile(&previous);

        assert_eq!(rebuilt.hunks.len(), 1);
        assert_eq!(rebuilt.hunks[0].status, AttributionStatus::Rejected);
        assert_eq!(
            rebuilt.hunks[0].origin,
            AttributionOrigin::Agent { turn_index: 5 }
        );
    }

    #[test]
    fn reverse_patch_context_mismatch_marks_stale_without_writing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("stale.txt");
        fs::write(&path, "one\nagent\nthree\n").unwrap();
        let mut ledger = HunkAttributionLedger::build(&[checkpoint(
            9,
            path.clone(),
            "one\ntwo\nthree\n",
            "one\nagent\nthree\n",
        )])
        .unwrap();
        fs::write(&path, "one\nexternal\nthree\n").unwrap();

        let error = ledger.reject(0).unwrap_err();

        assert!(matches!(error, ReviewHunkError::Stale { .. }));
        assert_eq!(ledger.hunks[0].status, AttributionStatus::Stale);
        assert_eq!(fs::read_to_string(path).unwrap(), "one\nexternal\nthree\n");
        assert!(ledger.hunks[0]
            .lines
            .iter()
            .any(|line| line.kind == AttributionLineKind::Added));
    }
}
