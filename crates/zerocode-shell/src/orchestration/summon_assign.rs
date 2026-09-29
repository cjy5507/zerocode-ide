//! The assign moment's one request (t-15554): the difficulty seat and the
//! model seat asked for one summons in ONE POST under one wall
//! ([`APPLY_DEADLINE_MS`]), each seat's row kept in its own ledger with its
//! own words, rubric and outcome, both naming the request they shared. The
//! questions, and which answer runs, are the core's
//! (`zerocode_core::summon_assign`, the worker-start plan); this is the wire.
use crate::agent_teams::Host;
use crate::systemone::{Spent, Wire, request_body};
use serde_json::{Value, json};
use std::path::Path;
use std::time::{Duration, Instant};
use zerocode_core::jev::{JevMode, SUMMON_DIFFICULTY, SUMMON_MODEL};
use zerocode_core::orchestration::PreparedWorkerStart;
use zerocode_core::summon_assign::{AssignAsk, Receipts, SHARED_REQUEST_KEY};
use zerocode_core::summon_difficulty::APPLY_DEADLINE_MS;

/// Every question `asked` carries, in one request: each riding seat's row.
///
/// The door clears the body under one seat's list of what it sends — the
/// two seats' lists are one (the core's test says so) — and counts the day
/// once. The request is written on the first riding seat's row
/// (`requests: 1`); the other row rode it (`requests: 0`), so a count over
/// every ledger adds to what the door counted.
pub(super) fn ask(wire: &Wire, asked: &AssignAsk, checkout: Option<&Path>) -> Receipts {
    let seat = if asked.difficulty.is_some() {
        &SUMMON_DIFFICULTY
    } else {
        &SUMMON_MODEL
    };
    let began = Instant::now();
    let answer = wire.ask(
        seat,
        checkout,
        request_body(&asked.state(), &asked.questions()),
        Duration::from_millis(APPLY_DEADLINE_MS),
    );
    let elapsed_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    let body = answer.answer.and_then(|body| {
        serde_json::from_str::<Value>(&body).map_err(|_| crate::systemone::SCHEMA.to_string())
    });
    let answers = body
        .as_ref()
        .map(|body| &body["answers"])
        .map_err(String::as_str);
    let rode = Spent {
        requests: 0,
        redacted_lines: 0,
        ..answer.spent.clone()
    };
    let mut spent = [answer.spent, rode].into_iter();
    let shared = asked.shared().then(|| uuid::Uuid::new_v4().to_string());
    let mut sent = |mut row: Value| {
        row["elapsedMs"] = json!(elapsed_ms);
        row["requestBytes"] = json!(answer.request_bytes);
        spent.next().unwrap_or_default().stamp(&mut row);
        if let Some(shared) = &shared {
            row[SHARED_REQUEST_KEY] = json!(shared);
        }
        row
    };
    Receipts {
        difficulty: asked.difficulty.as_ref().map(|look| {
            let mut row = sent(super::summon_difficulty::head(look));
            super::summon_difficulty::answered(wire, &mut row, answers);
            row
        }),
        model: asked.model.as_ref().map(|model| {
            let mut row = sent(super::summon_model::head(model));
            super::summon_model::answered(wire, model, &mut row, answers);
            row
        }),
    }
}

/// The receipts of a fresh summons under `origin`, asked on the launch's own
/// path — only while a riding seat applies, so a summons never waits on an
/// answer nothing will act on.
pub(super) fn choose(asked: &AssignAsk, origin: [&str; 3]) -> Receipts {
    choose_with(&Wire::of_this_machine(), asked, origin)
}

pub(super) fn choose_with(wire: &Wire, asked: &AssignAsk, origin: [&str; 3]) -> Receipts {
    let acts = asked.difficulty.is_some() && crate::systemone::applies(wire, &SUMMON_DIFFICULTY)
        || asked.model.is_some() && crate::systemone::applies(wire, &SUMMON_MODEL);
    if !acts {
        return Receipts::default();
    }
    let Some(checkout) = super::summon_difficulty::beat_checkout(origin) else {
        return Receipts::default();
    };
    // A seat that only records rides the acting seat's request — the
    // moment's one request, its state charged once (the coordinator,
    // 2026-09-29 23:07) — and its answer is written down, never run. A seat
    // the person switched off is not asked.
    let settings = wire.settings_root();
    let riding = asked.only(
        SUMMON_DIFFICULTY.mode_in(&settings).asks(),
        SUMMON_MODEL.mode_in(&settings).asks(),
    );
    ask(wire, &riding, checkout.as_deref())
}

/// What a summons launched, as both seats' rows name it.
#[derive(Clone)]
pub(super) struct Launched {
    run: String,
    worker: String,
    dispatch: Option<String>,
    task: Option<String>,
    agent: String,
    execution_model: Option<String>,
    effort: Option<String>,
}

impl Launched {
    fn of(prepared: &PreparedWorkerStart) -> Self {
        let pinned = prepared.summon_shadow.as_ref().map(|shadow| &shadow.pinned);
        Self {
            run: prepared.run.clone(),
            worker: prepared.worker.clone(),
            dispatch: prepared.dispatch.clone(),
            task: prepared.task.clone(),
            agent: prepared.agent.clone(),
            execution_model: pinned.and_then(|pinned| pinned.model.clone()),
            effort: pinned.and_then(|pinned| pinned.effort.clone()),
        }
    }

    /// Write the launch onto a seat's `row`, recorded under `mode` at
    /// `now_ms`.
    pub(super) fn stamp(&self, row: &mut Value, mode: JevMode, now_ms: i64) {
        row["at"] = json!(now_ms);
        row["requestAt"] = json!(now_ms);
        row["run"] = json!(self.run);
        row["worker"] = json!(self.worker);
        // Taskless summonses have a stable worker identity instead of a null
        // request name (which would join unrelated rows).
        row["dispatch"] = json!(self.dispatch.as_ref().unwrap_or(&self.worker));
        row["task"] = json!(self.task);
        row["mode"] = json!(mode.key());
        row["agent"] = json!(self.agent);
        row["executionModel"] = json!(self.execution_model);
        row["effort"] = json!(self.effort);
    }
}

/// Each seat's row beside what the summons launched — off the beat. A seat
/// whose receipt the path carried is not asked again; the seats that asked
/// nothing on the path ask together, in one request.
pub(super) fn record(
    host: &dyn Host,
    prepared: &PreparedWorkerStart,
    checkout: Option<&str>,
    now_ms: i64,
) {
    let Some(wire) = host.jev_wire() else { return };
    let settings = wire.settings_root();
    let asking = |seat: &zerocode_core::jev::JevUse| {
        let mode = seat.mode_in(&settings);
        crate::systemone::ledger_of(&wire, seat)
            .filter(|_| mode.asks())
            .map(|ledger| (mode, ledger))
    };
    let difficulty = prepared
        .difficulty_shadow
        .clone()
        .zip(asking(&SUMMON_DIFFICULTY));
    let model = prepared.model_shadow.clone().zip(asking(&SUMMON_MODEL));
    if difficulty.is_none() && model.is_none() {
        return;
    }
    let launched = Launched::of(prepared);
    let checkout = checkout.map(std::path::PathBuf::from);
    host.off_the_beat(Box::new(move || {
        let asked = AssignAsk {
            difficulty: difficulty
                .as_ref()
                .filter(|(shadow, _)| shadow.receipt.is_none())
                .map(|(shadow, _)| shadow.look.clone()),
            model: model
                .as_ref()
                .filter(|(shadow, _)| shadow.receipt.is_none())
                .map(|(shadow, _)| shadow.ask.clone()),
        };
        let asked = if asked.is_empty() {
            Receipts::default()
        } else {
            ask(&wire, &asked, checkout.as_deref())
        };
        // A background answer was never used for this launch.
        let unapplied = |mut row: Value| {
            row["applied"] = json!(false);
            row
        };
        if let Some((shadow, (mode, ledger))) = difficulty {
            let row = shadow
                .receipt
                .clone()
                .or_else(|| asked.difficulty.clone().map(unapplied))
                .unwrap_or_default();
            super::summon_difficulty::write(row, &shadow, &launched, mode, &ledger, now_ms);
        }
        if let Some((shadow, (mode, ledger))) = model {
            let row = shadow
                .receipt
                .clone()
                .or_else(|| asked.model.clone().map(unapplied))
                .unwrap_or_default();
            super::summon_model::write(row, &shadow, &launched, mode, &ledger, now_ms);
        }
    }));
}

#[cfg(test)]
mod tests;
