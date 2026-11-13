//! Jev's JSONL reader, shared by the window and the CLI — and the one bound
//! an appended ledger keeps ([`keep_within`]).
//!
//! The parsed rows are needed for version joins and historical comparisons;
//! a second allocation holding the entire file is not. Reuse one line buffer
//! and preserve file order, including a complete last row without a newline.

use std::fs::{self, File};
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

/// Amortize reads without retaining the whole serialized history.
pub const READ_BUFFER_BYTES: usize = 64 * 1024;

/// Read every valid JSON row without holding a copy of the whole file.
///
/// Malformed lines, including a torn final write, are skipped independently.
/// An I/O error rejects the read rather than presenting a partial history as
/// the complete ledger.
///
/// # Errors
/// Returns an error when the file cannot be opened or read.
pub fn read(path: &Path) -> io::Result<Vec<Value>> {
    read_from(BufReader::with_capacity(
        READ_BUFFER_BYTES,
        File::open(path)?,
    ))
}

/// Read rows from a buffered stream using one reusable scratch buffer.
///
/// # Errors
/// Returns the first I/O error; no partial result escapes.
pub fn read_from(mut reader: impl BufRead) -> io::Result<Vec<Value>> {
    let mut rows = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line)? == 0 {
            return Ok(rows);
        }
        super::promote::note_line_parsed();
        if let Ok(row) = serde_json::from_slice(&line) {
            rows.push(row);
        }
    }
}

/// Past this size a ledger kept bounded keeps only its newer half. A ledger
/// is evidence, not an archive; the rows that matter are the recent ones.
/// zo's shadow ledgers and the window's rows of a person's decisions keep
/// this one bound, so a ledger both write is cut the same whoever writes.
pub const LEDGER_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Cut the ledger at `path` to its newer half, by lines, when it is longer
/// than `max_bytes`; a ledger within it, or not there, is left alone. The
/// writer holds the ledger's lock: the cut is a read, a write beside it and a
/// rename over it.
///
/// # Errors
/// The ledger is longer than `max_bytes` and could not be read, or its newer
/// half could not be written or renamed over it.
pub fn keep_within(path: &Path, max_bytes: u64) -> io::Result<()> {
    if !fs::metadata(path).is_ok_and(|meta| meta.len() > max_bytes) {
        return Ok(());
    }
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
    use serde_json::json;
    use std::io::{Cursor, Read};

    #[test]
    fn malformed_lines_do_not_hide_complete_rows_or_change_their_order() {
        let input = b"{\"at\":1}\r\n\nnot json\n{\"at\":2}\n{\"at\":3}";
        let rows = read_from(BufReader::with_capacity(3, &input[..])).unwrap();
        assert_eq!(
            rows,
            vec![json!({"at":1}), json!({"at":2}), json!({"at":3})]
        );
    }

    #[test]
    fn a_torn_or_non_utf8_line_does_not_hide_other_rows() {
        let input = b"{\"at\":1}\n\xff\xfe\n{\"at\":2}\n{\"at\":";
        assert_eq!(
            read_from(Cursor::new(input)).unwrap(),
            vec![json!({"at":1}), json!({"at":2})]
        );
    }

    #[test]
    fn a_row_larger_than_the_read_buffer_keeps_its_unicode_text() {
        let row = json!({"text": "한글 🦀".repeat(8_000)});
        let input = serde_json::to_vec(&row).unwrap();
        let rows = read_from(BufReader::with_capacity(17, Cursor::new(input))).unwrap();
        assert_eq!(rows, vec![row]);
    }

    #[test]
    fn an_io_failure_does_not_return_a_partial_history() {
        struct FailingRead(Cursor<&'static [u8]>);
        impl Read for FailingRead {
            fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
                let read = self.0.read(target)?;
                if read == 0 {
                    Err(io::Error::other("failed after one complete row"))
                } else {
                    Ok(read)
                }
            }
        }
        let reader = BufReader::new(FailingRead(Cursor::new(b"{\"at\":1}\n")));
        assert!(read_from(reader).is_err());
    }

    /// A ledger past its bound keeps its newer half, in order; one within it
    /// and one not there are left as they are.
    #[test]
    fn a_ledger_past_its_bound_keeps_its_newer_half() {
        let folder = tempfile::tempdir().unwrap();
        let ledger = folder.path().join("rows.jsonl");
        keep_within(&ledger, 0).unwrap();
        assert!(!ledger.exists());
        let rows: String = (0..10).map(|at| format!("{{\"at\":{at}}}\n")).collect();
        fs::write(&ledger, &rows).unwrap();
        keep_within(&ledger, rows.len() as u64).unwrap();
        assert_eq!(fs::read_to_string(&ledger).unwrap(), rows);
        keep_within(&ledger, rows.len() as u64 - 1).unwrap();
        let kept: Vec<Value> = read(&ledger).unwrap();
        assert_eq!(
            kept,
            (5..10).map(|at| json!({ "at": at })).collect::<Vec<_>>()
        );
    }
}
