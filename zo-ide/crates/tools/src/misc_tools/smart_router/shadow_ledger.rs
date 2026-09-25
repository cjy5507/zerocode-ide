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

/// Whether a process that has not armed durable traces may write `path`.
///
/// The rule the prompt-cache and request-timing ledgers already keep
/// (`runtime::durable_traces_armed`, 2026-09-10), asked here for the Jev
/// seats: evidence under the PERSON's own home is written only by a host that
/// armed it — zo's binary, first thing in `main`. A process nobody armed is a
/// crate test or an embedded host that never asked, and cargo runs a unit test
/// from the package root, so its rows land under a project slug that reads
/// exactly like a session's (`…-zo-ide-crates-tools-…`). Forty-eight `no_key`
/// routing rows sat beside fifty-two a real session had written, and every
/// number the Jev dashboard shows was counted on the sum (t-5785, t-5805).
///
/// A path OUTSIDE that home is a path somebody chose on purpose — a test's
/// scratch directory, a harness's pinned `ZO_STATE_DIR` or `ZO_CONFIG_HOME` —
/// and is written for whoever asked. The person's home is asked for by the
/// name it has, never through `default_config_home`, because that one reads
/// the override and would refuse the isolation it exists to allow.
fn may_write(armed: bool, person_home: Option<&Path>, path: &Path) -> bool {
    armed || person_home.is_none_or(|home| !path.starts_with(home))
}

/// One serialized line, one `O_APPEND` write — the same discipline as the
/// outcome store, for the same reason (concurrent recorders must never
/// zipper a line). Past `max_bytes` the file is cut to its newer half first.
///
/// A write [`may_write`] refuses is not an error: nothing was written because
/// nothing should have been, which is the same answer an empty row set gives.
pub fn append_shadow_row<T: Serialize>(path: &Path, row: &T, max_bytes: u64) -> io::Result<()> {
    if !may_write(
        runtime::durable_traces_armed(),
        runtime::conventional_config_home().as_deref(),
        path,
    ) {
        return Ok(());
    }
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
pub(super) const TAIL_ROW_BYTES: u64 = 2_560;

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

/// Judge `seat` on the ledger it has just written, and write down a rise or a
/// fall — the routing seat's cadence (`decision_shadow::judge_ledger`), read
/// through the table's own judge for a seat whose agreement is the `agreed`
/// marks its own rows carry rather than a probe beside a judgment: the step
/// governor's and the recall seat's. One judge, because two seats that each
/// spelled the cadence would be two cadences.
#[must_use]
pub fn judge_seat_ledger(
    seat: &zerocode_core::jev::JevUse,
    ledger: &Path,
    now_ms: i64,
) -> Option<zerocode_core::jev::promote::Verdict> {
    use zerocode_core::jev::promote;
    let rows: Vec<serde_json::Value> = read_shadow_rows(ledger);
    if !promote::judgment_due(seat, &rows) {
        return None;
    }
    let judged = promote::judge_seat(seat, &rows)?;
    if let Some(row) = promote::transition_row(now_ms, judged.verdict, &judged.window) {
        let _ = append_shadow_row(ledger, &row, SHADOW_LEDGER_MAX_BYTES);
    }
    Some(judged.verdict)
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

    /// 무장하지 않은 판은 사람의 홈에 한 줄도 남기지 않는다 — 제가 고른
    /// 자리에는 그대로 남긴다.
    ///
    /// 판정은 순수 함수라 이 시험이 사람의 진짜 홈을 건드리지 않는다: 홈은
    /// 인자로 들어온다.
    #[test]
    fn an_unarmed_process_writes_nothing_under_the_persons_own_home() {
        let home = std::path::Path::new("/home/dev/.zo");
        let theirs = home.join("projects/app-1/state/smart-router/decision-shadow.jsonl");
        let mine = std::path::Path::new("/tmp/scratch-1/smart-router/decision-shadow.jsonl");

        assert!(!may_write(false, Some(home), &theirs), "unarmed, the person's home");
        assert!(may_write(true, Some(home), &theirs), "a host armed it");
        assert!(may_write(false, Some(home), mine), "unarmed, a directory it chose");
        assert!(may_write(false, None, &theirs), "no home resolves: nothing to protect");
        // 이름이 비슷한 이웃은 그 홈이 아니다.
        assert!(may_write(false, Some(home), std::path::Path::new("/home/dev/.zo-old/x.jsonl")));
    }

    /// 그리고 쓰는 길이 그 판정을 실제로 묻는다: 무장 안 한 이 시험 판이
    /// `HOME` 아래 `.zo` 로 쓰면 파일이 생기지 않는다.
    #[test]
    fn the_write_road_asks_it_and_leaves_no_file() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("HOME");
        std::env::set_var("HOME", dir.path());
        let refused = dir.path().join(".zo").join("projects").join("x").join("row.jsonl");
        let allowed = dir.path().join("scratch").join("row.jsonl");
        let wrote = |path: &Path| {
            append_shadow_row(path, &serde_json::json!({"n": 1}), u64::MAX).expect("no error");
            path.exists()
        };
        let (left, right) = (wrote(&refused), wrote(&allowed));
        match previous {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        assert!(!left, "an unarmed process wrote a row into the person's own home");
        assert!(right, "a path the process chose is still written");
    }

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
