//! The wakeup records `ScheduleWakeup` writes and the session scheduler reads.
//!
//! One file per call under `<state>/.zo/wakeups/<id>.json`, where `<state>` is
//! [`runtime::zo_state_base`] of the working directory. The tool is the only
//! writer and [`take_for_session`] the only reader: a record is consumed (its
//! file removed) the moment the owning session folds it into its loop
//! registry, so a wakeup can neither fire twice nor outlive a `stop`.
//!
//! The record is deliberately plain: what the model asked for, when, and for
//! which session. Clamping the delay to the autonomy limits and deciding when
//! the turn opens belong to the session's one scheduler, not to this file.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Directory name under `<state>/.zo/`.
pub const DIR_NAME: &str = "wakeups";
const ID_PREFIX: &str = "wakeup-";

/// One `ScheduleWakeup` call, as written to disk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WakeupRecord {
    /// Seconds the model asked to wait. Zero on a `stop` record.
    #[serde(rename = "delaySeconds", default)]
    pub delay_seconds: f64,
    #[serde(default)]
    pub reason: String,
    /// The prompt the wakeup turn opens with. Empty on a `stop` record.
    #[serde(default)]
    pub prompt: String,
    /// Unix seconds when the call was made — kept as a string, the shape the
    /// tool's receipt has carried since it first shipped.
    #[serde(rename = "scheduledAt")]
    pub scheduled_at: String,
    /// The session whose scheduler owns this record. Absent on an untracked
    /// host, and then nothing fires it.
    #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The model judged the tick that scheduled this as quiet.
    #[serde(default)]
    pub noop: bool,
    /// End the loop: cancel the pending wakeup, schedule nothing.
    #[serde(default)]
    pub stop: bool,
}

impl WakeupRecord {
    /// `scheduledAt` as unix seconds; zero when the field is not a number.
    #[must_use]
    pub fn scheduled_at_secs(&self) -> u64 {
        self.scheduled_at.trim().parse().unwrap_or(0)
    }

    /// Whole seconds of delay; a fractional request rounds to the nearest
    /// second and a negative one to zero.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is clamped to a non-negative whole number of seconds before the cast"
    )]
    pub fn delay_secs(&self) -> u64 {
        self.delay_seconds.max(0.0).round() as u64
    }
}

/// A record just written: its id and the file that holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub id: String,
    pub path: PathBuf,
}

/// Where a working directory's wakeup records live.
#[must_use]
pub fn dir(cwd: &Path) -> PathBuf {
    runtime::zo_state_base(cwd).join(".zo").join(DIR_NAME)
}

/// Write one record. Ids carry the write's nanosecond clock zero-padded, so
/// a plain name sort is the order the calls were made in.
pub fn write(cwd: &Path, record: &WakeupRecord) -> io::Result<Written> {
    let dir = dir(cwd);
    std::fs::create_dir_all(&dir)?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = format!("{ID_PREFIX}{nanos:019}");
    let path = dir.join(format!("{id}.json"));
    std::fs::write(&path, serde_json::to_vec_pretty(record)?)?;
    Ok(Written { id, path })
}

/// Consume every record written for `session_id`, oldest first. Each returned
/// record's file is gone; records that name another session, or none, stay.
#[must_use]
pub fn take_for_session(cwd: &Path, session_id: &str) -> Vec<WakeupRecord> {
    let Ok(entries) = std::fs::read_dir(dir(cwd)) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "json")
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(ID_PREFIX))
        })
        .collect();
    paths.sort();
    let mut taken = Vec::new();
    for path in paths {
        let Some(record) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<WakeupRecord>(&bytes).ok())
        else {
            continue;
        };
        if record.session_id.as_deref() != Some(session_id) {
            continue;
        }
        if std::fs::remove_file(&path).is_ok() {
            taken.push(record);
        }
    }
    taken
}

#[cfg(test)]
mod tests {
    use super::{take_for_session, write, WakeupRecord};

    fn record(session: Option<&str>, prompt: &str) -> WakeupRecord {
        WakeupRecord {
            delay_seconds: 45.4,
            reason: "poll".to_string(),
            prompt: prompt.to_string(),
            scheduled_at: "1700000000".to_string(),
            session_id: session.map(str::to_string),
            noop: false,
            stop: false,
        }
    }

    #[test]
    fn a_session_takes_its_own_records_in_write_order_and_leaves_the_rest() {
        let _state = crate::tests::EnvGuard::clear(core_types::paths::ZO_STATE_DIR_ENV);
        let root = tempfile::tempdir().expect("tempdir");
        let cwd = root.path().join("ws");
        std::fs::create_dir_all(&cwd).expect("cwd");
        // Exercise the temporary cwd even after another test or the shell
        // selected an operational state directory. The scoped guard restores it.
        let base = super::dir(&cwd);
        write(&cwd, &record(Some("s1"), "first")).expect("write");
        write(&cwd, &record(Some("s2"), "other session")).expect("write");
        write(&cwd, &record(None, "unowned")).expect("write");
        write(&cwd, &record(Some("s1"), "second")).expect("write");

        let taken = take_for_session(&cwd, "s1");

        assert_eq!(
            taken.iter().map(|record| record.prompt.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
        assert_eq!(taken[0].delay_secs(), 45);
        assert_eq!(taken[0].scheduled_at_secs(), 1_700_000_000);
        let left = std::fs::read_dir(&base).expect("dir").count();
        assert_eq!(left, 2, "records of other sessions and unowned ones stay");
        assert!(take_for_session(&cwd, "s1").is_empty(), "a record is consumed once");
    }
}
