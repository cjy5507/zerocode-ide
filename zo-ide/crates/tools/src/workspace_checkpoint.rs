use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Maximum number of bytes captured for one file state in a workspace checkpoint.
pub const MAX_CHECKPOINT_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum number of workspace checkpoints retained for one session.
pub const MAX_WORKSPACE_CHECKPOINTS: usize = 50;

/// Maximum bytes of file content (before and after states together) the
/// retained checkpoints of one session hold in memory, the turn in progress
/// included. The oldest checkpoints go first; a file state that would push the
/// total over the line is not kept — it is recorded as skipped (the same mark
/// an over-size file gets), so the list and a restore say it cannot be put
/// back. A zo process holds at most this much, so eight panes hold at most
/// 512 MiB, and only if each wrote that much (t-19459).
pub const MAX_WORKSPACE_CHECKPOINT_BYTES: u64 = 64 * 1024 * 1024;

const CHECKPOINT_SCHEMA_VERSION: u32 = 1;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSnapshotSkip {
    #[serde(default)]
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceFileSnapshot {
    /// `None` means the file did not exist. Ignored when `skipped` is present.
    #[serde(default)]
    pub content: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<WorkspaceSnapshotSkip>,
}

impl WorkspaceFileSnapshot {
    fn bytes(content: Vec<u8>) -> Self {
        Self {
            content: Some(content),
            skipped: None,
        }
    }

    fn missing() -> Self {
        Self::default()
    }

    fn skipped_size(size_bytes: u64) -> Self {
        Self {
            content: None,
            skipped: Some(WorkspaceSnapshotSkip { size_bytes }),
        }
    }

    #[must_use]
    pub fn is_oversized(&self) -> bool {
        self.skipped.is_some()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCheckpointFile {
    #[serde(default)]
    pub path: PathBuf,
    #[serde(default)]
    pub before: WorkspaceFileSnapshot,
    #[serde(default)]
    pub after: WorkspaceFileSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceCheckpoint {
    #[serde(default = "checkpoint_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub turn_index: usize,
    #[serde(default)]
    pub created_at_epoch_secs: u64,
    #[serde(default)]
    pub incomplete: bool,
    #[serde(default)]
    pub files: Vec<WorkspaceCheckpointFile>,
}

impl Default for WorkspaceCheckpoint {
    fn default() -> Self {
        Self {
            schema_version: checkpoint_schema_version(),
            turn_index: 0,
            created_at_epoch_secs: 0,
            incomplete: false,
            files: Vec::new(),
        }
    }
}

impl WorkspaceCheckpoint {
    /// Bytes of file content this checkpoint holds in memory.
    #[must_use]
    pub fn content_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|file| snapshot_bytes(&file.before) + snapshot_bytes(&file.after))
            .sum()
    }

    #[must_use]
    pub fn has_oversized_files(&self) -> bool {
        self.files
            .iter()
            .any(|file| file.before.is_oversized() || file.after.is_oversized())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceRestoreSkippedPath {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceRestoreSummary {
    pub target_turn_index: usize,
    pub restore_checkpoint_turn_index: Option<usize>,
    pub incomplete_range: bool,
    pub restored: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
    pub conflicted: Vec<PathBuf>,
    pub skipped: Vec<WorkspaceRestoreSkippedPath>,
}

#[must_use]
pub fn render_workspace_checkpoint_list(checkpoints: &[WorkspaceCheckpoint]) -> String {
    render_workspace_checkpoint_list_at(checkpoints, now_epoch_secs())
}

fn render_workspace_checkpoint_list_at(
    checkpoints: &[WorkspaceCheckpoint],
    now_epoch_secs: u64,
) -> String {
    if checkpoints.is_empty() {
        return "Workspace checkpoints\n  No file-edit checkpoints recorded yet.".to_string();
    }
    let mut output = String::from("Workspace checkpoints");
    for (number, checkpoint) in checkpoints.iter().rev().enumerate() {
        let age = format_age(
            now_epoch_secs.saturating_sub(checkpoint.created_at_epoch_secs),
        );
        let mut flags = Vec::new();
        if checkpoint.incomplete {
            flags.push("incomplete");
        }
        if checkpoint.has_oversized_files() {
            flags.push("skipped-oversized");
        }
        let flags = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        };
        let _ = write!(
            output,
            "\n  {}. Turn {} · {age} ago · {} file(s){flags}",
            number + 1,
            checkpoint.turn_index,
            checkpoint.files.len(),
        );
    }
    output.push_str(
        "\n  Restore with /rewind <Turn number> [force] — the Turn number in a row, not its position in the list.",
    );
    output
}

fn format_age(age_secs: u64) -> String {
    if age_secs < SECONDS_PER_MINUTE {
        format!("{age_secs}s")
    } else if age_secs < SECONDS_PER_HOUR {
        format!("{}m", age_secs / SECONDS_PER_MINUTE)
    } else if age_secs < SECONDS_PER_DAY {
        format!("{}h", age_secs / SECONDS_PER_HOUR)
    } else {
        format!("{}d", age_secs / SECONDS_PER_DAY)
    }
}

#[must_use]
pub fn render_workspace_restore_summary(summary: &WorkspaceRestoreSummary) -> String {
    let mut output = format!(
        "Workspace rewind\n  Target           before turn {}\n  Restored         {}\n  Deleted          {}\n  Conflicted       {}\n  Skipped          {}",
        summary.target_turn_index,
        summary.restored.len(),
        summary.deleted.len(),
        summary.conflicted.len(),
        summary.skipped.len(),
    );
    if let Some(turn_index) = summary.restore_checkpoint_turn_index {
        let _ = write!(output, "\n  Undo checkpoint  turn {turn_index}");
    }
    if summary.incomplete_range {
        output.push_str(
            "\n  Warning          range is incomplete because a shell command may have written files",
        );
    }
    if !summary.conflicted.is_empty() {
        output.push_str("\n  Conflicts");
        for path in &summary.conflicted {
            let _ = write!(output, "\n    {}", path.display());
        }
        let _ = write!(
            output,
            "\n  Retry with /rewind {} force to overwrite conflicts.",
            summary.target_turn_index
        );
    }
    if !summary.skipped.is_empty() {
        output.push_str("\n  Skipped paths");
        for skipped in &summary.skipped {
            let _ = write!(
                output,
                "\n    {} — {}",
                skipped.path.display(),
                skipped.reason
            );
        }
    }
    output
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceRestoreEntry {
    pub(crate) path: PathBuf,
    pub(crate) desired: WorkspaceFileSnapshot,
    pub(crate) expected_current: WorkspaceFileSnapshot,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspaceRestorePlan {
    pub(crate) target_turn_index: usize,
    pub(crate) suggested_checkpoint_turn_index: usize,
    pub(crate) incomplete_range: bool,
    pub(crate) entries: Vec<WorkspaceRestoreEntry>,
}

const fn checkpoint_schema_version() -> u32 {
    CHECKPOINT_SCHEMA_VERSION
}

#[derive(Debug, Clone)]
struct TouchedFile {
    before: WorkspaceFileSnapshot,
    write_succeeded: bool,
}

#[derive(Debug, Clone)]
struct ActiveCheckpoint {
    turn_index: usize,
    created_at_epoch_secs: u64,
    incomplete: bool,
    files: BTreeMap<PathBuf, TouchedFile>,
    /// Content bytes this turn's captured states hold so far.
    bytes: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceCheckpointStore {
    checkpoints: VecDeque<WorkspaceCheckpoint>,
    active: Option<ActiveCheckpoint>,
    durable_dir: Option<PathBuf>,
    /// Test seam: a smaller byte budget than [`MAX_WORKSPACE_CHECKPOINT_BYTES`].
    byte_cap: Option<u64>,
}

impl WorkspaceCheckpointStore {
    pub(crate) fn begin_turn(&mut self, suggested_turn_index: usize) -> usize {
        let next_index = self
            .checkpoints
            .back()
            .map_or(suggested_turn_index, |checkpoint| {
                suggested_turn_index.max(checkpoint.turn_index.saturating_add(1))
            });
        self.active = Some(ActiveCheckpoint {
            turn_index: next_index,
            created_at_epoch_secs: now_epoch_secs(),
            incomplete: false,
            files: BTreeMap::new(),
            bytes: 0,
        });
        next_index
    }

    pub(crate) fn mark_incomplete(&mut self) {
        if let Some(active) = self.active.as_mut() {
            active.incomplete = true;
        }
    }

    pub(crate) fn record_before(&mut self, path: &Path) -> io::Result<()> {
        match self.active.as_ref() {
            None => return Ok(()),
            Some(active) if active.files.contains_key(path) => return Ok(()),
            Some(_) => {}
        }
        let before = capture_snapshot(path)?;
        let in_turn = self.active.as_ref().map_or(0, |active| active.bytes);
        let before = self.within_budget(before, in_turn);
        let held = snapshot_bytes(&before);
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        active.bytes += held;
        active.files.insert(
            path.to_path_buf(),
            TouchedFile {
                before,
                write_succeeded: false,
            },
        );
        Ok(())
    }

    /// The snapshot as it may be kept: unchanged while the retained
    /// checkpoints and the turn in progress still fit the byte budget, else a
    /// skipped mark that keeps only its size.
    fn within_budget(&self, snapshot: WorkspaceFileSnapshot, in_turn: u64) -> WorkspaceFileSnapshot {
        let size = snapshot_bytes(&snapshot);
        let retained: u64 = self
            .checkpoints
            .iter()
            .map(WorkspaceCheckpoint::content_bytes)
            .sum();
        if retained + in_turn + size > self.byte_cap.unwrap_or(MAX_WORKSPACE_CHECKPOINT_BYTES) {
            WorkspaceFileSnapshot::skipped_size(size)
        } else {
            snapshot
        }
    }

    pub(crate) fn record_write_success(&mut self, path: &Path) {
        if let Some(touched) = self
            .active
            .as_mut()
            .and_then(|active| active.files.get_mut(path))
        {
            touched.write_succeeded = true;
        }
    }

    pub(crate) fn finish_turn(&mut self) -> io::Result<Option<WorkspaceCheckpoint>> {
        let Some(active) = self.active.take() else {
            return Ok(None);
        };
        let mut files = Vec::new();
        let mut in_turn = active.bytes;
        for (path, touched) in active.files {
            if !touched.write_succeeded {
                continue;
            }
            let after = self.within_budget(capture_snapshot(&path)?, in_turn);
            in_turn += snapshot_bytes(&after);
            files.push(WorkspaceCheckpointFile {
                path,
                before: touched.before,
                after,
            });
        }
        if files.is_empty() {
            return Ok(None);
        }
        let checkpoint = WorkspaceCheckpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            turn_index: active.turn_index,
            created_at_epoch_secs: active.created_at_epoch_secs,
            incomplete: active.incomplete,
            files,
        };
        self.checkpoints.push_back(checkpoint.clone());
        while self.checkpoints.len() > MAX_WORKSPACE_CHECKPOINTS {
            if let Some(evicted) = self.checkpoints.pop_front() {
                self.remove_durable_checkpoint(evicted.turn_index);
            }
        }
        self.evict_over_budget(MAX_WORKSPACE_CHECKPOINT_BYTES);
        if let Some(dir) = self.durable_dir.as_ref() {
            persist_checkpoint(dir, &checkpoint)?;
        }
        Ok(Some(checkpoint))
    }

    /// Drop the oldest checkpoints until the retained content fits
    /// `byte_cap`; the newest one stays whatever its size.
    fn evict_over_budget(&mut self, byte_cap: u64) {
        let mut held: u64 = self
            .checkpoints
            .iter()
            .map(WorkspaceCheckpoint::content_bytes)
            .sum();
        while held > byte_cap && self.checkpoints.len() > 1 {
            let Some(evicted) = self.checkpoints.pop_front() else {
                break;
            };
            held = held.saturating_sub(evicted.content_bytes());
            self.remove_durable_checkpoint(evicted.turn_index);
        }
    }

    pub(crate) fn checkpoints(&self) -> Vec<WorkspaceCheckpoint> {
        self.checkpoints.iter().cloned().collect()
    }

    pub(crate) fn compose_restore_plan(
        &self,
        target_turn_index: usize,
    ) -> Result<WorkspaceRestorePlan, String> {
        let Some(target_position) = self
            .checkpoints
            .iter()
            .position(|checkpoint| checkpoint.turn_index == target_turn_index)
        else {
            return Err(format!(
                "workspace checkpoint for turn {target_turn_index} was not found"
            ));
        };
        let range = self.checkpoints.range(target_position..);
        let incomplete_range = range.clone().any(|checkpoint| checkpoint.incomplete);
        let mut desired = BTreeMap::new();
        let mut expected_current = BTreeMap::new();
        for checkpoint in range.rev() {
            for file in &checkpoint.files {
                expected_current
                    .entry(file.path.clone())
                    .or_insert_with(|| file.after.clone());
                desired.insert(file.path.clone(), file.before.clone());
            }
        }
        let entries = desired
            .into_iter()
            .map(|(path, desired)| WorkspaceRestoreEntry {
                expected_current: expected_current
                    .remove(&path)
                    .expect("every desired path has an expected snapshot"),
                path,
                desired,
            })
            .collect();
        let suggested_checkpoint_turn_index = self
            .checkpoints
            .back()
            .map_or(target_turn_index.saturating_add(1), |checkpoint| {
                checkpoint.turn_index.saturating_add(1)
            });
        Ok(WorkspaceRestorePlan {
            target_turn_index,
            suggested_checkpoint_turn_index,
            incomplete_range,
            entries,
        })
    }

    pub(crate) fn reset_session(&mut self, durable_dir: Option<PathBuf>) -> io::Result<()> {
        self.active = None;
        self.checkpoints.clear();
        self.durable_dir = durable_dir;
        let Some(dir) = self.durable_dir.as_ref() else {
            return Ok(());
        };
        fs::create_dir_all(dir)?;
        let _ = core_types::paths::restrict_permissions_owner_only(dir);
        let mut loaded = BTreeMap::new();
        for entry in fs::read_dir(dir)? {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            let Ok(checkpoint) = serde_json::from_slice::<WorkspaceCheckpoint>(&bytes) else {
                continue;
            };
            loaded.insert(checkpoint.turn_index, checkpoint);
        }
        self.checkpoints = loaded.into_values().collect();
        while self.checkpoints.len() > MAX_WORKSPACE_CHECKPOINTS {
            if let Some(evicted) = self.checkpoints.pop_front() {
                self.remove_durable_checkpoint(evicted.turn_index);
            }
        }
        Ok(())
    }

    fn remove_durable_checkpoint(&self, turn_index: usize) {
        if let Some(dir) = self.durable_dir.as_ref() {
            let _ = fs::remove_file(checkpoint_path(dir, turn_index));
        }
    }
}

fn persist_checkpoint(dir: &Path, checkpoint: &WorkspaceCheckpoint) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let _ = core_types::paths::restrict_permissions_owner_only(dir);
    let path = checkpoint_path(dir, checkpoint.turn_index);
    let temporary = dir.join(format!(
        ".checkpoint-{:020}-{}.tmp",
        checkpoint.turn_index,
        std::process::id()
    ));
    let encoded = serde_json::to_vec_pretty(checkpoint)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    fs::write(&temporary, encoded)?;
    let _ = core_types::paths::restrict_permissions_owner_only(&temporary);
    match fs::rename(&temporary, &path) {
        Ok(()) => {
            let _ = core_types::paths::restrict_permissions_owner_only(&path);
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_file(temporary);
            Err(error)
        }
    }
}

fn checkpoint_path(dir: &Path, turn_index: usize) -> PathBuf {
    dir.join(format!("checkpoint-{turn_index:020}.json"))
}

pub(crate) fn capture_snapshot(path: &Path) -> io::Result<WorkspaceFileSnapshot> {
    capture_snapshot_with_cap(path, MAX_CHECKPOINT_FILE_BYTES)
}

fn capture_snapshot_with_cap(path: &Path, cap: u64) -> io::Result<WorkspaceFileSnapshot> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(WorkspaceFileSnapshot::missing());
        }
        Err(error) => return Err(error),
    };
    if metadata.len() > cap {
        return Ok(WorkspaceFileSnapshot::skipped_size(metadata.len()));
    }
    let content = fs::read(path)?;
    let size_bytes = u64::try_from(content.len()).unwrap_or(u64::MAX);
    if size_bytes > cap {
        return Ok(WorkspaceFileSnapshot::skipped_size(size_bytes));
    }
    Ok(WorkspaceFileSnapshot::bytes(content))
}

/// Content bytes a snapshot holds in memory: a skipped or missing file holds none.
fn snapshot_bytes(snapshot: &WorkspaceFileSnapshot) -> u64 {
    snapshot
        .content
        .as_ref()
        .map_or(0, |content| content.len() as u64)
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ToolContext;

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "zo-workspace-checkpoint-{label}-{}-{}",
            std::process::id(),
            now_epoch_secs()
        ));
        fs::create_dir_all(&path).expect("temp directory");
        path.canonicalize().expect("canonical temp directory")
    }

    fn checkpoint_changes(
        context: &ToolContext,
        turn_index: usize,
        changes: &[(&Path, Option<&[u8]>)],
    ) {
        context.begin_workspace_checkpoint(turn_index);
        for (path, content) in changes {
            context
                .record_workspace_checkpoint_before(path)
                .expect("capture before");
            match content {
                Some(bytes) => fs::write(path, bytes).expect("write change"),
                None => fs::remove_file(path).expect("delete change"),
            }
            context.record_workspace_checkpoint_write(path);
        }
        context
            .finish_workspace_checkpoint()
            .expect("finish turn")
            .expect("stored checkpoint");
    }

    fn history(label: &str) -> (PathBuf, ToolContext) {
        let dir = temp_dir(label);
        let a = dir.join("a.bin");
        let b = dir.join("b.bin");
        let c = dir.join("c.bin");
        fs::write(&a, b"a0").unwrap();
        fs::write(&b, b"b0").unwrap();
        let mut context = ToolContext::new();
        context.workspace_root = Some(dir.clone());
        context.cwd = Some(dir.clone());
        checkpoint_changes(
            &context,
            1,
            &[
                (a.as_path(), Some(b"a1")),
                (c.as_path(), Some(b"c1")),
            ],
        );
        checkpoint_changes(
            &context,
            2,
            &[
                (a.as_path(), Some(b"a2")),
                (b.as_path(), Some(b"b2")),
            ],
        );
        checkpoint_changes(
            &context,
            3,
            &[
                (a.as_path(), Some(b"a3")),
                (c.as_path(), Some(b"c3")),
            ],
        );
        (dir, context)
    }

    #[test]
    fn first_write_wins_and_after_is_captured_at_turn_end() {
        let dir = temp_dir("first-write");
        let path = dir.join("file.txt");
        fs::write(&path, b"before").expect("seed file");
        let mut store = WorkspaceCheckpointStore::default();
        store.begin_turn(3);
        store.record_before(&path).expect("capture before");
        fs::write(&path, b"middle").expect("first write");
        store.record_write_success(&path);
        store.record_before(&path).expect("second capture is ignored");
        fs::write(&path, b"after").expect("second write");
        store.record_write_success(&path);

        let checkpoint = store
            .finish_turn()
            .expect("finish checkpoint")
            .expect("checkpoint stored");
        assert_eq!(checkpoint.turn_index, 3);
        assert_eq!(checkpoint.files[0].before.content.as_deref(), Some(b"before".as_slice()));
        assert_eq!(checkpoint.files[0].after.content.as_deref(), Some(b"after".as_slice()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn created_file_records_none_before() {
        let dir = temp_dir("created");
        let path = dir.join("created.bin");
        let mut store = WorkspaceCheckpointStore::default();
        store.begin_turn(1);
        store.record_before(&path).expect("capture missing file");
        fs::write(&path, b"new").expect("create file");
        store.record_write_success(&path);

        let checkpoint = store.finish_turn().unwrap().unwrap();
        assert_eq!(checkpoint.files[0].before.content, None);
        assert!(checkpoint.files[0].before.skipped.is_none());
        assert_eq!(checkpoint.files[0].after.content.as_deref(), Some(b"new".as_slice()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_snapshot_records_size() {
        let dir = temp_dir("oversized");
        let path = dir.join("large.bin");
        fs::write(&path, [0_u8; 9]).expect("seed file");

        let snapshot = capture_snapshot_with_cap(&path, 8).expect("capture snapshot");
        assert_eq!(snapshot.content, None);
        assert_eq!(snapshot.skipped.map(|skip| skip.size_bytes), Some(9));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn binary_content_roundtrips_without_utf8_conversion() {
        let dir = temp_dir("binary");
        let path = dir.join("binary.dat");
        let before = [0, 159, 146, 150, 255];
        let after = [255, 0, 1, 2, 3];
        fs::write(&path, before).expect("seed binary");
        let mut store = WorkspaceCheckpointStore::default();
        store.begin_turn(1);
        store.record_before(&path).unwrap();
        fs::write(&path, after).expect("write binary");
        store.record_write_success(&path);

        let checkpoint = store.finish_turn().unwrap().unwrap();
        assert_eq!(checkpoint.files[0].before.content.as_deref(), Some(before.as_slice()));
        assert_eq!(checkpoint.files[0].after.content.as_deref(), Some(after.as_slice()));
        let encoded = serde_json::to_vec(&checkpoint).expect("serialize checkpoint");
        let decoded: WorkspaceCheckpoint =
            serde_json::from_slice(&encoded).expect("deserialize checkpoint");
        assert_eq!(decoded, checkpoint);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn ring_buffer_evicts_oldest_checkpoint() {
        let dir = temp_dir("ring");
        let path = dir.join("file.txt");
        fs::write(&path, b"0").unwrap();
        let mut store = WorkspaceCheckpointStore::default();
        for turn in 1..=(MAX_WORKSPACE_CHECKPOINTS + 1) {
            store.begin_turn(turn);
            store.record_before(&path).unwrap();
            fs::write(&path, turn.to_string()).unwrap();
            store.record_write_success(&path);
            store.finish_turn().unwrap();
        }
        let checkpoints = store.checkpoints();
        assert_eq!(checkpoints.len(), MAX_WORKSPACE_CHECKPOINTS);
        assert_eq!(checkpoints.first().map(|checkpoint| checkpoint.turn_index), Some(2));
        assert_eq!(checkpoints.last().map(|checkpoint| checkpoint.turn_index), Some(51));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn unsuccessful_write_does_not_create_checkpoint() {
        let dir = temp_dir("failed-write");
        let path = dir.join("file.txt");
        fs::write(&path, b"before").unwrap();
        let mut store = WorkspaceCheckpointStore::default();
        store.begin_turn(1);
        store.record_before(&path).unwrap();
        assert!(store.finish_turn().unwrap().is_none());
        assert!(store.checkpoints().is_empty());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_turn_number_a_list_row_shows_is_the_one_rewind_accepts() {
        let (dir, context) = history("numbers");
        let listing = render_workspace_checkpoint_list(&context.workspace_checkpoints());
        let shown: Vec<usize> = listing
            .lines()
            .filter_map(|line| line.split("Turn ").nth(1))
            .filter_map(|rest| rest.split_whitespace().next()?.parse().ok())
            .collect();
        assert!(shown.len() >= 2, "{listing}");
        for turn in shown {
            context
                .restore_workspace_to_before(turn, true)
                .unwrap_or_else(|error| panic!("/rewind {turn} refused: {error}"));
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_state_past_the_budget_is_marked_skipped_not_kept() {
        let dir = temp_dir("active-budget");
        let small = dir.join("small.bin");
        let big = dir.join("big.bin");
        fs::write(&small, vec![1u8; 10]).unwrap();
        fs::write(&big, vec![2u8; 50]).unwrap();
        let mut store = WorkspaceCheckpointStore {
            byte_cap: Some(40),
            ..WorkspaceCheckpointStore::default()
        };
        store.begin_turn(1);
        store.record_before(&small).unwrap();
        store.record_before(&big).unwrap();
        fs::write(&small, vec![3u8; 10]).unwrap();
        fs::write(&big, vec![4u8; 50]).unwrap();
        store.record_write_success(&small);
        store.record_write_success(&big);
        let checkpoint = store.finish_turn().unwrap().expect("a checkpoint");
        let by_name = |name: &str| {
            checkpoint
                .files
                .iter()
                .find(|file| file.path.file_name().is_some_and(|n| n == name))
                .expect("file recorded")
        };
        assert!(!by_name("small.bin").before.is_oversized());
        assert!(by_name("big.bin").before.is_oversized(), "past the budget: skipped");
        assert!(checkpoint.has_oversized_files());
        assert!(checkpoint.content_bytes() <= 40);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn retained_checkpoints_stay_under_the_byte_budget_and_keep_the_newest() {
        let dir = temp_dir("budget");
        let path = dir.join("big.bin");
        fs::write(&path, vec![0u8; 100]).unwrap();
        let mut store = WorkspaceCheckpointStore::default();
        for turn in 1..=3 {
            store.begin_turn(turn);
            store.record_before(&path).unwrap();
            fs::write(&path, vec![u8::try_from(turn).unwrap(); 100]).unwrap();
            store.record_write_success(&path);
            store.finish_turn().unwrap();
        }
        assert_eq!(store.checkpoints().len(), 3);
        // Each checkpoint holds 200 bytes (before + after): a 450-byte budget
        // keeps two, a 10-byte budget still keeps the newest one.
        store.evict_over_budget(450);
        let kept: Vec<usize> = store.checkpoints().iter().map(|c| c.turn_index).collect();
        assert_eq!(kept, vec![2, 3]);
        store.evict_over_budget(10);
        let kept: Vec<usize> = store.checkpoints().iter().map(|c| c.turn_index).collect();
        assert_eq!(kept, vec![3]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_composes_overlapping_paths_to_each_turn_boundary() {
        for (target, expected_a, expected_b, expected_c) in [
            (3, b"a2".as_slice(), b"b2".as_slice(), Some(b"c1".as_slice())),
            (2, b"a1".as_slice(), b"b0".as_slice(), Some(b"c1".as_slice())),
            (1, b"a0".as_slice(), b"b0".as_slice(), None),
        ] {
            let (dir, context) = history(&format!("restore-{target}"));

            let summary = context
                .restore_workspace_to_before(target, false)
                .expect("restore succeeds");

            assert!(summary.conflicted.is_empty());
            assert!(summary.skipped.is_empty());
            assert_eq!(fs::read(dir.join("a.bin")).unwrap(), expected_a);
            assert_eq!(fs::read(dir.join("b.bin")).unwrap(), expected_b);
            match expected_c {
                Some(expected_bytes) => {
                    assert_eq!(fs::read(dir.join("c.bin")).unwrap(), expected_bytes);
                }
                None => assert!(!dir.join("c.bin").exists()),
            }
            assert_eq!(
                context
                    .workspace_checkpoints()
                    .last()
                    .map(|checkpoint| checkpoint.turn_index),
                Some(4)
            );
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn conflict_blocks_restore_without_force_and_force_records_an_undo_checkpoint() {
        let dir = temp_dir("conflict");
        let path = dir.join("file.bin");
        fs::write(&path, b"before").unwrap();
        let mut context = ToolContext::new();
        context.workspace_root = Some(dir.clone());
        context.cwd = Some(dir.clone());
        checkpoint_changes(&context, 1, &[(path.as_path(), Some(b"after"))]);
        fs::write(&path, b"external").unwrap();

        let blocked = context
            .restore_workspace_to_before(1, false)
            .expect("conflict is reported");
        assert_eq!(blocked.conflicted, vec![path.clone()]);
        assert_eq!(fs::read(&path).unwrap(), b"external");
        assert_eq!(context.workspace_checkpoints().len(), 1);

        let forced = context
            .restore_workspace_to_before(1, true)
            .expect("forced restore succeeds");
        assert!(forced.conflicted.is_empty());
        assert_eq!(fs::read(&path).unwrap(), b"before");
        assert_eq!(forced.restore_checkpoint_turn_index, Some(2));
        let restore_checkpoint = context.workspace_checkpoints().pop().unwrap();
        assert_eq!(
            restore_checkpoint.files[0].before.content.as_deref(),
            Some(b"external".as_slice())
        );
        assert_eq!(
            restore_checkpoint.files[0].after.content.as_deref(),
            Some(b"before".as_slice())
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn list_rendering_numbers_newest_first_with_age_and_flags() {
        let checkpoints = vec![
            WorkspaceCheckpoint {
                turn_index: 4,
                created_at_epoch_secs: 1_000,
                files: vec![WorkspaceCheckpointFile::default()],
                ..WorkspaceCheckpoint::default()
            },
            WorkspaceCheckpoint {
                turn_index: 8,
                created_at_epoch_secs: 1_060,
                incomplete: true,
                files: vec![WorkspaceCheckpointFile {
                    before: WorkspaceFileSnapshot::skipped_size(11),
                    ..WorkspaceCheckpointFile::default()
                }],
                ..WorkspaceCheckpoint::default()
            },
        ];

        let rendered = render_workspace_checkpoint_list_at(&checkpoints, 1_120);

        assert!(rendered.contains("1. Turn 8 · 1m ago · 1 file(s) [incomplete, skipped-oversized]"));
        assert!(rendered.contains("2. Turn 4 · 2m ago · 1 file(s)"));
        assert!(rendered.contains("/rewind <Turn number> [force]"));
    }

    #[test]
    fn durable_roundtrip_loads_legacy_and_ignores_corrupt_files() {
        let dir = temp_dir("durable");
        let durable_dir = dir.join("checkpoints");
        let path = dir.join("file.bin");
        fs::write(&path, b"before").unwrap();
        let mut store = WorkspaceCheckpointStore::default();
        store
            .reset_session(Some(durable_dir.clone()))
            .expect("configure durability");
        store.begin_turn(1);
        store.record_before(&path).unwrap();
        fs::write(&path, b"after").unwrap();
        store.record_write_success(&path);
        store.finish_turn().expect("persist checkpoint");
        fs::write(
            durable_dir.join("checkpoint-legacy.json"),
            br#"{"turn_index":7,"created_at_epoch_secs":9,"incomplete":true,"files":[],"future_field":"ignored"}"#,
        )
        .unwrap();
        fs::write(durable_dir.join("checkpoint-corrupt.json"), b"not json").unwrap();

        let mut reloaded = WorkspaceCheckpointStore::default();
        reloaded
            .reset_session(Some(durable_dir))
            .expect("reload checkpoints");

        let checkpoints = reloaded.checkpoints();
        assert_eq!(
            checkpoints
                .iter()
                .map(|checkpoint| checkpoint.turn_index)
                .collect::<Vec<_>>(),
            vec![1, 7]
        );
        assert_eq!(checkpoints[0].files[0].before.content.as_deref(), Some(b"before".as_slice()));
        assert_eq!(checkpoints[1].schema_version, CHECKPOINT_SCHEMA_VERSION);
        assert!(checkpoints[1].incomplete);
        let _ = fs::remove_dir_all(dir);
    }
}
