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
        &[LADDER[0].2],
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

/// The pair `model` at the ladder's easy effort — an effort every question
/// here offers a model that names none at.
fn pair(model: &str) -> String {
    model::option_word(model, LADDER[0].2)
}

fn answer(word: &str) -> String {
    answer_to(&asked(), word)
}

/// Jev's answer `word` to `asked`, over every option it offered.
fn answer_to(asked: &ModelAsk, word: &str) -> String {
    let words: Vec<String> = asked
        .offered()
        .iter()
        .map(|(offered, effort)| model::option_word(offered, effort))
        .chain([model::ABSTAIN.to_string()])
        .collect();
    // The rest of the chosen word's share, spread over the others.
    let rest = 0.15 / f64::from(u32::try_from(words.len() - 1).unwrap_or(u32::MAX));
    let probabilities: serde_json::Map<String, Value> = words
        .iter()
        .map(|each| (each.clone(), json!(if each == word { 0.85 } else { rest })))
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
        ("shadow", pair("model-b"), false, false),
        ("on", pair("model-b"), true, true),
        ("on", model::ABSTAIN.to_string(), true, false),
    ] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(&word), 0);
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
            let named = if applied { "model-b" } else { word.as_str() };
            assert_eq!(row["chosen"], named);
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

/// A launcher whose summons reads a synthetic lineup and answers every
/// difficulty `low`. Its model seat only records, or acts — and then counts
/// how often the launch's own path asked it.
#[derive(Default)]
struct Lined {
    acts: bool,
    asked_on_the_path: std::sync::atomic::AtomicUsize,
}
impl Lined {
    fn acting() -> Self {
        Self {
            acts: true,
            ..Self::default()
        }
    }

    fn asked_on_the_path(&self) -> usize {
        self.asked_on_the_path
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}
impl zerocode_core::orchestration::Launcher for Lined {
    fn command_for(&self, _: &str, _: &str, _: &[String]) -> Result<String, String> {
        Ok("claude".into())
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
    fn choose_assign(
        &self,
        asked: &zerocode_core::summon_assign::AssignAsk,
        _: [&str; 3],
    ) -> zerocode_core::summon_assign::Receipts {
        let model = asked.model.as_ref().filter(|_| self.acts).map(|_| {
            self.asked_on_the_path
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            json!({
                "outcome": "answered", "chosen": "model-b", model::EFFORT_KEY: LADDER[0].2,
                "confidence": 0.85, "applied": true,
            })
        });
        zerocode_core::summon_assign::Receipts {
            difficulty: asked
                .difficulty
                .as_ref()
                .map(|_| json!({"chosen": LADDER[0].0, "applied": true})),
            model,
        }
    }
}

/// A run with one claude summons after another under `launcher`: each
/// summons's reply and what it prepared.
fn summoned(
    launcher: &Lined,
    summonses: usize,
) -> Vec<(Value, zerocode_core::orchestration::PreparedWorkerStart)> {
    let mut ledger = zerocode_core::orchestration::Ledger::new();
    let mut team = zerocode_core::agent_teams::Team::new("team-test", "test-token", 1);
    let mut commands = vec!["run-create --name model --retry-request create".to_string()];
    commands.extend((0..summonses).map(|nth| {
        format!("worker-start --agent claude --prompt translate --retry-request start-{nth}")
    }));
    let mut summoned = Vec::new();
    for (at, command) in (1..).zip(commands) {
        let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
        let planned = zerocode_core::orchestration::plan(
            &mut ledger,
            &mut team,
            launcher,
            &argv,
            "%1",
            at,
            Some("test-actor"),
        );
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        ledger.file_receipt(&planned, at);
        if let Some(prepared) = planned.prepared_worker_start {
            summoned.push((
                serde_json::from_str(&planned.reply.stdout).unwrap(),
                prepared,
            ));
        }
    }
    summoned
}

/// The first claude summons of a ledger is a challenger's turn at easy
/// work: it launches a model with no record, and its row — asked after the
/// pane opened — says so and grades no answer.
#[test]
fn a_challengers_turn_is_recorded_off_the_beat_and_marked() {
    let (reply, prepared) = summoned(&Lined::default(), 1).remove(0);
    assert_eq!(reply["dials"]["modelFrom"], "challenge", "{reply}");
    assert_eq!(
        reply["model"], "model-c",
        "the low difficulty's untried model"
    );
    let home = tempfile::tempdir().unwrap();
    let kept = prepared
        .model_shadow
        .as_ref()
        .expect("the question is kept");
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer_to(&kept.ask, &pair("model-b")), 0);
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

/// A summons never waits on an answer nothing will act on: on a challenger's
/// turn an acting seat is not asked on the launch's own path — its question
/// is kept for the record, asked after the pane opens — and on every other
/// turn it is asked once and its pair runs.
#[test]
fn a_challengers_turn_does_not_wait_on_the_acting_seat() {
    let launcher = Lined::acting();
    let mut summoned = summoned(&launcher, 2);
    let (other, _) = summoned.remove(1);
    let (challenge, prepared) = summoned.remove(0);
    assert_eq!(challenge["dials"]["modelFrom"], "challenge", "{challenge}");
    assert_eq!(challenge["model"], "model-c");
    let shadow = prepared.model_shadow.expect("the question is kept");
    assert!(shadow.challenge);
    assert_eq!(
        shadow.receipt, None,
        "nothing was asked on the launch's path"
    );
    assert_eq!(other["dials"]["modelFrom"], "jev", "{other}");
    assert_eq!(other["model"], "model-b");
    assert_eq!(
        launcher.asked_on_the_path(),
        1,
        "only the summons that acts on the answer waited for it"
    );
}
