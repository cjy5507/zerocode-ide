//! How many Jev requests this machine has sent in a local day — the count the
//! door's budget reads (`smart.jev.dailyRequests`), shared by every zo and the
//! window.
//!
//! One file per day under zo's config home, one byte per request. A byte
//! rather than a line: the day's count is the file's length, read without
//! opening it, and one request is one `O_APPEND` write of a single byte, which
//! concurrent writers cannot interleave. The position a write leaves is that
//! request's own place in the day, so two programs counting at the same
//! moment each learn which of them took the last place.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

/// The directory under zo's config home the day files live in.
pub const REQUESTS_DIR: &str = "jev";

/// What a day file's name starts and ends with, around the local date.
const DAY_FILE_PREFIX: &str = "requests-";
const DAY_FILE_SUFFIX: &str = ".count";

/// The byte one request is counted with.
const ONE_REQUEST: &[u8] = b".";

/// What an hour file's name starts with, before its local hour and its key.
const HOUR_FILE_PREFIX: &str = "hour-";

/// The lock beside every hour file, shared across decisions and hours.
pub(super) const HOURS_LOCK_FILE: &str = ".hours.lock";

/// The maximum wait and retry interval for an hour writer, as in the decisions store.
const HOURS_LOCK_WAIT: Duration = Duration::from_millis(25);
const HOURS_LOCK_POLL: Duration = Duration::from_millis(1);

/// The hour lock is readable and writable by its owner alone.
#[cfg(unix)]
const HOURS_LOCK_MODE: u32 = 0o600;

/// The local hour an hour file's name holds, in the shape [`hour_of`] writes.
const HOUR_SHAPE: &str = "YYYY-MM-DDTHH";

/// One minute, in milliseconds: the unit a clock's offset is given in.
const MS_PER_MINUTE: i64 = 60_000;

/// The day file for `day` (`YYYY-MM-DD`, local) under `config_home`.
#[must_use]
pub fn requests_path(config_home: &Path, day: &str) -> PathBuf {
    config_home
        .join(REQUESTS_DIR)
        .join(format!("{DAY_FILE_PREFIX}{day}{DAY_FILE_SUFFIX}"))
}

/// The local date `epoch_ms` falls on, for a clock `offset_minutes` ahead of
/// UTC — the day a request is counted in.
#[must_use]
pub fn day_of(epoch_ms: i64, offset_minutes: i32) -> String {
    crate::civil::iso_date_of(epoch_ms, offset_minutes)
}

/// Requests counted in `path`: zero when the day has counted none.
#[must_use]
pub fn sent(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |meta| meta.len())
}

/// Count one request in `path` and answer its place in the day — the count
/// including it. The first request of a day clears the files of earlier days
/// ([`forget_earlier_days`]).
///
/// # Errors
/// A day file that cannot be created or written.
pub fn count_one(path: &Path) -> io::Result<u64> {
    let (place, first_of_the_day) = append_one(path)?;
    if first_of_the_day {
        forget_earlier_days(path, DAY_FILE_PREFIX, DAY_FILE_SUFFIX, |_, _| false);
    }
    Ok(place)
}

/// The local hour `epoch_ms` falls in, as `YYYY-MM-DDTHH`, for a clock
/// `offset_minutes` ahead of UTC: the hour a person's decision counts its
/// requests in (`super::custom::HOURLY_REQUESTS`).
#[must_use]
pub fn hour_of(epoch_ms: i64, offset_minutes: i32) -> String {
    let local_ms = epoch_ms + i64::from(offset_minutes) * MS_PER_MINUTE;
    let hour = local_ms.rem_euclid(crate::civil::MS_PER_DAY) / crate::civil::MS_PER_HOUR;
    format!("{}T{hour:02}", day_of(epoch_ms, offset_minutes))
}

/// The hour file for `key` in `hour` (`YYYY-MM-DDTHH`, local) under
/// `config_home`: `key` is a decision's id, or the key every decision is
/// counted under together (`super::custom::EVERY_DECISION`).
#[must_use]
pub fn hour_path(config_home: &Path, key: &str, hour: &str) -> PathBuf {
    config_home
        .join(REQUESTS_DIR)
        .join(format!("{HOUR_FILE_PREFIX}{hour}-{key}{DAY_FILE_SUFFIX}"))
}

/// Exclude every other hour writer until the returned file is dropped.
/// A lock that stays busy past [`HOURS_LOCK_WAIT`] returns `WouldBlock`.
pub(super) fn lock_hours(every: &Path) -> io::Result<File> {
    let path = every.with_file_name(HOURS_LOCK_FILE);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(HOURS_LOCK_MODE);
    }
    let file = options.open(path)?;
    let until = Instant::now() + HOURS_LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < until => {
                thread::sleep(HOURS_LOCK_POLL);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "another writer holds the hourly counts",
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// An hour's count, with an unreadable count refused instead of read as zero.
pub(super) fn sent_in_the_hour(path: &Path) -> io::Result<u64> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(error),
    }
}

/// Count one request in `path`, an hour file, and answer its place in the
/// hour. The first request a file counts clears the hour files of earlier
/// hours, whatever their key — never its own hour's nor a later one's, for
/// the reason the day's count keeps today's ([`forget_earlier_days`]).
///
/// # Errors
/// An hour file that cannot be created or written.
pub fn count_one_in_the_hour(path: &Path) -> io::Result<u64> {
    let (place, first) = append_one(path)?;
    if first {
        forget_earlier_hours(path);
    }
    Ok(place)
}

/// Return an hour file to `before` after a refused reservation. The caller
/// must exclude other hour writers until the reservation is kept or returned.
/// A failed return leaves the count high and must never allow the request.
pub(super) fn return_hour_to(path: &Path, before: u64) -> io::Result<()> {
    let file = match OpenOptions::new().write(true).open(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound && before == 0 => return Ok(()),
        result => result?,
    };
    if file.metadata()?.len() > before {
        file.set_len(before)?;
    }
    Ok(())
}

/// Append one request's byte to `path`: the place it took, and whether it
/// was the file's first.
fn append_one(path: &Path) -> io::Result<(u64, bool)> {
    let first = !path.exists();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(ONE_REQUEST)?;
    Ok((file.stream_position()?, first))
}

/// The day before `day` (`YYYY-MM-DD`), or `None` for a word that is not a
/// date. Calendar labels, so no clock is read: the day before a date is the
/// same on every machine.
#[must_use]
pub fn day_before(day: &str) -> Option<String> {
    let midnight = crate::civil::epoch_ms_of_iso(&format!("{day}T00:00:00Z"))?;
    Some(crate::civil::iso_date_of(midnight - 86_400_000, 0))
}

/// The day a day file names — the `YYYY-MM-DD` between `prefix` and
/// `suffix` — or `None` for any other name.
#[must_use]
pub fn day_named(path: &Path, prefix: &str, suffix: &str) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let day = name.strip_prefix(prefix)?.strip_suffix(suffix)?;
    day_before(day).map(|_| day.to_string())
}

/// Remove the day files beside `today` — named `prefix<day>suffix` — whose
/// day is EARLIER than `today`'s own and that `keep` does not hold on to.
///
/// Never `today`'s file and never a later day's. A program that took its day
/// before midnight and writes after it — a request cleared at 23:59:59 and
/// counted at 00:00:01, a reservation read on one side of midnight and
/// written on the other — writes into a day that has already ended, and its
/// first write there must not wipe the day that has begun: that wipe took
/// the new day's reservations and count with it, and the next reader saw a
/// share nobody had spent (t-6263). A name whose day does not read as a date
/// is kept: what this cannot place, it cannot call earlier. `keep` is the
/// file's own rule for a past day it may still need (the challenger arm's
/// book, while a reservation charged before midnight is still out); the
/// count keeps none.
///
/// Best effort: a file another program is still writing in comes back with
/// its next line. Shared by every per-day file in the door's folder (this
/// count, the challenger arm's spend book), so the rule for what a past day
/// leaves behind is written once.
pub fn forget_earlier_days(
    today: &Path,
    prefix: &str,
    suffix: &str,
    keep: impl Fn(&str, &Path) -> bool,
) {
    let (Some(dir), Some(today_is)) = (today.parent(), day_named(today, prefix, suffix)) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(day) = day_named(&path, prefix, suffix) else {
            continue;
        };
        // `YYYY-MM-DD` orders as the calendar does.
        if day < today_is && !keep(&day, &path) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Remove the hour files beside `this_hour` whose hour is earlier than its
/// own. A name whose hour does not read is kept, as a day file's is.
fn forget_earlier_hours(this_hour: &Path) {
    let (Some(dir), Some(now)) = (this_hour.parent(), hour_named(this_hour)) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `YYYY-MM-DDTHH` orders as the clock does.
        if hour_named(&path).is_some_and(|hour| hour < now) {
            let _ = fs::remove_file(&path);
        }
    }
}

/// The hour an hour file names — the `YYYY-MM-DDTHH` after its prefix,
/// before a dash and its key — or `None` for any other name.
fn hour_named(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let rest = name
        .strip_prefix(HOUR_FILE_PREFIX)?
        .strip_suffix(DAY_FILE_SUFFIX)?;
    let (hour, key) = rest.split_at_checked(HOUR_SHAPE.len())?;
    let at = crate::civil::epoch_ms_of_iso(&format!("{hour}:00:00Z"))?;
    (hour_of(at, 0) == hour && key.strip_prefix('-').is_some_and(|key| !key.is_empty()))
        .then(|| hour.to_string())
}
