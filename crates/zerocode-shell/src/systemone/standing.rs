//! Where a seat stands, read off its ledger a piece at a time (t-19914).
//!
//! Every hook event asks where a seat stands, and the answer used to start at
//! the ledger's first line each time, so the price of an event was the
//! ledger's size: about 15% of a core in the window on a busy day. A ledger
//! only grows by appending, so the answer is the newest line that spoke —
//! and the newest line that spoke is either in what was appended since the
//! last read or is what the last read already found.
//!
//! One reader serves both roads. [`standing_of`] keeps, per ledger file and
//! per seat, a [`Cursor`]: the byte offset read to, the file it was read
//! from and the standing the lines before the offset left. The whole file is
//! the same read from a cursor that has read nothing, so the first read of a
//! file, a file that shrank or was replaced, and the next read of one that
//! only grew are one code path, and the answer cannot differ between them.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use zerocode_core::jev::JevUse;
use zerocode_core::jev::promote::{Stand, newest_standing_in};

/// How much of a file's first bytes a cursor remembers to tell the file it
/// read from a different one written under the same name and size. A rotated
/// ledger starts with its own first row, which differs from the old file's
/// within a few dozen bytes (a row opens with its timestamp), and a file
/// replaced by a longer one with the same inode — a rewrite in place — would
/// otherwise be told apart from the grown one by nothing. Bounded so the
/// check costs one small read however long the ledger is.
const HEAD_BYTES: usize = 256;

/// What a read of a ledger left, to start the next read from.
struct Cursor {
    /// The file the bytes were read from: `(device, inode)`.
    identity: (u64, u64),
    /// The first bytes of that file, at most [`HEAD_BYTES`] of them.
    head: Vec<u8>,
    /// How far the file has been read: always just past a line's newline, so
    /// a record the writer is half way through is never parsed and is read
    /// whole by the next read.
    offset: u64,
    /// The newest standing any line before `offset` said; `None` when none did.
    said: Option<Stand>,
    /// A line before `offset` is not UTF-8, which the whole-file read
    /// ([`super::read_rows`] reads text) answers with no rows at all; the
    /// answer stays so until the file is not the one read.
    unreadable: bool,
}

/// What names a cursor: the ledger, and the two things of a seat that decide
/// what a line says about it (`stands_on`) — the words it asks and where it
/// starts. Two seats reading one file under different words read it apart.
type Key = (PathBuf, u32, bool);

/// The cursors, one per ledger and seat read in this process. Bounded by the
/// seats times the ledgers there are, a few dozen small values.
static CURSORS: Mutex<Option<HashMap<Key, Cursor>>> = Mutex::new(None);

/// Where `seat` stands by the rows of `ledger`: the same answer as
/// `promote::standing(seat, &read_rows(ledger))`, read from where the last
/// read of this file stopped.
pub(super) fn standing_of(seat: &JevUse, ledger: &Path) -> Stand {
    let key: Key = (
        ledger.to_path_buf(),
        seat.rubric_version,
        matches!(seat.auto_starts, Stand::Applying),
    );
    let before = CURSORS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_mut()
        .and_then(|cursors| cursors.remove(&key));
    let (after, stand) = read_on(seat, ledger, before);
    if let Some(after) = after {
        CURSORS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(key, after);
    }
    stand
}

/// Read `ledger` on from `before`, or from its first line when `before` is
/// not a cursor of this file as it is now. The cursor to keep (none for a
/// file that cannot be read) and where `seat` stands.
fn read_on(seat: &JevUse, ledger: &Path, before: Option<Cursor>) -> (Option<Cursor>, Stand) {
    let start = seat.auto_starts;
    let Ok(mut file) = File::open(ledger) else {
        return (None, start);
    };
    let Some(identity) = file.metadata().ok().and_then(|meta| identity_of(&meta)) else {
        // A file with no identity to tell it by is read whole every time.
        return (None, whole_file_standing(seat, file));
    };
    let mut cursor = before
        .filter(|cursor| cursor.identity == identity && still_that_file(cursor, &mut file))
        .unwrap_or_else(|| Cursor {
            identity,
            head: Vec::new(),
            offset: 0,
            said: None,
            unreadable: false,
        });
    let mut appended = Vec::new();
    if file.seek(SeekFrom::Start(cursor.offset)).is_err()
        || file.read_to_end(&mut appended).is_err()
    {
        return (None, start);
    }
    // Only whole lines move the cursor; the unfinished one after the last
    // newline is read again, whole, once its writer has finished it.
    let whole = appended
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |last| last + 1);
    let (lines, tail) = appended.split_at(whole);
    let room = HEAD_BYTES.saturating_sub(cursor.head.len());
    cursor
        .head
        .extend_from_slice(&lines[..room.min(lines.len())]);
    match std::str::from_utf8(lines) {
        Ok(text) => cursor.said = newest_standing_in(seat, text).or(cursor.said),
        Err(_) => cursor.unreadable = true,
    }
    cursor.offset += lines.len() as u64;
    // A last line with no newline yet still counts if it is a whole record,
    // as it does in a whole-file read — it is read now and read again later.
    let stand = if cursor.unreadable {
        start
    } else {
        match std::str::from_utf8(tail) {
            Ok(text) => newest_standing_in(seat, text)
                .or(cursor.said)
                .unwrap_or(start),
            Err(_) => start,
        }
    };
    (Some(cursor), stand)
}

/// Whether the file at `cursor.offset` and before is still the one `cursor`
/// read: not shorter than what was read, and opening with the same bytes.
fn still_that_file(cursor: &Cursor, file: &mut File) -> bool {
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return false;
    };
    if len < cursor.offset {
        return false;
    }
    let mut head = vec![0; cursor.head.len()];
    file.seek(SeekFrom::Start(0)).is_ok()
        && file.read_exact(&mut head).is_ok()
        && head == cursor.head
}

/// Where `seat` stands by all of `file`, with no cursor: for a platform that
/// gives a file no identity.
fn whole_file_standing(seat: &JevUse, mut file: File) -> Stand {
    let mut text = String::new();
    match file.read_to_string(&mut text) {
        Ok(_) => newest_standing_in(seat, &text).unwrap_or(seat.auto_starts),
        Err(_) => seat.auto_starts,
    }
}

#[cfg(unix)]
fn identity_of(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino()))
}

#[cfg(not(unix))]
fn identity_of(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde_json::{Value, json};
    use zerocode_core::jev::promote::{self, FELL, ROSE};
    use zerocode_core::jev::summary::TRANSITION;
    use zerocode_core::jev::{BROWSER, COVER, JevUse, Run, count};

    use super::standing_of;
    use crate::systemone::{Wire, read_rows, standing_in};

    /// The ledger lines of a busy day's seat: thousands of requests.
    const BUSY_DAY_LINES: usize = 4_000;
    /// The hook events the busy ledger is asked about, one row appended each.
    const EVENTS: usize = 50;

    fn request(seat: &JevUse, number: usize) -> Value {
        json!({"request": number, "rubricVersion": seat.rubric_version, "ms": 1_700_000_000 + number})
    }

    fn transition(word: &str, rubric: u32) -> Value {
        json!({(TRANSITION.canonical): word, "rubricVersions": [rubric]})
    }

    fn append(path: &std::path::Path, text: &str) {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .expect("an appended ledger");
    }

    fn line(row: &Value) -> String {
        format!("{row}\n")
    }

    /// What the whole-file read says: every row parsed, newest first.
    fn from_scratch(seat: &JevUse, path: &std::path::Path) -> promote::Stand {
        promote::standing(seat, &read_rows(path))
    }

    /// N hook events against a ledger of M lines parse what each event
    /// appended, not N times M (t-19914): the number was 50 × 4,001 = 200,050
    /// before the standing was read from where the last read stopped.
    #[test]
    fn a_hook_event_parses_what_was_appended_and_not_the_ledger() {
        let home = tempfile::tempdir().expect("a zo home");
        let settings = home.path().join("settings.json");
        std::fs::write(&settings, "{}").expect("a settings file");
        let wire = Wire::at("http://127.0.0.1:9", "test-key", Some(settings));
        let ledger = home.path().join(count::REQUESTS_DIR).join(COVER.ledger);
        std::fs::create_dir_all(ledger.parent().expect("a folder")).expect("a ledger folder");
        let mut busy = String::new();
        for number in 0..BUSY_DAY_LINES {
            busy.push_str(&line(&request(&COVER, number)));
        }
        busy.push_str(&line(&transition(ROSE, COVER.rubric_version)));
        append(&ledger, &busy);

        let before = promote::lines_parsed();
        for event in 0..EVENTS {
            append(&ledger, &line(&request(&COVER, BUSY_DAY_LINES + event)));
            let _ = standing_in(&wire, &COVER, Run::Fresh);
        }
        let parsed = promote::lines_parsed() - before;
        let whole_ledger_every_event = (EVENTS * (BUSY_DAY_LINES + 1)) as u64;
        assert!(
            parsed < whole_ledger_every_event / 100,
            "{EVENTS} events parsed {parsed} lines; a whole read each is {whole_ledger_every_event}"
        );
    }

    /// The incremental standing is the whole read's over appends, a partial
    /// last line, its completion, a truncation and two replacements — for a
    /// seat that starts recording and one that starts acting.
    #[test]
    fn the_incremental_standing_is_the_from_scratch_standing_at_every_step() {
        for seat in [&COVER, &BROWSER] {
            let home = tempfile::tempdir().expect("a folder");
            let path = home.path().join(seat.ledger);
            let check = |step: &str| {
                assert_eq!(
                    standing_of(seat, &path),
                    from_scratch(seat, &path),
                    "{} after {step}",
                    seat.id
                );
            };
            check("no file");
            append(&path, &line(&request(seat, 1)));
            check("a request");
            append(&path, &line(&transition(ROSE, seat.rubric_version)));
            check("a rise");
            append(&path, &line(&request(seat, 2)));
            check("a request after the rise");
            let fall = line(&transition(FELL, seat.rubric_version));
            let (first, rest) = fall.split_at(fall.len() / 2);
            append(&path, first);
            check("half a fall");
            append(&path, rest);
            check("the fall finished");
            let whole_without_newline = transition(ROSE, seat.rubric_version).to_string();
            append(&path, &whole_without_newline);
            check("a whole rise with no newline yet");
            append(&path, "\n");
            check("its newline");
            append(&path, &line(&transition(ROSE, seat.rubric_version + 1)));
            check("a rise of other words");
            append(
                &path,
                &line(&request(seat, seat.rubric_version as usize + 9)),
            );
            check("more");
            let mut newer = request(seat, 3);
            newer["rubricVersion"] = json!(seat.rubric_version + 1);
            append(&path, &line(&newer));
            check("a request of newer words");

            // A shorter file: truncated to a rise.
            std::fs::write(&path, line(&transition(ROSE, seat.rubric_version)))
                .expect("a truncation");
            check("a truncation");
            // Replaced by a file of the same size, then by a longer one.
            let was = std::fs::read_to_string(&path).expect("the ledger");
            let swapped = home.path().join("swapped");
            std::fs::write(&swapped, was.replace("rise", "fall")).expect("a same-size swap");
            std::fs::rename(&swapped, &path).expect("a replacement");
            check("a replacement of the same size");
            std::fs::write(
                &swapped,
                format!(
                    "{}{}",
                    line(&request(seat, 8)),
                    line(&transition(ROSE, seat.rubric_version))
                ),
            )
            .expect("a longer one");
            std::fs::rename(&swapped, &path).expect("a replacement");
            check("a replacement by a longer file");
            std::fs::write(
                &path,
                line(&request(seat, 7))
                    + &line(&transition(FELL, seat.rubric_version))
                    + &line(&request(seat, 4)),
            )
            .expect("a rewrite in place");
            check("a rewrite in place");
        }
    }

    /// Lines of the synthetic ledger a cost report reads, by ledger size: a
    /// busy day, and ten of them.
    #[cfg(unix)]
    const REPORT_LEDGER_LINES: [usize; 2] = [4_000, 40_000];
    /// Hook events in a report's burst.
    #[cfg(unix)]
    const REPORT_EVENTS: usize = 2_000;

    /// This process's user+system CPU seconds so far.
    #[cfg(unix)]
    fn cpu_seconds() -> f64 {
        let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
        // SAFETY: `getrusage` fills the struct it is given and reads nothing of ours.
        let usage = unsafe {
            libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr());
            usage.assume_init()
        };
        let seconds =
            |time: libc::timeval| time.tv_sec as f64 + f64::from(time.tv_usec) / 1e6;
        seconds(usage.ru_utime) + seconds(usage.ru_stime)
    }

    /// The cost of one hook event's standing read, whole-file against
    /// incremental, on a synthetic ledger in a temporary folder — never a
    /// person's (t-19914). Run by hand: `-- --ignored --nocapture
    /// standing_cost_report`, once plain and once under `taskpolicy -b`.
    #[cfg(unix)]
    #[test]
    #[ignore = "a measurement, run by hand"]
    fn standing_cost_report() {
        let seat = &COVER;
        for lines in REPORT_LEDGER_LINES {
            let home = tempfile::tempdir().expect("a folder");
            let path = home.path().join(seat.ledger);
            let mut busy = String::new();
            for number in 0..lines {
                busy.push_str(&line(&request(seat, number)));
                if number % 500 == 0 {
                    busy.push_str(&line(&transition(ROSE, seat.rubric_version)));
                }
            }
            append(&path, &busy);
            let measure = |name: &str, read: &dyn Fn() -> promote::Stand| {
                let (parsed, cpu, wall) = (
                    promote::lines_parsed(),
                    cpu_seconds(),
                    std::time::Instant::now(),
                );
                for event in 0..REPORT_EVENTS {
                    append(&path, &line(&request(seat, lines + event)));
                    std::hint::black_box(read());
                }
                let wall = wall.elapsed().as_secs_f64();
                println!(
                    "ledger {lines:>6} lines, {name:<11}: {:>8.1} us/event wall, {:>7.3} s cpu over {REPORT_EVENTS} events, {:>6} lines parsed/event",
                    wall / REPORT_EVENTS as f64 * 1e6,
                    cpu_seconds() - cpu,
                    (promote::lines_parsed() - parsed) / REPORT_EVENTS as u64,
                );
            };
            measure("whole read", &|| promote::standing(seat, &read_rows(&path)));
            measure("incremental", &|| standing_of(seat, &path));
        }
    }
}
