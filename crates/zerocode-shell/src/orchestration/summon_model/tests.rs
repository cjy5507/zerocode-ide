use super::*;
use crate::systemone::tests::Endpoint;
use zerocode_core::summon_difficulty::lineup::{Lineup, Seen};
use zerocode_core::summon_difficulty::{LADDER, Look};

fn look() -> Look {
    Look {
        title: "Translate labels".into(),
        spec: "Translate the settings labels".into(),
        attempt: 0,
        failures: 0,
        retry_of: false,
    }
}

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

fn asked() -> ModelAsk {
    let options = model::options(
        "claude",
        &lineup(),
        None,
        &Default::default(),
        LADDER[0].2,
        |_| None,
        0,
    );
    model::ask(&look(), &options).unwrap()
}

fn wire(home: &tempfile::TempDir, endpoint: &Endpoint, mode: &str) -> Wire {
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {SUMMON_MODEL.setting: mode, "jev": {"workspaces": ["*"]}}}).to_string(),
    )
    .unwrap();
    Wire::at(&endpoint.base(), "test-key", Some(settings))
}

fn answer(word: &str) -> String {
    let words = ["model-a", "model-b", "model-c", model::ABSTAIN];
    let probabilities: serde_json::Map<String, Value> = words
        .iter()
        .map(|each| {
            (
                (*each).to_string(),
                json!(if *each == word { 0.85 } else { 0.05 }),
            )
        })
        .collect();
    json!({"model": "jev-1.13.0", "answers": {model::QUESTION: {
        "type": "choice", "choice": word, "probabilities": probabilities, "confidence": 0.85,
    }}})
    .to_string()
}

#[test]
fn only_a_seat_that_applies_asks_on_the_launchs_path_and_abstain_never_acts() {
    let key = ["model-team", "%1", "model-request"];
    for (mode, word, receipt, applied) in [
        ("shadow", "model-b", false, false),
        ("on", "model-b", true, true),
        ("on", model::ABSTAIN, true, false),
    ] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(word), 0);
        let wire = wire(&home, &endpoint, mode);
        let guard = super::super::summon_difficulty::origin_with_for_tests(
            key,
            Some(home.path().to_path_buf()),
            true,
            wire.settings_root(),
        );
        let row = choose_with(&wire, &asked(), key);
        assert_eq!(
            row.is_some(),
            receipt,
            "{mode}: the launch waits only on an acting seat"
        );
        assert_eq!(endpoint.asked().len(), usize::from(receipt));
        if let Some(row) = row {
            assert_eq!(row["chosen"], word);
            assert_eq!(row["applied"], applied, "{word}");
            assert_eq!(
                row[model::EFFORT_KEY],
                if applied {
                    json!(LADDER[0].2)
                } else {
                    Value::Null
                }
            );
        }
        drop(guard);
    }
}

/// A launcher whose summons reads a synthetic lineup, answers every
/// difficulty `low`, and whose model seat only records.
struct Lined;
impl zerocode_core::orchestration::Launcher for Lined {
    fn command_for(&self, _: &str, _: &str, _: &[String]) -> Result<String, String> {
        Ok("claude".into())
    }
    fn choose_difficulty(&self, _: &Look, _: [&str; 3]) -> Option<Value> {
        Some(json!({"chosen": LADDER[0].0, "applied": true}))
    }
    fn difficulty_profile(
        &self,
        agent: &str,
        difficulty: &str,
        _: [&str; 3],
    ) -> Result<Option<zerocode_core::summon_difficulty::lineup::Row>, String> {
        zerocode_core::summon_difficulty::lineup::row_at(
            &Value::Null,
            agent,
            difficulty,
            Some(&lineup()),
            None,
            0,
        )
    }
    fn model_facts(&self, _: &str, _: [&str; 3]) -> Option<model::Facts> {
        Some(model::Facts {
            lineup: lineup(),
            seen: Seen::default(),
            records: Default::default(),
        })
    }
}

/// The first claude summons of a ledger is a challenger's turn at easy
/// work: it launches a model with no record, and its row — asked after the
/// pane opened — says so and grades no answer.
#[test]
fn a_challengers_turn_is_recorded_off_the_beat_and_marked() {
    let mut ledger = zerocode_core::orchestration::Ledger::new();
    let mut team = zerocode_core::agent_teams::Team::new("team-test", "test-token", 1);
    let mut prepared = None;
    let mut reply = Value::Null;
    for (at, command) in [
        (1, "run-create --name model --retry-request create"),
        (
            2,
            "worker-start --agent claude --prompt translate --retry-request start",
        ),
    ] {
        let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        let planned = zerocode_core::orchestration::plan(
            &mut ledger,
            &mut team,
            &Lined,
            &argv,
            "%1",
            at,
            Some("test-actor"),
        );
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        ledger.file_receipt(&planned, at);
        if planned.prepared_worker_start.is_some() {
            reply = serde_json::from_str(&planned.reply.stdout).unwrap();
            prepared = planned.prepared_worker_start;
        }
    }
    assert_eq!(reply["dials"]["modelFrom"], "challenge", "{reply}");
    assert_eq!(
        reply["model"], "model-c",
        "the low difficulty's untried model"
    );
    let prepared = prepared.unwrap();
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer("model-b"), 0);
    let host =
        super::super::summon_difficulty::tests::Deferred::on(&wire(&home, &endpoint, "shadow"));
    record(&host, &prepared, home.path().to_str(), 3);
    assert!(
        endpoint.asked().is_empty(),
        "recording returns before a socket opens"
    );
    host.drain();
    assert_eq!(
        endpoint.asked().len(),
        1,
        "Jev is still asked, for the record"
    );
    let rows = crate::systemone::read_rows(
        &crate::systemone::ledger_of(&host.wire, &SUMMON_MODEL).unwrap(),
    );
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["chosen"], "model-b");
    assert_eq!(row["applied"], false);
    assert_eq!(row[model::CHALLENGE_KEY], true);
    assert_eq!(row["executionModel"], "model-c");
    assert!(
        row.get("state").is_none() && row.get("spec").is_none(),
        "task words never enter this ledger"
    );
}
