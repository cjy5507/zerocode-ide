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

/// How a dump's header opens as the log holds it: `note_window_event` puts
/// `window: ` ahead of the words `reportStrayJamo` sends.
const DUMP_OPENS: &str = "window: ime: bare jamo left through ";

/// The mark a finished withdrawal leaves beside the log.
const WITHDRAWN_MARK: &str = "window-errors.ime-withdrawn";

/// The log and the one rotation `note_window_event` keeps.
const LOGS: [&str; 2] = [WINDOW_LOG, WINDOW_LOG_ROTATED];
use crate::system_runtime::{WINDOW_LOG, WINDOW_LOG_ROTATED};

/// Withdraw every old dump from the window's log, once per data folder.
///
/// Runs at boot before anything else writes the log, so the rewrite cannot
/// race an append. Nothing here can stop the boot: a file that cannot be
/// read or replaced is said on stderr, and the mark is left unwritten so the
/// next boot tries again.
pub(crate) fn withdraw_once(local_data_root: &Path) {
    let _ = local_data_root;
}

/// The log with every old dump withdrawn, and how many there were — `None`
/// when there was nothing to withdraw.
fn withdrawn(log: &[u8]) -> Option<(Vec<u8>, usize)> {
    let _ = log;
    None
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
        text.chars().any(|ch| {
            ('\u{3130}'..='\u{318f}').contains(&ch) || ('\u{ac00}'..='\u{d7a3}').contains(&ch)
        })
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
