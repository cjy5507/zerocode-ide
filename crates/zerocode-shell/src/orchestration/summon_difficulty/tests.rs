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

/// A host that keeps the jobs it is handed off the beat until the test runs
/// them, so a test sees what the summons did before any socket opened.
/// A host whose off-the-beat jobs wait to be run by hand — shared with the
/// model seat's tests.
pub(in crate::orchestration) struct Deferred {
    pub(in crate::orchestration) wire: Wire,
    jobs: std::cell::RefCell<Vec<Box<dyn FnOnce() + Send>>>,
}
impl Deferred {
    pub(in crate::orchestration) fn on(wire: &Wire) -> Self {
        Self {
            wire: wire.clone(),
            jobs: Default::default(),
        }
    }
    /// Run every job handed off the beat so far.
    pub(in crate::orchestration) fn drain(&self) {
        let jobs: Vec<_> = self.jobs.borrow_mut().drain(..).collect();
        for job in jobs {
            job();
        }
    }
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

#[test]
fn recording_is_deferred_and_an_acting_receipt_is_not_asked_twice() {
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

/// t-14437: the window's lineup reaches a fresh summons — a model nobody
/// typed launches the difficulty its rung serves — while a person's row
/// stays theirs, and `agent-list`'s rows say "nobody looked" before any read.
#[test]
fn a_fresh_summons_reads_todays_lineup_and_keeps_the_persons_row() {
    use zerocode_core::summon_difficulty::lineup::{Lineup, Seen};
    let key = ["lineup-team", "%1", "lineup-request"];
    let low = difficulty::LADDER[0].0;
    let catalog = json!({"models": [
        {"provider": "claude", "id": "model-a", "band": "second", "rungs": ["hard"]},
        {"provider": "claude", "id": "model-b", "band": "rest", "rungs": ["easy", "medium"]},
    ]});
    let lineup = crate::summon_lineup::Snapshot {
        lineups: std::collections::BTreeMap::from([(
            "claude".to_string(),
            Lineup::from_catalog(&catalog, "claude").unwrap(),
        )]),
        seen: Seen::default(),
    };
    let held = origin_with_lineup_for_tests(key, Value::Null, lineup.clone());
    let row = profile("claude", low, key).unwrap().unwrap();
    assert_eq!(row.model, "model-b", "the lineup's easy rung");
    assert_eq!(row.from, difficulty::lineup::Source::Lineup);
    drop(held);
    let person = json!({"smart":{difficulty::PROFILES_SETTING:{"claude":{low:{"model":"model-x","effort":"high"}}}}});
    let held = origin_with_lineup_for_tests(key, person.clone(), lineup.clone());
    let row = profile("claude", low, key).unwrap().unwrap();
    assert_eq!(
        (row.model.as_str(), row.from),
        ("model-x", difficulty::lineup::Source::Person)
    );
    drop(held);
    assert!(
        rows_in(
            &Value::Null,
            "claude",
            &crate::summon_lineup::Snapshot::default()
        )
        .is_none()
    );
    let listed = rows_in(&person, "claude", &lineup).unwrap();
    assert_eq!(listed.len(), difficulty::LADDER.len());
}

/* ---- the one switch (t-11989) ---- */

/// Settings as a person who turned Jev on from the settings card holds them
/// (§6.1): the switch on, every folder consented, and — unless `word` says
/// one — no word of the difficulty seat's own.
fn switched_on(home: &tempfile::TempDir, endpoint: &Endpoint, word: Option<&str>) -> Wire {
    use zerocode_core::jev::door::{ENABLED_SETTING, EVERY_WORKSPACE, JEV_SETTINGS_KEY};
    let settings = home.path().join("settings.json");
    let mut smart =
        json!({JEV_SETTINGS_KEY: {ENABLED_SETTING: true, "workspaces": [EVERY_WORKSPACE]}});
    if let Some(word) = word {
        smart[SUMMON_DIFFICULTY.setting] = json!(word);
    }
    std::fs::write(&settings, json!({"smart": smart}).to_string()).unwrap();
    Wire::at(&endpoint.base(), "test-key", Some(settings))
}

/// A launcher whose difficulty seat and table are this window's own, asked
/// on `wire` — the roads the live catalog takes, on a socket of the test's.
struct Seated<'a> {
    wire: &'a Wire,
}
impl zerocode_core::orchestration::Launcher for Seated<'_> {
    fn command_for(
        &self,
        _agent: &str,
        _prompt: &str,
        _tuning: &[String],
    ) -> Result<String, String> {
        Ok("claude".into())
    }
    fn choose_assign(
        &self,
        asked: &zerocode_core::summon_assign::AssignAsk,
        origin: [&str; 3],
    ) -> zerocode_core::summon_assign::Receipts {
        super::super::summon_assign::choose_with(self.wire, asked, origin)
    }
    fn choose_agent(
        &self,
        look: &zerocode_core::summon_choice::SummonLook<'_>,
        options: &[zerocode_core::summon_choice::Summonable],
        origin: [&str; 3],
    ) -> Option<String> {
        super::super::summon_choice::choose_with(self.wire, look, options, origin)
    }
    fn difficulty_profile(
        &self,
        agent: &str,
        level: &str,
        origin: [&str; 3],
    ) -> Result<Option<difficulty::lineup::Row>, String> {
        profile(agent, level, origin)
    }
}

/// One `worker-start` of a claude worker with `flags`, planned by the ledger
/// on the seats of `wire` from a coordinator's checkout at `checkout`: the
/// reply, and the reservation the window carries on.
fn summoned(
    wire: &Wire,
    checkout: &Path,
    flags: &str,
    request: &str,
) -> (Value, PreparedWorkerStart) {
    let mut ledger = zerocode_core::orchestration::Ledger::new();
    let mut team = zerocode_core::agent_teams::Team::new("team-switch", "test-token", 1);
    let launcher = Seated { wire };
    // The window's own observation of the summoning pane, under the key
    // the ledger names it by.
    let _origin = origin_with(
        [team.id.as_str(), "%1", request],
        Some(checkout.to_path_buf()),
        true,
        wire.settings_root(),
    );
    let mut reply = Value::Null;
    let mut prepared = None;
    for (at, command) in [
        (
            1,
            "run-create --name switch --retry-request create".to_string(),
        ),
        (
            2,
            format!(
                "worker-start --agent claude --prompt translate {flags} --retry-request {request}"
            ),
        ),
    ] {
        let argv = command
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>();
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
        if planned.prepared_worker_start.is_some() {
            reply = serde_json::from_str(&planned.reply.stdout).unwrap();
            prepared = planned.prepared_worker_start;
        }
    }
    (reply, prepared.expect("a worker was reserved"))
}

/// The row the window wrote for a summons, once its off-the-beat job ran.
fn recorded(wire: &Wire, prepared: &PreparedWorkerStart, checkout: &Path) -> Vec<Value> {
    let host = Deferred::on(wire);
    record(&host, prepared, checkout.to_str(), 3);
    host.drain();
    crate::systemone::read_rows(&crate::systemone::ledger_of(wire, &SUMMON_DIFFICULTY).unwrap())
}

/// With the one switch on and no word for the seat, a summons whose
/// coordinator left `--model` and `--effort` out launches on the profile row
/// of Jev's answer, asked once on the beat; its row says the answer was
/// carried out and is that request's own receipt — asked once, not again off
/// the beat — so what became of the work grades the seat.
#[test]
fn a_switch_left_on_carries_out_the_difficulty_answer_for_an_open_dial() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
    let wire = switched_on(&home, &endpoint, None);
    let (reply, prepared) = summoned(&wire, home.path(), "", "open-dials");
    let low = difficulty::profile(&Value::Null, "claude", difficulty::LADDER[0].0)
        .unwrap()
        .unwrap();
    assert_eq!(reply["model"], json!(low.model), "{reply}");
    assert_eq!(reply["effort"], json!(low.effort), "{reply}");
    assert_eq!(
        endpoint.asked().len(),
        1,
        "asked once, while the summons waited"
    );
    let rows = recorded(&wire, &prepared, home.path());
    assert_eq!(
        endpoint.asked().len(),
        1,
        "the acting receipt is not asked twice"
    );
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["applied"], true, "{row}");
    assert_eq!(row["chosen"], difficulty::LADDER[0].0, "{row}");
    assert_eq!(
        row["mode"],
        zerocode_core::jev::JevMode::Auto.key(),
        "{row}"
    );
    assert_eq!(row["requests"], 1, "{row}");
    assert_eq!(row["executionModel"], json!(low.model), "{row}");
    assert_eq!(row["effort"], json!(low.effort), "{row}");
    assert!(
        row["pinnedEffort"].is_null(),
        "nobody pinned an effort: {row}"
    );
}

/// What the switch leaves alone: a summons whose coordinator named both
/// dials applies no answer — the seat asks off the beat and the row says it
/// was not carried out — and a person's own `off` for the seat asks nothing
/// at all, the summons launching on the middle row as before.
#[test]
fn under_the_switch_a_pin_applies_no_answer_and_a_persons_off_asks_nothing() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
    let wire = switched_on(&home, &endpoint, None);
    let (reply, prepared) = summoned(&wire, home.path(), "--model opus --effort max", "pinned");
    assert_eq!(
        (reply["model"].as_str(), reply["effort"].as_str()),
        (Some("opus"), Some("max"))
    );
    assert!(
        endpoint.asked().is_empty(),
        "a pinned summons waits on no question"
    );
    let rows = recorded(&wire, &prepared, home.path());
    assert_eq!(endpoint.asked().len(), 1, "recorded off the beat");
    assert_eq!(rows[0]["applied"], false, "{}", rows[0]);
    assert_eq!(rows[0]["pinnedEffort"], "max", "{}", rows[0]);

    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer(), 0);
    let wire = switched_on(
        &home,
        &endpoint,
        Some(zerocode_core::jev::JevMode::Off.key()),
    );
    let (reply, prepared) = summoned(&wire, home.path(), "", "off");
    let mid = difficulty::profile(&Value::Null, "claude", difficulty::FALLBACK_DIFFICULTY)
        .unwrap()
        .unwrap();
    assert_eq!(reply["model"], json!(mid.model), "{reply}");
    assert_eq!(reply["effort"], json!(mid.effort), "{reply}");
    assert!(recorded(&wire, &prepared, home.path()).is_empty());
    assert!(endpoint.asked().is_empty(), "a person's off sends nothing");
}

/// An answer that does not come back inside the seat's two seconds is no
/// answer: the summons launches on the middle row within the wall, and the
/// row says nothing was carried out.
#[test]
fn under_the_switch_a_late_answer_falls_back_to_the_middle_row_within_the_wall() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Endpoint::serving(
        "HTTP/1.1 200 OK",
        answer(),
        difficulty::APPLY_DEADLINE_MS + 500,
    );
    let wire = switched_on(&home, &endpoint, None);
    let began = Instant::now();
    let (reply, prepared) = summoned(&wire, home.path(), "", "late");
    assert!(began.elapsed() < Duration::from_millis(difficulty::APPLY_DEADLINE_MS + 400));
    let mid = difficulty::profile(&Value::Null, "claude", difficulty::FALLBACK_DIFFICULTY)
        .unwrap()
        .unwrap();
    assert_eq!(reply["model"], json!(mid.model), "{reply}");
    assert_eq!(reply["effort"], json!(mid.effort), "{reply}");
    let receipt = prepared
        .difficulty_shadow
        .as_ref()
        .and_then(|shadow| shadow.receipt.clone())
        .expect("the request that was waited on is the row's own");
    assert_eq!(receipt["applied"], false, "{receipt}");
    assert!(receipt.get("chosen").is_none(), "{receipt}");
}

/// One task summoned, reported with its head and landed by the coordinator
/// as verified and merged — a first attempt that succeeded — in `ledger`'s
/// run: the dispatch it was carried on.
fn landed_attempt(
    ledger: &mut zerocode_core::orchestration::Ledger,
    team: &mut zerocode_core::agent_teams::Team,
    n: i64,
) -> String {
    let mut at = n * 10;
    let mut planned = |team: &mut zerocode_core::agent_teams::Team,
                       ledger: &mut zerocode_core::orchestration::Ledger,
                       pane: &str,
                       actor: &str,
                       argv: Vec<String>| {
        at += 1;
        let planned =
            zerocode_core::orchestration::plan(ledger, team, &Launch, &argv, pane, at, Some(actor));
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        ledger.file_receipt(&planned, at);
        planned
    };
    let words = |line: String| {
        line.split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let task = planned(
        team,
        ledger,
        "%1",
        "test-actor",
        words(format!(
            "task-create --spec translate-{n} --retry-request task-{n}"
        )),
    );
    let task: Value = serde_json::from_str(&task.reply.stdout).unwrap();
    let task = task["taskId"].as_str().unwrap().to_string();
    let started = planned(
        team,
        ledger,
        "%1",
        "test-actor",
        words(format!(
            "worker-start --agent claude --task {task} --prompt translate --retry-request start-{n}"
        )),
    );
    let zerocode_core::agent_teams::Effect::Split {
        pane,
        from,
        direction,
        ..
    } = started.effect.clone()
    else {
        panic!("no split: {:?}", started.effect);
    };
    team.record_split(&pane, u32::try_from(100 + n).unwrap(), &from, direction);
    let dispatch = started
        .prepared_worker_start
        .as_ref()
        .and_then(|prepared| prepared.dispatch.clone())
        .expect("a dispatch");
    planned(
        team,
        ledger,
        &pane,
        "worker-actor",
        vec![
            "send".into(),
            "--type".into(),
            "worker_done".into(),
            "--body".into(),
            r#"{"ok":true,"head":"abc1234"}"#.into(),
            "--retry-request".into(),
            format!("done-{n}"),
        ],
    );
    planned(
        team,
        ledger,
        "%1",
        "test-actor",
        vec![
            "task-update".into(),
            "--task".into(),
            task,
            "--result".into(),
            r#"{"verified":true,"merged":true}"#.into(),
            "--attempt".into(),
            dispatch.clone(),
            "--source".into(),
            "abc1234".into(),
            "--retry-request".into(),
            format!("land-{n}"),
        ],
    );
    dispatch
}

/// What became of a summons' work grades the seat only where the seat's
/// answer was carried out: an applied request's first attempt is its
/// `agreed` mark, and a request whose answer was not applied — a pin ran
/// instead — grades the pin, as the baseline's mark, never the seat.
#[test]
fn a_carried_out_answers_first_attempt_is_the_seats_mark() {
    let mut ledger = zerocode_core::orchestration::Ledger::new();
    let mut team = zerocode_core::agent_teams::Team::new("team-marks", "test-token", 1);
    let created = zerocode_core::orchestration::plan(
        &mut ledger,
        &mut team,
        &Launch,
        &["run-create", "--name", "marks", "--retry-request", "create"].map(str::to_string),
        "%1",
        1,
        Some("test-actor"),
    );
    assert_eq!(created.reply.exit_code, 0, "{}", created.reply.stderr);
    ledger.file_receipt(&created, 1);
    let run = ledger.runs()[0].id.clone();
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(SUMMON_DIFFICULTY.ledger);
    let mut requests = String::new();
    let mut dispatches = Vec::new();
    for (n, applied) in [(1, true), (2, false)] {
        let dispatch = landed_attempt(&mut ledger, &mut team, n);
        requests.push_str(
            &json!({"at": n, "requestAt": n, "run": run, "dispatch": dispatch,
                "rubricVersion": difficulty::RUBRIC_VERSION, "outcome": "answered",
                "chosen": difficulty::LADDER[0].0, "agent": "claude", "applied": applied,
                "attempt": 0, "retryOf": false})
            .to_string(),
        );
        requests.push('\n');
        dispatches.push((dispatch, applied));
    }
    std::fs::write(&path, requests).unwrap();
    let mut costs = super::super::cost_book::CostBook::default();
    let (_, observed) = observations_at(path, &ledger, &mut costs).expect("a ledger to read");
    assert_eq!(observed.len(), 2, "{observed:?}");
    for (dispatch, applied) in dispatches {
        let row = observed
            .iter()
            .find(|row| row["dispatch"] == dispatch.as_str())
            .expect("an observation of each request");
        assert_eq!(
            row[difficulty::outcomes::KEY]["firstAttemptSuccess"],
            true,
            "{row}"
        );
        let (mark, other) = if applied {
            (
                zerocode_core::jev::summary::AGREED,
                zerocode_core::jev::summary::BASELINE_AGREED,
            )
        } else {
            (
                zerocode_core::jev::summary::BASELINE_AGREED,
                zerocode_core::jev::summary::AGREED,
            )
        };
        assert_eq!(row[mark.canonical], true, "{row}");
        assert!(row.get(other.canonical).is_none(), "{row}");
    }
}

/// The live check t-11989 hands in, on the real Jev service: with the one
/// switch on and no word of either seat's own, a summons whose coordinator
/// left both dials out launches on the profile row of Jev's real answer and
/// its row says the answer was carried out; and a summons typed
/// `--agent auto` lands on the agent the summon question really chose. It
/// plans through the ledger's own `worker-start` on a ledger of its own, under
/// a zo home of its own — the settings, the day's count and the seats' ledgers
/// are that home's — and opens no pane, so the person's files and screen are
/// untouched. The key rides one command's environment and is never printed.
///
/// ```sh
/// TYPESAFE_API_KEY="$(security find-generic-password \
///     -s dev.zerocode.key.TYPESAFE_API_KEY -a "$USER" -w)" \
///   cargo test -p zerocode-shell --bin zerocode-shell \
///   orchestration::summon_difficulty::tests::live_the_switch_carries_out_real_answers \
///   -- --ignored --nocapture
/// ```
#[test]
#[ignore = "live Jev requests; requires a command-scoped key"]
fn live_the_switch_carries_out_real_answers() {
    use zerocode_core::jev::door::{ENABLED_SETTING, EVERY_WORKSPACE, JEV_SETTINGS_KEY};
    let key = std::env::var("TYPESAFE_API_KEY").expect("a command-scoped key");
    let home = tempfile::tempdir().unwrap();
    let settings = home.path().join("settings.json");
    std::fs::write(
        &settings,
        json!({"smart": {JEV_SETTINGS_KEY: {ENABLED_SETTING: true, "workspaces": [EVERY_WORKSPACE]}}})
            .to_string(),
    )
    .unwrap();
    let wire = Wire::at(crate::systemone::SYSTEMONE_BASE_URL, &key, Some(settings));
    /// The seats of [`Seated`], and what this machine has on its `PATH` — the
    /// set the summon question chooses among, as the live catalog measures it.
    struct OnThisMachine<'a>(Seated<'a>);
    impl zerocode_core::orchestration::Launcher for OnThisMachine<'_> {
        fn command_for(
            &self,
            agent: &str,
            prompt: &str,
            tuning: &[String],
        ) -> Result<String, String> {
            self.0.command_for(agent, prompt, tuning)
        }
        fn choose_agent(
            &self,
            look: &zerocode_core::summon_choice::SummonLook<'_>,
            options: &[zerocode_core::summon_choice::Summonable],
            origin: [&str; 3],
        ) -> Option<String> {
            self.0.choose_agent(look, options, origin)
        }
        fn choose_assign(
            &self,
            asked: &zerocode_core::summon_assign::AssignAsk,
            origin: [&str; 3],
        ) -> zerocode_core::summon_assign::Receipts {
            self.0.choose_assign(asked, origin)
        }
        fn difficulty_profile(
            &self,
            agent: &str,
            level: &str,
            origin: [&str; 3],
        ) -> Result<Option<difficulty::lineup::Row>, String> {
            self.0.difficulty_profile(agent, level, origin)
        }
        fn presence(&self) -> Option<Vec<zerocode_core::agent::AgentPresence>> {
            Some(zerocode_core::agent::agent_presence(
                std::env::var_os("PATH").as_deref(),
                std::env::consts::OS,
            ))
        }
    }
    let launcher = OnThisMachine(Seated { wire: &wire });
    let mut ledger = zerocode_core::orchestration::Ledger::new();
    let mut team = zerocode_core::agent_teams::Team::new("team-live", "test-token", 1);
    let mut summon = |at: i64, argv: Vec<String>| {
        let request = argv.last().cloned().unwrap_or_default();
        let _origin = origin_with(
            [team.id.as_str(), "%1", request.as_str()],
            Some(home.path().to_path_buf()),
            true,
            wire.settings_root(),
        );
        let planned = zerocode_core::orchestration::plan(
            &mut ledger,
            &mut team,
            &launcher,
            &argv,
            "%1",
            at,
            Some("test-actor"),
        );
        ledger.file_receipt(&planned, at);
        planned
    };
    let created = summon(
        1,
        [
            "run-create",
            "--name",
            "live-switch",
            "--retry-request",
            "create",
        ]
        .map(str::to_string)
        .to_vec(),
    );
    assert_eq!(created.reply.exit_code, 0, "{}", created.reply.stderr);

    // The difficulty: a small documentation fix, both dials left out.
    let started = summon(
        2,
        [
            "worker-start",
            "--agent",
            "claude",
            "--prompt",
            "Fix the typo in the README install section: 'recieve' should read 'receive'. Change nothing else.",
            "--retry-request",
            "live-difficulty",
        ]
        .map(str::to_string)
        .to_vec(),
    );
    assert_eq!(started.reply.exit_code, 0, "{}", started.reply.stderr);
    let reply: Value = serde_json::from_str(&started.reply.stdout).unwrap();
    let prepared = started
        .prepared_worker_start
        .expect("a worker was reserved");
    let rows = recorded(&wire, &prepared, home.path());
    let row = rows.first().expect("the summons' row");
    let row_of = |level: &str| {
        difficulty::profile(&Value::Null, "claude", level)
            .unwrap()
            .unwrap()
    };
    println!(
        "difficulty: outcome={} chosen={} confidence={} applied={} requests={} elapsedMs={} -> launched model={} effort={}",
        row["outcome"],
        row["chosen"],
        row["confidence"],
        row["applied"],
        row["requests"],
        row["elapsedMs"],
        reply["model"],
        reply["effort"]
    );
    if let Some(level) = row["chosen"].as_str() {
        let profile = row_of(level);
        assert_eq!(row["applied"], true, "{row}");
        assert_eq!(reply["model"], json!(profile.model), "{reply}");
        assert_eq!(reply["effort"], json!(profile.effort), "{reply}");
        println!(
            "difficulty: the launch is the {level} row ({} / {})",
            profile.model, profile.effort
        );
    } else {
        let mid = row_of(difficulty::FALLBACK_DIFFICULTY);
        assert_eq!(row["applied"], false, "{row}");
        assert_eq!(reply["model"], json!(mid.model), "{reply}");
        println!("difficulty: no answer inside the wall, so the middle row");
    }

    // The agent: `--agent auto`, and the seat's pick is the summons' agent.
    let auto = summon(
        3,
        [
            "worker-start",
            "--agent",
            "auto",
            "--prompt",
            "Write unit tests for the date parser's leap-year branch.",
            "--retry-request",
            "live-agent",
        ]
        .map(str::to_string)
        .to_vec(),
    );
    println!(
        "agent auto: exit={} agent={} stderr={}",
        auto.reply.exit_code,
        serde_json::from_str::<Value>(&auto.reply.stdout)
            .map_or(Value::Null, |reply| reply["agent"].clone()),
        auto.reply.stderr.trim()
    );
    assert_eq!(auto.reply.exit_code, 0, "{}", auto.reply.stderr);
    let shadow = auto
        .prepared_worker_start
        .as_ref()
        .and_then(|prepared| prepared.summon_shadow.as_ref())
        .expect("the summons' judgment half");
    assert!(shadow.auto, "the receipt says the seat chose");
    assert_ne!(
        shadow.pinned.agent,
        zerocode_core::orchestration::SUMMON_AUTO_AGENT
    );
}
