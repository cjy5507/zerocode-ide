//! Shared JSONL log-file maintenance for the rotating per-turn trace and the
//! dreamer / self-improve candidate logs.
//!
//! Both writers keep append-only `.jsonl` histories that need the same two
//! operations: line-count pruning (retain only the newest N records, rewritten
//! atomically through a temp file) and mtime-ordered file discovery (for
//! rotating whole files). These were re-implemented identically in `turn_trace`
//! and `memory::dreamer`; this module is the single source of truth.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The file beside a directory's logs that keeps where the last pass stopped
/// reading each one ([`read_jsonl_resumed`]).
pub(crate) const READ_CURSOR_FILE: &str = ".read-cursor.json";
/// A cursor larger than this reads as missing: that pass reads every log in full.
const MAX_READ_CURSOR_BYTES: u64 = 64 * 1024 * 1024;
/// How many bytes before a log's read offset its fingerprint covers.
const EDGE_BYTES: u64 = 64;

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
#[cfg(not(unix))]
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

/// Where reading each of one directory's logs stopped.
#[derive(Serialize, Deserialize)]
struct ReadCursor<T> {
    files: Vec<FileCursor<T>>,
}

#[derive(Serialize, Deserialize)]
struct FileCursor<T> {
    name: String,
    dev: u64,
    ino: u64,
    /// Just past the last complete line read.
    offset: u64,
    /// FNV-1a of the [`EDGE_BYTES`] before `offset`: a log rewritten where it
    /// lies no longer matches and is read again from its start.
    edge: u64,
    /// Whether items older than `items` were let go to keep the window at its cap.
    trimmed: bool,
    /// The newest items the lines before `offset` gave that the window still holds.
    items: VecDeque<T>,
}

/// What `parse` finds in the lines of `dir`'s newest `keep_files` logs: the
/// newest `cap` items, oldest first — what reading every log in full gives —
/// reading each log only past where the previous call stopped.
///
/// Where each log was left, and those of its items the window still holds
/// (at most `cap` over every log), is kept in [`READ_CURSOR_FILE`] beside the
/// logs. A log that was replaced (another inode), shortened, or changed just
/// before that point is read again from its start, and so is a log whose
/// let-go items are back inside the window because it was appended to and
/// moved up the order. An unfinished last line counts this time and is read
/// again once finished. A missing or unreadable cursor costs one full read;
/// one that cannot be written costs the next pass the same.
pub(crate) fn read_jsonl_resumed<T: Serialize + DeserializeOwned>(
    dir: &Path,
    keep_files: usize,
    cap: usize,
    mut parse: impl FnMut(&str) -> Option<T>,
) -> Vec<T> {
    let mut earlier: HashMap<String, FileCursor<T>> = load_read_cursor(dir)
        .into_iter()
        .map(|file| (file.name.clone(), file))
        .collect();
    let mut cursor = ReadCursor { files: Vec::new() };
    let mut paths = Vec::new();
    let mut tails = Vec::new();
    let mut changed = false;
    let mut held = 0;
    for path in jsonl_files_oldest_first(dir, keep_files) {
        let Some(name) = path.file_name().and_then(std::ffi::OsStr::to_str) else {
            continue;
        };
        let before = earlier.remove(name);
        let Ok((file, tail, moved)) = read_file_resumed(&path, name, before, cap, &mut parse) else {
            changed = true;
            continue;
        };
        changed |= moved;
        held += file.items.len();
        cursor.files.push(file);
        paths.push(path);
        tails.push(tail);
        // One window over every log, as a full read keeps: the oldest go first.
        for file in &mut cursor.files {
            while held > cap && file.items.pop_front().is_some() {
                file.trimmed = true;
                held -= 1;
                changed = true;
            }
        }
    }
    let mut need = cap;
    for ((file, tail), path) in cursor.files.iter_mut().zip(&mut tails).zip(&paths).rev() {
        if file.trimmed && need > file.items.len() + usize::from(tail.is_some()) {
            if let Ok((again, again_tail, _)) = read_file_resumed(path, &file.name, None, cap, &mut parse) {
                *file = again;
                *tail = again_tail;
                changed = true;
            }
        }
        let take = need.min(file.items.len() + usize::from(tail.is_some()));
        if take == 0 {
            *tail = None;
        }
        while file.items.len() + usize::from(tail.is_some()) > take {
            file.items.pop_front();
            file.trimmed = true;
            changed = true;
        }
        need -= take;
    }
    if changed || !earlier.is_empty() {
        save_read_cursor(dir, &cursor);
    }
    cursor
        .files
        .into_iter()
        .zip(tails)
        .flat_map(|(file, tail)| file.items.into_iter().chain(tail))
        .collect()
}

/// Read one log past where `before` stopped, or from its start when `before`
/// no longer describes it. Returns its cursor, what an unfinished last line
/// gave, and whether the cursor moved.
fn read_file_resumed<T>(
    path: &Path,
    name: &str,
    before: Option<FileCursor<T>>,
    cap: usize,
    parse: &mut impl FnMut(&str) -> Option<T>,
) -> std::io::Result<(FileCursor<T>, Option<T>, bool)> {
    let mut file = open_jsonl_no_symlink(path)?;
    let metadata = file.metadata()?;
    let (dev, ino) = file_identity(&metadata);
    let mut resumed = None;
    if let Some(before) = before {
        if before.dev == dev
            && before.ino == ino
            && before.offset <= metadata.len()
            && edge_before(&mut file, before.offset)? == before.edge
        {
            resumed = Some(before);
        }
    }
    let restarted = resumed.is_none();
    let mut cursor = resumed.unwrap_or_else(|| FileCursor {
        name: name.to_string(),
        dev,
        ino,
        offset: 0,
        edge: fnv1a(&[]),
        trimmed: false,
        items: VecDeque::new(),
    });
    let start = cursor.offset;
    file.seek(SeekFrom::Start(start))?;
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut tail = None;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        let item = std::str::from_utf8(&line)
            .ok()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .and_then(&mut *parse);
        if line.last() != Some(&b'\n') {
            tail = item;
            break;
        }
        cursor.offset = cursor.offset.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if let Some(item) = item {
            cursor.items.push_back(item);
            if cursor.items.len() > cap {
                cursor.items.pop_front();
                cursor.trimmed = true;
            }
        }
    }
    let moved = restarted || cursor.offset != start;
    if moved {
        cursor.edge = edge_before(reader.get_mut(), cursor.offset)?;
    }
    Ok((cursor, tail, moved))
}

fn edge_before(file: &mut File, offset: u64) -> std::io::Result<u64> {
    let start = offset.saturating_sub(EDGE_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    Read::by_ref(file).take(offset - start).read_to_end(&mut bytes)?;
    Ok(fnv1a(&bytes))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt as _;
    (metadata.dev(), metadata.ino())
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn load_read_cursor<T: DeserializeOwned>(dir: &Path) -> Vec<FileCursor<T>> {
    crate::secure_fs::read_regular_file_absolute_no_follow_bounded(
        &dir.join(READ_CURSOR_FILE),
        MAX_READ_CURSOR_BYTES,
    )
    .ok()
    .flatten()
    .and_then(|text| serde_json::from_str::<ReadCursor<T>>(&text).ok())
    .map_or_else(Vec::new, |cursor| cursor.files)
}

fn save_read_cursor<T: Serialize>(dir: &Path, cursor: &ReadCursor<T>) {
    if let Ok(bytes) = serde_json::to_vec(cursor) {
        let _ = crate::secure_fs::write_atomic_owner_only_named_temp(
            dir,
            Path::new(READ_CURSOR_FILE),
            &bytes,
            crate::secure_fs::Durability::Published,
        );
    }
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

    // The read cursor's bytes (t-87042). Items here are JSON strings, so a line
    // and the item it gives are about the same size, as a lesson's are.

    /// What a read cursor may take on disk: the pass criterion of t-87042
    /// (4275 m-87009), a mebibyte.
    const CURSOR_LIMIT: u64 = 1024 * 1024;

    /// One JSON-string line of `width` bytes, newline included, that names its
    /// log and its place in it.
    fn string_line(name: &str, line: usize, width: usize) -> String {
        let head = format!("{name}:{line}:");
        let pad = width.saturating_sub(head.len() + 3);
        format!("\"{head}{}\"\n", "x".repeat(pad))
    }

    /// `lines` lines of `width` bytes as `dir`/`name`.
    fn write_log(dir: &Path, name: &str, lines: usize, width: usize) {
        let text: String = (0..lines).map(|line| string_line(name, line, width)).collect();
        fs::write(dir.join(name), text).unwrap();
    }

    /// `lines` more lines of `width` bytes at the end of `dir`/`name`, numbered from `first`.
    fn append_log(dir: &Path, name: &str, first: usize, lines: usize, width: usize) {
        let mut file = fs::OpenOptions::new().append(true).open(dir.join(name)).unwrap();
        for line in first..first + lines {
            file.write_all(string_line(name, line, width).as_bytes()).unwrap();
        }
    }

    fn parse_string(line: &str) -> Option<String> {
        serde_json::from_str(line).ok()
    }

    fn read_window(dir: &Path, cap: usize) -> Vec<String> {
        read_jsonl_resumed(dir, 256, cap, parse_string)
    }

    /// A full read: the pass with no cursor beside the logs. The cursor that
    /// was there stays as it was.
    fn full_window(dir: &Path, cap: usize) -> Vec<String> {
        let path = dir.join(READ_CURSOR_FILE);
        let kept = fs::read(&path).ok();
        let _ = fs::remove_file(&path);
        let items = read_window(dir, cap);
        match kept {
            Some(bytes) => fs::write(&path, bytes).unwrap(),
            None => {
                let _ = fs::remove_file(&path);
            }
        }
        items
    }

    fn cursor_len(dir: &Path) -> u64 {
        fs::metadata(dir.join(READ_CURSOR_FILE)).map_or(0, |metadata| metadata.len())
    }

    /// The cursor an earlier version wrote for the log `name` once it had read
    /// it through: every one of `items`, however long they are.
    fn write_earlier_cursor(dir: &Path, name: &str, items: Vec<String>) {
        let mut file = open_jsonl_no_symlink(&dir.join(name)).unwrap();
        let metadata = file.metadata().unwrap();
        let (dev, ino) = file_identity(&metadata);
        let offset = metadata.len();
        let edge = edge_before(&mut file, offset).unwrap();
        let file = FileCursor { name: name.to_string(), dev, ino, offset, edge, trimmed: false, items: items.into() };
        let bytes = serde_json::to_vec(&ReadCursor { files: vec![file] }).unwrap();
        fs::write(dir.join(READ_CURSOR_FILE), bytes).unwrap();
    }

    #[test]
    fn a_cursor_over_long_lines_stays_within_its_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        // 400 lines of 8 KiB: the window of 10,000 items holds all of them, 3 MiB of items.
        write_log(tmp.path(), "long.jsonl", 400, 8 * 1024);
        assert_eq!(read_window(tmp.path(), 10_000).len(), 400);
        let size = cursor_len(tmp.path());
        assert!(size <= CURSOR_LIMIT, "the cursor took {size} bytes for 400 lines of 8 KiB, over {CURSOR_LIMIT}");
    }

    #[test]
    fn a_cursor_over_the_lesson_logs_of_measurement_d_stays_within_its_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        // t-85282 measurement D: about 12,000 lessons of about 410 bytes over 256
        // logs filled the window of 10,000 items and took 4,051.5 KB of cursor.
        for log in 0..256 {
            write_log(tmp.path(), &format!("observed-{log:03}.jsonl"), 47, 410);
        }
        assert_eq!(read_window(tmp.path(), 10_000).len(), 10_000);
        let size = cursor_len(tmp.path());
        assert!(size <= CURSOR_LIMIT, "the cursor took {size} bytes for the lesson logs of measurement D, over {CURSOR_LIMIT}");
    }

    #[test]
    fn a_cursor_an_earlier_version_wrote_is_read_as_it_was_and_written_again_within_its_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        write_log(tmp.path(), "long.jsonl", 400, 8 * 1024);
        let text = fs::read_to_string(tmp.path().join("long.jsonl")).unwrap();
        let lines: Vec<String> = text.lines().filter_map(parse_string).collect();
        assert_eq!(lines.len(), 400);
        write_earlier_cursor(tmp.path(), "long.jsonl", lines.clone());
        assert!(cursor_len(tmp.path()) > CURSOR_LIMIT, "the cursor of an earlier version holds every item");

        assert_eq!(read_window(tmp.path(), 10_000), lines, "its items are read as they were");
        let size = cursor_len(tmp.path());
        assert!(size <= CURSOR_LIMIT, "the pass left the cursor of an earlier version at {size} bytes, over {CURSOR_LIMIT}");
        assert_eq!(read_window(tmp.path(), 10_000), lines, "and the next pass sees the same");
    }

    #[test]
    fn a_resumed_read_of_long_lines_sees_what_a_full_read_sees() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Five logs of 300 KiB: a cursor can keep only some of them whole.
        for log in 0..5 {
            write_log(dir, &format!("log-{log}.jsonl"), 60, 5 * 1024);
        }
        assert_eq!(read_window(dir, 10_000), full_window(dir, 10_000));
        for step in 0..10 {
            append_log(dir, &format!("log-{}.jsonl", step % 5), 60 + step, 2, 5 * 1024);
            let resumed = read_window(dir, 10_000);
            assert_eq!(resumed, full_window(dir, 10_000), "step {step}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_pass_that_changes_nothing_leaves_the_cursor_alone_even_over_long_lines() {
        use std::os::unix::fs::MetadataExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_log(dir, "long.jsonl", 400, 8 * 1024);
        write_log(dir, "short.jsonl", 3, 100);
        let first = read_window(dir, 10_000);
        let cursor = dir.join(READ_CURSOR_FILE);
        let (inode, modified, bytes) = (
            fs::metadata(&cursor).unwrap().ino(),
            fs::metadata(&cursor).unwrap().modified().unwrap(),
            fs::read(&cursor).unwrap(),
        );

        assert_eq!(read_window(dir, 10_000), first);
        let after = fs::metadata(&cursor).unwrap();
        assert_eq!((after.ino(), after.modified().unwrap()), (inode, modified), "a pass that changes nothing writes nothing");
        assert_eq!(fs::read(&cursor).unwrap(), bytes);
    }

    #[test]
    fn a_cursor_that_cannot_be_read_reads_as_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_log(dir, "a.jsonl", 5, 200);
        write_log(dir, "b.jsonl", 5, 200);
        assert_eq!(read_window(dir, 100).len(), 10);
        let cursor = dir.join(READ_CURSOR_FILE);
        let whole = fs::read(&cursor).unwrap();

        let broken: [&[u8]; 4] = [&whole[..whole.len() / 2], br#"{"files":[{"name":1}]}"#, b"", b"not json"];
        for (round, broken) in broken.into_iter().enumerate() {
            fs::write(&cursor, broken).unwrap();
            append_log(dir, "a.jsonl", 5 + round, 1, 200);
            let resumed = read_window(dir, 100);
            assert_eq!(resumed, full_window(dir, 100), "round {round}");
            assert_eq!(resumed.len(), 11 + round, "round {round}");
            let mended = fs::read(&cursor).unwrap();
            assert!(serde_json::from_slice::<ReadCursor<String>>(&mended).is_ok(), "round {round}: the pass wrote a cursor that reads");
        }
    }
}
