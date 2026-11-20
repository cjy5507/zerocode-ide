//! The person's 11-20 scene (CU slowness, t-121376 · t-121372), measured against
//! the helper the machine has installed and this build's, turn about in one run,
//! on owned fixture windows that never take focus (`TwinWindowsFixture.swift`):
//! one window alone, two at one frame with one title, two at one frame that
//! both carry the button, and two of one size 22 points apart — the button the
//! errand was for in the one behind. Semantic presses only: no pointer, no keys,
//! no card.
//!
//! A scripted road per scene stands in for the model, the same for both
//! helpers. `window`: look at the target window by its number, press its
//! «Authorize» by its words there, and when that is refused take the step the
//! refusal names — a window move for «move or resize one of them» — once, then
//! try again once. `app`: the road the model took first that day — press
//! «Authorize» by its words in the app's front window; when the refusal names
//! another window (`window_id N` in zo's tool, `--window-id N` from the helper
//! itself), look at it and press the number its look
//! reads; else take the `window` road. Each road is counted in steps, refusals
//! (their codes) and milliseconds, and judged by the windows' own record of who
//! was pressed: the target once, any other never.
//!
//! Input rules (CU pane m-122438): the fixture scenes and the late-press scene
//! are background presses only — no pointer, no keys, no window brought
//! forward — and run with the person at the machine. The Chrome and the key
//! scenes bring a window forward: they start only after the person has left
//! the mouse and the keyboard alone for two minutes, and stop at once when
//! they touch either.
//!
//! ```text
//! swiftc -O tools/computer-bench/TwinWindowsFixture.swift -o <dir>/twin-fixture -framework AppKit
//! ZEROCODE_TWIN_FIXTURE=<dir>/twin-fixture \
//! ZEROCODE_TWIN_BASELINE=/Applications/ZeroCode.app/Contents/Resources/ZeroCode\ Computer\ Use.app \
//! ZEROCODE_TWIN_BUILT=<this build's helper .app> \
//! ZEROCODE_TWIN_OUT=<dir>/twin-report.json \
//! cargo test -p zerocode-shell --bin zerocode-shell twin_bench -- --ignored --nocapture --test-threads=1
//! ```

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Session, launch_at};
use crate::computer_use::session::{ProviderSession as _, SessionFailure};

/// How long the person must have left the mouse and keys alone before a road.
const PERSON_AWAY: Duration = Duration::from_secs(120);

struct Fixture {
    process: Child,
    directory: PathBuf,
    ready: Value,
}

impl Fixture {
    fn start(program: &Path, root: &Path, mode: &str, name: &str) -> Self {
        let directory = root.join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let process = crate::proc::quiet_command(program)
            .arg(mode)
            .arg(&directory)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("fixture");
        let started = Instant::now();
        let ready = loop {
            let said = std::fs::read_to_string(directory.join("ready"))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok());
            if let Some(ready) = said {
                break ready;
            }
            assert!(started.elapsed() < Duration::from_secs(10), "fixture ready");
            std::thread::sleep(Duration::from_millis(10));
        };
        // The window server lists the windows a moment after they are ordered.
        std::thread::sleep(Duration::from_millis(250));
        Self {
            process,
            directory,
            ready,
        }
    }

    fn app(&self) -> String {
        format!("pid:{}", self.process.id())
    }

    /// The window the errand is for: the one behind, or the only one.
    fn target(&self) -> u64 {
        self.ready["back"]
            .as_u64()
            .or_else(|| self.ready["front"].as_u64())
            .expect("the fixture names its windows")
    }

    /// The windows pressed so far, by their numbers, in order.
    fn presses(&self) -> Vec<u64> {
        self.numbers_after("pressed ")
    }

    /// The windows that took a key so far, by their numbers, in order.
    fn keys(&self) -> Vec<u64> {
        self.numbers_after("key ")
    }

    fn numbers_after(&self, prefix: &str) -> Vec<u64> {
        std::fs::read_to_string(self.directory.join("events.log"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.strip_prefix(prefix))
            .filter_map(|rest| rest.split_whitespace().next())
            .filter_map(|number| number.parse().ok())
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

/// How long ago the person last touched the mouse or the keyboard.
fn person_idle() -> Duration {
    let out = crate::proc::quiet_command("/usr/sbin/ioreg")
        .args(["-c", "IOHIDSystem"])
        .output()
        .expect("ioreg");
    let nanos = String::from_utf8_lossy(&out.stdout)
        .lines()
        .find(|line| line.contains("\"HIDIdleTime\""))
        .and_then(|line| line.split('=').nth(1))
        .and_then(|nanos| nanos.trim().parse::<u64>().ok())
        .unwrap_or(0);
    Duration::from_nanos(nanos)
}

/// One road's steps as they went.
#[derive(Default)]
struct Road {
    steps: Vec<Value>,
    refusals: Vec<String>,
    last_refusal: Option<String>,
}

impl Road {
    fn step(
        &mut self,
        session: &Session,
        what: &str,
        method: &str,
        params: &Value,
    ) -> Option<Value> {
        let (ms, answer) = call(session, method, params);
        let refused = answer.as_ref().err().cloned();
        self.steps
            .push(json!({"step": what, "method": method, "ms": ms.round(), "refused": refused}));
        if let Some(refused) = &refused {
            self.refusals
                .push(refused.split(':').next().unwrap_or_default().to_string());
        }
        self.last_refusal = refused;
        answer.ok()
    }

    fn report(&self, fixture: &Fixture, target: u64) -> Value {
        let presses = fixture.presses();
        let ms: f64 = self
            .steps
            .iter()
            .filter_map(|step| step["ms"].as_f64())
            .sum();
        json!({
            "steps": self.steps.len(),
            "refusals": self.refusals.len(),
            "codes": self.refusals,
            "ms": ms.round(),
            "pressed_target": presses.iter().filter(|&&number| number == target).count(),
            "pressed_other": presses.iter().filter(|&&number| number != target).count(),
            "trail": self.steps,
        })
    }
}

/// The number on a look's tree line that reads `words`.
fn number_of(state: &Value, words: &str) -> Option<u64> {
    state["snapshot"]["treeText"]
        .as_str()?
        .lines()
        .map(str::trim)
        .find(|line| line.contains(words))
        .and_then(|line| line.split(' ').next())
        .and_then(|number| number.parse().ok())
}

/// The window a refusal names as the app's other window, if it names exactly
/// one that is not `seen`: `window_id N` in zo's tool, `--window-id N` from
/// the helper itself, which this bench asks directly.
fn named_window(refused: &str, seen: Option<u64>) -> Option<u64> {
    let named: Vec<u64> = ["window_id ", "--window-id "]
        .into_iter()
        .flat_map(|marker| refused.split(marker).skip(1))
        .filter_map(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
        .filter_map(|number| number.parse().ok())
        .filter(|number| Some(*number) != seen)
        .collect();
    match named[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// Look at window `target` and press its «Authorize» by its words there,
/// taking the step a refusal names once and trying once more.
fn window_road(road: &mut Road, session: &Session, fixture: &Fixture, target: u64) {
    let app = fixture.app();
    let look = json!({"app": app, "windowId": target, "noScreenshot": true, "background": true});
    let press = json!({
        "app": app, "windowId": target, "label": "Authorize", "role": "AXButton",
        "background": true, "noScreenshot": true,
    });
    for attempt in 0..2 {
        if road
            .step(session, "look at the target window", "getAppState", &look)
            .is_some()
            && road
                .step(session, "press Authorize in it", "click", &press)
                .is_some()
        {
            return;
        }
        let advice = road.last_refusal.clone().unwrap_or_default();
        if attempt == 1 || !advice.contains("move or resize one of them") {
            return;
        }
        // The refusal's own advice: move one of them a little, then look again.
        let frame = &fixture.ready["windows"][0]["frame"];
        let x = frame[0].as_f64().unwrap_or(100.0) + 40.0;
        let y = frame[1].as_f64().unwrap_or(100.0) + 40.0;
        let _ = road.step(
            session,
            "move it (the refusal's advice)",
            "windowAction",
            &json!({"action": "move", "windowId": target, "x": x, "y": y}),
        );
    }
}

/// The model's road that day: the app's front window by words first, then
/// the window a refusal names, else the window road.
fn app_road(road: &mut Road, session: &Session, fixture: &Fixture, target: u64) {
    let app = fixture.app();
    let first = json!({"app": app, "label": "Authorize", "role": "AXButton", "background": true, "noScreenshot": true});
    if road
        .step(
            session,
            "press Authorize in the app's front window",
            "click",
            &first,
        )
        .is_some()
    {
        return;
    }
    let refused = road.last_refusal.clone().unwrap_or_default();
    if let Some(named) = named_window(&refused, fixture.ready["front"].as_u64()) {
        let look = json!({"app": app, "windowId": named, "noScreenshot": true, "background": true});
        if let Some(state) = road.step(
            session,
            "look at the window the refusal names",
            "getAppState",
            &look,
        ) && let Some(number) = number_of(&state, "Authorize")
        {
            let press = json!({
                "app": app, "windowId": named, "elementIndex": number,
                "background": true, "noScreenshot": true,
            });
            let _ = road.step(session, "press the number that look read", "click", &press);
        }
        return;
    }
    window_road(road, session, fixture, target);
}

/// The scenes and roads, each on a fresh fixture, under one helper.
fn under(session: &Session, program: &Path, root: &Path, helper: &str) -> Value {
    let mut scenes = json!({});
    for mode in ["single", "twins", "twins-same", "offset"] {
        for (name, road_of) in [
            (
                "window",
                window_road as fn(&mut Road, &Session, &Fixture, u64),
            ),
            ("app", app_road as fn(&mut Road, &Session, &Fixture, u64)),
        ] {
            if mode == "twins-same" && name == "app" {
                // Both windows carry the words: a press that names no window
                // takes the app's front one under any helper. The scene asks
                // whether a road that names the window ever presses another.
                continue;
            }
            let fixture = Fixture::start(program, root, mode, &format!("{helper}-{mode}-{name}"));
            let target = fixture.target();
            let mut road = Road::default();
            let started = Instant::now();
            road_of(&mut road, session, &fixture, target);
            // A press's effect reaches the window's record a moment after its answer.
            std::thread::sleep(Duration::from_millis(300));
            let mut report = road.report(&fixture, target);
            report["wall_ms"] = json!((started.elapsed().as_secs_f64() * 1000.0).round());
            scenes[mode][name] = report;
        }
    }
    scenes
}

/// The person's 11-20 scene under the installed helper and this build's.
#[test]
#[ignore = "drives the installed and the built helper against owned fixtures, background presses only; run by reports/cu-slow-chrome-1515/jobs/repro-twin-web.sh"]
fn twin_bench() {
    let program = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_FIXTURE").expect("fixture"));
    let baseline = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BASELINE").expect("baseline"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_OUT").expect("report path"));
    let root = out.parent().expect("report folder").join("fixtures");
    let mut report = json!({});
    for (label, helper) in [("installed", &baseline), ("built", &built)] {
        let session = Session::connect(launch_at(helper).expect("helper")).expect("answers");
        report[label] = under(&session, &program, &root, label);
    }
    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

/// The Chrome for Testing processes a run started: ended and waited on when
/// the run ends, however it ends, as a fixture's process is.
struct Launched(Vec<Child>);

impl Drop for Launched {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The flags Playwright starts Chrome for Testing with: no keychain prompt, no
/// first-run or choice screens, nothing fetched in the background.
const QUIET_CHROME: [&str; 11] = [
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
];

/// The window numbers `app` lists, in the helper's order; none while the
/// helper cannot name the app yet.
fn listed(session: &Session, app: &str) -> Vec<u64> {
    let (_, answer) = call(session, "listWindows", &json!({"app": app}));
    answer
        .ok()
        .and_then(|answer| {
            answer["windows"].as_array().map(|windows| {
                windows
                    .iter()
                    .filter_map(|window| window["id"].as_u64())
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Opens one more window of the throwaway profile on `page`, and returns its
/// number once the helper lists it, set to `frame` while it still stands alone
/// at its own.
fn chrome_window(
    session: &Session,
    chrome: &Path,
    profile: &Path,
    page: &Path,
    frame: [u64; 4],
    app: Option<&str>,
    launched: &mut Launched,
) -> (String, u64) {
    let before = app.map(|app| listed(session, app)).unwrap_or_default();
    let name = chrome
        .file_stem()
        .expect("the app's name")
        .to_string_lossy()
        .to_string();
    // The first opens the browser and lives until the run ends it; a later one
    // hands its window to it and ends. The run ends and waits on both.
    let child = crate::proc::quiet_command(chrome.join("Contents/MacOS").join(&name))
        .arg(format!("--user-data-dir={}", profile.display()))
        .args(QUIET_CHROME)
        .arg("--new-window")
        .arg(format!("file://{}", page.display()))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Chrome for Testing");
    launched.0.push(child);
    let pattern = format!(
        "Contents/MacOS/{name} --user-data-dir={}",
        profile.display()
    );
    let started = Instant::now();
    loop {
        let found = crate::proc::quiet_command("/usr/bin/pgrep")
            .arg("-o")
            .arg("-f")
            .arg(&pattern)
            .output()
            .expect("pgrep");
        let pid = String::from_utf8_lossy(&found.stdout)
            .lines()
            .next()
            .and_then(|line| line.trim().parse::<i32>().ok());
        if let Some(pid) = pid {
            let app = format!("pid:{pid}");
            if let Some(number) = listed(session, &app)
                .into_iter()
                .find(|number| !before.contains(number))
            {
                let _ = call(
                    session,
                    "windowAction",
                    &json!({"action": "move", "windowId": number, "x": frame[0], "y": frame[1]}),
                );
                let _ = call(
                    session,
                    "windowAction",
                    &json!({"action": "resize", "windowId": number, "width": frame[2], "height": frame[3]}),
                );
                return (app, number);
            }
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "a new Chrome for Testing window"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The reviewer's first question (t-121748): does the accessibility library
/// link each of Chrome's own windows to the window server's number? Two Chrome
/// for Testing windows of one throwaway profile, set to one frame, one title,
/// each showing one word — «BACK» opened first, «FRONT» last. Under each
/// helper a look at each window by its number reads that window's word, or is
/// refused: the built helper should read both, each its own; the installed
/// one refuses both, as on 11-20.
#[test]
#[ignore = "opens Chrome for Testing with a throwaway profile while the person is away; run by reports/cu-slow-chrome-1515/jobs/repro-twin-web.sh"]
fn twin_bench_chrome() {
    let chrome =
        PathBuf::from(std::env::var_os("ZEROCODE_TWIN_CHROME").expect("Chrome for Testing"));
    let baseline = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BASELINE").expect("baseline"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_CHROME_OUT").expect("report path"));
    let root = out.with_extension("run");
    assert!(
        !root.exists(),
        "a run opens in a folder of its own: {}",
        root.display()
    );
    let idle = person_idle();
    if idle < PERSON_AWAY {
        let text =
            json!({"skipped": format!("the person was at the machine {}s ago", idle.as_secs())})
                .to_string();
        std::fs::write(&out, &text).expect("the report");
        println!("{text}");
        return;
    }
    let profile = root.join("profile");
    std::fs::create_dir_all(&profile).expect("a profile folder");
    let (back_page, front_page) = (root.join("back.html"), root.join("front.html"));
    std::fs::write(
        &back_page,
        "<title>Home | Fixture</title><button>BACK</button>",
    )
    .expect("a page");
    std::fs::write(
        &front_page,
        "<title>Home | Fixture</title><button>FRONT</button>",
    )
    .expect("a page");
    // The person touched the mouse or keys since the run began: nothing more
    // is opened or asked (t-122045 I-5).
    let touched = || person_idle() < PERSON_AWAY.min(idle);
    let frame = [120, 120, 1000, 700];
    let setup = Session::connect(launch_at(&built).expect("helper")).expect("answers");
    // Chrome for Testing ends with the run, however the run ends.
    let mut launched = Launched(Vec::new());
    let (app, back) = chrome_window(
        &setup,
        &chrome,
        &profile,
        &back_page,
        frame,
        None,
        &mut launched,
    );
    let mut report = json!({"back": back});
    let mut stopped = touched();
    if !stopped {
        let (_, front) = chrome_window(
            &setup,
            &chrome,
            &profile,
            &front_page,
            frame,
            Some(&app),
            &mut launched,
        );
        report["front"] = json!(front);
        std::thread::sleep(Duration::from_millis(500));
    }
    'arms: for (label, helper) in [("installed", &baseline), ("built", &built)] {
        if stopped {
            break;
        }
        let session = Session::connect(launch_at(helper).expect("helper")).expect("answers");
        for (side, word) in [("back", "BACK"), ("front", "FRONT")] {
            if touched() {
                stopped = true;
                break 'arms;
            }
            let number = report[side].as_u64().unwrap_or_default();
            let (ms, answer) = call(
                &session,
                "getAppState",
                &json!({"app": app, "windowId": number, "noScreenshot": true, "background": true}),
            );
            report[label][side] = match answer {
                Ok(state) => {
                    let tree = state["snapshot"]["treeText"].as_str().unwrap_or_default();
                    json!({"ms": ms.round(), "reads_its_own_word": tree.contains(word), "reads_the_other": tree.contains(if word == "BACK" { "FRONT" } else { "BACK" })})
                }
                Err(refused) => json!({"ms": ms.round(), "refused": refused}),
            };
        }
    }
    report["stopped_by_the_person"] = json!(stopped);
    drop(launched);
    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

/// The third review's condition (t-122045): one key sent in the foreground to
/// the window a look named, after the app was raised for it. On one window
/// alone the key must reach it — the look's window element and the focused
/// one are the same element. On twins, a window stacked off the target and a
/// smaller window over it, the key must reach no other window. Real input, to
/// the fixture windows only, while the person is away.
#[test]
#[ignore = "sends a real key to owned fixture windows while the person is away; run by reports/cu-slow-chrome-1515/jobs/repro-twin-web.sh"]
fn twin_bench_keys() {
    let program = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_FIXTURE").expect("fixture"));
    let baseline = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BASELINE").expect("baseline"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_KEYS_OUT").expect("report path"));
    let root = out.parent().expect("report folder").join("key-fixtures");
    let mut report = json!({});
    for (label, helper) in [("installed", &baseline), ("built", &built)] {
        let session = Session::connect(launch_at(helper).expect("helper")).expect("answers");
        for mode in ["single", "twins", "offset", "small-over"] {
            let idle = person_idle();
            if idle < PERSON_AWAY {
                report[label][mode] = json!({"skipped": format!("the person was at the machine {}s ago", idle.as_secs())});
                continue;
            }
            let fixture = Fixture::start(&program, &root, mode, &format!("{label}-{mode}-keys"));
            let (app, target) = (fixture.app(), fixture.target());
            let look =
                json!({"app": app, "windowId": target, "noScreenshot": true, "background": true});
            let (_, looked) = call(&session, "getAppState", &look);
            let (ms, answer) = if looked.is_ok() && person_idle() >= PERSON_AWAY.min(idle) {
                // The borrowed desktop's road: the key goes to the fixture's own process
                // and never through the HID stream, which the person's idle time reads
                // and the front app at that instant takes (t-122428).
                let key = json!({"app": app, "windowId": target, "key": "Return", "restoreWindow": true, "mayBorrowDesktop": true});
                call(&session, "pressKey", &key)
            } else {
                (
                    0.0,
                    Err("not sent: the look was refused or the person came back".to_string()),
                )
            };
            // A key's effect reaches the window's record a moment after its answer.
            std::thread::sleep(Duration::from_millis(300));
            let keys = fixture.keys();
            report[label][mode] = json!({
                "look_refused": looked.err(),
                "ms": ms.round(),
                "refused": answer.err(),
                "key_target": keys.iter().filter(|&&number| number == target).count(),
                "key_other": keys.iter().filter(|&&number| number != target).count(),
            });
        }
    }
    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

/// A's scene 2 (t-121372): `LatePressFixture.swift`, a web-like window whose
/// «Authorize» offers its press only `delay` seconds after the app's tree was
/// switched on, beside a link that reads «Go» and shows «Delete project».
struct LateFixture {
    process: Child,
    directory: PathBuf,
}

impl LateFixture {
    fn start(program: &Path, root: &Path, mode: &str, delay: &str, name: &str) -> Self {
        let directory = root.join(name);
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).expect("fixture directory");
        let process = crate::proc::quiet_command(program)
            .arg(mode)
            .arg(&directory)
            .arg(delay)
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
        Self { process, directory }
    }

    fn app(&self) -> String {
        format!("pid:{}", self.process.id())
    }

    /// How many lines of the fixture's record begin with `what`.
    fn count(&self, what: &str) -> usize {
        std::fs::read_to_string(self.directory.join("events.log"))
            .unwrap_or_default()
            .lines()
            .filter(|line| line.starts_with(what))
            .count()
    }
}

impl Drop for LateFixture {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// The words the window hands the helper for the person's last step.
fn delete_guard() -> Value {
    json!({"delete": ["delete"]})
}

/// One background press by words on a fresh fixture, right after its first
/// look: its answer, its milliseconds and the fixture's record of presses.
fn late_press(
    session: &Session,
    program: &Path,
    root: &Path,
    name: &str,
    delay: &str,
    label: &str,
    guard: Option<Value>,
) -> Value {
    let fixture = LateFixture::start(program, root, "web", delay, name);
    let app = fixture.app();
    let _ = call(
        session,
        "getAppState",
        &json!({"app": app, "noScreenshot": true, "background": true}),
    );
    let mut press = json!({"app": app, "label": label, "background": true, "noScreenshot": true});
    if let Some(guard) = guard {
        press["confirmGuard"] = guard;
    }
    let (ms, answer) = call(session, "click", &press);
    std::thread::sleep(Duration::from_millis(300));
    json!({
        "ms": ms.round(),
        "refused": answer.err(),
        "pressed_button": fixture.count("pressed button"),
        "pressed_link": fixture.count("pressed link"),
        "ignored": fixture.count("ignored-press") + fixture.count("ignored-mouse"),
    })
}

/// Five presses by words and five by number, after the tree has long stood:
/// the helper's milliseconds for each, where nothing should wait.
fn settled_presses(
    session: &Session,
    program: &Path,
    root: &Path,
    mode: &str,
    name: &str,
) -> Value {
    let fixture = LateFixture::start(program, root, mode, "0", name);
    let app = fixture.app();
    let look = json!({"app": app, "noScreenshot": true, "background": true});
    let (_, first) = call(session, "getAppState", &look);
    // Past the settle window a press waits for nothing (t-121372).
    std::thread::sleep(Duration::from_secs(9));
    let number = first
        .ok()
        .as_ref()
        .and_then(|state| number_of(state, "Authorize"));
    let (mut by_words, mut by_number) = (vec![], vec![]);
    for _ in 0..5 {
        let (ms, _) = call(
            session,
            "click",
            &json!({"app": app, "label": "Authorize", "background": true, "noScreenshot": true}),
        );
        by_words.push(ms.round());
        if let Some(number) = number {
            let (ms, _) = call(
                session,
                "click",
                &json!({"app": app, "elementIndex": number, "background": true, "noScreenshot": true}),
            );
            by_number.push(ms.round());
        }
    }
    json!({"by_words_ms": by_words, "by_number_ms": by_number, "pressed": fixture.count("pressed")})
}

/// A's scene 2 under the installed helper and this build's: a press offered
/// late (P7), never offered (P8), the milliseconds where nothing waits (P9),
/// and a refusal that never names a control by text holding the person's
/// confirmation word (P10). Background presses only.
#[test]
#[ignore = "drives the installed and the built helper against an owned late-press fixture, background presses only; run by reports/cu-slow-chrome-1515/jobs/repro-twin-web.sh"]
fn twin_bench_late() {
    let program =
        PathBuf::from(std::env::var_os("ZEROCODE_TWIN_LATE_FIXTURE").expect("late fixture"));
    let baseline = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BASELINE").expect("baseline"));
    let built = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_BUILT").expect("built helper"));
    let out = PathBuf::from(std::env::var_os("ZEROCODE_TWIN_LATE_OUT").expect("report path"));
    let root = out.parent().expect("report folder").join("late-fixtures");
    let mut report = json!({});
    for (label, helper) in [("installed", &baseline), ("built", &built)] {
        let session = Session::connect(launch_at(helper).expect("helper")).expect("answers");
        let at = |name: &str| format!("{label}-{name}");
        report[label] = json!({
            "late_guarded": late_press(&session, &program, &root, &at("late-guarded"), "3", "Authorize", Some(delete_guard())),
            "late_plain": late_press(&session, &program, &root, &at("late-plain"), "3", "Authorize", None),
            "never": late_press(&session, &program, &root, &at("never"), "never", "Authorize", Some(delete_guard())),
            "settled_web": settled_presses(&session, &program, &root, "web", &at("settled-web")),
            "settled_native": settled_presses(&session, &program, &root, "native", &at("settled-native")),
            "words_guarded": late_press(&session, &program, &root, &at("words-guarded"), "0", "project", Some(delete_guard())),
            "words_plain": late_press(&session, &program, &root, &at("words-plain"), "0", "project", Some(json!({"payment": ["pay"]}))),
        });
    }
    let text = serde_json::to_string_pretty(&report).expect("a report");
    std::fs::write(&out, &text).expect("the report");
    println!("{text}");
}

#[test]
fn a_refusal_names_the_one_other_window_it_points_to() {
    let refused = "element_not_found: no control in Fixture's window «Home | Fixture» reads label «Authorize»; \
                   Fixture has other windows on screen: window_id 41 «Home | Fixture»; look at it by its window_id";
    assert_eq!(named_window(refused, Some(42)), Some(41));
    assert_eq!(
        named_window(refused, Some(41)),
        None,
        "the window the press was in is not another"
    );
    assert_eq!(
        named_window("element_not_found: no control reads «Authorize»", None),
        None
    );
    assert_eq!(
        named_window(
            "other windows: window_id 41 «A»; window_id 43 «B»",
            Some(42)
        ),
        None,
        "two named: the road does not guess"
    );
}

/// The helper's own refusal, word for word, from the run that found the bench
/// reading only zo's wording (j-1795173248666-0335, `twin-report.json`,
/// built · twins · app): the app road fell to the window road before.
#[test]
fn the_helpers_own_refusal_names_its_window_by_the_flag() {
    let refused = "element_not_found: no control in twin-fixture's window «Home | Fixture» reads --role AXButton --label «Authorize»; look with --marks, or find; twin-fixture has other windows on screen: --window-id 243623 «Home | Fixture»; the control may be in one of them — look at it by its --window-id";
    assert_eq!(named_window(refused, Some(243622)), Some(243623));
    assert_eq!(
        named_window(refused, Some(243623)),
        None,
        "the window the press was in is not another"
    );
}
