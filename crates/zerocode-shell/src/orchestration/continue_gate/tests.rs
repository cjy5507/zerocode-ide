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
        started_ms: NOW,
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
        call: None,
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

/// A measurement, not a test (t-26583): what the gate costs the window on the
/// paths a slow machine feels, with the numbers printed — the hook road's one
/// event, the beat's judgment of many workers, a transcript look that finds
/// nothing new and one that finds a kilobyte-sized call, and what a pane's record
/// weighs. Run it on purpose:
///
/// `cargo test -p zerocode-shell --bin zerocode-shell measure_the_gate -- --ignored --nocapture`
///
/// (and under `taskpolicy -b` for the efficiency-core profile).
#[test]
#[ignore = "a measurement; it prints its numbers"]
fn measure_the_gate_on_the_paths_a_slow_machine_feels() {
    use std::io::Write as _;
    use std::time::Instant;

    use crate::orchestration::gate_book::{self, PaneGate};
    use crate::orchestration::gate_meter::{Meter, POLL_MS};

    let per = |total: std::time::Duration, count: u32| total.as_nanos() as f64 / f64::from(count);

    // The hook road: one tool event of one pane into the window's book, the lock
    // and the hash and all.
    let payload = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"cargo test -p zerocode-core --lib -- case_7","description":"Run the core tests"}}"#;
    let events = 200_000_u32;
    let call_started = activity(Phase::Started);
    let started = Instant::now();
    for index in 0..events {
        gate_book::note_hook(
            90_000,
            "launch",
            &call_started,
            payload,
            NOW + i64::from(index),
        );
    }
    eprintln!(
        "hook road: {:.0} ns per tool event through gate_book::note_hook ({events} events)",
        per(started.elapsed(), events)
    );

    // The beat: every live worker judged and recorded, a full window and a full ring
    // of costs each.
    for workers in [1_u32, 10, 50] {
        let door = FakeDoor::default();
        let mut book = GateBook::default();
        for term in 0..workers {
            let gate = book.pane(term, "launch", NOW);
            work(gate, 150);
            for index in 0..64 {
                gate.book
                    .note_cost(Some(0.15 + f64::from(index % 7) * 0.01));
            }
        }
        let beats = 2_000_u32;
        let started = Instant::now();
        for beat in 0..beats {
            for term in 0..workers {
                let gate = book.pane_mut(term).expect("a record");
                step(
                    gate,
                    &pane(),
                    settings(Mode::Notify),
                    &around(Allowance::default()),
                    &door,
                    NOW + i64::from(beat) * SECOND,
                );
            }
        }
        let elapsed = started.elapsed();
        eprintln!(
            "beat: {:.0} µs for {workers} workers judged and recorded ({:.0} ns a worker)",
            per(elapsed, beats) / 1_000.0,
            per(elapsed, beats * workers)
        );
    }

    // A transcript look: nothing grew (a stat), and a growth of one model call.
    let dir = tempfile::tempdir().expect("a transcript dir");
    let path = dir.path().join("session.jsonl");
    let call = |index: usize| {
        serde_json::json!({
            "type": "assistant", "sessionId": "session-1", "timestamp": "2026-10-03T00:00:00.000Z",
            "uuid": format!("uuid-{index}"), "requestId": format!("request-{index}"),
            "message": {"id": format!("message-{index}"), "model": "claude-haiku-4-5", "role": "assistant",
                "content": [{"type": "text", "text": "Reading the file before I change it."}],
                "usage": {"input_tokens": 3, "output_tokens": 120, "cache_read_input_tokens": 90_000,
                    "cache_creation_input_tokens": 400}},
        })
        .to_string()
    };
    std::fs::write(&path, format!("{}\n", call(0))).expect("a transcript");
    let text = path.to_str().expect("a utf-8 path");
    let mut meter = Meter::new("claude");
    meter.poll(Some(text), 0);
    let looks = 20_000_u32;
    let started = Instant::now();
    for look in 1..=looks {
        meter.poll(Some(text), i64::from(look) * POLL_MS);
    }
    eprintln!(
        "transcript look, nothing new: {:.1} µs (an open, a stat and a flush, {looks} looks)",
        per(started.elapsed(), looks) / 1_000.0
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("the transcript opens");
    let mut grown = 0_u32;
    let mut spent = std::time::Duration::ZERO;
    for index in 1..=2_000_usize {
        writeln!(file, "{}", call(index)).expect("the transcript grows");
        let at = i64::from(looks + 1) * POLL_MS + i64::try_from(index).expect("fits") * POLL_MS;
        let started = Instant::now();
        meter.poll(Some(text), at);
        spent += started.elapsed();
        grown += 1;
    }
    eprintln!(
        "transcript look, one model call grown: {:.1} µs ({grown} looks)",
        per(spent, grown) / 1_000.0
    );

    // What a record weighs in the book.
    eprintln!(
        "records: PaneGate {} bytes on the stack; a book of {} panes is bounded by it and the heap of one \
         window of {} marks and one ring of {} costs",
        std::mem::size_of::<PaneGate>(),
        gate_book::PANES_MAX,
        zerocode_core::continue_gate::REWORK_WINDOW_STEPS,
        zerocode_core::continue_gate::COST_RING
    );
}

#[test]
fn a_worker_whose_attempt_began_before_the_window_did_is_met_already_running() {
    let dir = tempfile::tempdir().expect("a transcript dir");
    let path = dir.path().join("session.jsonl");
    let call = |index: usize| {
        serde_json::json!({
            "type": "assistant", "sessionId": "session-1", "timestamp": "2026-10-03T00:00:00.000Z",
            "uuid": format!("uuid-{index}"), "requestId": format!("request-{index}"),
            "message": {"id": format!("message-{index}"), "model": "claude-haiku-4-5", "role": "assistant",
                "usage": {"input_tokens": 1_000_000, "output_tokens": 0, "cache_read_input_tokens": 0,
                    "cache_creation_input_tokens": 0}},
        })
        .to_string()
    };
    let lines: Vec<String> = (0..3).map(call).collect();
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).expect("a transcript");
    let text = path.to_str().expect("a utf-8 path");

    let mut running = meter_for("claude", NOW - 1, NOW);
    let met = running.poll(Some(text), 0);
    assert!(
        met.counted.is_empty() && !met.known.is_empty(),
        "an attempt that began before the window did was already spending: {met:?}"
    );

    let mut watched = meter_for("claude", NOW + 1, NOW);
    let seen = watched.poll(Some(text), 0);
    assert_eq!(
        seen.counted.len(),
        2,
        "watched from its start, every finished call counts"
    );
    assert!(seen.known.is_empty());
}
