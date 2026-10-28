//! t-15554: the assign moment — a summons that leaves the model open asks
//! the difficulty and the model-and-effort pair in ONE request, the model
//! question built without the difficulty's answer, and decides in code what
//! runs once that one answer is back.

use super::*;
use crate::summon_assign::{AssignAsk, Receipts};
use crate::summon_difficulty::lineup::{self, Lineup, Seen, row_at};
use crate::summon_difficulty::{LADDER, PROFILES_SETTING, UNJUDGED_DIFFICULTY};
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

/// [`lineup`] with each model naming the efforts it accepts, as every model
/// of `zo models --json` did on 2026-10-28: model-a all of claude's ladder,
/// model-b nothing above `medium`, model-c nothing above `high`.
fn lineup_with_efforts() -> Lineup {
    let mut held = lineup();
    for (model, efforts) in [
        ("model-a", ladder_efforts("claude")),
        ("model-b", vec!["low", "medium"]),
        ("model-c", vec!["low", "medium", "high"]),
    ] {
        held.models
            .iter_mut()
            .find(|live| live.id == model)
            .expect("a model of the lineup")
            .efforts = Some(efforts.into_iter().map(str::to_string).collect());
    }
    held
}

/// Every model with a record, so no summons is a challenger's turn.
fn recorded() -> BTreeMap<String, ModelRecord> {
    ["model-a", "model-b", "model-c"]
        .into_iter()
        .map(|model| {
            (
                model.to_string(),
                ModelRecord {
                    overall: crate::summon_model::ModelPerformance {
                        ended: lineup::CHALLENGE_MIN_SAMPLES,
                        ..crate::summon_model::ModelPerformance::default()
                    },
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
    lineup: Lineup,
    records: BTreeMap<String, ModelRecord>,
    difficulty: serde_json::Value,
    model: serde_json::Value,
    asked: std::cell::RefCell<Vec<AssignAsk>>,
}

impl Assigned {
    fn answering(records: BTreeMap<String, ModelRecord>, picked: (&str, &str)) -> Self {
        Self {
            settings: serde_json::Value::Null,
            lineup: lineup(),
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
            Some(&self.lineup),
            Some(&Seen::default()),
            0,
        )
    }
    fn model_facts(&self, _: &str, _: [&str; 3]) -> Option<crate::summon_model::Facts> {
        Some(crate::summon_model::Facts {
            lineup: self.lineup.clone(),
            seen: Seen::default(),
            records: self.records.clone(),
        })
    }
    fn choose_assign(&self, asked: &AssignAsk, _: [&str; 3]) -> Receipts {
        self.asked.borrow_mut().push(asked.clone());
        Receipts {
            difficulty: asked.difficulty.as_ref().map(|_| self.difficulty.clone()),
            model: asked.model.as_ref().map(|_| self.model.clone()),
            ..Default::default()
        }
    }
}

/// One claude summons with every dial open under `launcher`: its reply and
/// what it prepared.
fn summon(launcher: &Assigned) -> (serde_json::Value, PreparedWorkerStart) {
    summon_with(
        launcher,
        "worker-start --agent claude --prompt translate-labels",
    )
}

/// [`summon`] of the `worker-start` line `line`.
fn summon_with(launcher: &Assigned, line: &str) -> (serde_json::Value, PreparedWorkerStart) {
    let mut bench = Bench::new();
    bench.json("run-create --name assign");
    let planned = planned_on(
        &mut bench.ledger,
        &mut bench.team,
        launcher,
        line,
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

/// The efforts claude's ladder offers, lowest first, once each.
fn claude_ladder() -> Vec<&'static str> {
    ladder_efforts("claude")
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
    assert!(
        reply["dials"]["modelUnjudged"].is_null(),
        "the seat's pair ran, and is said to have not: {reply}"
    );
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
    assert!(
        reply["dials"]["modelUnjudged"].is_null(),
        "a challenger's turn was said to be an unasked seat: {reply}"
    );
    assert_eq!(reply["model"], "model-c", "the low row's untried model");
    let model = prepared.model_shadow.expect("the question is kept");
    assert!(model.challenge);
    assert_eq!(model.receipt, None, "nothing was asked on the path for it");
}

/// A4: a late or broken answer leaves both defaults — the row for unjudged
/// work (t-45961) — after one wall, where two were waited on.
#[test]
fn a_late_answer_leaves_both_defaults_after_one_wall() {
    let launcher = Assigned::late();
    let (reply, prepared) = summon(&launcher);
    assert_eq!(launcher.asked.borrow().len(), 1, "one wall, never two");
    let unjudged = row_at(
        &serde_json::Value::Null,
        "claude",
        UNJUDGED_DIFFICULTY,
        Some(&lineup()),
        Some(&Seen::default()),
        0,
    )
    .unwrap()
    .unwrap();
    assert_eq!(reply["model"], serde_json::json!(unjudged.model), "{reply}");
    assert_eq!(
        reply["effort"],
        serde_json::json!(unjudged.effort),
        "{reply}"
    );
    assert_eq!(reply["dials"]["difficulty"], UNJUDGED_DIFFICULTY);
    assert_eq!(reply["dials"]["difficultyFrom"], "facts");
    assert_eq!(reply["dials"]["modelFrom"], "lineup");
    // t-74181: why the seat's pair did not run, and what ran instead, are
    // written where the evaluation reads them.
    let unjudged = &reply["dials"]["modelUnjudged"];
    assert!(
        unjudged["why"]
            .as_str()
            .is_some_and(|why| why != "not_asked")
            && unjudged["says"]
                .as_str()
                .is_some_and(|says| says.contains("model judgment")),
        "{reply}"
    );
    let model = prepared.model_shadow.unwrap();
    let receipt = model.receipt.unwrap();
    assert_eq!(receipt["applied"], false);
    assert_eq!(receipt["ranFrom"], "lineup", "{receipt}");
}

/// t-45961: a summons nobody judged runs the provider's top implementation
/// model — the classifier's hard rung, `model-a` here — and never the
/// ladder's middle row, which a misjudged lineup put on its weakest model
/// (10-10~11: six summonses out of six on the cheapest model). The reply
/// says the facts chose, not that nothing did.
#[test]
fn an_unjudged_summons_runs_the_top_implementer_and_says_who_chose() {
    let launcher = Assigned::late();
    let (reply, _) = summon(&launcher);
    assert_eq!(reply["model"], "model-a", "{reply}");
    assert_ne!(reply["dials"]["difficultyFrom"], "fallback", "{reply}");
    assert_eq!(reply["dials"]["difficultyFrom"], "facts", "{reply}");
    assert_eq!(reply["dials"]["modelFrom"], "lineup", "{reply}");
    // And why no judgment chose, in a word and in words (the coordinator,
    // 10-11: this machine's Jev requests come back refused).
    assert_eq!(reply["dials"]["unjudged"]["why"], "timeout", "{reply}");
    assert!(
        reply["dials"]["unjudged"]["says"]
            .as_str()
            .is_some_and(|says| says.contains("time limit")),
        "{reply}"
    );
}

/// t-45961: agent-list says whether summonses are being judged and why not
/// — here the judgment service refused the last request with HTTP 402 —
/// and what a summons nobody judged runs; a host that cannot say says
/// nothing.
#[test]
fn agent_list_says_why_summonses_go_unjudged() {
    struct Refusing;
    impl Launcher for Refusing {
        fn command_for(
            &self,
            agent: &str,
            prompt: &str,
            tuning: &[String],
        ) -> Result<String, String> {
            Catalog(&["claude"]).command_for(agent, prompt, tuning)
        }
        fn summon_judgment(&self) -> Option<SummonJudgment> {
            Some(SummonJudgment {
                mode: crate::jev::JevMode::Auto.key().to_string(),
                last_outcome: Some("http_402".to_string()),
                last_at_ms: Some(1),
            })
        }
    }
    let said = answered(&Refusing, "agent-list");
    let seat = &said["summonJudgment"];
    assert_eq!(seat["unjudged"]["why"], "http_402", "{said}");
    assert!(
        seat["unjudged"]["says"]
            .as_str()
            .is_some_and(|says| says.contains("HTTP 402")),
        "{seat}"
    );
    assert_eq!(seat["unjudgedRuns"], UNJUDGED_DIFFICULTY);
    assert!(
        answered(&Catalog(&["claude"]), "agent-list")["summonJudgment"].is_null(),
        "a host that cannot say says nothing"
    );
}

/// t-45961: the dials say the difficulty as a word every time (it was null
/// for a summons that names its model and leaves the effort open, with
/// nothing judged): such a summons stands at the rung an unjudged summons
/// stands at, and says that no difficulty ran — its CLI's own effort stands.
#[test]
fn a_named_model_with_its_effort_open_still_says_the_difficulty() {
    let launcher = Assigned::late();
    let mut bench = Bench::new();
    bench.json("run-create --name assign");
    let planned = planned_on(
        &mut bench.ledger,
        &mut bench.team,
        &launcher,
        "worker-start --agent claude --prompt translate-labels --model model-c",
        bench.clock + 1,
    );
    assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
    let reply: serde_json::Value = serde_json::from_str(&planned.reply.stdout).unwrap();
    assert_eq!(reply["model"], "model-c", "{reply}");
    assert_eq!(reply["dials"]["difficulty"], UNJUDGED_DIFFICULTY, "{reply}");
    assert_eq!(reply["dials"]["difficultyFrom"], "none", "{reply}");
    assert_eq!(reply["dials"]["modelFrom"], "request", "{reply}");
    assert!(reply["effort"].is_null(), "no difficulty ran: {reply}");
}

/// t-45961: the person's own row for unjudged work comes before the facts.
#[test]
fn an_unjudged_summons_runs_the_persons_row_before_the_facts() {
    let mut launcher = Assigned::late();
    launcher.settings = serde_json::json!({crate::jev::SMART_SETTINGS_KEY: {PROFILES_SETTING: {
        "claude": {LADDER[2].0: {"model": "model-b", "effort": "high"}},
    }}});
    let (reply, _) = summon(&launcher);
    assert_eq!(
        (reply["model"].as_str(), reply["effort"].as_str()),
        (Some("model-b"), Some("high")),
        "{reply}"
    );
    assert_eq!(reply["dials"]["difficultyFrom"], "person", "{reply}");
    assert_eq!(reply["dials"]["modelFrom"], "person", "{reply}");
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

/// A window like [`Assigned`] for any agent of the catalog: every agent's
/// lineup is the same synthetic one, and every agent's command is its id.
struct AnyAgent(Assigned);

impl Launcher for AnyAgent {
    fn command_for(&self, agent: &str, _: &str, tuning: &[String]) -> Result<String, String> {
        Ok(std::iter::once(agent.to_string())
            .chain(tuning.iter().cloned())
            .collect::<Vec<_>>()
            .join(" "))
    }
    fn difficulty_profile(
        &self,
        agent: &str,
        difficulty: &str,
        origin: [&str; 3],
    ) -> Result<Option<lineup::Row>, String> {
        self.0.difficulty_profile(agent, difficulty, origin)
    }
    fn model_facts(&self, agent: &str, origin: [&str; 3]) -> Option<crate::summon_model::Facts> {
        self.0.model_facts(agent, origin)
    }
    fn choose_assign(&self, asked: &AssignAsk, origin: [&str; 3]) -> Receipts {
        self.0.choose_assign(asked, origin)
    }
}

/// A8 (the person, 2026-09-29: Jev is used the same whatever CLI runs): the
/// summons is the one layer every agent of the catalog passes. An agent
/// whose command line takes a model and an effort is asked the difficulty
/// and the pair in one request and runs the pair; one that takes a model
/// but no effort has no pair to offer and is asked nothing, its CLI's own
/// default running and the reply saying so; one that takes neither is asked
/// nothing either, and refuses a model it was handed.
#[test]
fn every_agent_of_the_catalog_passes_the_same_moment() {
    let (mut pairs, mut models_only, mut neither) = (0, 0, 0);
    for spec in crate::agent::AGENT_SPECS {
        let agent = spec.id;
        let launcher = AnyAgent(Assigned::answering(recorded(), ("model-b", "high")));
        let mut bench = Bench::new();
        bench.json("run-create --name catalog");
        let planned = planned_on(
            &mut bench.ledger,
            &mut bench.team,
            &launcher,
            &format!("worker-start --agent {agent} --prompt translate-labels"),
            bench.clock + 1,
        );
        assert_eq!(
            planned.reply.exit_code, 0,
            "{agent}: {}",
            planned.reply.stderr
        );
        let reply: serde_json::Value = serde_json::from_str(&planned.reply.stdout).unwrap();
        let asked = launcher.0.asked.borrow();
        let takes_a_model = launch_tuning(agent, Some("model-b"), None).is_ok();
        let takes_an_effort = !ladder_efforts(agent).is_empty();
        match (takes_a_model, takes_an_effort) {
            (true, true) => {
                pairs += 1;
                assert_eq!(asked.len(), 1, "{agent}: one request");
                assert!(asked[0].shared(), "{agent}: both questions ride it");
                assert_eq!(
                    (reply["model"].as_str(), reply["effort"].as_str()),
                    (Some("model-b"), Some("high")),
                    "{agent}: {reply}"
                );
                assert_eq!(reply["dials"]["modelFrom"], "jev", "{agent}: {reply}");
            }
            (true, false) => {
                models_only += 1;
                assert!(asked.is_empty(), "{agent}: no pair to offer: {asked:?}");
                assert_eq!(
                    reply["dials"]["modelFrom"], DIALS_FROM_CLI_DEFAULT,
                    "{agent}: the reply says its CLI's own default runs: {reply}"
                );
            }
            (false, _) => {
                neither += 1;
                assert!(asked.is_empty(), "{agent}: {asked:?}");
                assert!(
                    launch_tuning(agent, Some("model-b"), None)
                        .is_err_and(|refused| refused.contains(agent)),
                    "{agent}: a model it cannot take is refused, and says whose"
                );
            }
        }
    }
    assert!(
        pairs > 0 && models_only > 0 && neither > 0,
        "the catalog holds every kind: {pairs} {models_only} {neither}"
    );
}

/// t-74181: a summons Jev did not judge — switched off, the service
/// failing, or no answer in time — puts no effort of ours on the command
/// line: the agent's CLI runs at its own default. Before, the unjudged row
/// read `max` from a shipped table and a coded ladder. The reply says that
/// nobody set the effort, why no seat judged, and that the pair has no
/// measured record here.
#[test]
fn a_summons_jev_did_not_judge_runs_at_its_clis_default_effort() {
    for outcome in [
        crate::jev::door::Refused::Off.token(),
        "http_500",
        "timeout",
    ] {
        let unjudged = serde_json::json!({"outcome": outcome, "applied": false});
        let launcher = Assigned {
            difficulty: unjudged.clone(),
            model: unjudged,
            ..Assigned::answering(recorded(), ("model-b", "high"))
        };
        let (reply, _) = summon(&launcher);
        assert_eq!(reply["model"], "model-a", "{outcome}: {reply}");
        assert!(reply["effort"].is_null(), "{outcome}: {reply}");
        let dials = &reply["dials"];
        assert_eq!(dials["difficultyFrom"], "facts", "{outcome}: {reply}");
        assert_eq!(dials["unjudged"]["why"], outcome, "{reply}");
        assert_eq!(dials["modelUnjudged"]["why"], outcome, "{reply}");
        assert_eq!(
            dials["why"]["effortRule"], "cli-default",
            "{outcome}: {reply}"
        );
        assert_eq!(dials["why"]["pairSamples"], 0, "{outcome}: {reply}");
        assert_eq!(dials["why"]["evidence"], "unverified", "{outcome}: {reply}");
    }
}

/// t-74181: the person's effort binds — with Jev silent it is what runs, and
/// beside Jev's applied pair it still is: the seat may choose the model the
/// person left open, never move the effort the person wrote. Written on
/// every row, it is the one effort the question offers (review P2-2), so the
/// seat's answer names it.
#[test]
fn the_persons_effort_binds_with_jev_silent_and_beside_its_pair() {
    let written = serde_json::json!({
        crate::jev::SMART_SETTINGS_KEY: {PROFILES_SETTING: {"claude": LADDER
            .iter()
            .map(|(difficulty, _)| ((*difficulty).to_string(), serde_json::json!({"effort": "medium"})))
            .collect::<serde_json::Map<_, _>>()}},
    });
    let silent = Assigned {
        settings: written.clone(),
        ..Assigned::late()
    };
    let (reply, _) = summon(&silent);
    assert_eq!(reply["effort"], "medium", "{reply}");
    assert_eq!(reply["dials"]["why"]["effortRule"], "person", "{reply}");
    let beside = Assigned {
        settings: written,
        ..Assigned::answering(recorded(), ("model-a", "medium"))
    };
    let (reply, _) = summon(&beside);
    assert_eq!(reply["model"], "model-a", "the seat's model ran: {reply}");
    assert_eq!(
        reply["effort"], "medium",
        "the person's effort stood: {reply}"
    );
    assert_eq!(reply["dials"]["why"]["effortRule"], "person", "{reply}");
}

/// t-74181: the pair is Jev's — when its answer runs, the effort is the one
/// it chose, the top included, and the reply says Jev set it; the pair's own
/// finished record decides whether that choice is measured or unverified.
#[test]
fn the_effort_jev_chose_runs_and_its_record_says_whether_it_is_measured() {
    let launcher = Assigned::answering(recorded(), ("model-a", "max"));
    let (reply, _) = summon(&launcher);
    assert_eq!(reply["model"], "model-a", "{reply}");
    assert_eq!(
        reply["effort"], "max",
        "the top, when Jev judges it: {reply}"
    );
    assert_eq!(reply["dials"]["why"]["effortRule"], "jev", "{reply}");
    assert_eq!(reply["dials"]["why"]["evidence"], "unverified", "{reply}");
    let mut records = recorded();
    records.get_mut("model-a").unwrap().by_effort.insert(
        "max".to_string(),
        crate::summon_model::ModelPerformance {
            ended: lineup::CHALLENGE_MIN_SAMPLES,
            ..crate::summon_model::ModelPerformance::default()
        },
    );
    let (reply, _) = summon(&Assigned::answering(records, ("model-a", "max")));
    assert_eq!(
        reply["dials"]["why"]["pairSamples"],
        lineup::CHALLENGE_MIN_SAMPLES,
        "{reply}"
    );
    assert_eq!(reply["dials"]["why"]["evidence"], "measured", "{reply}");
}

/// The person's effort written on `difficulties` of claude's rows, and no
/// model: a half row the settings screen allows.
fn effort_written_at(effort: &str, difficulties: &[&str]) -> serde_json::Value {
    serde_json::json!({
        crate::jev::SMART_SETTINGS_KEY: {PROFILES_SETTING: {"claude": difficulties
            .iter()
            .map(|difficulty| ((*difficulty).to_string(), serde_json::json!({"effort": effort})))
            .collect::<serde_json::Map<_, _>>()}},
    })
}

/// The difficulty seat's applied answer `difficulty`.
fn difficulty_answered(difficulty: &str) -> serde_json::Value {
    serde_json::json!({"outcome": "answered", "chosen": difficulty, "applied": true})
}

/// t-74181 (review P2-2): where the person wrote an effort alone on every
/// row the summons can land on, that effort binds whatever model runs, so
/// the pair question becomes a model question — each model offered once, at
/// the effort it would run the person's at — and the answer that runs is
/// the answer given. When the seat's model is not the row's, the person's
/// effort is brought inside what that model accepts, and the reply says so.
#[test]
fn the_persons_effort_alone_makes_the_pair_question_a_model_question() {
    let everywhere: Vec<&str> = LADDER.iter().map(|(difficulty, _)| *difficulty).collect();
    let launcher = Assigned {
        settings: effort_written_at("high", &everywhere),
        lineup: lineup_with_efforts(),
        difficulty: difficulty_answered(LADDER[2].0),
        ..Assigned::answering(recorded(), ("model-b", "medium"))
    };
    let (reply, prepared) = summon(&launcher);
    let asked = launcher.asked.borrow();
    let offered = asked[0]
        .model
        .as_ref()
        .expect("the pair question")
        .offered();
    assert_eq!(
        offered,
        [
            ("model-a".to_string(), "high".to_string()),
            ("model-b".to_string(), "medium".to_string()),
            ("model-c".to_string(), "high".to_string()),
        ],
        "one option a model, at the effort the person's runs at on it"
    );
    assert_eq!(reply["model"], "model-b", "the seat's model ran: {reply}");
    assert_eq!(
        reply["effort"], "medium",
        "the person's high, inside what model-b accepts: {reply}"
    );
    let dials = &reply["dials"];
    assert_eq!(dials["modelFrom"], "jev", "{reply}");
    assert_eq!(dials["why"]["effortRule"], "person", "{reply}");
    assert_eq!(
        dials["effortClamped"],
        serde_json::json!({"from": "high", "to": "medium"}),
        "{reply}"
    );
    let receipt = prepared
        .model_shadow
        .and_then(|shadow| shadow.receipt)
        .expect("the pair answer's row");
    assert_eq!(
        receipt["applied"], true,
        "the pair that ran is the answer: {receipt}"
    );
}

/// t-74181 (review P2-2): where the person's effort binds only some rows,
/// the pair question stays a pair question; an answer whose effort is not
/// the one that binds at the difficulty that ran did not run — the row ran,
/// and the answer is written down as recorded, never graded as applied. An
/// answer at the binding effort runs.
#[test]
fn a_pair_whose_effort_the_person_bound_otherwise_did_not_run() {
    let launcher = |picked| Assigned {
        settings: effort_written_at("high", &[LADDER[2].0]),
        lineup: lineup_with_efforts(),
        difficulty: difficulty_answered(LADDER[2].0),
        ..Assigned::answering(recorded(), picked)
    };
    let other = launcher(("model-c", "low"));
    let (reply, prepared) = summon(&other);
    assert!(
        other.asked.borrow()[0]
            .model
            .as_ref()
            .expect("the pair question")
            .offered()
            .contains(&("model-c".to_string(), "low".to_string())),
        "a row the person left open keeps the pairs"
    );
    assert_eq!(reply["model"], "model-a", "the hard row ran: {reply}");
    assert_eq!(reply["effort"], "high", "{reply}");
    assert_eq!(reply["dials"]["modelFrom"], "lineup", "{reply}");
    assert_eq!(reply["dials"]["why"]["effortRule"], "person", "{reply}");
    let receipt = prepared
        .model_shadow
        .and_then(|shadow| shadow.receipt)
        .expect("the pair answer's row");
    assert_eq!(receipt["applied"], false, "{receipt}");
    assert_eq!(receipt["ranFrom"], "lineup", "{receipt}");

    let (reply, prepared) = summon(&launcher(("model-c", "high")));
    assert_eq!(reply["model"], "model-c", "{reply}");
    assert_eq!(reply["effort"], "high", "{reply}");
    assert_eq!(reply["dials"]["modelFrom"], "jev", "{reply}");
    let receipt = prepared
        .model_shadow
        .and_then(|shadow| shadow.receipt)
        .expect("the pair answer's row");
    assert_eq!(receipt["applied"], true, "{receipt}");
}

/// t-74181 (review P3-6): `baselineHigh` marks a summons the hard row ran as
/// it stands — not any summons of that row's model. With no effort on the
/// row, a request that named the model alone runs the same words, but no
/// row ran it.
#[test]
fn baseline_high_is_the_hard_row_as_it_ran_not_any_summons_of_its_model() {
    let (_, prepared) = summon(&Assigned::late());
    assert!(
        prepared
            .difficulty_shadow
            .expect("a difficulty row")
            .baseline_high,
        "unjudged, the hard row ran"
    );
    let hard = row_at(
        &serde_json::Value::Null,
        "claude",
        LADDER[2].0,
        Some(&lineup()),
        None,
        0,
    )
    .unwrap()
    .expect("a hard row");
    let (reply, prepared) = summon_with(
        &Assigned::late(),
        &format!(
            "worker-start --agent claude --model {} --prompt translate-labels",
            hard.model
        ),
    );
    assert_eq!(reply["model"], hard.model.as_str(), "{reply}");
    assert!(
        !prepared
            .difficulty_shadow
            .expect("a difficulty row")
            .baseline_high,
        "a request that named the hard row's model ran no row: {reply}"
    );
}
