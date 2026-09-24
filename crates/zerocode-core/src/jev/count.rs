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

use std::fs::{self, OpenOptions};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};

/// The directory under zo's config home the day files live in.
pub const REQUESTS_DIR: &str = "jev";

/// What a day file's name starts and ends with, around the local date.
const DAY_FILE_PREFIX: &str = "requests-";
const DAY_FILE_SUFFIX: &str = ".count";

/// The byte one request is counted with.
const ONE_REQUEST: &[u8] = b".";

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
    let first_of_the_day = !path.exists();
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(ONE_REQUEST)?;
    let place = file.stream_position()?;
    if first_of_the_day {
        forget_earlier_days(path, DAY_FILE_PREFIX, DAY_FILE_SUFFIX, |_, _| false);
    }
    Ok(place)
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
