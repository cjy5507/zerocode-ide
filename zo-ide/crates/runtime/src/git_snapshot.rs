use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct GitSnapshot {
    tree_hash: String,
    turn_number: usize,
}

#[derive(Debug)]
pub struct SnapshotStack {
    snapshots: Vec<GitSnapshot>,
    redo_stack: Vec<GitSnapshot>,
    git_root: PathBuf,
}

impl SnapshotStack {
    #[must_use]
    pub fn new(git_root: PathBuf) -> Self {
        Self {
            snapshots: Vec::new(),
            redo_stack: Vec::new(),
            git_root,
        }
    }

    #[must_use]
    pub fn try_new() -> Option<Self> {
        let cwd = std::env::current_dir().ok()?;
        Self::try_new_at(&cwd)
    }

    #[must_use]
    pub fn try_new_at(cwd: &Path) -> Option<Self> {
        let root = find_git_root(cwd)?;
        Some(Self::new(root))
    }

    pub fn capture(&mut self, turn_number: usize) -> Result<(), io::Error> {
        let tree_hash = write_worktree_tree(&self.git_root)?;
        self.adopt_capture(tree_hash, turn_number);
        Ok(())
    }

    /// Workspace root this stack snapshots — lets callers run
    /// [`compute_worktree_tree`] for it off-thread.
    #[must_use]
    pub fn git_root(&self) -> &Path {
        &self.git_root
    }

    /// Adopt a tree computed by [`compute_worktree_tree`] as the checkpoint
    /// for `turn_number` — the bookkeeping half of [`Self::capture`], split
    /// out so the expensive hash can run on a worker thread.
    pub fn adopt_capture(&mut self, tree_hash: String, turn_number: usize) {
        self.snapshots.push(GitSnapshot {
            tree_hash,
            turn_number,
        });
        self.redo_stack.clear();
    }

    pub fn undo(&mut self) -> Result<UndoResult, io::Error> {
        let [.., target, current] = self.snapshots.as_slice() else {
            return Err(io::Error::other("no previous state to undo to"));
        };
        let (current, target) = (current.clone(), target.clone());

        restore_tree(&self.git_root, &current.tree_hash, &target.tree_hash)?;

        self.snapshots.pop();
        self.redo_stack.push(current);

        Ok(UndoResult {
            restored_turn: target.turn_number,
            remaining: self.snapshots.len().saturating_sub(1),
        })
    }

    pub fn redo(&mut self) -> Result<UndoResult, io::Error> {
        let snapshot = self
            .redo_stack
            .last()
            .cloned()
            .ok_or_else(|| io::Error::other("no snapshots to redo"))?;
        let current = self
            .snapshots
            .last()
            .ok_or_else(|| io::Error::other("no current snapshot to redo from"))?
            .clone();

        restore_tree(&self.git_root, &current.tree_hash, &snapshot.tree_hash)?;
        self.redo_stack.pop();
        let restored_turn = snapshot.turn_number;
        self.snapshots.push(snapshot);

        Ok(UndoResult {
            restored_turn,
            remaining: self.redo_stack.len(),
        })
    }

    /// List every captured snapshot oldest-first, for an interactive rewind
    /// viewer. The last entry is the live worktree state (`is_current`).
    #[must_use]
    pub fn entries(&self) -> Vec<SnapshotEntry> {
        let last = self.snapshots.len().saturating_sub(1);
        self.snapshots
            .iter()
            .enumerate()
            .map(|(index, snapshot)| SnapshotEntry {
                index,
                turn_number: snapshot.turn_number,
                is_current: index == last,
            })
            .collect()
    }

    /// Per-file line deltas that snapshot `index` introduced over its
    /// predecessor (i.e. what that turn changed). The baseline (index 0) has
    /// no predecessor, so it returns an empty list.
    ///
    /// # Errors
    /// Propagates git failures; errors if `index` is out of range.
    pub fn diff_stat(&self, index: usize) -> Result<Vec<FileDelta>, io::Error> {
        let snapshot = self
            .snapshots
            .get(index)
            .ok_or_else(|| io::Error::other("snapshot index out of range"))?;
        if index == 0 {
            return Ok(Vec::new());
        }
        let prev = &self.snapshots[index - 1];
        numstat_between(&self.git_root, &prev.tree_hash, &snapshot.tree_hash)
    }

    /// The unified diff for a single `path` that snapshot `index` introduced
    /// over its predecessor. Empty for the baseline (index 0).
    ///
    /// # Errors
    /// Propagates git failures; errors if `index` is out of range.
    pub fn unified_diff(&self, index: usize, path: &str) -> Result<String, io::Error> {
        let snapshot = self
            .snapshots
            .get(index)
            .ok_or_else(|| io::Error::other("snapshot index out of range"))?;
        if index == 0 {
            return Ok(String::new());
        }
        let prev = &self.snapshots[index - 1];
        let output = git_output(
            &self.git_root,
            &["diff", &prev.tree_hash, &snapshot.tree_hash, "--", path],
        )?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    /// The full unified diff (all files) that snapshot `index` introduced over
    /// its predecessor, ready to feed a unified-diff parser. Empty for the
    /// baseline (index 0).
    ///
    /// # Errors
    /// Propagates git failures; errors if `index` is out of range.
    pub fn turn_diff(&self, index: usize) -> Result<String, io::Error> {
        let snapshot = self
            .snapshots
            .get(index)
            .ok_or_else(|| io::Error::other("snapshot index out of range"))?;
        if index == 0 {
            return Ok(String::new());
        }
        let prev = &self.snapshots[index - 1];
        let output = git_output(
            &self.git_root,
            &["diff", &prev.tree_hash, &snapshot.tree_hash],
        )?;
        Ok(String::from_utf8_lossy(&output).into_owned())
    }

    /// Rewind the worktree to an arbitrary earlier snapshot, moving every
    /// snapshot after `index` onto the redo stack (newest first) so `redo`
    /// replays them one step at a time. Generalizes [`Self::undo`], which is
    /// `rewind_to(depth - 2)`.
    ///
    /// # Errors
    /// Errors if `index` is the current snapshot or out of range, and refuses
    /// (via `restore_tree`) to overwrite a path edited since the snapshot.
    pub fn rewind_to(&mut self, index: usize) -> Result<UndoResult, io::Error> {
        if index + 1 >= self.snapshots.len() {
            return Err(io::Error::other(
                "rewind target is the current snapshot or out of range",
            ));
        }
        let current = self
            .snapshots
            .last()
            .expect("guard above ensures at least index + 2 snapshots")
            .clone();
        let target = self.snapshots[index].clone();
        restore_tree(&self.git_root, &current.tree_hash, &target.tree_hash)?;

        // Move everything after `index` onto the redo stack, newest first.
        let tail = self.snapshots.split_off(index + 1);
        self.redo_stack.extend(tail.into_iter().rev());

        Ok(UndoResult {
            restored_turn: target.turn_number,
            remaining: self.snapshots.len().saturating_sub(1),
        })
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.snapshots.len()
    }

    /// Dry-run companion to [`Self::undo`]: the tracked paths an Esc-Esc
    /// rewind would revert, without touching the worktree. `None` when there
    /// is no earlier snapshot to undo to (so the caller can say "nothing to
    /// rewind" rather than show an empty list). Used to populate the
    /// confirmation modal so a mistaken double-tap can be cancelled before any
    /// file is overwritten.
    #[must_use]
    pub fn preview_undo(&self) -> Option<Vec<PathBuf>> {
        let [.., target, current] = self.snapshots.as_slice() else {
            return None;
        };
        changed_paths_between(&self.git_root, &current.tree_hash, &target.tree_hash).ok()
    }

    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.redo_stack.len()
    }
}

#[derive(Debug)]
struct TempIndex {
    path: PathBuf,
}

impl TempIndex {
    fn new() -> Result<Self, io::Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                io::Error::other(format!("system clock before unix epoch: {error}"))
            })?;
        let path = std::env::temp_dir().join(format!(
            "zo-snapshot-index-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        Ok(Self { path })
    }

    fn git(&self, git_root: &Path) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(git_root)
            .env("GIT_INDEX_FILE", &self.path);
        command
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let lock = self.path.with_extension("lock");
        let _ = std::fs::remove_file(lock);
    }
}

#[derive(Debug)]
pub struct UndoResult {
    pub restored_turn: usize,
    pub remaining: usize,
}

/// One row in the interactive rewind viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotEntry {
    /// Position in the stack (0 = baseline, ascending).
    pub index: usize,
    /// The turn this snapshot was captured after.
    pub turn_number: usize,
    /// Whether this is the live worktree state (the newest snapshot).
    pub is_current: bool,
}

/// Per-file line delta between two snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDelta {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// The git work-tree root for `cwd` via `git rev-parse --show-toplevel`,
/// decoding stdout strictly (non-UTF-8 → `None`) and rejecting an empty result.
///
/// Shared by the system-prompt builder and the skill-tools project resolver so
/// both agree on how the project root is located. Distinct from the
/// snapshot-internal `find_git_root`, which decodes lossily for diff display.
///
/// Two things keep `git` itself off the hot path (t-2902). A fork+exec of
/// `git` costs 15–28 ms on macOS (`/usr/bin/git` is Apple's `xcrun` shim,
/// so it is two execs), and every sub-agent spawn asked this question twice —
/// once for the prompt's project root, once for the skill roots — inside the
/// window between the parent's `Agent` call and the child's first provider
/// request. So:
///
/// 1. a `cwd` with no `.git` entry on itself or any ancestor is in no
///    repository, and is answered `None` without asking `git` at all —
///    unless `GIT_DIR`/`GIT_WORK_TREE`/`GIT_COMMON_DIR` name one elsewhere,
///    in which case `git` is asked as before;
/// 2. a root `git` did name is remembered for the life of the process, per
///    `cwd`: a work tree's top level cannot move while zo runs in it. Only
///    positive answers are kept, so a `.git` that `git` refuses today
///    (dubious ownership, a broken link) is asked again tomorrow.
#[must_use]
pub fn read_git_root(cwd: &Path) -> Option<PathBuf> {
    read_git_root_with(cwd, git_toplevel)
}

/// [`read_git_root`] with the `git` call injected — the memo and the ancestry
/// precheck, testable without a repository or a subprocess.
fn read_git_root_with(
    cwd: &Path,
    git_toplevel: impl FnOnce(&Path) -> Option<PathBuf>,
) -> Option<PathBuf> {
    if !repository_ancestry_possible(cwd) {
        return None;
    }
    if let Some(root) = remembered_git_roots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(cwd)
    {
        return Some(root.clone());
    }
    let root = git_toplevel(cwd)?;
    remembered_git_roots()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(cwd.to_path_buf(), root.clone());
    Some(root)
}

/// Whether `git` could possibly find a repository from `cwd`: a `.git` entry
/// (a directory, or the file a linked work tree or submodule carries) on `cwd`
/// or an ancestor, or an environment variable that points `git` somewhere
/// else entirely. `false` is a definite "not in a repository".
fn repository_ancestry_possible(cwd: &Path) -> bool {
    if ["GIT_DIR", "GIT_WORK_TREE", "GIT_COMMON_DIR"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return true;
    }
    cwd.ancestors().any(|dir| dir.join(".git").exists())
}

/// Process-lifetime memo of `cwd → work-tree root`, positive answers only.
fn remembered_git_roots() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, PathBuf>>
{
    static ROOTS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, PathBuf>>,
    > = std::sync::OnceLock::new();
    ROOTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// One `git rev-parse --show-toplevel`, strictly decoded.
fn git_toplevel(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

fn find_git_root(start: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(PathBuf::from(root))
}

fn git_output(git_root: &Path, args: &[&str]) -> Result<Vec<u8>, io::Error> {
    let output = Command::new("git")
        .args(args)
        .current_dir(git_root)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output.stdout)
}

/// The blocking half of [`SnapshotStack::capture`]: hash the current worktree
/// into a git tree without touching any stack state. Forks `git add`/`git
/// write-tree` over the whole worktree — seconds on a large or cold repo — so
/// latency-sensitive callers run it on a worker thread and hand the result to
/// [`SnapshotStack::adopt_capture`].
pub fn compute_worktree_tree(git_root: &Path) -> Result<String, io::Error> {
    write_worktree_tree(git_root)
}

fn write_worktree_tree(git_root: &Path) -> Result<String, io::Error> {
    let index = TempIndex::new()?;
    let has_head = Command::new("git")
        .args(["rev-parse", "--verify", "HEAD^{tree}"])
        .current_dir(git_root)
        .output()?
        .status
        .success();

    let read_tree_args = if has_head {
        vec!["read-tree", "HEAD"]
    } else {
        vec!["read-tree", "--empty"]
    };
    let output = index.git(git_root).args(read_tree_args).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git read-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let output = index
        .git(git_root)
        .args(["add", "-A", "--", "."])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git add -A failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let output = index.git(git_root).args(["write-tree"]).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git write-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn restore_tree(git_root: &Path, current_tree: &str, target_tree: &str) -> Result<(), io::Error> {
    let changed_paths = changed_paths_between(git_root, current_tree, target_tree)?;
    for path in &changed_paths {
        if !worktree_matches_tree_path(git_root, current_tree, path)? {
            return Err(io::Error::other(format!(
                "{} changed since snapshot; refusing to overwrite",
                path.display()
            )));
        }
    }

    for path in changed_paths {
        restore_path_from_tree(git_root, target_tree, &path)?;
    }
    Ok(())
}

/// Per-file `(added, removed)` line counts between two trees, via
/// `git diff --numstat -z --no-renames`. Binary files report zero.
fn numstat_between(
    git_root: &Path,
    from_tree: &str,
    to_tree: &str,
) -> Result<Vec<FileDelta>, io::Error> {
    let output = git_output(
        git_root,
        &[
            "diff",
            "--numstat",
            "-z",
            "--no-renames",
            from_tree,
            to_tree,
            "--",
        ],
    )?;
    Ok(parse_numstat_z(&output))
}

/// Parse `git diff --numstat -z --no-renames` output: NUL-terminated records of
/// `added\tremoved\tpath`, where binary files carry `-` counts.
fn parse_numstat_z(bytes: &[u8]) -> Vec<FileDelta> {
    let text = String::from_utf8_lossy(bytes);
    let mut deltas = Vec::new();
    for record in text.split('\0') {
        if record.is_empty() {
            continue;
        }
        let mut fields = record.splitn(3, '\t');
        let added = fields.next().unwrap_or("0");
        let removed = fields.next().unwrap_or("0");
        let Some(path) = fields.next() else {
            continue;
        };
        deltas.push(FileDelta {
            path: path.to_string(),
            added: added.parse().unwrap_or(0),
            removed: removed.parse().unwrap_or(0),
        });
    }
    deltas
}

fn changed_paths_between(
    git_root: &Path,
    from_tree: &str,
    to_tree: &str,
) -> Result<Vec<PathBuf>, io::Error> {
    let output = git_output(
        git_root,
        &["diff", "--name-only", "-z", from_tree, to_tree, "--"],
    )?;
    Ok(output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(String::from_utf8_lossy(path).into_owned()))
        .collect())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TreePathKind {
    File,
    Executable,
    Symlink,
}

fn tree_path_kind(
    git_root: &Path,
    tree_hash: &str,
    path: &Path,
) -> Result<Option<TreePathKind>, io::Error> {
    let path = path_to_git_path(path);
    let output = Command::new("git")
        .args(["ls-tree", "-z", tree_hash, "--", &path])
        .current_dir(git_root)
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "git ls-tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }

    let entry = output
        .stdout
        .split(|byte| *byte == b'\t')
        .next()
        .unwrap_or_default();
    let header = String::from_utf8_lossy(entry);
    let mode = header.split_whitespace().next().unwrap_or_default();
    Ok(match mode {
        "100755" => Some(TreePathKind::Executable),
        "120000" => Some(TreePathKind::Symlink),
        // "100644" and all other modes default to File
        _ => Some(TreePathKind::File),
    })
}

fn worktree_matches_tree_path(
    git_root: &Path,
    tree_hash: &str,
    path: &Path,
) -> Result<bool, io::Error> {
    let Some(kind) = tree_path_kind(git_root, tree_hash, path)? else {
        return Ok(!path_exists(&git_root.join(path)));
    };

    let actual_path = git_root.join(path);
    let Ok(metadata) = std::fs::symlink_metadata(&actual_path) else {
        return Ok(false);
    };

    let expected = read_tree_blob(git_root, tree_hash, path)?;
    match kind {
        TreePathKind::Symlink => symlink_matches(&actual_path, &expected),
        TreePathKind::File | TreePathKind::Executable => {
            if !metadata.file_type().is_file() {
                return Ok(false);
            }
            Ok(std::fs::read(actual_path)? == expected)
        }
    }
}

fn restore_path_from_tree(git_root: &Path, tree_hash: &str, path: &Path) -> Result<(), io::Error> {
    let actual_path = git_root.join(path);
    let Some(kind) = tree_path_kind(git_root, tree_hash, path)? else {
        remove_path_if_present(&actual_path)?;
        remove_empty_parent_dirs(git_root, path)?;
        return Ok(());
    };

    if let Some(parent) = actual_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    remove_path_if_present(&actual_path)?;

    let blob = read_tree_blob(git_root, tree_hash, path)?;
    match kind {
        TreePathKind::Symlink => restore_symlink(&actual_path, &blob)?,
        TreePathKind::File | TreePathKind::Executable => {
            std::fs::write(&actual_path, blob)?;
            set_executable(&actual_path, kind == TreePathKind::Executable)?;
        }
    }
    Ok(())
}

fn read_tree_blob(git_root: &Path, tree_hash: &str, path: &Path) -> Result<Vec<u8>, io::Error> {
    let spec = format!("{tree_hash}:{}", path_to_git_path(path));
    git_output(git_root, &["show", &spec])
}

fn path_to_git_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn remove_path_if_present(path: &Path) -> Result<(), io::Error> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn remove_empty_parent_dirs(git_root: &Path, path: &Path) -> Result<(), io::Error> {
    let mut parent = git_root.join(path).parent().map(Path::to_path_buf);
    while let Some(dir) = parent {
        if dir == git_root {
            break;
        }
        match std::fs::remove_dir(&dir) {
            Ok(()) => parent = dir.parent().map(Path::to_path_buf),
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                parent = dir.parent().map(Path::to_path_buf);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink_matches(path: &Path, expected: &[u8]) -> Result<bool, io::Error> {
    use std::os::unix::ffi::OsStrExt;

    let target = std::fs::read_link(path)?;
    Ok(target.as_os_str().as_bytes() == expected)
}

#[cfg(windows)]
fn symlink_matches(path: &Path, expected: &[u8]) -> Result<bool, io::Error> {
    let target = std::fs::read_link(path)?;
    let rendered = target.to_string_lossy().replace('\\', "/");
    Ok(rendered.as_bytes() == expected)
}

#[cfg(not(any(unix, windows)))]
fn symlink_matches(path: &Path, expected: &[u8]) -> Result<bool, io::Error> {
    Ok(std::fs::read(path)? == expected)
}

#[cfg(unix)]
fn restore_symlink(path: &Path, target: &[u8]) -> Result<(), io::Error> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    std::os::unix::fs::symlink(OsStr::from_bytes(target), path)
}

#[cfg(windows)]
fn restore_symlink(path: &Path, target: &[u8]) -> Result<(), io::Error> {
    let target = std::str::from_utf8(target).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Git symlink target is not valid UTF-8 on Windows",
        )
    })?;
    let target = PathBuf::from(target);
    let resolved = if target.is_absolute() {
        target.clone()
    } else {
        path.parent().unwrap_or_else(|| Path::new("")).join(&target)
    };
    // Windows requires the link kind at creation. Use the target's actual kind
    // when it exists; a missing target is represented as a file symlink. If
    // Developer Mode/SeCreateSymbolicLinkPrivilege is unavailable, propagate
    // the error rather than silently materializing attacker-controlled blob
    // bytes as a regular file.
    if resolved.is_dir() {
        std::os::windows::fs::symlink_dir(target, path)
    } else {
        std::os::windows::fs::symlink_file(target, path)
    }
}

#[cfg(not(any(unix, windows)))]
fn restore_symlink(path: &Path, target: &[u8]) -> Result<(), io::Error> {
    std::fs::write(path, target)
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) -> Result<(), io::Error> {
    use std::os::unix::fs::PermissionsExt;

    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn set_executable(_path: &Path, _executable: bool) -> Result<(), io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A directory with no `.git` on itself or any ancestor is answered
    /// without a `git` process (t-2902): the ancestry precheck says "not a
    /// repository" and the injected `git` is never called.
    #[test]
    fn a_cwd_outside_every_repository_never_spawns_git() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let mut asked = 0;
        let root = read_git_root_with(&nested, |_| {
            asked += 1;
            Some(PathBuf::from("/never"))
        });
        assert_eq!(root, None);
        assert_eq!(asked, 0, "no `.git` anywhere above → git is not asked");
    }

    /// The `.git` a linked work tree carries is a FILE, and it still marks the
    /// tree as a repository: the precheck lets `git` answer.
    #[test]
    fn a_git_file_marks_a_linked_work_tree_as_a_repository() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".git"), "gitdir: /elsewhere/.git/worktrees/x\n").unwrap();
        let nested = dir.path().join("src");
        fs::create_dir_all(&nested).unwrap();
        let mut asked = 0;
        let root = read_git_root_with(&nested, |cwd| {
            asked += 1;
            assert_eq!(cwd, nested.as_path());
            Some(dir.path().to_path_buf())
        });
        assert_eq!(root.as_deref(), Some(dir.path()));
        assert_eq!(asked, 1);
    }

    /// A root `git` named once is remembered for that `cwd`; a refusal is not,
    /// so the next caller asks again.
    #[test]
    fn a_named_root_is_remembered_and_a_refusal_is_not() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        let refused = dir.path().join("refused");
        let named = dir.path().join("named");
        fs::create_dir_all(&refused).unwrap();
        fs::create_dir_all(&named).unwrap();

        let mut asked = 0;
        for _ in 0..2 {
            let root = read_git_root_with(&refused, |_| {
                asked += 1;
                None
            });
            assert_eq!(root, None);
        }
        assert_eq!(asked, 2, "a refusal is asked again");

        let mut asked = 0;
        for _ in 0..3 {
            let root = read_git_root_with(&named, |_| {
                asked += 1;
                Some(dir.path().to_path_buf())
            });
            assert_eq!(root.as_deref(), Some(dir.path()));
        }
        assert_eq!(asked, 1, "a named root is asked once per process");
    }

    fn setup_temp_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        Command::new("git")
            .args(["init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        fs::write(dir.path().join("init.txt"), "init").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(dir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "init"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        dir
    }

    #[test]
    fn snapshot_capture_and_undo_restores_files() {
        // Serialize git-spawning tests against process-global env mutators
        // (e.g. the prompt test that temporarily repoints $HOME), so a
        // concurrent HOME change can't corrupt these git invocations.
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());

        fs::write(dir.path().join("a.txt"), "version1").unwrap();
        stack.capture(1).unwrap();
        assert_eq!(stack.depth(), 1);

        fs::write(dir.path().join("a.txt"), "version2").unwrap();
        stack.capture(2).unwrap();
        assert_eq!(stack.depth(), 2);

        let result = stack.undo().unwrap();
        assert_eq!(result.restored_turn, 1);
        assert_eq!(stack.depth(), 1);
        let content = fs::read_to_string(dir.path().join("a.txt")).unwrap();
        assert_eq!(content, "version1");
    }

    #[test]
    fn preview_undo_lists_changes_without_touching_worktree() {
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());

        // No snapshot yet → nothing earlier to preview.
        assert!(stack.preview_undo().is_none());

        fs::write(dir.path().join("a.txt"), "version1").unwrap();
        stack.capture(1).unwrap();
        // Only one snapshot → still nothing earlier to undo to.
        assert!(stack.preview_undo().is_none());

        fs::write(dir.path().join("a.txt"), "version2").unwrap();
        stack.capture(2).unwrap();

        // Preview reports the path the undo would revert …
        let preview = stack.preview_undo().expect("two snapshots → Some");
        assert_eq!(preview, vec![PathBuf::from("a.txt")]);

        // … without touching the worktree or mutating the stack (dry run).
        assert_eq!(
            fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "version2"
        );
        assert_eq!(stack.depth(), 2);
    }

    #[test]
    fn redo_reverses_undo() {
        // Serialize git-spawning tests against process-global env mutators
        // (e.g. the prompt test that temporarily repoints $HOME), so a
        // concurrent HOME change can't corrupt these git invocations.
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());

        fs::write(dir.path().join("b.txt"), "first").unwrap();
        stack.capture(1).unwrap();

        fs::write(dir.path().join("b.txt"), "second").unwrap();
        stack.capture(2).unwrap();

        stack.undo().unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "first"
        );

        stack.redo().unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("b.txt")).unwrap(),
            "second"
        );
    }

    #[test]
    fn capture_preserves_real_index() {
        // Serialize git-spawning tests against process-global env mutators
        // (e.g. the prompt test that temporarily repoints $HOME), so a
        // concurrent HOME change can't corrupt these git invocations.
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let staged = dir.path().join("staged.txt");
        let unstaged = dir.path().join("unstaged.txt");
        let untracked = dir.path().join("untracked.txt");
        fs::write(&staged, "staged").unwrap();
        fs::write(&unstaged, "unstaged").unwrap();
        fs::write(&untracked, "untracked").unwrap();

        Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(dir.path())
            .output()
            .unwrap();

        let mut stack = SnapshotStack::new(dir.path().to_path_buf());
        stack.capture(1).unwrap();

        let output = Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), "staged.txt\n");
    }

    #[test]
    fn undo_preserves_unrelated_untracked_files() {
        // Serialize git-spawning tests against process-global env mutators
        // (e.g. the prompt test that temporarily repoints $HOME), so a
        // concurrent HOME change can't corrupt these git invocations.
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());

        fs::write(dir.path().join("tracked.txt"), "one").unwrap();
        stack.capture(1).unwrap();

        fs::write(dir.path().join("tracked.txt"), "two").unwrap();
        stack.capture(2).unwrap();

        fs::write(dir.path().join("user-note.txt"), "do not delete").unwrap();
        stack.undo().unwrap();

        assert_eq!(
            fs::read_to_string(dir.path().join("tracked.txt")).unwrap(),
            "one"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("user-note.txt")).unwrap(),
            "do not delete"
        );
    }

    #[test]
    fn baseline_then_turn_capture_undo_restores_pre_turn_code() {
        // Mirrors the Esc-Esc checkpoint model: capture a pristine
        // baseline at session start (turn 0), then capture the post-turn
        // tree after a turn edits a file. A single undo must roll the
        // worktree back to the pre-turn baseline.
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());

        // Session start: baseline checkpoint of the pristine worktree.
        stack.capture(0).unwrap();
        assert_eq!(stack.depth(), 1);

        // A turn edits a tracked file and adds a new file.
        fs::write(dir.path().join("init.txt"), "edited by turn").unwrap();
        fs::write(dir.path().join("turn-new.txt"), "created by turn").unwrap();
        // Post-turn checkpoint.
        stack.capture(2).unwrap();
        assert_eq!(stack.depth(), 2);

        // Esc-Esc: undo the turn's code edits.
        let result = stack.undo().unwrap();
        assert_eq!(result.restored_turn, 0);
        assert_eq!(
            fs::read_to_string(dir.path().join("init.txt")).unwrap(),
            "init",
            "tracked file restored to baseline"
        );
        assert!(
            !dir.path().join("turn-new.txt").exists(),
            "file created during the turn is removed on rewind"
        );
    }

    #[test]
    fn undo_refuses_to_overwrite_path_changed_after_snapshot() {
        // Serialize git-spawning tests against process-global env mutators
        // (e.g. the prompt test that temporarily repoints $HOME), so a
        // concurrent HOME change can't corrupt these git invocations.
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());

        fs::write(dir.path().join("tracked.txt"), "one").unwrap();
        stack.capture(1).unwrap();

        fs::write(dir.path().join("tracked.txt"), "two").unwrap();
        stack.capture(2).unwrap();

        fs::write(dir.path().join("tracked.txt"), "user edit").unwrap();
        let err = stack.undo().expect_err("user edit should block undo");

        assert!(err.to_string().contains("changed since snapshot"));
        assert_eq!(
            fs::read_to_string(dir.path().join("tracked.txt")).unwrap(),
            "user edit"
        );
        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.redo_depth(), 0);
    }

    #[test]
    fn entries_lists_snapshots_with_current_flag() {
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());
        stack.capture(0).unwrap();
        fs::write(dir.path().join("a.txt"), "x").unwrap();
        stack.capture(1).unwrap();

        let entries = stack.entries();
        assert_eq!(
            entries,
            vec![
                SnapshotEntry {
                    index: 0,
                    turn_number: 0,
                    is_current: false,
                },
                SnapshotEntry {
                    index: 1,
                    turn_number: 1,
                    is_current: true,
                },
            ]
        );
    }

    #[test]
    fn diff_stat_reports_lines_a_turn_changed() {
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());
        stack.capture(0).unwrap();
        fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
        stack.capture(1).unwrap();

        assert!(
            stack.diff_stat(0).unwrap().is_empty(),
            "baseline introduced nothing"
        );
        let stat = stack.diff_stat(1).unwrap();
        assert_eq!(
            stat,
            vec![FileDelta {
                path: "a.txt".to_string(),
                added: 3,
                removed: 0,
            }]
        );
    }

    #[test]
    fn unified_diff_shows_turn_change_for_path() {
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());
        fs::write(dir.path().join("a.txt"), "old\n").unwrap();
        stack.capture(0).unwrap();
        fs::write(dir.path().join("a.txt"), "new\n").unwrap();
        stack.capture(1).unwrap();

        let diff = stack.unified_diff(1, "a.txt").unwrap();
        assert!(diff.contains("-old"), "diff should show the removed line");
        assert!(diff.contains("+new"), "diff should show the added line");
        assert!(stack.unified_diff(0, "a.txt").unwrap().is_empty());
    }

    #[test]
    fn turn_diff_spans_every_file_a_turn_touched() {
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());
        stack.capture(0).unwrap();
        fs::write(dir.path().join("a.txt"), "alpha\n").unwrap();
        fs::write(dir.path().join("b.txt"), "bravo\n").unwrap();
        stack.capture(1).unwrap();

        let diff = stack.turn_diff(1).unwrap();
        assert!(diff.contains("a.txt"), "turn diff covers a.txt");
        assert!(diff.contains("b.txt"), "turn diff covers b.txt");
        assert!(diff.contains("+alpha"));
        assert!(diff.contains("+bravo"));
        assert!(stack.turn_diff(0).unwrap().is_empty());
    }

    #[test]
    fn rewind_to_jumps_multiple_snapshots_and_redo_replays() {
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());
        let file = dir.path().join("a.txt");
        fs::write(&file, "v0").unwrap();
        stack.capture(0).unwrap();
        fs::write(&file, "v1").unwrap();
        stack.capture(1).unwrap();
        fs::write(&file, "v2").unwrap();
        stack.capture(2).unwrap();
        assert_eq!(stack.depth(), 3);

        // Jump straight back to the baseline (two steps in one call).
        let result = stack.rewind_to(0).unwrap();
        assert_eq!(result.restored_turn, 0);
        assert_eq!(stack.depth(), 1);
        assert_eq!(stack.redo_depth(), 2);
        assert_eq!(fs::read_to_string(&file).unwrap(), "v0");

        // Redo replays forward one snapshot at a time.
        stack.redo().unwrap();
        assert_eq!(fs::read_to_string(&file).unwrap(), "v1");
    }

    #[test]
    fn rewind_to_rejects_current_and_out_of_range() {
        let _env = crate::test_env_lock();
        let dir = setup_temp_repo();
        let mut stack = SnapshotStack::new(dir.path().to_path_buf());
        fs::write(dir.path().join("a.txt"), "v0").unwrap();
        stack.capture(0).unwrap();
        fs::write(dir.path().join("a.txt"), "v1").unwrap();
        stack.capture(1).unwrap();

        assert!(
            stack.rewind_to(1).is_err(),
            "index 1 is the current snapshot"
        );
        assert!(stack.rewind_to(9).is_err(), "index 9 is out of range");
    }

    #[test]
    fn parse_numstat_z_handles_records_and_binary() {
        let deltas = parse_numstat_z(b"3\t1\tsrc/a.rs\0-\t-\tlogo.png\0");
        assert_eq!(
            deltas,
            vec![
                FileDelta {
                    path: "src/a.rs".to_string(),
                    added: 3,
                    removed: 1,
                },
                FileDelta {
                    path: "logo.png".to_string(),
                    added: 0,
                    removed: 0,
                },
            ]
        );
    }
}
