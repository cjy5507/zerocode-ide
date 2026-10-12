//! Native trust-boundary cases. These fixtures attest observations; they do not run a gate.
use super::*;
use crate::orchestration::gate_reuse as gate;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn inputs() -> Value {
    let digest = "b".repeat(64);
    json!({"version":1,"gate":"gate-root","head":HEAD,"tree":"c".repeat(40),
        "source":digest,"deps":digest,"lock":digest,"script":digest,"recipe":digest,
        "toolchain":digest,"platform":digest,"profile":digest,"env":digest,"scope":"deterministic"})
}

struct Fixture {
    bench: Bench,
    task: String,
    attempt: String,
    worker_pane: String,
}

impl Fixture {
    fn new() -> Self {
        let mut bench = Bench::new();
        bench.json("run-create --name gates");
        let task = bench.json("task-create --spec one-gate")["taskId"]
            .as_str()
            .unwrap()
            .to_string();
        let (_, worker_pane) = bench.seat(&format!("worker-start --agent codex --task {task}"));
        bench.json_at(
            &worker_pane,
            &format!("send --type worker_done --body {{\"ok\":true,\"head\":\"{HEAD}\"}}"),
        );
        let attempt = bench.ledger.runs()[0]
            .newest_attempt(&task)
            .unwrap()
            .id
            .clone();
        Self {
            bench,
            task,
            attempt,
            worker_pane,
        }
    }

    fn call(&mut self, verb: &str, value: &Value) -> Decided {
        self.bench.at_argv(
            "%1",
            vec![
                verb.into(),
                "--task".into(),
                self.task.clone(),
                "--attempt".into(),
                self.attempt.clone(),
                "--body".into(),
                value.to_string(),
            ],
        )
    }

    fn json(&mut self, verb: &str, value: &Value) -> Value {
        let planned = self.call(verb, value);
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        serde_json::from_str(&planned.reply.stdout).unwrap()
    }

    fn registered(&mut self, inputs: Value) -> Value {
        let registration = self.json(gate::REGISTER, &inputs);
        let started = self.record_action("--start", &registration, Some(&inputs));
        json!({"version":1,"registration":registration["registration"],"attempt":self.attempt,
            "lease":registration["lease"],"execution":started["execution"],
            "inputs":inputs,"startedMs":started["startedMs"],"endedMs":started["startedMs"],
            "durationMs":0,"rc":0,"outcome":"passed","tests":2,"logSha256":"d".repeat(64),
            "logBytes":123,"inputsUnchanged":true,"executed":true})
    }

    fn record_action(&mut self, action: &str, registered: &Value, body: Option<&Value>) -> Value {
        let mut argv = vec![
            gate::RECORD.into(),
            action.into(),
            registered["registration"].as_str().unwrap().into(),
            "--lease".into(),
            registered["lease"].as_str().unwrap().into(),
            "--task".into(),
            self.task.clone(),
            "--attempt".into(),
            self.attempt.clone(),
        ];
        if let Some(body) = body {
            argv.extend(["--body".into(), body.to_string()]);
        }
        let planned = self.bench.at_argv("%1", argv);
        assert_eq!(planned.reply.exit_code, 0, "{}", planned.reply.stderr);
        serde_json::from_str(&planned.reply.stdout).unwrap()
    }

    fn completed(&mut self) -> (Value, Value) {
        let metadata = self.registered(inputs());
        let record = self.json(gate::RECORD, &metadata);
        (metadata, record)
    }
}

fn request(metadata: &Value) -> Value {
    json!({"version":1,"registration":metadata["registration"],"inputs":metadata["inputs"],
        "lease":metadata["lease"],
        "metadataSha256":format!("{:x}",Sha256::digest(metadata.to_string().as_bytes())),
        "logSha256":metadata["logSha256"],"logBytes":metadata["logBytes"],"tests":metadata["tests"]})
}

#[test]
fn native_evidence_survives_rebuild_and_reuse_is_not_a_second_execution() {
    let mut fixture = Fixture::new();
    let (metadata, record) = fixture.completed();
    fixture.bench.ledger =
        Ledger::rebuild(fixture.bench.ledger.export()).expect("native evidence reloads");
    let answer = fixture.json(gate::REUSE, &request(&metadata));
    assert_eq!(answer["decision"], "hit", "{answer}");
    assert_eq!(answer["original"], record["original"]);
    assert_eq!(answer["executed"], false);
    assert_eq!(answer["newExecutions"], 0);
    assert_eq!(
        answer["mandatoryChecks"],
        json!(["windows", "flake-judgment", "publish"])
    );
    let run = &fixture.bench.ledger.runs()[0];
    assert_eq!(run.dispatches.len(), 1);
    assert_eq!(
        run.messages
            .iter()
            .filter(|row| row.kind == MessageKind::LandCheck
                && row.dispatch.as_deref() == Some(fixture.attempt.as_str()))
            .count(),
        1
    );
    let decision = run
        .messages
        .iter()
        .find(|row| row.id == answer["decisionRecord"].as_str().unwrap())
        .unwrap();
    assert_eq!(decision.from, LEDGER_ITSELF);
    assert!(decision.payload.as_str().contains("authenticated inputs"));
}

#[test]
fn changing_or_forgetting_any_input_dimension_misses() {
    let mut fixture = Fixture::new();
    let (metadata, _) = fixture.completed();
    for key in [
        "source",
        "deps",
        "lock",
        "script",
        "recipe",
        "toolchain",
        "platform",
        "profile",
        "env",
        "tree",
    ] {
        for changed in [
            Value::Null,
            json!("e".repeat(if key == "tree" { 40 } else { 64 })),
        ] {
            let mut observed = request(&metadata);
            observed["inputs"][key] = changed;
            let answer = fixture.json(gate::REUSE, &observed);
            assert_eq!(answer["decision"], "miss", "{key}: {answer}");
            assert_eq!(answer["newExecutions"], 0);
        }
    }
}

#[test]
fn a_different_head_with_the_same_tree_misses() {
    let mut fixture = Fixture::new();
    let (metadata, _) = fixture.completed();
    let mut observed = request(&metadata);
    observed["inputs"]["head"] = "f".repeat(40).into();
    let answer = fixture.json(gate::REUSE, &observed);
    assert_eq!(answer["decision"], "miss", "{answer}");
    assert!(answer["reason"].as_str().unwrap().contains("HEAD"));
}

#[test]
fn every_unknown_registered_identity_misses_even_when_the_consumer_repeats_it() {
    for key in [
        "source",
        "deps",
        "lock",
        "script",
        "recipe",
        "toolchain",
        "platform",
        "profile",
        "env",
        "tree",
    ] {
        let mut fixture = Fixture::new();
        let mut input = inputs();
        input[key] = Value::Null;
        let metadata = fixture.registered(input);
        fixture.json(gate::RECORD, &metadata);
        let answer = fixture.json(gate::REUSE, &request(&metadata));
        assert_eq!(answer["decision"], "miss", "{key}: {answer}");
        assert!(answer["reason"].as_str().unwrap().contains("unknown"));
    }
}

#[test]
fn unknown_measured_and_nondeterministic_gates_never_reuse() {
    for scope in ["unknown", "measurement", "nondeterministic"] {
        let mut fixture = Fixture::new();
        let mut input = inputs();
        input["scope"] = scope.into();
        let metadata = fixture.registered(input);
        fixture.json(gate::RECORD, &metadata);
        assert_eq!(
            fixture.json(gate::REUSE, &request(&metadata))["decision"],
            "miss"
        );
    }
}

#[test]
fn log_bytes_and_all_metadata_bytes_are_bound_to_the_native_completion() {
    let mut fixture = Fixture::new();
    let (metadata, record) = fixture.completed();
    for (field, value) in [
        ("metadataSha256", json!("e".repeat(64))),
        ("logSha256", json!("e".repeat(64))),
        ("logBytes", json!(124)),
        ("tests", json!(3)),
    ] {
        let mut observed = request(&metadata);
        observed[field] = value;
        let answer = fixture.json(gate::REUSE, &observed);
        assert_eq!(answer["decision"], "miss", "{field}: {answer}");
        assert_eq!(answer["original"], record["original"]);
    }
    // Even harmless whitespace changes the byte digest of the complete metadata object.
    let mut observed = request(&metadata);
    observed["metadataSha256"] =
        format!("{:x}", Sha256::digest(format!("{metadata}\n").as_bytes())).into();
    assert_eq!(fixture.json(gate::REUSE, &observed)["decision"], "miss");
}

#[test]
fn failures_cancellation_zero_tests_and_changed_inputs_never_reuse() {
    for (field, value) in [
        ("rc", json!(1)),
        ("rc", Value::Null),
        ("outcome", json!("failed")),
        ("outcome", json!("cancelled")),
        ("outcome", json!("timed_out")),
        ("outcome", json!("unknown")),
        ("tests", json!(0)),
        ("inputsUnchanged", json!(false)),
        ("logBytes", json!(0)),
    ] {
        let mut fixture = Fixture::new();
        let mut metadata = fixture.registered(inputs());
        metadata[field] = value;
        fixture.json(gate::RECORD, &metadata);
        let answer = fixture.json(gate::REUSE, &request(&metadata));
        assert_eq!(answer["decision"], "miss", "{field}: {answer}");
    }
}

#[test]
fn an_execution_has_one_immutable_completion() {
    let mut fixture = Fixture::new();
    let (mut metadata, original) = fixture.completed();
    assert_eq!(
        fixture.json(gate::RECORD, &metadata)["original"],
        original["original"]
    );
    let before = fixture.bench.ledger.runs()[0].messages.len();
    metadata["durationMs"] = json!(1);
    let refused = fixture.call(gate::RECORD, &metadata);
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("immutable"));
    assert_eq!(fixture.bench.ledger.runs()[0].messages.len(), before);
}

#[test]
fn registration_and_completion_are_bound_to_the_current_successful_attempt() {
    let mut fixture = Fixture::new();
    let (metadata, _) = fixture.completed();
    fixture.attempt = "dp-other".into();
    assert_eq!(
        fixture.json(gate::REUSE, &request(&metadata))["decision"],
        "miss"
    );
    fixture.attempt = fixture.bench.ledger.runs()[0].dispatches[0].id.clone();
    fixture.bench.ledger.runs[0].dispatches[0].succeeded = Some(false);
    assert_eq!(
        fixture.json(gate::REUSE, &request(&metadata))["decision"],
        "miss"
    );
    let refused = fixture.call(gate::REGISTER, &inputs());
    assert_ne!(refused.reply.exit_code, 0);
}

#[test]
fn event_order_uses_ledger_registration_instead_of_git_tree_time() {
    for field in ["startedMs", "endedMs"] {
        let mut fixture = Fixture::new();
        let mut metadata = fixture.registered(inputs());
        metadata[field] = if field == "startedMs" {
            json!(0)
        } else {
            json!(9_999_999)
        };
        let refused = fixture.call(gate::RECORD, &metadata);
        assert_ne!(refused.reply.exit_code, 0, "{field}");
        assert!(refused.reply.stderr.contains("event order"));
    }
    let mut fixture = Fixture::new();
    let (metadata, _) = fixture.completed();
    fixture.bench.clock = 900;
    let answer = fixture.json(gate::REUSE, &request(&metadata));
    assert_eq!(answer["decision"], "miss");
    assert!(answer["reason"].as_str().unwrap().contains("precedes"));
}

#[test]
fn a_coordinator_change_interrupts_an_unfinished_registration() {
    let mut fixture = Fixture::new();
    let metadata = fixture.registered(inputs());
    fixture.bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .unwrap()
        .generation += 1;
    let refused = fixture.call(gate::RECORD, &metadata);
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("coordinator changed"));
}

#[test]
fn a_completed_receipt_misses_after_a_coordinator_generation_change() {
    let mut fixture = Fixture::new();
    let (metadata, _) = fixture.completed();
    fixture.bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .unwrap()
        .generation += 1;
    let answer = fixture.json(gate::REUSE, &request(&metadata));
    assert_eq!(
        answer["decision"], "miss",
        "old generation reused: {answer}"
    );
}

#[test]
fn an_unrelated_candidate_without_source_ancestry_is_refused() {
    let mut fixture = Fixture::new();
    let mut input = inputs();
    input["head"] = "f".repeat(40).into();
    input.as_object_mut().unwrap().remove("handedInHead");
    input.as_object_mut().unwrap().remove("sourceAncestor");
    let refused = fixture.call(gate::REGISTER, &input);
    assert_ne!(refused.reply.exit_code, 0, "unrelated source registered");
}

#[test]
fn a_reuse_decision_records_candidate_source_and_recipe_provenance() {
    let mut fixture = Fixture::new();
    let (metadata, _) = fixture.completed();
    let answer = fixture.json(gate::REUSE, &request(&metadata));
    assert_eq!(answer["head"], metadata["inputs"]["head"]);
    assert_eq!(answer["tree"], metadata["inputs"]["tree"]);
    assert_eq!(answer["handedInHead"], HEAD);
    assert_eq!(answer["recipe"], metadata["inputs"]["recipe"]);
}

#[test]
fn gate_completions_do_not_count_as_legacy_land_check_executions() {
    let mut fixture = Fixture::new();
    fixture.completed();
    let run = &fixture.bench.ledger.runs()[0];
    let attempt = run.newest_attempt(&fixture.task).unwrap();
    let time = crate::orchestration::attempt_time::attempt_time(run, attempt);
    assert!(
        !time.events.contains_key("land_check"),
        "gate completion counted as a merge check"
    );
}

#[test]
fn a_windows_process_failure_code_is_recorded_without_losing_the_failure() {
    let mut fixture = Fixture::new();
    let mut metadata = fixture.registered(inputs());
    metadata["rc"] = json!(3_221_225_477_u64);
    metadata["outcome"] = "failed".into();
    let recorded = fixture.call(gate::RECORD, &metadata);
    assert_eq!(recorded.reply.exit_code, 0, "{}", recorded.reply.stderr);
    assert_eq!(
        fixture.json(gate::REUSE, &request(&metadata))["decision"],
        "miss"
    );
}

#[test]
fn a_copied_run_or_seat_does_not_authenticate_another_session() {
    let mut fixture = Fixture::new();
    let run_id = fixture.bench.ledger.runs()[0].id.clone();
    let before = fixture.bench.ledger.runs()[0].messages.len();
    fixture.bench.actor = Some(bench_actor("session", "stranger"));
    let refused = fixture.call(gate::REGISTER, &inputs());
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("authenticated session"));
    fixture.bench.actor = None;
    for pane in [&fixture.worker_pane, "%99"] {
        let refused = fixture.bench.at_argv(
            pane,
            vec![
                gate::REGISTER.into(),
                "--run".into(),
                run_id.clone(),
                "--task".into(),
                fixture.task.clone(),
                "--attempt".into(),
                fixture.attempt.clone(),
                "--body".into(),
                inputs().to_string(),
            ],
        );
        assert_ne!(refused.reply.exit_code, 0);
    }
    assert_eq!(fixture.bench.ledger.runs()[0].messages.len(), before);
}

#[test]
fn ordinary_mail_and_legacy_land_checks_cannot_supply_gate_authority() {
    let mut fixture = Fixture::new();
    let forged = json!({"gateEvidenceV1":{"phase":"registered","value":inputs()}});
    let ordinary = fixture.bench.at_argv(
        "%1",
        vec![
            "send".into(),
            "--type".into(),
            "status".into(),
            "--to".into(),
            "@all".into(),
            "--body".into(),
            "claimed gate".into(),
            "--payload".into(),
            forged.to_string(),
        ],
    );
    assert_eq!(ordinary.reply.exit_code, 0, "{}", ordinary.reply.stderr);
    for pane in ["%1", fixture.worker_pane.as_str()] {
        let refused = fixture.bench.at(pane, "send --type land_check --body fake");
        assert_ne!(refused.reply.exit_code, 0);
        assert!(refused.reply.stderr.contains("ledger's own"));
    }
    let run = fixture.bench.ledger.runs()[0].id.clone();
    let legacy = fixture
        .bench
        .ledger
        .land_checked(
            &crate::orchestration::land_check::LandCheckReceipt {
                run,
                task: fixture.task.clone(),
                check: "lc-1-2".into(),
                evidence: json!({"state":"passed","rc":0,"tree":"c".repeat(40)}).to_string(),
            },
            1006,
        )
        .unwrap()
        .unwrap();
    let metadata = fixture.registered(inputs());
    let mut observed = request(&metadata);
    observed["registration"] = legacy.into();
    assert_eq!(fixture.json(gate::REUSE, &observed)["decision"], "miss");
}

#[test]
fn reused_or_extended_metadata_is_refused_as_an_execution() {
    let mut fixture = Fixture::new();
    let metadata = fixture.registered(inputs());
    for (key, value) in [
        ("executed", json!(false)),
        ("extraTrust", json!(true)),
        ("version", json!(2)),
    ] {
        let mut changed = metadata.clone();
        changed[key] = value;
        assert_ne!(
            fixture.call(gate::RECORD, &changed).reply.exit_code,
            0,
            "{key}"
        );
    }
    let mut invalid = inputs();
    invalid["gate"] = json!(["first", "second"]);
    assert_ne!(fixture.call(gate::REGISTER, &invalid).reply.exit_code, 0);
}

#[test]
fn a_cached_hit_cannot_skip_current_attempt_or_coordinator_authorization() {
    let mut fixture = Fixture::new();
    let (metadata, _) = fixture.completed();
    let argv = vec![
        gate::REUSE.into(),
        "--task".into(),
        fixture.task.clone(),
        "--attempt".into(),
        fixture.attempt.clone(),
        "--body".into(),
        request(&metadata).to_string(),
        RETRY_REQUEST.into(),
        "same-gate-reuse-request".into(),
    ];
    let first = fixture.bench.at_argv("%1", argv.clone());
    assert_eq!(first.reply.exit_code, 0, "{}", first.reply.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&first.reply.stdout).unwrap()["decision"],
        "hit"
    );
    let same = fixture.bench.at_argv("%1", argv.clone());
    assert_eq!(same.reply.stdout, first.reply.stdout);
    let before = fixture.bench.ledger.runs()[0].messages.len();
    fixture.bench.ledger.runs[0].dispatches[0].succeeded = Some(false);
    let refused = fixture.bench.at_argv("%1", argv.clone());
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("no longer establishes"));
    fixture.bench.ledger.runs[0].dispatches[0].succeeded = Some(true);
    fixture.bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .unwrap()
        .vacated_ms = Some(fixture.bench.clock);
    let refused = fixture.bench.at_argv("%1", argv);
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("live coordinator"));
    assert_eq!(fixture.bench.ledger.runs()[0].messages.len(), before);
}

#[test]
fn a_replayed_registration_cannot_start_work_after_the_coordinator_generation_changes() {
    let mut fixture = Fixture::new();
    let argv = vec![
        gate::REGISTER.into(),
        "--task".into(),
        fixture.task.clone(),
        "--attempt".into(),
        fixture.attempt.clone(),
        "--body".into(),
        inputs().to_string(),
        RETRY_REQUEST.into(),
        "same-gate-registration".into(),
    ];
    let first = fixture.bench.at_argv("%1", argv.clone());
    assert_eq!(first.reply.exit_code, 0, "{}", first.reply.stderr);
    fixture.bench.ledger.runs[0]
        .coordinator
        .as_mut()
        .unwrap()
        .generation += 1;
    let refused = fixture.bench.at_argv("%1", argv);
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("registration event changed"));
}

#[test]
fn a_merge_candidate_and_the_handed_in_source_are_bound_as_distinct_heads() {
    let mut fixture = Fixture::new();
    let mut input = inputs();
    input["head"] = "f".repeat(40).into();
    let metadata = fixture.registered(input);
    fixture.json(gate::RECORD, &metadata);
    assert_eq!(
        fixture.json(gate::REUSE, &request(&metadata))["decision"],
        "hit"
    );
    let registered = &fixture.bench.ledger.runs()[0]
        .messages
        .iter()
        .find(|row| row.id == metadata["registration"].as_str().unwrap())
        .unwrap();
    let stored: Value = serde_json::from_str(registered.payload.as_str()).unwrap();
    assert_eq!(stored["gateEvidenceV1"]["value"]["handedInHead"], HEAD);
    assert_eq!(
        stored["gateEvidenceV1"]["value"]["inputs"]["head"],
        "f".repeat(40)
    );
    fixture.bench.ledger.runs[0].dispatches[0].source = Some("e".repeat(40));
    assert_eq!(
        fixture.json(gate::REUSE, &request(&metadata))["decision"],
        "miss"
    );
}

#[test]
fn a_coordinators_direct_task_has_an_explicit_no_attempt_execution() {
    let mut bench = Bench::new();
    bench.json("run-create --name direct");
    let task = bench.json("task-create --spec direct-gate")["taskId"]
        .as_str()
        .unwrap()
        .to_string();
    let mut fixture = Fixture {
        bench,
        task,
        attempt: NO_ATTEMPT.into(),
        worker_pane: String::new(),
    };
    let (metadata, _) = fixture.completed();
    assert_eq!(
        fixture.json(gate::REUSE, &request(&metadata))["decision"],
        "hit"
    );
    assert!(fixture.bench.ledger.runs()[0].dispatches.is_empty());
    fixture.bench.seat(&format!(
        "worker-start --agent codex --task {}",
        fixture.task
    ));
    assert_eq!(
        fixture.json(gate::REUSE, &request(&metadata))["decision"],
        "miss"
    );
}

#[test]
fn a_lease_start_is_single_use_including_replays_and_competing_requests() {
    let mut fixture = Fixture::new();
    let registration = fixture.json(gate::REGISTER, &inputs());
    let mut argv = vec![
        gate::RECORD.into(),
        "--task".into(),
        fixture.task.clone(),
        "--attempt".into(),
        fixture.attempt.clone(),
        "--start".into(),
        registration["registration"].as_str().unwrap().into(),
        "--lease".into(),
        registration["lease"].as_str().unwrap().into(),
        "--body".into(),
        inputs().to_string(),
        RETRY_REQUEST.into(),
        "lease-start-one".into(),
    ];
    let first = fixture.bench.at_argv("%1", argv.clone());
    assert_eq!(first.reply.exit_code, 0, "{}", first.reply.stderr);
    let replay = fixture.bench.at_argv("%1", argv.clone());
    assert_ne!(replay.reply.exit_code, 0);
    assert!(replay.reply.stderr.contains("cannot be replayed"));
    *argv.last_mut().unwrap() = "competing-lease-start".into();
    let other = fixture.bench.at_argv("%1", argv);
    assert_ne!(other.reply.exit_code, 0);
    assert!(other.reply.stderr.contains("already started"));
}

#[test]
fn cancelled_or_expired_leases_cannot_complete_or_reuse() {
    let mut fixture = Fixture::new();
    let (metadata, original) = fixture.completed();
    fixture.record_action("--cancel", &metadata, None);
    let answer = fixture.json(gate::REUSE, &request(&metadata));
    assert_eq!(answer["decision"], "miss");
    assert_eq!(answer["original"], original["original"]);
    assert!(answer["reason"].as_str().unwrap().contains("cancelled"));
    assert_ne!(fixture.call(gate::RECORD, &metadata).reply.exit_code, 0);

    let mut fixture = Fixture::new();
    let metadata = fixture.registered(inputs());
    fixture.bench.clock += 2 * 60 * 60 * 1000;
    let refused = fixture.call(gate::RECORD, &metadata);
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("expired"));
}

#[test]
fn an_expired_registration_never_authorizes_an_execution_start() {
    let mut fixture = Fixture::new();
    let registration = fixture.json(gate::REGISTER, &inputs());
    fixture.bench.clock = registration["expiresMs"].as_i64().unwrap();
    let refused = fixture.bench.at_argv(
        "%1",
        vec![
            gate::RECORD.into(),
            "--task".into(),
            fixture.task.clone(),
            "--attempt".into(),
            fixture.attempt.clone(),
            "--start".into(),
            registration["registration"].as_str().unwrap().into(),
            "--lease".into(),
            registration["lease"].as_str().unwrap().into(),
            "--body".into(),
            inputs().to_string(),
        ],
    );
    assert_ne!(refused.reply.exit_code, 0);
    assert!(refused.reply.stderr.contains("expired"));
}

#[test]
fn metadata_cannot_claim_another_lease_or_execution_start() {
    for field in ["lease", "execution"] {
        let mut fixture = Fixture::new();
        let mut metadata = fixture.registered(inputs());
        metadata[field] = json!("other-server-id");
        let refused = fixture.call(gate::RECORD, &metadata);
        assert_ne!(refused.reply.exit_code, 0);
        assert!(refused.reply.stderr.contains("lease changed"));
    }
}

#[test]
fn only_root_and_zo_scopes_can_hit_and_mandatory_checks_always_remain() {
    for scope in [
        "gate-root",
        "gate-zo",
        "windows",
        "flake-judgment",
        "publish",
        "unknown",
    ] {
        let mut fixture = Fixture::new();
        let mut input = inputs();
        input["gate"] = scope.into();
        let metadata = fixture.registered(input);
        fixture.json(gate::RECORD, &metadata);
        let answer = fixture.json(gate::REUSE, &request(&metadata));
        assert_eq!(
            answer["decision"],
            if ["gate-root", "gate-zo"].contains(&scope) {
                "hit"
            } else {
                "miss"
            }
        );
        assert_eq!(
            answer["mandatoryChecks"],
            json!(["windows", "flake-judgment", "publish"])
        );
    }
}
