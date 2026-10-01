//! The model question of a summons that leaves `--model` open (t-14437),
//! riding the assign moment's one request beside the difficulty's
//! (`summon_assign`, t-15554), its row recorded once the pane really opens.
//! The question itself, and which answer runs, are the core's
//! (`zerocode_core::summon_model`, the worker-start plan); this is the wire.
use crate::systemone::Wire;
use serde_json::{Value, json};
use std::path::Path;
use zerocode_core::jev::SUMMON_MODEL;
use zerocode_core::summon_model::{self as model, ModelAsk};

/// The seat's row before any answer: the task's own attempt history, as the
/// difficulty seat's rows carry it — what the judge reads to count one
/// sample per task — and the models it chose among.
pub(super) fn head(asked: &ModelAsk) -> Value {
    json!({
        "rubricVersion": model::RUBRIC_VERSION,
        "attempt": asked.state["attempt"],
        "failures": asked.state["failures"],
        "retryOf": asked.state["retryOf"],
        "options": asked.offered().iter().map(|(offered, _)| offered).collect::<Vec<_>>(),
        "applied": false,
    })
}

/// What the seat's question got back — `answers`, or the word nothing came
/// back with — written onto its `row`: the pair, and whether it runs.
pub(super) fn answered(
    wire: &Wire,
    asked: &ModelAsk,
    row: &mut Value,
    answers: Result<&Value, &str>,
) {
    let read = answers
        .map_err(str::to_string)
        .and_then(|answers| asked.read(answers).map_err(|err| err.token().to_string()));
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
}

/// The seat's receipt on a summons's own path, asked alone — how its own
/// tests ask it.
#[cfg(test)]
fn choose_with(wire: &Wire, asked: &ModelAsk, origin: [&str; 3]) -> Option<Value> {
    super::summon_assign::choose_with(
        wire,
        &zerocode_core::summon_assign::AssignAsk {
            model: Some(asked.clone()),
            ..Default::default()
        },
        origin,
    )
    .model
}

/// The row the window writes for a summons: `row` — the path's receipt, or
/// the answer asked after the pane opened — beside what was launched. A
/// challenger's turn ran another model than any answer named: its row is
/// marked, and says nothing was carried out.
pub(super) fn write(
    mut row: Value,
    shadow: &model::Shadow,
    launched: &super::summon_assign::Launched,
    mode: zerocode_core::jev::JevMode,
    ledger: &Path,
    now_ms: i64,
) {
    if shadow.challenge {
        row["applied"] = json!(false);
        row[model::CHALLENGE_KEY] = json!(true);
    }
    launched.stamp(&mut row, mode, now_ms);
    crate::systemone::record_rows(&SUMMON_MODEL, ledger, &[row], now_ms);
}

/// The seats' rows beside what the summons launched, off the beat — how
/// this seat's own tests record one.
#[cfg(test)]
fn record(
    host: &dyn crate::agent_teams::Host,
    prepared: &zerocode_core::orchestration::PreparedWorkerStart,
    checkout: Option<&str>,
    now_ms: i64,
) {
    super::summon_assign::record(host, prepared, checkout, now_ms);
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
