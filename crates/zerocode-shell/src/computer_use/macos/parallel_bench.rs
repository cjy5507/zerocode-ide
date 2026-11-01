//! What the helper does for many agents at once, measured against the helper the
//! machine has installed. Run by hand, on an owned fixture window that never takes
//! focus: semantic presses only, no pointer, no keys, no card.
//!
//! ```text
//! swiftc -O tools/computer-bench/ParallelFixture.swift -o <dir>/fixture -framework AppKit
//! ZEROCODE_PARALLEL_FIXTURE=<dir>/fixture \
//! ZEROCODE_PARALLEL_BASELINE=/Applications/ZeroCode.app/Contents/Resources/ZeroCode\ Computer\ Use.app \
//! ZEROCODE_PARALLEL_BUILT=target/debug/ZeroCode\ Computer\ Use.app \
//! ZEROCODE_PARALLEL_OUT=<dir>/report.json \
//! cargo test -p zerocode-shell --bin zerocode-shell parallel_bench -- --ignored --nocapture
//! ```
//!
//! The two helpers are measured turn about in one run, so the numbers compare the
//! helpers and not the machine at two moments.

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Session, launch_at};
use crate::computer_use::session::{ProviderSession as _, SessionFailure};

struct Fixture {
    process: Child,
    directory: PathBuf,
    acks: u64,
}

impl Fixture {
    fn start(program: &Path, root: &Path, name: &str, rows: usize) -> Self {
        let directory = root.join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let process = crate::proc::quiet_command(program)
            .arg(rows.to_string())
            .arg(&directory)
            .arg(name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("fixture");
        let started = Instant::now();
        while !directory.join("ready").exists() {
            assert!(started.elapsed() < Duration::from_secs(10), "fixture ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(250));
        Self {
            process,
            directory,
            acks: 0,
        }
    }

    fn app(&self) -> String {
        format!("pid:{}", self.process.id())
    }

    fn ctl(&mut self, line: &str) {
        let tmp = self.directory.join("ctl.tmp");
        std::fs::write(&tmp, line).expect("control");
        std::fs::rename(&tmp, self.directory.join("ctl")).expect("publish control");
        let started = Instant::now();
        loop {
            let said = std::fs::read_to_string(self.directory.join("ack"))
                .ok()
                .and_then(|ack| ack.trim().parse::<u64>().ok())
                .unwrap_or(0);
            if said > self.acks {
                self.acks = said;
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "ack for {line}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        // The app redraws and rebuilds its accessibility objects after a change.
        std::thread::sleep(Duration::from_millis(120));
    }

    fn events(&self) -> Vec<String> {
        std::fs::read_to_string(self.directory.join("events.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn call(session: &Session, method: &str, params: &Value) -> (f64, Result<Value, String>) {
    let started = Instant::now();
    let answer = session.request(method, params.clone());
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    (
        ms,
        answer.map_err(|failure| match failure {
            SessionFailure::Provider(error) | SessionFailure::Transport(error) => {
                format!("{}: {}", error.code, error.message)
            }
        }),
    )
}

fn stats(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    let at = |fraction: f64| {
        samples
            .get(((samples.len() as f64 - 1.0) * fraction).round() as usize)
            .copied()
            .unwrap_or_default()
    };
    json!({
        "n": samples.len(),
        "p50": (at(0.5) * 10.0).round() / 10.0,
        "p95": (at(0.95) * 10.0).round() / 10.0,
    })
}

fn look(session: &Session, app: &str) -> Value {
    let (_, answer) = call(
        session,
        "getAppState",
        &json!({"app": app, "noScreenshot": true, "background": true}),
    );
    answer.expect("a look")
}

/// The number on the tree line that reads `words`, the `nth` from the top.
fn number_of(state: &Value, words: &str, nth: usize) -> u64 {
    state["snapshot"]["treeText"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| line.contains(words))
        .nth(nth)
        .and_then(|line| line.split(' ').next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("no line reads {words}"))
}

fn press(session: &Session, app: &str, number: u64) -> Value {
    let (ms, answer) = call(
        session,
        "click",
        &json!({
            "app": app, "elementIndex": number, "background": true,
            "noScreenshot": true, "afterAct": "changes",
        }),
    );
    match answer {
        Ok(result) => json!({
            "ms": ms.round(),
            "delivery": result.pointer("/action/delivery"),
            "effect": result.pointer("/action/effect"),
            "changes": result.get("changes"),
        }),
        Err(refused) => json!({"ms": ms.round(), "refused": refused}),
    }
}

/// What one helper holds as a long session goes on: the same presses on the same
/// window, from a fresh process.
#[test]
#[ignore = "drives the signed helper against an owned fixture; run by hand, see the module comment"]
fn parallel_bench_memory() {
    let program = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_FIXTURE").expect("fixture"));
    let baseline = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BASELINE").expect("baseline"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_OUT").expect("report path"));
    let root = out.parent().expect("report folder").join("fixtures");
    let mut report = json!({});
    for (label, helper, after) in [
        ("installed", &baseline, None),
        ("new", &built, Some("changes")),
    ] {
        let session = Session::connect(launch_at(helper).expect("helper")).expect("answers");
        let fixture = Fixture::start(&program, &root, &format!("memory-{label}"), 1000);
        let app = fixture.app();
        let number = number_of(&look(&session, &app), "button Target", 0);
        let mut params = json!({"app": app, "elementIndex": number, "background": true, "noScreenshot": true, "timing": true});
        if let Some(after) = after {
            params["afterAct"] = after.into();
        }
        let mut rss = vec![helper_rss_kib(session.helper_pid())];
        let (mut acts, mut in_the_app) = (vec![], vec![]);
        for round in 1..=150 {
            let (_, answer) = call(&session, "click", &params);
            let answer = answer.expect("a press");
            if let Some(ms) = answer.pointer("/action/actMs").and_then(Value::as_f64) {
                acts.push(ms);
            }
            if let Some(ms) = answer
                .pointer("/action/appActionMs")
                .and_then(Value::as_f64)
            {
                in_the_app.push(ms);
            }
            if round % 50 == 0 {
                rss.push(helper_rss_kib(session.helper_pid()));
            }
        }
        report[label] = json!({"rss_kib_at_0_50_100_150_presses": rss, "act_ms": stats(acts), "in_the_app_ms": stats(in_the_app)});
    }
    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

/// How long ago the person last touched the mouse or the keyboard, in seconds.
fn person_idle_secs() -> u64 {
    let out = crate::proc::quiet_command("/usr/sbin/ioreg")
        .args(["-c", "IOHIDSystem"])
        .output()
        .expect("ioreg");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|line| line.contains("\"HIDIdleTime\""))
        .and_then(|line| line.split('=').nth(1))
        .and_then(|nanos| nanos.trim().parse::<u64>().ok())
        .map_or(0, |nanos| nanos / 1_000_000_000)
}

fn front_app() -> String {
    let out = crate::proc::quiet_command("/usr/bin/lsappinfo")
        .arg("front")
        .output()
        .expect("lsappinfo");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn pointer_at(session: &Session) -> (f64, f64) {
    let (_, at) = call(session, "cursorPosition", &json!({}));
    let at = at.expect("the pointer");
    (at["x"].as_f64().expect("x"), at["y"].as_f64().expect("y"))
}

/// What one borrowed act left behind: its answer, and whether the person's
/// pointer and front app came back.
fn borrowed_act(session: &Session, method: &str, params: &Value) -> Value {
    let (pointer, front) = (pointer_at(session), front_app());
    let (ms, answer) = call(session, method, params);
    let back = pointer_at(session);
    let moved = (back.0 - pointer.0).abs().max((back.1 - pointer.1).abs());
    // Bringing the app back is asked of the system; give it a moment to answer.
    let settled = Instant::now();
    while front_app() != front && settled.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(50));
    }
    let front_back = front_app() == front;
    assert!(moved < 1.0, "{method} left the pointer {moved} away");
    assert!(front_back, "{method} left another app in front");
    match answer {
        Ok(result) => json!({
            "ms": ms.round(),
            "borrowed": result.pointer("/action/borrowedDesktop"),
            "path": result.pointer("/action/path"),
            "delivery": result.pointer("/action/delivery"),
            "pointerBack": true,
            "frontBack": true,
        }),
        Err(refused) => json!({
            "ms": ms.round(),
            "refused": refused,
            "pointerBack": true,
            "frontBack": true,
        }),
    }
}

/// Real input, on owned windows only, and only while the person is away: an act
/// borrows the pointer and the keyboard, another agent's background presses go on
/// beside it, and the person's pointer and front app come back. Under a minute.
#[test]
#[ignore = "takes the real pointer on an owned fixture; run by hand while the person is away"]
fn parallel_bench_borrow() {
    const AWAY_SECS: u64 = 120;
    let idle = person_idle_secs();
    assert!(
        idle >= AWAY_SECS,
        "the person acted {idle} s ago: real input waits for {AWAY_SECS} s away"
    );
    let program = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_FIXTURE").expect("fixture"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_OUT").expect("report path"));
    let root = out.parent().expect("report folder").join("fixtures");
    let session = Session::connect(launch_at(&built).expect("helper")).expect("answers");
    let borrowed = Fixture::start(&program, &root, "borrowed", 30);
    let beside = Fixture::start(&program, &root, "beside", 30);
    let (app, other) = (borrowed.app(), beside.app());
    let state = look(&session, &app);
    let (target, field) = (
        number_of(&state, "button Target", 0),
        number_of(&state, "text field", 0),
    );
    let other_target = number_of(&look(&session, &other), "button Target", 0);

    let (presses, report) = std::thread::scope(|scope| {
        let side = scope.spawn(|| {
            let mut answers = vec![];
            for _ in 0..10 {
                answers.push(press(&session, &other, other_target));
                std::thread::sleep(Duration::from_millis(100));
            }
            answers
        });
        let borrow = |extra: Value| {
            let mut params = json!({
                "app": app, "noScreenshot": true, "afterAct": "none",
                "mayBorrowDesktop": true, "borrowApp": app,
            });
            for (key, value) in extra.as_object().expect("an object") {
                params[key] = value.clone();
            }
            params
        };
        // A double press has no semantic road, so it is the real pointer's.
        let pointer = borrowed_act(
            &session,
            "click",
            &borrow(json!({"elementIndex": target, "clickCount": 2})),
        );
        // Background first: this one the background refuses, so it borrows.
        let ladder = borrowed_act(
            &session,
            "drag",
            &borrow(json!({
                "fromElementIndex": target, "toElementIndex": field, "background": true,
            })),
        );
        // A key goes to the fixture's own process, wherever the focus has gone by then.
        let focus = borrowed_act(&session, "click", &borrow(json!({"elementIndex": field})));
        let keyboard = borrowed_act(&session, "pressKey", &borrow(json!({"key": "z"})));
        let report =
            json!({"pointer": pointer, "ladder": ladder, "focus": focus, "keyboard": keyboard});
        (side.join().expect("the other agent"), report)
    });
    let refused = presses
        .iter()
        .filter(|one| one.get("refused").is_some())
        .count();
    let delivered = beside
        .events()
        .iter()
        .filter(|line| *line == "target")
        .count();
    let pressed = borrowed
        .events()
        .iter()
        .filter(|line| *line == "target")
        .count();
    let field_now = look(&session, &app)["snapshot"]["treeText"]
        .as_str()
        .unwrap_or_default()
        .lines()
        .find(|line| line.contains("text field"))
        .map(str::to_string);
    assert!(
        field_now
            .as_deref()
            .is_some_and(|line| line.contains("Value: z")),
        "the borrowed key reached the fixture's field: {field_now:?}"
    );
    let report = json!({
        "idleSecsAtStart": idle,
        "acts": report,
        "windowSawRealPress": pressed,
        "fieldAfterTyping": field_now,
        "beside": {"presses": presses.len(), "refused": refused, "landed": delivered},
    });
    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

fn helper_rss_kib(pid: Option<i32>) -> u64 {
    let Some(pid) = pid else { return 0 };
    crate::proc::quiet_command("/bin/ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()
        .and_then(|out| String::from_utf8_lossy(&out.stdout).trim().parse().ok())
        .unwrap_or(0)
}

#[test]
#[ignore = "drives the signed helper against an owned fixture; run by hand, see the module comment"]
#[allow(clippy::too_many_lines)]
fn parallel_bench() {
    let program = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_FIXTURE").expect("fixture"));
    let baseline = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BASELINE").expect("baseline"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_OUT").expect("report path"));
    let root = out.parent().expect("report folder").join("fixtures");
    let built = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BUILT").expect("built helper"));
    let old = Session::connect(launch_at(&baseline).expect("baseline")).expect("baseline answers");
    let new = Session::connect(launch_at(&built).expect("built")).expect("built answers");
    let mut report = json!({});

    // ---- what one look and one press cost, turn about
    let mut fixture = Fixture::start(&program, &root, "cost", 10);
    let app = fixture.app();
    let mut by_size = serde_json::Map::new();
    for rows in [10_usize, 100, 500, 1000] {
        fixture.ctl(&format!("rows {rows}"));
        let (old_state, new_state) = (look(&old, &app), look(&new, &app));
        let old_target = number_of(&old_state, "button Target", 0);
        let new_target = number_of(&new_state, "button Target", 0);
        let (mut old_look, mut new_look, mut old_press, mut new_press) =
            (vec![], vec![], vec![], vec![]);
        let (mut old_find, mut new_find) = (vec![], vec![]);
        let (mut old_bytes, mut new_bytes) = (0, 0);
        let look_params = json!({"app": app, "noScreenshot": true, "background": true});
        let find_params =
            json!({"app": app, "text": "Target", "noScreenshot": true, "background": true});
        for _ in 0..9 {
            old_look.push(call(&old, "getAppState", &look_params).0);
            new_look.push(call(&new, "getAppState", &look_params).0);
            old_find.push(call(&old, "findElements", &find_params).0);
            new_find.push(call(&new, "findElements", &find_params).0);
            let (ms, answer) = call(
                &old,
                "click",
                &json!({"app": app, "elementIndex": old_target, "background": true, "noScreenshot": true}),
            );
            old_press.push(ms);
            old_bytes = answer.expect("old press").to_string().len();
            let (ms, answer) = call(
                &new,
                "click",
                &json!({"app": app, "elementIndex": new_target, "background": true, "noScreenshot": true, "afterAct": "changes"}),
            );
            new_press.push(ms);
            new_bytes = answer.expect("new press").to_string().len();
        }
        let elements = new_state["snapshot"]["elementCount"].clone();
        by_size.insert(
            rows.to_string(),
            json!({
                "elements": elements,
                "look_ms": {"installed": stats(old_look), "new": stats(new_look)},
                "find_ms": {"installed": stats(old_find), "new": stats(new_find)},
                "press_ms": {"installed": stats(old_press), "new": stats(new_press)},
                "press_answer_bytes": {"installed": old_bytes, "new": new_bytes},
            }),
        );
    }
    report["by_size"] = Value::Object(by_size);
    report["pressed_all"] = json!({"events": fixture.events().len()});
    drop(fixture);

    // ---- many at once
    let mut big = Fixture::start(&program, &root, "big", 1000);
    let small = Fixture::start(&program, &root, "small", 10);
    let (big_app, small_app) = (big.app(), small.app());
    let big_look = json!({"app": big_app, "noScreenshot": true, "background": true});
    let small_look = json!({"app": small_app, "noScreenshot": true, "background": true});
    let measure_side_by_side = |session: &Session| {
        let alone: Vec<f64> = (0..5)
            .map(|_| call(session, "getAppState", &small_look).0)
            .collect();
        let beside: Vec<f64> = (0..5)
            .map(|_| {
                std::thread::scope(|scope| {
                    let slow = scope.spawn(|| call(session, "getAppState", &big_look).0);
                    std::thread::sleep(Duration::from_millis(15));
                    let quick = call(session, "getAppState", &small_look).0;
                    let _ = slow.join();
                    quick
                })
            })
            .collect();
        json!({"small_look_alone": stats(alone), "small_look_beside_a_big_look": stats(beside)})
    };
    report["beside_a_big_look"] =
        json!({"installed": measure_side_by_side(&old), "new": measure_side_by_side(&new)});

    let hung_measure = |session: &Session, big: &mut Fixture| {
        big.ctl("rows 10");
        look(session, &big_app);
        let started = Instant::now();
        big.ctl_no_wait("hang 8000");
        std::thread::sleep(Duration::from_millis(60));
        let result = std::thread::scope(|scope| {
            let stuck = scope.spawn(|| {
                let (ms, answer) = call(session, "getAppState", &big_look);
                (ms, answer.err().unwrap_or_else(|| "answered".into()))
            });
            std::thread::sleep(Duration::from_millis(150));
            let other = call(session, "getAppState", &small_look).0;
            let (stuck_ms, stuck_said) = stuck.join().expect("stuck look");
            json!({"look_at_hung_app_ms": stuck_ms.round(), "said": stuck_said, "other_app_look_ms": other.round()})
        });
        // Let the app wake before the next helper asks it anything.
        std::thread::sleep(Duration::from_millis(8100).saturating_sub(started.elapsed()));
        big.settle_after_hang();
        result
    };
    report["hung_app"] = json!({
        "app_hung_for_ms": 8000,
        "installed": hung_measure(&old, &mut big),
        "new": hung_measure(&new, &mut big),
    });
    drop(big);
    drop(small);

    // ---- several apps pressed together
    let apps: Vec<Fixture> = (0..3)
        .map(|at| Fixture::start(&program, &root, &format!("app{at}"), 100))
        .collect();
    let prepared: Vec<(String, u64)> = apps
        .iter()
        .map(|fixture| {
            let app = fixture.app();
            let number = number_of(&look(&new, &app), "button Target", 0);
            (app, number)
        })
        .collect();
    let run = |together: bool| {
        let started = Instant::now();
        let press_all = |(app, number): &(String, u64)| {
            for _ in 0..20 {
                let (_, answer) = call(
                    &new,
                    "click",
                    &json!({"app": app, "elementIndex": number, "background": true, "noScreenshot": true, "afterAct": "changes"}),
                );
                answer.expect("a press");
            }
        };
        if together {
            std::thread::scope(|scope| {
                let press_all = &press_all;
                for target in &prepared {
                    scope.spawn(move || press_all(target));
                }
            });
        } else {
            prepared.iter().for_each(press_all);
        }
        started.elapsed().as_secs_f64() * 1000.0
    };
    let in_turn = run(false);
    let together = run(true);
    report["three_apps_20_presses_each_ms"] =
        json!({"one_after_another": in_turn.round(), "at_once": together.round()});
    report["three_apps_pressed"] = json!(
        apps.iter()
            .map(|fixture| fixture.events().len())
            .collect::<Vec<_>>()
    );
    drop(apps);

    // ---- pressing the right thing
    let row_of = |state: &Value, nth: usize| -> (u64, u64) {
        let tree = state["snapshot"]["treeText"].as_str().unwrap_or_default();
        let lines: Vec<&str> = tree.lines().map(str::trim).collect();
        let at = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains("button Delete"))
            .nth(nth)
            .map(|(at, _)| at)
            .expect("a row");
        let row = lines[at - 1]
            .split(' ')
            .next_back()
            .and_then(|row| row.parse().ok())
            .expect("a row number");
        let number = lines[at]
            .split(' ')
            .next()
            .and_then(|number| number.parse().ok())
            .expect("an element number");
        (row, number)
    };
    let mut accuracy = serde_json::Map::new();
    for (label, session) in [("installed", &old), ("new", &new)] {
        let mut rows = Fixture::start(&program, &root, &format!("rows-{label}"), 10);
        let app = rows.app();
        let state = look(session, &app);
        let (row, number) = row_of(&state, 3);
        rows.ctl(&format!("remove {row}"));
        let before = rows.events().len();
        let reply = press(session, &app, number);
        let removed = json!({"looked_at_row": row, "reply": reply, "the_window_recorded": rows.events()[before..]});

        rows.ctl("rows 10");
        let state = look(session, &app);
        let (row, number) = row_of(&state, 5);
        rows.ctl("insert-top");
        let before = rows.events().len();
        let reply = press(session, &app, number);
        let arrived = json!({"looked_at_row": row, "reply": reply, "the_window_recorded": rows.events()[before..]});

        rows.ctl("rows 10");
        let state = look(session, &app);
        let flaky = number_of(&state, "Flaky", 0);
        let before = rows.events().len();
        let reply = press(session, &app, flaky);
        let disabled = json!({"reply": reply, "the_window_recorded": rows.events()[before..]});
        rows.ctl("enable-flaky 400");
        let before = rows.events().len();
        let reply = press(session, &app, flaky);
        let turning_on = json!({"reply": reply, "the_window_recorded": rows.events()[before..]});

        rows.ctl("rows 100");
        let state = look(session, &app);
        let (row, number) = row_of(&state, 90);
        let before = rows.events().len();
        let reply = press(session, &app, number);
        let below =
            json!({"row": row, "reply": reply, "the_window_recorded": rows.events()[before..]});
        accuracy.insert(
            label.to_owned(),
            json!({
                "row_removed": removed,
                "row_arrived_above": arrived,
                "disabled": disabled,
                "enabled_400ms_later": turning_on,
                "below_the_fold": below,
            }),
        );
    }
    report["accuracy"] = Value::Object(accuracy);

    // ---- what the helper holds after a long session
    let hold = Fixture::start(&program, &root, "hold", 1000);
    let app = hold.app();
    let target_old = number_of(&look(&old, &app), "button Target", 0);
    let target_new = number_of(&look(&new, &app), "button Target", 0);
    for _ in 0..60 {
        let _ = call(
            &old,
            "click",
            &json!({"app": app, "elementIndex": target_old, "background": true, "noScreenshot": true}),
        );
        let _ = call(
            &new,
            "click",
            &json!({"app": app, "elementIndex": target_new, "background": true, "noScreenshot": true, "afterAct": "changes"}),
        );
    }
    report["helper_rss_kib_after_60_presses_on_1000_rows"] = json!({
        "installed": helper_rss_kib(old.helper_pid()),
        "new": helper_rss_kib(new.helper_pid()),
    });
    drop(hold);

    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

impl Fixture {
    /// Send a line and do not wait: the app is about to stop answering.
    fn ctl_no_wait(&self, line: &str) {
        let tmp = self.directory.join("ctl.tmp");
        std::fs::write(&tmp, line).expect("control");
        std::fs::rename(&tmp, self.directory.join("ctl")).expect("publish control");
    }

    fn settle_after_hang(&mut self) {
        self.acks = std::fs::read_to_string(self.directory.join("ack"))
            .ok()
            .and_then(|ack| ack.trim().parse::<u64>().ok())
            .unwrap_or(self.acks);
    }
}
