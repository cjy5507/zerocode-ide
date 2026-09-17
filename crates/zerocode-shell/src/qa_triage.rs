//! A QA scenario's failing verdict becomes a task (docs/design/
//! computer-use-full-operator.md §4), on the standing-order beat and through
//! the same argv door the crash and scoreboard roads use (`seat_triage`).
//!
//! The automation's `done` reads the verdict its evidence folder holds and
//! leaves a pending file here; every beat asks the door once for the oldest
//! pending failure this boot has not set aside. A run the ledger refuses past
//! the table is set aside with its reason and the failures behind it are
//! still asked about; a degraded ledger is one line and ends the asking for
//! this boot. The request name is the run id, so a retry after a lost answer
//! is the SAME request, never a second task; a filed failure is stamped and
//! its pending file goes, so no beat reads it again.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager as _};

use crate::agent_teams::Host;
use crate::seat_triage::{Door, Road, SeatDoor, Sweep, Triage, tell, triage};

/// Under the window's local data root: the failures waiting, and the stamps
/// of those filed.
pub(crate) const PENDING_DIR: &str = "computer-use/qa-failures";
pub(crate) const FILED_DIR: &str = "computer-use/qa-failures/filed";
/// A failure older than this is not news a task should carry.
pub(crate) const WINDOW_MS: i64 = 7 * 86_400_000;
/// How much of a reason the task's spec keeps.
pub(crate) const REASON_CHARS: usize = 400;

const ROAD: Road = Road {
    name: "qa task",
    item: "run ",
    event: "qa:triaged",
    id_field: "runId",
};

/// One failing verdict, as the `done` left it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct QaFailure {
    pub run_id: String,
    pub automation_id: String,
    pub evidence_dir: String,
    pub reason: String,
    pub at_ms: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Filed {
    task: String,
}

fn pending_file(root: &Path, run_id: &str) -> PathBuf {
    root.join(PENDING_DIR).join(format!("{run_id}.json"))
}

fn filed_file(root: &Path, run_id: &str) -> PathBuf {
    root.join(FILED_DIR).join(format!("{run_id}.json"))
}

/// Leave a failing verdict for the beat. Idempotent per run: the same run
/// failing twice is one pending file.
pub(crate) fn note_failure(root: &Path, failure: &QaFailure) -> Result<PathBuf, String> {
    let file = pending_file(root, &failure.run_id);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_vec_pretty(failure).map_err(|error| error.to_string())?;
    std::fs::write(&file, body)
        .map_err(|error| format!("could not write {}: {error}", file.display()))?;
    Ok(file)
}

/// The oldest failure still waiting that this boot has not set aside:
/// pending, not stamped, within the window. A stale or stamped one is removed
/// on the way — the stamp is the record — so the folder holds only news.
pub(crate) fn pending(
    root: &Path,
    now_ms: i64,
    set_aside: &BTreeMap<String, String>,
) -> Option<QaFailure> {
    let entries = std::fs::read_dir(root.join(PENDING_DIR)).ok()?;
    entries
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let bytes = std::fs::read(entry.path()).ok()?;
            serde_json::from_slice::<QaFailure>(&bytes).ok()
        })
        .filter(|failure| {
            let stale = now_ms.saturating_sub(failure.at_ms) > WINDOW_MS;
            if stale || filed_file(root, &failure.run_id).exists() {
                let _ = std::fs::remove_file(pending_file(root, &failure.run_id));
                return false;
            }
            !set_aside.contains_key(&failure.run_id)
        })
        .min_by_key(|failure| failure.at_ms)
}

/// The once-only stamp: this run's failure became that task. Written after
/// the ledger answered, never before; the pending file goes after it.
pub(crate) fn note_filed(root: &Path, run_id: &str, task: &str) -> Result<(), String> {
    let file = filed_file(root, run_id);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let body = serde_json::to_vec_pretty(&Filed {
        task: task.to_string(),
    })
    .map_err(|error| error.to_string())?;
    std::fs::write(&file, body)
        .map_err(|error| format!("could not write {}: {error}", file.display()))?;
    let _ = std::fs::remove_file(pending_file(root, run_id));
    Ok(())
}

/// The task's spec: what failed, where the evidence is, what to do.
pub(crate) fn task_body(failure: &QaFailure) -> String {
    let reason: String = failure.reason.chars().take(REASON_CHARS).collect();
    format!(
        "QA 실패: 자동화 {} 실행 {} — {}\n증거 폴더: {}\n단계 로그 steps.jsonl 과 뒤 프레임, qa-verdict.json, report.md 를 읽고 원인을 고치거나 시나리오를 바로잡는다. 고친 뒤 같은 시나리오를 다시 돌려 verdict --pass 를 남긴다.",
        failure.automation_id, failure.run_id, reason, failure.evidence_dir
    )
}

/// The `task-create` verb for one failure, element by element; the request
/// name is the run id.
pub(crate) fn task_argv(failure: &QaFailure) -> Vec<String> {
    vec![
        "task-create".to_string(),
        "--retry-request".to_string(),
        format!("qa-{}", failure.run_id),
        "--spec".to_string(),
        task_body(failure),
    ]
}

/// Ask the door once for the oldest pending failure this boot has not set
/// aside.
pub(crate) fn file_qa_task(
    root: &Path,
    now_ms: i64,
    set_aside: &BTreeMap<String, String>,
    door: &dyn Door,
) -> Triage {
    let Some(failure) = pending(root, now_ms, set_aside) else {
        return Triage::Nothing;
    };
    let argv = task_argv(&failure);
    triage(failure.run_id, door.file(&argv), |run, task| {
        note_filed(root, run, task)
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
        file_qa_task(root, now_ms, set_aside, door)
    })
}

/// One beat: ask the door for the oldest failure this boot has not set aside.
pub(crate) fn sweep(
    app: &AppHandle,
    host: &dyn Host,
    overrides: &[(String, zerocode_core::launch::LaunchOverride)],
    now_ms: i64,
) {
    let root = app
        .state::<crate::AppState>()
        .local_data_root()
        .to_path_buf();
    let door = SeatDoor {
        host,
        overrides,
        now_ms,
    };
    let (said, line) = beat(
        &mut SWEEP.lock().unwrap_or_else(|held| held.into_inner()),
        &root,
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

    fn failure(run: &str, at_ms: i64) -> QaFailure {
        QaFailure {
            run_id: run.to_string(),
            automation_id: "nightly-checkout".to_string(),
            evidence_dir: format!("/tmp/evidence/{run}"),
            reason: "the cart stayed empty".to_string(),
            at_ms,
        }
    }

    fn none() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    /// A ledger that refuses the named runs (or is degraded), files
    /// everything else as `t-7`, and counts every ask.
    #[derive(Default)]
    struct FakeDoor {
        refuse: Vec<&'static str>,
        down: bool,
        asked: std::cell::Cell<u32>,
    }

    impl Door for FakeDoor {
        fn file(&self, argv: &[String]) -> Result<String, SeatFiling> {
            self.asked.set(self.asked.get() + 1);
            if self.down {
                return Err(SeatFiling::Unavailable("the ledger is gone".into()));
            }
            if self.refuse.iter().any(|run| argv[2] == format!("qa-{run}")) {
                return Err(SeatFiling::Refused("already answered".into()));
            }
            Ok("t-7".to_string())
        }

        fn open_task(&self, _: &str) -> Option<String> {
            None
        }
    }

    #[test]
    fn a_failure_waits_oldest_first_until_stamped_and_a_stale_one_is_swept() {
        let root = tempfile::tempdir().expect("tempdir");
        assert_eq!(pending(root.path(), 10_000, &none()), None);
        note_failure(root.path(), &failure("run-b", 2_000)).expect("noted");
        note_failure(root.path(), &failure("run-a", 1_000)).expect("noted");
        note_failure(root.path(), &failure("run-a", 1_000)).expect("noted twice is one file");
        assert_eq!(
            pending(root.path(), 10_000, &none()).map(|f| f.run_id),
            Some("run-a".to_string()),
            "oldest first"
        );
        note_filed(root.path(), "run-a", "t-9").expect("stamped");
        assert_eq!(
            pending(root.path(), 10_000, &none()).map(|f| f.run_id),
            Some("run-b".to_string()),
            "a stamped one is done"
        );
        note_failure(root.path(), &failure("run-old", 0)).expect("noted");
        // At this instant run-old (at 0) is past the window; run-b (at 2_000) is not.
        assert_eq!(
            pending(root.path(), WINDOW_MS + 1_000, &none()).map(|f| f.run_id),
            Some("run-b".to_string()),
            "a failure older than the window is not news"
        );
        assert!(
            !root.path().join(PENDING_DIR).join("run-old.json").exists(),
            "and it was swept"
        );
    }

    #[test]
    fn the_task_names_the_run_the_reason_and_the_evidence_and_the_request_is_the_run() {
        let argv = task_argv(&failure("run-7", 1));
        assert_eq!(
            &argv[..4],
            &["task-create", "--retry-request", "qa-run-7", "--spec"]
        );
        assert!(
            argv[4].contains("nightly-checkout")
                && argv[4].contains("the cart stayed empty")
                && argv[4].contains("/tmp/evidence/run-7")
        );
        let long = QaFailure {
            reason: "x".repeat(REASON_CHARS * 2),
            ..failure("run-8", 1)
        };
        assert!(
            task_body(&long).matches('x').count() == REASON_CHARS,
            "the reason is cut at the table"
        );
    }

    #[test]
    fn a_run_refused_past_the_table_is_set_aside_and_the_failure_behind_it_is_filed() {
        let root = tempfile::tempdir().expect("tempdir");
        note_failure(root.path(), &failure("run-a", 1_000)).expect("noted");
        note_failure(root.path(), &failure("run-b", 2_000)).expect("noted");
        let door = FakeDoor {
            refuse: vec!["run-a"],
            ..FakeDoor::default()
        };
        let mut sweep = Sweep::new();
        let (said, lines): (Vec<Triage>, Vec<Option<String>>) = (0..=REFUSALS_PER_BOOT)
            .map(|_| beat(&mut sweep, root.path(), 10_000, &door))
            .unzip();
        assert!(
            said.contains(&Triage::Filed {
                item: "run-b".into(),
                task: "t-7".into()
            }),
            "a set-aside run does not hold the failure behind it: {said:?}"
        );
        let lines: Vec<String> = lines.into_iter().flatten().collect();
        assert_eq!(
            lines,
            [
                "qa task: run run-a not filed — refused 3 times (already answered) — set aside until the next boot",
                "qa task: run run-b filed as t-7",
            ]
        );
        assert_eq!(
            pending(root.path(), 10_000, &none()).map(|f| f.run_id),
            Some("run-a".to_string()),
            "the set-aside run waits for the next boot"
        );
    }

    #[test]
    fn a_degraded_ledger_is_one_line_and_is_not_asked_again_this_boot() {
        let root = tempfile::tempdir().expect("tempdir");
        note_failure(root.path(), &failure("run-a", 1_000)).expect("noted");
        let door = FakeDoor {
            down: true,
            ..FakeDoor::default()
        };
        let mut sweep = Sweep::new();
        let lines: Vec<String> = (0..5)
            .filter_map(|_| beat(&mut sweep, root.path(), 10_000, &door).1)
            .collect();
        assert_eq!(lines.len(), 1, "one line, not one per 1 s beat: {lines:?}");
        assert_eq!(
            lines[0],
            "qa task: not filed — the ledger is gone — the next boot asks again"
        );
        assert_eq!(
            door.asked.get(),
            1,
            "a degraded ledger is decided once per boot"
        );
    }

    #[test]
    fn a_filed_failure_leaves_the_pending_folder_so_no_beat_reads_it_again() {
        let root = tempfile::tempdir().expect("tempdir");
        note_failure(root.path(), &failure("run-a", 1_000)).expect("noted");
        let door = FakeDoor::default();
        let mut sweep = Sweep::new();
        let (said, _) = beat(&mut sweep, root.path(), 10_000, &door);
        assert_eq!(
            said,
            Triage::Filed {
                item: "run-a".into(),
                task: "t-7".into()
            }
        );
        assert!(
            !pending_file(root.path(), "run-a").exists(),
            "the stamp is the record; the pending file is read on every beat"
        );
        // The same run's `done` heard again is still the one task.
        note_failure(root.path(), &failure("run-a", 1_000)).expect("noted again");
        assert_eq!(
            beat(&mut sweep, root.path(), 10_000, &door),
            (Triage::Nothing, None)
        );
        assert_eq!(door.asked.get(), 1);
        assert!(
            !pending_file(root.path(), "run-a").exists(),
            "and its second pending file went on the way"
        );
    }

    /// What one beat's look at the folder costs once N failures were filed
    /// this week. 2026-09-12, while a filed failure's pending file stayed and
    /// was read again on every beat: 20 filed → 305 µs a beat, 100 → 1.49 ms;
    /// once it leaves with its stamp: 16 µs and 11 µs — flat in N.
    #[test]
    #[ignore = "measurement, not a rule"]
    fn measure_the_beats_look_at_the_folder() {
        for n in [20_usize, 100] {
            let root = tempfile::tempdir().expect("tempdir");
            let door = FakeDoor::default();
            let mut sweep = Sweep::new();
            for i in 0..n {
                let at_ms = 1_000 + i64::try_from(i).expect("small");
                note_failure(root.path(), &failure(&format!("run-{i:04}"), at_ms)).expect("noted");
                beat(&mut sweep, root.path(), 10_000, &door);
            }
            let beats = 2_000;
            let started = std::time::Instant::now();
            for _ in 0..beats {
                std::hint::black_box(beat(&mut sweep, root.path(), 10_000, &door));
            }
            let each = started.elapsed() / beats;
            println!("MEASURE filed={n} per_beat={each:?}");
        }
    }
}
