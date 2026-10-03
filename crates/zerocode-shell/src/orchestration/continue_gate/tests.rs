use std::cell::RefCell;

use zerocode_core::continue_gate::plan::STOP_RETRY_MS;
use zerocode_core::continue_gate::{Code, REWORK_MIN_STEPS};
use zerocode_core::hook::{Activity, Phase, Tool};

use super::*;
use crate::orchestration::gate_book::GateBook;

const NOW: i64 = 1_800_000_000_000;
const SECOND: i64 = 1_000;

/// The door of a test: a log of what the gate did, in order, and what each act
/// is to answer.
#[derive(Default)]
struct FakeDoor {
    log: RefCell<Vec<String>>,
    told: RefCell<Vec<GateReceipt>>,
    snapshot_says: RefCell<Option<Result<Option<String>, String>>>,
    stop_fails: bool,
}

impl FakeDoor {
    fn log(&self) -> Vec<String> {
        self.log.borrow().clone()
    }
}

impl GateDoor for FakeDoor {
    fn snapshot(&self, _pane: &WorkerPane, number: u32) -> Result<Option<String>, String> {
        self.log.borrow_mut().push(format!("snapshot {number}"));
        self.snapshot_says
            .borrow()
            .clone()
            .unwrap_or_else(|| Ok(Some(format!("refs/zerocode/checkpoints/w-1/{number}"))))
    }

    fn tell(&self, receipt: GateReceipt, _now_ms: i64) -> bool {
        self.log.borrow_mut().push("tell".to_string());
        self.told.borrow_mut().push(receipt);
        true
    }

    fn stop(&self, _pane: &WorkerPane, why: &str, _now_ms: i64) -> Result<(), String> {
        self.log.borrow_mut().push(format!("stop {why}"));
        if self.stop_fails {
            Err("the seat is gone".to_string())
        } else {
            Ok(())
        }
    }
}

fn pane() -> WorkerPane {
    WorkerPane {
        run: "run-1".into(),
        worker: "w-1".into(),
        dispatch: "dp-1".into(),
        task: "t-1".into(),
        agent: "claude".into(),
        term: 7,
        floor: None,
        checkout: Some("/repo/checkout".into()),
    }
}

fn activity(phase: Phase) -> Activity {
    Activity {
        verb: Tool::Other("tool".into()),
        target: None,
        phase,
        reads: Vec::new(),
        writes: Vec::new(),
        vcs: Vec::new(),
        cwd: None,
    }
}

fn call(gate: &mut PaneGate, command: &str) {
    let payload = format!(
        r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{{"command":"{command}"}}}}"#
    );
    gate.book.note_activity(&activity(Phase::Started), &payload);
}

/// `count` different calls: the ordinary look of a worker making progress.
fn work(gate: &mut PaneGate, count: usize) {
    for index in 0..count {
        call(gate, &format!("echo {index}"));
    }
}

fn settings(mode: Mode) -> Settings {
    Settings {
        mode,
        task_usd: None,
        day_usd: None,
    }
}

fn around(allowance: Allowance) -> Around {
    Around {
        allowance,
        task_spent: 0.0,
        day_spent: 0.0,
        cost: CostNote::Read,
    }
}

fn nothing() -> Around {
    around(Allowance::default())
}

/// A budget this worker is about to spend: nine of ten dollars gone and two
/// more on their way.
fn about_to_overspend() -> Allowance {
    Allowance {
        task: Some(Cap {
            limit_usd: 10.0,
            spent_usd: 9.0,
            ahead_usd: 2.0,
        }),
        day: None,
    }
}

#[test]
fn a_worker_making_progress_is_judged_and_nothing_is_done() {
    let door = FakeDoor::default();
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    work(gate, 10);
    step(gate, &pane(), settings(Mode::Stop), &nothing(), &door, NOW);
    assert_eq!(door.log(), Vec::<String>::new());
    let reading = gate.reading.clone().expect("a reading for the board");
    assert_eq!(reading.verdict, Verdict::Continue);
    assert_eq!(reading.metrics.steps, 10);
    assert_eq!(reading.mode, Mode::Stop);
}

#[test]
fn a_checkpoint_that_is_due_saves_the_tree_once_and_counts_it() {
    let door = FakeDoor::default();
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    work(gate, 200);
    step(
        gate,
        &pane(),
        settings(Mode::Notify),
        &nothing(),
        &door,
        NOW,
    );
    assert_eq!(door.log(), ["snapshot 1"]);
    let reading = gate.reading.clone().expect("a reading");
    assert_eq!(reading.verdict, Verdict::Checkpoint);
    assert_eq!(
        reading
            .snapshot
            .expect("the board says what was saved")
            .reference,
        Some("refs/zerocode/checkpoints/w-1/1".to_string())
    );
    assert_eq!(gate.book.metrics().checkpoints, 1);
    assert_eq!(gate.book.metrics().since_checkpoint, 0);

    step(
        gate,
        &pane(),
        settings(Mode::Notify),
        &nothing(),
        &door,
        NOW + SECOND,
    );
    assert_eq!(door.log(), ["snapshot 1"], "and not again the next beat");
    assert_eq!(
        gate.reading.clone().expect("a reading").verdict,
        Verdict::Continue
    );
}

#[test]
fn a_checkout_that_cannot_be_saved_is_noted_and_not_asked_again_every_beat() {
    let door = FakeDoor::default();
    *door.snapshot_says.borrow_mut() = Some(Err("the disk is full".to_string()));
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    work(gate, 200);
    step(
        gate,
        &pane(),
        settings(Mode::Notify),
        &nothing(),
        &door,
        NOW,
    );
    let note = gate
        .reading
        .clone()
        .expect("a reading")
        .snapshot
        .expect("a note");
    assert_eq!(note.error.as_deref(), Some("the disk is full"));
    assert_eq!(note.reference, None);
    step(
        gate,
        &pane(),
        settings(Mode::Notify),
        &nothing(),
        &door,
        NOW + SECOND,
    );
    assert_eq!(door.log(), ["snapshot 1"]);
}

#[test]
fn a_loop_pauses_and_tells_the_coordinator_once_with_its_reasons() {
    let door = FakeDoor::default();
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    for _ in 0..REWORK_MIN_STEPS + 4 {
        call(gate, "cargo test");
    }
    step(
        gate,
        &pane(),
        settings(Mode::Notify),
        &nothing(),
        &door,
        NOW,
    );
    step(
        gate,
        &pane(),
        settings(Mode::Notify),
        &nothing(),
        &door,
        NOW + SECOND,
    );
    let told = door.told.borrow();
    assert_eq!(told.len(), 1, "told once however long it loops");
    let receipt = &told[0];
    assert_eq!(receipt.key, "gate-dp-1-pause");
    assert_eq!(receipt.worker, "w-1");
    assert_eq!(receipt.verdict, Verdict::Pause);
    assert_eq!(receipt.acted, Acted::Told);
    assert_eq!(receipt.reasons[0].code, Code::ReworkLoop);
    assert!(receipt.metrics.rework_permille.is_some());
    assert_eq!(
        gate.reading.clone().expect("a reading").verdict,
        Verdict::Pause
    );
}

#[test]
fn in_stop_mode_a_worker_about_to_overspend_is_saved_then_ended_then_told() {
    let door = FakeDoor::default();
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    work(gate, 30);
    step(
        gate,
        &pane(),
        settings(Mode::Stop),
        &around(about_to_overspend()),
        &door,
        NOW,
    );
    assert_eq!(
        door.log(),
        [
            "snapshot 1",
            "stop gate: task_budget_stop — $11.00 of $10.00",
            "tell"
        ],
        "the tree first, the worker second, the word last"
    );
    let told = door.told.borrow();
    assert_eq!(told[0].verdict, Verdict::Stop);
    assert_eq!(told[0].acted, Acted::Stopped);
    assert_eq!(
        told[0].snapshot.as_deref(),
        Some("refs/zerocode/checkpoints/w-1/1")
    );
}

#[test]
fn a_stop_that_failed_is_told_as_failed_and_asked_again_only_after_its_time() {
    let door = FakeDoor {
        stop_fails: true,
        ..FakeDoor::default()
    };
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    let overspend = around(about_to_overspend());
    step(gate, &pane(), settings(Mode::Stop), &overspend, &door, NOW);
    assert_eq!(door.told.borrow()[0].acted, Acted::StopFailed);
    step(
        gate,
        &pane(),
        settings(Mode::Stop),
        &overspend,
        &door,
        NOW + SECOND,
    );
    let stops = |door: &FakeDoor| {
        door.log()
            .iter()
            .filter(|line| line.starts_with("stop"))
            .count()
    };
    assert_eq!(stops(&door), 1, "not again the next beat");
    step(
        gate,
        &pane(),
        settings(Mode::Stop),
        &overspend,
        &door,
        NOW + STOP_RETRY_MS,
    );
    assert_eq!(stops(&door), 2, "asked again once its time is up");
    assert_eq!(door.told.borrow().len(), 1, "the word is not repeated");
}

#[test]
fn a_gate_that_only_tells_never_ends_a_worker_however_far_over_it_would_go() {
    let door = FakeDoor::default();
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    for beat in 0..20 {
        step(
            gate,
            &pane(),
            settings(Mode::Notify),
            &around(about_to_overspend()),
            &door,
            NOW + beat * SECOND,
        );
    }
    assert!(!door.log().iter().any(|line| line.starts_with("stop")));
    let told = door.told.borrow();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].verdict, Verdict::Stop);
    assert_eq!(told[0].acted, Acted::Told, "it said so and ended nobody");
}

#[test]
fn the_reading_carries_what_was_spent_the_budgets_and_why_the_cost_is_what_it_is() {
    let door = FakeDoor::default();
    let mut book = GateBook::default();
    let gate = book.pane(7, "launch", NOW);
    let facts = Around {
        allowance: Allowance::default(),
        task_spent: 4.25,
        day_spent: 17.5,
        cost: CostNote::NoReader,
    };
    let limits = Settings {
        mode: Mode::Notify,
        task_usd: Some(20.0),
        day_usd: Some(100.0),
    };
    step(gate, &pane(), limits, &facts, &door, NOW);
    let reading = gate.reading.clone().expect("a reading");
    assert_eq!(reading.task_spent_usd, 4.25);
    assert_eq!(reading.task_limit_usd, Some(20.0));
    assert_eq!(reading.day_spent_usd, 17.5);
    assert_eq!(reading.day_limit_usd, Some(100.0));
    assert_eq!(reading.cost, CostNote::NoReader);
    assert_eq!(reading.at_ms, NOW);
}

#[test]
fn what_a_person_set_becomes_the_caps_of_the_allowance_and_nothing_set_is_no_cap() {
    let none = allowance_of(settings(Mode::Notify), 3.0, 9.0, 1.0, 4.0);
    assert_eq!(none, Allowance::default());
    let set = allowance_of(
        Settings {
            mode: Mode::Stop,
            task_usd: Some(20.0),
            day_usd: Some(100.0),
        },
        3.0,
        9.0,
        1.0,
        4.0,
    );
    assert_eq!(
        set.task,
        Some(Cap {
            limit_usd: 20.0,
            spent_usd: 3.0,
            ahead_usd: 1.0,
        }),
        "the task's projection is this worker's own"
    );
    assert_eq!(
        set.day,
        Some(Cap {
            limit_usd: 100.0,
            spent_usd: 9.0,
            ahead_usd: 4.0,
        }),
        "the day's is everything running"
    );
}

#[test]
fn the_ledgers_note_of_a_stop_names_the_budget_it_would_have_spent() {
    use zerocode_core::continue_gate::Reason;
    let judgement = Judgement {
        verdict: Verdict::Stop,
        reasons: vec![
            Reason {
                code: Code::ReworkLoop,
                value: 900.0,
                limit: 300.0,
            },
            Reason {
                code: Code::DayBudgetStop,
                value: 51.0,
                limit: 50.0,
            },
        ],
        metrics: zerocode_core::continue_gate::StepBook::default().metrics(),
    };
    assert_eq!(
        stop_reason(&judgement),
        "gate: day_budget_stop — $51.00 of $50.00"
    );
    let calm = Judgement {
        reasons: Vec::new(),
        ..judgement
    };
    assert_eq!(stop_reason(&calm), "gate");
}
