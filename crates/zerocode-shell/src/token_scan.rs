//! Walking a vendor's transcript tree, and reading it a line at a time.
//!
//! Two vendors keep their records in the same shape of place — a directory of
//! newline-delimited JSON under the person's home — and count entirely
//! differently once a line is in hand. What they share is the walking and the
//! reading, which is what lives here; what they do not share is what a line
//! MEANS, which stays in `claude_tokens` and `codex_tokens`.
//!
//! The bounds are the point of having this at all. A scan walks whatever the
//! vendor laid down, and a corpus is large: 915 MB for one on this machine and
//! 4.4 GB for the other, with a single 504 MB file in it. Unbounded recursion
//! turns a symlink into a hang, and reading a file into a `String` first puts
//! the corpus in memory to count it.

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

/// How deep a walk goes below the root it was given.
///
/// The measured maximum is 5 for Claude's layout and 4 for Codex's. Eight
/// leaves room for a layout the vendors have not shipped yet without letting a
/// loop run forever.
pub(crate) const MAX_DEPTH: u8 = 8;

/// The longest line either reader will hold.
///
/// The largest record measured across both corpora is 1,747,798 bytes — one
/// turn that pasted a file in. Four mebibytes is twice that headroom, and the
/// point of the cap is that a single pathological line cannot make a scan
/// allocate without bound.
pub(crate) const MAX_RECORD_BYTES: u64 = 4 * 1024 * 1024;

/// Every file under `root` whose name ends in `suffix`, to [`MAX_DEPTH`].
///
/// Sorted, because two scans of the same tree should answer in the same order:
/// a rollup keyed by day does not care, but a reader comparing two runs does,
/// and `read_dir` promises nothing.
pub(crate) fn files_under(root: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, suffix, MAX_DEPTH, &mut found);
    found.sort();
    found
}

fn walk(root: &Path, suffix: &str, depth: u8, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth > 0 {
                walk(&path, suffix, depth - 1, found);
            }
        } else if path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.ends_with(suffix))
        {
            found.push(path);
        }
    }
}

/// Hand `read` every complete line of `path`, in order.
///
/// One buffer for the whole file, capped per line: a line at the cap is
/// skipped rather than truncated, because half a JSON record is not a record.
/// A file that will not open is simply not read — a scan of somebody else's
/// directory has no business failing over one unreadable entry.
pub(crate) fn for_each_line(path: &Path, mut read: impl FnMut(&str)) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    loop {
        line.clear();
        let got = match (&mut reader)
            .take(MAX_RECORD_BYTES)
            .read_until(b'\n', &mut line)
        {
            Ok(0) => break,
            Ok(got) => got,
            Err(_) => break,
        };
        if u64::try_from(got).unwrap_or(u64::MAX) >= MAX_RECORD_BYTES {
            continue;
        }
        if let Ok(text) = std::str::from_utf8(&line) {
            read(text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The walk finds what it should, stops where it should, and answers in
    /// the same order twice.
    #[test]
    fn the_walk_is_bounded_suffixed_and_ordered() {
        let root = tempfile::tempdir().expect("tempdir");
        let deep = root.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&deep).expect("mkdir");
        std::fs::write(root.path().join("top.jsonl"), "").expect("write");
        std::fs::write(deep.join("rollout-1.jsonl"), "").expect("write");
        std::fs::write(deep.join("notes.txt"), "").expect("write");
        std::fs::write(deep.join("rollout-2.jsonl"), "").expect("write");

        let held = files_under(root.path(), ".jsonl");
        assert_eq!(held.len(), 3, "the walk missed or invented a file");
        assert!(
            held.iter()
                .all(|path| path.extension().is_some_and(|held| held == "jsonl"))
        );
        assert_eq!(
            held,
            files_under(root.path(), ".jsonl"),
            "two walks disagreed"
        );

        // A narrower suffix is a narrower answer.
        assert_eq!(files_under(root.path(), "rollout-2.jsonl").len(), 1);
        // A root that is not there is no files, not a failure.
        assert!(files_under(&root.path().join("nowhere"), ".jsonl").is_empty());
    }

    /// Complete lines only, and one absurd line cannot take the file down.
    #[test]
    fn a_line_past_the_cap_is_skipped_and_the_rest_still_read() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = root.path().join("one.jsonl");
        let huge = "x".repeat(usize::try_from(MAX_RECORD_BYTES).unwrap_or(0) + 16);
        std::fs::write(&path, format!("first\n{huge}\nlast\n")).expect("write");

        let mut seen = Vec::new();
        for_each_line(&path, |line| seen.push(line.trim_end().to_string()));
        assert!(
            seen.contains(&"first".to_string()),
            "the first line was lost"
        );
        assert!(
            seen.contains(&"last".to_string()),
            "the reader stopped at the huge line"
        );
        assert!(
            !seen.iter().any(|line| line.len() > 1000),
            "a line past the cap was handed over anyway"
        );

        // A file that is not there is quiet.
        let mut none = 0;
        for_each_line(&root.path().join("missing.jsonl"), |_| none += 1);
        assert_eq!(none, 0);
    }
}
