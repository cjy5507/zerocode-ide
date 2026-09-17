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

/// Where shadow ledgers live under a project's state dir, next to the outcome
/// store they must never be confused with.
pub const SHADOW_LEDGER_DIR: &str = "smart-router";

/// Past this size a shadow ledger keeps only its newer half. A soak is
/// evidence, not an archive; the rows that matter are the recent ones.
pub const SHADOW_LEDGER_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Where a project's shadow ledger `file` lives.
#[must_use]
pub fn shadow_ledger_path(cwd: &Path, file: &str) -> PathBuf {
    runtime::zo_project_state_dir(cwd).join(SHADOW_LEDGER_DIR).join(file)
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

/// A ledger's last row, read from its end rather than the whole file: `None`
/// for a missing or empty ledger.
#[must_use]
pub fn last_shadow_line(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(LAST_ROW_WINDOW_BYTES))).ok()?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).ok()?;
    String::from_utf8_lossy(&tail).lines().rev().map(str::trim).find(|line| !line.is_empty()).map(str::to_string)
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
