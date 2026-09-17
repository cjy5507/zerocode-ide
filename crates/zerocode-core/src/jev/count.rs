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
/// including it. The first request of a day clears the files of earlier days.
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
        forget_other_days(path);
    }
    Ok(place)
}

/// Remove every day file beside `today` but `today` — best effort: a file
/// another program is still counting in comes back with its next request.
fn forget_other_days(today: &Path) {
    let (Some(dir), Some(name)) = (today.parent(), today.file_name()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let other = entry.file_name();
        let other = other.to_string_lossy();
        if other != name.to_string_lossy()
            && other.starts_with(DAY_FILE_PREFIX)
            && other.ends_with(DAY_FILE_SUFFIX)
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}
