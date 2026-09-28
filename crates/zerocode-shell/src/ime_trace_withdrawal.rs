//! The Korean-input husk's old dumps, withdrawn once (t-11740).
//!
//! Until this build the window's Korean-input diagnostic (`reportStrayJamo`
//! in `ui/shell-input.js`) wrote its trace ring to `window-errors.log` with
//! every hangul syllable the person typed kept: the log read as their
//! sentences, and it is a file agents read to diagnose the window. The
//! window now writes only the typing's shape. What earlier builds wrote is
//! still on disk, in the log and in its rotated `.1`, so the first boot of a
//! build that knows better withdraws it.
//!
//! Each old dump keeps its header's road — how often a half letter leaked,
//! and through which door, is still worth counting — and loses the rest: the
//! leaked letter and every trace line below it. Every other line is left
//! byte for byte. A mark in the window's data folder says it was done, so
//! later boots pay one `stat`.

use std::path::Path;

use crate::durable_file;
use crate::system_runtime::{WINDOW_LOG, WINDOW_LOG_ROTATED, note_window_event};

/// How a dump's header opens as the log holds it: `note_window_event` puts
/// `window: ` ahead of the words `reportStrayJamo` sends.
const DUMP_OPENS: &str = "window: ime: bare jamo left through ";

/// The mark a finished withdrawal leaves beside the log.
const WITHDRAWN_MARK: &str = "window-errors.ime-withdrawn";

/// The log and the one rotation `note_window_event` keeps.
const LOGS: [&str; 2] = [WINDOW_LOG, WINDOW_LOG_ROTATED];

/// Withdraw every old dump from the window's log, once per data folder.
///
/// Runs at boot before anything else writes the log, so the rewrite cannot
/// race an append. Nothing here can stop the boot: a file that cannot be
/// read or replaced is said on stderr, and the mark is left unwritten so the
/// next boot tries again.
pub(crate) fn withdraw_once(local_data_root: &Path) {
    let mark = local_data_root.join(WITHDRAWN_MARK);
    if mark.exists() {
        return;
    }
    let mut dumps = 0;
    for name in LOGS {
        match withdraw_from(&local_data_root.join(name)) {
            Ok(count) => dumps += count,
            Err(error) => {
                eprintln!(
                    "zerocode-shell: the old Korean-input traces in {name} could not be withdrawn: {error}"
                );
                return;
            }
        }
    }
    if dumps > 0 {
        note_window_event(
            local_data_root,
            &format!("withdrew {dumps} Korean-input traces that carried typed text"),
        );
    }
    if let Err(error) = durable_file::replace_bytes(&mark, b"") {
        eprintln!("zerocode-shell: the Korean-input withdrawal could not be marked done: {error}");
    }
}

/// One file's old dumps withdrawn, replaced whole or not at all.
fn withdraw_from(path: &Path) -> std::io::Result<usize> {
    let log = match std::fs::read(path) {
        Ok(log) => log,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let Some((clean, dumps)) = withdrawn(&log) else {
        return Ok(0);
    };
    durable_file::replace_bytes(path, &clean)?;
    Ok(dumps)
}

/// The log with every old dump withdrawn, and how many there were — `None`
/// when there was nothing to withdraw.
///
/// A dump is ONE write: `note_window_event` writes the header and the trace
/// beneath it with a single `write_all`, so nothing can land between them.
/// The trace lines carry the ring's own stamps, all taken before the header
/// was written; the next write's stamp is the window's clock after it. So a
/// dump runs from its header to the first line stamped later than the
/// header, and a line with no stamp at all — a trace line a pasted newline
/// broke in two, before payloads were scrubbed — is still inside it. A
/// write stamped in the very millisecond of the header is taken with the
/// dump: losing one such line is the price of never leaving typed text.
fn withdrawn(log: &[u8]) -> Option<(Vec<u8>, usize)> {
    let mut clean = Vec::with_capacity(log.len());
    let mut dumps = 0;
    let mut lines = log.split_inclusive(|byte| *byte == b'\n').peekable();
    while let Some(line) = lines.next() {
        let Some((stamp, road)) = old_dump(line) else {
            clean.extend_from_slice(line);
            continue;
        };
        dumps += 1;
        let mut trace = 0;
        while lines
            .next_if(|next| stamp_of(next).is_none_or(|at| at <= stamp))
            .is_some()
        {
            trace += 1;
        }
        clean.extend_from_slice(
            format!(
                "{stamp} {DUMP_OPENS}{road} — withdrawn with its {trace}-line trace: it carried typed text\n"
            )
            .as_bytes(),
        );
    }
    (dumps > 0).then_some((clean, dumps))
}

/// A dump header an earlier build wrote: its stamp and road. Such a header
/// names the leaked letter itself, so hangul after the opening words is
/// what tells it from a dump written in shapes.
fn old_dump(line: &[u8]) -> Option<(u64, &str)> {
    let stamp = stamp_of(line)?;
    let text = std::str::from_utf8(line).ok()?;
    let (_, rest) = text.split_once(' ')?;
    let told = rest.strip_prefix(DUMP_OPENS)?;
    if !told.chars().any(is_hangul) {
        return None;
    }
    let road = told.split(':').next().unwrap_or_default();
    let road = if road
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.')
    {
        road
    } else {
        ""
    };
    Some((stamp, road))
}

/// The epoch-millisecond stamp every write to the log opens with.
fn stamp_of(line: &[u8]) -> Option<u64> {
    let digits = line.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 || line.get(digits) != Some(&b' ') {
        return None;
    }
    std::str::from_utf8(&line[..digits]).ok()?.parse().ok()
}

/// Hangul as the window's husk hears it: a compatibility jamo or a finished
/// syllable (`HANGUL_ANY` in `ui/shell-input.js`).
fn is_hangul(ch: char) -> bool {
    ('\u{3130}'..='\u{318f}').contains(&ch) || ('\u{ac00}'..='\u{d7a3}').contains(&ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dump as an earlier build wrote it: the header's stamp is the
    /// window's write, the trace lines are the ring's own, older, stamps.
    /// Invented typing — nobody's words.
    const OLD_DUMP: &str = "\
1700000005000 window: ime: bare jamo left through drain: ㄴ
1700000004100 comp.start word=
1700000004101 comp.update 오늘
1700000004103 key.ime ㄹ
1700000004500 drain sink=·· out=날씨
1700000004990 half.hold ㄴ
";

    fn log_with(dump: &str) -> String {
        format!(
            "1700000001000 window: tabstrip: bare press 3,4 beside button — kept from drag\n\
             {dump}\
             1700000009000 reaped 1 stale Codex app-server sidecars\n\
             1700000009500 panic: invented\n   0: zerocode_shell::system_runtime::record_panic\n"
        )
    }

    fn hangul_in(text: &str) -> bool {
        text.chars().any(is_hangul)
    }

    #[test]
    fn an_old_dump_keeps_its_road_and_loses_its_letters_and_nothing_else_moves() {
        let log = log_with(OLD_DUMP);
        let (after, dumps) = withdrawn(log.as_bytes()).expect("the old dump is withdrawn");
        let after = String::from_utf8(after).unwrap();
        assert_eq!(dumps, 1);
        assert!(!hangul_in(&after), "typed text survived:\n{after}");
        assert_eq!(
            after,
            log_with(
                "1700000005000 window: ime: bare jamo left through drain \
                 — withdrawn with its 5-line trace: it carried typed text\n"
            ),
        );
    }

    #[test]
    fn a_trace_line_the_typing_broke_in_two_goes_with_its_dump() {
        // Before the payload was scrubbed a pasted newline split a trace
        // line; the piece has no stamp and still belongs to the dump.
        let dump = "\
1700000005000 window: ime: bare jamo left through escort: ㅋ
1700000004100 drain sink=first half
second half of a paste
1700000004990 half.escort out=ㅋ
";
        let (after, dumps) = withdrawn(log_with(dump).as_bytes()).unwrap();
        let after = String::from_utf8(after).unwrap();
        assert_eq!(dumps, 1);
        assert!(!after.contains("half of a paste"), "{after}");
        assert!(
            after.contains("through escort — withdrawn with its 3-line trace"),
            "{after}"
        );
        assert!(after.contains("   0: zerocode_shell::system_runtime::record_panic\n"));
    }

    #[test]
    fn a_shaped_dump_and_a_log_without_dumps_are_left_alone() {
        let shaped = "\
1700000005000 window: ime: bare jamo left through drain: c (c=consonant v=vowel s=open S=closed ·=other)
1700000004101 comp.update sS
";
        assert!(withdrawn(log_with(shaped).as_bytes()).is_none());
        assert!(withdrawn(log_with("").as_bytes()).is_none());
        assert!(withdrawn(b"").is_none());
    }

    #[test]
    fn two_files_are_rewritten_once_and_the_mark_stops_the_next_boot() {
        let root = tempfile::tempdir().unwrap();
        let log = root.path().join(WINDOW_LOG);
        let rotated = root.path().join(WINDOW_LOG_ROTATED);
        std::fs::write(&log, log_with(OLD_DUMP)).unwrap();
        std::fs::write(&rotated, log_with(OLD_DUMP).repeat(2)).unwrap();

        withdraw_once(root.path());

        let now = std::fs::read_to_string(&log).unwrap();
        let before = std::fs::read_to_string(&rotated).unwrap();
        assert!(!hangul_in(&now) && !hangul_in(&before), "{now}\n{before}");
        assert_eq!(before.matches("withdrawn with its 5-line trace").count(), 2);
        assert!(root.path().join(WITHDRAWN_MARK).exists());
        // The rewrite leaves no staging file behind.
        let names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().all(|name| !name.starts_with('.')), "{names:?}");

        // Once marked, a later boot does not read the log at all.
        std::fs::write(&log, log_with(OLD_DUMP)).unwrap();
        withdraw_once(root.path());
        assert_eq!(std::fs::read_to_string(&log).unwrap(), log_with(OLD_DUMP));
    }

    #[test]
    fn a_folder_without_a_log_is_marked_and_a_log_that_cannot_be_read_is_tried_again() {
        let root = tempfile::tempdir().unwrap();
        withdraw_once(root.path());
        assert!(root.path().join(WITHDRAWN_MARK).exists());

        let root = tempfile::tempdir().unwrap();
        // A directory where the log should be: unreadable as a file.
        std::fs::create_dir(root.path().join(WINDOW_LOG)).unwrap();
        withdraw_once(root.path());
        assert!(!root.path().join(WITHDRAWN_MARK).exists());
    }
}
