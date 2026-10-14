//! Today's model lineup for every agent a summons can tune, as this window
//! last read it (t-14437).
//!
//! The lineup is the catalog the model chip already reads (`zo models
//! --json`, [`crate::slash_catalog::agent_lineup`]) — no second road to a
//! provider. Readers inside the ledger actor only PEEK at what is held: a
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
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::Emitter;
use zerocode_core::summon_difficulty::lineup::{Change, Lineup, Seen};

/// How long a read lineup stands before a reader sends another — zo's own
/// connection rule for its sources (`model_discovery::LIVE_TTL_SECS`, an
/// hour); an earlier read would ask zo for the answer it already holds.
const REFRESH_AFTER_MS: i64 = 60 * 60 * 1000;
/// The book of models seen, under the window's config root.
const SEEN_FILE: &str = "summon-lineup-seen.json";
/// What the window's one-line notice listens for.
pub(crate) const CHANGED_EVENT: &str = "summon-lineup:changed";
/// What every read says of the releases it holds as superseded
/// ([`superseded`]), for the words the window puts beside a pane's model
/// (t-46882).
pub(crate) const SUPERSEDED_EVENT: &str = "summon-lineup:superseded";

/// Every lineup and the book, as a reader takes them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) lineups: BTreeMap<String, Lineup>,
    pub(crate) seen: Seen,
}

#[derive(Default)]
struct Held {
    read_at_ms: Option<i64>,
    snapshot: Snapshot,
    reading: bool,
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

static ROOTS: OnceLock<Roots> = OnceLock::new();
static WINDOW: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Named once at boot, and the first read sent; until then every reader is
/// answered with nothing held, which the rows read as "no lineup".
pub(crate) fn configure(app: tauri::AppHandle, home: PathBuf, config_root: PathBuf) {
    let _ = WINDOW.set(app);
    let _ = ROOTS.set(Roots { home, config_root });
    read_in_background(crate::usage_runtime::epoch_ms_now());
}

/// What is held, whatever its age; an old answer sends one read.
pub(crate) fn snapshot() -> Snapshot {
    let now_ms = crate::usage_runtime::epoch_ms_now();
    let (snapshot, stale) = {
        let held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let stale = held
            .read_at_ms
            .is_none_or(|at| now_ms.saturating_sub(at) >= REFRESH_AFTER_MS);
        (held.snapshot.clone(), stale)
    };
    if stale {
        read_in_background(now_ms);
    }
    snapshot
}

fn read_in_background(now_ms: i64) {
    let Some(roots) = ROOTS.get() else { return };
    {
        let mut held = held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.reading {
            return;
        }
        held.reading = true;
    }
    let spawned = std::thread::Builder::new()
        .name("summon-lineup".into())
        .spawn(move || {
            let lineups =
                lineups_of(|agent| crate::slash_catalog::agent_lineup(agent, &roots.home));
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
                held.read_at_ms = Some(now_ms);
                held.reading = false;
            }
            if let Some(app) = WINDOW.get() {
                if !changes.is_empty() {
                    let _ = app.emit(CHANGED_EVENT, &changes);
                }
                let _ = app.emit(SUPERSEDED_EVENT, &said);
            }
        });
    if spawned.is_err() {
        held()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .reading = false;
    }
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
    zerocode_core::agent::AGENT_SPECS
        .iter()
        .filter(|spec| {
            zerocode_core::orchestration::difficulty_effort(
                spec.id,
                zerocode_core::summon_difficulty::LADDER[0].0,
            )
            .is_some()
        })
        .filter_map(|spec| Some((spec.id.to_string(), read(spec.id)?)))
        .collect()
}

/// What one agent's lineup changed, as the notice line reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentChange {
    pub(crate) agent: String,
    #[serde(flatten)]
    pub(crate) change: Change,
}

/// Lay every lineup against the book; the agents whose lineup moved.
fn observe(seen: &mut Seen, lineups: &BTreeMap<String, Lineup>, now_ms: i64) -> Vec<AgentChange> {
    lineups
        .iter()
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

    #[test]
    fn only_agents_that_take_a_model_at_a_difficulty_get_a_lineup() {
        let read = lineups_of(|_| Some(lineup(&[("model-a", true)])));
        assert!(!read.is_empty());
        for agent in read.keys() {
            assert!(
                zerocode_core::orchestration::difficulty_effort(
                    agent,
                    zerocode_core::summon_difficulty::LADDER[0].0
                )
                .is_some(),
                "{agent} takes no effort at a difficulty"
            );
        }
        assert!(lineups_of(|_| None).is_empty(), "no catalog, no lineup");
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
}
