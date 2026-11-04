//! The bench of the road from inside (t-90894), end to end on an iOS simulator with
//! the patched WebDriverAgent. `tools/real-phone-bench/run.sh` builds what it needs
//! and runs it; run by hand it takes:
//!
//! - `ZC_PHONE_BENCH_SIM`: a booted simulator's UDID
//! - `ZC_PHONE_BENCH_PLAN`: the `.xctestrun` a build-for-testing wrote for it
//! - `ZC_PHONE_BENCH_SIM2`: a second booted simulator, for two devices at once
//! - `ZC_PHONE_BENCH_OUT`: the JSON report's path
//!
//! It goes through the verbs agents use (marks, click, text, tree, find), so what it
//! times is the road, and never reads the person's phone. `step` is one agent step,
//! a press and the next screen's number, by a press and `marks` and by a press's
//! preview (t-93254).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::computer_use::EmulatorPlatform;
use zerocode_core::computer_use_protocol::marks::LOOK_ID_KEY;

use super::runner::{self, Life, watched};
use super::*;
use crate::emulator::marks::{self, Device};

const MESSAGE: &str = "안녕하세요 시험 메시지입니다 abc 123";
const SWEEP_ROUNDS: usize = 20;
const TIMED_ROUNDS: usize = 11;

struct SimRoad {
    port: u16,
}

impl Road for SimRoad {
    fn allowed(&self) -> bool {
        true
    }
    fn installed(&self) -> bool {
        true
    }
    fn base(&self) -> Result<String, String> {
        Ok(words::runner_base("127.0.0.1", self.port))
    }
    fn locked_roads(&self) -> LockedRoads {
        locked_roads_now()
    }
    /// A simulator has no lock `devicectl` reads; the runner's answer stands.
    fn locked_now(&self) -> Option<bool> {
        None
    }
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(future)
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required"))
}

fn sim_phone(handle: &str, udid: &str, port: u16, plan: &str) -> Arc<Life> {
    let udid = udid.to_string();
    let plan = plan.to_string();
    let spawn: runner::Spawn = Arc::new(move || {
        let mut command = watched("/usr/bin/xcrun", &{
            let mut args = vec!["xcodebuild".to_string()];
            args.extend(supply::run_args(std::path::Path::new(&plan), &udid));
            args
        });
        // A simulator's runner listens on the Mac itself, and says so.
        let bound = "127.0.0.1".to_string();
        command.env("TEST_RUNNER_USE_PORT", port.to_string());
        command.env(words::RUNNER_BIND_ENV, &bound);
        Ok(runner::Started {
            child: command.spawn()?,
            bound,
        })
    });
    let life = Life::new(
        spawn,
        Arc::new(|| false),
        Duration::from_secs(600),
        Vec::new(),
    );
    register(handle, Arc::new(SimRoad { port }), Arc::clone(&life)).expect("registered");
    life
}

fn device(handle: &str) -> Device {
    Device {
        address: handle.to_string(),
        identity: handle.to_string(),
    }
}

/// The mark whose legend line holds any of these words.
/// The number of the first legend line `pick` takes.
fn mark_where(answer: &Value, pick: &dyn Fn(&str) -> bool) -> Option<usize> {
    let legend = answer["legend"].as_str()?;
    let line = legend.lines().find(|line| pick(line))?;
    line.chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn mark_of(answer: &Value, names: &[&str]) -> Option<usize> {
    mark_where(answer, &|line| names.iter().any(|name| line.contains(name)))
}

/// A legend line's centre, `… @x,y`.
fn centre_of(line: &str) -> Option<(f64, f64)> {
    let (x, y) = line.rsplit_once('@')?.1.split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

const FIELDS: &[&str] = &["text field", "text view"];

fn is_field(line: &str) -> bool {
    FIELDS.iter().any(|role| line.contains(&format!(" {role}")))
}

/// The top-left button: a conversation's way back, whatever it is called (in
/// Korean it is called 메시지, as the field is).
fn is_back(line: &str) -> bool {
    line.contains(" button") && centre_of(line).is_some_and(|(x, y)| x < 120.0 && y < 140.0)
}

fn is_conversation(line: &str) -> bool {
    line.contains(" cell ")
}

async fn look(handle: &str) -> Result<(Value, String), ProviderError> {
    let answer = marks::observe(EmulatorPlatform::Ios, device(handle), None).await?;
    let id = answer[LOOK_ID_KEY].as_str().unwrap_or_default().to_string();
    Ok((answer, id))
}

/// Look until a mark `pick` takes stands, then press it: a screen in change is
/// looked at again, and a stale number comes back refused rather than pressed.
async fn press_where(
    handle: &str,
    what: &str,
    pick: &dyn Fn(&str) -> bool,
) -> Result<Value, String> {
    let began = Instant::now();
    let mut last = String::new();
    while began.elapsed() < Duration::from_secs(15) {
        let (answer, id) = look(handle).await.map_err(|error| error.message)?;
        if let Some(mark) = mark_where(&answer, pick) {
            match marks::click(
                EmulatorPlatform::Ios,
                device(handle),
                mark,
                &id,
                None,
                false,
            )
            .await
            {
                Ok(pressed) => return Ok(pressed),
                Err(error) if error.code == error_code::PIN_BROKEN => {
                    last = error.message;
                    continue;
                }
                Err(error) => return Err(error.message),
            }
        }
        last = answer["legend"].as_str().unwrap_or_default().to_string();
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(format!("{what} never stood: {last}"))
}

async fn press_named(handle: &str, names: &[&str]) -> Result<Value, String> {
    press_where(handle, &format!("{names:?}"), &|line| {
        names.iter().any(|name| line.contains(name))
    })
    .await
}

static BACK_SWIPES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static TOP_BAR: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// Every element of the tree in the top bar, with what its centre answered.
fn top_bar(node: &Value, out: &mut Vec<Value>) {
    let frame = &node["frame"];
    if frame["y"].as_f64().is_some_and(|y| y < 170.0)
        && frame["height"].as_f64().is_some_and(|h| h < 120.0)
    {
        out.push(
            json!({ "type": node["type"], "label": node["AXLabel"], "frame": frame,
                         "hit_at_centre": node["hit_at_centre"] }),
        );
    }
    for child in node["children"].as_array().into_iter().flatten() {
        top_bar(child, out);
    }
}

async fn back_by_swipe(handle: &str) -> Result<(), String> {
    BACK_SWIPES.fetch_add(1, Ordering::SeqCst);
    let first = TOP_BAR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_none();
    if first {
        let answer = tree(handle).await.map_err(|error| error.message)?;
        let mut found = Vec::new();
        for root in answer["tree"].as_array().into_iter().flatten() {
            top_bar(root, &mut found);
        }
        *TOP_BAR
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(json!(found));
    }
    swipe(handle, (0.01, 0.45), (0.8, 0.45), Some(300))
        .await
        .map_err(|error| error.message)?;
    Ok(())
}

/// A sheet's cancel, the way out of a new message the app may open on.
fn is_cancel(line: &str) -> bool {
    line.contains(" button 취소") || line.contains(" button Cancel")
}

/// From anywhere to a conversation's field with the focus: home, the app's icon,
/// back to the list when the app opens where it was left, a conversation, its field.
/// A simulator comes with two conversations (`+1 (888) 555-1212`, `+1 (555)
/// 564-8583`); the errand chooses the first.
async fn open_conversation(handle: &str) -> Result<(), String> {
    button(handle, "home")
        .await
        .map_err(|error| error.message)?;
    tokio::time::sleep(Duration::from_millis(800)).await;
    press_named(handle, &["button 메시지", "button Messages"]).await?;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let mut last = String::new();
    for _ in 0..12 {
        let (answer, _) = look(handle).await.map_err(|error| error.message)?;
        let legend = answer["legend"].as_str().unwrap_or_default().to_string();
        if legend.lines().any(is_cancel) {
            // A new message's sheet: out of it, by its own cancel.
            press_where(handle, "the sheet's cancel", &is_cancel).await?;
        } else if legend.lines().any(is_field) {
            // Opened inside a conversation: back to the list, by the top-left
            // button when it has a number, else by the edge swipe iOS answers
            // everywhere; what the top bar held is kept for the report.
            if legend.lines().any(is_back) {
                press_where(handle, "the way back", &is_back).await?;
            } else {
                back_by_swipe(handle).await?;
            }
        } else if legend.lines().any(is_conversation) {
            press_where(handle, "a conversation", &is_conversation).await?;
            tokio::time::sleep(Duration::from_millis(800)).await;
            press_where(handle, "the field", &is_field).await?;
            tokio::time::sleep(Duration::from_millis(600)).await;
            return Ok(());
        } else {
            // The list has not drawn yet (an app's first start is slow).
            tokio::time::sleep(Duration::from_millis(500)).await;
            last = legend;
            continue;
        }
        tokio::time::sleep(Duration::from_millis(800)).await;
        last = legend;
    }
    Err(format!("never reached a conversation: {last}"))
}

/// One errand: open the app from home, choose whom to write to, write in Korean,
/// send, and check the words stand in the conversation. A wrong press would leave one
/// of these steps with nothing to find, so success counts the presses that were right.
async fn message_flow(handle: &str) -> Result<(), String> {
    open_conversation(handle).await?;
    text(handle, MESSAGE).await.map_err(|error| error.message)?;
    press_named(handle, &["button 보내기", "button Send"]).await?;
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(10) {
        let source = tree(handle).await.map_err(|error| error.message)?;
        if source.to_string().contains(MESSAGE) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err("the message is not in the conversation".to_string())
}

/// What one agent step cost: from a look, a press, until a look of the screen it
/// led to that holds the next number.
struct Step {
    ms: u128,
    calls: usize,
}

/// One step as an agent takes it (t-93254): press what `now` picks on `look`, and
/// come back with a look in which `next` picks a mark — the press's own preview
/// when `preview`, else a press and then `marks`, as agents did on v1.1.119. A
/// number refused as stale, or a look without the next number, is looked again,
/// and every call is counted.
async fn step(
    handle: &str,
    mut look_now: (Value, String),
    now: &dyn Fn(&str) -> bool,
    next: &dyn Fn(&str) -> bool,
    preview: bool,
) -> Result<(Step, (Value, String)), String> {
    let began = Instant::now();
    let mut calls = 0;
    let deadline = Duration::from_secs(15);
    let mut pressed = None;
    while pressed.is_none() && began.elapsed() < deadline {
        let Some(mark) = mark_where(&look_now.0, now) else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            look_now = look(handle).await.map_err(|error| error.message)?;
            calls += 1;
            continue;
        };
        calls += 1;
        match marks::click(
            EmulatorPlatform::Ios,
            device(handle),
            mark,
            &look_now.1,
            None,
            preview,
        )
        .await
        {
            Ok(answer) => pressed = Some(answer),
            Err(error) if error.code == error_code::PIN_BROKEN => {
                look_now = look(handle).await.map_err(|error| error.message)?;
                calls += 1;
            }
            Err(error) => return Err(error.message),
        }
    }
    let answer = pressed.ok_or("the press never went out")?;
    let mut after = match answer["preview"][LOOK_ID_KEY].as_str() {
        Some(id) if preview => (answer["preview"].clone(), id.to_string()),
        _ => {
            calls += 1;
            look(handle).await.map_err(|error| error.message)?
        }
    };
    while mark_where(&after.0, next).is_none() {
        if began.elapsed() >= deadline {
            return Err(format!(
                "the next number never stood: {}",
                after.0["legend"].as_str().unwrap_or_default()
            ));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        after = look(handle).await.map_err(|error| error.message)?;
        calls += 1;
    }
    Ok((
        Step {
            ms: began.elapsed().as_millis(),
            calls,
        },
        after,
    ))
}

/// Steps between a conversation and the list, the one way and the other: back to
/// the list with the next conversation's number, into one with its field's number.
async fn steps(handle: &str, preview: bool) -> Value {
    let mut times = Vec::new();
    let mut calls = 0;
    let mut failures = Vec::new();
    if let Err(why) = open_conversation(handle).await {
        return json!({ "not_measured": why.chars().take(600).collect::<String>() });
    }
    let Ok(mut now) = look(handle).await else {
        return json!({ "not_measured": "no look" });
    };
    for _ in 0..TIMED_ROUNDS {
        if mark_where(&now.0, &is_back).is_some() {
            match step(handle, now.clone(), &is_back, &is_conversation, preview).await {
                Ok((took, after)) => {
                    times.push(took.ms);
                    calls += took.calls;
                    now = after;
                }
                Err(why) => failures.push(why.chars().take(300).collect::<String>()),
            }
        } else {
            // No number for the way back: out by the edge, which is no step.
            if back_by_swipe(handle).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(800)).await;
            let Ok(looked) = look(handle).await else {
                break;
            };
            now = looked;
        }
        match step(handle, now.clone(), &is_conversation, &is_field, preview).await {
            Ok((took, after)) => {
                times.push(took.ms);
                calls += took.calls;
                now = after;
            }
            Err(why) => {
                failures.push(why.chars().take(300).collect::<String>());
                break;
            }
        }
    }
    let count = times.len();
    json!({
        "steps": count,
        "calls_per_step": if count == 0 { Value::Null } else { json!(calls as f64 / count as f64) },
        "time": if count == 0 { Value::Null } else { stats(times) },
        "failures": failures,
    })
}

fn stats(mut samples: Vec<u128>) -> Value {
    samples.sort_unstable();
    let at = |share: f64| samples[((samples.len() - 1) as f64 * share).round() as usize];
    json!({ "n": samples.len(), "p50_ms": at(0.5), "p90_ms": at(0.9), "max_ms": at(1.0) })
}

async fn timed<F, Fut, T, E>(rounds: usize, mut act: F) -> Value
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut samples = Vec::with_capacity(rounds);
    let mut failed = Vec::new();
    for _ in 0..rounds {
        let began = Instant::now();
        match act().await {
            Ok(_) => samples.push(began.elapsed().as_millis()),
            Err(why) => failed.push(format!("{why:?}").chars().take(300).collect::<String>()),
        }
    }
    let mut said = if samples.is_empty() {
        json!({})
    } else {
        stats(samples)
    };
    if !failed.is_empty() {
        said["failed"] = json!(failed);
    }
    said
}

/// The Mac's front app and its pointer, without moving either.
fn desk() -> Value {
    let front = output_of(
        "/usr/bin/osascript",
        &[
            "-e",
            "tell application \"System Events\" to get name of first process whose frontmost is true",
        ],
    );
    let pointer = output_of(
        "/usr/bin/swift",
        &[
            "-e",
            "import CoreGraphics; print(CGEvent(source: nil)!.location)",
        ],
    );
    // Seconds since the Mac last had a pointer or key event from anyone: a run
    // longer than this saw input, and a pointer that moved is then not the road's.
    let hid_idle_s = output_of("/usr/sbin/ioreg", &["-c", "IOHIDSystem", "-d", "4"])
        .and_then(|text| {
            text.lines()
                .find(|line| line.contains("\"HIDIdleTime\""))
                .and_then(|line| line.rsplit('=').next())
                .and_then(|value| value.trim().parse::<u64>().ok())
        })
        .map(|nanos| nanos / 1_000_000_000);
    json!({ "front": front.map(|word| word.trim().to_string()), "pointer": pointer.map(|word| word.trim().to_string()),
            "hid_idle_s": hid_idle_s })
}

/// A busy road that shares the Mac with a phone's: the desktop's own read of its
/// windows, the sort a browser or an app agent makes, again and again.
fn desktop_load(stop: Arc<AtomicBool>) -> std::thread::JoinHandle<u128> {
    std::thread::spawn(move || {
        let mut most = 0;
        while !stop.load(Ordering::SeqCst) {
            let began = Instant::now();
            let _ = output_of(
                "/usr/bin/osascript",
                &[
                    "-e",
                    "tell application \"System Events\" to get name of every process",
                ],
            );
            most = most.max(began.elapsed().as_millis());
        }
        most
    })
}

#[test]
#[ignore = "needs a booted simulator and a built runner: tools/real-phone-bench/run.sh"]
fn the_bench() {
    let plan = env("ZC_PHONE_BENCH_PLAN");
    let one = "phone-bench0001";
    let life = sim_phone(one, &env("ZC_PHONE_BENCH_SIM"), 8123, &plan);
    let cold = Instant::now();
    life.ensure_up(START_WAIT, &|| true)
        .expect("the runner came up");
    let start_ms = cold.elapsed().as_millis();
    let mut report = json!({ "start_no_build_ms": start_ms });
    let desk_before = desk();

    // Speed.
    let speed = run(async {
        button(one, "home").await.expect("home");
        tokio::time::sleep(Duration::from_millis(600)).await;
        let lock_ms = timed(TIMED_ROUNDS, || async { driven(one).await.map(drop) }).await;
        let tree_ms = timed(TIMED_ROUNDS, || tree(one)).await;
        let marks_ms = timed(TIMED_ROUNDS, || look(one)).await;
        let find_ms = timed(TIMED_ROUNDS, || {
            crate::emulator::checks::observe(
                EmulatorPlatform::Ios,
                device(one),
                zerocode_core::computer_use::EmulatorMethod::Find,
                "메시지",
            )
        })
        .await;
        // A tap on an empty spot, so no screen changes under the next one.
        let tap_ms = timed(TIMED_ROUNDS, || tap(one, 0.5, 0.42)).await;
        let text_ms = match open_conversation(one).await {
            Ok(()) => timed(TIMED_ROUNDS, || text(one, "안녕 abc 123 ")).await,
            Err(why) => json!({ "not_measured": why.chars().take(600).collect::<String>() }),
        };
        json!({ "lock_check": lock_ms, "tree": tree_ms, "marks": marks_ms, "find": find_ms,
                "tap": tap_ms, "text_11_characters": text_ms })
    });
    report["speed"] = speed;

    // Accuracy: the whole errand, 20 times.
    let accuracy = run(async {
        let mut times = Vec::new();
        let mut failures = Vec::new();
        for round in 0..SWEEP_ROUNDS {
            let began = Instant::now();
            match message_flow(one).await {
                Ok(()) => times.push(began.elapsed().as_millis()),
                Err(why) => failures.push(format!(
                    "round {round}: {}",
                    why.chars().take(1200).collect::<String>()
                )),
            }
        }
        json!({ "rounds": SWEEP_ROUNDS, "succeeded": times.len(), "flow": if times.is_empty() { Value::Null } else { stats(times) },
                "failures": failures })
    });
    report["accuracy"] = accuracy;
    report["accuracy"]["back_by_edge_swipe"] = json!(BACK_SWIPES.load(Ordering::SeqCst));
    report["accuracy"]["top_bar_without_a_back_number"] = TOP_BAR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or(Value::Null);

    // One step, a press and the next number (t-93254): a press and `marks`, as on
    // v1.1.119, against a press whose preview is the next look.
    report["step"] = run(async {
        json!({ "press_then_marks": steps(one, false).await,
                "press_with_preview": steps(one, true).await })
    });

    // A stale number on a real screen change: looked at home, pressed in an app.
    let stale = run(async {
        let step = async {
            button(one, "home").await.map_err(|error| error.message)?;
            tokio::time::sleep(Duration::from_millis(800)).await;
            let (answer, id) = look(one).await.map_err(|error| error.message)?;
            let mark = mark_of(&answer, &["button 메시지", "button Messages"])
                .ok_or_else(|| "no icon".to_string())?;
            press_named(one, &["button 메시지", "button Messages"]).await?;
            tokio::time::sleep(Duration::from_millis(800)).await;
            Ok::<_, String>(
                marks::click(EmulatorPlatform::Ios, device(one), mark, &id, None, false)
                    .await
                    .map(|_| "pressed".to_string())
                    .unwrap_or_else(|error| error.code),
            )
        };
        match step.await {
            Ok(said) => json!({ "stale_number_after_a_screen_change": said }),
            Err(why) => json!({ "not_measured": why }),
        }
    });
    report["stale"] = stale;

    // Together: alone, then beside a desktop road, then two devices.
    let alone = run(async { timed(TIMED_ROUNDS, || tree(one)).await });
    let stop = Arc::new(AtomicBool::new(false));
    let load = desktop_load(Arc::clone(&stop));
    let beside = run(async { timed(TIMED_ROUNDS, || tree(one)).await });
    stop.store(true, Ordering::SeqCst);
    let desktop_most = load.join().unwrap_or_default();
    let mut together = json!({ "alone_tree": alone, "beside_a_desktop_agent_tree": beside,
        "desktop_agent_slowest_call_ms": desktop_most as u64 });

    if let Ok(second) = std::env::var("ZC_PHONE_BENCH_SIM2") {
        let two = "phone-bench0002";
        let life2 = sim_phone(two, &second, 8124, &plan);
        life2
            .ensure_up(START_WAIT, &|| true)
            .expect("the second runner came up");
        let both = run(async {
            let alone_second = timed(TIMED_ROUNDS, || tree(two)).await;
            let (first, second) = tokio::join!(
                timed(TIMED_ROUNDS, || tree(one)),
                timed(TIMED_ROUNDS, || tree(two)),
            );
            json!({ "second_alone": alone_second, "first_with_second": first, "second_with_first": second })
        });
        together["two_devices"] = both;
        life2.stop_and_wait();
    }

    // One device, two callers: they queue and neither is refused.
    let same = run(async {
        let began = Instant::now();
        let (first, second) = tokio::join!(tree(one), tree(one));
        json!({ "both_answered": first.is_ok() && second.is_ok(), "ms": began.elapsed().as_millis() as u64 })
    });
    together["same_device_two_callers"] = same;
    report["together"] = together;

    report["desk_before"] = desk_before;
    report["desk_after"] = desk();
    // The Mac's front app and pointer say something only when nobody else used the
    // Mac meanwhile: its last input is older than the run.
    let quiet = report["desk_after"]["hid_idle_s"]
        .as_u64()
        .is_some_and(|idle| u128::from(idle) * 1000 >= cold.elapsed().as_millis());
    report["desk_judged"] = json!(quiet);
    if !quiet {
        report["desk_not_judged_because"] =
            json!("the Mac had input from someone during the run (HID idle shorter than the run)");
    }
    life.stop_and_wait();
    let said = serde_json::to_string_pretty(&report).expect("a report");
    if let Ok(path) = std::env::var("ZC_PHONE_BENCH_OUT") {
        std::fs::write(path, &said).expect("the report");
    }
    println!("{said}");
    if quiet {
        assert_eq!(
            report["desk_before"]["front"],
            report["desk_after"]["front"]
        );
        assert_eq!(
            report["desk_before"]["pointer"],
            report["desk_after"]["pointer"]
        );
    }
}
