use super::*;
use crate::systemone::tests::{ANSWERING_VERSION, Endpoint};
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};
use zerocode_core::jev::{SUMMON, SUMMON_DIFFICULTY, SUMMON_MODEL};
use zerocode_core::summon_assign::SHARED_REQUEST_KEY;
use zerocode_core::summon_difficulty::lineup::{self, Lineup, Seen};
use zerocode_core::summon_difficulty::{self as difficulty, APPLY_DEADLINE_MS, LADDER, Look};
use zerocode_core::summon_model::{self as model, ModelAsk, ModelRecord};

const SPEC: &str = "Translate the settings labels into five languages";

fn look() -> Look {
    Look {
        title: "Translate labels".into(),
        spec: SPEC.into(),
        attempt: 0,
        failures: 0,
        retry_of: false,
    }
}

/// Three models that name no efforts, as every live model on the machine
/// this was written on does.
fn lineup() -> Lineup {
    Lineup::from_catalog(
        &json!({"models": [
            {"provider": "claude", "id": "model-a", "builtin": true, "band": "second", "rungs": ["hard"]},
            {"provider": "claude", "id": "model-b", "builtin": false, "band": "rest", "rungs": ["easy", "medium"]},
            {"provider": "claude", "id": "model-c", "builtin": false, "band": "rest", "rungs": []},
        ]}),
        "claude",
    )
    .unwrap()
}

/// Every model with a record: no summons is a challenger's turn.
fn records() -> std::collections::BTreeMap<String, ModelRecord> {
    ["model-a", "model-b", "model-c"]
        .into_iter()
        .map(|id| {
            (
                id.to_string(),
                ModelRecord {
                    ended: lineup::CHALLENGE_MIN_SAMPLES,
                    ..ModelRecord::default()
                },
            )
        })
        .collect()
}

fn model_ask() -> ModelAsk {
    let options = model::options(
        "claude",
        &lineup(),
        None,
        &records(),
        &[LADDER[0].2],
        |_| None,
        0,
    );
    model::ask(&look(), &options).unwrap()
}

fn both() -> AssignAsk {
    AssignAsk {
        difficulty: Some(look()),
        model: Some(model_ask()),
        ..Default::default()
    }
}

/// Settings with the difficulty seat at `difficulty` and the model seat at
/// `model`, every folder consented.
fn wire(home: &tempfile::TempDir, endpoint: &Endpoint, difficulty: &str, model: &str) -> Wire {
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {
            SUMMON_DIFFICULTY.setting: difficulty,
            SUMMON_MODEL.setting: model,
            "jev": {"workspaces": ["*"]},
        }})
        .to_string(),
    )
    .unwrap();
    Wire::at(&endpoint.base(), "test-key", Some(settings))
}

/// Jev as a loopback endpoint: every question the request carries answered —
/// the difficulty `low`, the model `model-b` at the first effort offered for
/// it — so one body serves a request of one question or of two.
fn answer_every(request: &str) -> String {
    answered(request, None)
}

/// [`answer_every`], choosing `agent` where the agent question is asked.
fn answered(request: &str, agent: Option<&str>) -> String {
    let body: Value = request
        .split_once("\r\n\r\n")
        .and_then(|(_, body)| serde_json::from_str(body).ok())
        .unwrap_or_default();
    let mut answers = Map::new();
    for (question, asked) in body["questions"].as_object().into_iter().flatten() {
        let words: Vec<&String> = asked["criteria"]
            .as_object()
            .map(|criteria| criteria.keys().collect())
            .unwrap_or_default();
        let Some(chosen) = words
            .iter()
            .find(|word| {
                word.as_str() == LADDER[0].0
                    || Some(word.as_str()) == agent
                    || word.starts_with(&format!("model-b{}", model::PAIR_SEP))
            })
            .or(words.first())
        else {
            continue;
        };
        let rest = 0.1 / f64::from(u32::try_from(words.len() - 1).unwrap_or(1).max(1));
        let probabilities: Map<String, Value> = words
            .iter()
            .map(|word| {
                (
                    (*word).clone(),
                    json!(if word == chosen { 0.9 } else { rest }),
                )
            })
            .collect();
        answers.insert(
            question.clone(),
            json!({"type": "choice", "choice": chosen, "probabilities": probabilities, "confidence": 0.9}),
        );
    }
    json!({"model": ANSWERING_VERSION, "answers": answers}).to_string()
}

/// How many requests the door counted today under `home`.
fn counted_today(home: &tempfile::TempDir) -> u64 {
    zerocode_core::jev::count::sent(&zerocode_core::jev::count::requests_path(
        home.path(),
        &crate::systemone::today(),
    ))
}

/// A1, A2: two acting seats, one request — the state's words sent once, each
/// seat's row in its own shape and rubric, both naming the request, and the
/// day counting one.
#[test]
fn two_acting_seats_ride_one_request() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, 0);
    let wire = wire(&home, &endpoint, "on", "on");
    let key = ["assign-team", "%1", "both-on"];
    let _origin = super::super::summon_difficulty::origin_with_for_tests(
        key,
        Some(home.path().to_path_buf()),
        true,
        wire.settings_root(),
    );
    let receipts = choose_with(&wire, &both(), key);
    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one request where there were two");
    assert_eq!(heard[0].matches(SPEC).count(), 1, "the spec is sent once");
    assert_eq!(counted_today(&home), 1, "the day counts one request");
    let difficulty_row = receipts.difficulty.expect("the difficulty's row");
    let model_row = receipts.model.expect("the model's row");
    assert_eq!(difficulty_row["rubricVersion"], difficulty::RUBRIC_VERSION);
    assert_eq!(model_row["rubricVersion"], model::RUBRIC_VERSION);
    assert_eq!(difficulty_row["chosen"], LADDER[0].0);
    assert_eq!(difficulty_row["applied"], true, "{difficulty_row}");
    assert_eq!(model_row["chosen"], "model-b");
    assert_eq!(model_row["applied"], true, "{model_row}");
    let shared = &difficulty_row[SHARED_REQUEST_KEY];
    assert!(shared.is_string(), "{difficulty_row}");
    assert_eq!(&model_row[SHARED_REQUEST_KEY], shared);
    assert_eq!(
        (&difficulty_row["requests"], &model_row["requests"]),
        (&json!(1), &json!(0)),
        "the request is counted on one row"
    );
}

/// The mixed case of the day the release lands (the coordinator, 23:07): the
/// difficulty seat acts, the model seat only records — both ride the path's
/// one request, and the recording seat's answer is written down, never run.
#[test]
fn a_recording_seat_rides_the_acting_seats_request() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, 0);
    let wire = wire(&home, &endpoint, "on", "shadow");
    let key = ["assign-team", "%1", "mixed"];
    let _origin = super::super::summon_difficulty::origin_with_for_tests(
        key,
        Some(home.path().to_path_buf()),
        true,
        wire.settings_root(),
    );
    let receipts = choose_with(&wire, &both(), key);
    assert_eq!(endpoint.asked().len(), 1);
    assert_eq!(
        receipts.difficulty.map(|row| row["applied"].clone()),
        Some(json!(true))
    );
    let model_row = receipts.model.expect("the recording seat's row rode along");
    assert_eq!(model_row["chosen"], "model-b");
    assert_eq!(model_row["applied"], false, "{model_row}");
}

/// A4: a late answer leaves both rows unapplied, after one wall.
#[test]
fn a_late_answer_costs_one_wall_and_applies_nothing() {
    let home = tempfile::tempdir().unwrap();
    let endpoint =
        Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, APPLY_DEADLINE_MS + 500);
    let wire = wire(&home, &endpoint, "on", "on");
    let key = ["assign-team", "%1", "late"];
    let _origin = super::super::summon_difficulty::origin_with_for_tests(
        key,
        Some(home.path().to_path_buf()),
        true,
        wire.settings_root(),
    );
    let began = Instant::now();
    let receipts = choose_with(&wire, &both(), key);
    assert!(
        began.elapsed() < Duration::from_millis(APPLY_DEADLINE_MS + 400),
        "one wall, never two: {:?}",
        began.elapsed()
    );
    for row in [receipts.difficulty, receipts.model] {
        let row = row.expect("each seat's row");
        assert_eq!(row["applied"], false, "{row}");
        assert!(row.get("chosen").is_none(), "{row}");
    }
}

/// A launcher whose seats are this window's own, on a test's wire, over
/// `lineup` — the synthetic one unless a measurement sizes its own.
struct Wired<'a> {
    wire: &'a Wire,
    lineup: Lineup,
    /// The agents installed here, when a test leaves the agent open.
    agents: &'static [&'static str],
    /// Each agent's own lineup, where a measurement sizes them apart; every
    /// other agent has `lineup`.
    lineups: &'a [(&'static str, Lineup)],
}
impl Wired<'_> {
    fn lineup_of(&self, agent: &str) -> &Lineup {
        self.lineups
            .iter()
            .find(|(id, _)| *id == agent)
            .map_or(&self.lineup, |(_, lineup)| lineup)
    }
}
impl zerocode_core::orchestration::Launcher for Wired<'_> {
    fn command_for(&self, agent: &str, _: &str, _: &[String]) -> Result<String, String> {
        Ok(agent.to_string())
    }
    fn choose_assign(&self, asked: &AssignAsk, origin: [&str; 3]) -> Receipts {
        choose_with(self.wire, asked, origin)
    }
    fn difficulty_profile(
        &self,
        agent: &str,
        level: &str,
        _: [&str; 3],
    ) -> Result<Option<lineup::Row>, String> {
        lineup::row_at(
            &Value::Null,
            agent,
            level,
            Some(self.lineup_of(agent)),
            None,
            0,
        )
    }
    /// Which agents are here — the ones a test names (`Wired::agents`), so an
    /// open agent has something to choose between.
    fn presence(&self) -> Option<Vec<zerocode_core::agent::AgentPresence>> {
        (!self.agents.is_empty()).then(|| {
            zerocode_core::agent::agent_presence(None, "macos")
                .into_iter()
                .map(|mut row| {
                    row.installed = self.agents.contains(&row.id);
                    row
                })
                .collect()
        })
    }
    fn model_facts(&self, agent: &str, _: [&str; 3]) -> Option<model::Facts> {
        let lineup = self.lineup_of(agent);
        Some(model::Facts {
            lineup: lineup.clone(),
            seen: Seen::default(),
            // Every model with a record: no summons is a challenger's turn.
            records: lineup
                .models
                .iter()
                .map(|held| {
                    (
                        held.id.clone(),
                        ModelRecord {
                            ended: lineup::CHALLENGE_MIN_SAMPLES,
                            ..ModelRecord::default()
                        },
                    )
                })
                .collect(),
        })
    }
}

/// One claude summons with every dial open, planned on `wire`'s seats from a
/// checkout at `checkout`: the reply and what it prepared.
fn summoned(
    wire: &Wire,
    checkout: &Path,
    request: &str,
) -> (Value, zerocode_core::orchestration::PreparedWorkerStart) {
    summoned_as(wire, checkout, request, "claude", lineup())
}

/// [`summoned`], of `agent` over `lineup`.
fn summoned_as(
    wire: &Wire,
    checkout: &Path,
    request: &str,
    agent: &str,
    lineup: Lineup,
) -> (Value, zerocode_core::orchestration::PreparedWorkerStart) {
    summoned_among(wire, checkout, request, agent, lineup, &[])
}

/// [`summoned_as`] with `agents` installed — `agent` may be `auto`.
fn summoned_among(
    wire: &Wire,
    checkout: &Path,
    request: &str,
    agent: &str,
    lineup: Lineup,
    agents: &'static [&'static str],
) -> (Value, zerocode_core::orchestration::PreparedWorkerStart) {
    summoned_each(wire, checkout, request, agent, lineup, agents, &[])
}

/// [`summoned_among`] with each agent's own lineup in `lineups`.
fn summoned_each(
    wire: &Wire,
    checkout: &Path,
    request: &str,
    agent: &str,
    lineup: Lineup,
    agents: &'static [&'static str],
    lineups: &[(&'static str, Lineup)],
) -> (Value, zerocode_core::orchestration::PreparedWorkerStart) {
    let mut ledger = zerocode_core::orchestration::Ledger::new();
    let mut team = zerocode_core::agent_teams::Team::new("team-assign", "test-token", 1);
    let _origin = super::super::summon_difficulty::origin_with_for_tests(
        [team.id.as_str(), "%1", request],
        Some(checkout.to_path_buf()),
        true,
        wire.settings_root(),
    );
    let launcher = Wired {
        wire,
        lineup,
        agents,
        lineups,
    };
    let mut summoned = None;
    for (at, command) in [
        (
            1,
            "run-create --name assign --retry-request create".to_string(),
        ),
        (
            2,
            format!("worker-start --agent {agent} --prompt {SPEC_WORD} --retry-request {request}"),
        ),
    ] {
        let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        let planned = zerocode_core::orchestration::plan(
            &mut ledger,
            &mut team,
            &launcher,
            &argv,
            "%1",
            at,
            Some("test-actor"),
        );
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        ledger.file_receipt(&planned, at);
        if let Some(prepared) = planned.prepared_worker_start {
            summoned = Some((
                serde_json::from_str(&planned.reply.stdout).unwrap(),
                prepared,
            ));
        }
    }
    summoned.expect("a worker was reserved")
}

/// The summons's words, one word so the command line keeps it whole.
const SPEC_WORD: &str = "translate-labels";

/// A1, A4: a summons whose two seats act waits on one request and launches
/// the pair it answered; recording afterwards asks nothing more.
#[test]
fn a_summons_with_two_acting_seats_waits_on_one_request() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, 0);
    let wire = wire(&home, &endpoint, "on", "on");
    let (reply, prepared) = summoned(&wire, home.path(), "both-acting");
    assert_eq!(endpoint.asked().len(), 1, "one request on the path");
    assert_eq!(reply["model"], "model-b", "{reply}");
    assert_eq!(reply["dials"]["modelFrom"], "jev", "{reply}");
    let host = super::super::summon_difficulty::tests::Deferred::on(&wire);
    record(&host, &prepared, home.path().to_str(), 3);
    host.drain();
    assert_eq!(endpoint.asked().len(), 1, "receipts are not asked twice");
    assert_shared_rows(&wire, true);
}

/// [`wire`] with the agent seat at `agent` too.
fn wire_with_agent(
    home: &tempfile::TempDir,
    endpoint: &Endpoint,
    agent: &str,
    difficulty: &str,
    model: &str,
) -> Wire {
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {
            SUMMON.setting: agent,
            SUMMON_DIFFICULTY.setting: difficulty,
            SUMMON_MODEL.setting: model,
            "jev": {"workspaces": ["*"]},
        }})
        .to_string(),
    )
    .unwrap();
    Wire::at(&endpoint.base(), "test-key", Some(settings))
}

/// t-16578 A1: a summons that leaves the agent and the dials open waits on
/// ONE request carrying the agent, difficulty and pair questions, and runs the
/// agent it chose with that agent's pair; each seat's row names the request,
/// and recording afterwards asks nothing more.
#[test]
fn an_open_agent_rides_the_one_request() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, 0);
    let wire = wire_with_agent(&home, &endpoint, "on", "on", "on");
    let (reply, prepared) = summoned_among(
        &wire,
        home.path(),
        "agent-open",
        "auto",
        lineup(),
        &["claude", "codex"],
    );
    let heard = endpoint.asked();
    assert_eq!(heard.len(), 1, "one request where there were three");
    assert_eq!(heard[0].matches(SPEC).count(), 1, "the spec is sent once");
    for question in [
        "\"summon\"",
        "summon_difficulty",
        "summon_model_claude",
        "summon_model_codex",
    ] {
        assert!(heard[0].contains(question), "{question} rides");
    }
    assert_eq!(counted_today(&home), 1);
    assert_eq!(prepared.agent, "claude", "the answer's agent runs: {reply}");
    assert_eq!(reply["model"], "model-b", "{reply}");
    let receipt = prepared
        .summon_shadow
        .as_ref()
        .and_then(|s| s.receipt.clone());
    let receipt = receipt.expect("the agent seat's receipt rode the path");
    assert_eq!(receipt["chosen"], "claude");
    assert_eq!(receipt["applied"], true, "{receipt}");
    let host = super::super::summon_difficulty::tests::Deferred::on(&wire);
    super::super::summon_choice::record(&host, &prepared, home.path().to_str(), 3);
    record(&host, &prepared, home.path().to_str(), 3);
    host.drain();
    assert_eq!(endpoint.asked().len(), 1, "receipts are not asked twice");
}

/// t-16578 A1: a late answer on the one request leaves every dial to its
/// default and the agent refused by name — after one wall, never two.
#[test]
fn a_late_answer_to_the_open_agent_refuses_after_one_wall() {
    let home = tempfile::tempdir().unwrap();
    let endpoint =
        Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, APPLY_DEADLINE_MS + 500);
    let wire = wire_with_agent(&home, &endpoint, "on", "on", "on");
    let key = ["assign-team", "%1", "late-agent"];
    let _origin = super::super::summon_difficulty::origin_with_for_tests(
        key,
        Some(home.path().to_path_buf()),
        true,
        wire.settings_root(),
    );
    let asked = AssignAsk {
        agent: Some(zerocode_core::summon_choice::ask(&agent_look(), &rooms()).unwrap()),
        difficulty: Some(look()),
        ..Default::default()
    };
    let began = Instant::now();
    let receipts = choose_with(&wire, &asked, key);
    assert!(began.elapsed() < Duration::from_millis(APPLY_DEADLINE_MS + 400));
    let row = receipts.agent.expect("the agent seat's row");
    assert_eq!(row["applied"], false, "{row}");
    assert!(row.get("chosen").is_none(), "{row}");
}

fn agent_look() -> zerocode_core::summon_choice::SummonLook<'static> {
    zerocode_core::summon_choice::SummonLook {
        brief: SPEC,
        brief_chars: SPEC.len(),
        worktree: false,
        replaces_an_attempt: false,
        carries_a_task: false,
        attempts: 0,
        failures: 0,
        pinned_model: None,
    }
}

fn rooms() -> Vec<zerocode_core::summon_choice::Summonable> {
    ["claude", "codex"]
        .into_iter()
        .map(|id| zerocode_core::summon_choice::Summonable {
            id: id.to_string(),
            spent_percent: None,
            window: None,
            record: Default::default(),
        })
        .collect()
}

/// A5: two seats that only record are asked after the pane opens — one
/// request for both, each row in its own ledger, neither applied.
#[test]
fn two_recording_seats_share_one_request_after_the_pane_opens() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, 0);
    let wire = wire(&home, &endpoint, "shadow", "shadow");
    let (_, prepared) = summoned(&wire, home.path(), "both-recording");
    assert!(endpoint.asked().is_empty(), "nothing waits on the path");
    let host = super::super::summon_difficulty::tests::Deferred::on(&wire);
    record(&host, &prepared, home.path().to_str(), 3);
    assert!(
        endpoint.asked().is_empty(),
        "recording returns before a socket opens"
    );
    host.drain();
    assert_eq!(endpoint.asked().len(), 1, "one request for both");
    assert_shared_rows(&wire, false);
}

/// Each seat's ledger holds one row of its own rubric, `applied` as said,
/// and both name one shared request.
fn assert_shared_rows(wire: &Wire, applied: bool) {
    let rows =
        |seat| crate::systemone::read_rows(&crate::systemone::ledger_of(wire, seat).unwrap());
    let (difficulty_rows, model_rows) = (rows(&SUMMON_DIFFICULTY), rows(&SUMMON_MODEL));
    assert_eq!((difficulty_rows.len(), model_rows.len()), (1, 1));
    let (difficulty_row, model_row) = (&difficulty_rows[0], &model_rows[0]);
    assert_eq!(difficulty_row["rubricVersion"], difficulty::RUBRIC_VERSION);
    assert_eq!(model_row["rubricVersion"], model::RUBRIC_VERSION);
    assert_eq!(difficulty_row["applied"], applied, "{difficulty_row}");
    assert_eq!(model_row["applied"], applied, "{model_row}");
    assert!(
        difficulty_row[SHARED_REQUEST_KEY].is_string(),
        "{difficulty_row}"
    );
    assert_eq!(
        model_row[SHARED_REQUEST_KEY],
        difficulty_row[SHARED_REQUEST_KEY]
    );
    for row in [difficulty_row, model_row] {
        assert!(
            row.get("state").is_none() && row.get("spec").is_none(),
            "task words never enter a ledger"
        );
    }
}

/// A6: what a summons waits for Jev and sends, before and after, against a
/// loopback endpoint that answers in a fixed 250 ms — per agent summoned
/// this week, each over as many models naming no efforts as its lineup
/// holds on the machine it was measured on (`zo models --json`, 2026-09-29:
/// claude 8, codex 6, zo 26 with its custom providers'). Prints one JSON
/// line an agent; run by hand
/// (`-- --ignored --nocapture --exact
/// orchestration::summon_assign::tests::assign_moment_numbers`).
#[test]
#[ignore = "a measurement, run by hand"]
fn assign_moment_numbers() {
    const SAMPLES: usize = 30;
    const HOLD_MS: u64 = 250;
    for (agent, models) in [("claude", 8), ("codex", 6), ("zo", 26)] {
        let lineup = Lineup::from_catalog_all(&json!({"models": (0..models)
            .map(|nth| json!({"provider": "synthetic", "id": format!("model-{nth}"), "builtin": true}))
            .collect::<Vec<_>>()}))
        .unwrap();
        let mut waits = Vec::new();
        let mut requests = 0;
        let mut bytes = 0;
        for nth in 0..SAMPLES {
            let home = tempfile::tempdir().unwrap();
            let endpoint = Endpoint::answering_each("HTTP/1.1 200 OK", answer_every, HOLD_MS);
            let wire = wire(&home, &endpoint, "on", "on");
            let began = Instant::now();
            let _ = summoned_as(
                &wire,
                home.path(),
                &format!("measure-{nth}"),
                agent,
                lineup.clone(),
            );
            waits.push(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
            let heard = endpoint.asked();
            requests += heard.len();
            bytes += heard
                .iter()
                .map(|request| {
                    request
                        .split_once("\r\n\r\n")
                        .map_or(0, |(_, body)| body.len())
                })
                .sum::<usize>();
        }
        waits.sort_unstable();
        let at = |share: f64| zerocode_core::jev::summary::percentile(&waits, share);
        println!(
            "{}",
            json!({
                "agent": agent, "models": models, "samples": SAMPLES, "holdMs": HOLD_MS,
                "requestsPerSummons": requests as f64 / SAMPLES as f64,
                "requestBytesPerSummons": bytes / SAMPLES,
                "waitP50Ms": at(0.50), "waitP95Ms": at(0.95),
            })
        );
    }
}

/// t-16578 A4: what a summons that leaves the agent AND the dials open waits
/// for Jev and sends, before and after — before, the agent question asked
/// alone and then the difficulty and pair on a second request; after, one
/// request carrying all of it with a pair question for each agent the answer
/// may choose — against a loopback endpoint that answers in a fixed 250 ms,
/// per agent the answer lands on, each agent over as many models naming no
/// efforts as its lineup held (`zo models --json`, 2026-09-29: claude 8,
/// codex 6, zo 26) with all three installed. `joinBytes` is the bytes the pair
/// questions put in the one request ([`AssignAsk::join_bytes`]); `pairOptions`
/// the options the landing agent's own pair question offers, which scoping
/// does not change. Prints one JSON line an agent; run by hand
/// (`-- --ignored --nocapture --exact
/// orchestration::summon_assign::tests::open_agent_numbers`).
#[test]
#[ignore = "a measurement, run by hand"]
fn open_agent_numbers() {
    const SAMPLES: usize = 30;
    const HOLD_MS: u64 = 250;
    let lineups: Vec<(&'static str, Lineup)> = [("claude", 8), ("codex", 6), ("zo", 26)]
        .into_iter()
        .map(|(agent, models)| {
            let catalog = json!({"models": (0..models)
                .map(|nth| json!({"provider": "synthetic", "id": format!("model-{nth}"), "builtin": true}))
                .collect::<Vec<_>>()});
            (agent, Lineup::from_catalog_all(&catalog).unwrap())
        })
        .collect();
    let installed: &'static [&'static str] = &["claude", "codex", "zo"];
    let bytes_of = |heard: &[String]| {
        heard
            .iter()
            .map(|request| {
                request
                    .split_once("\r\n\r\n")
                    .map_or(0, |(_, body)| body.len())
            })
            .sum::<usize>()
    };
    for (agent, lineup) in &lineups {
        let agent: &'static str = agent;
        let mut results = Vec::new();
        for joined in [false, true] {
            let mut waits = Vec::new();
            let (mut requests, mut bytes) = (0, 0);
            for nth in 0..SAMPLES {
                let home = tempfile::tempdir().unwrap();
                let endpoint = Endpoint::answering_each(
                    "HTTP/1.1 200 OK",
                    move |request| answered(request, Some(agent)),
                    HOLD_MS,
                );
                let wire = wire_with_agent(&home, &endpoint, "on", "on", "on");
                let request = format!("measure-{joined}-{nth}");
                let began = Instant::now();
                if joined {
                    let _ = summoned_each(
                        &wire,
                        home.path(),
                        &request,
                        "auto",
                        lineup.clone(),
                        installed,
                        &lineups,
                    );
                } else {
                    // Before: the agent alone, then the dials on the agent it
                    // chose — two requests, one after the other.
                    let key = ["assign-team", "%1", request.as_str()];
                    let _origin = super::super::summon_difficulty::origin_with_for_tests(
                        key,
                        Some(home.path().to_path_buf()),
                        true,
                        wire.settings_root(),
                    );
                    let chosen = super::super::summon_choice::choose_with(
                        &wire,
                        &agent_look(),
                        &rooms_of(installed),
                        key,
                    );
                    assert_eq!(chosen.as_deref(), Some(agent));
                    let _ = summoned_each(
                        &wire,
                        home.path(),
                        &request,
                        agent,
                        lineup.clone(),
                        &[],
                        &lineups,
                    );
                }
                waits.push(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
                let heard = endpoint.asked();
                requests += heard.len();
                bytes += bytes_of(&heard);
            }
            waits.sort_unstable();
            let at = |share: f64| zerocode_core::jev::summary::percentile(&waits, share);
            results.push(json!({
                "shape": if joined { "after: one request" } else { "before: agent, then dials" },
                "requestsPerSummons": requests as f64 / SAMPLES as f64,
                "requestBytesPerSummons": bytes / SAMPLES,
                "waitP50Ms": at(0.50), "waitP95Ms": at(0.95),
            }));
        }
        let models = lineup.models.len();
        println!(
            "{}",
            json!({
                "agent": agent, "models": models, "samples": SAMPLES, "holdMs": HOLD_MS,
                "installed": installed,
                "results": results,
            })
        );
    }
}

fn rooms_of(agents: &[&str]) -> Vec<zerocode_core::summon_choice::Summonable> {
    agents
        .iter()
        .map(|id| zerocode_core::summon_choice::Summonable {
            id: (*id).to_string(),
            spent_percent: None,
            window: None,
            record: Default::default(),
        })
        .collect()
}

/// A6 against the real service: the path's request with the difficulty
/// question alone, and with the model question riding beside it, asked in
/// turn — what a summons waits for each (p50, p95) and what each sends. The
/// number that decides whether a recording seat rides an acting seat's
/// request (the coordinator, 2026-09-29 23:07: apart if the joint request is
/// slower at p95 by more than a fifth). Settings, the day's count and the
/// ledgers are a temporary home's; the key rides one command's environment
/// and is never printed.
///
/// ```sh
/// TYPESAFE_API_KEY="$(security find-generic-password \
///     -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w)" \
///   cargo test -p zerocode-shell --bin zerocode-shell \
///   orchestration::summon_assign::tests::live_assign_moment_numbers \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "live Jev requests; requires a command-scoped key"]
fn live_assign_moment_numbers() {
    const SAMPLES: usize = 20;
    let key = std::env::var("TYPESAFE_API_KEY").expect("a command-scoped key");
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {
            SUMMON_DIFFICULTY.setting: "on",
            SUMMON_MODEL.setting: "on",
            "jev": {"workspaces": ["*"]},
        }})
        .to_string(),
    )
    .unwrap();
    let wire = Wire::at(crate::systemone::SYSTEMONE_BASE_URL, &key, Some(settings));
    let mut single = Vec::new();
    let mut joint = Vec::new();
    let answered =
        |row: &Option<Value>| row.as_ref().is_some_and(|row| row["outcome"] == "answered");
    for _ in 0..SAMPLES {
        for (asked, held) in [
            (both().only(false, true, false), &mut single),
            (both(), &mut joint),
        ] {
            let receipts = ask(&wire, &asked, Some(home.path()));
            let row = receipts.difficulty.clone();
            if answered(&row) && (asked.model.is_none() || answered(&receipts.model)) {
                let row = row.unwrap_or_default();
                held.push((
                    row["elapsedMs"].as_u64().unwrap_or(u64::MAX),
                    row["requestBytes"].as_u64().unwrap_or_default(),
                ));
            }
        }
    }
    let said = |held: &[(u64, u64)]| {
        let bytes = held.first().map(|(_, bytes)| *bytes);
        let mut waits: Vec<u64> = held.iter().map(|(wait, _)| *wait).collect();
        waits.sort_unstable();
        json!({
            "answered": waits.len(),
            "p50Ms": zerocode_core::jev::summary::percentile(&waits, 0.50),
            "p95Ms": zerocode_core::jev::summary::percentile(&waits, 0.95),
            "requestBytes": bytes,
        })
    };
    println!(
        "{}",
        json!({"samples": SAMPLES, "single": said(&single), "joint": said(&joint)})
    );
}

/// Synthetic work of every weight, for the comparison below: a title and the
/// first words of a spec.
const WEIGHED: [(&str, &str); 12] = [
    (
        "Fix a typo in the settings label",
        "One label reads 'Langauge'. Correct the word in the five catalogs.",
    ),
    (
        "Translate the onboarding page",
        "Translate twelve strings of the onboarding page into five languages.",
    ),
    (
        "Add a unit test for the date parser",
        "The parser has no test for a leap day. Add one; change no product code.",
    ),
    (
        "Update the README's install section",
        "The install command changed. Rewrite the section and its two examples.",
    ),
    (
        "A settings card for the update channel",
        "Add a card with a picker of three channels, saved through the settings road, with tests.",
    ),
    (
        "A benchmark for the file search",
        "Measure the search over a synthetic tree of fifty thousand files; report p50 and p95.",
    ),
    (
        "Port the task list to the terminal view",
        "The window's task list, drawn in the terminal view with the same keys and states.",
    ),
    (
        "A sidebar filter by worker state",
        "Filter the sidebar's rows by state; the filter is remembered per workspace.",
    ),
    (
        "Make the ledger survive a full disk",
        "When the disk is full the ledger closes. Design and build a recovery that loses no row.",
    ),
    (
        "A race between restore and account switch",
        "Two roads seat one worker twice under load. Find the race, prove it, and close it.",
    ),
    (
        "Replace the scheduler of the release lane",
        "Phases run in order today. Design a scheduler that runs independent phases side by side.",
    ),
    (
        "Migrate the transcript store's format",
        "A new on-disk format, read and written by two versions at once, with a migration nobody waits on.",
    ),
];

/// Eight models that name no efforts — what an agent's lineup was on the
/// machine this was written on.
fn eight() -> Lineup {
    let models: Vec<Value> = ["a", "b", "c", "d", "e", "f", "g", "h"]
        .into_iter()
        .enumerate()
        .map(|(at, name)| {
            let (band, rungs) = match at {
                0 => ("first", json!(["hard"])),
                1 | 2 => ("second", json!(["medium", "hard"])),
                _ => ("rest", json!(["easy", "medium"])),
            };
            json!({"provider": "claude", "id": format!("model-{name}"), "builtin": at < 2,
                   "band": band, "rungs": rungs})
        })
        .collect();
    Lineup::from_catalog(&json!({ "models": models }), "claude").unwrap()
}

/// Whether riding one request moves what the difficulty seat says: every
/// task of [`WEIGHED`] asked alone and beside the model question (eight
/// models at each effort of the ladder), the two answers side by side. The
/// difficulty's rubric did not change; its state gained the models' keys,
/// and this is how much that moved its answers. Live, as above — the key
/// rides one command's environment and is never printed.
#[test]
#[ignore = "live Jev requests; requires a command-scoped key"]
fn live_difficulty_answers_alone_and_joint() {
    let key = std::env::var("TYPESAFE_API_KEY").expect("a command-scoped key");
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {
            SUMMON_DIFFICULTY.setting: "on",
            SUMMON_MODEL.setting: "on",
            "jev": {"workspaces": ["*"]},
        }})
        .to_string(),
    )
    .unwrap();
    let wire = Wire::at(crate::systemone::SYSTEMONE_BASE_URL, &key, Some(settings));
    let lineup = eight();
    let records: std::collections::BTreeMap<String, ModelRecord> = lineup
        .models
        .iter()
        .map(|model| {
            (
                model.id.clone(),
                ModelRecord {
                    ended: lineup::CHALLENGE_MIN_SAMPLES,
                    ..ModelRecord::default()
                },
            )
        })
        .collect();
    let ladder: Vec<&str> = LADDER.iter().map(|(_, _, effort)| *effort).collect();
    let options = model::options("claude", &lineup, None, &records, &ladder, |_| None, 0);
    let said = |row: Option<Value>| {
        let row = row.unwrap_or_default();
        json!({
            "outcome": row["outcome"],
            "chosen": row["chosen"],
            "confidence": row["confidence"],
        })
    };
    let mut same = 0;
    let mut compared = 0;
    let mut pairs = 0;
    for (at, (title, spec)) in WEIGHED.into_iter().enumerate() {
        let look = Look {
            title: title.into(),
            spec: spec.into(),
            attempt: 0,
            failures: 0,
            retry_of: false,
        };
        let asked = AssignAsk {
            difficulty: Some(look.clone()),
            model: Some(model::ask(&look, &options).unwrap()),
            ..Default::default()
        };
        let alone = said(ask(&wire, &asked.only(false, true, false), Some(home.path())).difficulty);
        let joint = ask(&wire, &asked, Some(home.path()));
        pairs += usize::from(
            joint
                .model
                .as_ref()
                .is_some_and(|row| row["outcome"] == "answered"),
        );
        let beside = said(joint.difficulty);
        if alone["outcome"] == "answered" && beside["outcome"] == "answered" {
            compared += 1;
            same += usize::from(alone["chosen"] == beside["chosen"]);
        }
        println!("{}", json!({"task": at, "alone": alone, "joint": beside}));
    }
    println!(
        "{}",
        json!({"tasks": WEIGHED.len(), "compared": compared, "sameAnswer": same,
               "modelAnswered": pairs, "options": options.len()})
    );
}
