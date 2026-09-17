//! The scoreboard beat's findings become tasks (docs/design/
//! scoreboard-beat-20260911.md), on the standing-order beat and through the
//! same argv door the crash and QA roads use (`seat_triage`).
//!
//! `zo scoreboard --defer <pending>` runs on a launchd clock with no pane
//! identity, so it cannot speak to the ledger itself; it leaves one JSON line
//! per finding here. Every beat asks the door once for the oldest pending
//! finding whose key was not filed inside the refile window and that this
//! boot has not set aside: a key the ledger refused past the table is set
//! aside with its reason, and the findings behind it are still asked about —
//! one stuck key never holds the rest, and a degraded ledger is one line and
//! ends the asking for this boot. The request name is the key and the
//! day, so a retry after a lost answer is the SAME request, never a second
//! task; a filed key is stamped beside the pending file and its lines leave
//! the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::agent_teams::Host;
use crate::seat_triage::{Door, Road, SeatDoor, Sweep, Triage, tell, triage};

/// The inbox's directory under the home: beside the beat's own state
/// (`tools/scoreboard/beat.sh` defers to `$STATE/pending.jsonl`). Deliberately
/// NOT the window's local data root — which root that is (platform or legacy)
/// is the window's path authority to decide and a launchd shell script cannot
/// ask it, the same reason the release lane's files live under
/// `~/.local/share/zerocode` (`update_runtime::release_dir`).
pub(crate) const BEAT_DIR: &[&str] = &[".local", "share", "zerocode", "scoreboard"];
/// In the inbox: the findings waiting (one JSON line each, as `zo scoreboard
/// --defer` writes them) and the stamps of the keys filed.
pub(crate) const PENDING_FILE: &str = "pending.jsonl";
pub(crate) const FILED_DIR: &str = "filed";
/// A finding older than this is not news a task should carry.
pub(crate) const WINDOW_MS: i64 = 7 * 86_400_000;
/// The same key is filed at most once inside this window — a regression
/// that stays is one task, not one per beat. Past it the ledger is asked:
/// while a task carrying the key is open, that task is the answer and the
/// window starts over; only a key with no open task is filed again.
pub(crate) const REFILE_WINDOW_MS: i64 = 7 * 86_400_000;
/// The title marker `zo scoreboard --file-tasks` also writes, so both roads'
/// tasks read as one family.
pub(crate) const TITLE_MARK: &str = "[scoreboard:";

const ROAD: Road = Road {
    name: "scoreboard task",
    item: "",
    event: "scoreboard:filed",
    id_field: "key",
};

/// One finding, as `zo scoreboard --defer` left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Finding {
    pub key: String,
    pub observed: String,
    pub baseline: String,
    pub words: String,
    /// Unix seconds when the beat found it.
    pub found_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Filed {
    task: String,
    at_ms: i64,
}

/// The inbox under a given home. Separate from [`inbox_dir`] so a test points
/// it at a temporary directory without touching `$HOME`.
pub(crate) fn inbox_dir_under(home: &Path) -> PathBuf {
    BEAT_DIR
        .iter()
        .fold(home.to_path_buf(), |dir, part| dir.join(part))
}

/// `~/.local/share/zerocode/scoreboard`, or `None` on a machine with no home.
pub(crate) fn inbox_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|home| inbox_dir_under(&home))
}

fn pending_file(root: &Path) -> PathBuf {
    root.join(PENDING_FILE)
}

/// A key as a file name and a request word: alphanumerics kept, the rest `-`.
pub(crate) fn key_slug(key: &str) -> String {
    key.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn filed_file(root: &Path, key: &str) -> PathBuf {
    root.join(FILED_DIR).join(format!("{}.json", key_slug(key)))
}

/// Every well-formed line of the pending file, in file order; a torn line
/// costs only itself.
pub(crate) fn read_pending(root: &Path) -> Vec<Finding> {
    std::fs::read_to_string(pending_file(root))
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Finding>(line).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Rewrite the pending file with exactly these findings (none: no file).
fn rewrite_pending(root: &Path, kept: &[Finding]) -> Result<(), String> {
    let file = pending_file(root);
    if kept.is_empty() {
        let _ = std::fs::remove_file(&file);
        return Ok(());
    }
    let mut body = String::new();
    for finding in kept {
        body.push_str(&serde_json::to_string(finding).map_err(|error| error.to_string())?);
        body.push('\n');
    }
    let stage = file.with_extension("jsonl.stage");
    std::fs::write(&stage, body).map_err(|error| error.to_string())?;
    std::fs::rename(&stage, &file).map_err(|error| error.to_string())
}

/// When a key was last filed, if inside the refile window.
fn filed_recently(root: &Path, key: &str, now_ms: i64) -> bool {
    std::fs::read_to_string(filed_file(root, key))
        .ok()
        .and_then(|text| serde_json::from_str::<Filed>(&text).ok())
        .is_some_and(|stamp| now_ms.saturating_sub(stamp.at_ms) < REFILE_WINDOW_MS)
}

/// Whether one line is still news: found inside the window, its key not
/// filed inside the refile window. Judged per line — a stale line of a key
/// says nothing about the fresh lines of the same key behind it.
fn is_news(root: &Path, finding: &Finding, now_ms: i64) -> bool {
    let found_ms = i64::try_from(finding.found_at)
        .unwrap_or(0)
        .saturating_mul(1000);
    now_ms.saturating_sub(found_ms) <= WINDOW_MS && !filed_recently(root, &finding.key, now_ms)
}

/// The oldest pending finding worth a task whose key this boot has not set
/// aside. Lines that are no longer news leave the file on the way; a set-aside
/// key's lines stay for the next boot, and never stand in front of the others.
pub(crate) fn pending(
    root: &Path,
    now_ms: i64,
    set_aside: &BTreeMap<String, String>,
) -> Option<Finding> {
    let (news, old): (Vec<Finding>, Vec<Finding>) = read_pending(root)
        .into_iter()
        .partition(|finding| is_news(root, finding, now_ms));
    if !old.is_empty() {
        let _ = rewrite_pending(root, &news);
    }
    // `min_by_key` keeps the first of equals: a tie goes to file order.
    news.into_iter()
        .filter(|finding| !set_aside.contains_key(&finding.key))
        .min_by_key(|finding| finding.found_at)
}

/// Stamp a key as filed and take its line out of the pending file.
pub(crate) fn note_filed(root: &Path, key: &str, task: &str, now_ms: i64) -> Result<(), String> {
    let file = filed_file(root, key);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_vec_pretty(&Filed {
        task: task.to_string(),
        at_ms: now_ms,
    })
    .map_err(|error| error.to_string())?;
    std::fs::write(&file, body)
        .map_err(|error| format!("could not write {}: {error}", file.display()))?;
    let kept: Vec<Finding> = read_pending(root)
        .into_iter()
        .filter(|finding| finding.key != key)
        .collect();
    rewrite_pending(root, &kept)
}

/// `YYYYMMDD` of a unix time (civil-from-days, Hinnant) — the day part of the
/// request name, so one key is one request per day.
pub(crate) fn day_stamp(secs: u64) -> String {
    let z = i64::try_from(secs / 86_400).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}")
}

/// The `task-create` verb for one finding, element by element; the request
/// name is the key and the day it was found.
pub(crate) fn task_argv(finding: &Finding) -> Vec<String> {
    vec![
        "task-create".to_string(),
        "--retry-request".to_string(),
        format!(
            "scoreboard-{}-{}",
            key_slug(&finding.key),
            day_stamp(finding.found_at)
        ),
        "--title".to_string(),
        format!("점수표 {} {}", title_key(&finding.key), finding.observed),
        "--spec".to_string(),
        format!(
            "{} — observed {}, baseline {}. Filed by the scoreboard beat (docs/design/scoreboard-beat-20260911.md): find the cause with numbers, fix red-first, and say what moved in the commit.",
            finding.words, finding.observed, finding.baseline
        ),
    ]
}

/// `[scoreboard:<key>]` — the part of a task's title that names its key, on
/// both roads (`zo scoreboard --file-tasks` writes the same marker).
pub(crate) fn title_key(key: &str) -> String {
    format!("{TITLE_MARK}{key}]")
}

/// Ask the door once for the oldest pending finding this boot has not set
/// aside — one read of the pending file, so the finding judged is the finding
/// filed.
pub(crate) fn file_scoreboard_task(
    root: &Path,
    now_ms: i64,
    set_aside: &BTreeMap<String, String>,
    door: &dyn Door,
) -> Triage {
    let Some(finding) = pending(root, now_ms, set_aside) else {
        return Triage::Nothing;
    };
    // A regression that stays is the task already open for it: the stamp is
    // renewed with that task and the lines leave, so the refile window counts
    // from now and a second task is never cut while the first is open.
    if let Some(task) = door.open_task(&title_key(&finding.key)) {
        return match note_filed(root, &finding.key, &task, now_ms) {
            Ok(()) => Triage::Open {
                item: finding.key,
                task,
            },
            Err(why) => Triage::Refused {
                item: finding.key,
                why: format!("open as {task} but the stamp could not be written: {why}"),
            },
        };
    }
    let argv = task_argv(&finding);
    triage(finding.key, door.file(&argv), |key, task| {
        note_filed(root, key, task, now_ms)
    })
}

static SWEEP: Mutex<Sweep> = Mutex::new(Sweep::new());

/// One beat over this boot's memory: what the door said and the line to log.
pub(crate) fn beat(
    sweep: &mut Sweep,
    root: &Path,
    now_ms: i64,
    door: &dyn Door,
) -> (Triage, Option<String>) {
    sweep.beat(&ROAD, |set_aside| {
        file_scoreboard_task(root, now_ms, set_aside, door)
    })
}

/// One beat: ask the door for the oldest finding this boot has not set aside.
pub(crate) fn sweep(
    app: &AppHandle,
    host: &dyn Host,
    overrides: &[(String, zerocode_core::launch::LaunchOverride)],
    now_ms: i64,
) {
    let Some(inbox) = inbox_dir() else {
        return;
    };
    let door = SeatDoor {
        host,
        overrides,
        now_ms,
    };
    let (said, line) = beat(
        &mut SWEEP.lock().unwrap_or_else(|held| held.into_inner()),
        &inbox,
        now_ms,
        &door,
    );
    tell(app, &ROAD, &said, line.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::SeatFiling;
    use crate::seat_triage::REFUSALS_PER_BOOT;

    fn finding(key: &str, found_at: u64) -> Finding {
        Finding {
            key: key.to_string(),
            observed: "p50 2600 ms".to_string(),
            baseline: "p50 2000 ms".to_string(),
            words: format!("{key} rose"),
            found_at,
        }
    }

    fn none() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// A ledger that refuses the named keys, holds the named open tasks by
    /// title key, files everything else as `t-7`, and remembers the titles it
    /// filed.
    #[derive(Default)]
    struct FakeDoor {
        refuse: Vec<&'static str>,
        open: Vec<(&'static str, &'static str)>,
        filed: std::cell::RefCell<Vec<String>>,
    }

    impl Door for FakeDoor {
        fn open_task(&self, title_key: &str) -> Option<String> {
            self.open
                .iter()
                .find(|(key, _)| *key == title_key)
                .map(|(_, task)| (*task).to_string())
        }

        fn file(&self, argv: &[String]) -> Result<String, SeatFiling> {
            let title = &argv[4];
            if self
                .refuse
                .iter()
                .any(|key| title.contains(&title_key(key)))
            {
                return Err(SeatFiling::Refused("already answered".into()));
            }
            self.filed.borrow_mut().push(title.clone());
            Ok("t-7".to_string())
        }
    }

    #[test]
    fn a_key_whose_task_is_still_open_is_that_task_and_not_a_new_one() {
        let root = tempfile::tempdir().expect("tempdir");
        let now_ms = WINDOW_MS + 5_000_000;
        // Past the refile window the stamp no longer holds the key back; the
        // task it named (or one the CLI road filed) is still open.
        leave(root.path(), &[finding("disk", 8_000)]);
        let door = FakeDoor {
            open: vec![("[scoreboard:disk]", "t-3682")],
            ..FakeDoor::default()
        };
        let mut sweep = Sweep::default();
        let (said, line) = beat(&mut sweep, root.path(), now_ms, &door);
        assert!(
            door.filed.borrow().is_empty(),
            "no second task while t-3682 is open: {said:?}"
        );
        assert_eq!(
            said,
            Triage::Open {
                item: "disk".into(),
                task: "t-3682".into()
            }
        );
        assert_eq!(line, None, "the ledger already shows the open task");
        assert!(
            read_pending(root.path()).is_empty(),
            "the line left the file"
        );
        let stamp: Filed = serde_json::from_slice(
            &std::fs::read(filed_file(root.path(), "disk")).expect("stamped"),
        )
        .expect("stamp");
        assert_eq!(stamp.task, "t-3682");
        assert_eq!(stamp.at_ms, now_ms, "the refile window starts over");
    }

    fn leave(root: &Path, findings: &[Finding]) {
        let file = pending_file(root);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mut body = String::new();
        for f in findings {
            body.push_str(&serde_json::to_string(f).unwrap());
            body.push('\n');
        }
        body.push_str("{not json\n");
        std::fs::write(file, body).unwrap();
    }

    #[test]
    fn the_oldest_unfiled_finding_is_next_and_stale_or_recently_filed_ones_leave_the_file() {
        let root = tempfile::tempdir().expect("tempdir");
        // Seven days and a little after the epoch: findings at seconds 8 000
        // and 9 000 are fresh, one at second 1 is past the window.
        let now_ms = WINDOW_MS + 5_000_000;
        assert_eq!(pending(root.path(), now_ms, &none()), None);
        leave(
            root.path(),
            &[
                finding("gate:gate-zo", 9_000),
                finding("first-byte:m", 8_000),
                finding("disk", 1),
            ],
        );
        assert_eq!(
            pending(root.path(), now_ms, &none()).map(|f| f.key),
            Some("first-byte:m".to_string()),
            "oldest fresh first"
        );
        assert_eq!(
            read_pending(root.path()).len(),
            2,
            "the stale line and the torn line were swept"
        );
        note_filed(root.path(), "first-byte:m", "t-9", now_ms).expect("stamped");
        assert_eq!(
            pending(root.path(), now_ms, &none()).map(|f| f.key),
            Some("gate:gate-zo".to_string()),
            "a filed key leaves the file"
        );
        assert_eq!(read_pending(root.path()).len(), 1);
        // The same key found again inside the refile window is not news.
        leave(
            root.path(),
            &[
                finding("first-byte:m", 9_500),
                finding("gate:gate-zo", 9_000),
            ],
        );
        assert_eq!(
            pending(root.path(), now_ms + 1_000, &none()).map(|f| f.key),
            Some("gate:gate-zo".to_string())
        );
        assert_eq!(
            read_pending(root.path()).len(),
            1,
            "the recently filed key was swept"
        );
        // Past the refile window it is news again (found freshly, so not stale).
        let later_ms = now_ms + REFILE_WINDOW_MS + 1_000;
        let found_at = u64::try_from(later_ms / 1000 - 10).expect("positive");
        leave(root.path(), &[finding("first-byte:m", found_at)]);
        assert_eq!(
            pending(root.path(), later_ms, &none()).map(|f| f.key),
            Some("first-byte:m".to_string())
        );
    }

    #[test]
    fn a_key_given_up_this_boot_is_set_aside_and_the_next_finding_is_filed() {
        let root = tempfile::tempdir().expect("tempdir");
        let now_ms = WINDOW_MS + 5_000_000;
        // `disk` is the oldest and the ledger refuses it every time (the same
        // request name already answered for another payload); `first-byte:m`
        // behind it is fileable.
        leave(
            root.path(),
            &[finding("disk", 8_000), finding("first-byte:m", 9_000)],
        );
        let mut sweep = Sweep::default();
        let door = FakeDoor {
            refuse: vec!["disk"],
            ..FakeDoor::default()
        };
        let (said, lines): (Vec<Triage>, Vec<Option<String>>) = (0..(REFUSALS_PER_BOOT + 2))
            .map(|_| beat(&mut sweep, root.path(), now_ms, &door))
            .unzip();
        assert!(
            said.contains(&Triage::Filed {
                item: "first-byte:m".into(),
                task: "t-7".into()
            }),
            "a given-up key does not block the next finding: {said:?}"
        );
        assert_eq!(
            lines.into_iter().flatten().collect::<Vec<_>>(),
            [
                "scoreboard task: disk not filed — refused 3 times (already answered) — set aside until the next boot",
                "scoreboard task: first-byte:m filed as t-7",
            ]
        );
        assert_eq!(
            sweep.set_aside().get("disk").map(String::as_str),
            Some("already answered"),
            "set aside with its reason"
        );
        assert_eq!(
            read_pending(root.path())
                .iter()
                .map(|f| f.key.as_str())
                .collect::<Vec<_>>(),
            ["disk"],
            "the given-up line waits in the file for the next boot"
        );
    }

    #[test]
    fn a_stale_line_leaves_alone_and_takes_no_fresh_line_of_its_key_with_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let now_ms = WINDOW_MS + 5_000_000;
        leave(root.path(), &[finding("disk", 1), finding("disk", 8_000)]);
        assert_eq!(
            pending(root.path(), now_ms, &none()).map(|f| f.found_at),
            Some(8_000)
        );
        assert_eq!(
            read_pending(root.path())
                .iter()
                .map(|f| f.found_at)
                .collect::<Vec<_>>(),
            [8_000],
            "only the stale line was swept"
        );
    }

    #[test]
    fn the_beat_defers_to_the_inbox_this_window_reads() {
        // The launchd beat and the window meet at one file; pin both halves
        // so neither moves alone.
        let beat = include_str!("../../../tools/scoreboard/beat.sh");
        let state = format!(
            "STATE=${{SCOREBOARD_STATE:-$HOME_DIR/{}}}",
            BEAT_DIR.join("/")
        );
        assert!(beat.contains(&state), "beat.sh keeps its state at {state}");
        let pending = format!("PENDING=\"$STATE/{PENDING_FILE}\"");
        assert!(beat.contains(&pending), "beat.sh defers to {pending}");
        assert!(
            inbox_dir_under(Path::new("/h")).ends_with(".local/share/zerocode/scoreboard"),
            "{:?}",
            inbox_dir_under(Path::new("/h"))
        );
    }

    #[test]
    fn the_task_carries_the_key_the_numbers_and_a_request_named_by_key_and_day() {
        let argv = task_argv(&finding("first-byte:claude-opus-5", 1_789_100_000));
        assert_eq!(
            &argv[..3],
            &[
                "task-create",
                "--retry-request",
                "scoreboard-first-byte-claude-opus-5-20260911"
            ]
        );
        assert_eq!(argv[3], "--title");
        assert_eq!(
            argv[4],
            "점수표 [scoreboard:first-byte:claude-opus-5] p50 2600 ms"
        );
        assert_eq!(argv[5], "--spec");
        assert!(
            argv[6].contains("first-byte:claude-opus-5 rose")
                && argv[6].contains("baseline p50 2000 ms")
        );
        assert_eq!(day_stamp(0), "19700101");
    }
}
