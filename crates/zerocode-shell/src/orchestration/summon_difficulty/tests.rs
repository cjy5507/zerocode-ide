use super::*;
use crate::systemone::tests::Endpoint;

fn look() -> Look {
    Look {
        title: "Translate labels".into(),
        spec: "Translate the settings labels".into(),
        attempt: 0,
        failures: 0,
        retry_of: false,
    }
}
fn wire(home: &tempfile::TempDir, endpoint: &Endpoint, mode: &str) -> Wire {
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {
            SUMMON_DIFFICULTY.setting: mode, "jev": {"workspaces": ["*"]}
        }})
        .to_string(),
    )
    .unwrap();
    Wire::at(&endpoint.base(), "test-key", Some(settings))
}
fn answer() -> String {
    json!({"model":"jev-1.13.0", "answers": {difficulty::QUESTION: {
        "type":"choice", "choice":difficulty::LADDER[0].0,
        "probabilities": {difficulty::LADDER[0].0:0.9, difficulty::LADDER[1].0:0.05, difficulty::LADDER[2].0:0.05}, "confidence":0.85
    }}}).to_string()
}
#[test]
fn recording_and_acting_ask_the_same_question_but_only_acting_applies() {
    for (mode, applied) in [("shadow", false), ("on", true)] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
        let row = ask(&wire(&home, &endpoint, mode), &look(), Some(home.path()));
        assert_eq!(row["chosen"], difficulty::LADDER[0].0, "{row}");
        assert_eq!(row["applied"], applied);
        assert_eq!(endpoint.asked().len(), 1);
        assert!(
            row.get("agreed").is_none(),
            "execution never supplies its own teacher"
        );
    }
}
#[test]
fn deadline_and_schema_failures_supply_no_applied_effort() {
    for (body, hold) in [
        (answer(), difficulty::APPLY_DEADLINE_MS + 500),
        ("{}".into(), 0),
    ] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", body, hold);
        let began = Instant::now();
        let row = ask(&wire(&home, &endpoint, "on"), &look(), Some(home.path()));
        assert_eq!(row["applied"], false);
        assert!(row.get("chosen").is_none());
        assert!(began.elapsed() < Duration::from_millis(difficulty::APPLY_DEADLINE_MS + 400));
    }
}

/// Production question and door, on isolated settings and ledgers. Neither
/// the person's Jev files nor project state are written during replay.
#[test]
#[ignore = "live Jev measurement; requires seed, output and a command-scoped key"]
fn replay_recorded_summonses() {
    let seed = std::env::var("ZEROCODE_DIFFICULTY_REPLAY_SEED").expect("seed path");
    let output = std::env::var("ZEROCODE_DIFFICULTY_REPLAY_OUTPUT").expect("output path");
    let key = std::env::var("TYPESAFE_API_KEY").expect("command-scoped key");
    let rows: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(seed).unwrap()).unwrap();
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {SUMMON_DIFFICULTY.setting:"shadow", "jev":{"workspaces":["*"]}}})
            .to_string(),
    )
    .unwrap();
    let wire = Wire::at(crate::systemone::SYSTEMONE_BASE_URL, &key, Some(settings));
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        // Independent requests, a fixed modest concurrency, one per summons.
        for slice in rows.chunks(rows.len().div_ceil(8).max(1)) {
            let wire = &wire;
            let home = &home;
            let results = &results;
            scope.spawn(move || {
                for seed in slice {
                    let look = Look {
                        title: seed["title"].as_str().unwrap().into(),
                        spec: seed["spec"].as_str().unwrap().into(),
                        attempt: usize::try_from(seed["attempt"].as_u64().unwrap()).unwrap(),
                        failures: u32::try_from(seed["failures"].as_u64().unwrap()).unwrap(),
                        retry_of: seed["retryOf"].as_bool().unwrap(),
                    };
                    let mut row = ask(wire, &look, Some(home.path()));
                    difficulty::compare(&mut row, seed["effort"].as_str());
                    for name in [
                        "run",
                        "worker",
                        "task",
                        "dispatch",
                        "effort",
                        "reworkRounds",
                        "retryCount",
                        "workerDone",
                        "doneWithReceipts",
                        "verified",
                        "merged",
                        "ended",
                    ] {
                        row[name] = seed[name].clone();
                    }
                    row["wouldEffort"] = json!(row["chosen"].as_str().and_then(|chosen| {
                        zerocode_core::orchestration::difficulty_effort(
                            seed["agent"].as_str().unwrap(),
                            chosen,
                        )
                    }));
                    results.lock().unwrap().push(row);
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by(|a, b| a["worker"].as_str().cmp(&b["worker"].as_str()));
    std::fs::write(output, serde_json::to_string_pretty(&results).unwrap()).unwrap();
    assert_eq!(results.len(), rows.len());
    println!(
        "replayed {} summonses; operational promotion unchanged",
        results.len()
    );
}

#[test]
fn recording_is_deferred_and_an_acting_receipt_is_not_asked_twice() {
    struct Deferred {
        wire: Wire,
        jobs: std::cell::RefCell<Vec<Box<dyn FnOnce() + Send>>>,
    }
    impl Host for Deferred {
        fn split(
            &self,
            _team: &str,
            _leader_term: u32,
            _from_term: u32,
            _pane: &str,
            _direction: zerocode_core::agent_teams::Direction,
            _command: &str,
            _token: &str,
        ) -> Option<u32> {
            None
        }
        fn send(&self, _term: u32, _text: &str) -> bool {
            false
        }
        fn capture(&self, _term: u32) -> Option<String> {
            None
        }
        fn focus(&self, _term: u32) -> bool {
            false
        }
        fn close(&self, _term: u32) {}
        fn jev_wire(&self) -> Option<Wire> {
            Some(self.wire.clone())
        }
        fn off_the_beat(&self, job: Box<dyn FnOnce() + Send>) {
            self.jobs.borrow_mut().push(job);
        }
    }
    struct Launch;
    impl zerocode_core::orchestration::Launcher for Launch {
        fn command_for(
            &self,
            _agent: &str,
            _prompt: &str,
            _tuning: &[String],
        ) -> Result<String, String> {
            Ok("claude".into())
        }
    }
    let mut ledger = zerocode_core::orchestration::Ledger::new();
    let mut team = zerocode_core::agent_teams::Team::new("team-test", "test-token", 1);
    let mut prepared = None;
    for (at, command) in [
        (1, "run-create --name difficulty --retry-request create"),
        (
            2,
            "worker-start --agent claude --model opus --effort max --prompt translate --retry-request start",
        ),
    ] {
        let argv = command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
        let planned = zerocode_core::orchestration::plan(
            &mut ledger,
            &mut team,
            &Launch,
            &argv,
            "%1",
            at,
            Some("test-actor"),
        );
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        ledger.file_receipt(&planned, at);
        if planned.prepared_worker_start.is_some() {
            prepared = planned.prepared_worker_start;
        }
    }
    let mut prepared = prepared.unwrap();
    for carried in [false, true] {
        let home = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
        let host = Deferred {
            wire: wire(&home, &endpoint, "shadow"),
            jobs: Default::default(),
        };
        let shadow = prepared.difficulty_shadow.as_mut().unwrap();
        shadow.receipt = carried.then(
            || json!({"outcome":"answered", "chosen":difficulty::LADDER[0].0, "applied":true}),
        );
        record(&host, &prepared, home.path().to_str(), 3);
        assert!(
            endpoint.asked().is_empty(),
            "recording must return before a socket opens"
        );
        let job = host.jobs.borrow_mut().pop().unwrap();
        job();
        assert_eq!(endpoint.asked().len(), usize::from(!carried));
        let path = crate::systemone::ledger_of(&host.wire, &SUMMON_DIFFICULTY).unwrap();
        let rows = crate::systemone::read_rows(&path);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row["worker"], prepared.worker);
        assert_eq!(row["dispatch"], prepared.worker);
        assert_eq!(row["requestAt"], 3);
        assert_eq!(row["pinnedEffort"], difficulty::LADDER[2].2);
        assert!(row.get("agreed").is_none());
        assert!(row.get("baselineAgreed").is_none());
        assert_eq!(row["applied"], carried);
        assert!(
            row.get("spec").is_none() && row.get("state").is_none(),
            "task words never enter this ledger"
        );
    }
}

#[test]
fn an_acting_request_uses_only_its_hosts_scoped_consent_origin() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
    let wire = wire(&home, &endpoint, "on");
    let key = ["team-origin", "%1", "origin-request"];
    let guard = origin_with(
        key,
        Some(home.path().to_path_buf()),
        true,
        wire.settings_root(),
    );
    let row = choose_with(&wire, &look(), key).unwrap();
    assert_eq!(row["chosen"], difficulty::LADDER[0].0);
    assert_eq!(endpoint.asked().len(), 1);
    drop(guard);
    let row = choose_with(&wire, &look(), key).unwrap();
    assert_eq!(row["applied"], false);
    assert_eq!(row["requests"], 0);
    assert_eq!(
        endpoint.asked().len(),
        1,
        "a missing origin must never open a socket"
    );
    let other = origin_with(
        [key[0], key[1], "another-request"],
        Some(home.path().to_path_buf()),
        true,
        wire.settings_root(),
    );
    assert_eq!(choose_with(&wire, &look(), key).unwrap()["requests"], 0);
    drop(other);
}

/// Grade a fixed replay with production outcome and usage readers, from a
/// read-only authority snapshot. No Jev request or operating ledger write.
#[test]
#[ignore = "private read-only replay; requires snapshot, previous answers, output and config root"]
fn replay_execution_outcomes() {
    use zerocode_core::orchestration::{Ledger, PROJECTION_SCHEMA, task_cost};
    let snapshot = std::env::var("ZEROCODE_OUTCOME_SNAPSHOT").expect("snapshot");
    let previous = std::env::var("ZEROCODE_OUTCOME_ANSWERS").expect("answers");
    let output = std::env::var("ZEROCODE_OUTCOME_OUTPUT").expect("output");
    let config = std::env::var("ZEROCODE_OUTCOME_CONFIG").expect("config root");
    let connection =
        rusqlite::Connection::open_with_flags(snapshot, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let held =
        zerocode_orchestrator::ledger_store::read(&connection, "main-ledger", PROJECTION_SCHEMA)
            .unwrap()
            .unwrap();
    let ledger = Ledger::rebuild(held.projection).unwrap();
    let mut rows: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(previous).unwrap()).unwrap();
    let now = crate::now_epoch_ms();
    let claude = crate::usage_stats_scan::scan(Path::new(&config), &[], 0, now);
    let codex = crate::usage_stats_scan::codex_scan(Path::new(&config), &[], 0, now);
    assert!(
        !claude.capped && !codex.capped,
        "a capped usage scan is not a complete replay"
    );
    let mut sessions = task_cost::SessionBook::default();
    sessions.read_claude(&claude.ledger, now);
    sessions.read_codex(&codex.ledger, now);
    for row in &mut rows {
        let run = ledger.run(row["run"].as_str().unwrap()).unwrap();
        let dispatch = run.dispatch(row["dispatch"].as_str().unwrap()).unwrap();
        let worker = run.worker(&dispatch.worker).unwrap();
        row["agent"] = json!(worker.agent);
        row["executionModel"] = json!(worker.model);
        let high =
            difficulty::profile(&Value::Null, &worker.agent, difficulty::LADDER[2].0).unwrap();
        row["baselineHigh"] = json!(
            high.is_some_and(|p| worker.model.as_deref() == Some(p.model.as_str())
                && row["effort"].as_str() == Some(p.effort.as_str()))
        );
        row["attempt"] = json!(
            run.dispatches
                .iter()
                .filter(|d| d.task == dispatch.task && d.started_ms < dispatch.started_ms)
                .count()
        );
        row["retryOf"] = json!(dispatch.retry_of.is_some());
        let pin = row["effort"].as_str().map(str::to_owned);
        difficulty::compare(row, pin.as_deref());
        let generation = task_cost::attempt_generation(run, dispatch, &sessions);
        let total = task_cost::task_cost(
            run,
            &dispatch.task,
            &sessions,
            task_cost::JevTally::default(),
        );
        row[difficulty::outcomes::KEY] = serde_json::to_value(difficulty::outcomes::observe(
            run,
            dispatch,
            &generation,
            &total,
        ))
        .unwrap();
        row["usage"] = serde_json::to_value(generation).unwrap();
        row["review"] =
            serde_json::to_value(run.review_of(run.task(&dispatch.task).unwrap())).unwrap();
        row["head"] = json!(dispatch.source);
        row["applied"] = json!(false);
    }
    std::fs::write(
        output,
        serde_json::to_string_pretty(
            &json!({"at":now, "claudeFiles":claude.files, "codexFiles":codex.files, "rows":rows}),
        )
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn fresh_summonses_read_the_hosts_table_and_sealed_handovers_keep_their_tuning() {
    let key = ["profile-team", "%1", "profile-request"];
    assert!(
        profile("codex", difficulty::LADDER[0].0, key)
            .unwrap()
            .is_none()
    );
    let root = json!({"smart":{difficulty::PROFILES_SETTING:{"codex":{"low":{"model":"gpt-test-profile","effort":"high"}}}}});
    let fresh = origin_with(key, None, true, root.clone());
    assert_eq!(
        profile("codex", difficulty::LADDER[0].0, key)
            .unwrap()
            .unwrap()
            .model,
        "gpt-test-profile"
    );
    drop(fresh);
    let sealed = origin_with(key, None, false, root);
    assert!(
        profile("codex", difficulty::LADDER[0].0, key)
            .unwrap()
            .is_none()
    );
    drop(sealed);
}
