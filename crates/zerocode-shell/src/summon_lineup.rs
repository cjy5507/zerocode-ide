//! Today's model lineup for every agent a summons can tune, as this window
//! last read it (t-14437).
//!
//! Two feeders, one shape (t-124593). An agent whose row says zo serves its
//! models reads the catalog the model chip already reads (`zo models
//! --json`, [`crate::slash_catalog::agent_lineup`]) — no second road to a
//! provider; an agent whose row names its own CLI's list command is asked
//! that command ([`zerocode_core::cli_models`]) by the same read — after
//! zo's lineups are told, and only when its last answer is gone or older
//! than the refresh rule — and its answer becomes rows of the same kind
//! (`Lineup::from_listed`). Which is which is the row's value, never a name
//! here.
//!
//! Readers inside the ledger actor only PEEK at what is held: a
//! summons's origin, `agent-list`, the settings pane. A reader that finds the
//! held answer older than zo's own refresh rule sends ONE read on a thread of
//! its own, and the next reader gets it — the readiness table's rule
//! (`readiness_runtime::observe`).
//!
//! Each read is laid against the book of models this window has seen
//! ([`Seen`], kept under the window's config root, never the person's
//! settings): a model that arrived since is marked new for a while and said
//! once in a line of its own, and so is one that folded.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use serde::Serialize;
use tauri::Emitter;
use zerocode_core::cli_models;
use zerocode_core::summon_difficulty::lineup::{Change, Lineup, REFRESH_AFTER_MS, Seen};
/// How often the window's background ticker checks wall-clock staleness so an
/// open window refreshes its catalog every hour (including right after wake
/// from sleep) even if nothing in the ledger peeked at `snapshot()`.
const TICK_EVERY: Duration = Duration::from_secs(60);
/// The book of models seen, under the window's config root.
const SEEN_FILE: &str = "summon-lineup-seen.json";
/// What the window's one-line notice listens for.
pub(crate) const CHANGED_EVENT: &str = "summon-lineup:changed";
/// What every read says of the releases it holds as superseded
/// ([`superseded`]), for the words the window puts beside a pane's model
/// (t-46882).
pub(crate) const SUPERSEDED_EVENT: &str = "summon-lineup:superseded";
/// What every finished read says, so a screen showing the lineup repaints:
/// the epoch milliseconds it was read at.
pub(crate) const READ_EVENT: &str = "summon-lineup:read";
/// How long a person's 「목록 새로고침」 waits for the read it asked for —
/// zo's own wait for its catalog (`slash_catalog::MODELS_WAIT`) and room
/// for the read already running before it.
const REFRESH_WAIT: Duration = Duration::from_secs(150);

/// Every lineup and the book, as a reader takes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) lineups: BTreeMap<String, Lineup>,
    pub(crate) seen: Seen,
}

/// What the read that made the held lineups was told about where they came
/// from: when, each source's report as the window keeps it (a failed one
/// carries only its diagnosis), and the person's connections, usable or not.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Sources {
    pub(crate) read_at_ms: Option<i64>,
    pub(crate) reports: Vec<Report>,
    pub(crate) connections: Vec<Connection>,
}

/// One source's answer to the read: a provider's list, or why there was
/// none — by the window's diagnosis (`slash_catalog::diagnosed`), never the
/// provider's own words.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub(crate) provider: String,
    pub(crate) source: String,
    pub(crate) ok: bool,
    pub(crate) count: u64,
    /// How old zo says the answer is (`"6m old"`).
    pub(crate) age: Option<String>,
    /// Skipped for want of a credential: there is no login to read.
    pub(crate) skipped: bool,
    /// The window's word for any other failure (`auth`, `network`, …).
    pub(crate) failure: Option<String>,
}

/// One of the person's own connections (`customProviders`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Connection {
    pub(crate) name: String,
    /// zo can reach it — its key is there; a connection without one is
    /// listed and offers nothing.
    pub(crate) usable: bool,
    pub(crate) models: Vec<String>,
}

#[derive(Default)]
struct Held {
    read_at_ms: Option<i64>,
    snapshot: Snapshot,
    sources: Sources,
    /// What each agent whose row names its own list command said at the
    /// read — kept whatever it said, so a failure is a fact a reader sees.
    answers: BTreeMap<String, cli_models::Snapshot>,
    reading: bool,
    /// Somebody asked for a read while one was running: read once more
    /// after it, so the answer postdates the ask.
    again: bool,
    /// Whether the queued read must pass `--refresh` to `zo models` so zo
    /// re-queries live provider registries rather than its disk cache.
    force_next: bool,
    /// Reads finished since boot — what a waiter counts.
    finished: u64,
}

/// Where a read looks: the home zo is installed under, and the window's
/// config root the book is kept in.
struct Roots {
    home: PathBuf,
    config_root: PathBuf,
}

fn held() -> &'static Mutex<Held> {
    static HELD: OnceLock<Mutex<Held>> = OnceLock::new();
    HELD.get_or_init(Mutex::default)
}

/// Rung at every finished read, for [`refresh_and_wait`].
fn finished() -> &'static Condvar {
    static FINISHED: Condvar = Condvar::new();
    &FINISHED
}

static ROOTS: OnceLock<Roots> = OnceLock::new();
static WINDOW: OnceLock<tauri::AppHandle> = OnceLock::new();
static TICKER: OnceLock<()> = OnceLock::new();

/// Named once at boot, and the first read sent; until then every reader is
/// answered with nothing held, which the rows read as "no lineup".
pub(crate) fn configure(app: tauri::AppHandle, home: PathBuf, config_root: PathBuf) {
    let _ = WINDOW.set(app);
    let _ = ROOTS.set(Roots { home, config_root });
    read_in_background(crate::usage_runtime::epoch_ms_now(), false);
    TICKER.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("summon-lineup-tick".into())
            .spawn(periodic_tick);
    });
}

fn due_for_periodic_refresh(read_at_ms: Option<i64>, now_ms: i64) -> bool {
    read_at_ms.is_none_or(|at| now_ms.saturating_sub(at) >= REFRESH_AFTER_MS)
}

fn periodic_tick() {
    loop {
        std::thread::sleep(TICK_EVERY);
        let now_ms = crate::usage_runtime::epoch_ms_now();
        let stale = {
            let held = held()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            !held.reading && due_for_periodic_refresh(held.read_at_ms, now_ms)
        };
        if stale {
            read_in_background(now_ms, false);
        }
    }
}

/// What is held, whatever its age; an old answer sends one read.
pub(crate) fn snapshot() -> Snapshot {
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let (snapshot, stale) = {
        let held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stale = due_for_periodic_refresh(held.read_at_ms, now_ms);
        (held.snapshot.clone(), stale)
    };
    if stale {
        read_in_background(now_ms, false);
    }
    snapshot
}

fn read_in_background(now_ms: i64, force: bool) {
    let Some(roots) = ROOTS.get() else { return };
    {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.reading {
            if force {
                held.again = true;
                held.force_next = true;
            }
            return;
        }
        held.reading = true;
        held.force_next = force;
    }
    let spawned = std::thread::Builder::new()
        .name("summon-lineup".into())
        .spawn(move || {
            let mut now_ms = now_ms;
            loop {
                let force_this_run = {
                    let mut held = held()
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    std::mem::take(&mut held.force_next)
                };
                if force_this_run {
                    crate::slash_catalog::refresh_zo_models();
                } else {
                    crate::slash_catalog::forget_zo_models();
                }
                read_once(roots, now_ms);
                let mut held = held()
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !std::mem::take(&mut held.again) {
                    held.reading = false;
                    break;
                }
                now_ms = crate::usage_runtime::epoch_ms_now();
            }
        });
    if spawned.is_err() {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held.reading = false;
        held.force_next = false;
    }
}

/// One read: every agent's lineup and what zo said of its sources, laid
/// against the book, held, and told to the window — then, after that is
/// told, the CLI lists that are due (t-124593).
///
/// zo's catalog is published first, with each CLI's list as last heard, so
/// a reader — a login switch's refresh, the settings screen's — never waits
/// on a CLI. A list is asked again only when none is held or the one held
/// is older than [`REFRESH_AFTER_MS`]: the same rule a reader's peek uses,
/// so a refresh between two hours asks no CLI at all. The read counts as
/// finished once its lists are in.
fn read_once(roots: &Roots, now_ms: i64) {
    let heard = held()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .answers
        .clone();
    let lineups = lineups_of(|agent| match cli_models::road_of(agent) {
        Some(_) => listed_lineup(agent, heard.get(agent)),
        None => crate::slash_catalog::agent_lineup(agent, &roots.home),
    });
    // The same cached catalog the lineups were read from — never a second
    // `zo models` run.
    let sources = sources_of(&crate::slash_catalog::zo_model_list(&roots.home), now_ms);
    let due = lists_due(&heard, now_ms);
    publish(roots, lineups, Some(sources), now_ms, due.is_empty());
    if due.is_empty() {
        return;
    }
    let asked: BTreeMap<String, cli_models::Snapshot> = due
        .into_iter()
        .filter_map(|agent| Some((agent.to_string(), ask_own_list(agent, now_ms)?)))
        .collect();
    let lineups = {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut lineups = held.snapshot.lineups.clone();
        for (agent, answer) in &asked {
            match listed_lineup(agent, Some(answer)) {
                Some(lineup) => {
                    lineups.insert(agent.clone(), lineup);
                }
                None => {
                    lineups.remove(agent);
                }
            }
        }
        held.answers.extend(asked);
        lineups
    };
    publish(roots, lineups, None, now_ms, true);
}

/// The agents whose own CLI's list is due: none held, or the one held past
/// [`REFRESH_AFTER_MS`].
fn lists_due(heard: &BTreeMap<String, cli_models::Snapshot>, now_ms: i64) -> Vec<&'static str> {
    lineup_agents()
        .filter(|agent| cli_models::road_of(agent).is_some())
        .filter(|agent| {
            heard
                .get(*agent)
                .is_none_or(|held| now_ms.saturating_sub(held.read_at_ms) >= REFRESH_AFTER_MS)
        })
        .collect()
}

/// The lineup a CLI's own list gives, from what it last said.
fn listed_lineup(agent: &str, heard: Option<&cli_models::Snapshot>) -> Option<Lineup> {
    heard.and_then(|heard| Lineup::from_listed(agent, heard.answer.listed()))
}

/// Lay `lineups` against the book, hold them — with zo's report on its
/// sources when this read has one — and tell the window; `done` counts the
/// read finished for [`refresh_and_wait`].
fn publish(
    roots: &Roots,
    lineups: BTreeMap<String, Lineup>,
    sources: Option<Sources>,
    now_ms: i64,
    done: bool,
) {
    let path = roots.config_root.join(SEEN_FILE);
    let mut seen = load_seen(&path);
    let changes = observe(&mut seen, &lineups, now_ms);
    if !changes.is_empty() {
        let _ = save_seen(&path, &seen);
    }
    let said = superseded(&lineups);
    {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held.snapshot = Snapshot { lineups, seen };
        if let Some(sources) = sources {
            held.sources = sources;
        }
        held.read_at_ms = Some(now_ms);
        if done {
            held.finished += 1;
        }
    }
    if done {
        finished().notify_all();
    }
    if let Some(app) = WINDOW.get() {
        if !changes.is_empty() {
            let _ = app.emit(CHANGED_EVENT, &changes);
        }
        let _ = app.emit(SUPERSEDED_EVENT, &said);
        let _ = app.emit(READ_EVENT, now_ms);
    }
}

/// Read again now after a local settings change (a connection saved or
/// removed): zo's in-memory catalog cache is forgotten first, so the read asks
/// zo rather than the window's 60-second cache; a read already running
/// finishes and is followed by this one.
pub(crate) fn refresh() {
    crate::slash_catalog::forget_zo_models();
    {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.reading {
            held.again = true;
            return;
        }
    }
    read_in_background(crate::usage_runtime::epoch_ms_now(), false);
}

/// Re-query live provider registries via `zo models --refresh --json` in the
/// background: a login switch, a Google sign-in/sign-out, or an explicit
/// 「목록 새로고침」.
pub(crate) fn force_refresh() {
    crate::slash_catalog::refresh_zo_models();
    {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.reading {
            held.again = true;
            held.force_next = true;
            return;
        }
    }
    read_in_background(crate::usage_runtime::epoch_ms_now(), true);
}

/// [`force_refresh`], and wait — up to [`REFRESH_WAIT`] — for the read that
/// postdates it; the settings screen answers with what that read holds.
pub(crate) fn refresh_and_wait() {
    if ROOTS.get().is_none() {
        return;
    }
    let target = {
        let held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // A read already running answers an ask made before it began; the
        // one after it is the one asked for.
        held.finished + if held.reading { 2 } else { 1 }
    };
    force_refresh();
    let held = held()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _ = finished()
        .wait_timeout_while(held, REFRESH_WAIT, |held| held.finished < target)
        .unwrap_or_else(std::sync::PoisonError::into_inner);
}

/// What the held read was told about its sources.
pub(crate) fn sources() -> Sources {
    held()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .sources
        .clone()
}

/// The settings screen's part of zo's list ([`crate::slash_catalog::zo_model_list`]):
/// each report's diagnosis and every connection, nothing else of either.
fn sources_of(list: &serde_json::Value, now_ms: i64) -> Sources {
    let rows = |key: &str| {
        list.get(key)
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let word = |row: &serde_json::Value, key: &str| {
        row.get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Sources {
        read_at_ms: Some(now_ms),
        reports: rows("reports")
            .iter()
            .map(|row| {
                let diagnosis = row.get("diagnosis");
                Report {
                    provider: word(row, "provider"),
                    source: word(row, "source"),
                    ok: row
                        .get("ok")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    count: row
                        .get("count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    age: row
                        .get("age")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    skipped: diagnosis
                        .and_then(|diagnosis| diagnosis.get("skipped"))
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
                    failure: diagnosis
                        .and_then(|diagnosis| diagnosis.get("kind"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                }
            })
            .collect(),
        connections: rows("customProviders")
            .iter()
            .map(|row| Connection {
                name: word(row, "name"),
                usable: row
                    .get("usable")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                models: row
                    .get("models")
                    .and_then(serde_json::Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect(),
            })
            .filter(|connection| !connection.name.is_empty())
            .collect(),
    }
}

/// Every agent a summons tunes a model and an effort for at a difficulty —
/// the ones a lineup is read for and the settings screen has rows for.
pub(crate) fn tunable_agents() -> impl Iterator<Item = &'static str> {
    zerocode_core::agent::AGENT_SPECS
        .iter()
        .map(|spec| spec.id)
        .filter(|agent| zerocode_core::orchestration::takes_effort(agent))
}

/// Every release `lineups` hold as superseded, by agent, and the one that
/// stands for each today (t-46882) — what the window's sidebar row and
/// status bar say beside a pane's model. An agent none of whose releases has
/// a standing successor is left out.
pub(crate) fn superseded(
    lineups: &BTreeMap<String, Lineup>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    lineups
        .iter()
        .map(|(agent, lineup)| {
            let moved: BTreeMap<String, String> = lineup
                .models
                .iter()
                .filter_map(|model| {
                    Some((
                        model.id.clone(),
                        lineup.current_release(&model.id)?.to_string(),
                    ))
                })
                .collect();
            (agent.clone(), moved)
        })
        .filter(|(_, moved)| !moved.is_empty())
        .collect()
}

/// The release that stands today for `model` in `agent`'s lineup, when the
/// classifier superseded it (t-46882): `snapshot`'s answer, `None` where it
/// holds no lineup for the agent.
pub(crate) fn current_release(snapshot: &Snapshot, agent: &str, model: &str) -> Option<String> {
    snapshot
        .lineups
        .get(agent)?
        .current_release(model)
        .map(str::to_string)
}

/// The lineup of every agent that takes a model at a difficulty and drives
/// one provider — `read` is the catalog road, a test's synthetic one.
fn lineups_of(read: impl Fn(&str) -> Option<Lineup>) -> BTreeMap<String, Lineup> {
    lineup_agents()
        .filter_map(|agent| Some((agent.to_string(), read(agent)?)))
        .collect()
}

/// Every agent a lineup is read for: its row takes a model at launch and
/// names where its models are learned (t-124593). The four zo serves read as
/// before; a CLI that lists its own reads that list.
fn lineup_agents() -> impl Iterator<Item = &'static str> {
    zerocode_core::agent::AGENT_SPECS
        .iter()
        .filter(|spec| {
            spec.harness.tuning.is_some()
                && spec.harness.models != zerocode_core::capabilities::ModelSource::Unknown
        })
        .map(|spec| spec.id)
}

/// Ask `agent`'s own list command, on the binary a launch of it would start,
/// from the window's own process (never a pane, so no nested agent run) and
/// through its quiet spawn door; `None` when its row names no list or this
/// machine does not hold it.
fn ask_own_list(agent: &str, now_ms: i64) -> Option<cli_models::Snapshot> {
    let road = cli_models::road_of(agent)?;
    let path = crate::shell_path::launch_path();
    let program = zerocode_core::agent::agent_spec(agent)?
        .detect_names()
        .find_map(|name| zerocode_core::agent::resolve_on_path(path.as_deref(), name))?;
    // The launch PATH rides through the window's one door for a child given a
    // PATH of its own, which spawns without forking the window (t-47141).
    let command = crate::proc::quiet_command_on_path(&program, path.as_deref());
    Some(cli_models::ask(
        command,
        agent,
        &road,
        cli_models::ASK_LIMIT,
        now_ms,
    ))
}

/// The models an agent's own CLI last listed, peeked from the held lineup
/// without starting a subprocess (safe for hot paths like `pane_log`).
pub(crate) fn peek_own_list_models(agent: &str) -> Vec<cli_models::Listed> {
    let held = held()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.answers
        .get(agent)
        .map(|heard| heard.answer.listed().to_vec())
        .unwrap_or_default()
}

/// The models an agent's own CLI listed, from the window's one lineup cache —
/// triggering a background refresh when stale, or asking the CLI once when
/// nothing has been read yet (e.g. a composer menu opened right after boot).
pub(crate) fn own_list_models(agent: &str) -> Vec<cli_models::Listed> {
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let existing = {
        let held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        held.answers.get(agent).cloned()
    };
    if let Some(heard) = existing {
        if now_ms.saturating_sub(heard.read_at_ms) >= REFRESH_AFTER_MS {
            let _ = snapshot();
        }
        return heard.answer.listed().to_vec();
    }
    let Some(answer) = ask_own_list(agent, now_ms) else {
        return Vec::new();
    };
    let listed = answer.answer.listed().to_vec();
    {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(lineup) = listed_lineup(agent, Some(&answer)) {
            held.snapshot.lineups.insert(agent.to_string(), lineup);
        }
        held.answers.insert(agent.to_string(), answer);
    }
    listed
}

/// What `agent`'s models were last read as, for `agent-list`: its own CLI's
/// answer, or the lineup zo's catalog gave it, said the same way; `None`
/// while nothing has been read. An old read sends one more, as every peek
/// here does.
pub(crate) fn models_of(agent: &str) -> Option<cli_models::Snapshot> {
    let lineups = snapshot().lineups;
    let held = held()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(heard) = held.answers.get(agent) {
        return Some(heard.clone());
    }
    let read_at_ms = held.read_at_ms?;
    let lineup = lineups.get(agent)?;
    Some(cli_models::Snapshot {
        agent: agent.to_string(),
        asked: cli_models::said_as(
            cli_models::ZO_CATALOG_PROGRAM,
            &cli_models::ZO_CATALOG_WORDS,
        ),
        answer: cli_models::Answer::Listed {
            models: lineup
                .models
                .iter()
                .filter(|model| !model.folded())
                .map(|model| cli_models::Listed {
                    id: model.id.clone(),
                    name: None,
                    family: None,
                    context_window: None,
                })
                .collect(),
            default: None,
        },
        read_at_ms,
        took_ms: 0,
    })
}

/// What one agent's lineup changed, as the notice line reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentChange {
    pub(crate) agent: String,
    #[serde(flatten)]
    pub(crate) change: Change,
}

/// Lay every lineup with difficulty rows against the book; the agents whose
/// lineup moved. An agent whose launch takes no effort has no rows
/// (`lineup::build`) and its models are only ever called by name, so a
/// model arriving there is no news: the notice says 「…의 쉬운 일부터 써
/// 봅니다」, a promise only a row keeps (the review's F2, t-124593 — devin,
/// kiro and grok's lists move often).
fn observe(seen: &mut Seen, lineups: &BTreeMap<String, Lineup>, now_ms: i64) -> Vec<AgentChange> {
    lineups
        .iter()
        .filter(|(agent, _)| zerocode_core::orchestration::takes_effort(agent))
        .filter_map(|(agent, lineup)| {
            let change = seen.observe(agent, lineup, now_ms);
            (!change.is_empty()).then(|| AgentChange {
                agent: agent.clone(),
                change,
            })
        })
        .collect()
}

fn load_seen(path: &Path) -> Seen {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Written beside itself and moved into place, so a reader never meets half
/// a book.
fn save_seen(path: &Path, seen: &Seen) -> std::io::Result<()> {
    let body = serde_json::to_vec_pretty(seen).map_err(std::io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let beside = path.with_extension("json.tmp");
    std::fs::write(&beside, body)?;
    std::fs::rename(&beside, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lineup(ids: &[(&str, bool)]) -> Lineup {
        Lineup::from_catalog(
            &json!({"models": ids.iter().map(|(id, builtin)| json!({
                "provider": "p", "id": id, "builtin": builtin, "band": "rest", "rungs": ["easy"],
            })).collect::<Vec<_>>()}),
            "p",
        )
        .unwrap()
    }

    /// A lineup is read for every agent whose row takes a model and names
    /// where its models are learned — the four zo serves and the CLIs that
    /// list their own (t-124593) — and for no other.
    #[test]
    fn only_agents_that_take_a_model_and_name_their_source_get_a_lineup() {
        let read = lineups_of(|_| Some(lineup(&[("model-a", true)])));
        let agents: Vec<&str> = read.keys().map(String::as_str).collect();
        for old in ["zo", "claude", "codex", "antigravity"] {
            assert!(agents.contains(&old), "{old} lost its lineup");
        }
        for listed in ["devin", "kiro", "grok"] {
            assert!(agents.contains(&listed), "{listed} reads no lineup");
        }
        for unknown in ["qwen-code", "cursor", "opencode"] {
            assert!(
                !agents.contains(&unknown),
                "{unknown} reads a lineup from nowhere"
            );
        }
        assert!(lineups_of(|_| None).is_empty(), "no catalog, no lineup");
    }

    /// A model arriving beside an agent with no difficulty rows is no news —
    /// nothing promises to use it (the review's F2, t-124593); the same
    /// arrival beside an agent with rows is.
    #[test]
    fn only_an_agent_with_difficulty_rows_is_told_of_a_model_that_arrived() {
        let mut seen = Seen::default();
        let first = BTreeMap::from([
            ("claude".to_string(), lineup(&[("model-a", true)])),
            ("devin".to_string(), lineup(&[("model-a", true)])),
        ]);
        assert!(
            observe(&mut seen, &first, 1_000).is_empty(),
            "a first look is no news"
        );
        let grown = BTreeMap::from([
            (
                "claude".to_string(),
                lineup(&[("model-a", true), ("model-b", false)]),
            ),
            (
                "devin".to_string(),
                lineup(&[("model-a", true), ("model-b", false)]),
            ),
        ]);
        let told: Vec<String> = observe(&mut seen, &grown, 2_000)
            .into_iter()
            .map(|one| one.agent)
            .collect();
        assert_eq!(told, ["claude"]);
    }

    /// A CLI's list is asked again only when none is held or the one held is
    /// past the refresh rule — so a login switch's refresh between two hours
    /// asks no CLI at all (t-124593).
    #[test]
    fn a_cli_list_is_asked_again_only_when_none_is_held_or_it_is_old() {
        let heard = |agent: &str, read_at_ms: i64| {
            (
                agent.to_string(),
                cli_models::Snapshot {
                    agent: agent.to_string(),
                    asked: String::new(),
                    answer: cli_models::Answer::NoAnswer,
                    read_at_ms,
                    took_ms: 0,
                },
            )
        };
        let now = 10 * REFRESH_AFTER_MS;
        assert_eq!(lists_due(&BTreeMap::new(), now), ["devin", "kiro"]);
        let held = BTreeMap::from([
            heard("devin", now - 1),
            heard("kiro", now - REFRESH_AFTER_MS),
        ]);
        assert_eq!(lists_due(&held, now), ["kiro"]);
    }

    /// t-46882: what the window is told beside a pane's model — each
    /// superseded release with the one standing for it, by agent; an agent
    /// with nothing superseded says nothing, and neither does a lineup whose
    /// classifier named no successor.
    #[test]
    fn the_window_hears_which_releases_stand_for_the_superseded_ones() {
        let read = |rows: serde_json::Value| Lineup::from_catalog(&json!({ "models": rows }), "p");
        let lineups = BTreeMap::from([
            (
                "claude".to_string(),
                read(json!([
                    {"provider": "p", "id": "model-a-5", "band": "superseded", "supersededBy": "model-a-5-1"},
                    {"provider": "p", "id": "model-a-5-1", "band": "top", "rungs": []},
                ]))
                .unwrap(),
            ),
            (
                "codex".to_string(),
                read(json!([
                    {"provider": "p", "id": "model-b", "band": "second", "rungs": ["hard"]},
                    {"provider": "p", "id": "model-twin", "band": "superseded"},
                ]))
                .unwrap(),
            ),
        ]);
        assert_eq!(
            serde_json::to_value(superseded(&lineups)).unwrap(),
            json!({"claude": {"model-a-5": "model-a-5-1"}})
        );
        let snapshot = Snapshot {
            lineups,
            seen: Seen::default(),
        };
        assert_eq!(
            current_release(&snapshot, "claude", "model-a-5").as_deref(),
            Some("model-a-5-1")
        );
        assert_eq!(current_release(&snapshot, "codex", "model-twin"), None);
        assert_eq!(
            current_release(&snapshot, "zo", "model-a-5"),
            None,
            "no lineup"
        );
    }

    /// What the worker rows say beside the lineup: each source's answer —
    /// a failed one only by the window's diagnosis — and every connection, a
    /// keyless one too, with nothing else of either (no credential store,
    /// no failure detail, no key variable).
    #[test]
    fn the_rows_hear_each_sources_answer_and_every_connection() {
        let list = json!({
            "reports": [
                {"provider": "anthropic", "source": "anthropic-api", "ok": true, "count": 14,
                 "age": "6m old", "credentialStore": "CLAUDE_CONFIG_DIR:/Users/dev/x"},
                {"provider": "xai", "source": "xai-api", "ok": false, "count": 0,
                 "diagnosis": {"status": null, "kind": null, "skipped": true}},
                {"provider": "openai", "source": "chatgpt-backend", "ok": false, "count": 0,
                 "diagnosis": {"status": 401, "kind": "auth", "skipped": false}},
            ],
            "customProviders": [
                {"name": "custom-a", "usable": true, "models": ["custom-a/model-x"],
                 "credentialEnvVars": ["ZEROCODE_ROUTER_CUSTOM_A_KEY"]},
                {"name": "custom-b", "usable": false, "models": ["custom-b/model-y"]},
            ],
        });
        let said = serde_json::to_value(sources_of(&list, 7)).unwrap();
        assert_eq!(
            said,
            json!({
                "readAtMs": 7,
                "reports": [
                    {"provider": "anthropic", "source": "anthropic-api", "ok": true, "count": 14,
                     "age": "6m old", "skipped": false, "failure": null},
                    {"provider": "xai", "source": "xai-api", "ok": false, "count": 0,
                     "age": null, "skipped": true, "failure": null},
                    {"provider": "openai", "source": "chatgpt-backend", "ok": false, "count": 0,
                     "age": null, "skipped": false, "failure": "auth"},
                ],
                "connections": [
                    {"name": "custom-a", "usable": true, "models": ["custom-a/model-x"]},
                    {"name": "custom-b", "usable": false, "models": ["custom-b/model-y"]},
                ],
            })
        );
        assert_eq!(sources_of(&json!({}), 1).connections, Vec::new());
    }

    #[test]
    fn an_arrival_is_said_once_and_the_book_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SEEN_FILE);
        let mut seen = load_seen(&path);
        let first = BTreeMap::from([("claude".to_string(), lineup(&[("model-a", true)]))]);
        assert!(
            observe(&mut seen, &first, 1).is_empty(),
            "shipped models are not news"
        );
        save_seen(&path, &seen).unwrap();
        let mut seen = load_seen(&path);
        let second = BTreeMap::from([(
            "claude".to_string(),
            lineup(&[("model-a", true), ("model-b", false)]),
        )]);
        let said = observe(&mut seen, &second, 2);
        assert_eq!(said.len(), 1);
        assert_eq!(said[0].agent, "claude");
        assert_eq!(said[0].change.entered, ["model-b"]);
        assert_eq!(
            serde_json::to_value(&said[0]).unwrap(),
            json!({"agent": "claude", "entered": ["model-b"], "folded": []}),
            "the notice's payload"
        );
        save_seen(&path, &seen).unwrap();
        let mut seen = load_seen(&path);
        assert!(
            observe(&mut seen, &second, 3).is_empty(),
            "said once, across a restart"
        );
    }

    #[test]
    fn periodic_refresh_is_due_when_unseeded_or_at_the_hour_boundary() {
        assert!(due_for_periodic_refresh(None, 1_000));
        assert!(!due_for_periodic_refresh(
            Some(1_000),
            1_000 + REFRESH_AFTER_MS - 1
        ));
        assert!(due_for_periodic_refresh(
            Some(1_000),
            1_000 + REFRESH_AFTER_MS
        ));
        assert!(due_for_periodic_refresh(
            Some(1_000),
            1_000 + 8 * REFRESH_AFTER_MS
        ));
    }

    /// The strings in `text` shaped like a model id — a lowercase word, a
    /// hyphen and a digit somewhere (`example-family-2-max`) — on lines that
    /// are not comments.
    fn model_shaped(text: &str) -> Vec<String> {
        let shaped = |word: &str| {
            word.starts_with(|first: char| first.is_ascii_lowercase())
                && word.contains('-')
                && word.contains(|one: char| one.is_ascii_digit())
                && word.chars().all(|one| {
                    one.is_ascii_lowercase() || one.is_ascii_digit() || one == '-' || one == '.'
                })
        };
        let quoted = |line: &str| {
            let mut found = Vec::new();
            let mut open: Option<String> = None;
            let mut escaped = false;
            for one in line.chars() {
                if let Some(word) = open.as_mut() {
                    if escaped {
                        word.push(one);
                        escaped = false;
                    } else if one == '\\' {
                        escaped = true;
                    } else if one == '"' {
                        found.extend(open.take());
                    } else {
                        word.push(one);
                    }
                } else if one == '"' {
                    open = Some(String::new());
                }
            }
            found
        };
        text.lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .flat_map(quoted)
            .filter(|word| shaped(word))
            .collect()
    }

    /// t-125871 (contract): the code that picks a summons' model names no
    /// model — every name a summons launches comes from the list the window
    /// read or the row the person wrote (the person, 11-22: 「하드코딩금지
    /// … 모델목록을 보고 거기서 뽑아야함」). Read as it ships, in this crate
    /// and in the core: every file under a name that starts with `summon`
    /// (code and data alike), the CLI-list reader and the agent catalog
    /// whose rows put a model on a command line — each without its trailing
    /// test module, test files skipped.
    #[test]
    fn no_model_name_is_written_in_the_code_that_picks_a_summons_model() {
        fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).expect("a source folder").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, into);
                } else {
                    into.push(path);
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the crates folder");
        let named = ["cli_models.rs", "agent.rs", "capabilities.rs"];
        let mut read = Vec::new();
        for source in ["zerocode-shell", "zerocode-core"].map(|one| crates.join(one).join("src")) {
            let mut files = Vec::new();
            walk(&source, &mut files);
            for path in files {
                let within = path
                    .strip_prefix(&source)
                    .expect("inside its source folder");
                let summons = within
                    .iter()
                    .any(|part| part.to_str().is_some_and(|part| part.starts_with("summon")));
                let test = within.iter().any(|part| {
                    part.to_str().is_some_and(|part| {
                        part == "tests" || part == "tests.rs" || part.ends_with("_tests.rs")
                    })
                });
                let catalog = within
                    .to_str()
                    .is_some_and(|within| named.contains(&within));
                if (summons || catalog) && !test {
                    read.push(path);
                }
            }
        }
        assert!(read.len() > named.len(), "{read:?}");
        let mut named_in_code = Vec::new();
        for path in &read {
            let text = std::fs::read_to_string(path).expect("a source file");
            let shipped = text
                .split("\n#[cfg(test)]\nmod tests {")
                .next()
                .unwrap_or_default();
            for word in model_shaped(shipped) {
                named_in_code.push(format!("{}: {word}", path.display()));
            }
        }
        assert!(
            named_in_code.is_empty(),
            "a model is named in the code that picks one: {named_in_code:#?}"
        );
    }
}
