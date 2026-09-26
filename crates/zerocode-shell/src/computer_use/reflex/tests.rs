use std::sync::Arc;

use serde_json::{Value, json};
use zerocode_core::computer_flow::{
    FLOW_HEADING, FLOW_HEADING_CHECKS, FLOW_KEY_EVIDENCE, FLOW_KEY_FINGERPRINT, FLOW_KEY_MONEY,
    FLOW_KEY_POLICY, FLOW_MONEY_AMOUNT, FLOW_MONEY_ID, FLOW_MONEY_RECIPIENT,
};
use zerocode_core::computer_recipe::{
    RECIPE_HEADING_STEPS, RECIPE_MARK_MONEY, RECIPE_MARK_SEPARATOR,
};
use zerocode_core::computer_use::COMPUTER_USE_PROTOCOL_VERSION;
use zerocode_core::computer_use_protocol::reflex::{ReflexPlan, plan_hash};
use zerocode_core::jev::JevMode;
use zerocode_core::jev::questions::{REFLEX_DECIDE_OPTIONS, REFLEX_DECIDE_QUESTION};

use super::*;
use crate::systemone::tests::{ANSWERING_VERSION, Endpoint};

/// Every call a fake helper was asked, in order.
type Calls = Vec<(String, Value)>;

/// valid_basic's plan, its scope's surface `surface`, hashed again.
fn plan(surface: &str) -> ReflexPlan {
    let raw: Value = serde_json::from_str(include_str!(
        "../../../../zerocode-core/fixtures/reflex-contract/valid_basic.json"
    ))
    .expect("the golden");
    let mut plan: ReflexPlan = serde_json::from_value(raw["plan"].clone()).expect("a plan");
    plan.scope.surface = serde_json::from_value(json!(surface)).expect("a surface");
    plan.plan_hash = plan_hash(&plan);
    plan
}

/// A Flow document under `policy`, with a money line when `money`, carrying
/// `plan`'s reflex sections when there is one.
fn flow(policy: &str, money: bool, plan: Option<&ReflexPlan>) -> String {
    let (mark, line) = if money {
        (
            format!("{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_MONEY}"),
            format!(
                "- {FLOW_KEY_MONEY}: {FLOW_MONEY_ID}=txn {FLOW_MONEY_AMOUNT}=amount {FLOW_MONEY_RECIPIENT}=recipient\n"
            ),
        )
    } else {
        (String::new(), String::new())
    };
    format!(
        "# reflex\n\n{RECIPE_HEADING_STEPS}\n\n1. `zerocode-browser click browser-1 #send`{mark}\n\n\
         {FLOW_HEADING}\n\n\
         - {FLOW_KEY_POLICY}: {policy}\n\
         - {FLOW_KEY_EVIDENCE}: full\n\
         - {FLOW_KEY_FINGERPRINT}: hosts=fixture.example protocol={COMPUTER_USE_PROTOCOL_VERSION}\n\
         {line}\n\
         {FLOW_HEADING_CHECKS}\n\n\
         1. `zerocode-browser find browser-1 \"done\"` — event, required\n{}",
        plan.map(ReflexPlan::written_sections).unwrap_or_default()
    )
}

/// The door open: a macOS desktop the table claims, the setting on, nobody stopped.
fn open() -> DoorFacts {
    DoorFacts {
        supported: true,
        enabled: true,
        stopped: None,
    }
}

/// A start's words: a Flow document, a display, sixty seconds.
fn start_words() -> Value {
    json!({ "flow": "/flows/reflex.md", "display": 0, "seconds": 60 })
}

/// The handshake of a helper with its kernel installed that reads this plan
/// contract and run policy 1.
fn reading_handshake() -> Value {
    json!({ "providerVersion": "1.0.0", "supports": { "desktop": { "reflex": {
        "planVersion": VERSION, "runPolicy": RUN_POLICY_VERSION, "kernel": true
    } } } })
}

/// A helper that answers the handshake as a helper that reads this contract,
/// a start as running, and anything else from `rest`.
fn helper(calls: &mut Calls) -> impl FnMut(&str, Value) -> Result<Value, ComputerUseError> + '_ {
    move |method: &str, params: Value| {
        calls.push((method.to_string(), params.clone()));
        Ok(match method {
            "handshake" => reading_handshake(),
            "reflexStart" => json!({ "runId": params["runId"], "state": "running", "fires": 0 }),
            "reflexStatus" => {
                json!({ "runId": params["run"], "state": "running", "receiptsPending": 2 })
            }
            "reflexStop" => {
                json!({ "runId": params["run"], "state": "stopped", "reason": "request" })
            }
            _ => json!({}),
        })
    }
}

/// Every road a start may not take never reaches a helper: a stopped operator,
/// a platform the table does not claim, the setting off, a Flow that does not
/// read, is no Flow, carries no reflex sections, moves money or is guarded,
/// and a plan for another surface. The same start through an open door reaches
/// the helper's handshake and then its start — nothing before.
#[test]
fn barred_or_unknown_scope_never_enters_reflex() {
    let desktop = plan("macos_desktop");
    let cases: Vec<(&str, DoorFacts, Option<String>)> = vec![
        (
            "stopped",
            DoorFacts {
                stopped: Some("hotkey".into()),
                ..open()
            },
            Some(flow("dry", false, Some(&desktop))),
        ),
        (
            "unsupported",
            DoorFacts {
                supported: false,
                ..open()
            },
            Some(flow("dry", false, Some(&desktop))),
        ),
        (
            "setting off",
            DoorFacts {
                enabled: false,
                ..open()
            },
            Some(flow("dry", false, Some(&desktop))),
        ),
        ("unreadable", open(), None),
        ("not a Flow", open(), Some("# a recipe, no Flow\n".into())),
        ("no reflex sections", open(), Some(flow("dry", false, None))),
        ("money", open(), Some(flow("dry", true, Some(&desktop)))),
        (
            "guarded",
            open(),
            Some(flow("guarded", true, Some(&desktop))),
        ),
        (
            "another surface",
            open(),
            Some(flow("dry", false, Some(&plan("ios_device")))),
        ),
    ];
    for (name, facts, document) in cases {
        let mut calls = Calls::new();
        let refused = start(
            &start_words(),
            |_| {
                document
                    .clone()
                    .ok_or_else(|| std::io::Error::other("unreadable"))
            },
            &facts,
            &mut helper(&mut calls),
        );
        assert!(refused.is_err(), "{name} was let through");
        assert!(calls.is_empty(), "{name}: the helper was asked {calls:?}");
    }
    let mut calls = Calls::new();
    let (answer, admitted) = start(
        &start_words(),
        |_| Ok(flow("dry", false, Some(&desktop))),
        &open(),
        &mut helper(&mut calls),
    )
    .expect("an open door");
    assert_eq!(
        calls
            .iter()
            .map(|(method, _)| method.as_str())
            .collect::<Vec<_>>(),
        ["handshake", "reflexStart"]
    );
    assert_eq!(answer["state"], json!("running"));
    assert_eq!(
        admitted.workspace.as_deref(),
        Some(Path::new("/flows")),
        "the Flow's folder is the words' workspace"
    );
}

/// A helper that does not say it reads run policy 1 with its kernel installed
/// is asked its handshake and never sent a start.
#[test]
fn a_helper_without_the_run_policy_is_never_sent_a_start() {
    let desktop = plan("macos_desktop");
    for handshake in [
        json!({ "supports": { "desktop": { "reflex": { "planVersion": VERSION, "kernel": true } } } }),
        json!({ "supports": { "desktop": { "reflex": { "planVersion": VERSION, "runPolicy": RUN_POLICY_VERSION, "kernel": false } } } }),
        json!({ "supports": { "desktop": { "reflex": { "planVersion": VERSION + 1, "runPolicy": RUN_POLICY_VERSION, "kernel": true } } } }),
    ] {
        let mut asked = Vec::new();
        let refused = start(
            &start_words(),
            |_| Ok(flow("dry", false, Some(&desktop))),
            &open(),
            &mut |method: &str, _params: Value| {
                asked.push(method.to_string());
                Ok(handshake.clone())
            },
        );
        assert_eq!(
            refused.expect_err("refused").code,
            error_code::UNSUPPORTED_CAPABILITY
        );
        assert_eq!(asked, ["handshake"]);
    }
}

/// Live reflex is advertised only where it can run. On a platform other than
/// macOS nothing claims it — whatever the table's row or the helper says —
/// so `capabilities` and `reflex-status` say `supported: false` and a start
/// is refused at the door with no helper asked. On a Mac, `supported` needs
/// the table's row and a helper with its kernel that reads this contract and
/// run policy 1; and `enabled` is the setting's word, apart from it.
#[test]
fn unsupported_platform_never_advertises_live_reflex() {
    let claims = |live_reflex| ReflexCapability {
        schema_version: VERSION,
        live_reflex,
        instant_pointer: false,
    };
    for claimed in [claims(true), claims(false)] {
        assert!(
            !platform_runs_reflex(false, claimed),
            "another platform claims nothing"
        );
        assert_eq!(platform_runs_reflex(true, claimed), claimed.live_reflex);
    }
    // This build's table claims the macOS desktop: a Mac supports it, every
    // other platform this test runs on does not.
    assert_eq!(DoorFacts::now(true).supported, cfg!(target_os = "macos"));

    let elsewhere = DoorFacts {
        supported: platform_runs_reflex(false, claims(true)),
        ..open()
    };
    let not_here = json!({ "supported": false, "enabled": true });
    let mut calls = Calls::new();
    let answer = capabilities(&json!({}), &elsewhere, &mut helper(&mut calls)).expect("answered");
    assert_eq!(
        answer[LIVE_REFLEX], not_here,
        "even beside a helper that reads it all"
    );
    let answer = status(
        &json!({ "run": "rx-a" }),
        &elsewhere,
        &mut helper(&mut calls),
    )
    .expect("answered");
    assert_eq!(answer[LIVE_REFLEX], not_here);
    calls.clear();
    let refused = start(
        &start_words(),
        |_| Ok(flow("dry", false, Some(&plan("macos_desktop")))),
        &elsewhere,
        &mut helper(&mut calls),
    )
    .expect_err("refused at the door");
    assert_eq!(refused.code, error_code::UNSUPPORTED_CAPABILITY);
    assert!(calls.is_empty(), "the helper was asked {calls:?}");

    // A Mac whose helper has no kernel, or reads another contract or run
    // policy, or says nothing of reflex, supports no run either.
    let reflex = |fields: Value| json!({ "supports": { "desktop": { "reflex": fields } } });
    for handshake in [
        reflex(json!({ "planVersion": VERSION, "runPolicy": RUN_POLICY_VERSION, "kernel": false })),
        reflex(
            json!({ "planVersion": VERSION + 1, "runPolicy": RUN_POLICY_VERSION, "kernel": true }),
        ),
        reflex(
            json!({ "planVersion": VERSION, "runPolicy": RUN_POLICY_VERSION + 1, "kernel": true }),
        ),
        json!({ "supports": { "desktop": {} } }),
    ] {
        assert_eq!(
            standing(&open(), &handshake),
            json!({ "supported": false, "enabled": true }),
            "{handshake}"
        );
    }
    assert_eq!(
        standing(&open(), &reading_handshake()),
        json!({ "supported": true, "enabled": true })
    );
}

/// The person's setting off refuses a start at the door — before the helper
/// is asked anything, its handshake included — and names the setting that
/// turns it on; `capabilities` says the run is supported here and not
/// enabled, two words; and the manual says whose runs these are.
#[test]
fn a_disabled_setting_refuses_before_the_helper() {
    let off = DoorFacts {
        enabled: false,
        ..open()
    };
    let mut calls = Calls::new();
    let refused = start(
        &start_words(),
        |_| Ok(flow("dry", false, Some(&plan("macos_desktop")))),
        &off,
        &mut helper(&mut calls),
    )
    .expect_err("refused at the door");
    assert_eq!(refused.code, error_code::UNSUPPORTED_CAPABILITY);
    assert!(
        refused.message.contains(COMPUTER_LIVE_REFLEX),
        "{}",
        refused.message
    );
    assert!(calls.is_empty(), "the helper was asked {calls:?}");
    let answer = capabilities(&json!({}), &off, &mut helper(&mut calls)).expect("answered");
    assert_eq!(
        answer[LIVE_REFLEX],
        json!({ "supported": true, "enabled": false })
    );
    assert!(
        zerocode_core::computer_use::usage()
            .contains("macOS only, and only with the live reflex setting on"),
        "the manual names the platform and the setting"
    );
}

/// The settings page's check reads what a start's door and handshake read —
/// nobody stopped, in the window or in the helper; this platform's row; a
/// helper whose kernel reads this plan contract and run policy — and asks the
/// helper nothing else: no plan, no frame, no input. Every failing road says
/// the sentence the door says for it, and the check names the helper it
/// checked.
#[test]
fn the_settings_check_reads_what_a_start_reads_and_runs_nothing() {
    let at_ms = 1_790_000_000_000;
    let checked = |facts: &DoorFacts, handshake: Value, helper_status: Value| {
        let mut asked = Vec::new();
        let answer = check(
            facts,
            &handshake,
            &mut |method: &str, _params: Value| {
                asked.push(method.to_string());
                Ok(helper_status.clone())
            },
            at_ms,
        )
        .expect("checked");
        (answer, asked)
    };
    let idle = json!({ "stopped": false, "reason": null });

    let (passed, asked) = checked(&open(), reading_handshake(), idle.clone());
    assert_eq!(
        passed,
        json!({
            "ok": true,
            "at_ms": at_ms,
            "helper": helper_identity(&reading_handshake()),
            "reason": null,
        })
    );
    assert_eq!(
        helper_identity(&reading_handshake()),
        json!({ "version": "1.0.0", "planVersion": VERSION })
    );
    assert_eq!(
        asked,
        ["status"],
        "the check asked the helper for more than its stop"
    );

    // The window stopped, or a platform the table does not claim: refused in
    // the door's words before the helper is asked anything.
    let stopped = DoorFacts {
        stopped: Some("hotkey".into()),
        ..open()
    };
    let not_here = DoorFacts {
        supported: false,
        ..open()
    };
    for (facts, said) in [
        (stopped, super::super::guard::refusal("hotkey").message),
        (not_here, NOT_HERE.to_string()),
    ] {
        let (failed, asked) = checked(&facts, reading_handshake(), idle.clone());
        assert_eq!(failed["ok"], json!(false));
        assert_eq!(failed["reason"], json!(said));
        assert!(asked.is_empty(), "the helper was asked {asked:?}");
    }

    // A helper that does not read this contract, in the handshake's words.
    let (failed, asked) = checked(
        &open(),
        json!({ "supports": { "desktop": { "reflex": { "planVersion": VERSION, "kernel": true } } } }),
        idle,
    );
    assert_eq!(failed["reason"], json!(HELPER_DOES_NOT_READ));
    assert!(asked.is_empty(), "the helper was asked {asked:?}");

    // A helper stopped on its own — the person's chord heard on the desktop.
    let (failed, _) = checked(
        &open(),
        reading_handshake(),
        json!({ "stopped": true, "reason": "hotkey" }),
    );
    assert_eq!(failed["ok"], json!(false));
    assert_eq!(
        failed["reason"],
        json!(super::super::guard::refusal("hotkey").message)
    );
}

/// The settings page's answer is `capabilities` with the check beside it: a
/// check made now is kept beside the settings — never in them — and read back
/// only while the helper that answers is the one it checked; another helper
/// version, another plan contract or no kept check reads as none.
#[test]
fn a_kept_check_vouches_only_for_the_helper_it_checked() {
    let home = tempfile::tempdir().expect("a home");
    let kept = home.path().join(CHECK_FILE);
    let at_ms = 1_790_000_000_000;
    let answering = |handshake: Value| {
        move |method: &str, _params: Value| -> Result<Value, ComputerUseError> {
            Ok(match method {
                "handshake" => handshake.clone(),
                "status" => json!({ "stopped": false, "reason": null }),
                other => panic!("the page asked the helper for {other}"),
            })
        }
    };

    let fresh = settings_answer(&open(), &mut answering(reading_handshake()), &kept, None)
        .expect("answered");
    assert_eq!(
        fresh[LIVE_REFLEX],
        json!({ "supported": true, "enabled": true })
    );
    assert_eq!(fresh[CHECK], Value::Null, "a check nobody made");

    let made = settings_answer(
        &open(),
        &mut answering(reading_handshake()),
        &kept,
        Some(at_ms),
    )
    .expect("checked");
    assert_eq!(made[CHECK]["ok"], json!(true));
    assert_eq!(made[CHECK]["at_ms"], json!(at_ms));
    assert!(kept.is_file(), "the check was not kept");

    let again = settings_answer(&open(), &mut answering(reading_handshake()), &kept, None)
        .expect("answered");
    assert_eq!(
        again[CHECK], made[CHECK],
        "the same helper reads its check back"
    );

    let mut newer = reading_handshake();
    newer["providerVersion"] = json!("1.0.1");
    let mut other_plan = reading_handshake();
    other_plan["supports"]["desktop"]["reflex"]["planVersion"] = json!(VERSION + 1);
    for handshake in [newer, other_plan] {
        let answer = settings_answer(&open(), &mut answering(handshake.clone()), &kept, None)
            .expect("answered");
        assert_eq!(
            answer[CHECK],
            Value::Null,
            "an old check vouched for {handshake}"
        );
    }
}

/// The switch on the settings page opens the door a start passes: the
/// person's setting written through the settings document is the `enabled`
/// a start reads (`DoorFacts::now`), and on this platform the start reaches
/// the helper's handshake and its start — on a Mac — or is refused at the
/// door with nothing asked elsewhere.
#[test]
fn the_setting_turned_on_opens_the_door_on_this_platform() {
    let home = tempfile::tempdir().expect("a home");
    let repository = crate::settings::SettingsRepository::new(home.path());
    let desktop = plan("macos_desktop");
    let starting = |facts: &DoorFacts| {
        let mut calls = Calls::new();
        let started = start(
            &start_words(),
            |_| Ok(flow("dry", false, Some(&desktop))),
            facts,
            &mut helper(&mut calls),
        );
        (started.map(|_| ()), calls)
    };
    // Nobody's stop: another test's stop is the process's, not this one's.
    let now = |enabled| DoorFacts {
        stopped: None,
        ..DoorFacts::now(enabled)
    };

    let off = now(crate::settings_runtime::computer_live_reflex(&repository));
    assert!(!off.enabled, "the switch is on before anyone turned it on");
    let (refused, calls) = starting(&off);
    assert!(
        refused
            .expect_err("refused")
            .message
            .contains(COMPUTER_LIVE_REFLEX)
            || !off.supported
    );
    assert!(calls.is_empty(), "the helper was asked {calls:?}");

    crate::settings_runtime::mutate_settings(&repository, |document| {
        document.computer_live_reflex = true;
        Ok(())
    })
    .expect("the switch turned on");
    let on = now(crate::settings_runtime::computer_live_reflex(&repository));
    assert!(on.enabled, "the switch did not reach the door");
    let (started, calls) = starting(&on);
    let asked: Vec<&str> = calls.iter().map(|(method, _)| method.as_str()).collect();
    if cfg!(target_os = "macos") {
        started.expect("admitted");
        assert_eq!(asked, ["handshake", "reflexStart"]);
    } else {
        assert_eq!(
            started.expect_err("refused").code,
            error_code::UNSUPPORTED_CAPABILITY
        );
        assert!(asked.is_empty(), "the helper was asked {asked:?}");
    }
}

/// A start answers at once — the run's id and its state from the helper,
/// which holds the hand for the run — and asks nothing to end or wait: the
/// window never stops what it just started, and its watch reads the run
/// afterwards on its own road.
#[test]
fn one_hand_remains_owned_after_start_returns() {
    let desktop = plan("macos_desktop");
    let mut calls = Calls::new();
    let began = Instant::now();
    let (answer, admitted) = start(
        &start_words(),
        |_| Ok(flow("dry", false, Some(&desktop))),
        &open(),
        &mut helper(&mut calls),
    )
    .expect("started");
    assert!(began.elapsed() < Duration::from_secs(1), "answered at once");
    assert_eq!(
        answer["state"],
        json!("running"),
        "the helper's hand runs the plan"
    );
    let sent = &calls[1].1;
    assert_eq!(calls[1].0, "reflexStart");
    assert!(reflex::identifier(
        sent["runId"].as_str().unwrap_or_default()
    ));
    // What the start carries is the window's: the plan's canonical wire, its
    // tables, the run's policy and the capability table.
    assert_eq!(
        sent["plan"],
        json!(String::from_utf8(reflex::wire_bytes(admitted.plan.plan())).unwrap())
    );
    assert_eq!(
        sent["limits"],
        json!(String::from_utf8(reflex::limits_wire()).unwrap())
    );
    assert_eq!(
        sent["perception"],
        json!(String::from_utf8(game_state::limits_wire(&game_state::LIMITS)).unwrap())
    );
    assert_eq!(
        sent["runPolicy"],
        json!(String::from_utf8(admitted.policy.wire()).unwrap())
    );
    assert_eq!(
        sent["capability"],
        json!(String::from_utf8(reflex::capability_wire().to_vec()).unwrap())
    );
    assert!(
        calls
            .iter()
            .all(|(method, _)| !matches!(method.as_str(), "reflexStop" | "stop" | "reflexAck")),
        "the start stops nothing it started"
    );
}

/// `reflex-stop --run` carries the run it names to the helper, which compares
/// it there: the window keeps no idea of which run is current.
#[test]
fn reflex_stop_carries_its_run_to_the_helper() {
    let words = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>()
    };
    let command =
        zerocode_core::computer_use::parse_command(&words(&["reflex-stop", "--run", "rx-a"]))
            .expect("a stop");
    let mut calls = Calls::new();
    let answer = stop(&command.params, &mut helper(&mut calls)).expect("answered");
    assert_eq!(
        calls,
        vec![("reflexStop".to_string(), json!({ "run": "rx-a" }))]
    );
    assert_eq!(answer["runId"], json!("rx-a"));
    assert!(
        zerocode_core::computer_use::parse_command(&words(&["reflex-stop"])).is_err(),
        "a stop names its run"
    );
    assert!(
        zerocode_core::computer_use::parse_command(&words(&["reflex-stop", "--run", "not an id"]))
            .is_err()
    );
}

/// A public status reads the run and nothing more: it never reads or
/// acknowledges the receipts the run's collector alone takes.
#[test]
fn public_status_never_takes_collector_receipts() {
    let mut calls = Calls::new();
    for _ in 0..3 {
        let answer =
            status(&json!({ "run": "rx-a" }), &open(), &mut helper(&mut calls)).expect("answered");
        assert!(answer.get("receipts").is_none());
    }
    // It asks the helper what it reads (for `liveReflex`) and the run's
    // status — never its receipts, never an acknowledgement.
    assert!(calls.iter().all(
        |(method, params)| (method == "handshake" && params == &json!({}))
            || (method == "reflexStatus" && params == &json!({ "run": "rx-a" }))
    ));
    assert_eq!(
        calls
            .iter()
            .filter(|(method, _)| method == "reflexStatus")
            .count(),
        3
    );
}

/// A helper's run as the collector sees it: receipts numbered 1…`issued`,
/// held until acknowledged; `lose` answers of `reflexReceipts` lost on the way.
struct FakeRun {
    issued: u64,
    acknowledged: u64,
    state: &'static str,
    reason: Option<&'static str>,
    lose: usize,
    calls: Calls,
}

impl FakeRun {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, ComputerUseError> {
        self.calls.push((method.to_string(), params.clone()));
        match method {
            "reflexReceipts" => {
                if self.lose > 0 {
                    self.lose -= 1;
                    return Err(ComputerUseError::new("transport", "the answer was lost"));
                }
                let after = params["after"].as_u64().unwrap_or(0).max(self.acknowledged);
                let receipts: Vec<Value> = ((after + 1)..=self.issued)
                    .map(|seq| json!({ "seq": seq, "ruleId": "follow", "actionId": "click1", "outcome": "done" }))
                    .collect();
                Ok(json!({
                    "runId": params["run"], "state": self.state, "reason": self.reason,
                    "receiptsIssued": self.issued, "receiptsPending": self.issued - self.acknowledged,
                    "receipts": receipts,
                    "sightings": [{ "detector": "ball", "value": 1, "unknown": null, "track": 5, "ageNs": 2_000_000 }],
                    "outcomes": { "done": self.issued },
                    "scene": { "stream": 1, "geometry": 1, "owner": 3, "plan": 1 },
                    "lastCapture": self.issued, "lastCaptureAgeNs": 2_000_000,
                }))
            }
            "reflexAck" => {
                self.acknowledged = self
                    .acknowledged
                    .max(params["through"].as_u64().unwrap_or(0));
                Ok(json!({ "receiptsPending": self.issued - self.acknowledged }))
            }
            other => Ok(json!({ "unexpected": other })),
        }
    }
}

/// A sink in memory that fails its next `fail` writes halfway — some lines on
/// "disk", then the error.
#[derive(Default)]
struct HalfwaySink {
    disk: Vec<u8>,
    fail: usize,
}

impl ReceiptSink for HalfwaySink {
    fn keep(&mut self, durable: u64, lines: &[u8]) -> std::io::Result<u64> {
        self.disk
            .truncate(usize::try_from(durable).unwrap_or(usize::MAX));
        if self.fail > 0 {
            self.fail -= 1;
            self.disk.extend_from_slice(&lines[..lines.len() / 2]);
            return Err(std::io::Error::other("the disk refused the rest"));
        }
        self.disk.extend_from_slice(lines);
        Ok(self.disk.len() as u64)
    }
}

impl HalfwaySink {
    fn seqs(&self) -> Vec<u64> {
        String::from_utf8_lossy(&self.disk)
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("a whole line")["seq"]
                    .as_u64()
                    .expect("a seq")
            })
            .collect()
    }
}

fn never_asked() -> Asker {
    Arc::new(|_state: Value| panic!("no question is asked while the seat is off"))
}

/// Receipts leave the helper only once the window has them on disk: a write
/// that failed halfway acknowledges nothing and is written again from the
/// last durable length; an answer lost on the way reads the same receipts
/// again; each lands on disk once, in order, and the run's last batch is read
/// after it ended.
#[test]
fn receipts_leave_only_after_the_window_acks() {
    let mut run = FakeRun {
        issued: 3,
        acknowledged: 0,
        state: "running",
        reason: None,
        lose: 0,
        calls: Calls::new(),
    };
    let mut sink = HalfwaySink {
        fail: 1,
        ..HalfwaySink::default()
    };
    let mut watch = Watch::new("rx-a", None);
    let mut rows = Vec::new();
    let mut pass = |run: &mut FakeRun, sink: &mut HalfwaySink, watch: &mut Watch| {
        watch.pass(
            &mut |method, params| run.call(method, params),
            sink,
            || JevMode::Off,
            &never_asked(),
            &mut |more| rows.extend(more),
        )
    };
    assert!(!pass(&mut run, &mut sink, &mut watch));
    assert_eq!(
        run.acknowledged, 0,
        "a write that failed acknowledges nothing"
    );
    assert_eq!(watch.report().write_failures, 1);
    assert!(!pass(&mut run, &mut sink, &mut watch));
    assert_eq!(
        sink.seqs(),
        [1, 2, 3],
        "written again from where it was durable, once"
    );
    assert_eq!(run.acknowledged, 3, "acknowledged once on disk");
    // Two more leaves; the answer to the next read is lost on the way.
    run.issued = 5;
    run.lose = 1;
    assert!(!pass(&mut run, &mut sink, &mut watch));
    assert_eq!(run.acknowledged, 3);
    assert!(!pass(&mut run, &mut sink, &mut watch));
    assert_eq!(sink.seqs(), [1, 2, 3, 4, 5]);
    // The run ends with one more receipt: the last batch is read after the end.
    run.issued = 6;
    run.state = "stopped";
    run.reason = Some("deadline");
    assert!(
        pass(&mut run, &mut sink, &mut watch),
        "done once the ended run's last receipt is on disk"
    );
    assert_eq!(sink.seqs(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(watch.report().verified, Some(true));
    let acks: Vec<u64> = run
        .calls
        .iter()
        .filter(|(method, _)| method == "reflexAck")
        .map(|(_, params)| params["through"].as_u64().unwrap())
        .collect();
    assert_eq!(acks, [3, 5, 6], "every acknowledgement after its write");
    assert!(rows.is_empty(), "the seat was off: no rows");
}

/// A run that ended because its queue filled is not verified, whatever made
/// it to disk.
#[test]
fn an_overflowed_run_is_not_verified() {
    let mut run = FakeRun {
        issued: 2,
        acknowledged: 0,
        state: "stopped",
        reason: Some("overflow"),
        lose: 0,
        calls: Calls::new(),
    };
    let mut sink = HalfwaySink::default();
    let mut watch = Watch::new("rx-o", None);
    assert!(watch.pass(
        &mut |method, params| run.call(method, params),
        &mut sink,
        || JevMode::Off,
        &never_asked(),
        &mut |_| {}
    ));
    assert_eq!(watch.report().verified, Some(false));
    assert_eq!(sink.seqs(), [1, 2]);
}

// ---- the reflex decision on a real socket ----------------------------------------

/// A settings file under a temporary home: Jev switched on, consent for
/// `consented`, and the reflex decision's word `mode` when one is given.
fn settings(consented: &[&str], mode: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let home = tempfile::tempdir().expect("a home");
    let path = home.path().join("settings.json");
    let mut root = json!({ "smart": { "jev": { "enabled": true, "workspaces": consented } } });
    if let Some(mode) = mode {
        root["smart"][REFLEX_DECIDE.setting] = json!(mode);
    }
    std::fs::write(&path, root.to_string()).expect("written");
    (home, path)
}

/// The endpoint's body naming `word`.
fn answer_naming(word: &str) -> String {
    let probabilities: serde_json::Map<String, Value> = REFLEX_DECIDE_OPTIONS
        .iter()
        .map(|(option, _)| {
            (
                (*option).to_string(),
                json!(if *option == word { 0.8 } else { 0.1 }),
            )
        })
        .collect();
    json!({ "answers": { REFLEX_DECIDE_QUESTION: {
        "type": "choice", "choice": word, "probabilities": probabilities, "confidence": 0.7
    } }, "model": ANSWERING_VERSION })
    .to_string()
}

fn a_state() -> Value {
    reflex_decide::snapshot_of(&json!({
        "runId": "rx-a", "sightings": [{ "detector": "ball", "value": 1, "unknown": null, "track": 5, "ageNs": 1 }],
        "outcomes": { "done": 3 },
    }))
    .state
}

/// A question that left counts as one request whatever became of it — an HTTP
/// error or a wall — with its bytes and its round trip kept, and the endpoint
/// heard it once; nothing asked again, and no other model asked.
#[test]
fn a_sent_request_counts_even_if_failed_or_late() {
    let (_home, path) = settings(&["*"], Some("shadow"));
    let failing = Endpoint::serving("HTTP/1.1 503 Service Unavailable", "{}".into(), 0);
    let (wired, _) = asker(
        Wire::at(&failing.base(), "key", Some(path.clone())),
        Some(PathBuf::from("/flows")),
    )(a_state());
    assert_eq!(wired.attempts, 1);
    assert_eq!(wired.answer, Err("http_503".to_string()));
    assert!(wired.request_bytes > 0);
    assert_eq!(failing.asked().len(), 1, "heard once, never asked again");
    let slow = Endpoint::serving(
        "HTTP/1.1 200 OK",
        answer_naming("pause"),
        REFLEX_DECIDE_DEADLINE_MS + 500,
    );
    let (wired, _) = asker(
        Wire::at(&slow.base(), "key", Some(path)),
        Some(PathBuf::from("/flows")),
    )(a_state());
    assert_eq!(wired.answer, Err("timeout".to_string()));
    assert_eq!(wired.attempts, 1);
    assert!(
        wired.rtt_ms >= REFLEX_DECIDE_DEADLINE_MS,
        "the wall: {} ms",
        wired.rtt_ms
    );
    assert_eq!(slow.asked().len(), 1);
    let asked = &slow.asked()[0];
    assert!(
        asked.contains("\"model\":\"jev-latest\""),
        "the alias, never another model"
    );
}

/// A decision the seat did not consent to never reaches a model: the seat off
/// asks nothing at all, and a workspace nobody consented to is refused at the
/// door with nothing sent.
#[test]
fn reflex_decision_does_not_call_an_unconsented_model() {
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer_naming("continue"), 0);
    let (_home, path) = settings(&["/somewhere/else"], Some("shadow"));
    let (wired, spent) = asker(
        Wire::at(&endpoint.base(), "key", Some(path)),
        Some(PathBuf::from("/flows")),
    )(a_state());
    assert_eq!(wired.attempts, 0);
    assert_eq!(spent.requests, 0);
    assert_eq!(wired.answer, Err("not_consented".to_string()));
    assert!(endpoint.asked().is_empty());
    // The seat off: the watch asks nothing, whatever the desktop's consent says.
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counting: Asker = {
        let asked = Arc::clone(&asked);
        Arc::new(move |_state: Value| {
            asked.fetch_add(1, Ordering::SeqCst);
            (
                Wired {
                    answer: Err("off".into()),
                    attempts: 0,
                    request_bytes: 0,
                    rtt_ms: 0,
                },
                Spent::default(),
            )
        })
    };
    let mut run = FakeRun {
        issued: 1,
        acknowledged: 0,
        state: "running",
        reason: None,
        lose: 0,
        calls: Calls::new(),
    };
    let mut watch = Watch::new("rx-a", None);
    let mut rows = Vec::new();
    for _ in 0..3 {
        run.issued += 1;
        watch.pass(
            &mut |method, params| run.call(method, params),
            &mut HalfwaySink::default(),
            || JevMode::Off,
            &counting,
            &mut |more| rows.extend(more),
        );
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        asked.load(Ordering::SeqCst),
        0,
        "no question with the seat off"
    );
    assert!(rows.is_empty(), "and no row");
    assert_eq!(
        REFLEX_DECIDE.mode_in(&json!({ "smart": { "desktopAction": "on" } })),
        JevMode::Off
    );
}

/// Pass until the question in flight comes back, a few at most.
fn until_answered(
    watch: &mut Watch,
    run: &mut FakeRun,
    mode: &dyn Fn() -> JevMode,
    ask: &Asker,
) -> Vec<Value> {
    let mut rows = Vec::new();
    for _ in 0..200 {
        watch.pass(
            &mut |method, params| run.call(method, params),
            &mut HalfwaySink::default(),
            mode,
            ask,
            &mut |more| rows.extend(more),
        );
        if rows
            .iter()
            .any(|row: &Value| row["road"] == json!(reflex_decide::ROAD_JEV))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    rows
}

/// A question that left while the seat asked, and whose seat was switched off
/// before its answer came, stays counted — its bytes and round trip — and its
/// answer is not kept.
#[test]
fn consent_off_after_send_keeps_count_drops_answer() {
    let (_home, path) = settings(&["*"], Some("shadow"));
    let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer_naming("pause"), 50);
    let ask = asker(
        Wire::at(&endpoint.base(), "key", Some(path)),
        Some(PathBuf::from("/flows")),
    );
    let mut run = FakeRun {
        issued: 1,
        acknowledged: 0,
        state: "running",
        reason: None,
        lose: 0,
        calls: Calls::new(),
    };
    let mut watch = Watch::new("rx-a", None);
    let asking = std::sync::atomic::AtomicBool::new(true);
    let mode = || {
        if asking.load(Ordering::SeqCst) {
            JevMode::Shadow
        } else {
            JevMode::Off
        }
    };
    let mut rows = Vec::new();
    watch.pass(
        &mut |method, params| run.call(method, params),
        &mut HalfwaySink::default(),
        mode,
        &ask,
        &mut |more| rows.extend(more),
    );
    assert!(rows.is_empty(), "sent, not yet answered");
    asking.store(false, Ordering::SeqCst);
    let rows = until_answered(&mut watch, &mut run, &mode, &ask);
    let row = rows
        .iter()
        .find(|row| row["road"] == json!(reflex_decide::ROAD_JEV))
        .expect("the sent question's row");
    assert_eq!(row["outcome"], json!(reflex_decide::WITHDRAWN));
    assert_eq!(row["attempts"], json!(1));
    assert!(row["requestBytes"].as_u64().unwrap_or(0) > 0);
    assert!(
        row.get("chosen").is_none() && row.get("probabilities").is_none(),
        "the answer is not kept"
    );
    assert_eq!(endpoint.asked().len(), 1);
}

/// A decision under `shadow` is recorded and changes nothing the run does:
/// the watch asks the helper for its receipts and acknowledges them, and
/// never stops, pauses or re-plans the run — whatever the teacher answered.
#[test]
fn a_shadow_decision_changes_no_input() {
    let (_home, path) = settings(&["*"], Some("shadow"));
    for word in ["pause", "replan", "continue"] {
        let endpoint = Endpoint::serving("HTTP/1.1 200 OK", answer_naming(word), 0);
        let ask = asker(
            Wire::at(&endpoint.base(), "key", Some(path.clone())),
            Some(PathBuf::from("/flows")),
        );
        let mut run = FakeRun {
            issued: 1,
            acknowledged: 0,
            state: "running",
            reason: None,
            lose: 0,
            calls: Calls::new(),
        };
        let mut watch = Watch::new("rx-a", None);
        let rows = until_answered(&mut watch, &mut run, &|| JevMode::Shadow, &ask);
        let row = rows
            .iter()
            .find(|row| row["road"] == json!(reflex_decide::ROAD_JEV))
            .expect("answered");
        assert_eq!(row["chosen"], json!(word));
        assert_eq!(row["applied"], json!(false));
        assert!(
            run.calls
                .iter()
                .all(|(method, _)| matches!(method.as_str(), "reflexReceipts" | "reflexAck")),
            "{word}: the watch only reads and acknowledges: {:?}",
            run.calls
                .iter()
                .map(|(method, _)| method.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(watch.report().attempts, 1);
    }
}

/// The reflex decision against the real endpoint, one question after another
/// (t-9205 §11): run by hand, the key in one command's environment
/// (`ZEROCODE_REFLEX_MEASURE_KEY`) and a temporary config home
/// (`ZO_CONFIG_HOME`) — never the person's own. It prints what it measured and
/// never the key.
#[test]
#[ignore = "crosses the real wire with a key from the environment"]
fn reflex_decide_real_wire_sequential() {
    let key = std::env::var("ZEROCODE_REFLEX_MEASURE_KEY")
        .expect("the key, in this command's environment");
    let home = PathBuf::from(std::env::var("ZO_CONFIG_HOME").expect("a temporary config home"));
    let count: usize = std::env::var("ZEROCODE_REFLEX_MEASURE_N")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(100);
    let path = home.join("settings.json");
    std::fs::write(
        &path,
        json!({ "smart": { "jev": { "enabled": true, "workspaces": ["*"] }, (REFLEX_DECIDE.setting): "shadow" } }).to_string(),
    )
    .expect("the temporary settings");
    let ask = asker(
        Wire::at(crate::systemone::SYSTEMONE_BASE_URL, &key, Some(path)),
        Some(home.clone()),
    );
    let mut rtts = Vec::new();
    let mut bytes = Vec::new();
    let mut outcomes: BTreeMap<String, u64> = BTreeMap::new();
    let mut models: BTreeMap<String, u64> = BTreeMap::new();
    let mut chosen: BTreeMap<String, u64> = BTreeMap::new();
    let mut attempts = 0_u64;
    for at in 0..count {
        let state = reflex_decide::snapshot_of(&json!({
            "runId": "rx-measure",
            "sightings": [
                { "detector": "ball", "value": i64::from(at % 3 != 0), "unknown": if at % 5 == 0 { json!("occluded") } else { Value::Null }, "track": at / 4 + 1, "ageNs": 3_000_000 + at as u64 * 10_000 },
            ],
            "outcomes": { "done": at as u64 * 2, "moved": (at / 7) as u64, "lease": (at / 11) as u64 },
        }))
        .state;
        let (wired, spent) = ask(state);
        attempts += u64::from(wired.attempts);
        rtts.push(wired.rtt_ms);
        bytes.push(wired.request_bytes as u64);
        let pending = reflex_decide::Pending {
            id: at as u64 + 1,
            snapshot: reflex_decide::snapshot_of(&json!({})),
        };
        let row = reflex_decide::asked_row("rx-measure", &pending, &wired, None, true, false);
        *outcomes
            .entry(row["outcome"].as_str().unwrap_or("none").to_string())
            .or_default() += 1;
        if let Some(word) = row["chosen"].as_str() {
            *chosen.entry(word.to_string()).or_default() += 1;
        }
        *models
            .entry(spent.model.unwrap_or_else(|| "none".into()))
            .or_default() += 1;
    }
    let rank = |values: &[u64], p: f64| {
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let at = ((p / 100.0 * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
        sorted[at - 1]
    };
    println!(
        "{}",
        json!({
            "n": count, "attempts": attempts, "outcomes": outcomes, "chosen": chosen, "answeredBy": models,
            "rttMs": { "p50": rank(&rtts, 50.0), "p95": rank(&rtts, 95.0), "max": rtts.iter().max() },
            "requestBytes": { "p50": rank(&bytes, 50.0), "max": bytes.iter().max() },
            "deadlineMs": REFLEX_DECIDE_DEADLINE_MS,
        })
    );
}
