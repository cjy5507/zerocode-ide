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

/// Opens `wanted` connections to the helper one after another and leaves them
/// idle, so that as many requests in the same instant each find one. A helper's
/// listen queue holds eight, and what the window does when it is full — it
/// tries again — is another measurement's (`parallel_bench_at_once`): here no
/// request is to wait on it, whichever helper it reaches.
fn warm(session: &Session, wanted: usize) {
    let opened: Vec<_> = (0..wanted)
        .map(|_| {
            session
                .take_wire(super::QUEUE_FULL_WAIT)
                .unwrap_or_else(|_| panic!("a connection to the helper"))
        })
        .collect();
    session
        .idle
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .extend(opened);
}

/// How long the helper lets a pointer stay after its agent's last act: its own
/// number, read where the helper has it.
fn pointer_stays() -> Duration {
    let seconds = include_str!(
        "../../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/BackgroundPointerOverlay.swift"
    )
    .split("static let staysSeconds = ")
    .nth(1)
    .and_then(|rest| rest.lines().next())
    .and_then(|seconds| seconds.trim().parse().ok())
    .expect("how long the helper's pointer stays");
    Duration::from_secs(seconds)
}

/// Waits until the helper shows no pointer and has none left to draw. A pointer
/// stays for a while after its agent's last act, whatever became of its app, and
/// the helper's count is of every agent's pointers in every app: what one
/// section counts starts here, clear of the section before (2026-11-16: four
/// apps read five, the fifth the press before them had left). Waited out and
/// never stopped: what a stop costs is a section's own measure.
fn none_shown(session: &Session) {
    /// A timer's turn on a busy machine, past the pointer's own time.
    const LATE: Duration = Duration::from_secs(5);
    let bound = pointer_stays() + LATE;
    let started = Instant::now();
    loop {
        let (_, said) = call(session, "status", &json!({}));
        let status = said.expect("a status");
        let pointer = &status["backgroundPointer"];
        // The helper before says nothing of pointers waiting: it draws them at once.
        if pointer["pointers"].as_u64() == Some(0) && pointer["pending"].as_bool() != Some(true) {
            return;
        }
        assert!(
            started.elapsed() < bound,
            "a pointer outstays its time: {pointer}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
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
    // The Z key writes what the Mac's input source makes of it — `z`, or `ㅋ`
    // under 2-Set Korean — so the field is asked for the one character it
    // reached, not for a letter.
    let typed = field_now
        .as_deref()
        .and_then(|line| line.split("Value: ").nth(1))
        .and_then(|value| value.split_whitespace().next())
        .map(|value| value.chars().count());
    assert_eq!(
        typed,
        Some(1),
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

/// What a helper holds as macOS counts it, in KiB: its footprint — what the
/// Activity Monitor shows and memory pressure is charged for — and its
/// resident pages, which also count what it gave back and the system has not
/// yet taken. The resident size alone said 197 MiB of a helper whose windows
/// had gone (2026-11-16); both are read so the two are never taken for each
/// other. Zeros when the helper cannot be read.
fn helper_holds_kib(pid: Option<i32>) -> (u64, u64) {
    let Some(pid) = pid else { return (0, 0) };
    // SAFETY: zero is a valid `rusage_info_v4` (integers and a byte array).
    let mut info: libc::rusage_info_v4 = unsafe { std::mem::zeroed() };
    // SAFETY: the buffer is a live `rusage_info_v4`, the size the V4 flavor
    // writes; the call reads nothing else of ours.
    let read = unsafe {
        libc::proc_pid_rusage(
            pid,
            libc::RUSAGE_INFO_V4,
            std::ptr::from_mut(&mut info).cast(),
        )
    };
    if read != 0 {
        return (0, 0);
    }
    (info.ri_phys_footprint / 1024, info.ri_resident_size / 1024)
}

/// The same reading as the report writes it.
fn held(pid: Option<i32>) -> Value {
    let (footprint, resident) = helper_holds_kib(pid);
    json!({"footprint_kib": footprint, "resident_kib": resident})
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

/// What a press costs when the drawn pointer has to travel to it, how many
/// pointers one agent working in four apps shows, what many apps at once cost
/// with every pointer drawn, and how soon the helper's main thread hears a
/// stop while those pointers are drawn — a helper built from the commit
/// before against the one built here, turn about (2026-11-15: the press
/// waited for its pointer, 240 of 423 ms, and one agent had one pointer
/// however many apps). A count of pointers starts with none shown
/// (`none_shown`), and what each helper holds is read with four apps counted
/// and after the first and the last stop, so pointers drawn and cleared over
/// and over show what they leave behind. Build it in a folder named `target`:
/// a helper admits a test only from `target/debug/deps`, and the build line's
/// own folder is not one.
///
/// ```text
/// CARGO_TARGET_DIR=<checkout>/target ZEROCODE_PARALLEL_FIXTURE=… \
/// ZEROCODE_PARALLEL_BASELINE=<the helper built before> ZEROCODE_PARALLEL_BUILT=<the helper built here> \
/// ZEROCODE_PARALLEL_OUT=… \
/// cargo test -p zerocode-shell --bin zerocode-shell parallel_bench_pointer -- --ignored --nocapture
/// ```
#[test]
#[ignore = "drives the signed helper against owned fixtures; run by hand, see the module comment"]
fn parallel_bench_pointer() {
    const ROUNDS: usize = 12;
    const APPS: usize = 4;
    const MANY: usize = 24;
    const MANY_ROUNDS: usize = 6;
    const STOP_ROUNDS: usize = 24;
    /// How long after the presses set off the stop is sent, round by round.
    const STOP_AFTER_MS: [u64; 4] = [10, 20, 40, 80];
    let program = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_FIXTURE").expect("fixture"));
    let baseline = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BASELINE").expect("baseline"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_OUT").expect("report path"));
    let root = out.parent().expect("report folder").join("fixtures");
    let old = Session::connect(launch_at(&baseline).expect("baseline")).expect("baseline answers");
    let new = Session::connect(launch_at(&built).expect("built")).expect("built answers");
    let press_as = |session: &Session, app: &str, number: u64, agent: &str| {
        let (ms, answer) = call(
            session,
            "click",
            &json!({
                "app": app, "elementIndex": number, "background": true,
                "noScreenshot": true, "afterAct": "changes", "agent": agent,
            }),
        );
        answer.expect("a press");
        ms
    };
    // A number belongs to the agent whose look gave it: the look is the presser's.
    let look_as = |session: &Session, app: &str, agent: &str| {
        let (_, answer) = call(
            session,
            "getAppState",
            &json!({"app": app, "noScreenshot": true, "background": true, "agent": agent}),
        );
        answer.expect("a look")
    };
    // The pointers are drawn on the helper's main queue just after the answers:
    // the most it shows while they stay, and how long until it shows `wanted`.
    let pointers_shown = |session: &Session, wanted: usize| {
        let started = Instant::now();
        let mut most = 0;
        while most < wanted && started.elapsed() < Duration::from_millis(1_500) {
            let (_, said) = call(session, "status", &json!({}));
            let shown = said.expect("a status")["backgroundPointer"]["pointers"]
                .as_u64()
                .and_then(|shown| usize::try_from(shown).ok())
                .expect("a count of pointers");
            most = most.max(shown);
            if most < wanted {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let took = (most >= wanted).then(|| started.elapsed().as_secs_f64() * 1000.0);
        (most, took)
    };
    let held_by = |session: &Session| held(session.helper_pid());
    let mut report = json!({});

    // ---- a press whose pointer travels, and one whose pointer is already there
    // Eight rows keep the last button inside the window, so its pointer is drawn.
    let travel = Fixture::start(&program, &root, "travel", 8);
    let app = travel.app();
    let mut by_helper = serde_json::Map::new();
    let (mut far, mut same): (Vec<Vec<f64>>, Vec<Vec<f64>>) =
        (vec![vec![], vec![]], vec![vec![], vec![]]);
    let targets: Vec<(u64, u64)> = [&old, &new]
        .iter()
        .map(|session| {
            let state = look_as(session, &app, "bench");
            (
                number_of(&state, "button Target", 0),
                number_of(&state, "button Submit", 0),
            )
        })
        .collect();
    for round in 0..ROUNDS {
        for (at, session) in [&old, &new].into_iter().enumerate() {
            let (top, bottom) = targets[at];
            far[at].push(press_as(
                session,
                &app,
                if round % 2 == 0 { bottom } else { top },
                "bench",
            ));
        }
    }
    for _ in 0..ROUNDS {
        for (at, session) in [&old, &new].into_iter().enumerate() {
            same[at].push(press_as(session, &app, targets[at].0, "bench"));
        }
    }
    for (at, label) in ["before", "new"].into_iter().enumerate() {
        by_helper.insert(
            label.into(),
            json!({
                "press_to_a_far_point_ms": stats(std::mem::take(&mut far[at])),
                "press_on_the_same_point_ms": stats(std::mem::take(&mut same[at])),
            }),
        );
    }
    report["press_ms"] = Value::Object(by_helper);
    report["travel_pressed"] = json!({"events": travel.events().len(), "asked": ROUNDS * 4});
    drop(travel);

    // ---- one agent in four apps at once: how many pointers the helper shows
    let mut apps: Vec<Fixture> = (0..APPS)
        .map(|at| Fixture::start(&program, &root, &format!("pointer{at}"), 10))
        .collect();
    // Each in a place of its own, so a person sees each pointer apart.
    for (at, fixture) in apps.iter_mut().enumerate() {
        fixture.ctl(&format!("place {} 0", at * 150));
    }
    let mut pointers = serde_json::Map::new();
    // What each helper holds, moment by moment: (moment, before, new).
    let mut holds: Vec<(&str, Value, Value)> = Vec::new();
    for (label, session) in [("before", &old), ("new", &new)] {
        none_shown(session);
        let prepared: Vec<(String, u64)> = apps
            .iter()
            .map(|fixture| {
                let app = fixture.app();
                let state = look_as(session, &app, "one-agent");
                (app, number_of(&state, "button Target", 0))
            })
            .collect();
        std::thread::scope(|scope| {
            for (app, number) in &prepared {
                let press_as = &press_as;
                scope.spawn(move || press_as(session, app, *number, "one-agent"));
            }
        });
        pointers.insert(label.into(), json!(pointers_shown(session, APPS).0));
    }
    holds.push(("four_apps_counted", held_by(&old), held_by(&new)));
    report["one_agent_in_four_apps_pointers"] = Value::Object(pointers);
    report["four_apps_pressed"] = json!(
        apps.iter()
            .map(|fixture| fixture.events().len())
            .collect::<Vec<_>>()
    );
    drop(apps);

    // ---- one agent in many apps at once: every pointer drawn, every press timed.
    // Each round presses the other of two far buttons, so every pointer travels.
    let mut many: Vec<Fixture> = (0..MANY)
        .map(|at| Fixture::start(&program, &root, &format!("many{at}"), 8))
        .collect();
    // Six to a row, each in a place of its own: every pointer has its own way to go.
    for (at, fixture) in many.iter_mut().enumerate() {
        fixture.ctl(&format!("place {} {}", (at % 6) * 150, (at / 6) * 100));
    }
    let targets: Vec<Vec<(String, u64, u64)>> = [&old, &new]
        .iter()
        .map(|session| {
            many.iter()
                .map(|fixture| {
                    let app = fixture.app();
                    let state = look_as(session, &app, "one-agent");
                    (
                        app,
                        number_of(&state, "button Target", 0),
                        number_of(&state, "button Submit", 0),
                    )
                })
                .collect()
        })
        .collect();
    let (mut rounds, mut presses): (Vec<Vec<f64>>, Vec<Vec<f64>>) =
        (vec![vec![], vec![]], vec![vec![], vec![]]);
    let mut first_shown = [(0, None), (0, None)];
    for round in 0..MANY_ROUNDS {
        for (at, session) in [&old, &new].into_iter().enumerate() {
            if round == 0 {
                none_shown(session);
            }
            warm(session, MANY);
            let started = Instant::now();
            let took: Vec<f64> = std::thread::scope(|scope| {
                let mut pressing = Vec::with_capacity(MANY);
                for (app, top, bottom) in &targets[at] {
                    let press_as = &press_as;
                    let number = if round % 2 == 0 { *bottom } else { *top };
                    pressing.push(scope.spawn(move || press_as(session, app, number, "one-agent")));
                }
                pressing
                    .into_iter()
                    .map(|press| press.join().expect("a press"))
                    .collect()
            });
            rounds[at].push(started.elapsed().as_secs_f64() * 1000.0);
            presses[at].extend(took);
            if round == 0 {
                first_shown[at] = pointers_shown(session, MANY);
            }
        }
    }
    let mut by_helper = serde_json::Map::new();
    for (at, (label, session)) in [("before", &old), ("new", &new)].into_iter().enumerate() {
        let (pointers, all_shown_ms) = first_shown[at];
        by_helper.insert(
            label.into(),
            json!({
                "round_ms": stats(std::mem::take(&mut rounds[at])),
                "press_ms": stats(std::mem::take(&mut presses[at])),
                "pointers": pointers,
                "all_pointers_shown_ms": all_shown_ms.map(f64::round),
                "helper_holds": held_by(session),
            }),
        );
    }
    report["one_agent_in_many_apps"] = json!({
        "apps": MANY, "rounds": MANY_ROUNDS, "by_helper": by_helper,
    });
    report["many_apps_pressed"] = json!(
        many.iter()
            .map(|fixture| fixture.events().len())
            .collect::<Vec<_>>()
    );

    // ---- how soon the helper's main thread hears a stop while many pointers
    // are being drawn (t-109788). The person's stop chord is heard on that
    // thread, and the chord is theirs to press: the window's own stop is sent
    // instead. The helper takes it on a queue of its own and then clears the
    // pointers on the main thread, behind whatever that thread is doing — the
    // wait the chord would have. Timed from the signal to the first pointer
    // gone; the stop is lifted after, as the window lifts its own.
    let stop_heard = |session: &Session, targets: &[(String, u64, u64)], round: usize| {
        let pid = session.helper_pid().expect("the helper's pid");
        let pointers = || {
            let (_, said) = call(session, "status", &json!({}));
            said.ok()
                .and_then(|said| said["backgroundPointer"]["pointers"].as_u64())
                .unwrap_or(0)
        };
        warm(session, MANY + 2);
        let heard = std::thread::scope(|scope| {
            for (app, top, bottom) in targets {
                let number = if round.is_multiple_of(2) {
                    *bottom
                } else {
                    *top
                };
                // A press the stop catches is refused: its answer is no part of this.
                scope.spawn(move || {
                    let _ = call(
                        session,
                        "click",
                        &json!({
                            "app": app, "elementIndex": number, "background": true,
                            "noScreenshot": true, "afterAct": "changes", "agent": "one-agent",
                        }),
                    );
                });
            }
            std::thread::sleep(Duration::from_millis(
                STOP_AFTER_MS[round % STOP_AFTER_MS.len()],
            ));
            let mut shown = pointers();
            let waited = Instant::now();
            while shown == 0 && waited.elapsed() < Duration::from_millis(500) {
                std::thread::sleep(Duration::from_millis(2));
                shown = pointers();
            }
            if shown == 0 {
                return None;
            }
            let sent = Instant::now();
            // SAFETY: a signal to the helper this bench started — the one the
            // window's stop sends it.
            let signalled = unsafe { libc::kill(pid, libc::SIGUSR1) };
            assert_eq!(signalled, 0, "the stop signal");
            loop {
                let now = pointers();
                if now < shown {
                    return Some(sent.elapsed().as_secs_f64() * 1000.0);
                }
                shown = now;
                if sent.elapsed() > Duration::from_secs(5) {
                    return None;
                }
            }
        });
        let (_, lifted) = call(session, "resume", &json!({"lifter": "person"}));
        lifted.expect("the stop is lifted");
        std::thread::sleep(Duration::from_millis(250));
        heard
    };
    let (mut heard, mut untimed): (Vec<Vec<f64>>, [u32; 2]) = (vec![vec![], vec![]], [0; 2]);
    for round in 0..STOP_ROUNDS {
        for (at, session) in [&old, &new].into_iter().enumerate() {
            match stop_heard(session, &targets[at], round) {
                Some(ms) => heard[at].push(ms),
                None => untimed[at] += 1,
            }
        }
        // A stop clears every pointer, and each round after draws them again:
        // what a helper holds after the first round against after the last is
        // what the rounds between left behind, with no pointer shown at either.
        let moment = match round + 1 {
            1 => "first_stop_lifted",
            STOP_ROUNDS => "last_stop_lifted",
            _ => continue,
        };
        none_shown(&old);
        none_shown(&new);
        holds.push((moment, held_by(&old), held_by(&new)));
    }
    let mut by_helper = serde_json::Map::new();
    for (at, label) in ["before", "new"].into_iter().enumerate() {
        // Every round's, slowest last: twenty-four are too few for a p95 to
        // show the slowest one.
        let mut all = std::mem::take(&mut heard[at]);
        all.sort_by(f64::total_cmp);
        let all: Vec<f64> = all.iter().map(|ms| (ms * 10.0).round() / 10.0).collect();
        by_helper.insert(
            label.into(),
            json!({
                "heard_ms": stats(all.clone()),
                "longest_ms": all.last().copied().unwrap_or_default(),
                "all_ms": all,
                "not_timed": untimed[at],
            }),
        );
    }
    report["a_stop_while_many_pointers_are_drawn"] = json!({
        "apps": MANY, "rounds": STOP_ROUNDS, "stop_after_ms": STOP_AFTER_MS, "by_helper": by_helper,
    });
    report["helper_holds_at"] = Value::Object(
        holds
            .into_iter()
            .map(|(moment, before, new)| (moment.to_owned(), json!({"before": before, "new": new})))
            .collect(),
    );
    drop(many);

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

/// Many requests to one helper in the same instant, as the groups of one
/// `parallel` make them (t-109501). The helper's queue holds eight
/// connections it has not taken yet, and a connection it refuses must cost no
/// request and no session. Sixteen apps, then sixty-four, each pressed by a
/// thread of its own, all let go at once, round after round. Build it in a
/// folder named `target`: the helper admits a test only from there.
///
/// ```text
/// CARGO_TARGET_DIR=<checkout>/target ZEROCODE_PARALLEL_FIXTURE=… \
/// ZEROCODE_PARALLEL_BUILT=<the helper built here> ZEROCODE_PARALLEL_OUT=… \
/// cargo test -p zerocode-shell --bin zerocode-shell parallel_bench_at_once -- --ignored --nocapture
/// ```
#[test]
#[ignore = "drives the signed helper against owned fixtures; run by hand, see the module comment"]
fn parallel_bench_at_once() {
    const ROUNDS: usize = 5;
    let program = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_FIXTURE").expect("fixture"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_OUT").expect("report path"));
    let root = out
        .parent()
        .expect("report folder")
        .join("fixtures-at-once");
    let session = Session::connect(launch_at(&built).expect("built")).expect("built answers");
    let mut report = json!({});
    for apps in [16_usize, 64] {
        let fixtures: Vec<Fixture> = (0..apps)
            .map(|at| Fixture::start(&program, &root, &format!("at-once-{apps}-{at}"), 8))
            .collect();
        let prepared: Vec<(String, u64)> = fixtures
            .iter()
            .map(|fixture| {
                let app = fixture.app();
                let number = number_of(&look(&session, &app), "button Target", 0);
                (app, number)
            })
            .collect();
        let (mut rounds, mut presses, mut refused) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..ROUNDS {
            let gate = std::sync::Barrier::new(apps);
            let started = Instant::now();
            let said: Vec<(f64, Result<Value, String>)> = std::thread::scope(|scope| {
                let mut pressing = Vec::with_capacity(apps);
                for (app, number) in &prepared {
                    let (session, gate) = (&session, &gate);
                    pressing.push(scope.spawn(move || {
                        gate.wait();
                        call(
                            session,
                            "click",
                            &json!({
                                "app": app, "elementIndex": number, "background": true,
                                "noScreenshot": true, "afterAct": "changes",
                            }),
                        )
                    }));
                }
                pressing
                    .into_iter()
                    .map(|press| press.join().expect("a press"))
                    .collect()
            });
            rounds.push(started.elapsed().as_secs_f64() * 1000.0);
            for (ms, answer) in said {
                match answer {
                    Ok(_) => presses.push(ms),
                    Err(why) => refused.push(why),
                }
            }
        }
        let landed: usize = fixtures.iter().map(|fixture| fixture.events().len()).sum();
        refused.sort();
        refused.dedup();
        report[format!("{apps}_at_once")] = json!({
            "rounds": ROUNDS,
            "asked": apps * ROUNDS,
            "answered": presses.len(),
            "landed": landed,
            "refusals": refused,
            "round_ms": stats(rounds),
            "press_ms": stats(presses),
        });
        drop(fixtures);
    }
    // A request the session answers after all of it says the session was never discarded.
    let (_, still) = call(&session, "status", &json!({}));
    report["session_still_answers"] = json!(still.is_ok());
    report["helper_pid"] = json!(session.helper_pid());

    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

/// The page the front bench presses: a button that counts its presses in the
/// window's title, and a field. Its words are its own, so a tree line that
/// reads them is the page's.
const FRONT_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>ZC front</title></head>
<body style="font: 24px -apple-system; padding: 40px">
<button id="t" style="font-size: 24px; padding: 12px 24px">ZC-TARGET</button>
<p><input id="f" aria-label="ZC-FIELD" style="font-size: 24px"></p>
<script>let n = 0; document.getElementById("t").onclick = () => { n += 1; document.title = "ZC count " + n; };</script>
</body></html>
"#;

/// Chrome for Testing on the bench's own page, with an empty profile of the
/// bench's own: none of the person's sign-ins, tabs or history. It comes to the
/// front as it opens, which is the place this bench measures, and it is quit
/// when the bench ends. Nothing is deleted: each run opens in a new folder of
/// its own, and the folders are left for the person to say what goes.
struct TestBrowser {
    pid: i32,
}

impl TestBrowser {
    fn open(app: &Path, root: &Path) -> Self {
        assert!(
            !root.exists(),
            "a run opens in a folder of its own: {}",
            root.display()
        );
        let profile = root.join("profile");
        std::fs::create_dir_all(&profile).expect("a profile folder");
        let page = root.join("page.html");
        std::fs::write(&page, FRONT_PAGE).expect("the page");
        let opened = crate::proc::quiet_command("/usr/bin/open")
            .arg("-n")
            .arg("-a")
            .arg(app)
            .arg("--args")
            .arg(format!("--user-data-dir={}", profile.display()))
            // Playwright's own: no keychain prompt, no first-run or choice screens, nothing fetched in the background.
            .args([
                "--no-first-run",
                "--no-default-browser-check",
                "--use-mock-keychain",
                "--disable-search-engine-choice-screen",
                "--disable-background-networking",
                "--disable-component-update",
                "--disable-default-apps",
                "--disable-extensions",
                "--disable-sync",
                "--no-service-autorun",
                "--force-renderer-accessibility",
                "--new-window",
            ])
            .arg(format!("file://{}", page.display()))
            .status()
            .expect("open");
        assert!(opened.success(), "Chrome for Testing did not open");
        let name = app
            .file_stem()
            .expect("the app's name")
            .to_string_lossy()
            .to_string();
        // The browser's own process, not its helpers: the main executable, then this profile.
        let pattern = format!(
            "Contents/MacOS/{name} --user-data-dir={}",
            profile.display()
        );
        let started = Instant::now();
        let pid = loop {
            let found = crate::proc::quiet_command("/usr/bin/pgrep")
                .arg("-f")
                .arg(&pattern)
                .output()
                .expect("pgrep");
            if let Some(pid) = String::from_utf8_lossy(&found.stdout)
                .lines()
                .next()
                .and_then(|line| line.trim().parse::<i32>().ok())
            {
                break pid;
            }
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "Chrome for Testing never came up"
            );
            std::thread::sleep(Duration::from_millis(100));
        };
        let started = Instant::now();
        let mut asked_again = false;
        while front_pid() != Some(pid) {
            if !asked_again && started.elapsed() > Duration::from_secs(5) {
                // It came up behind: ask for it once more, by its own app.
                let _ = crate::proc::quiet_command("/usr/bin/open")
                    .arg("-a")
                    .arg(app)
                    .status();
                asked_again = true;
            }
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "Chrome for Testing did not come to the front"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        Self { pid }
    }
}

impl Drop for TestBrowser {
    fn drop(&mut self) {
        let pid = self.pid.to_string();
        let _ = crate::proc::quiet_command("/bin/kill").arg(&pid).status();
        let started = Instant::now();
        while crate::proc::quiet_command("/bin/kill")
            .args(["-0", pid.as_str()])
            .status()
            .is_ok_and(|alive| alive.success())
            && started.elapsed() < Duration::from_secs(10)
        {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// The process of the app in front, by the system's own word.
fn front_pid() -> Option<i32> {
    let asn = front_app();
    let said = crate::proc::quiet_command("/usr/bin/lsappinfo")
        .args(["info", "-only", "pid", asn.as_str()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&said.stdout)
        .rsplit('=')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// A look at the app in front once its page has drawn `words` into the tree.
/// A look reads; it is not asked to stay in the background.
fn front_look_with(session: &Session, app: &str, words: &str) -> Value {
    let started = Instant::now();
    loop {
        let (_, answer) = call(
            session,
            "getAppState",
            &json!({"app": app, "noScreenshot": true}),
        );
        if let Ok(state) = answer
            && state["snapshot"]["treeText"]
                .as_str()
                .is_some_and(|tree| tree.contains(words))
        {
            return state;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the page's {words} never showed in the tree"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// How many presses reached the page: the count its title shows, whatever
/// the window adds after it.
fn page_count(title: &str) -> u64 {
    title
        .split("ZC count ")
        .nth(1)
        .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|count| count.parse().ok())
        .unwrap_or(0)
}

/// The window's title once the page has counted `pressed` presses, or as it
/// stands after three seconds: a title follows its page a moment later.
fn settled_title(session: &Session, app: &str, pressed: u64) -> String {
    let started = Instant::now();
    loop {
        let title = front_look_with(session, app, "ZC-FIELD")["snapshot"]["window"]["title"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if page_count(&title) >= pressed || started.elapsed() > Duration::from_secs(3) {
            return title;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// One helper's turn on the page in front: the button pressed `presses`
/// times, then a written value, then the two acts that stay refused in front
/// (a field's press, a key). Stops at once when the person comes back or
/// another app comes to the front.
fn front_turn(session: &Session, app: &str, pid: i32, presses: usize) -> Value {
    let mut rows = vec![];
    let mut person_back = false;
    for _ in 0..presses {
        if person_idle_secs() < 2 || front_pid() != Some(pid) {
            person_back = true;
            break;
        }
        // Each press goes on a fresh look, as the tool's own presses do after
        // every act: a press changes the page's title, and a number read on
        // an older look is refused once the tree around it moved.
        let target = number_of(&front_look_with(session, app, "ZC-FIELD"), "ZC-TARGET", 0);
        let pointer = pointer_at(session);
        let (ms, answer) = call(
            session,
            "click",
            &json!({"app": app, "elementIndex": target, "background": true, "noScreenshot": true, "afterAct": "none"}),
        );
        let now = pointer_at(session);
        let moved = (now.0 - pointer.0).abs().max((now.1 - pointer.1).abs());
        let front_kept = front_pid() == Some(pid);
        rows.push(match answer {
            Ok(result) => json!({
                "ms": ms.round(), "path": result.pointer("/action/path"), "action": result.pointer("/action/actionName"),
                "delivery": result.pointer("/action/delivery"), "moved": moved, "frontKept": front_kept,
            }),
            Err(refused) => json!({"ms": ms.round(), "refused": refused, "moved": moved, "frontKept": front_kept}),
        });
        std::thread::sleep(Duration::from_millis(150));
    }
    let sent = rows
        .iter()
        .filter(|row| row.get("refused").is_none())
        .count();
    let title = settled_title(session, app, u64::try_from(sent).expect("a count"));
    let field = number_of(&front_look_with(session, app, "ZC-FIELD"), "ZC-FIELD", 0);
    let act = |method: &str, extra: Value| {
        let mut params =
            json!({"app": app, "background": true, "noScreenshot": true, "afterAct": "none"});
        for (key, value) in extra.as_object().expect("an object") {
            params[key] = value.clone();
        }
        call(session, method, &params).1.err()
    };
    let wrote = act(
        "setValue",
        json!({"elementIndex": field, "value": "zc front"}),
    );
    let field_press = act("click", json!({"elementIndex": field}));
    let key = act("pressKey", json!({"key": "a"}));
    let mut ms: Vec<f64> = rows
        .iter()
        .filter(|row| row.get("refused").is_none())
        .filter_map(|row| row["ms"].as_f64())
        .collect();
    ms.sort_by(f64::total_cmp);
    json!({
        "pressed": rows.iter().filter(|row| row.get("refused").is_none()).count(),
        "pressMsP50": ms.get(ms.len() / 2),
        "maxMoved": rows.iter().filter_map(|row| row["moved"].as_f64()).fold(0.0, f64::max),
        "frontKept": rows.iter().all(|row| row["frontKept"] == true),
        "title": title,
        "writeRefused": wrote,
        "fieldPressRefused": field_press,
        "keyRefused": key,
        "personBack": person_back,
        "rows": rows,
    })
}

/// Helper item 1 of the five the person approved on 2026-11-17 22:3x: the
/// app's own press, and a written value, in the app the person has in front.
/// Measured on Chrome for Testing — a browser of its own, an empty profile,
/// none of the person's sign-ins — in front only while the person is away:
/// each turn stops the moment they come back. The installed helper (the one
/// before) and the built one take a turn each on the same button: before, a
/// press in front is refused `requires_foreground`; after, it goes by the
/// app's own action, the pointer where it was and the app still in front, and
/// a field's press and a key stay refused.
///
/// ```text
/// ZEROCODE_FRONT_CHROME=<…>/Google\ Chrome\ for\ Testing.app \
/// ZEROCODE_PARALLEL_BASELINE=/Applications/ZeroCode.app/Contents/Resources/ZeroCode\ Computer\ Use.app \
/// ZEROCODE_PARALLEL_BUILT=<worktree>/crates/zerocode-shell/native/computer-use-macos/.build/release/ZeroCode\ Computer\ Use.app \
/// ZEROCODE_FRONT_OUT=<dir>/front-report.json \
/// cargo test -p zerocode-shell --bin zerocode-shell front_press_bench -- --ignored --nocapture
/// ```
#[test]
#[ignore = "brings Chrome for Testing to the front; run by hand while the person is away"]
fn front_press_bench() {
    const AWAY_SECS: u64 = 60;
    const PRESSES: usize = 10;
    let idle = person_idle_secs();
    assert!(
        idle >= AWAY_SECS,
        "the person acted {idle} s ago: this bench waits for {AWAY_SECS} s away"
    );
    let chrome =
        PathBuf::from(std::env::var_os("ZEROCODE_FRONT_CHROME").expect("Chrome for Testing"));
    let before =
        PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BASELINE").expect("the helper before"));
    let built =
        PathBuf::from(std::env::var_os("ZEROCODE_PARALLEL_BUILT").expect("the built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_FRONT_OUT").expect("report path"));
    // Both helpers answer before anything comes to the front: one that does
    // not costs the person's away time nothing.
    let sessions = [("before", &before), ("after", &built)].map(|(side, helper)| {
        let session = Session::connect(launch_at(helper).expect("helper"))
            .unwrap_or_else(|error| panic!("the {side} helper does not answer: {error:?}"));
        (side, session)
    });
    // The run's own folder, beside its report: the page and the browser's profile.
    let browser = TestBrowser::open(&chrome, &out.with_extension("run"));
    let app = format!("pid:{}", browser.pid);
    let mut report = json!({"presses": PRESSES});
    for (side, session) in &sessions {
        report[*side] = front_turn(session, &app, browser.pid, PRESSES);
        if report[*side]["personBack"] == true {
            break;
        }
    }
    drop(browser);
    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
    if report["before"]["personBack"] == true || report["after"]["personBack"] == true {
        // The person came back: the bench stood back, and says so rather than judge.
        return;
    }
    let (was, now) = (&report["before"], &report["after"]);
    let refused_in_front = |value: &Value| {
        value
            .as_str()
            .is_some_and(|said| said.starts_with("requires_foreground"))
    };
    assert_eq!(
        was["pressed"], 0,
        "the helper before refuses a press in front"
    );
    assert_eq!(now["pressed"], PRESSES, "every press in front went");
    assert!(
        now["maxMoved"].as_f64().unwrap_or(f64::MAX) < 1.0,
        "the pointer stayed where it was"
    );
    assert_eq!(now["frontKept"], true, "the app stayed in front");
    assert_eq!(
        page_count(now["title"].as_str().unwrap_or_default()),
        u64::try_from(PRESSES).expect("a count"),
        "the page counted every press"
    );
    assert!(
        now["writeRefused"].is_null(),
        "a written value goes in front: {}",
        now["writeRefused"]
    );
    assert!(
        refused_in_front(&now["fieldPressRefused"]),
        "a field's press stays refused in front: {}",
        now["fieldPressRefused"]
    );
    assert!(
        refused_in_front(&now["keyRefused"]),
        "a key stays refused in front: {}",
        now["keyRefused"]
    );
}

#[test]
fn the_front_page_counts_in_its_title() {
    assert_eq!(page_count("ZC count 10"), 10);
    assert_eq!(page_count("ZC count 7 - Google Chrome for Testing"), 7);
    assert_eq!(page_count("ZC front"), 0);
    assert!(FRONT_PAGE.contains("ZC-TARGET") && FRONT_PAGE.contains("ZC-FIELD"));
}
