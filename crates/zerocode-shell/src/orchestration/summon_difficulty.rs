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
fn origins() -> &'static std::sync::Mutex<std::collections::HashMap<OriginKey, std::path::PathBuf>>
{
    static ORIGINS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<OriginKey, std::path::PathBuf>>,
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
pub(super) fn origin(key: [&str; 3], checkout: Option<std::path::PathBuf>) -> Origin {
    let key = key.map(str::to_string);
    let mut held = origins()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    held.remove(&key);
    if let Some(checkout) = checkout {
        held.insert(key.clone(), checkout);
    }
    Origin(key)
}

pub(super) fn choose(look: &Look, origin: [&str; 3]) -> Option<Value> {
    choose_with(&Wire::of_this_machine(), look, origin)
}

fn choose_with(wire: &Wire, look: &Look, origin: [&str; 3]) -> Option<Value> {
    if !crate::systemone::applies(wire, &SUMMON_DIFFICULTY) {
        return None;
    }
    let checkout = origins()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&origin.map(str::to_string))
        .cloned();
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
        row["effort"] = json!(executed_effort);
        row["pinnedEffort"] = json!(shadow.teacher_effort);
        if row["outcome"] == "answered" {
            difficulty::compare(&mut row, shadow.teacher_effort.as_deref());
        }
        crate::systemone::record_rows(&SUMMON_DIFFICULTY, &ledger, &[row], now_ms);
    }));
}

#[cfg(test)]
mod tests;
