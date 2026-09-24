//! A phone walk timed end to end on a simulator of the measurer's own
//! (t-6385): the engine the window's `walk` drives — the marked look, the
//! judgment, the press by number, the reach check — with no window, no
//! mirror tab and none of the person's ledgers.
//!
//! Not part of the gate. It boots nothing and touches no device it was not
//! handed: `ZEROCODE_WALK_BENCH_UDID` names a simulator already booted for
//! the measurement, which is registered the way an open pane registers one
//! (so the verbs' own `ios_control` answers) and driven through this build's
//! helper. Before every walk Settings is relaunched and looked at until two
//! looks agree, then walked toward the goal. The judgment is the real one —
//! the key comes from `ZEROCODE_JEV_BENCH_KEY` — and every Jev row, the
//! memo included, lands in a zo home of the bench's own
//! (`ZEROCODE_WALK_BENCH_HOME`, else a temporary folder), never `~/.zo`.
//! One JSON line per walk is appended to `ZEROCODE_WALK_BENCH_OUT`: the
//! walk's rows as the window would write them, and every verb it drove with
//! its milliseconds and what it answered.

use std::cell::RefCell;
use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::computer_recipe::RecipeTool;
use zerocode_core::computer_use::{EmulatorPlatform, parse_emulator_command};
use zerocode_core::jev::{EMULATOR, JevMode};
use zerocode_hookd::TeamAnswer;

use crate::computer_use::errand::{self, desk, live};

/// The goal t-6350 measured: two presses, three screens.
const GOAL: &str = "설정 앱에서 일반을 누른 뒤 정보 화면을 연다";
/// The words that are on screen when it worked.
const UNTIL: &str = "iOS 버전";
/// The app every walk starts from.
const SETTINGS: &str = "com.apple.Preferences";
/// How many looks the relaunched app may take to read the same twice, and
/// how long a look that failed waits before the next (a relaunch answers
/// "no frontmost application" until the app is up).
const LOOKS_TO_STAND_STILL: usize = 12;
const AFTER_A_FAILED_LOOK: std::time::Duration = std::time::Duration::from_millis(300);

fn knob(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn simctl(words: &[&str]) {
    let _ = crate::proc::quiet_command("xcrun")
        .arg("simctl")
        .args(words)
        .output();
}

/// One verb down the road a walk's steps take in the window, timed.
fn drive(argv: &[String]) -> (TeamAnswer, f64) {
    let began = Instant::now();
    let command = parse_emulator_command(argv).expect("a walk's own argv parses");
    let answer = tauri::async_runtime::block_on(
        crate::agent_tools_runtime::answer_emulator_observation(command),
    );
    (answer, began.elapsed().as_secs_f64() * 1_000.0)
}

fn said(answer: &TeamAnswer) -> Value {
    serde_json::from_str(answer.stdout.trim())
        .or_else(|_| serde_json::from_str(answer.stderr.trim()))
        .unwrap_or(Value::Null)
}

/// What a verb answered, in the few words a table needs.
fn call_row(argv: &[String], ms: f64, answer: &TeamAnswer) -> Value {
    let said = said(answer);
    let result = said.get("result").cloned().unwrap_or(Value::Null);
    json!({
        "verb": argv.first(),
        "ms": (ms * 10.0).round() / 10.0,
        "ok": answer.exit_code == 0,
        "code": said.pointer("/error/code"),
        "error": said
            .pointer("/error/message")
            .and_then(Value::as_str)
            .map(|message| message.chars().take(160).collect::<String>()),
        "items": result.get("items").and_then(Value::as_array).map(Vec::len),
        "legend": result.get("legend"),
        "count": result.get("count"),
        "settle": result.get("settle"),
        "confirmedBy": result.get("confirmedBy"),
    })
}

fn look_argv(udid: &str) -> Vec<String> {
    ["marks", "--platform", "ios", "--device", udid, "--json"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// Settings from its first screen, looked at until two looks agree: how many
/// looks that took, or `None` when it never stood still.
fn to_the_first_screen(udid: &str) -> Option<usize> {
    simctl(&["terminate", udid, SETTINGS]);
    simctl(&["launch", udid, SETTINGS]);
    let mut last: Option<Value> = None;
    for looks in 1..=LOOKS_TO_STAND_STILL {
        let (answer, _) = drive(&look_argv(udid));
        let legend = said(&answer).pointer("/result/legend").cloned();
        if legend.is_some() && legend == last {
            return Some(looks);
        }
        if legend.is_none() {
            std::thread::sleep(AFTER_A_FAILED_LOOK);
        }
        last = legend;
    }
    None
}

/// The machine's one-minute load average — the bench's numbers are only
/// comparable between walks taken under similar load. The window's one
/// reader of it (the task board's machine strip reads the same).
fn load() -> Option<f64> {
    crate::orchestration::desk::load_average().map(|reading| reading.one_minute)
}

/// Register `udid` the way an open pane does, and start this build's helper
/// — or the one `ZEROCODE_WALK_BENCH_HELPER` names, so two helper builds can
/// be timed on the same Rust code.
fn stand_in_for_a_pane(udid: &str) {
    if let Some(helper) = knob("ZEROCODE_WALK_BENCH_HELPER") {
        super::ios_hid::use_helper(PathBuf::from(helper));
    }
    use super::session::{SessionKey, StartClaim, registry};
    let key = SessionKey::frames(super::EmulatorPlatform::Ios, udid);
    let StartClaim::Acquired(lease) = registry().claim(key).expect("a claim") else {
        panic!("the bench's device already has a session");
    };
    let descriptor = super::EmulatorStream {
        stream: format!("walk-bench-{udid}"),
        udid: udid.to_string(),
        name: "walk bench".to_string(),
        platform: super::EmulatorPlatform::Ios,
        interactive: true,
        reused: false,
    };
    let control = registry().new_control(&descriptor);
    lease.activate(descriptor, control);
    super::ios_hid::retain(udid).expect("this build's helper");
}

/// A failed or empty walk is a recorded failure, never a faster sample.
fn a_speed_sample(looks: Option<usize>, walked: &errand::Walked) -> bool {
    looks.is_some() && walked.pressed > 0 && walked.reached == Some(true)
}

#[test]
fn a_phone_speed_sample_requires_readiness_a_press_and_its_goal() {
    let done = errand::Walked {
        pressed: 2,
        reached: Some(true),
        ..Default::default()
    };
    assert!(a_speed_sample(Some(2), &done));
    assert!(!a_speed_sample(None, &done));
    assert!(!a_speed_sample(
        Some(2),
        &errand::Walked {
            pressed: 0,
            ..done.clone()
        }
    ));
    assert!(!a_speed_sample(
        Some(2),
        &errand::Walked {
            reached: Some(false),
            ..done.clone()
        }
    ));
    assert!(!a_speed_sample(
        Some(2),
        &errand::Walked {
            reached: None,
            ..done
        }
    ));
}

#[test]
#[ignore = "drives a booted simulator of the caller's and spends a real key; a measurement, printed"]
fn a_phone_walk_timed_on_a_simulator_of_our_own() {
    let udid = knob("ZEROCODE_WALK_BENCH_UDID").expect("a booted simulator's UDID");
    let key = knob("ZEROCODE_JEV_BENCH_KEY").expect("a TypeSafe key");
    let out = PathBuf::from(knob("ZEROCODE_WALK_BENCH_OUT").expect("where the rows go"));
    let walks: usize = knob("ZEROCODE_WALK_BENCH_WALKS").map_or(10, |n| n.parse().expect("walks"));
    let steps: usize = knob("ZEROCODE_WALK_BENCH_STEPS").map_or(4, |n| n.parse().expect("steps"));
    assert!(walks > 0, "a speed measurement needs at least one walk");
    let goal = knob("ZEROCODE_WALK_BENCH_GOAL").unwrap_or_else(|| GOAL.to_string());
    // An empty value is a walk given no `--until`.
    let until = match std::env::var("ZEROCODE_WALK_BENCH_UNTIL") {
        Ok(until) if until.is_empty() => None,
        Ok(until) => Some(until),
        Err(_) => Some(UNTIL.to_string()),
    };
    let overlap = knob("ZEROCODE_WALK_BENCH_OVERLAP").is_some_and(|flag| flag == "1");
    // `walk --replay`: the run the walk is asked in.
    let run = if knob("ZEROCODE_WALK_BENCH_REPLAY").is_some_and(|flag| flag == "1") {
        zerocode_core::jev::Run::Repeated
    } else {
        zerocode_core::jev::Run::Fresh
    };
    let cache = knob("ZEROCODE_WALK_BENCH_CACHE").unwrap_or_else(|| "off".to_string());
    let label = knob("ZEROCODE_WALK_BENCH_LABEL").unwrap_or_default();
    let base = knob("ZO_SYSTEMONE_BASE_URL").unwrap_or_else(|| "https://api.typesafe.ai".into());

    // A zo home of the bench's own: its settings consent one workspace and
    // switch the emulator seat on, and its ledgers are the only ones written.
    let temporary = tempfile::tempdir().expect("a zo home");
    let home = knob("ZEROCODE_WALK_BENCH_HOME")
        .map_or_else(|| temporary.path().to_path_buf(), PathBuf::from);
    let work = home.join("work");
    std::fs::create_dir_all(&work).expect("a workspace");
    let settings = home.join("settings.json");
    std::fs::write(
        &settings,
        json!({ "smart": {
            "enabled": true,
            "jev": { "workspaces": [work.display().to_string()] },
            (EMULATOR.setting): "on",
            (zerocode_core::jev::JUDGMENT_CACHE.setting): cache,
        } })
        .to_string(),
    )
    .expect("the bench's settings");

    stand_in_for_a_pane(&udid);
    // The helper's first tree pays for loading the accessibility framework.
    let _ = drive(&look_argv(&udid));

    let mut all_measured = true;
    for walk in 0..walks {
        let looks = to_the_first_screen(&udid);
        let calls = RefCell::new(Vec::new());
        let mut road = |_: RecipeTool, argv: &[String], _: &[String]| {
            let (answer, ms) = drive(argv);
            calls.borrow_mut().push(call_row(argv, ms, &answer));
            answer
        };
        let mut judge = live::LiveJudge::at(
            &base,
            &key,
            live::Doorway {
                settings: Some(settings.clone()),
                workspace: Some(work.clone()),
                seat: &EMULATOR,
            },
        )
        .in_run(run);
        let at = errand::Errand {
            goal: &goal,
            why: errand::Why::Goal { steps },
            flow: None,
            moves_money: false,
        };
        let began = Instant::now();
        let walked = {
            let mut world = desk::GoalWorld::new(
                &mut road,
                desk::Aim::Phone {
                    platform: EmulatorPlatform::Ios,
                    device: udid.clone(),
                },
                errand::Seen::default(),
                until.clone(),
                120_000,
                0,
            )
            .previewing(overlap);
            errand::run_with(
                JevMode::On,
                true,
                errand::Branching::OFF,
                &at,
                &mut judge,
                &mut world,
                errand::Options {
                    overlap,
                    rescue: false,
                },
                None,
            )
        };
        let walk_ms = began.elapsed().as_secs_f64() * 1_000.0;
        all_measured &= a_speed_sample(looks, &walked);
        let now = crate::project_runtime::now_epoch_ms();
        errand::write_rows(&EMULATOR, judge.wire(), None, &walked.rows, now);
        judge.write_memo_rows(None, now);
        let row = json!({
            "label": label,
            "walk": walk,
            "looksToStill": looks,
            "load": load(),
            "helper": knob("ZEROCODE_WALK_BENCH_HELPER"),
            "overlap": overlap,
            "cache": cache,
            "run": run.key(),
            "until": until,
            "walkMs": walk_ms.round(),
            "pressed": walked.pressed,
            "reached": walked.reached,
            "overlapped": walked.overlapped,
            "discarded": walked.discarded,
            "steps": walked.rows,
            "calls": calls.into_inner(),
            "at": now,
        });
        println!(
            "{label} walk {walk}: {:.0} ms, pressed {}, reached {:?}",
            walk_ms, walked.pressed, walked.reached
        );
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out)
            .expect("the bench's rows");
        std::io::Write::write_all(&mut file, format!("{row}\n").as_bytes()).expect("a row");
    }
    super::ios_hid::release(&udid);
    assert!(
        all_measured,
        "a walk did not start ready, make a press and reach its goal; inspect the recorded rows"
    );
}

/// The screens [`the_marks_a_set_of_screens_carries`] looks at when it is
/// named none: app roots whose trees differ in kind (lists, grids, maps,
/// toolbars), two Settings screens a walk reaches by pressing, and home.
const SCREENS: &str = "home,com.apple.Preferences,com.apple.Preferences>일반,com.apple.Preferences>일반>정보,com.apple.mobilecal,com.apple.mobileslideshow,com.apple.Maps,com.apple.mobilesafari,com.apple.reminders,com.apple.Health,com.apple.DocumentsApp";

/// Look until two looks agree; the last look's answer and how long each took.
fn still_look(udid: &str) -> (Value, Vec<f64>) {
    let mut last: Option<Value> = None;
    let mut millis = Vec::new();
    for _ in 0..LOOKS_TO_STAND_STILL {
        let (answer, ms) = drive(&look_argv(udid));
        millis.push((ms * 10.0).round() / 10.0);
        let said = said(&answer);
        let legend = said.pointer("/result/legend").cloned();
        if legend.is_some()
            && legend
                == last
                    .as_ref()
                    .and_then(|seen| seen.pointer("/result/legend").cloned())
        {
            return (said, millis);
        }
        if legend.is_none() {
            std::thread::sleep(AFTER_A_FAILED_LOOK);
        }
        last = Some(said);
    }
    (last.unwrap_or(Value::Null), millis)
}

/// Press the control a still look labels `label`, by its number.
fn press_labelled(udid: &str, look: &Value, label: &str) -> bool {
    let Some(mark) = look
        .pointer("/result/items")
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find(|item| item["label"] == label))
        .and_then(|item| item["mark"].as_u64())
    else {
        return false;
    };
    let look_id = look
        .pointer("/result/lookId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let argv: Vec<String> = [
        "click",
        "--platform",
        "ios",
        "--device",
        udid,
        "--mark",
        &mark.to_string(),
        "--look",
        look_id,
        "--json",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    drive(&argv).0.exit_code == 0
}

/// The marks a set of screens carries, looked at by this build's helper or
/// `ZEROCODE_WALK_BENCH_HELPER`'s (t-6385 U4): every screen in
/// `ZEROCODE_WALK_BENCH_SCREENS` (comma-separated; `home`, a bundle id, or a
/// bundle id followed by the labels to press, `>`-separated) is reached,
/// looked at until two looks agree and then three looks more, and its legend
/// and look milliseconds are appended to `ZEROCODE_WALK_BENCH_OUT`. Two runs
/// with two helpers are compared by the legends: a helper that reads a
/// screen faster must still number the same controls.
#[test]
#[ignore = "drives a booted simulator of the caller's; a measurement, printed"]
fn the_marks_a_set_of_screens_carries() {
    let udid = knob("ZEROCODE_WALK_BENCH_UDID").expect("a booted simulator's UDID");
    let out = PathBuf::from(knob("ZEROCODE_WALK_BENCH_OUT").expect("where the rows go"));
    let screens = knob("ZEROCODE_WALK_BENCH_SCREENS").unwrap_or_else(|| SCREENS.to_string());
    let label = knob("ZEROCODE_WALK_BENCH_LABEL").unwrap_or_default();
    stand_in_for_a_pane(&udid);
    let _ = drive(&look_argv(&udid));
    for screen in screens
        .split(',')
        .map(str::trim)
        .filter(|screen| !screen.is_empty())
    {
        let mut path = screen.split('>');
        let app = path.next().unwrap_or_default();
        if app == "home" {
            let _ = super::ios_hid::send(
                &udid,
                super::ios_hid::InputRequest::Button {
                    name: "home".to_string(),
                },
            );
        } else {
            simctl(&["terminate", &udid, app]);
            simctl(&["launch", &udid, app]);
        }
        let (mut look, _) = still_look(&udid);
        let mut reached = true;
        for step in path {
            reached &= press_labelled(&udid, &look, step);
            look = still_look(&udid).0;
        }
        let mut millis = Vec::new();
        for _ in 0..3 {
            let (answer, ms) = drive(&look_argv(&udid));
            millis.push((ms * 10.0).round() / 10.0);
            look = said(&answer);
        }
        let row = json!({
            "label": label,
            "helper": knob("ZEROCODE_WALK_BENCH_HELPER"),
            "screen": screen,
            "reached": reached,
            "load": load(),
            "lookMs": millis,
            "legend": look.pointer("/result/legend"),
            "candidates": look.pointer("/result/candidates"),
            "error": look.pointer("/error/message"),
        });
        println!("{label} {screen}: {millis:?}");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out)
            .expect("the bench's rows");
        std::io::Write::write_all(&mut file, format!("{row}\n").as_bytes()).expect("a row");
    }
    super::ios_hid::release(&udid);
}

/// Register an Android device the way its pane does — a video session under
/// its serial — so the door's own `android_control` answers.
fn stand_in_for_an_android_pane(serial: &str, avd: &str) {
    use super::session::{SessionKey, StartClaim, registry};
    let key = SessionKey::video(super::EmulatorPlatform::Android, serial);
    let StartClaim::Acquired(lease) = registry().claim(key).expect("a claim") else {
        panic!("the bench's device already has a session");
    };
    let descriptor = super::EmulatorStream {
        stream: format!("walk-bench-{serial}"),
        udid: serial.to_string(),
        name: avd.to_string(),
        platform: super::EmulatorPlatform::Android,
        interactive: true,
        reused: false,
    };
    let control = registry().new_control(&descriptor);
    lease.activate(descriptor, control);
}

/// The Android door's three walk verbs — a look, a check and a press by
/// number — timed on an AVD of the measurer's own (t-6385 U3), down the road
/// a walk's steps take in the window. `ZEROCODE_WALK_BENCH_SERIAL` and
/// `ZEROCODE_WALK_BENCH_AVD` name a copy of an AVD started on a port of its
/// own (t-5761's way); `ZEROCODE_WALK_BENCH_ADB` is the adb that launches
/// Settings and goes back between rounds. Each round looks until two looks
/// agree, then times one look, one `find` of `ZEROCODE_WALK_BENCH_UNTIL` and
/// one press of the control labelled `ZEROCODE_WALK_BENCH_PRESS`.
#[test]
#[ignore = "drives an AVD of the caller's; a measurement, printed"]
fn android_verbs_timed_on_an_avd_of_our_own() {
    let serial = knob("ZEROCODE_WALK_BENCH_SERIAL").expect("the AVD's serial");
    let avd = knob("ZEROCODE_WALK_BENCH_AVD").expect("the AVD's name");
    let adb = knob("ZEROCODE_WALK_BENCH_ADB").expect("adb");
    let out = PathBuf::from(knob("ZEROCODE_WALK_BENCH_OUT").expect("where the rows go"));
    let rounds: usize =
        knob("ZEROCODE_WALK_BENCH_WALKS").map_or(10, |n| n.parse().expect("rounds"));
    let label = knob("ZEROCODE_WALK_BENCH_LABEL").unwrap_or_default();
    let press = knob("ZEROCODE_WALK_BENCH_PRESS").unwrap_or_else(|| "Network & internet".into());
    let until = knob("ZEROCODE_WALK_BENCH_UNTIL").unwrap_or_else(|| "Battery".into());
    let settings = knob("ZEROCODE_WALK_BENCH_ACTIVITY")
        .unwrap_or_else(|| "com.android.settings/.Settings".into());
    stand_in_for_an_android_pane(&serial, &avd);
    let verb = |words: &[&str]| -> Vec<String> {
        let mut argv: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        argv.extend(["--platform", "android", "--device", &avd, "--json"].map(str::to_string));
        argv
    };
    let shell = |words: &[&str]| {
        let _ = crate::proc::quiet_command(&adb)
            .args(["-s", &serial, "shell"])
            .args(words)
            .output();
    };
    for round in 0..rounds {
        shell(&["am", "start", "-n", &settings]);
        let mut last: Option<Value> = None;
        for _ in 0..LOOKS_TO_STAND_STILL {
            let legend = said(&drive(&verb(&["marks"])).0)
                .pointer("/result/legend")
                .cloned();
            if legend.is_some() && legend == last {
                break;
            }
            last = legend;
        }
        let (looked, look_ms) = drive(&verb(&["marks"]));
        let look = said(&looked);
        let (found, find_ms) = drive(&verb(&["find", "--text", &until]));
        let mark = look
            .pointer("/result/items")
            .and_then(Value::as_array)
            // A row's name on Android is its own and its descendants' words
            // (`faces`' compound names): the control is found by how it starts.
            .and_then(|items| {
                items.iter().find(|item| {
                    item["label"]
                        .as_str()
                        .is_some_and(|label| label.starts_with(press.as_str()))
                })
            })
            .and_then(|item| item["mark"].as_u64());
        let look_id = look
            .pointer("/result/lookId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let (clicked, click_ms) = match mark {
            Some(mark) => {
                let mark = mark.to_string();
                let (answer, ms) = drive(&verb(&["click", "--mark", &mark, "--look", look_id]));
                (Some(answer), ms)
            }
            None => (None, 0.0),
        };
        shell(&["input", "keyevent", "4"]);
        std::thread::sleep(std::time::Duration::from_millis(1_000));
        let row = json!({
            "label": label,
            "round": round,
            "load": load(),
            "lookMs": (look_ms * 10.0).round() / 10.0,
            "lookOk": looked.exit_code == 0,
            "items": look.pointer("/result/items").and_then(Value::as_array).map(Vec::len),
            "findMs": (find_ms * 10.0).round() / 10.0,
            "found": said(&found).pointer("/result/count"),
            "clickMs": (click_ms * 10.0).round() / 10.0,
            "clickOk": clicked.as_ref().map(|answer| answer.exit_code == 0),
            "clickError": clicked.as_ref().and_then(|answer| said(answer).pointer("/error/message").cloned()),
        });
        println!("{label} round {round}: look {look_ms:.0} find {find_ms:.0} click {click_ms:.0}");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&out)
            .expect("the bench's rows");
        std::io::Write::write_all(&mut file, format!("{row}\n").as_bytes()).expect("a row");
    }
}
