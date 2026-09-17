//! Minimal calendar-date helpers, dependency-free by design.
//!
//! Single owner of the civil-from-days conversion (Howard Hinnant's
//! `civil_from_days` algorithm), of "today as `YYYY-MM-DD`", and of the
//! local zone — its offset and its name. The CLI
//! system prompt, sub-agent prompts, and the usage dashboard all format
//! dates from here so they cannot drift apart — and so nobody is tempted
//! to hardcode a "today" constant again (the prompt date was frozen at
//! `2026-03-31` for months because of exactly that).
//!
//! `build.rs` keeps its own private copy of the algorithm: build scripts
//! cannot depend on workspace crates.

/// Current **local** date as `YYYY-MM-DD` — what a prompt should call
/// "today" (Claude Code parity: the env block shows the user's local date;
/// a KST user's morning is still "yesterday" in UTC until 09:00).
///
/// `std` exposes no timezone database and the workspace forbids `unsafe`
/// (`libc::localtime_r`) and heavy date deps, so this consults the POSIX
/// `date` utility (`%F` = `YYYY-MM-DD`, locale-independent) and falls back
/// to the UTC date when the utility is unavailable (non-POSIX platform) or
/// prints something that is not a plausible date.
#[must_use]
pub fn current_local_date() -> String {
    local_date_from_date_utility().unwrap_or_else(current_utc_date)
}

fn local_date_from_date_utility() -> Option<String> {
    let output = std::process::Command::new("date").arg("+%F").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let candidate = text.trim();
    let shape_ok = candidate.len() == 10
        && candidate.bytes().enumerate().all(|(index, byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if shape_ok {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Seconds the local zone sits ahead of UTC, e.g. `32_400` for KST.
///
/// Same constraint as [`current_local_date`]: no timezone database in `std`, no
/// `unsafe`, no heavy date dependency — so the POSIX `date` utility answers it.
/// Cached for the process because a render path asks on every frame and forking
/// `date` that often would cost more than the label is worth. A zone that
/// changes mid-session (a DST boundary) is off by an hour until restart, which
/// is a fair trade for not forking per frame.
#[must_use]
pub fn local_utc_offset_secs() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| offset_from_date_utility().unwrap_or(0))
}

fn offset_from_date_utility() -> Option<i64> {
    let output = std::process::Command::new("date").arg("+%z").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let raw = text.trim();
    // `+0900` / `-0500`: sign, two hour digits, two minute digits.
    if raw.len() != 5 || !raw[1..].bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let sign = match raw.as_bytes()[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours: i64 = raw.get(1..3)?.parse().ok()?;
    let minutes: i64 = raw.get(3..5)?.parse().ok()?;
    if hours > 14 || minutes >= 60 {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// The local zone's IANA name, e.g. `Asia/Seoul` — the label a receipt prints
/// beside a wall-clock schedule so a person can check it against their own
/// clock. Falls back to `UTC±HH:MM` built from [`local_utc_offset_secs`], which
/// is always answerable, when the host names no zone.
///
/// Read from the system, in the order POSIX itself resolves it: `TZ` when it
/// names a zone (`date` honours the same variable, so label and offset cannot
/// disagree), then the `/etc/localtime` symlink every zoneinfo install leaves,
/// then Debian's `/etc/timezone`. Cached for the process for the same reason
/// [`local_utc_offset_secs`] is.
#[must_use]
pub fn local_time_zone_label() -> &'static str {
    static LABEL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LABEL.get_or_init(|| {
        zone_name_from_tz_env()
            .or_else(zone_name_from_localtime_link)
            .or_else(zone_name_from_timezone_file)
            .unwrap_or_else(|| utc_offset_label(local_utc_offset_secs()))
    })
}

fn zone_name_from_tz_env() -> Option<String> {
    let raw = std::env::var("TZ").ok()?;
    // A leading ':' is the POSIX spelling of "this is a zone name, not a rule".
    zone_name_if_iana(raw.strip_prefix(':').unwrap_or(&raw))
}

fn zone_name_from_localtime_link() -> Option<String> {
    let target = std::fs::read_link("/etc/localtime").ok()?;
    let text = target.to_str()?;
    // macOS points at /var/db/timezone/zoneinfo/…, Linux at /usr/share/zoneinfo/…
    zone_name_if_iana(text.rsplit_once("zoneinfo/")?.1)
}

fn zone_name_from_timezone_file() -> Option<String> {
    let text = std::fs::read_to_string("/etc/timezone").ok()?;
    zone_name_if_iana(text.lines().next()?)
}

/// An IANA zone name is `Area/Location` (`Asia/Seoul`, `America/Argentina/Salta`)
/// or the bare `UTC`. A POSIX rule string (`KST-9`, `EST5EDT`) is not a name and
/// falls through to the numeric label, which says the same thing without lying.
fn zone_name_if_iana(candidate: &str) -> Option<String> {
    let name = candidate.trim().trim_matches('/');
    if name.is_empty() || name.len() > 64 {
        return None;
    }
    let shaped = name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b'+'));
    if !shaped {
        return None;
    }
    if name == "UTC" || name.contains('/') {
        Some(name.to_string())
    } else {
        None
    }
}

/// `UTC+09:00` / `UTC-03:30` — the zone said as the only thing that is certain
/// about it when nothing named it.
fn utc_offset_label(offset_secs: i64) -> String {
    let sign = if offset_secs < 0 { '-' } else { '+' };
    let magnitude = offset_secs.abs();
    format!("UTC{sign}{:02}:{:02}", magnitude / 3_600, (magnitude % 3_600) / 60)
}

/// A reset instant as the local weekday and clock time, e.g. `Sun 11:00`.
///
/// A countdown ("5d") cannot be checked against what a user already knows — the
/// weekly window rolls over at a wall-clock time they remember, and in their own
/// zone, not UTC. This is the form that can be compared.
#[must_use]
pub fn local_weekday_clock(unix_secs: i64) -> String {
    /// Sunday-first, matching [`local_weekday_clock`]'s epoch arithmetic below.
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

    let local = unix_secs + local_utc_offset_secs();
    let days = local.div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    // The Unix epoch was a Thursday.
    let weekday = WEEKDAYS[usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0)];
    format!("{weekday} {:02}:{:02}", seconds / 3_600, (seconds % 3_600) / 60)
}

/// Current UTC date as `YYYY-MM-DD`, from the system clock.
///
/// Clock-before-epoch (or otherwise unreadable) degrades to the epoch date
/// rather than panicking: prompt assembly must never abort over a bad clock.
#[must_use]
pub fn current_utc_date() -> String {
    utc_date_from_unix_secs(current_unix_secs())
}

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// UTC date as `YYYY-MM-DD` for a Unix timestamp in seconds.
#[must_use]
pub fn utc_date_from_unix_secs(secs: u64) -> String {
    let days = i64::try_from(secs / 86_400).unwrap_or(i64::MAX);
    let (year, month, day) = civil_from_unix_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Civil (proleptic Gregorian) `(year, month, day)` from days since the Unix
/// epoch (1970-01-01). Howard Hinnant's `civil_from_days`. Inputs beyond the
/// representable range saturate (the epoch-shift addition and the year both
/// clamp) instead of overflowing or wrapping.
#[must_use]
pub fn civil_from_unix_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch.saturating_add(719_468);
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(if year.is_negative() { i32::MIN } else { i32::MAX }),
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

/// Days since the Unix epoch (1970-01-01) for a civil (proleptic Gregorian)
/// `(year, month, day)` — Howard Hinnant's `days_from_civil`, the exact inverse
/// of [`civil_from_unix_days`].
///
/// This is what puts a date on a weekday, which a calendar grid needs and a
/// `YYYY-MM-DD` string alone cannot give: the epoch was a Thursday, so
/// `(days + 4).rem_euclid(7)` is the weekday with Sunday at zero.
#[must_use]
pub fn unix_days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let shifted_year = i64::from(year) - i64::from(month <= 2);
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let month = i64::from(month);
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Unix seconds for an RFC 3339 timestamp, or `None` when the text is not one.
///
/// Provider usage endpoints date their reset instants this way
/// (`2026-08-09T02:00:00.447469+00:00`), and a quota window is only actionable
/// with the clock time it rolls over. Fractional seconds are dropped: a window
/// resetting within the same second is not a distinction any caller can act on.
#[must_use]
pub fn unix_secs_from_rfc3339(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once(['T', 't'])?;
    let days = unix_days_from_date_label(date)?;

    // The offset sign is the only '-' that can appear after the date, so
    // searching from the right cannot catch a date dash.
    let split_at = rest
        .find(['Z', 'z', '+'])
        .or_else(|| rest.rfind('-'))
        .unwrap_or(rest.len());
    let (clock, offset) = rest.split_at(split_at);

    let mut parts = clock.split(':');
    let hours: i64 = parts.next()?.parse().ok()?;
    let minutes: i64 = parts.next().unwrap_or("0").parse().ok()?;
    let seconds: i64 = parts
        .next()
        .unwrap_or("0")
        .split('.')
        .next()
        .unwrap_or("0")
        .parse()
        .ok()?;
    // A leap second is a real 60; anything past that is a malformed stamp.
    if !(0..24).contains(&hours) || !(0..60).contains(&minutes) || !(0..=60).contains(&seconds) {
        return None;
    }

    let local = days * 86_400 + hours * 3_600 + minutes * 60 + seconds;
    Some(local - rfc3339_offset_secs(offset)?)
}

/// Seconds an RFC 3339 offset suffix (`Z`, `+09:00`, `-05:00`) sits ahead of UTC.
fn rfc3339_offset_secs(offset: &str) -> Option<i64> {
    if offset.is_empty() || offset.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let (sign, rest) = offset.split_at(1);
    let sign = match sign {
        "+" => 1,
        "-" => -1,
        _ => return None,
    };
    let (hours, minutes) = rest.split_once(':').unwrap_or((rest, "0"));
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    if !(0..=14).contains(&hours) || !(0..60).contains(&minutes) {
        return None;
    }
    Some(sign * (hours * 3_600 + minutes * 60))
}

/// Days since the Unix epoch for a `YYYY-MM-DD` label, or `None` when the text
/// is not that shape.
///
/// The usage dashboard's daily labels are produced by
/// [`utc_date_from_unix_secs`], so this is the trip back — a monthly `YYYY-MM`
/// label is deliberately rejected rather than guessed at.
#[must_use]
pub fn unix_days_from_date_label(label: &str) -> Option<i64> {
    let bytes = label.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i32 = label.get(0..4)?.parse().ok()?;
    let month: u32 = label.get(5..7)?.parse().ok()?;
    let day: u32 = label.get(8..10)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(unix_days_from_civil(year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inverse has to be exact, or a calendar grid puts days in the wrong
    /// column and the whole picture lies about which weekday was busy.
    #[test]
    fn unix_days_invert_civil_and_land_on_the_right_weekday() {
        for label in ["1970-01-01", "2000-02-29", "2025-07-05", "2026-08-03"] {
            let days = unix_days_from_date_label(label).expect("a well-formed label parses");
            let secs = u64::try_from(days).expect("post-epoch") * 86_400;
            assert_eq!(utc_date_from_unix_secs(secs), label, "round trip {label}");
        }
        assert_eq!(unix_days_from_date_label("1970-01-01"), Some(0));

        // The epoch was a Thursday; 2026-08-03 is a Monday. Sunday is zero.
        let epoch = unix_days_from_date_label("1970-01-01").expect("parses");
        assert_eq!((epoch + 4).rem_euclid(7), 4, "1970-01-01 was a Thursday");
        let monday = unix_days_from_date_label("2026-08-03").expect("parses");
        assert_eq!((monday + 4).rem_euclid(7), 1, "2026-08-03 is a Monday");

        // A monthly label is not a day and must not be guessed at.
        assert_eq!(unix_days_from_date_label("2026-08"), None);
        assert_eq!(unix_days_from_date_label("not-a-date"), None);
        assert_eq!(unix_days_from_date_label("2026-13-01"), None);
    }

    /// Shapes the provider usage endpoints actually return, plus the offset
    /// handling that decides whether a reset reads hours early or late.
    #[test]
    fn rfc3339_stamps_convert_to_unix_seconds() {
        // The exact shape Anthropic's usage endpoint returns.
        let anthropic = unix_secs_from_rfc3339("2026-08-09T02:00:00.447469+00:00")
            .expect("a fractional-second UTC stamp parses");
        assert_eq!(anthropic, 1_786_240_800);
        assert_eq!(unix_secs_from_rfc3339("1970-01-01T00:00:00Z"), Some(0));

        // An offset is subtracted, not added: 09:00+09:00 is midnight UTC.
        assert_eq!(
            unix_secs_from_rfc3339("2026-08-09T09:00:00+09:00"),
            unix_secs_from_rfc3339("2026-08-09T00:00:00Z")
        );
        assert_eq!(
            unix_secs_from_rfc3339("2026-08-08T19:00:00-05:00"),
            unix_secs_from_rfc3339("2026-08-09T00:00:00Z")
        );

        assert_eq!(unix_secs_from_rfc3339("2026-08-09"), None);
        assert_eq!(unix_secs_from_rfc3339("2026-08-09T25:00:00Z"), None);
        assert_eq!(unix_secs_from_rfc3339("not a timestamp"), None);
    }

    /// The live Anthropic reset, checked against what a KST user sees on their
    /// own clock — the whole point of the label is that it can be compared to
    /// what they already know.
    #[test]
    fn a_reset_instant_reads_as_a_local_weekday_and_clock() {
        // 2026-08-09T02:00:00Z, the seven-day window's actual rollover.
        let reset = unix_secs_from_rfc3339("2026-08-09T02:00:00+00:00").expect("parses");
        assert_eq!(reset, 1_786_240_800);

        // Rendered against a fixed offset rather than the host's, so the
        // assertion holds wherever this runs.
        let kst = reset + 9 * 3_600;
        let days = kst.div_euclid(86_400);
        assert_eq!((days + 4).rem_euclid(7), 0, "that instant is a Sunday in KST");
        assert_eq!(kst.rem_euclid(86_400) / 3_600, 11, "at 11:00 KST");

        // UTC hosts get the same instant, named in their own zone.
        assert_eq!(local_weekday_clock(0), {
            let offset = local_utc_offset_secs();
            let days = offset.div_euclid(86_400);
            let secs = offset.rem_euclid(86_400);
            let names = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
            format!(
                "{} {:02}:{:02}",
                names[usize::try_from((days + 4).rem_euclid(7)).unwrap_or(0)],
                secs / 3_600,
                (secs % 3_600) / 60
            )
        });
    }

    /// The label is what a receipt prints beside a wall-clock schedule, so it
    /// must name a zone a person recognises — and must still answer something
    /// true on a host that names none.
    #[test]
    fn the_zone_label_is_an_iana_name_or_the_numeric_offset() {
        assert_eq!(zone_name_if_iana("Asia/Seoul").as_deref(), Some("Asia/Seoul"));
        assert_eq!(zone_name_if_iana(":Asia/Seoul").as_deref(), None, "the colon is stripped before this");
        assert_eq!(
            zone_name_if_iana("/var/db/timezone/zoneinfo/America/Argentina/Salta".rsplit_once("zoneinfo/").unwrap().1)
                .as_deref(),
            Some("America/Argentina/Salta")
        );
        assert_eq!(zone_name_if_iana("UTC").as_deref(), Some("UTC"));
        // POSIX rule strings are not names; they fall through to the number.
        assert_eq!(zone_name_if_iana("Etc/GMT+9").as_deref(), Some("Etc/GMT+9"));
        assert_eq!(zone_name_if_iana("KST-9"), None);
        assert_eq!(zone_name_if_iana("EST5EDT"), None);
        assert_eq!(zone_name_if_iana(""), None);

        assert_eq!(utc_offset_label(32_400), "UTC+09:00");
        assert_eq!(utc_offset_label(-12_600), "UTC-03:30");
        assert_eq!(utc_offset_label(0), "UTC+00:00");

        // The live host always answers one shape or the other.
        let live = local_time_zone_label();
        assert!(
            live.contains('/') || live == "UTC" || live.starts_with("UTC+") || live.starts_with("UTC-"),
            "unexpected zone label: {live}"
        );
    }

    #[test]
    fn civil_conversion_matches_known_fixtures() {
        // Epoch, a leap day, and an ordinary modern date (fixtures
        // cross-checked against Python's datetime).
        assert_eq!(utc_date_from_unix_secs(0), "1970-01-01");
        assert_eq!(utc_date_from_unix_secs(951_782_400), "2000-02-29");
        assert_eq!(utc_date_from_unix_secs(1_751_702_400), "2025-07-05");
        // Day boundaries: last second of a day vs first of the next.
        assert_eq!(utc_date_from_unix_secs(86_399), "1970-01-01");
        assert_eq!(utc_date_from_unix_secs(86_400), "1970-01-02");
        // Extremes saturate instead of overflowing (documented contract).
        let _ = civil_from_unix_days(i64::MAX);
        let _ = civil_from_unix_days(i64::MIN);
    }

    /// The exact frozen-constant regression guard: "today" must equal the
    /// date derived from the live clock at the moment of the call — a
    /// hardcoded literal can only pass this on the single day it names.
    #[test]
    fn current_utc_date_tracks_the_live_clock() {
        let before = utc_date_from_unix_secs(current_unix_secs());
        let now = current_utc_date();
        let after = utc_date_from_unix_secs(current_unix_secs());
        // `before`/`after` bracket the call across a possible midnight tick.
        assert!(
            now == before || now == after,
            "current_utc_date must come from the live clock: {now} vs {before}/{after}"
        );
    }

    /// Local "today" is the UTC date shifted by at most one day in either
    /// direction (UTC-12 … UTC+14), and always well-formed. A frozen literal
    /// or a broken `date` invocation cannot satisfy this against a live
    /// clock outside its own day.
    #[test]
    fn current_local_date_stays_within_one_day_of_utc() {
        let local = current_local_date();
        let secs = current_unix_secs();
        let yesterday = utc_date_from_unix_secs(secs.saturating_sub(86_400));
        let today = utc_date_from_unix_secs(secs);
        let tomorrow = utc_date_from_unix_secs(secs + 86_400);
        assert!(
            local == yesterday || local == today || local == tomorrow,
            "local date must be within one day of UTC: {local} vs {yesterday}/{today}/{tomorrow}"
        );
    }
}
