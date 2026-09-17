//! In-memory registries for Team and Cron lifecycle management.
//!
//! Provides TeamCreate/Delete and CronCreate/Delete/List runtime backing
//! to replace the stub implementations in the tools crate.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use api::sync_bridge::lock_recovered;
use serde::{Deserialize, Serialize};

use crate::registry_io;
use crate::secure_fs::Durability;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// The zone a cron expression is read in: how far it sits ahead of UTC, and
/// the name a receipt prints.
///
/// A person writes `21 21 6 9 *` meaning the clock on their own wall, and
/// Claude Code reads the same five fields that way ("cron in the user's local
/// timezone"), so zo does too. Matching shifts the instant into this zone
/// before any calendar arithmetic and shifts the answer back — the expression
/// itself carries no zone, so there is exactly one truth per session.
///
/// The zone comes from the host through `core-types`' date helpers, the
/// workspace's one owner of "what time is it here": no zone is hardcoded, and
/// no path reads only an environment variable.
///
/// The offset is read once per process, so a session that runs across a DST
/// boundary keeps the offset it started with until restart — the same limit
/// `core_types::date::local_utc_offset_secs` already documents, and the reason
/// this is an offset rather than a zone rule: naming the rule would need a
/// timezone database this workspace deliberately does not carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CronZone {
    /// IANA name where the host gives one (`Asia/Seoul`), else `UTC±HH:MM`.
    pub label: &'static str,
    /// Seconds ahead of UTC — `32_400` for KST.
    pub offset_secs: i64,
}

/// The zone every schedule in this session is read in.
#[must_use]
pub fn cron_zone() -> CronZone {
    CronZone {
        label: core_types::date::local_time_zone_label(),
        offset_secs: core_types::date::local_utc_offset_secs(),
    }
}

/// Validate the deliberately small cron subset Zo currently registers.
///
/// Automatic scheduler execution is not wired yet, so accepting arbitrary strings would
/// create persistent records that the manual due runner cannot interpret safely. Keep
/// this parse-only gate conservative: five Vixie-style fields with `*`, numeric
/// values, ranges, steps, comma lists, plus month/day names in the fields where
/// cron traditionally supports them. Macros (for example `@daily`), seconds
/// fields, and Quartz-only tokens can be added with an explicit migration when
/// execution lands.
pub fn validate_cron_schedule(schedule: &str) -> Result<(), String> {
    let parsed = parse_cron_schedule(schedule)?;
    if !schedule_has_possible_calendar_date(&parsed) {
        return Err("cron schedule has no possible matching calendar date in the supported Gregorian cycle".to_string());
    }
    Ok(())
}

const MONTH_ALIASES: &[(&str, u32)] = &[
    ("JAN", 1),
    ("FEB", 2),
    ("MAR", 3),
    ("APR", 4),
    ("MAY", 5),
    ("JUN", 6),
    ("JUL", 7),
    ("AUG", 8),
    ("SEP", 9),
    ("OCT", 10),
    ("NOV", 11),
    ("DEC", 12),
];

const WEEKDAY_ALIASES: &[(&str, u32)] = &[
    ("SUN", 0),
    ("MON", 1),
    ("TUE", 2),
    ("WED", 3),
    ("THU", 4),
    ("FRI", 5),
    ("SAT", 6),
];

#[derive(Debug, Clone)]
struct CronFieldMatcher {
    values: BTreeSet<u32>,
    wildcard: bool,
}

impl CronFieldMatcher {
    fn contains(&self, value: u32) -> bool {
        self.values.contains(&value) || (value == 0 && self.values.contains(&7))
    }
}

#[derive(Debug, Clone)]
struct ParsedCronSchedule {
    minute: CronFieldMatcher,
    hour: CronFieldMatcher,
    day_of_month: CronFieldMatcher,
    month: CronFieldMatcher,
    day_of_week: CronFieldMatcher,
}

fn parse_cron_schedule(schedule: &str) -> Result<ParsedCronSchedule, String> {
    let trimmed = schedule.trim();
    if trimmed.is_empty() {
        return Err("cron schedule must not be empty".to_string());
    }

    let fields = trimmed.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(
            "cron schedule must use exactly 5 fields: minute hour day-of-month month day-of-week, read in this session's own time zone; macros, seconds fields and a CRON_TZ=/TZ= prefix are not supported yet"
                .to_string(),
        );
    }

    Ok(ParsedCronSchedule {
        minute: parse_cron_field_matcher(fields[0], 1, 0, 59, &[])?,
        hour: parse_cron_field_matcher(fields[1], 2, 0, 23, &[])?,
        day_of_month: parse_cron_field_matcher(fields[2], 3, 1, 31, &[])?,
        month: parse_cron_field_matcher(fields[3], 4, 1, 12, MONTH_ALIASES)?,
        day_of_week: parse_cron_field_matcher(fields[4], 5, 0, 7, WEEKDAY_ALIASES)?,
    })
}

fn parse_cron_field_matcher(
    field: &str,
    field_index: usize,
    min: u32,
    max: u32,
    aliases: &[(&str, u32)],
) -> Result<CronFieldMatcher, String> {
    if field.is_empty() {
        return Err(format!("cron schedule field {field_index} must not be empty"));
    }
    let mut values = BTreeSet::new();
    let mut wildcard = false;
    for part in field.split(',') {
        let parsed = parse_cron_part_values(part, field_index, min, max, aliases)?;
        wildcard |= parsed.wildcard;
        values.extend(parsed.values);
    }
    Ok(CronFieldMatcher { values, wildcard })
}

fn parse_cron_part_values(
    part: &str,
    field_index: usize,
    min: u32,
    max: u32,
    aliases: &[(&str, u32)],
) -> Result<CronFieldMatcher, String> {
    if part.is_empty() {
        return Err(format!(
            "cron schedule field {field_index} contains an empty list item"
        ));
    }

    let (base, step) = match part.split_once('/') {
        Some((base, step)) => (base, Some(parse_cron_step(step, field_index, max)?)),
        None => (part, None),
    };
    if base.is_empty() {
        return Err(format!(
            "cron schedule field {field_index} contains an empty step base"
        ));
    }

    if base == "*" {
        let step = step.unwrap_or(1);
        return Ok(CronFieldMatcher {
            values: stepped_values(min, max, step),
            wildcard: true,
        });
    }

    if let Some((start, end)) = base.split_once('-') {
        let start = parse_cron_value(start, field_index, min, max, aliases)?;
        let end = parse_cron_value(end, field_index, min, max, aliases)?;
        if start > end {
            return Err(format!(
                "cron schedule field {field_index} has descending range {base}"
            ));
        }
        let step = step.unwrap_or(1);
        return Ok(CronFieldMatcher {
            values: stepped_values(start, end, step),
            wildcard: false,
        });
    }

    if step.is_some() {
        return Err(format!(
            "cron schedule field {field_index} only supports steps on '*' or ranges"
        ));
    }
    let mut values = BTreeSet::new();
    values.insert(parse_cron_value(base, field_index, min, max, aliases)?);
    Ok(CronFieldMatcher {
        values,
        wildcard: false,
    })
}

fn stepped_values(start: u32, end: u32, step: u32) -> BTreeSet<u32> {
    let mut values = BTreeSet::new();
    let mut value = start;
    while value <= end {
        values.insert(value);
        match value.checked_add(step) {
            Some(next) => value = next,
            None => break,
        }
    }
    values
}

fn parse_cron_step(step: &str, field_index: usize, max: u32) -> Result<u32, String> {
    let parsed = step.parse::<u32>().map_err(|_| {
        format!("cron schedule field {field_index} has non-numeric step: {step}")
    })?;
    if parsed == 0 || parsed > max.max(1) {
        return Err(format!(
            "cron schedule field {field_index} has out-of-range step: {step}"
        ));
    }
    Ok(parsed)
}

fn parse_cron_value(
    value: &str,
    field_index: usize,
    min: u32,
    max: u32,
    aliases: &[(&str, u32)],
) -> Result<u32, String> {
    if value.is_empty() {
        return Err(format!(
            "cron schedule field {field_index} contains an empty value"
        ));
    }
    if let Some((_, alias_value)) = aliases
        .iter()
        .find(|(alias, _)| value.eq_ignore_ascii_case(alias))
    {
        return Ok(*alias_value);
    }
    if !value.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(format!(
            "cron schedule field {field_index} contains unsupported token: {value}"
        ));
    }
    let parsed = value.parse::<u32>().map_err(|_| {
        format!("cron schedule field {field_index} contains invalid number: {value}")
    })?;
    if parsed < min || parsed > max {
        return Err(format!(
            "cron schedule field {field_index} value {value} is outside {min}-{max}"
        ));
    }
    Ok(parsed)
}



fn schedule_has_possible_calendar_date(parsed: &ParsedCronSchedule) -> bool {
    for year in 2024..=2027 {
        let mut weekday = day_of_week_from_civil_date(year, 1, 1);
        for month in 1..=12 {
            for day in 1..=days_in_month(year, month) {
                let dt = CronDateTime {
                    minute: *parsed.minute.values.iter().next().unwrap_or(&0),
                    hour: *parsed.hour.values.iter().next().unwrap_or(&0),
                    day,
                    month,
                    day_of_week: weekday,
                };
                if cron_date_matches(parsed, dt) {
                    return true;
                }
                weekday = (weekday + 1) % 7;
            }
        }
    }
    false
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn day_of_week_from_civil_date(year: i32, month: u32, day: u32) -> u32 {
    let days = days_from_civil(year, month, day);
    day_of_week_from_unix_days(days)
}

// Inverse of `civil_from_days`, also from Howard Hinnant's date algorithms.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let mut year = i64::from(year);
    let month = i64::from(month);
    let day = i64::from(day);
    year -= i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

const MAX_NEXT_DUE_LOOKAHEAD_DAYS: u64 = 5 * 366;
const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * SECONDS_PER_MINUTE;
const SECONDS_PER_DAY: u64 = 24 * SECONDS_PER_HOUR;

#[derive(Debug, Clone)]
pub struct CronDueEntry {
    pub entry: CronEntry,
    pub due_at: u64,
    pub next_due_at: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
struct CronDateTime {
    minute: u32,
    hour: u32,
    day: u32,
    month: u32,
    day_of_week: u32,
}

/// Return whether the supported five-field cron schedule matches the minute of
/// [`cron_zone`] that contains `timestamp`.
pub fn cron_matches_timestamp(schedule: &str, timestamp: u64) -> Result<bool, String> {
    cron_matches_timestamp_in_zone(schedule, timestamp, cron_zone().offset_secs)
}

/// [`cron_matches_timestamp`] against a pinned offset — what a fixture uses so
/// its instants mean the same thing on every host.
pub fn cron_matches_timestamp_in_zone(
    schedule: &str,
    timestamp: u64,
    offset_secs: i64,
) -> Result<bool, String> {
    let parsed = parse_cron_schedule(schedule)?;
    let wall = datetime_from_wall_secs(wall_secs(timestamp, offset_secs));
    Ok(cron_matches_parsed_datetime(&parsed, wall))
}

/// The same instant counted from the zone's own epoch, so every calendar helper
/// below keeps doing plain days-and-minutes arithmetic and only these two
/// functions know a zone exists.
fn wall_secs(timestamp: u64, offset_secs: i64) -> u64 {
    timestamp.saturating_add_signed(offset_secs)
}

/// Back from a wall-clock instant in that zone to unix seconds.
fn instant_secs(wall: u64, offset_secs: i64) -> u64 {
    wall.saturating_add_signed(offset_secs.saturating_neg())
}

fn cron_matches_parsed_datetime(parsed: &ParsedCronSchedule, dt: CronDateTime) -> bool {
    parsed.minute.contains(dt.minute)
        && parsed.hour.contains(dt.hour)
        && cron_date_matches(parsed, dt)
}

fn cron_date_matches(parsed: &ParsedCronSchedule, dt: CronDateTime) -> bool {
    let day_of_month_matches = parsed.day_of_month.contains(dt.day);
    let weekday_matches = parsed.day_of_week.contains(dt.day_of_week);
    let day_matches = if parsed.day_of_month.wildcard && parsed.day_of_week.wildcard {
        true
    } else if parsed.day_of_month.wildcard {
        weekday_matches
    } else if parsed.day_of_week.wildcard {
        day_of_month_matches
    } else {
        // Vixie cron semantics: when both day fields are restricted, either
        // field may match.
        day_of_month_matches || weekday_matches
    };

    parsed.month.contains(dt.month) && day_matches
}

/// Find the next minute of [`cron_zone`] strictly after `after_timestamp` that
/// matches the supported cron schedule, as unix seconds. Returns `None` only if
/// the bounded five-year search cannot find a match.
pub fn next_due_at_after(schedule: &str, after_timestamp: u64) -> Result<Option<u64>, String> {
    next_due_at_after_in_zone(schedule, after_timestamp, cron_zone().offset_secs)
}

/// [`next_due_at_after`] against a pinned offset.
pub fn next_due_at_after_in_zone(
    schedule: &str,
    after_timestamp: u64,
    offset_secs: i64,
) -> Result<Option<u64>, String> {
    let start = floor_to_minute(after_timestamp).saturating_add(SECONDS_PER_MINUTE);
    next_due_at_on_or_after(schedule, start, offset_secs)
}

fn next_due_at_on_or_after(
    schedule: &str,
    start_timestamp: u64,
    offset_secs: i64,
) -> Result<Option<u64>, String> {
    let parsed = parse_cron_schedule(schedule)?;
    // A zone offset is a whole number of minutes, so flooring commutes with the
    // shift: the wall-clock minute is the same minute either way round.
    let start = floor_to_minute(wall_secs(start_timestamp, offset_secs));
    let start_day = floor_to_day(start);
    for day_offset in 0..=MAX_NEXT_DUE_LOOKAHEAD_DAYS {
        let day_start = start_day.saturating_add(day_offset.saturating_mul(SECONDS_PER_DAY));
        let date = datetime_from_wall_secs(day_start);
        if !cron_date_matches(&parsed, date) {
            continue;
        }
        if let Some(candidate) = earliest_time_on_date(&parsed, day_start, start) {
            return Ok(Some(instant_secs(candidate, offset_secs)));
        }
    }
    Ok(None)
}

fn latest_due_at_between(
    schedule: &str,
    start_timestamp: u64,
    end_timestamp: u64,
    offset_secs: i64,
) -> Result<Option<u64>, String> {
    if start_timestamp > end_timestamp {
        return Ok(None);
    }
    let parsed = parse_cron_schedule(schedule)?;
    let start = floor_to_minute(wall_secs(start_timestamp, offset_secs));
    let end = floor_to_minute(wall_secs(end_timestamp, offset_secs));
    let end_day = floor_to_day(end);
    let earliest_day = floor_to_day(start).max(end_day.saturating_sub(
        MAX_NEXT_DUE_LOOKAHEAD_DAYS.saturating_mul(SECONDS_PER_DAY),
    ));
    for day_offset in 0..=MAX_NEXT_DUE_LOOKAHEAD_DAYS {
        let day_start = end_day.saturating_sub(day_offset.saturating_mul(SECONDS_PER_DAY));
        if day_start < earliest_day {
            break;
        }
        let date = datetime_from_wall_secs(day_start);
        if !cron_date_matches(&parsed, date) {
            continue;
        }
        if let Some(candidate) = latest_time_on_date(&parsed, day_start, start, end) {
            return Ok(Some(instant_secs(candidate, offset_secs)));
        }
    }
    Ok(None)
}

fn earliest_time_on_date(parsed: &ParsedCronSchedule, day_start: u64, lower_bound: u64) -> Option<u64> {
    for hour in &parsed.hour.values {
        for minute in &parsed.minute.values {
            let candidate = day_start
                .saturating_add(u64::from(*hour).saturating_mul(SECONDS_PER_HOUR))
                .saturating_add(u64::from(*minute).saturating_mul(SECONDS_PER_MINUTE));
            if candidate >= lower_bound {
                return Some(candidate);
            }
        }
    }
    None
}

fn latest_time_on_date(
    parsed: &ParsedCronSchedule,
    day_start: u64,
    lower_bound: u64,
    upper_bound: u64,
) -> Option<u64> {
    for hour in parsed.hour.values.iter().rev() {
        for minute in parsed.minute.values.iter().rev() {
            let candidate = day_start
                .saturating_add(u64::from(*hour).saturating_mul(SECONDS_PER_HOUR))
                .saturating_add(u64::from(*minute).saturating_mul(SECONDS_PER_MINUTE));
            if candidate <= upper_bound && candidate >= lower_bound {
                return Some(candidate);
            }
        }
    }
    None
}

fn floor_to_minute(timestamp: u64) -> u64 {
    timestamp - (timestamp % SECONDS_PER_MINUTE)
}

fn floor_to_day(timestamp: u64) -> u64 {
    timestamp - (timestamp % SECONDS_PER_DAY)
}

fn ceil_to_minute(timestamp: u64) -> u64 {
    let floored = floor_to_minute(timestamp);
    if timestamp == floored {
        floored
    } else {
        floored.saturating_add(SECONDS_PER_MINUTE)
    }
}

/// Split a wall-clock instant (seconds from the zone's own epoch, i.e. the
/// output of [`wall_secs`]) into the fields a cron expression names.
fn datetime_from_wall_secs(wall: u64) -> CronDateTime {
    let total_minutes = wall / SECONDS_PER_MINUTE;
    let minute = (total_minutes % 60) as u32;
    let total_hours = total_minutes / 60;
    let hour = u32::try_from(total_hours % 24).unwrap_or_default();
    let days = i64::try_from(total_hours / 24).unwrap_or(i64::MAX);
    let (_year, month, day) = civil_from_days(days);
    CronDateTime {
        minute,
        hour,
        day,
        month,
        day_of_week: day_of_week_from_unix_days(days),
    }
}

// Howard Hinnant's civil-from-days algorithm, with days counted from the Unix
// epoch. It keeps cron matching dependency-free; the zone is applied before it,
// by [`wall_secs`], so this stays pure arithmetic.
//
// `pub(crate)` because the second brain's ingest log stamps `YYYY-MM-DD HH:MM`
// on every promoted page, and a second copy of this algorithm is exactly the
// kind of drift a shared helper exists to prevent.
pub(crate) fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u32::try_from(month).unwrap_or_default(),
        u32::try_from(day).unwrap_or_default(),
    )
}

fn day_of_week_from_unix_days(days_since_unix_epoch: i64) -> u32 {
    u32::try_from((days_since_unix_epoch + 4).rem_euclid(7)).unwrap_or_default()
}

fn entry_next_due_at(entry: &CronEntry, now: u64) -> Option<u64> {
    if !entry.enabled {
        return None;
    }
    let created_floor = ceil_to_minute(entry.created_at);
    let last_next = entry.last_run_at.map_or(0, |last| {
        floor_to_minute(last).saturating_add(SECONDS_PER_MINUTE)
    });
    let start = floor_to_minute(now).max(created_floor).max(last_next);
    next_due_at_on_or_after(&entry.schedule, start, cron_zone().offset_secs)
        .ok()
        .flatten()
}

fn entry_is_due_at(entry: &CronEntry, timestamp: u64) -> Option<CronDueEntry> {
    if !entry.enabled {
        return None;
    }
    let end = floor_to_minute(timestamp);
    let created_floor = ceil_to_minute(entry.created_at);
    let last_next = entry.last_run_at.map_or(0, |last| {
        floor_to_minute(last).saturating_add(SECONDS_PER_MINUTE)
    });
    let start = created_floor.max(last_next);
    let due_at = latest_due_at_between(&entry.schedule, start, end, cron_zone().offset_secs)
        .ok()
        .flatten()?;
    let mut snapshot = entry.clone();
    snapshot.last_run_at = Some(due_at);
    Some(CronDueEntry {
        entry: entry.clone(),
        due_at,
        next_due_at: entry_next_due_at(&snapshot, due_at),
    })
}

#[cfg(test)]
const MAX_TEAM_REGISTRY_ENTRIES: usize = 4;
#[cfg(not(test))]
const MAX_TEAM_REGISTRY_ENTRIES: usize = 256;

#[cfg(test)]
const MAX_CRON_REGISTRY_ENTRIES: usize = 4;
#[cfg(not(test))]
const MAX_CRON_REGISTRY_ENTRIES: usize = 256;

/// The cron registry's file name under `.zo/registries/`.
pub const CRON_REGISTRY_FILE: &str = "crons.json";

/// What fires a due cron: the idle session's own driver
/// (`zo-ide/src/autonomy/crons.rs`). Every receipt that names the scheduler —
/// the `CronCreate`/`CronList` tools inside a session and `zo cron` outside
/// one — spells it from here.
pub const AUTOMATIC_SCHEDULER_STATUS: &str = "session_idle";
pub const AUTOMATIC_SCHEDULER_NOTE: &str =
    "a due cron opens a turn with its prompt in the idle zo session whose working directory owns this registry (schedules match the wall clock of the session's own time zone, named in the receipt); CronRunDue still enqueues due crons by hand as task records";

/// What [`CronRegistry::ensure_named`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CronEnsure {
    /// No entry carried the name; one was created.
    Created,
    /// An enabled entry with this name, schedule and prompt already stood.
    Unchanged,
    /// An entry with this name stood with another schedule or prompt (or
    /// disabled); it was removed and a fresh one created.
    Replaced,
}

impl CronEnsure {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Unchanged => "unchanged",
            Self::Replaced => "replaced",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub team_id: String,
    pub name: String,
    pub task_ids: Vec<String>,
    pub status: TeamStatus,
    pub created_at: u64,
    pub updated_at: u64,
    /// Monotonic registry revision at which this team was last written. Drives
    /// merge conflict resolution instead of the whole-second `updated_at`.
    /// Defaults to 0 for pre-revision files, which always lose to a stamped
    /// write.
    #[serde(default)]
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    Created,
    Running,
    Completed,
    Deleted,
}

impl std::fmt::Display for TeamStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Deleted => write!(f, "deleted"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TeamRegistry {
    /// Poison policy: recover (`lock_recovered`) — same judgment as
    /// `TaskRegistry`: every write finishes its in-memory mutation before
    /// the only fallible step (persistence, which swallows IO errors), so
    /// the map stays consistent at every panic point.
    inner: Arc<Mutex<TeamInner>>,
    persistence_path: Arc<Option<PathBuf>>,
    /// Warn-once latch for persistence failures (see
    /// [`registry_io::save_registry_inner_warn_once`]); re-armed by a
    /// successful save.
    persist_warned: Arc<AtomicBool>,
    /// Fail-closed persistence latch. Set the first time a persist fails; once
    /// set, this instance never writes to disk again (mutations stay in memory
    /// so local reads are truthful, but can never clobber a peer's committed
    /// state). Recovery is a process restart that reloads from disk.
    persist_disabled: Arc<AtomicBool>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TeamInner {
    teams: HashMap<String, Team>,
    counter: u64,
    /// Monotonic revision advanced on every mutation and max-merged across
    /// processes so committed writes get comparable revisions even within one
    /// wall-clock second. `#[serde(default)]` keeps pre-revision files loadable.
    #[serde(default)]
    revision: u64,
    /// Removal records: team id → the `revision` at which it was removed or
    /// pruned, so the merge cannot resurrect a hard deletion. Tombstones are
    /// unbounded and durable: they are never evicted, because a safe cap would
    /// require cross-process coordination to prove no arbitrarily-stale peer
    /// still holds the deleted team. Correctness (no resurrection) is preferred
    /// over the bounded file growth an eviction cap would give.
    /// `#[serde(default)]` keeps old files loadable.
    #[serde(default)]
    tombstones: HashMap<String, u64>,
}

impl TeamInner {
    fn bump_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn tombstone(&mut self, team_id: String) {
        let rev = self.bump_revision();
        self.tombstones.insert(team_id, rev);
    }
}

impl registry_io::MergeInto for TeamInner {
    /// Fold a concurrently-persisted on-disk copy in so a peer Zo process's
    /// teams survive this process's rewrite, while keeping hard removals
    /// authoritative. Order-independent: max revision/counter, union tombstones
    /// by max revision, keep whichever side of a team carries the newer `rev`
    /// (ties keep ours), then normalize so no team survives an equal-or-newer
    /// tombstone and no tombstone outlives a strictly newer re-insertion.
    fn merge_in(&mut self, on_disk: Self) {
        self.revision = self.revision.max(on_disk.revision);
        self.counter = self.counter.max(on_disk.counter);
        for (id, disk_rev) in on_disk.tombstones {
            let slot = self.tombstones.entry(id).or_insert(0);
            *slot = (*slot).max(disk_rev);
        }
        for (id, disk_team) in on_disk.teams {
            match self.teams.get(&id) {
                Some(mine) if mine.rev >= disk_team.rev => {}
                _ => {
                    self.teams.insert(id, disk_team);
                }
            }
        }
        let tombstones = &self.tombstones;
        self.teams
            .retain(|id, team| !matches!(tombstones.get(id), Some(&rev) if rev >= team.rev));
        let teams = &self.teams;
        self.tombstones
            .retain(|id, rev| !matches!(teams.get(id), Some(team) if team.rev > *rev));
    }
}

impl TeamRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::with_persistence_path(registry_io::default_registry_path("teams.json"))
    }

    #[must_use]
    pub fn new_in_memory() -> Self {
        Self::with_persistence_path(None)
    }

    #[must_use]
    pub fn with_persistence_path(path: Option<PathBuf>) -> Self {
        let inner = path
            .as_deref()
            .and_then(registry_io::load_registry_inner::<TeamInner>)
            .unwrap_or_default();
        Self {
            inner: Arc::new(Mutex::new(inner)),
            persistence_path: Arc::new(path),
            persist_warned: Arc::default(),
            persist_disabled: Arc::default(),
        }
    }

    pub fn create(&self, name: &str, task_ids: Vec<String>) -> Result<Team, String> {
        let mut inner = lock_recovered(&self.inner);
        // Capacity check, counter/revision/id allocation, and insert all happen
        // INSIDE the transaction, after the merge of the newest disk state, so a
        // peer process that committed on the same base revision cannot collide:
        // our counter is incremented off the merged (post-peer) value, yielding
        // a distinct id and a strictly newer rev.
        self.commit_mutation(&mut inner, move |inner| {
            prune_terminal_teams_for_create(inner);
            if inner.teams.len() >= MAX_TEAM_REGISTRY_ENTRIES {
                return Err(format!(
                    "team registry is full (max {MAX_TEAM_REGISTRY_ENTRIES}); delete completed/deleted teams before creating more"
                ));
            }
            inner.counter += 1;
            let rev = inner.bump_revision();
            let ts = now_secs();
            let team_id = format!("team_{:08x}_{}", ts, inner.counter);
            let team = Team {
                team_id: team_id.clone(),
                name: name.to_owned(),
                task_ids,
                status: TeamStatus::Created,
                created_at: ts,
                updated_at: ts,
                rev,
            };
            inner.teams.insert(team_id, team.clone());
            Ok(team)
        })
    }

    #[must_use]
    pub fn get(&self, team_id: &str) -> Option<Team> {
        let inner = lock_recovered(&self.inner);
        inner.teams.get(team_id).cloned()
    }

    #[must_use]
    pub fn list(&self) -> Vec<Team> {
        let inner = lock_recovered(&self.inner);
        inner.teams.values().cloned().collect()
    }

    pub fn delete(&self, team_id: &str) -> Result<Team, String> {
        let mut inner = lock_recovered(&self.inner);
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let rev = inner.bump_revision();
            let team = inner
                .teams
                .get_mut(team_id)
                .ok_or_else(|| format!("team not found: {team_id}"))?;
            team.status = TeamStatus::Deleted;
            team.updated_at = now_secs();
            team.rev = rev;
            let snapshot = team.clone();
            prune_terminal_teams(inner);
            Ok(snapshot)
        });
        fold_persistence(result, persisted)
    }

    #[must_use]
    pub fn remove(&self, team_id: &str) -> Option<Team> {
        let mut inner = lock_recovered(&self.inner);
        // Remove and tombstone INSIDE the transaction, after the on-disk merge,
        // so a concurrently-persisted peer copy cannot resurrect this team.
        self.commit_mutation(&mut inner, |inner| {
            let removed = inner.teams.remove(team_id);
            if removed.is_some() {
                inner.tombstone(team_id.to_string());
            }
            removed
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let inner = lock_recovered(&self.inner);
        inner.teams.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Run `mutate` as a persisted transaction: when a persistence path is
    /// configured, the merge→mutate→write happens under the cross-process lock
    /// so `mutate` can allocate ids/revisions rebased onto the newest committed
    /// disk state (see [`registry_io::commit_registry_mutation`]). Without a
    /// path (in-memory registry) `mutate` runs directly on the held guard.
    ///
    /// Best-effort variant used by infallible mutations; the transactional
    /// rollback in `commit_registry_mutation` still prevents a failed write from
    /// clobbering a peer on a later commit.
    fn commit_mutation<R>(
        &self,
        inner: &mut TeamInner,
        mutate: impl FnOnce(&mut TeamInner) -> R,
    ) -> R {
        self.commit_mutation_status(inner, mutate).0
    }

    /// Like [`Self::commit_mutation`] but also returns the persistence status so
    /// a `Result`-returning method can surface a persistence failure as an error
    /// instead of a silent success. On failure the in-memory state is rolled
    /// back.
    fn commit_mutation_status<R>(
        &self,
        inner: &mut TeamInner,
        mutate: impl FnOnce(&mut TeamInner) -> R,
    ) -> (R, Result<(), String>) {
        if let Some(path) = self.persistence_path.as_ref().as_ref() {
            registry_io::commit_registry_mutation_warn_once_status(
                "team registry",
                path,
                &self.persist_disabled,
                inner,
                &self.persist_warned,
                // A schedule or a team the person armed should outlive a power loss.
                Durability::Flushed,
                mutate,
            )
        } else {
            (mutate(inner), Ok(()))
        }
    }
}

fn prune_terminal_teams(inner: &mut TeamInner) {
    prune_teams_by_status_to(inner, TeamStatus::Deleted, MAX_TEAM_REGISTRY_ENTRIES);
    prune_teams_by_status_to(inner, TeamStatus::Completed, MAX_TEAM_REGISTRY_ENTRIES);
}

fn prune_terminal_teams_for_create(inner: &mut TeamInner) {
    let target_len = MAX_TEAM_REGISTRY_ENTRIES.saturating_sub(1);
    prune_teams_by_status_to(inner, TeamStatus::Deleted, target_len);
    prune_teams_by_status_to(inner, TeamStatus::Completed, target_len);
}

fn prune_teams_by_status_to(inner: &mut TeamInner, status: TeamStatus, target_len: usize) {
    let excess = inner.teams.len().saturating_sub(target_len);
    if excess == 0 {
        return;
    }

    let mut candidates: Vec<_> = inner
        .teams
        .values()
        .filter(|team| team.status == status)
        .map(|team| (team.updated_at, team.created_at, team.team_id.clone()))
        .collect();
    candidates.sort_unstable();
    for (_, _, team_id) in candidates.into_iter().take(excess) {
        inner.teams.remove(&team_id);
        // Prune is a removal too: tombstone it so the on-disk merge does not
        // revive a team this process just evicted for overflow.
        inner.tombstone(team_id);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronEntry {
    pub cron_id: String,
    pub schedule: String,
    pub prompt: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_run_at: Option<u64>,
    pub run_count: u64,
    /// Monotonic registry revision at which this entry was last written. Drives
    /// merge conflict resolution instead of the whole-second `updated_at`.
    /// Defaults to 0 for pre-revision files, which always lose to a stamped
    /// write.
    #[serde(default)]
    pub rev: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CronRegistry {
    /// Poison policy: recover (`lock_recovered`) — same judgment as
    /// `TaskRegistry`/`TeamRegistry` above.
    inner: Arc<Mutex<CronInner>>,
    persistence_path: Arc<Option<PathBuf>>,
    /// Warn-once latch for persistence failures (see
    /// [`registry_io::save_registry_inner_warn_once`]); re-armed by a
    /// successful save.
    persist_warned: Arc<AtomicBool>,
    /// Fail-closed persistence latch. Set the first time a persist fails; once
    /// set, this instance never writes to disk again (mutations stay in memory
    /// so local reads are truthful, but can never clobber a peer's committed
    /// state). Recovery is a process restart that reloads from disk.
    persist_disabled: Arc<AtomicBool>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct CronInner {
    entries: HashMap<String, CronEntry>,
    counter: u64,
    /// Monotonic revision advanced on every mutation and max-merged across
    /// processes so committed writes get comparable revisions even within one
    /// wall-clock second. `#[serde(default)]` keeps pre-revision files loadable.
    #[serde(default)]
    revision: u64,
    /// Removal records: cron id → the `revision` at which it was deleted or
    /// pruned, so the merge cannot resurrect a removal. Tombstones are unbounded
    /// and durable: they are never evicted, because a safe cap would require
    /// cross-process coordination to prove no arbitrarily-stale peer still holds
    /// the deleted cron. Correctness (no resurrection) is preferred over the
    /// bounded file growth an eviction cap would give. `#[serde(default)]` keeps
    /// old files loadable.
    #[serde(default)]
    tombstones: HashMap<String, u64>,
}

impl CronInner {
    fn bump_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn tombstone(&mut self, cron_id: String) {
        let rev = self.bump_revision();
        self.tombstones.insert(cron_id, rev);
    }
}

impl registry_io::MergeInto for CronInner {
    /// Fold a concurrently-persisted on-disk copy in so a peer Zo process's
    /// cron entries survive this process's rewrite, while keeping removals
    /// authoritative. Order-independent: max revision/counter, union tombstones
    /// by max revision, keep whichever side of an entry carries the newer `rev`
    /// (ties keep ours), then normalize so no entry survives an equal-or-newer
    /// tombstone and no tombstone outlives a strictly newer re-insertion.
    fn merge_in(&mut self, on_disk: Self) {
        self.revision = self.revision.max(on_disk.revision);
        self.counter = self.counter.max(on_disk.counter);
        for (id, disk_rev) in on_disk.tombstones {
            let slot = self.tombstones.entry(id).or_insert(0);
            *slot = (*slot).max(disk_rev);
        }
        for (id, disk_entry) in on_disk.entries {
            match self.entries.get(&id) {
                Some(mine) if mine.rev >= disk_entry.rev => {}
                _ => {
                    self.entries.insert(id, disk_entry);
                }
            }
        }
        let tombstones = &self.tombstones;
        self.entries
            .retain(|id, entry| !matches!(tombstones.get(id), Some(&rev) if rev >= entry.rev));
        let entries = &self.entries;
        self.tombstones
            .retain(|id, rev| !matches!(entries.get(id), Some(entry) if entry.rev > *rev));
    }
}

impl CronRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::with_persistence_path(registry_io::default_registry_path(CRON_REGISTRY_FILE))
    }

    /// The registry a session started in `cwd` reads — for a process that
    /// registers on that workspace's behalf without being a session there.
    #[must_use]
    pub fn for_workspace(cwd: &Path) -> Self {
        Self::with_persistence_path(Some(registry_io::registry_path_in(cwd, CRON_REGISTRY_FILE)))
    }

    /// Where this registry persists, if anywhere.
    #[must_use]
    pub fn persistence_path(&self) -> Option<&Path> {
        self.persistence_path.as_deref()
    }

    /// The entries whose `description` is exactly `name`, newest first. A
    /// description is the one field a caller chooses freely, so it is how an
    /// owner finds its own record again (`zerocode:second-brain:…`).
    #[must_use]
    pub fn find_named(&self, name: &str) -> Vec<CronEntry> {
        let mut held: Vec<CronEntry> = self
            .list(false)
            .into_iter()
            .filter(|entry| entry.description.as_deref() == Some(name))
            .collect();
        held.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| right.cron_id.cmp(&left.cron_id))
        });
        held
    }

    /// Exactly one enabled entry named `name` with this schedule and prompt —
    /// idempotent: calling it again with the same words changes nothing, and
    /// calling it with other words replaces the record rather than adding a
    /// second one under the same name.
    pub fn ensure_named(
        &self,
        name: &str,
        schedule: &str,
        prompt: &str,
    ) -> Result<(CronEntry, CronEnsure), String> {
        validate_cron_schedule(schedule)?;
        let schedule = schedule.trim();
        let prompt = prompt.trim();
        let held = self.find_named(name);
        let mut outcome = CronEnsure::Created;
        let mut keep: Option<CronEntry> = None;
        for entry in held {
            if keep.is_none()
                && entry.enabled
                && entry.schedule == schedule
                && entry.prompt == prompt
            {
                keep = Some(entry);
                continue;
            }
            // A stale twin, a disabled record, or the old words: gone before
            // the fresh record stands, so the name never answers twice.
            self.delete(&entry.cron_id)?;
            outcome = CronEnsure::Replaced;
        }
        if let Some(entry) = keep {
            return Ok((entry, if outcome == CronEnsure::Replaced { outcome } else { CronEnsure::Unchanged }));
        }
        let entry = self.create(schedule, prompt, Some(name))?;
        Ok((entry, outcome))
    }

    /// Remove every entry named `name`; answers how many stood.
    pub fn remove_named(&self, name: &str) -> Result<usize, String> {
        let held = self.find_named(name);
        let count = held.len();
        for entry in held {
            self.delete(&entry.cron_id)?;
        }
        Ok(count)
    }

    #[must_use]
    pub fn new_in_memory() -> Self {
        Self::with_persistence_path(None)
    }

    #[must_use]
    pub fn with_persistence_path(path: Option<PathBuf>) -> Self {
        let mut inner = path
            .as_deref()
            .and_then(registry_io::load_registry_inner::<CronInner>)
            .unwrap_or_default();
        let persist_warned: Arc<AtomicBool> = Arc::default();
        let sanitized = quarantine_invalid_crons_on_load(&mut inner);
        if sanitized {
            if let Some(path) = path.as_deref() {
                registry_io::save_registry_inner_warn_once(
                    "sanitized cron registry",
                    path,
                    &mut inner,
                    &persist_warned,
                );
            }
        }
        Self {
            inner: Arc::new(Mutex::new(inner)),
            persistence_path: Arc::new(path),
            persist_warned,
            persist_disabled: Arc::default(),
        }
    }

    pub fn create(
        &self,
        schedule: &str,
        prompt: &str,
        description: Option<&str>,
    ) -> Result<CronEntry, String> {
        validate_cron_schedule(schedule)?;
        if prompt.trim().is_empty() {
            return Err("cron prompt must not be empty".to_string());
        }

        let mut inner = lock_recovered(&self.inner);
        // Capacity check, counter/revision/id allocation, and insert all happen
        // INSIDE the transaction, after the merge of the newest disk state, so a
        // peer process that committed on the same base revision cannot collide.
        self.commit_mutation(&mut inner, move |inner| {
            prune_crons_for_create(inner);
            if inner.entries.len() >= MAX_CRON_REGISTRY_ENTRIES {
                return Err(format!(
                    "cron registry is full (max {MAX_CRON_REGISTRY_ENTRIES}); delete old crons before creating more"
                ));
            }
            inner.counter += 1;
            let rev = inner.bump_revision();
            let ts = now_secs();
            let cron_id = format!("cron_{:08x}_{}", ts, inner.counter);
            let entry = CronEntry {
                cron_id: cron_id.clone(),
                schedule: schedule.trim().to_owned(),
                prompt: prompt.trim().to_owned(),
                description: description.map(str::to_owned),
                enabled: true,
                created_at: ts,
                updated_at: ts,
                last_run_at: None,
                run_count: 0,
                rev,
            };
            inner.entries.insert(cron_id, entry.clone());
            Ok(entry)
        })
    }

    #[must_use]
    pub fn get(&self, cron_id: &str) -> Option<CronEntry> {
        let inner = lock_recovered(&self.inner);
        inner.entries.get(cron_id).cloned()
    }

    #[must_use]
    pub fn list(&self, enabled_only: bool) -> Vec<CronEntry> {
        let inner = lock_recovered(&self.inner);
        let mut entries = inner
            .entries
            .values()
            .filter(|e| !enabled_only || e.enabled)
            .cloned()
            .collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.cron_id.cmp(&right.cron_id))
        });
        entries
    }

    #[must_use]
    pub fn due_at(&self, timestamp: u64) -> Vec<CronDueEntry> {
        let inner = lock_recovered(&self.inner);
        let mut entries = inner
            .entries
            .values()
            .filter_map(|entry| entry_is_due_at(entry, timestamp))
            .collect::<Vec<_>>();
        entries.sort_unstable_by(|left, right| {
            left.entry
                .created_at
                .cmp(&right.entry.created_at)
                .then_with(|| left.entry.cron_id.cmp(&right.entry.cron_id))
        });
        entries
    }

    pub fn next_due_at(&self, cron_id: &str, now: u64) -> Result<Option<u64>, String> {
        let inner = lock_recovered(&self.inner);
        let entry = inner
            .entries
            .get(cron_id)
            .ok_or_else(|| format!("cron not found: {cron_id}"))?;
        Ok(entry_next_due_at(entry, now))
    }

    pub fn record_due_run_at(&self, cron_id: &str, due_at: u64) -> Result<bool, String> {
        let mut inner = lock_recovered(&self.inner);
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let rev = inner.bump_revision();
            let entry = inner
                .entries
                .get_mut(cron_id)
                .ok_or_else(|| format!("cron not found: {cron_id}"))?;
            if !entry.enabled {
                return Err(format!("cron is disabled: {cron_id}"));
            }
            let due_at = floor_to_minute(due_at);
            if entry
                .last_run_at
                .is_some_and(|last_run_at| floor_to_minute(last_run_at) >= due_at)
            {
                return Ok(false);
            }
            entry.last_run_at = Some(due_at);
            entry.run_count += 1;
            entry.updated_at = now_secs();
            entry.rev = rev;
            prune_crons(inner);
            Ok(true)
        });
        fold_persistence(result, persisted)
    }

    pub fn delete(&self, cron_id: &str) -> Result<CronEntry, String> {
        let mut inner = lock_recovered(&self.inner);
        // Remove and tombstone INSIDE the transaction, after the on-disk merge,
        // so a concurrently-persisted peer copy cannot resurrect this entry.
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let removed = inner
                .entries
                .remove(cron_id)
                .ok_or_else(|| format!("cron not found: {cron_id}"))?;
            inner.tombstone(cron_id.to_string());
            Ok(removed)
        });
        fold_persistence(result, persisted)
    }

    /// Disable a cron entry without removing it.
    pub fn disable(&self, cron_id: &str) -> Result<(), String> {
        let mut inner = lock_recovered(&self.inner);
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let rev = inner.bump_revision();
            let entry = inner
                .entries
                .get_mut(cron_id)
                .ok_or_else(|| format!("cron not found: {cron_id}"))?;
            entry.enabled = false;
            entry.updated_at = now_secs();
            entry.rev = rev;
            prune_crons(inner);
            Ok(())
        });
        fold_persistence(result, persisted)
    }

    /// Record a cron run.
    pub fn record_run(&self, cron_id: &str) -> Result<(), String> {
        let mut inner = lock_recovered(&self.inner);
        let (result, persisted) = self.commit_mutation_status(&mut inner, |inner| {
            let rev = inner.bump_revision();
            let entry = inner
                .entries
                .get_mut(cron_id)
                .ok_or_else(|| format!("cron not found: {cron_id}"))?;
            entry.last_run_at = Some(now_secs());
            entry.run_count += 1;
            entry.updated_at = now_secs();
            entry.rev = rev;
            prune_crons(inner);
            Ok(())
        });
        fold_persistence(result, persisted)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let inner = lock_recovered(&self.inner);
        inner.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Run `mutate` as a persisted transaction: when a persistence path is
    /// configured, the merge→mutate→write happens under the cross-process lock
    /// so `mutate` can allocate ids/revisions rebased onto the newest committed
    /// disk state (see [`registry_io::commit_registry_mutation`]). Without a
    /// path (in-memory registry) `mutate` runs directly on the held guard.
    ///
    /// Best-effort variant used by infallible mutations; the transactional
    /// rollback in `commit_registry_mutation` still prevents a failed write from
    /// clobbering a peer on a later commit.
    fn commit_mutation<R>(
        &self,
        inner: &mut CronInner,
        mutate: impl FnOnce(&mut CronInner) -> R,
    ) -> R {
        self.commit_mutation_status(inner, mutate).0
    }

    /// Like [`Self::commit_mutation`] but also returns the persistence status so
    /// a `Result`-returning method can surface a persistence failure as an error
    /// instead of a silent success. On failure the in-memory state is rolled
    /// back.
    fn commit_mutation_status<R>(
        &self,
        inner: &mut CronInner,
        mutate: impl FnOnce(&mut CronInner) -> R,
    ) -> (R, Result<(), String>) {
        if let Some(path) = self.persistence_path.as_ref().as_ref() {
            registry_io::commit_registry_mutation_warn_once_status(
                "cron registry",
                path,
                &self.persist_disabled,
                inner,
                &self.persist_warned,
                // A schedule or a team the person armed should outlive a power loss.
                Durability::Flushed,
                mutate,
            )
        } else {
            (mutate(inner), Ok(()))
        }
    }
}

/// Fold a persistence status into a mutation's result so a persist failure is
/// surfaced as an error rather than a silent success. A mutation error wins
/// (no write was attempted in that case); only a mutation that succeeded in
/// memory but failed to persist is downgraded to `Err`. Paired with the
/// fail-closed latch in `commit_registry_mutation`, a reported success is
/// durable and a reported failure never writes to disk, so no peer can be
/// clobbered by this instance later.
fn fold_persistence<T>(
    result: Result<T, String>,
    persisted: Result<(), String>,
) -> Result<T, String> {
    match result {
        Err(error) => Err(error),
        Ok(value) => persisted.map(|()| value),
    }
}

fn prune_crons(inner: &mut CronInner) {
    prune_crons_by_enabled_to(inner, false, MAX_CRON_REGISTRY_ENTRIES);
}

fn prune_crons_for_create(inner: &mut CronInner) {
    prune_crons_by_enabled_to(inner, false, MAX_CRON_REGISTRY_ENTRIES.saturating_sub(1));
}

fn prune_crons_by_enabled_to(inner: &mut CronInner, enabled: bool, target_len: usize) {
    let excess = inner.entries.len().saturating_sub(target_len);
    if excess == 0 {
        return;
    }

    let mut candidates: Vec<_> = inner
        .entries
        .values()
        .filter(|entry| entry.enabled == enabled)
        .map(|entry| (entry.updated_at, entry.created_at, entry.cron_id.clone()))
        .collect();
    candidates.sort_unstable();
    for (_, _, cron_id) in candidates.into_iter().take(excess) {
        inner.entries.remove(&cron_id);
        // Prune is a removal too: tombstone it so the on-disk merge does not
        // revive an entry this process just evicted for overflow.
        inner.tombstone(cron_id);
    }
}

fn quarantine_invalid_crons_on_load(inner: &mut CronInner) -> bool {
    let ts = now_secs();
    let mut changed = false;
    let mut quarantined: Vec<String> = Vec::new();
    for entry in inner.entries.values_mut() {
        let trimmed_schedule = entry.schedule.trim().to_string();
        if trimmed_schedule != entry.schedule {
            entry.schedule = trimmed_schedule;
            changed = true;
        }
        let trimmed_prompt = entry.prompt.trim().to_string();
        if trimmed_prompt != entry.prompt {
            entry.prompt = trimmed_prompt;
            changed = true;
        }
        if entry.enabled
            && (validate_cron_schedule(&entry.schedule).is_err() || entry.prompt.is_empty())
        {
            entry.enabled = false;
            entry.updated_at = ts;
            changed = true;
            quarantined.push(entry.cron_id.clone());
        }
    }
    // Stamp quarantined entries with fresh revisions so this sanitizing write
    // wins the on-disk merge instead of tying on whole-second `updated_at`.
    for cron_id in quarantined {
        let rev = inner.bump_revision();
        if let Some(entry) = inner.entries.get_mut(&cron_id) {
            entry.rev = rev;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ── Team tests ──────────────────────────────────────

    #[test]
    fn creates_and_retrieves_team() {
        let registry = TeamRegistry::new_in_memory();
        let team = registry
            .create("Alpha Squad", vec!["task_001".into(), "task_002".into()])
            .expect("create team");
        assert_eq!(team.name, "Alpha Squad");
        assert_eq!(team.task_ids.len(), 2);
        assert_eq!(team.status, TeamStatus::Created);

        let fetched = registry.get(&team.team_id).expect("team should exist");
        assert_eq!(fetched.team_id, team.team_id);
    }

    #[test]
    fn lists_and_deletes_teams() {
        let registry = TeamRegistry::new_in_memory();
        let t1 = registry.create("Team A", vec![]).expect("create team A");
        let t2 = registry.create("Team B", vec![]).expect("create team B");

        let all = registry.list();
        assert_eq!(all.len(), 2);

        let deleted = registry.delete(&t1.team_id).expect("delete should succeed");
        assert_eq!(deleted.status, TeamStatus::Deleted);

        // Team is still listable (soft delete)
        let still_there = registry.get(&t1.team_id).unwrap();
        assert_eq!(still_there.status, TeamStatus::Deleted);

        // Hard remove
        let _ = registry.remove(&t2.team_id);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn rejects_missing_team_operations() {
        let registry = TeamRegistry::new_in_memory();
        assert!(registry.delete("nonexistent").is_err());
        assert!(registry.get("nonexistent").is_none());
    }

    // ── Cron tests ──────────────────────────────────────

    #[test]
    fn creates_and_retrieves_cron() {
        let registry = CronRegistry::new_in_memory();
        let entry = registry
            .create("  0 * * * *  ", "  Check status  ", Some("hourly check"))
            .expect("create cron");
        assert_eq!(entry.schedule, "0 * * * *");
        assert_eq!(entry.prompt, "Check status");
        assert!(entry.enabled);
        assert_eq!(entry.run_count, 0);
        assert!(entry.last_run_at.is_none());

        let fetched = registry.get(&entry.cron_id).expect("cron should exist");
        assert_eq!(fetched.cron_id, entry.cron_id);
    }

    #[test]
    fn rejects_invalid_cron_schedules_before_persisting() {
        let registry = CronRegistry::new_in_memory();
        for schedule in [
            "",
            "not a cron",
            "@daily",
            "0 0 0 0 0 0",
            "* * * * *;rm",
            "foo bar baz qux quux",
            "999 999 999 999 999",
            "*/0 * * * *",
            "1-999 * * * *",
            "0 0 * FOO *",
            "0 0 * JAN-MON *",
            "0 0 31 FEB *",
        ] {
            let error = registry
                .create(schedule, "Check status", None)
                .expect_err("invalid schedule should be rejected");
            assert!(
                error.contains("cron schedule") || error.contains("unsupported characters"),
                "unexpected error for {schedule:?}: {error}"
            );
        }
        assert!(registry.is_empty());
    }

    /// Vixie's day-of-month-OR-day-of-week rule, pinned; and the fields are
    /// read against the zone's wall clock, not UTC. The same instant is a
    /// Monday 09:00 in London and a Monday 18:00 in Seoul, and each expression
    /// matches in its own zone only.
    #[test]
    fn cron_matching_reads_the_zones_wall_clock_with_vixie_semantics() {
        const UTC: i64 = 0;
        const KST: i64 = 9 * 3_600;
        const EST: i64 = -5 * 3_600;
        let monday_9am = 1_704_099_600; // 2024-01-01T09:00:00Z, Monday.
        let sunday_midnight = 1_704_585_600; // 2024-01-07T00:00:00Z, Sunday.

        // Vixie: both day fields restricted → either may match.
        assert!(cron_matches_timestamp_in_zone("0 9 * JAN MON-FRI", monday_9am, UTC).unwrap());
        assert!(!cron_matches_timestamp_in_zone("1 9 * JAN MON-FRI", monday_9am, UTC).unwrap());
        assert!(cron_matches_timestamp_in_zone("0 0 * * 0", sunday_midnight, UTC).unwrap());
        assert!(cron_matches_timestamp_in_zone("0 0 * * 7", sunday_midnight, UTC).unwrap());
        assert!(cron_matches_timestamp_in_zone("0 9 1 * TUE", monday_9am, UTC).unwrap());
        assert!(!cron_matches_timestamp_in_zone("0 9 2 * TUE", monday_9am, UTC).unwrap());

        // The zone, not the instant, decides which hour the expression names.
        assert!(!cron_matches_timestamp_in_zone("0 9 * JAN MON-FRI", monday_9am, KST).unwrap());
        assert!(cron_matches_timestamp_in_zone("0 18 * JAN MON-FRI", monday_9am, KST).unwrap());
        // 2024-01-07T00:00:00Z is already Sunday 09:00 in Seoul, and the day
        // field has to move with the hour.
        assert!(!cron_matches_timestamp_in_zone("0 0 * * 0", sunday_midnight, KST).unwrap());
        assert!(cron_matches_timestamp_in_zone("0 9 * * 0", sunday_midnight, KST).unwrap());
        // West of UTC the wall clock is still on the previous day: that instant
        // is Saturday 19:00 on the 6th in New York, so the Sunday expression
        // must not match and the Saturday one must.
        assert!(!cron_matches_timestamp_in_zone("0 0 * * 0", sunday_midnight, EST).unwrap());
        assert!(cron_matches_timestamp_in_zone("0 19 6 JAN SAT", sunday_midnight, EST).unwrap());
    }

    /// The red of t-2904: a person writes the wall clock they read off their
    /// own screen, so the next minute on that clock is a minute away — not the
    /// zone's offset away. (`21 21 6 9 *` aimed at local 21:21 answered nine
    /// hours late while matching was UTC.)
    #[test]
    fn a_cron_aimed_at_the_next_local_minute_is_due_a_minute_later() {
        let registry = CronRegistry::new_in_memory();
        let now = now_secs();
        let target = floor_to_minute(now).saturating_add(SECONDS_PER_MINUTE);
        let schedule = five_fields_for_wall_clock(target, core_types::date::local_utc_offset_secs());
        let entry = registry
            .create(&schedule, "the next local minute", None)
            .expect("create cron");

        let due = registry
            .next_due_at(&entry.cron_id, now)
            .expect("known cron")
            .expect("the next local minute is within the lookahead");
        assert_eq!(
            due, target,
            "{schedule:?} aims at the next local minute; answered {} s away instead of {}",
            i64::try_from(due).unwrap_or(i64::MAX) - i64::try_from(now).unwrap_or(0),
            target - now
        );
    }

    /// The five fields naming exactly the wall-clock minute `instant` falls in,
    /// for a zone `offset_secs` ahead of UTC. Test-side inverse of the matcher.
    fn five_fields_for_wall_clock(instant: u64, offset_secs: i64) -> String {
        let wall = instant.saturating_add_signed(offset_secs);
        let days = i64::try_from(wall / SECONDS_PER_DAY).expect("day count fits");
        let (_, month, day) = civil_from_days(days);
        let minute = (wall / SECONDS_PER_MINUTE) % 60;
        let hour = (wall / SECONDS_PER_HOUR) % 24;
        format!("{minute} {hour} {day} {month} *")
    }

    #[test]
    fn cron_next_due_and_due_at_coalesce_duplicate_minutes() {
        let registry = CronRegistry::new_in_memory();
        let entry = registry
            .create("*/15 9 * JAN MON", "quarter-hourly", None)
            .expect("create cron");
        let monday_9_15 = 1_704_100_500; // 2024-01-01T09:15:00Z.
        assert_eq!(
            next_due_at_after_in_zone(&entry.schedule, monday_9_15 - 60, 0).unwrap(),
            Some(monday_9_15)
        );
        assert_eq!(
            latest_due_at_between("0 9 * JAN MON", 0, 1_704_099_900, 0).unwrap(),
            Some(1_704_099_600)
        );

        // The newly-created in-memory entry is newer than the fixed timestamp,
        // so use the pure matcher above for historical timestamps and exercise
        // registry idempotency at the current local minute.
        let now = now_secs();
        let due_this_minute = registry.due_at(now);
        for due in due_this_minute {
            assert!(registry
                .record_due_run_at(&due.entry.cron_id, due.due_at)
                .expect("first record should succeed"));
            assert!(!registry
                .record_due_run_at(&due.entry.cron_id, due.due_at)
                .expect("duplicate record should be ignored"));
        }
    }

    #[test]
    fn accepts_supported_vixie_cron_subset() {
        let registry = CronRegistry::new_in_memory();
        for schedule in [
            "*/15 0-23/2 1,15 * 1-5",
            "0 9 * JAN MON-FRI",
            "30 18 * jan,mar sun",
            "0 0 1-31/2 1-12 0,7",
        ] {
            registry
                .create(schedule, "supported", None)
                .expect("supported cron subset should be accepted");
        }
        assert_eq!(registry.len(), 4);
    }

    #[test]
    fn rejects_empty_cron_prompt_before_persisting() {
        let registry = CronRegistry::new_in_memory();
        let error = registry
            .create("0 * * * *", "  ", None)
            .expect_err("empty prompt should be rejected");
        assert!(error.contains("cron prompt must not be empty"));
        assert!(registry.is_empty());
    }

    #[test]
    fn cron_list_is_stable_by_creation_order() {
        let registry = CronRegistry::new_in_memory();
        let first = registry
            .create("0 * * * *", "first", None)
            .expect("create first");
        let second = registry
            .create("5 * * * *", "second", None)
            .expect("create second");
        let third = registry
            .create("10 * * * *", "third", None)
            .expect("create third");

        let ids = registry
            .list(false)
            .into_iter()
            .map(|entry| entry.cron_id)
            .collect::<Vec<_>>();

        assert_eq!(ids, vec![first.cron_id, second.cron_id, third.cron_id]);
    }

    #[test]
    fn lists_with_enabled_filter() {
        let registry = CronRegistry::new_in_memory();
        let c1 = registry
            .create("* * * * *", "Task 1", None)
            .expect("create cron 1");
        let c2 = registry
            .create("0 * * * *", "Task 2", None)
            .expect("create cron 2");
        registry
            .disable(&c1.cron_id)
            .expect("disable should succeed");

        let all = registry.list(false);
        assert_eq!(all.len(), 2);

        let enabled_only = registry.list(true);
        assert_eq!(enabled_only.len(), 1);
        assert_eq!(enabled_only[0].cron_id, c2.cron_id);
    }

    #[test]
    fn deletes_cron_entry() {
        let registry = CronRegistry::new_in_memory();
        let entry = registry
            .create("* * * * *", "To delete", None)
            .expect("create cron");
        let deleted = registry
            .delete(&entry.cron_id)
            .expect("delete should succeed");
        assert_eq!(deleted.cron_id, entry.cron_id);
        assert!(registry.get(&entry.cron_id).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn records_cron_runs() {
        let registry = CronRegistry::new_in_memory();
        let entry = registry
            .create("*/5 * * * *", "Recurring", None)
            .expect("create recurring cron");
        registry.record_run(&entry.cron_id).unwrap();
        registry.record_run(&entry.cron_id).unwrap();

        let fetched = registry.get(&entry.cron_id).unwrap();
        assert_eq!(fetched.run_count, 2);
        assert!(fetched.last_run_at.is_some());
    }

    #[test]
    fn rejects_missing_cron_operations() {
        let registry = CronRegistry::new_in_memory();
        assert!(registry.delete("nonexistent").is_err());
        assert!(registry.disable("nonexistent").is_err());
        assert!(registry.record_run("nonexistent").is_err());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn team_status_display_all_variants() {
        // given
        let cases = [
            (TeamStatus::Created, "created"),
            (TeamStatus::Running, "running"),
            (TeamStatus::Completed, "completed"),
            (TeamStatus::Deleted, "deleted"),
        ];

        // when
        let rendered: Vec<_> = cases
            .into_iter()
            .map(|(status, expected)| (status.to_string(), expected))
            .collect();

        // then
        assert_eq!(
            rendered,
            vec![
                ("created".to_string(), "created"),
                ("running".to_string(), "running"),
                ("completed".to_string(), "completed"),
                ("deleted".to_string(), "deleted"),
            ]
        );
    }

    #[test]
    fn new_team_registry_is_empty() {
        // given
        let registry = TeamRegistry::new_in_memory();

        // when
        let teams = registry.list();

        // then
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(teams.is_empty());
    }

    #[test]
    fn team_remove_nonexistent_returns_none() {
        // given
        let registry = TeamRegistry::new_in_memory();

        // when
        let removed = registry.remove("missing");

        // then
        assert!(removed.is_none());
    }

    #[test]
    fn team_len_transitions() {
        // given
        let registry = TeamRegistry::new_in_memory();

        // when
        let alpha = registry.create("Alpha", vec![]).expect("create alpha");
        let beta = registry.create("Beta", vec![]).expect("create beta");
        let after_create = registry.len();
        let _ = registry.remove(&alpha.team_id);
        let after_first_remove = registry.len();
        let _ = registry.remove(&beta.team_id);

        // then
        assert_eq!(after_create, 2);
        assert_eq!(after_first_remove, 1);
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
    }

    #[test]
    fn team_registry_prunes_deleted_records_first() {
        let registry = TeamRegistry::new_in_memory();
        let active = registry.create("active", vec![]).expect("create active");

        for i in 0..(MAX_TEAM_REGISTRY_ENTRIES + 2) {
            let team = registry
                .create(&format!("deleted {i}"), vec![])
                .expect("create deleted team");
            registry
                .delete(&team.team_id)
                .expect("delete should succeed");
        }

        assert!(registry.len() <= MAX_TEAM_REGISTRY_ENTRIES);
        assert!(
            registry.get(&active.team_id).is_some(),
            "non-deleted team must not be pruned"
        );
    }

    #[test]
    fn team_registry_prunes_completed_records_after_deleted() {
        let registry = TeamRegistry::new_in_memory();
        let active = registry.create("active", vec![]).expect("create active");
        let mut completed_ids = Vec::new();

        for i in 0..(MAX_TEAM_REGISTRY_ENTRIES + 2) {
            let team = registry
                .create(&format!("completed {i}"), vec![])
                .expect("create completed team");
            completed_ids.push(team.team_id.clone());
            {
                let mut inner = lock_recovered(&registry.inner);
                inner.teams.get_mut(&team.team_id).unwrap().status = TeamStatus::Completed;
                prune_terminal_teams(&mut inner);
            }
        }

        assert!(registry.len() <= MAX_TEAM_REGISTRY_ENTRIES);
        assert!(registry.get(&active.team_id).is_some());
        assert!(
            completed_ids.iter().any(|id| registry.get(id).is_none()),
            "old completed teams should be eligible for pruning"
        );
    }

    #[test]
    fn team_registry_rejects_create_when_all_entries_are_live() {
        let registry = TeamRegistry::new_in_memory();
        for i in 0..MAX_TEAM_REGISTRY_ENTRIES {
            registry
                .create(&format!("live {i}"), vec![])
                .expect("create live team");
        }

        let error = registry
            .create("overflow", vec![])
            .expect_err("all-live registry should reject new teams");

        assert_eq!(registry.len(), MAX_TEAM_REGISTRY_ENTRIES);
        assert!(error.contains("team registry is full"));
    }

    #[test]
    fn cron_registry_prunes_disabled_entries_before_active() {
        let registry = CronRegistry::new_in_memory();
        let active = registry
            .create("* * * * *", "active", None)
            .expect("create active cron");
        let mut disabled_ids = Vec::new();

        for i in 0..(MAX_CRON_REGISTRY_ENTRIES + 2) {
            let cron = registry
                .create("0 * * * *", &format!("disabled {i}"), None)
                .expect("create disabled cron");
            disabled_ids.push(cron.cron_id.clone());
            registry.disable(&cron.cron_id).unwrap();
        }

        assert!(registry.len() <= MAX_CRON_REGISTRY_ENTRIES);
        assert!(registry.get(&active.cron_id).is_some());
        assert!(
            disabled_ids.iter().any(|id| registry.get(id).is_none()),
            "disabled crons should be pruned first"
        );
    }

    #[test]
    fn cron_registry_rejects_create_when_all_entries_are_enabled() {
        let registry = CronRegistry::new_in_memory();
        let first = registry
            .create("* * * * *", "first", None)
            .expect("create first cron");

        for i in 0..(MAX_CRON_REGISTRY_ENTRIES - 1) {
            registry
                .create("* * * * *", &format!("active {i}"), None)
                .expect("create active cron");
        }

        let error = registry
            .create("* * * * *", "overflow", None)
            .expect_err("all-enabled crons should reject, not silently delete live schedules");

        assert_eq!(registry.len(), MAX_CRON_REGISTRY_ENTRIES);
        assert!(registry.get(&first.cron_id).is_some());
        assert!(error.contains("cron registry is full"));
    }

    #[test]
    fn cron_list_all_disabled_returns_empty_for_enabled_only() {
        // given
        let registry = CronRegistry::new_in_memory();
        let first = registry
            .create("* * * * *", "Task 1", None)
            .expect("create first cron");
        let second = registry
            .create("0 * * * *", "Task 2", None)
            .expect("create second cron");
        registry
            .disable(&first.cron_id)
            .expect("disable should succeed");
        registry
            .disable(&second.cron_id)
            .expect("disable should succeed");

        // when
        let enabled_only = registry.list(true);
        let all_entries = registry.list(false);

        // then
        assert!(enabled_only.is_empty());
        assert_eq!(all_entries.len(), 2);
    }

    #[test]
    fn cron_create_without_description() {
        // given
        let registry = CronRegistry::new_in_memory();

        // when
        let entry = registry
            .create("*/15 * * * *", "Check health", None)
            .expect("create health cron");

        // then
        assert!(entry.cron_id.starts_with("cron_"));
        assert_eq!(entry.description, None);
        assert!(entry.enabled);
        assert_eq!(entry.run_count, 0);
        assert_eq!(entry.last_run_at, None);
    }

    #[test]
    fn new_cron_registry_is_empty() {
        // given
        let registry = CronRegistry::new_in_memory();

        // when
        let enabled_only = registry.list(true);
        let all_entries = registry.list(false);

        // then
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(enabled_only.is_empty());
        assert!(all_entries.is_empty());
    }

    #[test]
    fn cron_record_run_updates_timestamp_and_counter() {
        // given
        let registry = CronRegistry::new_in_memory();
        let entry = registry
            .create("*/5 * * * *", "Recurring", None)
            .expect("create recurring cron");

        // when
        registry
            .record_run(&entry.cron_id)
            .expect("first run should succeed");
        registry
            .record_run(&entry.cron_id)
            .expect("second run should succeed");
        let fetched = registry.get(&entry.cron_id).expect("entry should exist");

        // then
        assert_eq!(fetched.run_count, 2);
        assert!(fetched.last_run_at.is_some());
        assert!(fetched.updated_at >= entry.updated_at);
    }

    #[test]
    fn cron_disable_updates_timestamp() {
        // given
        let registry = CronRegistry::new_in_memory();
        let entry = registry
            .create("0 0 * * *", "Nightly", None)
            .expect("create nightly cron");

        // when
        registry
            .disable(&entry.cron_id)
            .expect("disable should succeed");
        let fetched = registry.get(&entry.cron_id).expect("entry should exist");

        // then
        assert!(!fetched.enabled);
        assert!(fetched.updated_at >= entry.updated_at);
    }

    #[test]
    fn persists_teams_to_disk_when_configured() {
        let root = std::env::temp_dir().join(format!("team-registry-test-{}", now_secs()));
        let path = root.join("teams.json");
        let registry = TeamRegistry::with_persistence_path(Some(path.clone()));
        let created = registry
            .create("Durable Team", vec!["task_1".into()])
            .expect("create durable team");

        let restored = TeamRegistry::with_persistence_path(Some(path.clone()));
        let fetched = restored.get(&created.team_id).expect("team should reload");
        assert_eq!(fetched.name, "Durable Team");
        assert_eq!(fetched.task_ids, vec!["task_1"]);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn persisted_invalid_crons_are_disabled_on_load() {
        let root = std::env::temp_dir().join(format!(
            "cron-registry-invalid-load-test-{}-{}",
            now_secs(),
            std::process::id()
        ));
        let path = root.join("crons.json");
        fs::create_dir_all(&root).expect("create temp root");
        fs::write(
            &path,
            serde_json::json!({
                "counter": 2,
                "entries": {
                    "cron_bad": {
                        "cron_id": "cron_bad",
                        "schedule": "foo bar baz qux quux",
                        "prompt": "bad persisted cron",
                        "description": null,
                        "enabled": true,
                        "created_at": 1,
                        "updated_at": 1,
                        "last_run_at": null,
                        "run_count": 0
                    },
                    "cron_empty_prompt": {
                        "cron_id": "cron_empty_prompt",
                        "schedule": "0 * * * *",
                        "prompt": "  ",
                        "description": null,
                        "enabled": true,
                        "created_at": 2,
                        "updated_at": 2,
                        "last_run_at": null,
                        "run_count": 0
                    },
                    "cron_good": {
                        "cron_id": "cron_good",
                        "schedule": "  */5 * * * *  ",
                        "prompt": "  good persisted cron  ",
                        "description": null,
                        "enabled": true,
                        "created_at": 3,
                        "updated_at": 3,
                        "last_run_at": null,
                        "run_count": 0
                    }
                }
            })
            .to_string(),
        )
        .expect("write invalid registry");

        let registry = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(!registry.get("cron_bad").expect("bad cron").enabled);
        assert!(
            !registry
                .get("cron_empty_prompt")
                .expect("empty prompt cron")
                .enabled
        );
        let good = registry.get("cron_good").expect("good cron");
        assert!(good.enabled);
        assert_eq!(good.schedule, "*/5 * * * *");
        assert_eq!(good.prompt, "good persisted cron");

        let restored = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(!restored.get("cron_bad").expect("bad cron").enabled);
        assert!(restored.get("cron_good").expect("good cron").enabled);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn persists_crons_to_disk_when_configured() {
        let root = std::env::temp_dir().join(format!("cron-registry-test-{}", now_secs()));
        let path = root.join("crons.json");
        let registry = CronRegistry::with_persistence_path(Some(path.clone()));
        let created = registry
            .create("*/10 * * * *", "Durable cron", Some("persisted"))
            .expect("create durable cron");
        registry
            .record_run(&created.cron_id)
            .expect("run should succeed");

        let restored = CronRegistry::with_persistence_path(Some(path.clone()));
        let fetched = restored.get(&created.cron_id).expect("cron should reload");
        assert_eq!(fetched.prompt, "Durable cron");
        assert_eq!(fetched.run_count, 1);

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    fn unique_suffix() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        format!(
            "{}-{}-{}",
            std::process::id(),
            now_secs(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn removed_team_is_not_resurrected_by_on_disk_merge() {
        let root = std::env::temp_dir().join(format!("team-remove-nores-{}", unique_suffix()));
        let path = root.join("teams.json");

        // A prior process persisted this team to disk.
        let writer = TeamRegistry::with_persistence_path(Some(path.clone()));
        let created = writer.create("resurrect?", Vec::new()).expect("create team");
        drop(writer);

        // Load that on-disk copy, then hard-remove. The remove's read/merge/
        // write reloads the on-disk copy that still holds the team; a plain
        // union merge would revive it.
        let remover = TeamRegistry::with_persistence_path(Some(path.clone()));
        assert!(remover.get(&created.team_id).is_some());
        assert!(remover.remove(&created.team_id).is_some());
        drop(remover);

        let reloaded = TeamRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&created.team_id).is_none(),
            "removed team was resurrected from disk"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn pruned_teams_are_not_resurrected_by_on_disk_merge() {
        let root = std::env::temp_dir().join(format!("team-prune-nores-{}", unique_suffix()));
        let path = root.join("teams.json");

        let registry = TeamRegistry::with_persistence_path(Some(path.clone()));
        // Overflow the deleted-team cap so the oldest deleted teams are pruned.
        let mut ids = Vec::new();
        for i in 0..(MAX_TEAM_REGISTRY_ENTRIES + 4) {
            let team = registry
                .create(&format!("team {i}"), Vec::new())
                .expect("create team");
            registry.delete(&team.team_id).expect("soft delete team");
            ids.push(team.team_id);
        }
        let pruned: Vec<String> = ids
            .iter()
            .filter(|id| registry.get(id).is_none())
            .cloned()
            .collect();
        assert!(!pruned.is_empty(), "expected some teams to be pruned");
        drop(registry);

        let reloaded = TeamRegistry::with_persistence_path(Some(path.clone()));
        for id in &pruned {
            assert!(
                reloaded.get(id).is_none(),
                "pruned team {id} was resurrected from disk"
            );
        }

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn deleted_cron_is_not_resurrected_by_on_disk_merge() {
        let root = std::env::temp_dir().join(format!("cron-delete-nores-{}", unique_suffix()));
        let path = root.join("crons.json");

        // A prior process persisted this cron to disk.
        let writer = CronRegistry::with_persistence_path(Some(path.clone()));
        let created = writer
            .create("*/5 * * * *", "resurrect?", None)
            .expect("create cron");
        drop(writer);

        // Load that on-disk copy, then delete. The delete's read/merge/write
        // reloads the on-disk copy that still holds the entry; a plain union
        // merge would revive it.
        let remover = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(remover.get(&created.cron_id).is_some());
        remover.delete(&created.cron_id).expect("delete cron");
        drop(remover);

        let reloaded = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&created.cron_id).is_none(),
            "deleted cron was resurrected from disk"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn pruned_crons_are_not_resurrected_by_on_disk_merge() {
        let root = std::env::temp_dir().join(format!("cron-prune-nores-{}", unique_suffix()));
        let path = root.join("crons.json");

        let registry = CronRegistry::with_persistence_path(Some(path.clone()));
        // Overflow the disabled-cron cap so the oldest disabled crons prune.
        let mut ids = Vec::new();
        for i in 0..(MAX_CRON_REGISTRY_ENTRIES + 4) {
            let created = registry
                .create("*/5 * * * *", &format!("cron {i}"), None)
                .expect("create cron");
            registry.disable(&created.cron_id).expect("disable cron");
            ids.push(created.cron_id);
        }
        let pruned: Vec<String> = ids
            .iter()
            .filter(|id| registry.get(id).is_none())
            .cloned()
            .collect();
        assert!(!pruned.is_empty(), "expected some crons to be pruned");
        drop(registry);

        let reloaded = CronRegistry::with_persistence_path(Some(path.clone()));
        for id in &pruned {
            assert!(
                reloaded.get(id).is_none(),
                "pruned cron {id} was resurrected from disk"
            );
        }

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn two_team_registries_on_same_base_create_distinct_ids() {
        let root = std::env::temp_dir().join(format!("team-concurrent-create-{}", unique_suffix()));
        let path = root.join("teams.json");

        // A and B both open the SAME on-disk registry with no prior state, so
        // they share the same base revision (0) and same counter (0). Before the
        // fix, both minted `team_<ts>_1` in the same second — an id collision
        // that silently collapsed the two creates into one on reload. The create
        // now allocates its counter/revision INSIDE the cross-process lock, after
        // merging the peer's committed write, so B rebases onto A's counter.
        let a = TeamRegistry::with_persistence_path(Some(path.clone()));
        let b = TeamRegistry::with_persistence_path(Some(path.clone()));

        let team_a = a.create("alpha", Vec::new()).expect("A create team");
        let team_b = b.create("bravo", Vec::new()).expect("B create team");

        assert_ne!(
            team_a.team_id, team_b.team_id,
            "concurrent creates on the same base produced a colliding team id"
        );

        // Both survive a fresh reload from disk: neither create was overwritten.
        let reloaded = TeamRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&team_a.team_id).is_some(),
            "team A was lost by a same-base concurrent create"
        );
        assert!(
            reloaded.get(&team_b.team_id).is_some(),
            "team B was lost by a same-base concurrent create"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn two_cron_registries_on_same_base_create_distinct_ids() {
        let root = std::env::temp_dir().join(format!("cron-concurrent-create-{}", unique_suffix()));
        let path = root.join("crons.json");

        // Same interleaving as the team case: A and B share base revision 0 and
        // counter 0 and both create in the same second.
        let a = CronRegistry::with_persistence_path(Some(path.clone()));
        let b = CronRegistry::with_persistence_path(Some(path.clone()));

        let cron_a = a
            .create("*/5 * * * *", "alpha", None)
            .expect("A create cron");
        let cron_b = b
            .create("*/7 * * * *", "bravo", None)
            .expect("B create cron");

        assert_ne!(
            cron_a.cron_id, cron_b.cron_id,
            "concurrent creates on the same base produced a colliding cron id"
        );

        let reloaded = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&cron_a.cron_id).is_some(),
            "cron A was lost by a same-base concurrent create"
        );
        assert!(
            reloaded.get(&cron_b.cron_id).is_some(),
            "cron B was lost by a same-base concurrent create"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn same_base_concurrent_cron_updates_serialize_without_tie_overwrite() {
        let root = std::env::temp_dir().join(format!("cron-concurrent-update-{}", unique_suffix()));
        let path = root.join("crons.json");

        // Seed one entry, then open two registries that both loaded that same
        // base revision.
        let seed = CronRegistry::with_persistence_path(Some(path.clone()));
        let entry = seed
            .create("*/5 * * * *", "seed", None)
            .expect("seed create cron");
        let base_rev = seed.get(&entry.cron_id).expect("seed present").rev;
        drop(seed);

        let a = CronRegistry::with_persistence_path(Some(path.clone()));
        let b = CronRegistry::with_persistence_path(Some(path.clone()));
        assert_eq!(a.get(&entry.cron_id).expect("A sees entry").rev, base_rev);
        assert_eq!(b.get(&entry.cron_id).expect("B sees entry").rev, base_rev);

        // Both mutate the same entry. Before the fix each stamped rev = base + 1
        // and the merge tie-rule kept `self`, so B silently overwrote A. With
        // the lock-serialized rebase, B allocates its revision after merging A's
        // committed write, so the entry's revision strictly advances by two.
        a.record_run(&entry.cron_id).expect("A record_run");
        b.record_run(&entry.cron_id).expect("B record_run");

        let reloaded = CronRegistry::with_persistence_path(Some(path.clone()));
        let final_entry = reloaded.get(&entry.cron_id).expect("entry survives");
        assert_eq!(
            final_entry.rev,
            base_rev + 2,
            "same-base concurrent updates did not serialize; a tie silently overwrote one write"
        );
        // Both runs are accounted for rather than one being dropped.
        assert_eq!(
            final_entry.run_count, 2,
            "a concurrent record_run was lost to a silent tie overwrite"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    /// Force the atomic write to fail WITHOUT destroying any existing committed
    /// file: make the parent directory read-only so the same-directory temp file
    /// cannot be created. The peer's already-persisted file is left intact.
    #[cfg(unix)]
    fn make_registry_path_unwritable(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let parent = path.parent().expect("registry path has a parent");
        fs::create_dir_all(parent).expect("ensure parent exists");
        fs::set_permissions(parent, fs::Permissions::from_mode(0o555))
            .expect("make parent read-only to force write failure");
    }

    #[cfg(unix)]
    fn restore_registry_path(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let parent = path.parent().expect("registry path has a parent");
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o755));
    }

    #[test]
    fn stale_peer_cannot_resurrect_a_removed_team_after_many_removals() {
        // Tombstones are unbounded and durable: no volume of later removals can
        // age out a team's tombstone, so a stale peer holding the removed team on
        // disk cannot revive it.
        let root = std::env::temp_dir().join(format!("team-tomb-resurrect-{}", unique_suffix()));
        let path = root.join("teams.json");

        let live = TeamRegistry::with_persistence_path(Some(path.clone()));
        let victim = live.create("victim", Vec::new()).expect("create victim");
        let victim_id = victim.team_id.clone();

        let stale = TeamRegistry::with_persistence_path(Some(path.clone()));
        assert!(stale.get(&victim_id).is_some(), "stale peer must see the victim");

        live.remove(&victim_id).expect("remove victim");
        for i in 0..(MAX_TEAM_REGISTRY_ENTRIES * 4) {
            let t = live.create(&format!("filler {i}"), Vec::new()).expect("create filler");
            live.remove(&t.team_id).expect("remove filler");
        }
        assert!(live.get(&victim_id).is_none(), "victim removed from live");

        // The stale peer performs any persisting mutation, re-merging disk; the
        // durable tombstone must keep the victim dead.
        let _ = stale.create("stale-write", Vec::new());

        let reloaded = TeamRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&victim_id).is_none(),
            "a removed team was resurrected by a stale peer after many removals"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[test]
    fn stale_peer_cannot_resurrect_a_deleted_cron_after_many_deletions() {
        let root = std::env::temp_dir().join(format!("cron-tomb-resurrect-{}", unique_suffix()));
        let path = root.join("crons.json");

        let live = CronRegistry::with_persistence_path(Some(path.clone()));
        let victim = live.create("*/5 * * * *", "victim", None).expect("create victim");
        let victim_id = victim.cron_id.clone();

        let stale = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(stale.get(&victim_id).is_some(), "stale peer must see the victim");

        live.delete(&victim_id).expect("delete victim");
        for i in 0..(MAX_CRON_REGISTRY_ENTRIES * 4) {
            let c = live
                .create("*/7 * * * *", &format!("filler {i}"), None)
                .expect("create filler");
            live.delete(&c.cron_id).expect("delete filler");
        }
        assert!(live.get(&victim_id).is_none(), "victim deleted from live");

        let _ = stale.create("*/9 * * * *", "stale-write", None);

        let reloaded = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&victim_id).is_none(),
            "a deleted cron was resurrected by a stale peer after many deletions"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn failed_team_create_and_remove_stay_truthful_locally_and_never_reach_disk() {
        // Best-effort team API contract under fail-closed persistence: create
        // returns a Team visible locally; remove returns Some and actually
        // removes locally; neither reaches disk, so a peer is never clobbered.
        let root = std::env::temp_dir().join(format!("team-failclosed-{}", unique_suffix()));
        let path = root.join("teams.json");

        let peer = TeamRegistry::with_persistence_path(Some(path.clone()));
        let peer_team = peer.create("peer", Vec::new()).expect("peer create");
        let peer_id = peer_team.team_id.clone();
        drop(peer);

        let reg = TeamRegistry::with_persistence_path(Some(path.clone()));
        make_registry_path_unwritable(&path);

        let created = reg.create("local", Vec::new()).expect("create returns a team");
        assert!(
            reg.get(&created.team_id).is_some(),
            "a failed team create must still be visible locally"
        );
        let removed = reg.remove(&created.team_id);
        assert!(removed.is_some(), "remove must report the removed team");
        assert!(
            reg.get(&created.team_id).is_none(),
            "a failed team remove must still remove locally"
        );
        restore_registry_path(&path);

        let reloaded = TeamRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&peer_id).is_some(),
            "a peer's committed team was clobbered by a latched instance"
        );
        assert!(
            reloaded.get(&created.team_id).is_none(),
            "a latched instance's memory-only team must never reach disk"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    #[cfg(unix)]
    #[test]
    fn failed_cron_create_is_truthful_locally_and_delete_reports_error() {
        // Best-effort cron create stays visible locally; a status-aware mutation
        // (delete) surfaces the persistence failure as an Err. Neither reaches
        // disk, so a peer is never clobbered.
        let root = std::env::temp_dir().join(format!("cron-failclosed-{}", unique_suffix()));
        let path = root.join("crons.json");

        let peer = CronRegistry::with_persistence_path(Some(path.clone()));
        let peer_cron = peer.create("*/5 * * * *", "peer", None).expect("peer create");
        let peer_id = peer_cron.cron_id.clone();
        drop(peer);

        let reg = CronRegistry::with_persistence_path(Some(path.clone()));
        make_registry_path_unwritable(&path);

        let created = reg.create("*/7 * * * *", "local", None).expect("create returns a cron");
        assert!(
            reg.get(&created.cron_id).is_some(),
            "a failed cron create must still be visible locally"
        );
        // A status-aware mutation surfaces the persistence failure.
        assert!(
            reg.record_run(&created.cron_id).is_err(),
            "a status-aware mutation must report the persistence failure as an error"
        );
        restore_registry_path(&path);

        let reloaded = CronRegistry::with_persistence_path(Some(path.clone()));
        assert!(
            reloaded.get(&peer_id).is_some(),
            "a peer's committed cron was clobbered by a latched instance"
        );
        assert!(
            reloaded.get(&created.cron_id).is_none(),
            "a latched instance's memory-only cron must never reach disk"
        );

        fs::remove_dir_all(root).expect("cleanup temp dir");
    }

    // ── named entries and the idempotent ensure (t-2927) ──────────────────

    #[test]
    fn ensure_named_is_idempotent_and_replaces_rather_than_doubling() {
        let registry = CronRegistry::new_in_memory();
        let name = "zerocode:second-brain:weekly-review";

        let (first, outcome) = registry
            .ensure_named(name, "0 21 * * 6", "review the week")
            .expect("first ensure");
        assert_eq!(outcome, CronEnsure::Created);
        assert_eq!(first.description.as_deref(), Some(name));
        assert!(first.enabled);

        let (again, outcome) = registry
            .ensure_named(name, "0 21 * * 6", "review the week")
            .expect("second ensure");
        assert_eq!(outcome, CronEnsure::Unchanged);
        assert_eq!(again.cron_id, first.cron_id, "the same words keep the same record");
        assert_eq!(registry.find_named(name).len(), 1);
        assert_eq!(registry.len(), 1);

        let (replaced, outcome) = registry
            .ensure_named(name, "0 22 * * 6", "review the week")
            .expect("ensure with a new schedule");
        assert_eq!(outcome, CronEnsure::Replaced);
        assert_ne!(replaced.cron_id, first.cron_id);
        assert_eq!(replaced.schedule, "0 22 * * 6");
        assert_eq!(registry.find_named(name).len(), 1, "one name, one record");
        assert!(registry.get(&first.cron_id).is_none(), "the old record is gone");

        // Another owner's record with another name is left alone.
        registry
            .create("* * * * *", "somebody else's", Some("other"))
            .expect("other");
        assert_eq!(registry.remove_named(name).expect("remove"), 1);
        assert_eq!(registry.find_named(name).len(), 0);
        assert_eq!(registry.remove_named(name).expect("remove again"), 0);
        assert_eq!(registry.len(), 1);
        assert!(registry.ensure_named(name, "not a schedule", "x").is_err());
    }

    #[test]
    fn a_workspace_registry_is_the_file_a_session_started_there_reads() {
        let root = tempfile::tempdir().expect("tempdir");
        let name = "zerocode:second-brain:weekly-review";
        let outside = CronRegistry::for_workspace(root.path());
        assert_eq!(
            outside.persistence_path(),
            Some(root.path().join(".zo/registries/crons.json").as_path())
        );
        outside
            .ensure_named(name, "0 21 * * 6", "review the week")
            .expect("ensure from outside");

        let inside = CronRegistry::with_persistence_path(Some(
            root.path().join(".zo").join("registries").join("crons.json"),
        ));
        let found = inside.find_named(name);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].prompt, "review the week");
        // The same words from the other side: still one record.
        let (_, outcome) = inside
            .ensure_named(name, "0 21 * * 6", "review the week")
            .expect("ensure from inside");
        assert_eq!(outcome, CronEnsure::Unchanged);
        assert_eq!(CronRegistry::for_workspace(root.path()).find_named(name).len(), 1);
    }
}
