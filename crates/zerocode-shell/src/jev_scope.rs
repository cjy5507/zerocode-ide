//! Where the Jev dashboard counts, and every project's numbers summed
//! (t-9091).
//!
//! zo counts one project at a time (`zo jev summary --cwd <dir>`). A seat zo
//! writes keeps its rows under that project's own state; a seat this window
//! writes keeps them in one place on this machine (`~/.zo/jev`, and the
//! window's Computer Use sessions), read the same whichever project asks. On
//! 2026-09-25 the dashboard was opened on a worker's checkout, which had no zo
//! records of its own, and drew every zo seat at zero while another project
//! had thirteen seats' rows that day — and nothing on it said which project it
//! had counted. So a reading names its scope, the projects that have records
//! when the one it counted has none, and — asked — the sum over every project,
//! the machine's rows counted once.

use crate::typesafe_settings::SeatNumbers;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// zo's layout of one project's records, as its runtime spells it — the
/// per-project state (`zo_project_state_dir`: `<zo home>/projects/<slug>/state`),
/// the folder every zo seat appends its ledger in (`JEV_LEDGER_DIR`), and the
/// workspace a session recorded beside its transcript (`session_cwd_path`,
/// `{"cwd": …}`). Held to zo's source by
/// `tests::the_layout_read_here_is_the_one_zo_writes`.
pub const ZO_PROJECTS_DIR: &str = "projects";
pub const ZO_PROJECT_STATE_DIR: &str = "state";
pub const JEV_LEDGER_DIR: &str = "smart-router";
pub const ZO_SESSIONS_DIR: &str = "sessions";
pub const SESSION_CWD_EXTENSION: &str = "cwd";
pub const LEDGER_EXTENSION: &str = "jsonl";

/// The days a record counts as recent: zo's own window over every seat
/// (`jev_summary::WINDOW_DAYS`, sent as `windowDays`), held to it by the same
/// test.
pub const WINDOW_DAYS: i64 = 7;
const MS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// How many projects' zo run at once for the every-project sum. One run is
/// one short process, and the sum waits for the slowest of each round:
/// measured over this machine's seven projects with records (2026-09-25),
/// 0.03 to 0.09 s each at a load near 10 (0.11 to 0.25 s near 20), and the
/// seven in 0.34 s one at a time against 0.12 s four at a time.
pub const PROJECT_ASKS_AT_ONCE: usize = 4;

/// Which numbers the dashboard asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    /// The checkout the person is looking at — zo's `--cwd`.
    #[default]
    Workspace,
    /// Every project with zo records in the window, summed.
    Projects,
}

/// Where a seat's rows are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Reach {
    /// Under the project's own zo state: each project has its own.
    Project,
    /// In this machine's one place ([`machine_places`]): the same rows
    /// whichever project asks.
    Machine,
}

/// A seat across the projects summed (t-9091): how many were counted, in how
/// many it acts, and whether its numbers are a sum of more than one reading —
/// when they are, a project's own judgment of its own rows (its verdict, its
/// judged window, its latency) is not carried ([`Carry::Own`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatAcross {
    pub projects: usize,
    pub applying: usize,
    pub summed: bool,
}

/// One project with zo records.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevProject {
    /// The workspace its sessions recorded.
    pub path: String,
    /// Its last folder's name — what a person calls it.
    pub name: String,
    /// When its newest record was written, ms since the epoch.
    pub newest_ms: i64,
}

/// What one reading counted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevScope {
    pub scope: Scope,
    /// The checkout the dashboard stands in, and its name.
    pub workspace: String,
    pub workspace_name: String,
    /// Whether that checkout has zo records of its own in the window.
    pub recorded: bool,
    /// The projects with zo records in the window, newest record first — read
    /// when the checkout has none, and the ones the every-project sum counted.
    pub projects: Vec<JevProject>,
    pub window_days: i64,
}

/// The dashboard's reading: what it counted, and the seats.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct JevReading {
    pub scope: JevScope,
    pub seats: Vec<SeatNumbers>,
}

/// Where a reading looks, handed in so a test reads a folder of its own
/// rather than the person's `~/.zo`.
pub struct Places {
    /// zo's config home (`~/.zo`, or `ZO_CONFIG_HOME`/`ZO_HOME`).
    pub zo_home: PathBuf,
    /// The window's Computer Use sessions, handed to zo as `--computer-use`.
    pub sessions: PathBuf,
    /// The machine's temporary roots ([`temporary_roots`]).
    pub temporary: Vec<PathBuf>,
}

/// Read what `scope` asks, with `ask` running zo over one project's
/// workspace. The checkout's own reading says whether it has records, and
/// only when it has none are the projects that do looked for. The
/// every-project sum asks each of them, [`PROJECT_ASKS_AT_ONCE`] at a time,
/// and counts the ones that answered; with no project to count it is the
/// checkout's own reading, summed over nothing.
///
/// # Errors
/// The checkout's zo failing, or every project's.
pub fn read(
    ask: &(dyn Fn(&Path) -> Result<Vec<SeatNumbers>, String> + Sync),
    places: &Places,
    workspace: &Path,
    scope: Scope,
    now_ms: i64,
) -> Result<JevReading, String> {
    let machine = machine_places(&places.zo_home, &places.sessions);
    let looked = || recorded_projects(&places.zo_home, now_ms, &places.temporary);
    let own = || -> Result<(Vec<SeatNumbers>, bool), String> {
        let seats = with_reach(ask(workspace)?, &machine);
        let recorded = has_own_records(&seats);
        Ok((seats, recorded))
    };
    let (seats, recorded, projects) = match scope {
        Scope::Workspace => {
            let (seats, recorded) = own()?;
            let projects = if recorded { Vec::new() } else { looked() };
            (seats, recorded, projects)
        }
        Scope::Projects => {
            let listed = looked();
            if listed.is_empty() {
                let (seats, recorded) = own()?;
                (seats, recorded, listed)
            } else {
                let answered = ask_each(ask, &listed)?;
                let recorded = answered
                    .iter()
                    .any(|(project, _)| Path::new(&project.path) == workspace);
                let readings: Vec<Vec<SeatNumbers>> = answered
                    .iter()
                    .map(|(_, seats)| with_reach(seats.clone(), &machine))
                    .collect();
                let projects = answered.into_iter().map(|(project, _)| project).collect();
                (summed(&readings)?, recorded, projects)
            }
        }
    };
    Ok(JevReading {
        scope: JevScope {
            scope,
            workspace: workspace.to_string_lossy().into_owned(),
            workspace_name: name_of(workspace),
            recorded,
            projects,
            window_days: WINDOW_DAYS,
        },
        seats,
    })
}

/// Every listed project's zo, [`PROJECT_ASKS_AT_ONCE`] at a time, each with
/// the project it counted. A project whose zo failed — its workspace moved
/// between the look and the ask — is left out.
fn ask_each(
    ask: &(dyn Fn(&Path) -> Result<Vec<SeatNumbers>, String> + Sync),
    listed: &[JevProject],
) -> Result<Vec<(JevProject, Vec<SeatNumbers>)>, String> {
    let mut answered = Vec::new();
    let mut failed = None;
    for round in listed.chunks(PROJECT_ASKS_AT_ONCE) {
        let results: Vec<Result<Vec<SeatNumbers>, String>> = std::thread::scope(|threads| {
            let running: Vec<_> = round
                .iter()
                .map(|project| threads.spawn(move || ask(Path::new(&project.path))))
                .collect();
            running
                .into_iter()
                .map(|one| {
                    one.join()
                        .unwrap_or_else(|_| Err("zo's reader panicked".to_string()))
                })
                .collect()
        });
        for (project, result) in round.iter().zip(results) {
            match result {
                Ok(seats) => answered.push((project.clone(), seats)),
                Err(why) => failed = failed.or(Some(why)),
            }
        }
    }
    if answered.is_empty() {
        return Err(failed.unwrap_or_default());
    }
    Ok(answered)
}

/// The places this machine keeps the rows of the seats the window writes:
/// zo's requests folder (`~/.zo/jev`, [`zerocode_core::jev::count::REQUESTS_DIR`])
/// and the window's Computer Use sessions, where a screen seat's walk writes
/// its copy.
#[must_use]
pub fn machine_places(zo_home: &Path, sessions: &Path) -> [PathBuf; 2] {
    [
        zo_home.join(zerocode_core::jev::count::REQUESTS_DIR),
        sessions.to_path_buf(),
    ]
}

/// Where a seat's rows are kept, read off the file zo found them in.
#[must_use]
pub fn reach_of(found: &str, machine: &[PathBuf]) -> Reach {
    let found = Path::new(found);
    if machine.iter().any(|place| found.starts_with(place)) {
        Reach::Machine
    } else {
        Reach::Project
    }
}

/// Every seat of one reading, told where its rows are kept; a seat zo found
/// no file for keeps none.
#[must_use]
pub fn with_reach(mut seats: Vec<SeatNumbers>, machine: &[PathBuf]) -> Vec<SeatNumbers> {
    for seat in &mut seats {
        seat.reach = seat.found.as_deref().map(|found| reach_of(found, machine));
    }
    seats
}

/// Whether a reading holds the project's own records in the window — a seat
/// whose rows are the project's, with rows this week.
#[must_use]
pub fn has_own_records(seats: &[SeatNumbers]) -> bool {
    seats
        .iter()
        .any(|seat| seat.reach == Some(Reach::Project) && seat.week.rows > 0)
}

/// The machine's temporary roots, as spelled and resolved — zo's scoreboard's
/// own (`temp_roots`): the system temporary directory and `/tmp`.
#[must_use]
pub fn temporary_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for root in [std::env::temp_dir(), PathBuf::from("/tmp")] {
        if let Ok(resolved) = root.canonicalize() {
            roots.push(resolved);
        }
        roots.push(root);
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Every project with zo records in the window — a ledger under its state
/// written since then — by the workspace its sessions recorded, newest
/// record first. A project whose sessions recorded no workspace, whose
/// workspace is gone, or under a temporary root (a bench round's `mkdtemp`, a
/// test's `tempfile`, a scratchpad) is not one a person keeps records in;
/// zo's scoreboard leaves the same out (`is_temporary_workspace`).
#[must_use]
pub fn recorded_projects(zo_home: &Path, now_ms: i64, temporary: &[PathBuf]) -> Vec<JevProject> {
    let since_ms = now_ms.saturating_sub(WINDOW_DAYS * MS_PER_DAY);
    let Ok(entries) = std::fs::read_dir(zo_home.join(ZO_PROJECTS_DIR)) else {
        return Vec::new();
    };
    let mut found: Vec<JevProject> = Vec::new();
    for entry in entries.flatten() {
        let project = entry.path();
        let ledgers = project.join(ZO_PROJECT_STATE_DIR).join(JEV_LEDGER_DIR);
        let Some(newest_ms) = newest_record_ms(&ledgers).filter(|at| *at >= since_ms) else {
            continue;
        };
        let Some(workspace) = recorded_workspace(&project) else {
            continue;
        };
        if temporary.iter().any(|root| workspace.starts_with(root)) || !workspace.is_dir() {
            continue;
        }
        let path = workspace.to_string_lossy().into_owned();
        if let Some(held) = found.iter_mut().find(|held| held.path == path) {
            held.newest_ms = held.newest_ms.max(newest_ms);
        } else {
            found.push(JevProject {
                name: name_of(&workspace),
                path,
                newest_ms,
            });
        }
    }
    found.sort_by(|a, b| {
        b.newest_ms
            .cmp(&a.newest_ms)
            .then_with(|| a.path.cmp(&b.path))
    });
    found
}

/// When the newest ledger in `ledgers` was written, ms since the epoch.
fn newest_record_ms(ledgers: &Path) -> Option<i64> {
    std::fs::read_dir(ledgers)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == LEDGER_EXTENSION)
        })
        .filter_map(|entry| entry.metadata().ok()?.modified().ok())
        .filter_map(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .max()
}

/// The workspace a project's sessions recorded — the newest `.cwd` sidecar
/// beside a transcript that names one.
fn recorded_workspace(project: &Path) -> Option<PathBuf> {
    #[derive(Deserialize)]
    struct Recorded {
        cwd: String,
    }
    let mut sidecars: Vec<(std::time::SystemTime, PathBuf)> =
        std::fs::read_dir(project.join(ZO_SESSIONS_DIR))
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == SESSION_CWD_EXTENSION)
            })
            .filter_map(|path| Some((path.metadata().ok()?.modified().ok()?, path)))
            .collect();
    sidecars.sort_by(|a, b| b.0.cmp(&a.0));
    sidecars.iter().find_map(|(_, sidecar)| {
        let recorded: Recorded =
            serde_json::from_str(&std::fs::read_to_string(sidecar).ok()?).ok()?;
        let cwd = recorded.cwd.trim();
        (!cwd.is_empty()).then(|| PathBuf::from(cwd))
    })
}

/// A path's last folder's name, or the whole path when it has none.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// How one of a seat's numbers is carried from each project's reading into
/// the every-project sum — the tables below ([`SEAT`], [`WINDOW`],
/// [`AGREEMENT`], [`DAY`]) are the one place those rules are written, and
/// [`carry`] reads them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carry {
    /// Added: a count, a cost, each count of a tally (a screen seat's guards
    /// and controls).
    Add,
    /// The same in every reading — the use table's own — or nothing when
    /// they differ.
    Agreed,
    /// A project's own judgment of its own rows — its verdict, its judged
    /// window, its latency percentiles: a sum of readings has none.
    Own,
    /// A window of counts, carried by [`WINDOW`].
    Window,
    /// Comparison marks, carried by [`AGREEMENT`].
    Agreement,
    /// Calendar days matched by their start, each carried by [`DAY`].
    Days,
    /// Every reading's last requests, newest first, as many as one reading
    /// lists.
    Newest,
    /// Rows by outcome token, added token by token, most first.
    Tokens,
    /// Read again off the added counts by the core's own counter
    /// ([`zerocode_core::jev::summary::Tally`],
    /// [`zerocode_core::jev::promote::Agreement`]).
    Recounted,
    /// Said across every project counted, not only the readings added
    /// ([`sum_seat`]): whether the seat acts, where its rows are kept, and in
    /// how many projects.
    Across,
}

const ANSWERED_SHARE: &str = "answeredShare";
const ANSWERED_LOWER_BOUND: &str = "answeredLowerBound";
const LOWER_BOUND: &str = "lowerBound";
const BASELINE_SHARE: &str = "baselineShare";

/// A seat's own numbers.
pub const SEAT: &[(&str, Carry)] = &[
    ("id", Carry::Agreed),
    ("stand", Carry::Across),
    ("applies", Carry::Across),
    ("verdict", Carry::Own),
    ("rowsToNextJudgment", Carry::Own),
    ("riseFloorPermille", Carry::Agreed),
    ("clearsRiseFloor", Carry::Own),
    ("today", Carry::Window),
    ("week", Carry::Window),
    ("judged", Carry::Own),
    ("agreementWeek", Carry::Agreement),
    ("baseline", Carry::Agreed),
    ("negativesWanted", Carry::Own),
    ("costUsd", Carry::Add),
    ("askedModel", Carry::Agreed),
    ("model", Carry::Agreed),
    ("days", Carry::Days),
    ("recent", Carry::Newest),
    ("calibration", Carry::Own),
    ("applyShare", Carry::Own),
    ("appliedErrorPermille", Carry::Own),
    ("baselineErrorPermille", Carry::Own),
    ("reach", Carry::Across),
    ("across", Carry::Across),
];

/// One window of a seat's rows.
pub const WINDOW: &[(&str, Carry)] = &[
    ("rows", Carry::Add),
    ("answered", Carry::Add),
    ("refused", Carry::Add),
    (ANSWERED_SHARE, Carry::Recounted),
    (ANSWERED_LOWER_BOUND, Carry::Recounted),
    ("p50Ms", Carry::Own),
    ("p95Ms", Carry::Own),
    ("redactedLines", Carry::Add),
    ("called", Carry::Add),
    ("requests", Carry::Add),
    ("inputTokens", Carry::Add),
    ("applied", Carry::Add),
    ("failures", Carry::Tokens),
    ("refusals", Carry::Tokens),
    ("guards", Carry::Add),
    ("controls", Carry::Add),
];

/// A seat's comparison marks.
pub const AGREEMENT: &[(&str, Carry)] = &[
    ("compared", Carry::Add),
    ("agreed", Carry::Add),
    (LOWER_BOUND, Carry::Recounted),
    ("controlRows", Carry::Add),
    ("baselineCompared", Carry::Add),
    ("baselineAgreed", Carry::Add),
    (BASELINE_SHARE, Carry::Recounted),
    ("notCompared", Carry::Add),
    ("notComparedBy", Carry::Add),
];

/// One calendar day of a seat's trend.
pub const DAY: &[(&str, Carry)] = &[
    ("startMs", Carry::Agreed),
    ("tally", Carry::Window),
    ("agreement", Carry::Agreement),
];

/// Every project's readings summed seat by seat, in the first reading's
/// order (zo's use table's).
///
/// # Errors
/// A sum the seat's own reader refuses — a table that lost a field the
/// reader needs.
pub fn summed(readings: &[Vec<SeatNumbers>]) -> Result<Vec<SeatNumbers>, String> {
    let Some(first) = readings.first() else {
        return Ok(Vec::new());
    };
    first
        .iter()
        .map(|seat| {
            let of: Vec<&SeatNumbers> = readings
                .iter()
                .filter_map(|reading| reading.iter().find(|one| one.id == seat.id))
                .collect();
            sum_seat(&of)
        })
        .collect()
}

/// One seat across every project's reading. The machine's rows are one set
/// whichever project asked, so they are read once; a project's own rows are
/// that project's, so each is added. One reading left is carried as it
/// stands, judgment and all — which is why a seat whose rows the machine
/// keeps reads the same numbers in every scope.
fn sum_seat(readings: &[&SeatNumbers]) -> Result<SeatNumbers, String> {
    let mut distinct: Vec<&SeatNumbers> = readings
        .iter()
        .copied()
        .filter(|one| one.reach == Some(Reach::Project))
        .collect();
    distinct.extend(
        readings
            .iter()
            .copied()
            .find(|one| one.reach == Some(Reach::Machine)),
    );
    if distinct.is_empty() {
        distinct.extend(readings.first().copied());
    }
    let Some(lead) = distinct.first().copied() else {
        return Err("a seat no reading carried".to_string());
    };
    let summed = distinct.len() > 1;
    let mut value = if summed {
        let values = distinct
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<Value>, _>>()
            .map_err(|error| error.to_string())?;
        carry(SEAT, &values.iter().collect::<Vec<_>>())
    } else {
        serde_json::to_value(lead).map_err(|error| error.to_string())?
    };
    let acting = readings
        .iter()
        .copied()
        .find(|one| one.applies)
        .unwrap_or(lead);
    let reach = if distinct.iter().any(|one| one.reach == Some(Reach::Project)) {
        Some(Reach::Project)
    } else {
        lead.reach
    };
    let across = SeatAcross {
        projects: readings.len(),
        applying: readings.iter().filter(|one| one.applies).count(),
        summed,
    };
    if let Some(fields) = value.as_object_mut() {
        fields.insert("stand".to_string(), json!(acting.stand));
        fields.insert("applies".to_string(), json!(across.applying > 0));
        fields.insert("reach".to_string(), json!(reach));
        fields.insert("across".to_string(), json!(across));
    }
    serde_json::from_value(value).map_err(|error| error.to_string())
}

/// `readings` carried field by field as `table` says.
fn carry(table: &[(&str, Carry)], readings: &[&Value]) -> Value {
    let mut out = Map::new();
    for (field, rule) in table {
        let held: Vec<&Value> = readings
            .iter()
            .filter_map(|one| one.get(*field))
            .filter(|value| !value.is_null())
            .collect();
        let carried = match rule {
            Carry::Across | Carry::Recounted => continue,
            Carry::Add => added(&held),
            Carry::Agreed => agreed(&held),
            Carry::Own => Value::Null,
            Carry::Window => window(&held),
            Carry::Agreement => agreement(&held),
            Carry::Days => days(&held),
            Carry::Newest => newest(&held),
            Carry::Tokens => tokens(&held),
        };
        out.insert((*field).to_string(), carried);
    }
    Value::Object(out)
}

fn added(held: &[&Value]) -> Value {
    if held.is_empty() {
        return Value::Null;
    }
    if held.iter().all(|value| value.is_object()) {
        let mut out = Map::new();
        for one in held {
            for key in one.as_object().into_iter().flat_map(Map::keys) {
                if !out.contains_key(key) {
                    let parts: Vec<&Value> = held.iter().filter_map(|each| each.get(key)).collect();
                    out.insert(key.clone(), added(&parts));
                }
            }
        }
        return Value::Object(out);
    }
    if held.iter().all(|value| value.is_u64()) {
        return json!(held.iter().filter_map(|value| value.as_u64()).sum::<u64>());
    }
    json!(held.iter().filter_map(|value| value.as_f64()).sum::<f64>())
}

fn agreed(held: &[&Value]) -> Value {
    match held.first() {
        Some(first) if held.iter().all(|one| one == first) => (*first).clone(),
        _ => Value::Null,
    }
}

fn count(of: &Value, field: &str) -> usize {
    of.get(field)
        .and_then(Value::as_u64)
        .and_then(|count| usize::try_from(count).ok())
        .unwrap_or(0)
}

fn window(held: &[&Value]) -> Value {
    let mut out = carry(WINDOW, held);
    let tally = zerocode_core::jev::summary::Tally {
        rows: count(&out, "rows"),
        answered: count(&out, "answered"),
        refused: count(&out, "refused"),
        ..zerocode_core::jev::summary::Tally::default()
    };
    if let Some(fields) = out.as_object_mut() {
        fields.insert(ANSWERED_SHARE.to_string(), json!(tally.answered_share()));
        fields.insert(
            ANSWERED_LOWER_BOUND.to_string(),
            json!(tally.answered_lower_bound()),
        );
    }
    out
}

fn agreement(held: &[&Value]) -> Value {
    let mut out = carry(AGREEMENT, held);
    let marks = zerocode_core::jev::promote::Agreement {
        compared: count(&out, "compared"),
        agreed: count(&out, "agreed"),
        baseline_compared: count(&out, "baselineCompared"),
        baseline_agreed: count(&out, "baselineAgreed"),
        not_compared: count(&out, "notCompared"),
    };
    if let Some(fields) = out.as_object_mut() {
        fields.insert(LOWER_BOUND.to_string(), json!(marks.lower_bound()));
        fields.insert(BASELINE_SHARE.to_string(), json!(marks.baseline_share()));
    }
    out
}

fn days(held: &[&Value]) -> Value {
    let mut by_start: std::collections::BTreeMap<i64, Vec<&Value>> =
        std::collections::BTreeMap::new();
    let mut longest = 0;
    for list in held {
        let list = list.as_array().map_or(&[][..], Vec::as_slice);
        longest = longest.max(list.len());
        for day in list {
            if let Some(start) = day.get("startMs").and_then(Value::as_i64) {
                by_start.entry(start).or_default().push(day);
            }
        }
    }
    let carried: Vec<Value> = by_start.values().map(|same| carry(DAY, same)).collect();
    Value::Array(carried[carried.len().saturating_sub(longest)..].to_vec())
}

fn newest(held: &[&Value]) -> Value {
    let mut longest = 0;
    let mut every: Vec<&Value> = Vec::new();
    for list in held {
        let list = list.as_array().map_or(&[][..], Vec::as_slice);
        longest = longest.max(list.len());
        every.extend(list);
    }
    every.sort_by_key(|one| std::cmp::Reverse(one.get("at").and_then(Value::as_i64).unwrap_or(0)));
    Value::Array(every.into_iter().take(longest).cloned().collect())
}

fn tokens(held: &[&Value]) -> Value {
    let mut rows: Vec<(String, u64)> = Vec::new();
    for list in held {
        for one in list.as_array().map_or(&[][..], Vec::as_slice) {
            let Some(token) = one.get("token").and_then(Value::as_str) else {
                continue;
            };
            let count = one.get("rows").and_then(Value::as_u64).unwrap_or(0);
            match rows.iter_mut().find(|(held, _)| held == token) {
                Some((_, rows)) => *rows += count,
                None => rows.push((token.to_string(), count)),
            }
        }
    }
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Value::Array(
        rows.into_iter()
            .map(|(token, rows)| json!({ "token": token, "rows": rows }))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// One seat as zo sends it: `week` rows answered and refused, the file it
    /// was read from, whether it acts; `extra` over the rest.
    fn seat(
        id: &str,
        found: Option<&str>,
        week: (u64, u64, u64),
        applies: bool,
        extra: Value,
    ) -> Value {
        let (rows, answered, refused) = week;
        let mut seat = json!({
            "id": id, "stand": if applies { "applying" } else { "recording" }, "applies": applies,
            "found": found,
            "today": { "rows": rows / 2, "answered": answered / 2 },
            "week": { "rows": rows, "answered": answered, "refused": refused },
        });
        if let (Some(fields), Some(more)) = (seat.as_object_mut(), extra.as_object()) {
            for (key, value) in more {
                fields.insert(key.clone(), value.clone());
            }
        }
        seat
    }

    fn reading(seats: &[Value]) -> Vec<SeatNumbers> {
        serde_json::from_value(Value::Array(seats.to_vec())).expect("zo's seats")
    }

    fn of<'a>(seats: &'a [SeatNumbers], id: &str) -> &'a SeatNumbers {
        seats.iter().find(|seat| seat.id == id).expect("the seat")
    }

    /// The names this reader walks are the ones zo writes: the folders of a
    /// project's records and its sessions, the transcript's workspace
    /// sidecar, the window, and the temporary roots its scoreboard leaves
    /// out — read out of zo's own source, so one of them moving there is a
    /// red here rather than a dashboard that finds no project.
    #[test]
    fn the_layout_read_here_is_the_one_zo_writes() {
        let config = include_str!("../../../zo-ide/crates/runtime/src/config/mod.rs");
        assert!(config.contains(&format!(
            "pub const JEV_LEDGER_DIR: &str = \"{JEV_LEDGER_DIR}\";"
        )));
        assert!(config.contains(&format!(
            ".join(\"{ZO_PROJECTS_DIR}\").join(slug).join(\"{ZO_PROJECT_STATE_DIR}\")"
        )));
        let sessions = include_str!("../../../zo-ide/crates/runtime/src/session_control.rs");
        assert!(sessions.contains(&format!(
            ".join(\"{ZO_PROJECTS_DIR}\").join(&slug).join(\"{ZO_SESSIONS_DIR}\")"
        )));
        let resume = include_str!("../../../zo-ide/crates/zo-ide/src/resume.rs");
        assert!(resume.contains(&format!("with_extension(\"{SESSION_CWD_EXTENSION}\")")));
        assert!(resume.contains("struct SessionCwd {\n    cwd: String,"));
        let summary =
            include_str!("../../../zo-ide/crates/tools/src/misc_tools/smart_router/jev_summary.rs");
        assert!(summary.contains(&format!("pub const WINDOW_DAYS: i64 = {WINDOW_DAYS};")));
        let scoreboard = include_str!("../../../zo-ide/crates/zo-ide/src/scoreboard_cli.rs");
        assert!(scoreboard.contains("for root in [std::env::temp_dir(), PathBuf::from(\"/tmp\")]"));
        for seat in zerocode_core::jev::JEV_USES {
            assert!(
                Path::new(seat.ledger)
                    .extension()
                    .is_some_and(|extension| extension == LEDGER_EXTENSION),
                "{} keeps its rows in {}",
                seat.id,
                seat.ledger
            );
        }
    }

    fn write_at(path: &Path, text: &str, at: std::time::SystemTime) {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the folder");
        std::fs::write(path, text).expect("the file");
        std::fs::File::options()
            .write(true)
            .open(path)
            .expect("reopened")
            .set_modified(at)
            .expect("its time");
    }

    /// A project is one with records in the window, named by the workspace
    /// its sessions recorded, newest record first; two folders for one
    /// workspace are one project. Old records, no recorded workspace, a
    /// workspace under a temporary root and a workspace that is gone are not
    /// a project a person keeps records in.
    #[test]
    fn a_project_is_one_with_records_in_the_window_by_the_workspace_its_sessions_recorded() {
        let home = tempfile::tempdir().expect("a zo home");
        let places = tempfile::tempdir().expect("the workspaces");
        let temporary = tempfile::tempdir().expect("a temporary root");
        let workspace = |name: &str| {
            let path = places.path().join(name);
            std::fs::create_dir_all(&path).expect("a workspace");
            path
        };
        let (atlas, notes) = (workspace("atlas"), workspace("notes"));
        let scratch = temporary.path().join("bench");
        std::fs::create_dir_all(&scratch).expect("a scratch workspace");
        let now = std::time::SystemTime::now();
        let hours = |count: u64| now - std::time::Duration::from_secs(count * 3_600);
        let project = |slug: &str, record: std::time::SystemTime, cwd: Option<&Path>| {
            let at = home.path().join(ZO_PROJECTS_DIR).join(slug);
            write_at(
                &at.join(ZO_PROJECT_STATE_DIR)
                    .join(JEV_LEDGER_DIR)
                    .join("decision-shadow.jsonl"),
                "{}\n",
                record,
            );
            if let Some(cwd) = cwd {
                write_at(
                    &at.join(ZO_SESSIONS_DIR).join("session-1.cwd"),
                    &json!({ "cwd": cwd }).to_string(),
                    record,
                );
            }
        };
        project("atlas-1", hours(2), Some(&atlas));
        project("atlas-2", hours(1), Some(&atlas));
        project("notes-1", hours(30), Some(&notes));
        project("old-1", hours(24 * 8), Some(&workspace("old")));
        project("unrecorded-1", hours(1), None);
        project("scratch-1", hours(1), Some(&scratch));
        project("gone-1", hours(1), Some(&places.path().join("gone")));
        let now_ms = i64::try_from(
            now.duration_since(std::time::UNIX_EPOCH)
                .expect("after the epoch")
                .as_millis(),
        )
        .expect("a time");

        let found = recorded_projects(home.path(), now_ms, &[temporary.path().to_path_buf()]);
        let named: Vec<(&str, &str)> = found
            .iter()
            .map(|one| (one.name.as_str(), one.path.as_str()))
            .collect();
        assert_eq!(
            named,
            vec![
                ("atlas", atlas.to_str().expect("a path")),
                ("notes", notes.to_str().expect("a path")),
            ]
        );
        assert!(found[0].newest_ms > found[1].newest_ms);
        assert!(recorded_projects(&home.path().join("nowhere"), now_ms, &[]).is_empty());
    }

    /// A seat's rows are the machine's when zo read them from `~/.zo/jev` or
    /// the window's sessions, the project's anywhere else, and neither when
    /// zo found no file.
    #[test]
    fn a_seats_rows_are_the_machines_or_the_projects_by_the_file_zo_read_them_from() {
        let machine = machine_places(Path::new("/h/.zo"), Path::new("/w/computer-use/sessions"));
        assert_eq!(
            reach_of("/h/.zo/jev/notify-call.jsonl", &machine),
            Reach::Machine
        );
        assert_eq!(
            reach_of(
                "/w/computer-use/sessions/20260920-1/desktop-action.jsonl",
                &machine
            ),
            Reach::Machine
        );
        assert_eq!(
            reach_of(
                "/h/.zo/projects/p-1/state/smart-router/decision-shadow.jsonl",
                &machine
            ),
            Reach::Project
        );
        let seats = with_reach(
            reading(&[
                seat(
                    "notify",
                    Some("/h/.zo/jev/notify-call.jsonl"),
                    (4, 4, 0),
                    false,
                    json!({}),
                ),
                seat("recall", None, (0, 0, 0), false, json!({})),
            ]),
            &machine,
        );
        assert_eq!(of(&seats, "notify").reach, Some(Reach::Machine));
        assert_eq!(of(&seats, "recall").reach, None);
        assert!(!has_own_records(&seats));
    }

    const MACHINE: &str = "/h/.zo/jev/notify-call.jsonl";

    /// Two projects' readings of three seats: routing the projects' own in
    /// both, recall the first project's alone, notify the machine's — one set
    /// of rows both projects read.
    fn two_projects() -> Vec<Vec<SeatNumbers>> {
        let day = |start: i64, rows: u64| json!({ "startMs": start, "tally": { "rows": rows, "answered": rows }, "agreement": { "compared": 1, "agreed": 1 } });
        let machine = machine_places(Path::new("/h/.zo"), Path::new("/w/sessions"));
        let notify = seat(
            "notify",
            Some(MACHINE),
            (460, 422, 10),
            false,
            json!({
                "verdict": { "verdict": "hold", "line": "agreement" },
                "judged": { "window": { "rows": 20, "answered": 20 }, "windowWanted": 20, "agreement": { "compared": 20, "agreed": 12 } },
                "costUsd": 0.0, "days": [day(1_000, 100), day(2_000, 60)],
            }),
        );
        let a = reading(&[
            seat(
                "routing",
                Some("/h/.zo/projects/a/state/smart-router/decision-shadow.jsonl"),
                (23, 23, 0),
                false,
                json!({
                    "verdict": { "verdict": "hold", "line": "too_few_rows" }, "rowsToNextJudgment": 50,
                    "costUsd": 0.002, "riseFloorPermille": 950, "model": "jev-1.13.0",
                    "agreementWeek": { "compared": 3, "agreed": 3, "baselineCompared": 2, "baselineAgreed": 1,
                                       "notCompared": 1, "notComparedBy": { "unanswered": 1 } },
                    "days": [day(1_000, 20), day(2_000, 3)],
                    "recent": [{ "at": 30, "outcome": "answered" }, { "at": 10, "outcome": "answered" }],
                }),
            ),
            seat(
                "recall",
                Some("/h/.zo/projects/a/state/smart-router/rerank-shadow.jsonl"),
                (800, 800, 0),
                false,
                json!({ "costUsd": 0.003 }),
            ),
            notify.clone(),
        ]);
        let mut routing_b = seat(
            "routing",
            Some("/h/.zo/projects/b/state/smart-router/decision-shadow.jsonl"),
            (5, 4, 1),
            true,
            json!({
                "verdict": { "verdict": "hold", "line": "answered" }, "costUsd": 0.001, "riseFloorPermille": 950, "model": "jev-1.12.0",
                "agreementWeek": { "compared": 1, "agreed": 0, "notCompared": 2,
                                   "notComparedBy": { "unanswered": 1, "away": 1 } },
                "days": [day(2_000, 5), day(3_000, 1)],
                "recent": [{ "at": 20, "outcome": "timeout" }],
            }),
        );
        routing_b["week"]["failures"] =
            json!([{ "token": "no_key", "rows": 1 }, { "token": "timeout", "rows": 2 }]);
        routing_b["week"]["p50Ms"] = json!(300);
        let b = reading(&[
            routing_b,
            seat("recall", None, (0, 0, 0), true, json!({})),
            notify,
        ]);
        vec![with_reach(a, &machine), with_reach(b, &machine)]
    }

    /// Every project, summed by the one table: counts and costs added, the
    /// shares and bounds read again off the sums by the core's own counter,
    /// failures added token by token, the days matched by their start and
    /// the last requests merged newest first; a project's own judgment and
    /// latency not carried; whether the seat acts said across every project
    /// counted. A seat only one project has rows of is that project's, as it
    /// stands.
    #[test]
    fn every_project_is_summed_by_the_one_table() {
        let summed = summed(&two_projects()).expect("a sum");
        let routing = of(&summed, "routing");
        assert_eq!(
            (
                routing.week.rows,
                routing.week.answered,
                routing.week.refused
            ),
            (28, 27, 1)
        );
        let tally = zerocode_core::jev::summary::Tally {
            rows: 28,
            answered: 27,
            refused: 1,
            ..zerocode_core::jev::summary::Tally::default()
        };
        assert_eq!(routing.week.answered_share, tally.answered_share());
        assert_eq!(
            routing.week.answered_lower_bound,
            tally.answered_lower_bound()
        );
        assert_eq!(
            routing.week.p50_ms, None,
            "a latency percentile is not a sum"
        );
        let failures: Vec<(&str, usize)> = routing
            .week
            .failures
            .iter()
            .map(|one| (one.token.as_str(), one.rows))
            .collect();
        assert_eq!(failures, vec![("timeout", 2), ("no_key", 1)]);
        assert!(
            routing
                .cost_usd
                .is_some_and(|cost| (cost - 0.003).abs() < 1e-12)
        );
        assert!(routing.verdict.is_none() && routing.rows_to_next_judgment.is_none());
        assert_eq!(
            routing.rise_floor_permille,
            Some(950),
            "the use table's own"
        );
        assert_eq!(routing.model, None, "two versions answered");
        let week = routing.agreement_week.as_ref().expect("the marks");
        assert_eq!(
            (week.compared, week.agreed, week.baseline_compared),
            (4, 3, 2)
        );
        let marks = zerocode_core::jev::promote::Agreement {
            compared: 4,
            agreed: 3,
            baseline_compared: 2,
            baseline_agreed: 1,
            not_compared: 3,
        };
        assert_eq!(week.lower_bound, marks.lower_bound());
        assert_eq!(week.baseline_share, marks.baseline_share());
        // Why the rows compared nothing, added word by word (t-9556).
        let sent = serde_json::to_value(routing).expect("the seat, sent");
        assert_eq!(sent["agreementWeek"]["notCompared"], 3);
        assert_eq!(
            sent["agreementWeek"]["notComparedBy"],
            json!({ "unanswered": 2, "away": 1 })
        );
        let days: Vec<(i64, usize)> = routing
            .days
            .iter()
            .map(|day| (day.start_ms, day.tally.rows))
            .collect();
        assert_eq!(
            days,
            vec![(2_000, 8), (3_000, 1)],
            "the last two days, matched by start"
        );
        let recent: Vec<i64> = routing.recent.iter().map(|one| one.at).collect();
        assert_eq!(
            recent,
            vec![30, 20],
            "newest first, as many as one project lists"
        );
        assert!(routing.applies, "it acts in one of the two");
        assert_eq!(routing.stand, "applying");
        assert_eq!(routing.reach, Some(Reach::Project));
        assert_eq!(
            routing.across,
            Some(SeatAcross {
                projects: 2,
                applying: 1,
                summed: true
            })
        );

        let recall = of(&summed, "recall");
        let alone = of(&two_projects()[0], "recall").clone();
        assert_eq!(
            (recall.week.clone(), recall.cost_usd),
            (alone.week, alone.cost_usd)
        );
        assert_eq!(
            recall.across,
            Some(SeatAcross {
                projects: 2,
                applying: 1,
                summed: false
            })
        );
    }

    /// A seat whose rows the machine keeps reads the same numbers in every
    /// scope — its rows read once, its judgment and latency carried as they
    /// stand — and says so.
    #[test]
    fn a_seat_the_machine_keeps_reads_the_same_in_every_scope() {
        let readings = two_projects();
        let summed = summed(&readings).expect("a sum");
        let here = of(&readings[0], "notify");
        let everywhere = of(&summed, "notify");
        assert_eq!(everywhere.reach, Some(Reach::Machine));
        assert_eq!(
            (&everywhere.today, &everywhere.week, &everywhere.days),
            (&here.today, &here.week, &here.days)
        );
        assert_eq!(
            (&everywhere.verdict, &everywhere.judged, everywhere.cost_usd),
            (&here.verdict, &here.judged, here.cost_usd)
        );
        assert_eq!(
            everywhere.across,
            Some(SeatAcross {
                projects: 2,
                applying: 0,
                summed: false
            })
        );
    }

    /// Every number a seat carries has one rule in the tables, and every
    /// rule names a number a seat carries: a field added to the reader is a
    /// red here until the sum says what becomes of it. The rules read again
    /// off the sums are the ones the core's counter answers.
    #[test]
    fn every_number_a_seat_carries_has_one_rule() {
        let window = json!({ "rows": 1, "answered": 1, "refused": 0, "answeredShare": 1.0, "answeredLowerBound": 0.2,
            "p50Ms": 1, "p95Ms": 1, "redactedLines": 0, "called": 1, "requests": 1, "inputTokens": 1, "applied": 0,
            "failures": [], "refusals": [], "guards": {}, "controls": {} });
        let marks = json!({ "compared": 1, "agreed": 1, "lowerBound": 0.2, "controlRows": 0, "baselineCompared": 1,
            "baselineAgreed": 1, "baselineShare": 1.0, "notCompared": 0, "notComparedBy": {} });
        let full: SeatNumbers = serde_json::from_value(json!({
            "id": "routing", "stand": "recording", "applies": false,
            "verdict": { "verdict": "hold" }, "rowsToNextJudgment": 1, "riseFloorPermille": 950, "clearsRiseFloor": false,
            "today": window, "week": window,
            "judged": { "window": window, "windowWanted": 20, "agreement": marks },
            "agreementWeek": marks, "baseline": "none", "negativesWanted": 1, "costUsd": 0.1,
            "askedModel": "jev-latest", "model": "jev-1.13.0",
            "days": [{ "startMs": 1, "tally": window, "agreement": marks }],
            "recent": [], "found": "/h/.zo/jev/a.jsonl", "reach": "machine",
            "across": { "projects": 1, "applying": 0, "summed": false },
        }))
        .expect("a full seat");
        let sent = serde_json::to_value(&full).expect("the seat, sent");
        let named = |table: &[(&'static str, Carry)]| {
            let mut names: Vec<&'static str> = table.iter().map(|(name, _)| *name).collect();
            names.sort_unstable();
            names
        };
        let keys = |value: &Value| {
            let mut keys: Vec<String> = value
                .as_object()
                .expect("an object")
                .keys()
                .cloned()
                .collect();
            keys.sort_unstable();
            keys
        };
        assert_eq!(
            keys(&sent),
            named(SEAT),
            "a seat's numbers and the seat table"
        );
        assert_eq!(
            keys(&sent["week"]),
            named(WINDOW),
            "a window's numbers and the window table"
        );
        assert_eq!(
            keys(&sent["agreementWeek"]),
            named(AGREEMENT),
            "the marks and the marks table"
        );
        assert_eq!(
            keys(&sent["days"][0]),
            named(DAY),
            "a day and the day table"
        );
        let recounted = |table: &[(&'static str, Carry)]| {
            table
                .iter()
                .filter(|(_, rule)| *rule == Carry::Recounted)
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            recounted(WINDOW),
            vec![ANSWERED_SHARE, ANSWERED_LOWER_BOUND]
        );
        assert_eq!(recounted(AGREEMENT), vec![LOWER_BOUND, BASELINE_SHARE]);
        assert!(
            sent.get("found").is_none(),
            "a path of the person's disk was sent"
        );
    }

    /// A reading names what it counted: the checkout with records of its own
    /// asks zo once and looks for no other project; one without says which
    /// projects have them; every project, asked, is each project's zo once,
    /// and a project whose zo failed is not one the sum counted.
    #[test]
    fn a_reading_names_its_scope_and_looks_elsewhere_only_when_the_checkout_has_nothing() {
        let home = tempfile::tempdir().expect("a zo home");
        let places_dir = tempfile::tempdir().expect("the workspaces");
        let workspace = places_dir.path().join("t-1");
        let (atlas, broken) = (
            places_dir.path().join("atlas"),
            places_dir.path().join("broken"),
        );
        for path in [&workspace, &atlas, &broken] {
            std::fs::create_dir_all(path).expect("a workspace");
        }
        let now = std::time::SystemTime::now();
        for (slug, cwd) in [("atlas-1", &atlas), ("broken-1", &broken)] {
            let at = home.path().join(ZO_PROJECTS_DIR).join(slug);
            write_at(
                &at.join(ZO_PROJECT_STATE_DIR)
                    .join(JEV_LEDGER_DIR)
                    .join("a.jsonl"),
                "{}\n",
                now,
            );
            write_at(
                &at.join(ZO_SESSIONS_DIR).join("s.cwd"),
                &json!({ "cwd": cwd }).to_string(),
                now,
            );
        }
        let places = Places {
            zo_home: home.path().to_path_buf(),
            sessions: places_dir.path().join("sessions"),
            temporary: Vec::new(),
        };
        let own = home
            .path()
            .join("projects/atlas-1/state/smart-router/a.jsonl");
        let machine = home.path().join("jev/notify-call.jsonl");
        let asked: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
        let with_records = Mutex::new(false);
        let ask = |project: &Path| -> Result<Vec<SeatNumbers>, String> {
            asked.lock().expect("the asks").push(project.to_path_buf());
            if project == broken {
                return Err("zo exited 1".to_string());
            }
            let records = project == atlas || *with_records.lock().expect("the switch");
            Ok(reading(&[
                seat(
                    "routing",
                    records.then(|| own.to_str().expect("a path")),
                    if records { (9, 9, 0) } else { (0, 0, 0) },
                    false,
                    json!({}),
                ),
                seat(
                    "notify",
                    Some(machine.to_str().expect("a path")),
                    (4, 4, 0),
                    false,
                    json!({}),
                ),
            ]))
        };
        let now_ms = crate::now_epoch_ms();

        let bare = read(&ask, &places, &workspace, Scope::Workspace, now_ms).expect("a reading");
        assert_eq!(
            (bare.scope.scope, bare.scope.recorded),
            (Scope::Workspace, false)
        );
        assert_eq!(bare.scope.workspace_name, "t-1");
        let names: Vec<&str> = bare
            .scope
            .projects
            .iter()
            .map(|one| one.name.as_str())
            .collect();
        assert!(
            names.contains(&"atlas") && names.contains(&"broken"),
            "{names:?}"
        );
        assert_eq!(bare.scope.window_days, WINDOW_DAYS);

        *with_records.lock().expect("the switch") = true;
        asked.lock().expect("the asks").clear();
        let kept = read(&ask, &places, &workspace, Scope::Workspace, now_ms).expect("a reading");
        assert!(kept.scope.recorded && kept.scope.projects.is_empty());
        assert_eq!(*asked.lock().expect("the asks"), vec![workspace.clone()]);

        *with_records.lock().expect("the switch") = false;
        asked.lock().expect("the asks").clear();
        let every = read(&ask, &places, &workspace, Scope::Projects, now_ms).expect("a reading");
        let mut asks = asked.lock().expect("the asks").clone();
        asks.sort();
        let mut listed = vec![atlas.clone(), broken.clone()];
        listed.sort();
        assert_eq!(
            asks, listed,
            "each project's zo once, and not the checkout's"
        );
        let counted: Vec<&str> = every
            .scope
            .projects
            .iter()
            .map(|one| one.name.as_str())
            .collect();
        assert_eq!(
            counted,
            vec!["atlas"],
            "a project whose zo failed was counted"
        );
        assert!(!every.scope.recorded);
        let routing = of(&every.seats, "routing");
        assert_eq!(routing.week.rows, 9);
        assert_eq!(
            routing.across,
            Some(SeatAcross {
                projects: 1,
                applying: 0,
                summed: false
            })
        );
    }
}
