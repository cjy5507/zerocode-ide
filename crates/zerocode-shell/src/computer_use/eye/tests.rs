use std::cell::RefCell;
use std::collections::VecDeque;

use serde_json::{Value, json};
use zerocode_core::computer_use::{EYE_FRAMES_PER_SECOND, EYE_POLL_MS};
use zerocode_core::computer_use_protocol::error_code;

use super::*;

/// ZeroCode's pane on the left, Mail on the right — under the Dock's window,
/// which on macOS 26 spans the whole screen at the Dock's layer and is an
/// overlay (this Mac, 2026-09-12).
fn windows() -> Value {
    json!({ "windows": [
        { "id": 11, "app": { "name": "Dock", "pid": 30 }, "x": 0, "y": 0, "width": 1512, "height": 982, "own": false, "layer": 20, "alpha": 1.0, "overlay": true },
        { "id": 1, "app": { "name": "ZeroCode", "pid": 10 }, "x": 0, "y": 0, "width": 700, "height": 900, "own": true, "layer": 0, "alpha": 1.0, "overlay": false },
        { "id": 2, "app": { "name": "Mail", "pid": 20 }, "x": 700, "y": 0, "width": 800, "height": 900, "own": false, "layer": 0, "alpha": 1.0, "overlay": false },
    ] })
}

fn repaint(seq: u64, at_ms: i64, rect: [f64; 4]) -> Value {
    json!({ "seq": seq, "atMs": at_ms, "rects": [rect] })
}

fn stream(seq: u64, now_ms: i64, changes: Vec<Value>) -> Value {
    json!({ "streaming": true, "streamId": "original-eye", "seq": seq, "nowMs": now_ms, "whole": true, "changes": changes })
}

fn interrupted_streams() -> Vec<Value> {
    let mut lost = stream(12, 1_200, Vec::new());
    lost["whole"] = false.into();
    let restarted = stream(1, 1_200, Vec::new());
    let mut replaced = stream(20, 1_200, Vec::new());
    replaced["streamId"] = "replacement-eye".into();
    vec![lost, restarted, replaced]
}

#[test]
fn an_ocr_wait_reads_after_history_loss_and_reanchors_to_the_current_stream() {
    for interrupted in interrupted_streams() {
        let mut next = interrupted.clone();
        next["whole"] = true.into();
        next["nowMs"] = 1_300.into();
        let script = Script::new().changes(vec![
            stream(10, 900, Vec::new()),
            stream(10, 1_000, Vec::new()),
            interrupted.clone(),
            next,
        ]);
        let memory = Memory::new();
        let mut call = script.call();
        let mut gate = Gate::new(&memory, &json!({}), &mut call).unwrap();
        assert!(gate.read(&mut call));
        assert!(
            gate.read(&mut call),
            "lost observations require fresh text: {interrupted}"
        );
        assert!(
            !gate.read(&mut call),
            "the repaired cursor can reuse unchanged text again"
        );
        drop(call);
        assert_eq!(
            script.params(CHANGES_METHOD).last().unwrap()[AFTER_KEY],
            interrupted[SEQ_KEY]
        );
    }
}

#[test]
fn a_watch_never_reports_quiet_or_change_from_a_lost_interval() {
    for interrupted in interrupted_streams() {
        for until in ["quiet", "change"] {
            let script =
                Script::new().changes(vec![stream(10, 1_000, Vec::new()), interrupted.clone()]);
            let memory = Memory::new();
            let error = watch(
                &memory,
                &json!({ "until": until, "timeoutMs": 500 }),
                &mut script.call(),
                &mut |_| {},
            )
            .unwrap_err();
            assert_eq!(error.code, HISTORY_LOST);
        }
    }
}

#[test]
fn settling_falls_back_to_real_observations_after_history_loss() {
    for interrupted in interrupted_streams() {
        let script = Script::new().changes(vec![stream(10, 1_000, Vec::new()), interrupted]);
        assert!(settle(&Memory::new(), None, &mut script.call(), &mut |_| {}).is_none());
    }
}

#[test]
fn a_watch_still_times_out_when_the_provider_clock_is_frozen() {
    let mut asks = 0;
    let mut call = |method: &str, _: Value| match method {
        CHANGES_METHOD => {
            asks += 1;
            assert!(
                asks <= 10,
                "a frozen provider clock must not keep the watch alive"
            );
            Ok(stream(10, 1_000, Vec::new()))
        }
        "listAllWindows" => Ok(windows()),
        other => panic!("unexpected {other}"),
    };
    let error = watch(
        &Memory::new(),
        &json!({ "timeoutMs": EYE_POLL_MS * 2 }),
        &mut call,
        &mut std::thread::sleep,
    )
    .unwrap_err();
    assert_eq!(error.code, error_code::TIMEOUT);
}

/// A helper from a script: each `eyeChanges` takes the next answer; every
/// ask is written down with its params.
struct Script {
    changes: RefCell<VecDeque<Result<Value, ComputerUseError>>>,
    frame: RefCell<VecDeque<Result<Value, ComputerUseError>>>,
    start: RefCell<VecDeque<Result<Value, ComputerUseError>>>,
    /// The window lists `listAllWindows` answers in turn; past them, `windows()`.
    lists: RefCell<VecDeque<Value>>,
    asked: RefCell<Vec<(String, Value)>>,
}

impl Script {
    fn new() -> Self {
        Self {
            changes: RefCell::new(VecDeque::new()),
            frame: RefCell::new(VecDeque::new()),
            start: RefCell::new(VecDeque::new()),
            lists: RefCell::new(VecDeque::new()),
            asked: RefCell::new(Vec::new()),
        }
    }

    fn changes(self, answers: Vec<Value>) -> Self {
        self.changes
            .borrow_mut()
            .extend(answers.into_iter().map(Ok));
        self
    }

    fn lists(self, lists: Vec<Value>) -> Self {
        self.lists.borrow_mut().extend(lists);
        self
    }

    fn call(&self) -> impl FnMut(&str, Value) -> Result<Value, ComputerUseError> + '_ {
        move |method, params| {
            self.asked.borrow_mut().push((method.to_string(), params));
            let next = |queue: &RefCell<VecDeque<Result<Value, ComputerUseError>>>| {
                queue
                    .borrow_mut()
                    .pop_front()
                    .unwrap_or_else(|| panic!("{method} asked past the script"))
            };
            match method {
                "eyeChanges" => next(&self.changes),
                "eyeFrame" => next(&self.frame),
                "eyeStart" => next(&self.start),
                "listAllWindows" => Ok(self.lists.borrow_mut().pop_front().unwrap_or_else(windows)),
                other => panic!("unexpected {other}"),
            }
        }
    }

    fn methods(&self) -> Vec<String> {
        self.asked
            .borrow()
            .iter()
            .map(|(method, _)| method.clone())
            .collect()
    }

    fn params(&self, method: &str) -> Vec<Value> {
        self.asked
            .borrow()
            .iter()
            .filter(|(asked, _)| asked == method)
            .map(|(_, params)| params.clone())
            .collect()
    }
}

fn not_watching() -> ComputerUseError {
    ComputerUseError::new(NOT_WATCHING, "no eye is open on that display")
}

#[test]
fn a_look_opens_the_eye_once_with_the_tables_numbers_and_reads_its_newest_frame() {
    let memory = Memory::new();
    let script = Script::new();
    let shot = json!({ "screenshot": { "data": "", "width": 1280, "height": 831, "scale": 0.85 }, "seq": 3 });
    script
        .frame
        .borrow_mut()
        .extend([Err(not_watching()), Ok(shot.clone()), Ok(shot.clone())]);
    script
        .start
        .borrow_mut()
        .push_back(Ok(stream(1, 0, Vec::new())));
    let mut call = script.call();
    assert_eq!(frame(&memory, None, &mut call), Some(shot.clone()));
    assert_eq!(frame(&memory, None, &mut call), Some(shot));
    drop(call);
    assert_eq!(
        script.methods(),
        ["eyeFrame", "eyeStart", "eyeFrame", "eyeFrame"]
    );
    let started = &script.params("eyeStart")[0];
    assert_eq!(started["framesPerSecond"], EYE_FRAMES_PER_SECOND);
    assert!(
        started.get("display").is_none(),
        "the main display unless one is named"
    );
}

#[test]
fn a_provider_without_the_eye_is_asked_once_per_idle_time() {
    let memory = Memory::new();
    let script = Script::new();
    script
        .frame
        .borrow_mut()
        .push_back(Err(ComputerUseError::new(
            error_code::UNSUPPORTED_CAPABILITY,
            "unknown method eyeFrame",
        )));
    let mut call = script.call();
    assert_eq!(frame(&memory, None, &mut call), None);
    assert_eq!(frame(&memory, None, &mut call), None, "not asked again");
    let mut pause = |_: Duration| panic!("no wait without the eye");
    assert_eq!(settle(&memory, None, &mut call, &mut pause), None);
    assert!(Gate::new(&memory, &json!({}), &mut call).is_none());
    drop(call);
    assert_eq!(script.methods(), ["eyeFrame"]);

    // An eye that would not open (screen recording not granted) is asked for
    // again after the idle time; the looks meanwhile capture as before.
    let memory = Memory::new();
    let script = Script::new();
    script.frame.borrow_mut().push_back(Err(not_watching()));
    script
        .start
        .borrow_mut()
        .push_back(Err(ComputerUseError::new(
            error_code::PERMISSION_DENIED,
            "screen recording is not granted",
        )));
    let mut call = script.call();
    assert_eq!(frame(&memory, None, &mut call), None);
    assert!(memory.refused_lately(), "not asked for again at once");
}

#[test]
fn the_look_after_an_act_asks_at_the_tables_pace_until_the_core_says_look() {
    let memory = Memory::new();
    let act = json!({ "seq": 10, "atMs": 990 });
    let mut status = stream(10, 1_000, Vec::new());
    status["act"] = act;
    let script = Script::new().changes(vec![
        status,
        // Mail repaints 10 ms after the act; ZeroCode's pane scrolls.
        stream(
            12,
            1_010,
            vec![
                repaint(11, 1_000, [800.0, 100.0, 300.0, 200.0]),
                repaint(12, 1_005, [10.0, 800.0, 600.0, 18.0]),
            ],
        ),
        stream(12, 1_060, Vec::new()),
        stream(
            13,
            1_110,
            vec![repaint(13, 1_108, [10.0, 800.0, 600.0, 18.0])],
        ),
    ]);
    let mut pauses = Vec::new();
    let mut call = script.call();
    let settled = settle(&memory, None, &mut call, &mut |pause| pauses.push(pause));
    drop(call);
    assert_eq!(
        settled,
        Some(Settled {
            settled: true,
            waited_ms: 110,
            polls: 3
        }),
        "still since 1 000 ms — the pane's line at 1 108 ms is ZeroCode's own"
    );
    assert_eq!(pauses, [Duration::from_millis(EYE_POLL_MS); 2]);
    let asked = script.params("eyeChanges");
    assert!(
        asked[0].get("after").is_none() && asked[0].get("fromMs").is_none(),
        "where it stands"
    );
    assert_eq!(
        asked[1]["fromMs"],
        990 - i64::try_from(EYE_BACKGROUND_MS).unwrap(),
        "back to what was already moving"
    );
    assert_eq!(
        (asked[2]["after"].as_u64(), asked[3]["after"].as_u64()),
        (Some(12), Some(12))
    );
    assert_eq!(settled.unwrap().answer()["settled"], true);
}

#[test]
fn a_watch_answers_where_the_screen_changed_or_says_how_long_it_waited() {
    let memory = Memory::new();
    let script = Script::new().changes(vec![
        stream(5, 2_000, Vec::new()),
        stream(5, 2_010, Vec::new()),
        stream(
            6,
            2_035,
            vec![repaint(6, 2_030, [800.0, 100.0, 50.0, 20.0])],
        ),
    ]);
    let mut call = script.call();
    let watched = watch(
        &memory,
        &json!({ "timeoutMs": 5_000 }),
        &mut call,
        &mut |_| {},
    )
    .expect("changed");
    drop(call);
    assert_eq!(watched["satisfied"], true);
    assert_eq!(watched["until"], "change");
    assert_eq!(watched["atMs"], 30);
    assert_eq!(
        watched["regions"],
        json!([{ "x": 800.0, "y": 100.0, "width": 50, "height": 20 }])
    );
    assert_eq!(watched["polls"], 2);

    let memory = Memory::new();
    let still: Vec<Value> = (0..10).map(|at| stream(5, 25 * at, Vec::new())).collect();
    let script = Script::new().changes(still);
    let mut call = script.call();
    let late = watch(
        &memory,
        &json!({ "timeoutMs": 100 }),
        &mut call,
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(late.code, error_code::TIMEOUT);
    assert!(late.message.contains("did not change"), "{}", late.message);
    let refused = watch(
        &memory,
        &json!({ "until": "still" }),
        &mut call,
        &mut |_| {},
    )
    .unwrap_err();
    assert_eq!(refused.code, error_code::INVALID_ARGUMENT);
}

#[test]
fn an_ocr_wait_reads_again_only_after_a_repaint_that_is_not_zerocodes() {
    let memory = Memory::new();
    let script = Script::new().changes(vec![
        stream(4, 100, Vec::new()), // the gate opens
        stream(4, 110, Vec::new()), // first reading: where it stands
        stream(4, 360, Vec::new()), // nothing painted
        stream(5, 610, vec![repaint(5, 600, [10.0, 10.0, 300.0, 18.0])]), // the pane's own line
        stream(6, 860, vec![repaint(6, 850, [900.0, 400.0, 200.0, 30.0])]), // Mail's text
    ]);
    script
        .changes
        .borrow_mut()
        .push_back(Err(ComputerUseError::new(
            "action_timeout",
            "the helper was slow",
        )));
    let mut call = script.call();
    let mut gate = Gate::new(&memory, &json!({}), &mut call).expect("the eye is open");
    let reads: Vec<bool> = (0..5).map(|_| gate.read(&mut call)).collect();
    assert_eq!(
        reads,
        [true, false, false, true, true],
        "the last: the eye could not say"
    );
    assert_eq!(gate.reads, 3);
}

/// The wait began with ZeroCode filling the screen; Mail then came forward
/// over it and painted. Judged by the list the wait began with, Mail's paint
/// was ZeroCode's own and the text waited for would never be read.
#[test]
fn a_wait_judges_repaints_by_the_windows_as_they_stand_now() {
    let memory = Memory::new();
    let zerocode = json!({ "id": 1, "app": { "name": "ZeroCode", "pid": 10 }, "x": 0, "y": 0, "width": 1512, "height": 982, "own": true, "layer": 0, "alpha": 1.0 });
    let mail = json!({ "id": 2, "app": { "name": "Mail", "pid": 20 }, "x": 100, "y": 100, "width": 800, "height": 600, "own": false, "layer": 0, "alpha": 1.0 });
    let script = Script::new()
        .lists(vec![
            json!({ "windows": [zerocode.clone()] }),
            json!({ "windows": [mail, zerocode] }),
        ])
        .changes(vec![
            stream(4, 100, Vec::new()), // the gate opens
            stream(4, 110, Vec::new()), // first reading: where it stands
            stream(5, 360, vec![repaint(5, 350, [200.0, 200.0, 300.0, 20.0])]),
        ]);
    let mut call = script.call();
    let mut gate = Gate::new(&memory, &json!({}), &mut call).expect("the eye is open");
    assert!(gate.read(&mut call), "the first reading");
    assert!(
        gate.read(&mut call),
        "Mail's paint is read, though ZeroCode was in front there when the wait began"
    );
    drop(call);
    assert_eq!(
        script.params("listAllWindows").len(),
        2,
        "listed again once the list was as old as the quiet"
    );
}

#[test]
fn a_desktop_reading_hands_the_helper_what_zerocode_shows() {
    let memory = Memory::new();
    let script = Script::new().changes(vec![stream(4, 100, Vec::new())]);
    let mut call = script.call();
    let asked = reading_with(&memory, &json!({ "ocr": true }), &mut call);
    assert_eq!(
        asked[OWN_REGION_KEY],
        json!([[0.0, 0.0, 700.0, 900.0]]),
        "the pane, the Dock's overlay notwithstanding"
    );
    assert_eq!(asked["ocr"], true);
    let app = json!({ "ocr": true, "app": "Mail" });
    assert_eq!(
        reading_with(&memory, &app, &mut call),
        app,
        "an app's window is read whole"
    );
    let tree = json!({ "app": "Mail" });
    assert_eq!(reading_with(&memory, &tree, &mut call), tree);
    drop(call);
    assert_eq!(
        script.methods(),
        ["eyeChanges", "listAllWindows"],
        "the eye kept open, the windows listed now"
    );
}

#[test]
fn an_ocr_wait_over_a_region_the_eye_does_not_show_reads_every_time() {
    let memory = Memory::new();
    let mut standing = stream(4, 100, Vec::new());
    standing["display"] =
        json!({ "index": 0, "bounds": { "x": 0, "y": 0, "width": 1512, "height": 982 } });
    let script = Script::new().changes(vec![standing.clone(), standing]);
    let mut call = script.call();
    let on_it = json!({ "region": { "x": 100, "y": 100, "width": 400, "height": 200 } });
    assert!(Gate::new(&memory, &on_it, &mut call).is_some());
    let beside = json!({ "region": { "x": 1600, "y": 100, "width": 400, "height": 200 } });
    assert!(
        Gate::new(&memory, &beside, &mut call).is_none(),
        "a region on the display right of the main one is not the main eye's to judge"
    );
}

#[test]
fn repaints_become_the_areas_a_diff_answers() {
    let regions = super::super::compare::rect_regions(&[
        Rect::new(100.0, 100.0, 30.0, 10.0),
        Rect::new(120.0, 104.0, 30.0, 10.0),
        Rect::new(-500.0, 600.0, 10.0, 10.0),
    ]);
    assert_eq!(
        regions.len(),
        2,
        "the two touching repaints are one area: {regions:?}"
    );
    assert!(
        regions.iter().any(|region| {
            let number = |key: &str| region[key].as_f64().unwrap();
            number("x") <= -500.0
                && number("y") <= 600.0
                && number("x") + number("width") >= -490.0
                && number("y") + number("height") >= 610.0
        }),
        "a display left of the main one keeps its negative points, on the diff's grid: {regions:?}"
    );
    assert!(super::super::compare::rect_regions(&[]).is_empty());
}
