//! One difficulty request per successful summons. Recording never delays the
//! pane; an acting request's receipt is carried through reservation instead.
use crate::agent_teams::Host;
use crate::systemone::{Wire, request_body};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use zerocode_core::jev::SUMMON_DIFFICULTY;
use zerocode_core::orchestration::PreparedWorkerStart;
use zerocode_core::summon_difficulty::{self as difficulty, Look};

fn ask(wire: &Wire, look: &Look, checkout: Option<&Path>) -> Value {
    let mut row = json!({
        "rubricVersion": difficulty::RUBRIC_VERSION,
        "attempt": look.attempt,
        "failures": look.failures,
        "retryOf": look.retry_of,
        "options": difficulty::LADDER.iter().map(|(key, _, _)| *key).collect::<Vec<_>>(),
        "applied": false,
    });
    let began = Instant::now();
    let answer = wire.ask(
        &SUMMON_DIFFICULTY,
        checkout,
        request_body(&look.state(), &difficulty::questions()),
        Duration::from_millis(difficulty::APPLY_DEADLINE_MS),
    );
    row["elapsedMs"] = json!(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
    row["requestBytes"] = json!(answer.request_bytes);
    answer.spent.stamp(&mut row);
    let read = answer.answer.and_then(|body| {
        let parsed: Value =
            serde_json::from_str(&body).map_err(|_| crate::systemone::SCHEMA.to_string())?;
        difficulty::read(&parsed["answers"]).map_err(|err| err.token().to_string())
    });
    match read {
        Ok(pick) => {
            row["outcome"] = json!("answered");
            row["chosen"] = json!(pick.chosen);
            row["confidence"] = json!(pick.confidence);
            row["probabilities"] = json!(pick.probabilities);
            row["applied"] = json!(
                crate::systemone::applies(wire, &SUMMON_DIFFICULTY)
                    && SUMMON_DIFFICULTY.acts_on(
                        pick.confidence,
                        crate::systemone::act_line(wire, &SUMMON_DIFFICULTY)
                    )
            );
        }
        Err(token) => row["outcome"] = json!(token),
    }
    row
}

type OriginKey = [String; 3];
#[derive(Clone)]
struct HostOrigin {
    checkout: Option<std::path::PathBuf>,
    settings: Value,
    fresh: bool,
    /// Today's lineups as the window held them when the summons began —
    /// peeked outside the actor, like the settings (t-14437).
    lineup: crate::summon_lineup::Snapshot,
}
fn origins() -> &'static std::sync::Mutex<std::collections::HashMap<OriginKey, HostOrigin>> {
    static ORIGINS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<OriginKey, HostOrigin>>,
    > = std::sync::OnceLock::new();
    ORIGINS.get_or_init(Default::default)
}
/// Ephemeral host observation for one actor call, never a caller-supplied path.
pub(super) struct Origin(OriginKey);
impl Drop for Origin {
    fn drop(&mut self) {
        origins()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.0);
    }
}
pub(super) fn origin(key: [&str; 3], checkout: Option<std::path::PathBuf>, fresh: bool) -> Origin {
    // Read the table outside the ledger actor. A handover has already sealed
    // its launch settings, including an explicitly inherited CLI default.
    origin_held(
        key,
        checkout,
        fresh,
        Wire::of_this_machine().settings_root(),
        crate::summon_lineup::snapshot(),
    )
}
fn origin_with(
    key: [&str; 3],
    checkout: Option<std::path::PathBuf>,
    fresh: bool,
    settings: Value,
) -> Origin {
    origin_held(
        key,
        checkout,
        fresh,
        settings,
        crate::summon_lineup::Snapshot::default(),
    )
}
fn origin_held(
    key: [&str; 3],
    checkout: Option<std::path::PathBuf>,
    fresh: bool,
    settings: Value,
    lineup: crate::summon_lineup::Snapshot,
) -> Origin {
    let key = key.map(str::to_string);
    origins()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            key.clone(),
            HostOrigin {
                checkout,
                settings,
                fresh,
                lineup,
            },
        );
    Origin(key)
}

/// [`origin`] with the observation handed in — how a test of another seat
/// that reads the same origin registers one.
#[cfg(test)]
pub(super) fn origin_with_for_tests(
    key: [&str; 3],
    checkout: Option<std::path::PathBuf>,
    fresh: bool,
    settings: Value,
) -> Origin {
    origin_with(key, checkout, fresh, settings)
}

/// The checkout the window observed for a fresh summons under `origin` —
/// the workspace a seat asked while that summons waits asks consent for.
/// `None` for a summons nobody observed and for a handover whose launch was
/// sealed: no seat asks on the beat for either.
pub(super) fn fresh_checkout(origin: [&str; 3]) -> Option<Option<std::path::PathBuf>> {
    origins()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&origin.map(str::to_string))
        .filter(|origin| origin.fresh)
        .map(|origin| origin.checkout.clone())
}

/// [`origin`] with the lineup handed in — how a test gives a summons a
/// synthetic lineup.
#[cfg(test)]
pub(super) fn origin_with_lineup_for_tests(
    key: [&str; 3],
    settings: Value,
    lineup: crate::summon_lineup::Snapshot,
) -> Origin {
    origin_held(key, None, true, settings, lineup)
}

/// The launch row a fresh summons under `origin` takes at `level`: the
/// person's, else the lineup's, else the shipped table's (t-14437).
pub(super) fn profile(
    agent: &str,
    level: &str,
    origin: [&str; 3],
) -> Result<Option<difficulty::lineup::Row>, String> {
    let held = origins()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(origin) = held
        .get(&origin.map(str::to_string))
        .filter(|origin| origin.fresh)
    else {
        return Ok(None);
    };
    difficulty::lineup::row_at(
        &origin.settings,
        agent,
        level,
        origin.lineup.lineups.get(agent),
        Some(&origin.lineup.seen),
        crate::usage_runtime::epoch_ms_now(),
    )
}

/// What the model seat reads for `agent` under a fresh summons's origin —
/// its lineup and book as the origin held them, and this ledger's record of
/// each model folded from both summons seats' outcome rows (t-14437).
pub(super) fn model_facts(
    agent: &str,
    origin: [&str; 3],
) -> Option<zerocode_core::summon_model::Facts> {
    let (lineup, seen) = {
        let held = origins()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let origin = held
            .get(&origin.map(str::to_string))
            .filter(|origin| origin.fresh)?;
        (
            origin.lineup.lineups.get(agent)?.clone(),
            origin.lineup.seen.clone(),
        )
    };
    let wire = Wire::of_this_machine();
    let rows: Vec<Value> = [&SUMMON_DIFFICULTY, &zerocode_core::jev::SUMMON_MODEL]
        .into_iter()
        .filter_map(|seat| crate::systemone::ledger_of(&wire, seat))
        .flat_map(|path| crate::systemone::read_rows(&path))
        .collect();
    let records = zerocode_core::summon_model::records(
        difficulty::outcomes::latest(rows.iter()),
        agent,
        |model| lineup.canonical(model).to_string(),
    );
    Some(zerocode_core::summon_model::Facts {
        lineup,
        seen,
        records,
    })
}

/// Every difficulty's launch row for `agent` as the window reads today —
/// `agent-list`'s peek (t-14437). `None` while no lineup has been read at
/// all, which is "nobody looked" and not "no choices".
pub(super) fn rows(agent: &str) -> Option<Vec<difficulty::lineup::Row>> {
    // `agent-list` asks once per catalog agent; one that cannot launch a
    // difficulty at all has no rows, and costs no settings read.
    zerocode_core::orchestration::difficulty_effort(agent, difficulty::LADDER[0].0)?;
    rows_in(
        &Wire::of_this_machine().settings_root(),
        agent,
        &crate::summon_lineup::snapshot(),
    )
}

fn rows_in(
    settings: &Value,
    agent: &str,
    lineup: &crate::summon_lineup::Snapshot,
) -> Option<Vec<difficulty::lineup::Row>> {
    if lineup.lineups.is_empty() {
        return None;
    }
    difficulty::lineup::rows(
        settings,
        agent,
        lineup.lineups.get(agent),
        Some(&lineup.seen),
        crate::usage_runtime::epoch_ms_now(),
    )
    .ok()
}

pub(super) fn choose(look: &Look, origin: [&str; 3]) -> Option<Value> {
    choose_with(&Wire::of_this_machine(), look, origin)
}

fn choose_with(wire: &Wire, look: &Look, origin: [&str; 3]) -> Option<Value> {
    if !crate::systemone::applies(wire, &SUMMON_DIFFICULTY) {
        return None;
    }
    let context = origins()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&origin.map(str::to_string))
        .cloned();
    if context.as_ref().is_some_and(|origin| !origin.fresh) {
        return None;
    }
    let checkout = context.and_then(|origin| origin.checkout);
    Some(ask(wire, look, checkout.as_deref()))
}

pub(super) fn record(
    host: &dyn Host,
    prepared: &PreparedWorkerStart,
    checkout: Option<&str>,
    now_ms: i64,
) {
    let Some(shadow) = prepared.difficulty_shadow.clone() else {
        return;
    };
    let Some(wire) = host.jev_wire() else { return };
    let mode = SUMMON_DIFFICULTY.mode_in(&wire.settings_root());
    let Some(ledger) =
        crate::systemone::ledger_of(&wire, &SUMMON_DIFFICULTY).filter(|_| mode.asks())
    else {
        return;
    };
    let run = prepared.run.clone();
    let worker = prepared.worker.clone();
    let dispatch = prepared.dispatch.clone();
    let task = prepared.task.clone();
    let agent = prepared.agent.clone();
    let execution_model = prepared
        .summon_shadow
        .as_ref()
        .and_then(|shadow| shadow.pinned.model.clone());
    let executed_effort = prepared
        .summon_shadow
        .as_ref()
        .and_then(|shadow| shadow.pinned.effort.clone());
    let checkout = checkout.map(std::path::PathBuf::from);
    host.off_the_beat(Box::new(move || {
        let mut row = shadow.receipt.unwrap_or_else(|| {
            let mut row = ask(&wire, &shadow.look, checkout.as_deref());
            // A background answer was never used for this launch.
            row["applied"] = json!(false);
            row
        });
        row["at"] = json!(now_ms);
        row["requestAt"] = json!(now_ms);
        row["run"] = json!(run);
        row["worker"] = json!(worker);
        // Taskless summonses have a stable worker identity instead of a null
        // request name (which would join unrelated rows).
        row["dispatch"] = json!(dispatch.unwrap_or(worker));
        row["task"] = json!(task);
        row["mode"] = json!(mode.key());
        row["agent"] = json!(agent);
        row["executionModel"] = json!(execution_model);
        row["effort"] = json!(executed_effort);
        row["pinnedEffort"] = json!(shadow.teacher_effort);
        row["baselineHigh"] = json!(shadow.baseline_high);
        if row["outcome"] == "answered" {
            difficulty::compare(&mut row, shadow.teacher_effort.as_deref());
        }
        crate::systemone::record_rows(&SUMMON_DIFFICULTY, &ledger, &[row], now_ms);
    }));
}

/// Current observations, including revisions after a retry or late usage scan.
/// Request identity and the launch fields are carried, never reconstructed
/// from mutable worker tuning. No transcript or human ledger is written.
pub(super) fn observations(
    ledger: &zerocode_core::orchestration::Ledger,
    costs: &mut super::cost_book::CostBook,
) -> Option<(std::path::PathBuf, Vec<Value>)> {
    observations_of(&SUMMON_DIFFICULTY, ledger, costs)
}

/// [`observations`] of either summons seat that is graded by the work its
/// answers launched — difficulty, or the model (t-14437).
pub(super) fn observations_of(
    seat: &zerocode_core::jev::JevUse,
    ledger: &zerocode_core::orchestration::Ledger,
    costs: &mut super::cost_book::CostBook,
) -> Option<(std::path::PathBuf, Vec<Value>)> {
    let wire = Wire::of_this_machine();
    let path = crate::systemone::ledger_of(&wire, seat)?;
    observations_in(path, seat.rubric_version, ledger, costs)
}

/// [`observations`] of the seat's ledger at `path`.
#[cfg(test)]
fn observations_at(
    path: std::path::PathBuf,
    ledger: &zerocode_core::orchestration::Ledger,
    costs: &mut super::cost_book::CostBook,
) -> Option<(std::path::PathBuf, Vec<Value>)> {
    observations_in(path, difficulty::RUBRIC_VERSION, ledger, costs)
}

fn observations_in(
    path: std::path::PathBuf,
    rubric: u32,
    ledger: &zerocode_core::orchestration::Ledger,
    costs: &mut super::cost_book::CostBook,
) -> Option<(std::path::PathBuf, Vec<Value>)> {
    let rows = crate::systemone::read_rows(&path);
    let latest = difficulty::outcomes::latest(rows.iter());
    let mut changed = Vec::new();
    for request in rows
        .iter()
        .filter(|row| row["outcome"] == "answered" && row["rubricVersion"] == rubric)
    {
        let Some(run) = request["run"].as_str().and_then(|id| ledger.run(id)) else {
            continue;
        };
        let Some(dispatch) = request["dispatch"].as_str().and_then(|id| run.dispatch(id)) else {
            continue;
        };
        let generation = costs.attempt_generation(run, dispatch);
        let total = costs.cost(run, run.task(&dispatch.task)?);
        let Some(outcome) = difficulty::outcomes::observe(run, dispatch, &generation, &total)
        else {
            continue;
        };
        let outcome = serde_json::to_value(outcome).ok()?;
        if latest.iter().any(|row| {
            row["run"] == request["run"]
                && row["dispatch"] == request["dispatch"]
                && row["requestAt"] == request["requestAt"]
                && row[difficulty::outcomes::KEY] == outcome
        }) {
            continue;
        }
        let mut row = json!({"label": request["dispatch"], difficulty::outcomes::KEY: outcome});
        for key in [
            "run",
            "worker",
            "task",
            "dispatch",
            "requestAt",
            "rubricVersion",
            "chosen",
            "agent",
            "executionModel",
            "effort",
            "pinnedEffort",
            "baselineHigh",
            "applied",
            "attempt",
            "retryOf",
            zerocode_core::summon_model::CHALLENGE_KEY,
        ] {
            row[key] = request[key].clone();
        }
        if request[zerocode_core::summon_model::CHALLENGE_KEY] == true {
            // A challenger's turn ran another model than any answer named:
            // its outcome is the model's record, and grades no answer.
            row[zerocode_core::jev::summary::NOT_COMPARED.canonical] =
                json!(zerocode_core::summon_model::CHALLENGE_KEY);
        } else if request["applied"] == true {
            if let Some(success) = row[difficulty::outcomes::KEY]["firstAttemptSuccess"].as_bool() {
                row[zerocode_core::jev::summary::AGREED.canonical] = json!(success);
            }
        } else if let Some(success) =
            row[difficulty::outcomes::KEY]["firstAttemptSuccess"].as_bool()
        {
            row[zerocode_core::jev::summary::BASELINE_AGREED.canonical] = json!(success);
        }
        changed.push(row);
    }
    Some((path, changed))
}

pub(super) fn record_observations(
    observations: Option<(std::path::PathBuf, Vec<Value>)>,
    now_ms: i64,
) {
    record_observations_of(&SUMMON_DIFFICULTY, observations, now_ms);
}

/// [`record_observations`] into `seat`'s own ledger.
pub(super) fn record_observations_of(
    seat: &'static zerocode_core::jev::JevUse,
    observations: Option<(std::path::PathBuf, Vec<Value>)>,
    now_ms: i64,
) {
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some((path, mut rows)) = observations.filter(|(_, rows)| !rows.is_empty()) {
        let _writer = WRITER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let held = crate::systemone::read_rows(&path);
        let latest = difficulty::outcomes::latest(&held);
        rows.retain(|row| {
            !latest.iter().any(|old| {
                old["run"] == row["run"]
                    && old["dispatch"] == row["dispatch"]
                    && old["requestAt"] == row["requestAt"]
                    && old[difficulty::outcomes::KEY] == row[difficulty::outcomes::KEY]
            })
        });
        if rows.is_empty() {
            return;
        }
        for row in &mut rows {
            row["at"] = json!(now_ms);
        }
        crate::systemone::record_rows(seat, &path, &rows, now_ms);
    }
}

#[cfg(test)]
pub(super) mod tests;
