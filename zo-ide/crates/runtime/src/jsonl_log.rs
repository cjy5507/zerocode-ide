//! Shared JSONL log-file maintenance for the rotating per-turn trace and the
//! dreamer / self-improve candidate logs.
//!
//! Both writers keep append-only `.jsonl` histories that need the same two
//! operations: line-count pruning (retain only the newest N records, rewritten
//! atomically through a temp file) and mtime-ordered file discovery (for
//! rotating whole files). These were re-implemented identically in `turn_trace`
//! and `memory::dreamer`; this module is the single source of truth.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

/// Rewrite `path` to retain only its newest `keep_lines` non-empty lines, or
/// delete it entirely when `keep_lines == 0`. The rewrite goes through a
/// sibling `.jsonl.tmp` file and an atomic rename, and is a no-op when the file
/// already holds `<= keep_lines` lines.
///
/// # Errors
/// Propagates I/O errors from opening, creating, renaming, or cleaning up the
/// log file.
pub(crate) fn prune_jsonl_lines(path: &Path, keep_lines: usize) -> std::io::Result<()> {
    if keep_lines == 0 {
        return remove_path_if_present(path);
    }
    let lines = read_non_empty_lines(open_jsonl_no_symlink(path)?)?;
    let total = lines.len();
    if total <= keep_lines {
        return Ok(());
    }
    let retained = lines.into_iter().skip(total - keep_lines);
    rewrite_path_lines(path, retained)
}

/// Read all non-empty lines from a real JSONL file without following symlinks.
///
/// # Errors
/// Propagates metadata, open, and read errors. An absent path is an error.
pub(crate) fn read_jsonl_lines(path: &Path) -> std::io::Result<Vec<String>> {
    read_non_empty_lines(open_jsonl_no_symlink(path)?)
}

/// Read `path`'s non-empty lines (symlink-safe), hand them to `transform`, and
/// atomically rewrite the file with whatever it returns. Missing files and
/// unchanged line counts are left alone.
pub(crate) fn rewrite_jsonl_lines_if_changed(
    path: &Path,
    transform: impl FnOnce(Vec<String>) -> Vec<String>,
) -> std::io::Result<()> {
    let file = match open_jsonl_no_symlink(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let lines = read_non_empty_lines(file)?;
    let original_len = lines.len();
    let retained = transform(lines);
    if retained.len() == original_len {
        return Ok(());
    }
    rewrite_path_lines(path, retained)
}

fn read_non_empty_lines(file: File) -> std::io::Result<Vec<String>> {
    BufReader::new(file)
        .lines()
        .filter_map(|result| match result {
            Ok(line) if line.trim().is_empty() => None,
            result => Some(result),
        })
        .collect()
}

fn open_jsonl_no_symlink(path: &Path) -> std::io::Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "jsonl path is not a real file",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "jsonl path is not a real file",
        ));
    }
    Ok(file)
}

fn rewrite_path_lines(
    path: &Path,
    lines: impl IntoIterator<Item = String>,
) -> std::io::Result<()> {
    let tmp = tmp_jsonl_path(path);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    let write_result = (|| {
        for line in lines {
            writeln!(file, "{line}")?;
        }
        file.sync_all()
    })();
    drop(file);
    if let Err(error) = write_result {
        return Err(with_path_cleanup(error, &tmp));
    }
    fs::rename(&tmp, path).map_err(|error| with_path_cleanup(error, &tmp))
}

fn remove_path_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn with_path_cleanup(error: std::io::Error, tmp: &Path) -> std::io::Error {
    match fs::remove_file(tmp) {
        Ok(()) => error,
        Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => error,
        Err(cleanup) => std::io::Error::other(format!(
            "{error}; JSONL temporary file cleanup also failed: {cleanup}"
        )),
    }
}

fn tmp_jsonl_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("log.jsonl");
    path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ))
}

#[cfg(all(unix, test))]
pub(crate) fn append_jsonl_line_retained(
    dir: &crate::secure_fs::RetainedDir,
    relative: &Path,
    line: &str,
) -> std::io::Result<()> {
    dir.append_owner_only(relative, line.as_bytes())
}

#[cfg(unix)]
pub(crate) fn append_jsonl_line_retained_durable(
    dir: &crate::secure_fs::RetainedDir,
    relative: &Path,
    line: &str,
) -> std::io::Result<()> {
    dir.append_owner_only_durable(relative, line.as_bytes())
}

#[cfg(unix)]
pub(crate) fn read_jsonl_lines_retained(
    dir: &crate::secure_fs::RetainedDir,
    relative: &Path,
) -> std::io::Result<Vec<String>> {
    let mut file = dir.open_regular_file(relative)?;
    Ok(file
        .read_to_string()?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

#[cfg(unix)]
pub(crate) fn rewrite_jsonl_lines_if_changed_retained(
    dir: &crate::secure_fs::RetainedDir,
    relative: &Path,
    transform: impl FnOnce(Vec<String>) -> Vec<String>,
) -> std::io::Result<()> {
    let lines = match read_jsonl_lines_retained(dir, relative) {
        Ok(lines) => lines,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let original_len = lines.len();
    let retained = transform(lines);
    if retained.len() == original_len {
        return Ok(());
    }
    let contents = lines_to_bytes(retained);
    dir.write_atomic_owner_only_retained(relative, &contents)
}

#[cfg(unix)]
pub(crate) fn prune_jsonl_lines_retained(
    dir: &crate::secure_fs::RetainedDir,
    relative: &Path,
    keep_lines: usize,
) -> std::io::Result<()> {
    if keep_lines == 0 {
        dir.remove_regular_file(relative)?;
        return Ok(());
    }
    rewrite_jsonl_lines_if_changed_retained(dir, relative, |lines| {
        let total = lines.len();
        lines
            .into_iter()
            .skip(total.saturating_sub(keep_lines))
            .collect()
    })
}

#[cfg(unix)]
fn lines_to_bytes(lines: impl IntoIterator<Item = String>) -> Vec<u8> {
    let mut contents = Vec::new();
    for line in lines {
        contents.extend_from_slice(line.as_bytes());
        contents.push(b'\n');
    }
    contents
}

/// All `*.jsonl` files in `dir`, newest first by mtime (with a deterministic
/// descending-path tiebreak). Returns an empty vec when `dir` is unreadable.
pub(crate) fn jsonl_files_newest_first(dir: &Path) -> Vec<PathBuf> {
    let Ok(metadata) = fs::symlink_metadata(dir) else {
        return Vec::new();
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            if !file_type.is_file() {
                return None;
            }
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "jsonl") {
                return None;
            }
            let modified = entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            Some((modified, path))
        })
        .collect();
    files.sort_by(|(left_time, left_path), (right_time, right_path)| {
        right_time
            .cmp(left_time)
            .then_with(|| right_path.cmp(left_path))
    });
    files.into_iter().map(|(_, path)| path).collect()
}

#[cfg(unix)]
pub(crate) fn jsonl_files_newest_first_retained(
    dir: &crate::secure_fs::RetainedDir,
) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for name in dir.entry_names()? {
        let path = PathBuf::from(name);
        if path.extension().is_none_or(|extension| extension != "jsonl") {
            continue;
        }
        let file = match dir.open_regular_file(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        files.push((file.modified().unwrap_or(std::time::UNIX_EPOCH), path));
    }
    files.sort_by(|(left_time, left_path), (right_time, right_path)| {
        right_time
            .cmp(left_time)
            .then_with(|| right_path.cmp(left_path))
    });
    Ok(files.into_iter().map(|(_, path)| path).collect())
}

#[cfg(unix)]
pub(crate) fn prune_jsonl_files_retained(
    dir: &crate::secure_fs::RetainedDir,
    keep_files: usize,
) -> std::io::Result<()> {
    for path in jsonl_files_newest_first_retained(dir)?
        .into_iter()
        .skip(keep_files)
    {
        dir.remove_regular_file(&path)?;
    }
    Ok(())
}

/// The newest `keep_latest_files` `*.jsonl` files in `dir`, returned oldest
/// first — the order to delete/rotate from.
pub(crate) fn jsonl_files_oldest_first(dir: &Path, keep_latest_files: usize) -> Vec<PathBuf> {
    let mut files = jsonl_files_newest_first(dir);
    if files.len() > keep_latest_files {
        files.truncate(keep_latest_files);
    }
    files.reverse();
    files
}

#[cfg(unix)]
pub(crate) fn jsonl_files_oldest_first_retained(
    dir: &crate::secure_fs::RetainedDir,
    keep_latest_files: usize,
) -> std::io::Result<Vec<PathBuf>> {
    let mut files = jsonl_files_newest_first_retained(dir)?;
    if files.len() > keep_latest_files {
        files.truncate(keep_latest_files);
    }
    files.reverse();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_pruning_retains_latest_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("events.jsonl");
        fs::write(&path, "one\n\ntwo\nthree\n").unwrap();

        prune_jsonl_lines(&path, 2).unwrap();

        assert_eq!(fs::read_to_string(path).unwrap(), "two\nthree\n");
    }

    #[cfg(unix)]
    #[test]
    fn retained_jsonl_operations_survive_parent_replacement() {
        let tmp = tempfile::tempdir().unwrap();
        let current = tmp.path().join("candidates");
        let original = tmp.path().join("original-candidates");
        fs::create_dir(&current).unwrap();
        fs::write(current.join("candidate.jsonl"), "one\ntwo\n").unwrap();
        let retained = crate::secure_fs::RetainedDir::open(&current).unwrap();

        fs::rename(&current, &original).unwrap();
        fs::create_dir(&current).unwrap();
        fs::write(current.join("candidate.jsonl"), "replacement\n").unwrap();

        append_jsonl_line_retained(&retained, Path::new("candidate.jsonl"), "three\n").unwrap();
        prune_jsonl_lines_retained(&retained, Path::new("candidate.jsonl"), 2).unwrap();

        assert_eq!(
            fs::read_to_string(original.join("candidate.jsonl")).unwrap(),
            "two\nthree\n"
        );
        assert_eq!(
            fs::read_to_string(current.join("candidate.jsonl")).unwrap(),
            "replacement\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn retained_pruning_rejects_hardlinks_without_side_effects() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("candidates");
        fs::create_dir(&dir).unwrap();
        let victim = tmp.path().join("victim.jsonl");
        fs::write(&victim, "sentinel\n").unwrap();
        fs::set_permissions(&victim, fs::Permissions::from_mode(0o644)).unwrap();
        fs::hard_link(&victim, dir.join("candidate.jsonl")).unwrap();
        let retained = crate::secure_fs::RetainedDir::open(&dir).unwrap();

        assert!(prune_jsonl_files_retained(&retained, 0).is_err());
        assert_eq!(fs::read_to_string(&victim).unwrap(), "sentinel\n");
        assert_eq!(fs::metadata(&victim).unwrap().mode() & 0o777, 0o644);
        assert_eq!(fs::metadata(&victim).unwrap().nlink(), 2);
    }
}
