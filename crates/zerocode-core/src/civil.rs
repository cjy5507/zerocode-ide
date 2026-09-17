//! Days and calendar dates, in both directions.
//!
//! Howard Hinnant's `days_from_civil` / `civil_from_days` — the standard
//! branchless pair, exact for every proleptic Gregorian date and with no era
//! table to get wrong. Four places in this tree had grown their own copy of
//! one direction or the other: the OAuth reset stamps, the vault's session
//! dates, the trust presets' file stamps, and the usage road's `Retry-After`
//! reader. They agreed, which is luck rather than design — one of them was
//! written with `div_euclid` and another with a hand-rolled negative-year
//! correction, and the two only meet for dates after 1970.
//!
//! So it lives here once, in the crate every consumer already depends on, and
//! it is signed for the whole range rather than for the half that happened to
//! be exercised: `i64` on both ends, negative days meaning before 1970.

/// Days since 1970-01-01 for a proleptic Gregorian date.
///
/// `month` is 1-12 and `day` is 1-31; a date that does not exist answers as
/// though the extra days rolled forward, which is the algorithm's own
/// behaviour and not a validation this owes its callers.
#[must_use]
pub const fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = month as i64;
    let day = day as i64;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The calendar date `days` after 1970-01-01 — the exact inverse of
/// [`days_from_civil`].
///
/// `div_euclid`/`rem_euclid` rather than `/` and `%`: a negative day count is
/// a date before 1970, and truncating division puts it in the wrong era.
#[must_use]
pub const fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "month is 1-12 and day is 1-31 by construction of the algorithm"
    )]
    (
        if month <= 2 { year + 1 } else { year },
        month as u32,
        day as u32,
    )
}

/// `YYYY-MM-DD` for a Unix millisecond stamp, `offset_minutes` east of UTC.
///
/// The offset is the CALLER's, not this module's: a day is a human unit and
/// only the window knows which midnight the person means. Passing it in is why
/// nothing here needs a timezone database.
#[must_use]
pub fn iso_date_of(epoch_ms: i64, offset_minutes: i32) -> String {
    let local_ms = epoch_ms + i64::from(offset_minutes) * 60_000;
    // Floor, not truncate: a stamp before 1970 belongs to the day it falls in,
    // not the one after.
    let days = local_ms.div_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Unix milliseconds for a strict ISO-8601 stamp:
/// `YYYY-MM-DDTHH:MM:SS(.fraction)?(Z|±HH:MM)`, with a space accepted where
/// the `T` goes because real payloads ship both.
///
/// Lived in the shell's OAuth module first, which put it on the wrong side of
/// a dependency: another reader imported it FROM the Claude/Codex road, and
/// every further provider (Kimi's `resetTime`, Grok's period stamps) would
/// have deepened that. The calendar crate is where both sides already look.
///
/// A stamp with no zone is refused, not guessed. `Date.parse` — which is what
/// Orca reads these with — resolves a zone-less string in whatever timezone
/// the machine is in, so the same deadline falls at different moments on
/// different machines and nothing anywhere reports a problem. A countdown
/// hours off is worse than none.
#[must_use]
pub fn epoch_ms_of_iso(text: &str) -> Option<i64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let date_time_split = bytes[10];
    if date_time_split != b'T' && date_time_split != b' ' {
        return None;
    }
    let year: i64 = text.get(0..4)?.parse().ok()?;
    let month: u32 = text.get(5..7)?.parse().ok()?;
    let day: u32 = text.get(8..10)?.parse().ok()?;
    let hour: i64 = text.get(11..13)?.parse().ok()?;
    let minute: i64 = text.get(14..16)?.parse().ok()?;
    let second: i64 = text.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let mut rest = &text[19..];
    let mut millis: i64 = 0;
    if let Some(after_dot) = rest.strip_prefix('.') {
        let digits: String = after_dot.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            return None;
        }
        let padded = format!("{digits:0<3}");
        millis = padded.get(0..3)?.parse().ok()?;
        rest = &after_dot[digits.len()..];
    }
    let offset_minutes: i64 = if rest == "Z" || rest == "z" {
        0
    } else {
        let sign = match rest.chars().next()? {
            '+' => 1,
            '-' => -1,
            _ => return None,
        };
        let body = &rest[1..];
        let (hours_text, minutes_text) = body.split_once(':')?;
        let hours: i64 = hours_text.parse().ok()?;
        let minutes: i64 = minutes_text.parse().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        sign * (hours * 60 + minutes)
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1_000 + millis)
}

/// A Unix millisecond stamp as `YYYY-MM-DDTHH:MM:SS.mmmZ` — the shape
/// JavaScript's `toISOString` writes, because every stamp this tree exchanges
/// was written by one of those.
///
/// The inverse of [`epoch_ms_of_iso`] for a UTC stamp: what comes out of here
/// goes back in and lands on the same millisecond.
#[must_use]
pub fn iso_utc_of(epoch_ms: i64) -> String {
    let days = epoch_ms.div_euclid(86_400_000);
    let rest = epoch_ms.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute) = (rest / 3_600_000, rest / 60_000 % 60);
    let (second, millis) = (rest / 1_000 % 60, rest % 1_000);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The epoch, the leap days, and the dates a hand-rolled version gets
    /// wrong.
    #[test]
    fn the_two_directions_are_each_others_inverse() {
        // Every row below except the last was computed by an independent
        // oracle (Python's `datetime.date`), not by reading this code back.
        for (days, date) in [
            (0, (1970, 1, 1)),
            (1, (1970, 1, 2)),
            (-1, (1969, 12, 31)),
            // 2000 is a leap year (divisible by 400); 1900 is not (by 100).
            (11_016, (2000, 2, 29)),
            (-25_509, (1900, 2, 28)),
            (-25_508, (1900, 3, 1)),
            (18_321, (2020, 2, 29)),
            (20_683, (2026, 8, 18)),
            // Well outside anything this tree will meet, to pin the era maths.
            // (Python's calendar stops at year 1, so this row is the
            // algorithm's own definition rather than an oracle's.)
            (-719_468, (0, 3, 1)),
        ] {
            let (year, month, day) = date;
            assert_eq!(days_from_civil(year, month, day), days, "{date:?} -> days");
            assert_eq!(civil_from_days(days), (year, month, day), "{days} -> date");
        }
    }

    /// A stamp written by [`iso_utc_of`] reads back as the same millisecond.
    #[test]
    fn a_written_stamp_parses_back_to_itself() {
        for millis in [
            0,
            1_755_000_000_123,
            951_782_400_000,
            -1,
            -86_400_000,
            1_740_787_199_999,
        ] {
            let text = iso_utc_of(millis);
            assert_eq!(
                epoch_ms_of_iso(&text),
                Some(millis),
                "{text} did not read back as {millis}"
            );
        }
        assert_eq!(iso_utc_of(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_utc_of(-1), "1969-12-31T23:59:59.999Z");
    }

    /// Round-trip across a long stretch, including before the epoch — the half
    /// the copies in this tree never exercised.
    #[test]
    fn a_century_of_days_survives_the_round_trip() {
        for days in (-40_000..40_000).step_by(7) {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days, "day {days}");
            assert!((1..=12).contains(&month), "month {month} at day {days}");
            assert!((1..=31).contains(&day), "day-of-month {day} at day {days}");
        }
    }

    /// A date is the caller's midnight, not UTC's.
    #[test]
    fn the_date_moves_with_the_offset_it_is_given() {
        // 2026-08-18T00:30:00Z
        let at = days_from_civil(2026, 8, 18) * 86_400_000 + 30 * 60_000;
        assert_eq!(iso_date_of(at, 0), "2026-08-18");
        // Nine hours east it is already mid-morning of the same day…
        assert_eq!(iso_date_of(at, 9 * 60), "2026-08-18");
        // …but five hours west it is still the night before.
        assert_eq!(iso_date_of(at, -5 * 60), "2026-08-17");
        // And the far side of midnight, the other way round.
        let late = days_from_civil(2026, 8, 18) * 86_400_000 + (23 * 60 + 30) * 60_000;
        assert_eq!(iso_date_of(late, 9 * 60), "2026-08-19");
        assert_eq!(iso_date_of(late, 0), "2026-08-18");
    }

    /// The written stamps of three vendors' APIs, read exactly or not at all.
    #[test]
    fn the_iso_stamps_read_strictly_and_refuse_a_zoneless_guess() {
        assert_eq!(epoch_ms_of_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            epoch_ms_of_iso("2026-08-18T00:00:00Z"),
            Some(20_683i64 * 86_400 * 1_000)
        );
        // An explicit offset shifts; fractions read; a space splits too.
        assert_eq!(
            epoch_ms_of_iso("2026-08-18T09:00:00+09:00"),
            epoch_ms_of_iso("2026-08-18T00:00:00Z")
        );
        assert_eq!(epoch_ms_of_iso("1970-01-01T00:00:00.250Z"), Some(250));
        assert_eq!(epoch_ms_of_iso("1970-01-01 00:00:01Z"), Some(1_000));
        // No zone is no guess — a countdown hours off is worse than none.
        assert_eq!(epoch_ms_of_iso("2026-08-18T00:00:00"), None);
        assert_eq!(epoch_ms_of_iso("not a date"), None);
    }

    /// The year is padded, so the strings sort the way the dates do.
    #[test]
    fn the_written_date_sorts_as_a_string() {
        let mut written = [
            iso_date_of(days_from_civil(2026, 12, 9) * 86_400_000, 0),
            iso_date_of(days_from_civil(2026, 1, 31) * 86_400_000, 0),
            iso_date_of(days_from_civil(2025, 7, 4) * 86_400_000, 0),
        ];
        written.sort();
        assert_eq!(written, ["2025-07-04", "2026-01-31", "2026-12-09"]);
    }
}
