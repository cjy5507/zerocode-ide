//! t-15554: the assign moment — a summons that leaves the model open asks
//! the difficulty and the model-and-effort pair in ONE request, the model
//! question built without the difficulty's answer, and decides in code what
//! runs once that one answer is back.

use super::*;
use crate::summon_assign::{AssignAsk, Receipts};
use crate::summon_difficulty::lineup::{self, Lineup, Seen, row_at};
use crate::summon_difficulty::{FALLBACK_DIFFICULTY, LADDER, PROFILES_SETTING};
use crate::summon_model::{EFFORT_KEY, ModelRecord};
use std::collections::BTreeMap;

/// Three models that name no efforts, as every live model on the machine
/// this was written on does (`zo models --json`, 2026-09-29).
fn lineup() -> Lineup {
    Lineup::from_catalog(
        &serde_json::json!({"models": [
            {"provider": "claude", "id": "model-a", "builtin": true,
             "band": "second", "rungs": ["hard"]},
            {"provider": "claude", "id": "model-b", "builtin": false,
             "band": "rest", "rungs": ["easy", "medium"]},
            {"provider": "claude", "id": "model-c", "builtin": false,
             "band": "rest", "rungs": []},
        ]}),
        "claude",
    )
    .unwrap()
}

/// Every model with a record, so no summons is a challenger's turn.
fn recorded() -> BTreeMap<String, ModelRecord> {
    ["model-a", "model-b", "model-c"]
        .into_iter()
        .map(|model| {
            (
                model.to_string(),
                ModelRecord {
                    ended: lineup::CHALLENGE_MIN_SAMPLES,
                    ..ModelRecord::default()
                },
            )
        })
        .collect()
}

/// A window whose seats answer every question they are asked in one call:
/// the difficulty `low`, the model the pair `picked` names.
struct Assigned {
    settings: serde_json::Value,
    records: BTreeMap<String, ModelRecord>,
    difficulty: serde_json::Value,
    model: serde_json::Value,
    asked: std::cell::RefCell<Vec<AssignAsk>>,
}

impl Assigned {
    fn answering(records: BTreeMap<String, ModelRecord>, picked: (&str, &str)) -> Self {
        Self {
            settings: serde_json::Value::Null,
            records,
            difficulty: serde_json::json!({
                "outcome": "answered", "chosen": LADDER[0].0, "applied": true,
            }),
            model: serde_json::json!({
                "outcome": "answered", "chosen": picked.0, EFFORT_KEY: picked.1,
                "confidence": 0.9, "applied": true,
            }),
            asked: std::cell::RefCell::default(),
        }
    }

    /// Nothing came back inside the wall, for either seat.
    fn late() -> Self {
        let late = serde_json::json!({"outcome": "timeout", "applied": false});
        Self {
            difficulty: late.clone(),
            model: late,
            ..Self::answering(recorded(), ("model-b", "high"))
        }
    }
}

impl Launcher for Assigned {
    fn command_for(&self, agent: &str, prompt: &str, tuning: &[String]) -> Result<String, String> {
        Catalog(&["claude"]).command_for(agent, prompt, tuning)
    }
    fn difficulty_profile(
        &self,
        agent: &str,
        difficulty: &str,
        _: [&str; 3],
    ) -> Result<Option<lineup::Row>, String> {
        row_at(
            &self.settings,
            agent,
            difficulty,
            Some(&lineup()),
            Some(&Seen::default()),
            0,
        )
    }
    fn model_facts(&self, _: &str, _: [&str; 3]) -> Option<crate::summon_model::Facts> {
        Some(crate::summon_model::Facts {
            lineup: lineup(),
            seen: Seen::default(),
            records: self.records.clone(),
        })
    }
    fn choose_assign(&self, asked: &AssignAsk, _: [&str; 3]) -> Receipts {
        self.asked.borrow_mut().push(asked.clone());
        Receipts {
            difficulty: asked.difficulty.as_ref().map(|_| self.difficulty.clone()),
            model: asked.model.as_ref().map(|_| self.model.clone()),
        }
    }
}

/// One claude summons with every dial open under `launcher`: its reply and
/// what it prepared.
fn summon(launcher: &Assigned) -> (serde_json::Value, PreparedWorkerStart) {
    let mut bench = Bench::new();
    bench.json("run-create --name assign");
    let planned = planned_on(
        &mut bench.ledger,
        &mut bench.team,
        launcher,
        "worker-start --agent claude --prompt translate-labels",
        bench.clock + 1,
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    (
        serde_json::from_str(&planned.reply.stdout).unwrap(),
        planned
            .prepared_worker_start
            .expect("a worker was reserved"),
    )
}

/// The efforts claude's ladder launches at, lowest first, once each.
fn claude_ladder() -> Vec<&'static str> {
    let mut efforts: Vec<&str> = LADDER
        .iter()
        .filter_map(|(difficulty, _, _)| difficulty_effort("claude", difficulty))
        .collect();
    efforts.dedup();
    efforts
}

/// A1, A3, A4: one call carries both questions; the model question offers a
/// model that names no efforts at every rung of the agent's ladder, whatever
/// the difficulty will be; the applied pair runs.
#[test]
fn one_request_carries_the_difficulty_and_the_model_pair() {
    let launcher = Assigned::answering(recorded(), ("model-b", "max"));
    let (reply, prepared) = summon(&launcher);
    let asked = launcher.asked.borrow();
    assert_eq!(
        asked.len(),
        1,
        "one request where there were two: {asked:?}"
    );
    assert!(asked[0].shared(), "both questions ride it: {:?}", asked[0]);
    let model = asked[0].model.as_ref().unwrap();
    for id in ["model-a", "model-b", "model-c"] {
        let efforts: Vec<&str> = model
            .offered()
            .iter()
            .filter(|(offered, _)| offered == id)
            .map(|(_, effort)| effort.as_str())
            .collect();
        assert_eq!(
            efforts,
            claude_ladder(),
            "{id} names no efforts: one option a rung of the ladder"
        );
    }
    assert_eq!(reply["model"], "model-b", "{reply}");
    assert_eq!(reply["effort"], "max", "{reply}");
    assert_eq!(reply["dials"]["difficultyFrom"], "jev");
    assert_eq!(reply["dials"]["modelFrom"], "jev");
    let difficulty = prepared.difficulty_shadow.unwrap();
    assert_eq!(difficulty.receipt.unwrap()["applied"], true);
    let model = prepared.model_shadow.unwrap();
    assert_eq!(model.receipt.unwrap()["applied"], true);
    assert!(!model.challenge);
}

/// A4: a challenger's turn runs its model and asks nothing about the model on
/// the launch's path — the difficulty question rides alone, and the model
/// question is kept for the record.
#[test]
fn a_challengers_turn_sends_the_difficulty_alone() {
    let launcher = Assigned::answering(BTreeMap::new(), ("model-b", "max"));
    let (reply, prepared) = summon(&launcher);
    let asked = launcher.asked.borrow();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].difficulty.is_some() && asked[0].model.is_none());
    assert_eq!(reply["dials"]["modelFrom"], "challenge", "{reply}");
    assert_eq!(reply["model"], "model-c", "the low row's untried model");
    let model = prepared.model_shadow.expect("the question is kept");
    assert!(model.challenge);
    assert_eq!(model.receipt, None, "nothing was asked on the path for it");
}

/// A4: a late or broken answer leaves both defaults — the ladder's middle row
/// — after one wall, where two were waited on.
#[test]
fn a_late_answer_leaves_both_defaults_after_one_wall() {
    let launcher = Assigned::late();
    let (reply, prepared) = summon(&launcher);
    assert_eq!(launcher.asked.borrow().len(), 1, "one wall, never two");
    let mid = row_at(
        &serde_json::Value::Null,
        "claude",
        FALLBACK_DIFFICULTY,
        Some(&lineup()),
        Some(&Seen::default()),
        0,
    )
    .unwrap()
    .unwrap();
    assert_eq!(reply["model"], serde_json::json!(mid.model), "{reply}");
    assert_eq!(reply["effort"], serde_json::json!(mid.effort), "{reply}");
    assert_eq!(reply["dials"]["difficultyFrom"], "fallback");
    assert_eq!(reply["dials"]["modelFrom"], "lineup");
    let model = prepared.model_shadow.unwrap();
    assert_eq!(model.receipt.unwrap()["applied"], false);
}

/// A2, A4: the model question rides before the difficulty is known; when the
/// difficulty lands on a row the person wrote, the person's model runs and
/// the model seat's row says its answer was not carried out.
#[test]
fn a_persons_row_keeps_its_model_and_the_pair_is_only_recorded() {
    let mut launcher = Assigned::answering(recorded(), ("model-b", "max"));
    launcher.settings = serde_json::json!({crate::jev::SMART_SETTINGS_KEY: {PROFILES_SETTING: {
        "claude": {LADDER[0].0: {"model": "model-a", "effort": "medium"}},
    }}});
    let (reply, prepared) = summon(&launcher);
    assert_eq!(launcher.asked.borrow().len(), 1);
    assert_eq!(
        (reply["model"].as_str(), reply["effort"].as_str()),
        (Some("model-a"), Some("medium")),
        "{reply}"
    );
    assert_eq!(reply["dials"]["modelFrom"], "person", "{reply}");
    let applied = prepared
        .model_shadow
        .and_then(|shadow| shadow.receipt)
        .map(|receipt| receipt["applied"].clone());
    assert_eq!(
        applied,
        Some(serde_json::json!(false)),
        "the answer is written down, never said to have run"
    );
}
