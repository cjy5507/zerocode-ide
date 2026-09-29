//! One model request per summons that leaves `--model` open (t-14437) — the
//! difficulty seat's shape: asked while the summons is planned, under the
//! same two-second wall, and its row recorded once the pane really opens.
//! The question itself, and which answer runs, are the core's
//! (`zerocode_core::summon_model`, the worker-start plan); this is the wire.
use crate::agent_teams::Host;
use crate::systemone::{Wire, request_body};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use zerocode_core::jev::SUMMON_MODEL;
use zerocode_core::orchestration::PreparedWorkerStart;
use zerocode_core::summon_model::{self as model, ModelAsk};

pub(super) fn ask(wire: &Wire, asked: &ModelAsk, checkout: Option<&Path>) -> Value {
    // The task's own attempt history, as the difficulty seat's rows carry
    // it: what the judge reads to count one sample per task.
    let mut row = json!({
        "rubricVersion": model::RUBRIC_VERSION,
        "attempt": asked.state["attempt"],
        "failures": asked.state["failures"],
        "retryOf": asked.state["retryOf"],
        "options": asked.offered().iter().map(|(offered, _)| offered).collect::<Vec<_>>(),
        "applied": false,
    });
    let began = Instant::now();
    let answer = wire.ask(
        &SUMMON_MODEL,
        checkout,
        request_body(&asked.state, &asked.questions),
        Duration::from_millis(zerocode_core::summon_difficulty::APPLY_DEADLINE_MS),
    );
    row["elapsedMs"] = json!(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
    row["requestBytes"] = json!(answer.request_bytes);
    answer.spent.stamp(&mut row);
    let read = answer.answer.and_then(|body| {
        let parsed: Value =
            serde_json::from_str(&body).map_err(|_| crate::systemone::SCHEMA.to_string())?;
        asked
            .read(&parsed["answers"])
            .map_err(|err| err.token().to_string())
    });
    match read {
        Ok(pick) => {
            row["outcome"] = json!("answered");
            row["chosen"] = json!(
                pick.chosen
                    .as_ref()
                    .map_or(model::ABSTAIN, |(chosen, _)| chosen.as_str())
            );
            row[model::EFFORT_KEY] = json!(pick.chosen.as_ref().map(|(_, effort)| effort));
            row["confidence"] = json!(pick.confidence);
            row["probabilities"] = json!(pick.probabilities);
            // `abstain` leaves the ladder's default: an answer, never an act.
            row["applied"] = json!(
                pick.chosen.is_some()
                    && crate::systemone::applies(wire, &SUMMON_MODEL)
                    && SUMMON_MODEL.acts_on(
                        pick.confidence,
                        crate::systemone::act_line(wire, &SUMMON_MODEL)
                    )
            );
        }
        Err(token) => row["outcome"] = json!(token),
    }
    row
}

/// The seat's receipt for a fresh summons under `origin` — asked on the
/// launch's own path only while the seat applies; a seat that only records
/// asks after the pane opens ([`record`]), so a summons never waits on an
/// answer nothing will act on.
pub(super) fn choose_with(wire: &Wire, asked: &ModelAsk, origin: [&str; 3]) -> Option<Value> {
    if !crate::systemone::applies(wire, &SUMMON_MODEL) {
        return None;
    }
    let checkout = super::summon_difficulty::fresh_checkout(origin)?;
    Some(ask(wire, asked, checkout.as_deref()))
}

/// The request's row, beside what the summons launched — off the beat.
pub(super) fn record(
    host: &dyn Host,
    prepared: &PreparedWorkerStart,
    checkout: Option<&str>,
    now_ms: i64,
) {
    let Some(shadow) = prepared.model_shadow.clone() else {
        return;
    };
    let Some(wire) = host.jev_wire() else { return };
    let mode = SUMMON_MODEL.mode_in(&wire.settings_root());
    let Some(ledger) = crate::systemone::ledger_of(&wire, &SUMMON_MODEL).filter(|_| mode.asks())
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
            let mut row = ask(&wire, &shadow.ask, checkout.as_deref());
            // A background answer was never used for this launch.
            row["applied"] = json!(false);
            row
        });
        if shadow.challenge {
            row["applied"] = json!(false);
            row[model::CHALLENGE_KEY] = json!(true);
        }
        row["at"] = json!(now_ms);
        row["requestAt"] = json!(now_ms);
        row["run"] = json!(run);
        row["worker"] = json!(worker);
        // Taskless summonses have a stable worker identity instead of a null
        // request name, as the difficulty seat's rows do.
        row["dispatch"] = json!(dispatch.unwrap_or(worker));
        row["task"] = json!(task);
        row["mode"] = json!(mode.key());
        row["agent"] = json!(agent);
        row["executionModel"] = json!(execution_model);
        row["effort"] = json!(executed_effort);
        crate::systemone::record_rows(&SUMMON_MODEL, &ledger, &[row], now_ms);
    }));
}

/// Label the seat's answered rows by the work they launched — the
/// difficulty seat's own reader, over this seat's ledger.
pub(super) fn observations(
    ledger: &zerocode_core::orchestration::Ledger,
    costs: &mut super::cost_book::CostBook,
) -> Option<(std::path::PathBuf, Vec<Value>)> {
    super::summon_difficulty::observations_of(&SUMMON_MODEL, ledger, costs)
}

pub(super) fn record_observations(
    observations: Option<(std::path::PathBuf, Vec<Value>)>,
    now_ms: i64,
) {
    super::summon_difficulty::record_observations_of(&SUMMON_MODEL, observations, now_ms);
}

#[cfg(test)]
mod tests;
