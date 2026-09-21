//! The append-only JSONL discipline every smart-router shadow ledger keeps.
//!
//! A shadow ledger is evidence a later phase reads before a shadow may decide
//! anything: the plan scorer's (`plan_shadow.rs`) and the typed routing
//! judgment's (`decision_shadow.rs`). Both write one row per line with one
//! `O_APPEND` write and stay bounded the same way, so that discipline lives
//! here once instead of once per ledger.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

/// Past this size a shadow ledger keeps only its newer half. A soak is
/// evidence, not an archive; the rows that matter are the recent ones.
pub const SHADOW_LEDGER_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Where a project's shadow ledgers live — the directory a reader that holds
/// several of them keeps, so a ledger's name is the only thing it has to know.
///
/// The runtime's, because the runtime reads these ledgers too: the system
/// prompt asks a Jev seat whether it stands before deciding what it says
/// about skills, and a folder name spelled twice is a reader looking in a
/// place nothing writes to.
#[must_use]
pub fn shadow_ledger_dir(cwd: &Path) -> PathBuf {
    runtime::jev_ledger_dir(cwd)
}

/// Where a project's shadow ledger `file` lives.
#[must_use]
pub fn shadow_ledger_path(cwd: &Path, file: &str) -> PathBuf {
    shadow_ledger_dir(cwd).join(file)
}

/// One serialized line, one `O_APPEND` write — the same discipline as the
/// outcome store, for the same reason (concurrent recorders must never
/// zipper a line). Past `max_bytes` the file is cut to its newer half first.
pub fn append_shadow_row<T: Serialize>(path: &Path, row: &T, max_bytes: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::metadata(path).is_ok_and(|meta| meta.len() > max_bytes) {
        keep_newer_half(path)?;
    }
    let mut line = serde_json::to_string(row).map_err(io::Error::other)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())
}

/// Every row of a shadow ledger that parses, oldest first. A missing ledger is
/// no rows; a line that does not parse — a torn write, a row from another
/// schema — is skipped rather than fatal, since one bad line must not blind a
/// reader to the rest.
#[must_use]
pub fn read_shadow_rows<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    fs::read_to_string(path)
        .map(|text| text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect())
        .unwrap_or_default()
}

/// How much of a ledger's end is read for its last row. A row is fingerprints,
/// tokens and at most a recall's dozen note names with their readings — a few
/// kilobytes — so this holds the last row with room to spare, and a row that
/// somehow outgrew it reads as a torn line rather than as a wrong one.
const LAST_ROW_WINDOW_BYTES: u64 = 64 * 1024;

/// Bytes a tail of several rows allows for each of them.
///
/// Measured on this machine 2026-09-18: the routing ledger's rows run to
/// 700 B (p90 685) and the recall ledger's — which name a dozen notes and
/// their readings — to 2,186 B (p90 1,775). 2.5 KiB holds the longest of
/// either with room, so a window of this times the rows asked for holds them
/// all and the reader never has to guess whether it saw the last of them.
const TAIL_ROW_BYTES: u64 = 2_560;

/// A ledger's last row, read from its end rather than the whole file: `None`
/// for a missing or empty ledger.
#[must_use]
pub fn last_shadow_line(path: &Path) -> Option<String> {
    last_shadow_lines(path, 1).pop()
}

/// A ledger's last `rows` rows, oldest first, read from its end rather than
/// the whole file — a shadow ledger runs to megabytes and a reader of its
/// recent past must not pay for its whole history.
///
/// A window that begins inside the file may begin inside a row, and half a
/// row is not one, so the first line of such a window is dropped. Every line
/// that comes back is a whole line as it was written; whether it parses is
/// the caller's question.
#[must_use]
pub fn last_shadow_lines(path: &Path, rows: usize) -> Vec<String> {
    let nothing = Vec::new();
    let Ok(mut file) = fs::File::open(path) else {
        return nothing;
    };
    let Ok(length) = file.metadata().map(|meta| meta.len()) else {
        return nothing;
    };
    let window = LAST_ROW_WINDOW_BYTES.max(TAIL_ROW_BYTES.saturating_mul(u64::try_from(rows).unwrap_or(u64::MAX)));
    let from = length.saturating_sub(window);
    if file.seek(SeekFrom::Start(from)).is_err() {
        return nothing;
    }
    let mut tail = Vec::new();
    if file.read_to_end(&mut tail).is_err() {
        return nothing;
    }
    let text = String::from_utf8_lossy(&tail);
    let whole: Vec<&str> = text
        .lines()
        .skip(usize::from(from > 0))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    whole[whole.len().saturating_sub(rows)..].iter().map(|line| (*line).to_string()).collect()
}

/// Drop the older half of a ledger's lines, atomically (write beside, rename).
fn keep_newer_half(path: &Path) -> io::Result<()> {
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();
    let keep = &lines[lines.len() / 2..];
    let mut newer = keep.join("\n");
    if !newer.is_empty() {
        newer.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    fs::write(&tmp, newer)?;
    fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_row_is_read_from_the_end() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        assert_eq!(last_shadow_line(&path), None);
        append_shadow_row(&path, &serde_json::json!({"n": 1}), u64::MAX).unwrap();
        append_shadow_row(&path, &serde_json::json!({"n": 2}), u64::MAX).unwrap();
        assert_eq!(last_shadow_line(&path).as_deref(), Some(r#"{"n":2}"#));
    }

    #[test]
    fn a_tail_reads_the_newest_whole_rows_and_no_more_than_it_was_asked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        assert!(last_shadow_lines(&path, 4).is_empty(), "a missing ledger is no rows");
        for n in 1..=6 {
            append_shadow_row(&path, &serde_json::json!({"n": n}), u64::MAX).unwrap();
        }
        assert_eq!(
            last_shadow_lines(&path, 3),
            vec![r#"{"n":4}"#, r#"{"n":5}"#, r#"{"n":6}"#],
            "the newest three, oldest first"
        );
        assert_eq!(last_shadow_lines(&path, 99).len(), 6, "a ledger shorter than the window is all of it");
    }

    /// Past the window the read starts inside the file, and may start inside a
    /// row: what comes back is whole rows or nothing.
    #[test]
    fn a_window_that_begins_mid_row_hands_back_no_half_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.jsonl");
        let padding = "x".repeat(1_024);
        for n in 1..=200 {
            append_shadow_row(&path, &serde_json::json!({"n": n, "pad": padding}), u64::MAX).unwrap();
        }
        assert!(
            fs::metadata(&path).unwrap().len() > LAST_ROW_WINDOW_BYTES,
            "the ledger outgrew one window, which is the case under test"
        );
        let tail = last_shadow_lines(&path, 3);
        assert_eq!(tail.len(), 3);
        let read: Vec<u64> = tail
            .iter()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("a whole row")["n"].as_u64().unwrap())
            .collect();
        assert_eq!(read, vec![198, 199, 200]);
    }

    #[test]
    fn a_reader_skips_what_does_not_parse_and_a_missing_ledger_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("ledger.jsonl");
        assert!(read_shadow_rows::<serde_json::Value>(&path).is_empty());
        append_shadow_row(&path, &serde_json::json!({"n": 1}), u64::MAX).unwrap();
        fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{\"n\": \n").unwrap();
        append_shadow_row(&path, &serde_json::json!({"n": 2}), u64::MAX).unwrap();
        let rows: Vec<serde_json::Value> = read_shadow_rows(&path);
        assert_eq!(rows, vec![serde_json::json!({"n": 1}), serde_json::json!({"n": 2})]);
    }
}
