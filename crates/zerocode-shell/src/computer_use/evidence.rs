//! Where the operator's actions leave their evidence when no automation run
//! asked for a folder (docs/design/computer-use-full-operator.md §1.4): one
//! session folder per helper session under the window's data root, the same
//! step log and frames `run_evidence` writes for a run, pruned by age, its
//! frames capped so a long night of clicking does not fill a disk.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde_json::Value;

/// Under the window's local data root.
pub const SESSIONS_DIR: &str = "computer-use/sessions";
/// How long a session folder is kept, and how many frames one may hold.
pub const KEEP_DAYS: u64 = 7;
pub const MAX_FRAMES_PER_SESSION: usize = 500;
/// How many steps `evidence` answers with when not asked for a number.
pub const REPORT_STEPS: usize = 20;

/// The data root the window told us, and the session folder once it stood.
static ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);
static SESSION: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Tell the evidence store where the window keeps its data.
pub fn set_root(root: &Path) {
    let mut held = ROOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if held.as_deref() != Some(root) {
        *held = Some(root.to_path_buf());
    }
}

/// The data root the window told us, if it has.
#[must_use]
pub fn root() -> Option<PathBuf> {
    ROOT.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// `YYYYMMDD-HHMMSS` in UTC from milliseconds since the epoch — a folder
/// name a person can sort and read.
#[must_use]
pub fn stamp(epoch_ms: i64) -> String {
    let secs = epoch_ms.div_euclid(1_000);
    let days = secs.div_euclid(86_400);
    let of_day = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        of_day / 3_600,
        (of_day % 3_600) / 60,
        of_day % 60
    )
}

/// The session folder, made on first use (and old sessions pruned then).
pub fn session_dir(now_epoch_ms: i64) -> Option<PathBuf> {
    session_dir_born(now_epoch_ms).map(|(dir, _)| dir)
}

/// The session folder, and whether this call made it — the caller that
/// catalogues folders registers a new one once.
pub fn session_dir_born(now_epoch_ms: i64) -> Option<(PathBuf, bool)> {
    let root = ROOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    let mut held = SESSION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(dir) = held.as_ref().filter(|dir| dir.is_dir()) {
        return Some((dir.clone(), false));
    }
    let sessions = root.join(SESSIONS_DIR);
    std::fs::create_dir_all(&sessions).ok()?;
    prune(&sessions, SystemTime::now());
    let dir = sessions.join(format!("{}-{}", stamp(now_epoch_ms), std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    *held = Some(dir.clone());
    Some((dir, true))
}

/// The folder an arena walk (`recipe-run --arena`) leaves its evidence in:
/// beside the session folders, named by the moment and the process like
/// one, with the walk's kind as its suffix — never the session's own, so a
/// rehearsal's lines never sit among a desk's — and pruned with them.
pub fn arena_dir(now_epoch_ms: i64) -> Option<PathBuf> {
    let root = ROOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()?;
    let sessions = root.join(SESSIONS_DIR);
    std::fs::create_dir_all(&sessions).ok()?;
    let dir = sessions.join(format!(
        "{}-{}-{}",
        stamp(now_epoch_ms),
        std::process::id(),
        super::arena::WALK_KIND
    ));
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// The verdict file a QA scenario leaves beside its steps (§4).
pub const VERDICT_FILE: &str = "qa-verdict.json";

/// Write the scenario's verdict into its evidence folder, and a step that
/// says the same (a failing verdict is a failed step, with its reason).
pub fn write_verdict(
    dir: &Path,
    argv: &[String],
    pass: bool,
    reason: Option<&str>,
    at_epoch_ms: i64,
) -> Result<PathBuf, String> {
    let file = dir.join(VERDICT_FILE);
    let body = serde_json::json!({ "pass": pass, "reason": reason, "atEpochMs": at_epoch_ms });
    std::fs::write(
        &file,
        serde_json::to_vec_pretty(&body).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("could not write {}: {error}", file.display()))?;
    let outcome = if pass {
        Ok(())
    } else {
        Err(reason.unwrap_or("failed"))
    };
    crate::run_evidence::record(
        dir,
        at_epoch_ms,
        "computer",
        argv,
        outcome,
        crate::run_evidence::Framing::None,
    );
    Ok(file)
}

/// The verdict a folder holds, if one was left: `(pass, reason)`.
#[must_use]
pub fn read_verdict(dir: &Path) -> Option<(bool, Option<String>)> {
    let body: Value = serde_json::from_slice(&std::fs::read(dir.join(VERDICT_FILE)).ok()?).ok()?;
    Some((
        body.get("pass").and_then(Value::as_bool)?,
        body.get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    ))
}

/// Forget the session folder; the next action starts a new one.
pub fn end_session() {
    *SESSION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

/// Remove session folders untouched for longer than the table. Best effort.
pub fn prune(sessions: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(sessions) else {
        return;
    };
    let keep = Duration::from_secs(KEEP_DAYS * 86_400);
    for entry in entries.flatten() {
        let path = entry.path();
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > keep);
        if path.is_dir() && old {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Whether one more frame fits under the table's cap.
#[must_use]
pub fn frame_allowed(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
                .count()
                < MAX_FRAMES_PER_SESSION
        })
        .unwrap_or(true)
}

/// What `evidence` answers: the folder, its last steps, and its size.
#[must_use]
pub fn report(dir: Option<&Path>, last: usize) -> Value {
    let Some(dir) = dir else {
        return serde_json::json!({ "dir": Value::Null, "steps": [], "count": 0, "frames": 0 });
    };
    let steps = crate::run_evidence::steps_in(dir);
    let frames = steps.iter().filter(|step| step.shot.is_some()).count();
    let tail: Vec<&crate::run_evidence::Step> = steps
        .iter()
        .rev()
        .take(last)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    serde_json::json!({
        "dir": dir.display().to_string(),
        "count": steps.len(),
        "frames": frames,
        "frameCap": MAX_FRAMES_PER_SESSION,
        "keepDays": KEEP_DAYS,
        "steps": tail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_reads_as_a_utc_date_and_time() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_788_879_792_000), "20260908-150312");
        assert_eq!(stamp(951_782_400_000), "20000229-000000", "a leap day");
    }

    #[test]
    fn old_sessions_are_pruned_and_the_frame_cap_holds() {
        let root = tempfile::tempdir().expect("tempdir");
        let sessions = root.path().join(SESSIONS_DIR);
        let old = sessions.join("20200101-000000-1");
        let fresh = sessions.join("20260908-000000-2");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&fresh).unwrap();
        let ancient = SystemTime::UNIX_EPOCH + Duration::from_secs(1_577_836_800);
        std::fs::File::open(&old)
            .unwrap()
            .set_modified(ancient)
            .unwrap();
        prune(&sessions, SystemTime::now());
        assert!(!old.exists(), "a session older than the table is gone");
        assert!(fresh.exists(), "a fresh session stays");

        assert!(frame_allowed(&fresh));
        for n in 0..MAX_FRAMES_PER_SESSION {
            std::fs::write(fresh.join(format!("{n:03}.png")), b"x").unwrap();
        }
        assert!(!frame_allowed(&fresh), "the cap is the table's number");
    }

    #[test]
    fn a_verdict_is_a_file_and_a_step_and_reads_back() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().to_path_buf();
        let argv = vec![
            "verdict".to_string(),
            "--fail".to_string(),
            "--reason".to_string(),
            "no cart".to_string(),
        ];
        let file = write_verdict(&dir, &argv, false, Some("no cart"), 1_000).expect("written");
        assert!(file.ends_with(VERDICT_FILE));
        assert_eq!(
            read_verdict(&dir),
            Some((false, Some("no cart".to_string())))
        );
        let steps = crate::run_evidence::steps_in(&dir);
        assert_eq!(steps.len(), 1);
        assert!(
            !steps[0].ok && steps[0].error.as_deref() == Some("no cart"),
            "a failing verdict is a failed step"
        );
        write_verdict(
            &dir,
            &["verdict".to_string(), "--pass".to_string()],
            true,
            None,
            2_000,
        )
        .expect("written");
        assert_eq!(read_verdict(&dir), Some((true, None)));
        assert_eq!(read_verdict(root.path().join("nowhere").as_path()), None);
    }

    #[test]
    fn a_report_names_the_folder_and_its_last_steps() {
        let root = tempfile::tempdir().expect("tempdir");
        let dir = root.path().to_path_buf();
        for n in 0..3 {
            crate::run_evidence::record(
                &dir,
                1_000 + n,
                "computer",
                &["mouse-click".to_string()],
                Ok(()),
                crate::run_evidence::Framing::None,
            );
        }
        let answer = report(Some(&dir), 2);
        assert_eq!(answer["count"], 3);
        assert_eq!(answer["steps"].as_array().map(Vec::len), Some(2));
        assert_eq!(answer["steps"][1]["n"], 3, "the last steps, in order");
        assert_eq!(report(None, 5)["count"], 0);
    }
}
