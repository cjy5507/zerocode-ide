//! The road from inside against a fake runner: an HTTP server that answers what a
//! real one does, and a shell line for `xcodebuild`. Nothing here touches a phone.

use std::collections::VecDeque;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::agent_emulator::real_phone::{
    self as words, MAC_POINTER_KEY, PHONE_LOCKED, PHONE_NOT_ALLOWED, PHONE_NOT_INSTALLED,
    PHONE_PATCH_MISMATCH, PHONE_SECURE_FIELD, PHONE_SOURCE_MISMATCH, PHONE_UNREACHABLE, ROAD_KEY,
    ROAD_WDA, Team, WDA_COMMIT, WDA_TREE,
};
use zerocode_core::computer_use::EmulatorPlatform;
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::marks::LOOK_ID_KEY;

use super::runner::{Life, Phase, watched};
use super::supply::{self, Tools};
use super::*;
use crate::credential_store::{MemorySecretStore, SecretStore};
use crate::emulator::marks::{self, Device};

// ---- A fake runner ----------------------------------------------------------

#[derive(Default)]
struct Fake {
    locked: AtomicBool,
    secure_focus: AtomicBool,
    no_focus: AtomicBool,
    /// Every press the runner received, as its W3C body.
    pressed: Mutex<Vec<Value>>,
    typed: Mutex<Vec<String>>,
    /// Trees answered in turn; the last one stays.
    trees: Mutex<VecDeque<Value>>,
    app: Mutex<String>,
    connections: AtomicUsize,
    requests: AtomicUsize,
    /// While a press holds, for a test of one phone's queue.
    press_ms: AtomicUsize,
    in_press: AtomicUsize,
    most_in_press: AtomicUsize,
    sessions: AtomicUsize,
    /// The session the runner knows.
    session: Mutex<Option<String>>,
    /// What `/wda/locked` answers instead of true or false, for a runner that
    /// says something else.
    locked_word: Mutex<Option<Value>>,
}

fn fake_runner() -> (Arc<Fake>, String) {
    let fake = Arc::new(Fake {
        app: Mutex::new(words::SPRINGBOARD.to_string()),
        ..Fake::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    let serving = Arc::clone(&fake);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            serving.connections.fetch_add(1, Ordering::SeqCst);
            let one = Arc::clone(&serving);
            std::thread::spawn(move || serve(&one, stream));
        }
    });
    (fake, base)
}

fn serve(fake: &Fake, stream: TcpStream) {
    let mut out = stream.try_clone().expect("a clone");
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let method = parts.next().unwrap_or_default().to_string();
        let path = parts.next().unwrap_or_default().to_string();
        let mut length = 0;
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap_or(0) == 0 {
                return;
            }
            if header.trim().is_empty() {
                break;
            }
            let lower = header.to_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        fake.requests.fetch_add(1, Ordering::SeqCst);
        let (status, answer) = fake.answer(&method, &path, &body);
        let text = answer.to_string();
        let reply = format!(
            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{text}",
            text.len()
        );
        if out.write_all(reply.as_bytes()).is_err() {
            return;
        }
    }
}

impl Fake {
    fn answer(&self, method: &str, path: &str, body: &Value) -> (u16, Value) {
        let value = |value: Value| (200, json!({ "value": value }));
        if path == "/wda/locked" {
            if let Some(word) = self.locked_word.lock().unwrap().clone() {
                return value(word);
            }
            return value(json!(self.locked.load(Ordering::SeqCst)));
        }
        if path == "/session" {
            let id = format!("s-{}", self.sessions.fetch_add(1, Ordering::SeqCst));
            *self.session.lock().unwrap() = Some(id.clone());
            return (200, json!({ "sessionId": id, "value": {} }));
        }
        if path == "/wda/homescreen" {
            return value(Value::Null);
        }
        if path == "/screenshot" {
            // Three bytes, base64.
            return value(json!("AAEC"));
        }
        if let Some(rest) = path.strip_prefix("/session/") {
            let (id, rest) = rest.split_once('/').unwrap_or((rest, ""));
            if self.session.lock().unwrap().as_deref() != Some(id) {
                return (404, json!({ "value": { "error": "invalid session id" } }));
            }
            let rest = format!("/{rest}");
            if rest.starts_with("/source") {
                assert!(rest.contains("excluded_attributes=visible"), "{rest}");
                let mut trees = self.trees.lock().unwrap();
                let tree = if trees.len() > 1 {
                    trees.pop_front().unwrap()
                } else {
                    trees.front().cloned().unwrap_or(Value::Null)
                };
                return value(tree);
            }
            match rest.as_str() {
                "/appium/settings" => return value(json!({})),
                "/wda/activeAppInfo" => {
                    return value(json!({ "bundleId": self.app.lock().unwrap().clone() }));
                }
                "/window/size" => return value(json!({ "width": 400, "height": 800 })),
                "/element/active" => {
                    if self.no_focus.load(Ordering::SeqCst) {
                        return (404, json!({ "value": { "error": "no such element" } }));
                    }
                    return value(json!({ "ELEMENT": "e1" }));
                }
                "/element/e1/name" => {
                    return value(json!(if self.secure_focus.load(Ordering::SeqCst) {
                        "XCUIElementTypeSecureTextField"
                    } else {
                        "XCUIElementTypeTextField"
                    }));
                }
                "/wda/keys" => {
                    let text: String = body["value"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_str)
                        .collect();
                    self.typed.lock().unwrap().push(text);
                    return value(Value::Null);
                }
                "/actions" if method == "POST" => {
                    if self.locked.load(Ordering::SeqCst) {
                        return (500, json!({ "value": { "error": "unknown error" } }));
                    }
                    let now = self.in_press.fetch_add(1, Ordering::SeqCst) + 1;
                    self.most_in_press.fetch_max(now, Ordering::SeqCst);
                    let hold = self.press_ms.load(Ordering::SeqCst) as u64;
                    if hold > 0 {
                        std::thread::sleep(Duration::from_millis(hold));
                    }
                    self.pressed.lock().unwrap().push(body.clone());
                    self.in_press.fetch_sub(1, Ordering::SeqCst);
                    return value(Value::Null);
                }
                _ => {}
            }
        }
        (404, json!({ "value": { "error": "unknown command" } }))
    }

    fn set_tree(&self, tree: Value) {
        let mut trees = self.trees.lock().unwrap();
        trees.clear();
        trees.push_back(tree);
    }

    fn press_count(&self) -> usize {
        self.pressed.lock().unwrap().len()
    }

    fn pressed_at(&self, index: usize) -> (f64, f64) {
        let pressed = self.pressed.lock().unwrap();
        let first = &pressed[index]["actions"][0]["actions"][0];
        (first["x"].as_f64().unwrap(), first["y"].as_f64().unwrap())
    }
}

struct FakeRoad {
    base: Mutex<String>,
    allowed: AtomicBool,
    installed: AtomicBool,
    /// What the phone itself says of its lock, as `devicectl` would.
    lock: Mutex<Option<bool>>,
    /// How many times that was asked.
    lock_reads: AtomicUsize,
}

impl FakeRoad {
    fn new(base: String) -> Self {
        Self {
            base: Mutex::new(base),
            allowed: AtomicBool::new(true),
            installed: AtomicBool::new(true),
            lock: Mutex::new(None),
            lock_reads: AtomicUsize::new(0),
        }
    }
}

impl Road for FakeRoad {
    fn allowed(&self) -> bool {
        self.allowed.load(Ordering::SeqCst)
    }

    fn installed(&self) -> bool {
        self.installed.load(Ordering::SeqCst)
    }

    fn base(&self) -> Result<String, String> {
        Ok(self.base.lock().unwrap().clone())
    }

    fn locked_roads(&self) -> LockedRoads {
        LockedRoads {
            mirroring_installed: true,
            mirroring_running: false,
            shared_desktop: false,
        }
    }

    fn locked_now(&self) -> Option<bool> {
        self.lock_reads.fetch_add(1, Ordering::SeqCst);
        *self.lock.lock().unwrap()
    }
}

/// A shell line that prints the runner's ready line and stays, as `xcodebuild` does.
fn stays() -> runner::Spawn {
    Arc::new(|| {
        watched(
            "/bin/sh",
            &[
                "-c".to_string(),
                "echo 'x ServerURLHere->http://0.0.0.0:8100<-ServerURLHere'; exec sleep 600"
                    .to_string(),
            ],
        )
        .spawn()
    })
}

fn never_unattended() -> runner::Unattended {
    Arc::new(|| false)
}

static NEXT_PHONE: AtomicUsize = AtomicUsize::new(0);

struct Bench {
    fake: Arc<Fake>,
    road: Arc<FakeRoad>,
    handle: String,
    life: Arc<Life>,
}

fn phone_on_a_fake() -> Bench {
    let (fake, base) = fake_runner();
    let handle = format!("phone-test{:04}", NEXT_PHONE.fetch_add(1, Ordering::SeqCst));
    let road = Arc::new(FakeRoad::new(base));
    let life = Life::new(
        stays(),
        never_unattended(),
        Duration::from_secs(600),
        Vec::new(),
    );
    register(
        &handle,
        Arc::clone(&road) as Arc<dyn Road>,
        Arc::clone(&life),
    )
    .expect("registered");
    Bench {
        fake,
        road,
        handle,
        life,
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        self.life.stop();
    }
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(future)
}

fn sized(kind: &str, label: &str, x: f64, y: f64, w: f64, h: f64) -> Value {
    json!({ "type": kind, "label": label, "rect": { "x": x, "y": y, "width": w, "height": h } })
}

fn screen(children: Vec<Value>) -> Value {
    json!({ "type": "Application", "label": "Synthetic",
            "rect": { "x": 0, "y": 0, "width": 400, "height": 800 }, "children": children })
}

fn device_of(bench: &Bench) -> Device {
    Device {
        address: bench.handle.clone(),
        identity: bench.handle.clone(),
    }
}

/// `marks`, and the number of the mark with this name.
fn look(bench: &Bench, name: &str) -> (String, usize) {
    let answer = run(marks::observe(
        EmulatorPlatform::Ios,
        device_of(bench),
        None,
    ))
    .expect("marks");
    let id = answer[LOOK_ID_KEY].as_str().expect("a look").to_string();
    let legend = answer["legend"].as_str().unwrap_or_default().to_string();
    let line = legend
        .lines()
        .find(|line| line.contains(name))
        .unwrap_or_else(|| panic!("no mark for {name} in:\n{legend}"));
    let digits: String = line
        .chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect();
    (id, digits.parse().expect("a number"))
}

fn press(bench: &Bench, look_id: &str, mark: usize) -> Result<Value, ProviderError> {
    press_with(bench, look_id, mark, None, false)
}

fn press_with(
    bench: &Bench,
    look_id: &str,
    mark: usize,
    count: Option<&str>,
    preview: bool,
) -> Result<Value, ProviderError> {
    run(marks::click(
        EmulatorPlatform::Ios,
        device_of(bench),
        mark,
        look_id,
        count,
        preview,
    ))
}

/// The number of the legend line with this name in a look's answer.
fn mark_in(answer: &Value, name: &str) -> Option<usize> {
    let legend = answer["legend"].as_str()?;
    let line = legend.lines().find(|line| line.contains(name))?;
    line.chars()
        .skip_while(|ch| !ch.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

// ---- 인수 기준 하나씩 --------------------------------------------------------

/// 번호로 누르기: the simulator's verbs, on a phone's tree, with the point in the
/// phone's own units and the answer saying which road pressed.
#[test]
fn a_press_by_number_lands_on_the_sized_control_and_names_its_road() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![
        sized("Icon", "Weather", 0.0, 0.0, 0.0, 0.0),
        sized("Icon", "Weather", 100.0, 200.0, 60.0, 60.0),
    ]));
    let (id, mark) = look(&bench, "Weather");
    let answer = press(&bench, &id, mark).expect("pressed");
    assert_eq!(answer[ROAD_KEY], ROAD_WDA);
    assert_eq!(answer[MAC_POINTER_KEY], false);
    assert_eq!(bench.fake.press_count(), 1);
    let (x, y) = bench.fake.pressed_at(0);
    assert!(
        (x - 130.0).abs() < 0.01 && (y - 230.0).abs() < 0.01,
        "숨은 요소를 눌렀다: {x} {y}"
    );
}

/// 잠금 거절: nothing goes out, the code is fixed and the road open now is said.
#[test]
fn a_locked_phone_refuses_every_verb_and_presses_nothing() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    let (id, mark) = look(&bench, "Go");
    bench.fake.locked.store(true, Ordering::SeqCst);
    let refused = press(&bench, &id, mark).unwrap_err();
    assert_eq!(refused.code, PHONE_LOCKED);
    assert!(
        refused.message.contains("com.apple.ScreenContinuity")
            || refused.message.contains("not running")
    );
    assert!(
        refused.message.contains("background only"),
        "{}",
        refused.message
    );
    for result in [
        run(tap(&bench.handle, 0.5, 0.5)),
        run(swipe(&bench.handle, (0.1, 0.1), (0.2, 0.2), None)),
        run(text(&bench.handle, "x")),
        run(button(&bench.handle, "home")),
        run(tree(&bench.handle)),
        run(marks::observe(
            EmulatorPlatform::Ios,
            device_of(&bench),
            None,
        )),
    ] {
        assert_eq!(result.unwrap_err().code, PHONE_LOCKED);
    }
    let shot = run(screenshot(&bench.handle)).unwrap_err();
    assert!(shot.starts_with(PHONE_LOCKED), "{shot}");
    assert_eq!(bench.fake.press_count(), 0);
    assert!(bench.fake.typed.lock().unwrap().is_empty());
}

/// t-93254 (a): a press by number waits for the screen it led to, as a simulator's
/// does, and answers how it settled, the words counted there, and the next screen's
/// numbers as a look to press by — one call a step, no `marks` between.
#[test]
fn a_press_waits_for_its_screen_and_its_preview_is_the_next_look() {
    let bench = phone_on_a_fake();
    let list = screen(vec![sized("Button", "Inbox", 10.0, 100.0, 300.0, 60.0)]);
    let opened = screen(vec![
        sized("Button", "Back", 10.0, 40.0, 80.0, 40.0),
        sized("Button", "Reply", 10.0, 600.0, 120.0, 50.0),
    ]);
    {
        // The look, the press's own read, then the screen it opened, which stays.
        let mut trees = bench.fake.trees.lock().unwrap();
        trees.clear();
        trees.extend([list.clone(), list, opened]);
    }
    let (id, mark) = look(&bench, "Inbox");
    let requests = bench.fake.requests.load(Ordering::SeqCst);
    let answer = press_with(&bench, &id, mark, Some("Reply"), true).expect("pressed");
    assert_eq!(answer[ROAD_KEY], ROAD_WDA, "{answer}");
    assert_eq!(answer[MAC_POINTER_KEY], false, "{answer}");
    assert_eq!(answer["settle"]["settle"], "still", "{answer}");
    assert_eq!(answer["landed"], words::LANDED_SCREEN_CHANGED, "{answer}");
    assert_eq!(answer["count"], 1, "{answer}");
    let preview = &answer["preview"];
    let next = preview[LOOK_ID_KEY]
        .as_str()
        .expect("the preview is a look");
    let reply = mark_in(preview, "Reply").expect("the next screen's numbers");
    assert!(
        mark_in(preview, "Inbox").is_none(),
        "the screen it left: {preview}"
    );
    // Two reads after the press settle a screen that changed at once.
    let spent = bench.fake.requests.load(Ordering::SeqCst) - requests;
    assert!(spent <= 10, "{spent} requests for one step");
    let pressed = press(&bench, next, reply).expect("the preview's number presses");
    assert_eq!(bench.fake.press_count(), 2, "{pressed}");
    let (x, y) = bench.fake.pressed_at(1);
    assert!(
        (x - 70.0).abs() < 0.01 && (y - 625.0).abs() < 0.01,
        "{x} {y}"
    );
}

/// t-93254 (a): a press that moved nothing says so, and a tap by position waits the
/// same way.
#[test]
fn a_press_that_moved_nothing_says_the_screen_did_not_change() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    let (id, mark) = look(&bench, "Go");
    let answer = press(&bench, &id, mark).expect("pressed");
    assert_eq!(answer["settle"]["settle"], "unmoved", "{answer}");
    assert_eq!(answer["landed"], words::LANDED_SCREEN_UNCHANGED, "{answer}");
    let tapped = run(tap(&bench.handle, 0.9, 0.9)).expect("tap");
    assert_eq!(tapped["settle"]["settle"], "unmoved", "{tapped}");
    assert_eq!(tapped["landed"], words::LANDED_SCREEN_UNCHANGED, "{tapped}");
    assert_eq!(tapped[ROAD_KEY], ROAD_WDA, "{tapped}");
    assert_eq!(tapped[MAC_POINTER_KEY], false, "{tapped}");
}

/// t-93254 (b): with no runner up, a locked phone is refused on the phone's own word
/// by every read too — `marks`, `find`, `tree` and `screenshot` — never answered as
/// an empty screen, and no runner is started for it.
#[test]
fn a_locked_phone_is_refused_by_every_read_before_a_runner_starts() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![]));
    *bench.road.lock.lock().unwrap() = Some(true);
    let marks_said = run(marks::observe(
        EmulatorPlatform::Ios,
        device_of(&bench),
        None,
    ))
    .unwrap_err();
    assert_eq!(marks_said.code, PHONE_LOCKED, "{marks_said:?}");
    let find_said = run(crate::emulator::checks::observe(
        EmulatorPlatform::Ios,
        device_of(&bench),
        zerocode_core::computer_use::EmulatorMethod::Find,
        "anything",
    ))
    .unwrap_err();
    assert_eq!(find_said.code, PHONE_LOCKED, "{find_said:?}");
    assert_eq!(run(tree(&bench.handle)).unwrap_err().code, PHONE_LOCKED);
    let shot = run(screenshot(&bench.handle)).unwrap_err();
    assert!(shot.starts_with(PHONE_LOCKED), "{shot}");
    assert_eq!(
        bench.life.starts(),
        0,
        "a runner was started on a locked phone"
    );
    assert_eq!(bench.fake.requests.load(Ordering::SeqCst), 0);
}

/// 검수 t-93358 P2: once a runner stands, its own lock answer (about 2 ms) is the
/// only one asked: no read or press waits for `devicectl` (333–1,477 ms), a
/// screenshot least of all, and an empty screen is a screen.
#[test]
fn a_standing_runner_answers_the_lock_and_devicectl_is_not_asked() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![]));
    run(tree(&bench.handle)).expect("started");
    let before = bench.road.lock_reads.load(Ordering::SeqCst);
    assert_eq!(before, 1, "the lock is read once, before the start");
    assert_eq!(
        run(screenshot(&bench.handle)).expect("a screenshot"),
        vec![0, 1, 2]
    );
    run(marks::observe(
        EmulatorPlatform::Ios,
        device_of(&bench),
        None,
    ))
    .expect("an empty screen is a screen");
    run(tree(&bench.handle)).expect("a tree");
    run(tap(&bench.handle, 0.5, 0.5)).expect("a tap");
    assert_eq!(
        bench.road.lock_reads.load(Ordering::SeqCst),
        before,
        "devicectl was asked while the runner stood"
    );
    bench.fake.locked.store(true, Ordering::SeqCst);
    let shot = run(screenshot(&bench.handle)).unwrap_err();
    assert!(shot.starts_with(PHONE_LOCKED), "{shot}");
}

/// t-93254 (c): with no runner up, a locked phone is refused on the phone's own word
/// before any start: nothing is spawned, the runner is not asked, and the lock is
/// read once. `phone-check`'s runner item takes the same road.
#[test]
fn a_locked_phone_is_refused_before_a_runner_is_started() {
    let bench = phone_on_a_fake();
    *bench.road.lock.lock().unwrap() = Some(true);
    assert_eq!(bench.life.phase(), Phase::Down);
    let refused = run(tap(&bench.handle, 0.5, 0.5)).unwrap_err();
    assert_eq!(refused.code, PHONE_LOCKED, "{refused:?}");
    assert_eq!(
        bench.life.starts(),
        0,
        "a runner was started on a locked phone"
    );
    assert_eq!(bench.road.lock_reads.load(Ordering::SeqCst), 1);
    assert_eq!(bench.fake.requests.load(Ordering::SeqCst), 0);
    // An open phone: the lock is read once before the start, and not again while
    // the runner stands.
    *bench.road.lock.lock().unwrap() = Some(false);
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    run(tree(&bench.handle)).expect("started");
    run(tree(&bench.handle)).expect("read again");
    assert_eq!(bench.life.starts(), 1);
    assert_eq!(bench.road.lock_reads.load(Ordering::SeqCst), 2);
}

/// 잠금, 그 사이에: the phone locks between the check and the press, the runner
/// answers 500, and the refusal is still the lock's.
#[test]
fn a_phone_that_locks_under_the_press_is_refused_as_locked() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    let mut wda = run(async { driven(&bench.handle).await.expect("ready").1 });
    bench.fake.locked.store(true, Ordering::SeqCst);
    let error = run(wda.actions(&words::tap_actions(1.0, 1.0))).unwrap_err();
    let phone = phone(&bench.handle).expect("known");
    let said = run(after_failed_act(&phone, &mut wda, error));
    assert_eq!(said.code, PHONE_LOCKED);
}

/// 겹친 요소: no number, so nothing to press on a guess.
#[test]
fn two_controls_over_one_point_get_no_number() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![
        sized("Button", "Under", 100.0, 100.0, 100.0, 100.0),
        sized("Button", "Over", 110.0, 110.0, 100.0, 100.0),
        sized("Button", "Clear", 100.0, 500.0, 100.0, 100.0),
    ]));
    let answer = run(marks::observe(
        EmulatorPlatform::Ios,
        device_of(&bench),
        None,
    ))
    .expect("marks");
    let legend = answer["legend"].as_str().unwrap();
    assert!(legend.contains("Clear"), "{legend}");
    assert!(
        !legend.contains("Under") && !legend.contains("Over"),
        "{legend}"
    );
}

/// 화면 전환 중 / 번호가 낡음: a mark that moved, a screen that changed and an app
/// that came to the front are refused, and nothing is pressed.
#[test]
fn a_stale_number_is_refused_and_presses_nothing() {
    let bench = phone_on_a_fake();
    let first = screen(vec![sized("Button", "Send", 10.0, 500.0, 100.0, 50.0)]);

    bench.fake.set_tree(first.clone());
    let (id, mark) = look(&bench, "Send");
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Send", 10.0, 100.0, 100.0, 50.0,
    )]));
    assert_eq!(
        press(&bench, &id, mark).unwrap_err().code,
        error_code::PIN_BROKEN,
        "the control moved"
    );

    bench.fake.set_tree(first.clone());
    let (id, mark) = look(&bench, "Send");
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Delete", 10.0, 500.0, 100.0, 50.0,
    )]));
    assert_eq!(
        press(&bench, &id, mark).unwrap_err().code,
        error_code::PIN_BROKEN,
        "other words stand there now"
    );

    bench.fake.set_tree(first);
    let (id, mark) = look(&bench, "Send");
    *bench.fake.app.lock().unwrap() = "com.example.other".to_string();
    assert_eq!(
        press(&bench, &id, mark).unwrap_err().code,
        error_code::PIN_BROKEN,
        "another app came to the front with the same words in the same place"
    );
    assert_eq!(bench.fake.press_count(), 0, "틀린 곳을 눌렀다");
}

/// 결제·이체: the person's policy stops a number and a position alike, at once, with
/// nobody asked and the fixed code.
#[test]
fn a_payment_and_a_transfer_stop_at_once_by_number_and_by_position() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![
        sized("Button", "결제하기", 10.0, 100.0, 100.0, 50.0),
        sized("Button", "송금 확인", 10.0, 300.0, 100.0, 50.0),
    ]));
    let (id, mark) = look(&bench, "결제하기");
    let began = Instant::now();
    assert_eq!(
        press(&bench, &id, mark).unwrap_err().code,
        error_code::CONFIRMATION_REQUIRED
    );
    assert_eq!(
        run(tap(&bench.handle, 60.0 / 400.0, 325.0 / 800.0))
            .unwrap_err()
            .code,
        error_code::CONFIRMATION_REQUIRED
    );
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "it waited for someone"
    );
    assert_eq!(bench.fake.press_count(), 0);
    run(tap(&bench.handle, 0.9, 0.9)).expect("an empty place takes a tap");
    assert_eq!(bench.fake.press_count(), 1);
}

/// 비밀번호 칸: the tree's type refuses, with the fixed code; a field with no focus
/// is not typed into either; a plain one takes Korean whole.
#[test]
fn a_password_field_refuses_typing_and_korean_goes_in_whole() {
    let bench = phone_on_a_fake();
    bench.fake.secure_focus.store(true, Ordering::SeqCst);
    assert_eq!(
        run(text(&bench.handle, "secret")).unwrap_err().code,
        PHONE_SECURE_FIELD
    );
    assert!(bench.fake.typed.lock().unwrap().is_empty());

    bench.fake.secure_focus.store(false, Ordering::SeqCst);
    bench.fake.no_focus.store(true, Ordering::SeqCst);
    assert_eq!(
        run(text(&bench.handle, "x")).unwrap_err().code,
        error_code::INVALID_ARGUMENT
    );

    bench.fake.no_focus.store(false, Ordering::SeqCst);
    let answer = run(text(&bench.handle, "안녕하세요 abc")).expect("typed");
    assert_eq!(answer[MAC_POINTER_KEY], false);
    assert_eq!(*bench.fake.typed.lock().unwrap(), ["안녕하세요 abc"]);
}

/// 동의 없음·빌드 없음: refused before anything starts.
#[test]
fn no_consent_and_no_build_start_nothing() {
    let bench = phone_on_a_fake();
    bench.road.allowed.store(false, Ordering::SeqCst);
    let refused = run(tap(&bench.handle, 0.5, 0.5)).unwrap_err();
    assert_eq!(refused.code, PHONE_NOT_ALLOWED);
    assert!(refused.message.contains("phone-install"));

    bench.road.allowed.store(true, Ordering::SeqCst);
    bench.road.installed.store(false, Ordering::SeqCst);
    assert_eq!(
        run(tap(&bench.handle, 0.5, 0.5)).unwrap_err().code,
        PHONE_NOT_INSTALLED
    );
    assert_eq!(bench.life.starts(), 0);
    assert_eq!(bench.fake.requests.load(Ordering::SeqCst), 0);
}

/// 에이전트가 스스로 켜지 못함: what the settings file holds is a seal. A word typed
/// into it, a seal for another team and a seal from another key turn nothing on.
#[test]
fn a_value_typed_into_the_settings_file_does_not_turn_the_road_on() {
    let seal = Seal::with_store(Arc::new(MemorySecretStore::default()) as Arc<dyn SecretStore>);
    assert!(!seal.holds(Said::Allowed, "TEAM", "phone-a", "true"));
    assert!(!seal.holds(Said::Allowed, "", "", ""));

    let answer = seal
        .sealed(Said::Allowed, "TEAM", "phone-a")
        .expect("sealed");
    assert!(seal.holds(Said::Allowed, "TEAM", "phone-a", &answer));
    assert!(!seal.holds(Said::Allowed, "OTHER", "phone-a", &answer));
    assert!(!seal.holds(Said::Allowed, "TEAM", "phone-b", &answer));
    assert!(!seal.holds(Said::Unattended, "TEAM", "phone-a", &answer));

    let another = Seal::with_store(Arc::new(MemorySecretStore::default()) as Arc<dyn SecretStore>);
    assert_eq!(
        another.tag(Said::Allowed.word(), "TEAM", "phone-a", false),
        None
    );
    let forged = another
        .sealed(Said::Allowed, "TEAM", "phone-a")
        .expect("sealed");
    assert!(!seal.holds(Said::Allowed, "TEAM", "phone-a", &forged));
}

/// 에이전트가 스스로 켜지 못함: the window's press road refuses the switch to an
/// agent, and no flag of any verb can say yes for the person.
#[test]
fn an_agent_cannot_reach_the_switch_or_answer_the_card() {
    use crate::computer_use::self_surface::{PERSON_ONLY_COMMANDS, person_only_refusal};
    for command in ["set_emulator_real_phone", "computer_confirm_answer"] {
        assert!(PERSON_ONLY_COMMANDS.contains(&command), "{command}");
        assert!(
            person_only_refusal(command, Some("agent")).is_some(),
            "{command}"
        );
        assert!(person_only_refusal(command, None).is_some(), "{command}");
    }
    for words_of in [
        ["phone-install", "--device", "phone-x", "--allow"],
        ["phone-install", "--device", "phone-x", "--yes"],
        ["phone-check", "--device", "phone-x", "--team"],
    ] {
        let argv: Vec<String> = words_of.iter().map(ToString::to_string).collect();
        assert!(zerocode_core::computer_use::parse_emulator_command(&argv).is_err());
    }
    // A recipe's walk cannot start setup: its table does not know these words.
    for verb in ["phone-install", "phone-remove", "phone-check"] {
        assert!(zerocode_core::agent_emulator::emulator_verb(verb).is_none());
    }
}

/// `walk`: a walk's steps are the door's words, and a phone's id takes the verbs with
/// their usual counts; there the door routes by the id alone.
#[test]
fn a_walks_steps_fit_a_phones_id() {
    let id = words::phone_handle("synthetic");
    assert!(words::is_real_phone(&id));
    for (verb, rest) in [
        ("marks", vec![]),
        ("tap", vec!["--x", "0.5", "--y", "0.5"]),
        ("text", vec!["--text", "hi"]),
    ] {
        let mut argv = vec![verb, "--platform", "ios", "--device", id.as_str()];
        argv.extend(rest);
        let argv: Vec<String> = argv.iter().map(ToString::to_string).collect();
        assert_eq!(
            zerocode_core::agent_emulator::arity_ok(&argv),
            Ok(()),
            "{verb}"
        );
        assert_eq!(
            zerocode_core::computer_use::parse_emulator_command(&argv)
                .expect("parsed")
                .device
                .as_deref(),
            Some(id.as_str())
        );
    }
}

/// 세션·연결 재사용: many verbs, one connection, one session.
#[test]
fn many_verbs_use_one_connection_and_one_session() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    for _ in 0..5 {
        run(tree(&bench.handle)).expect("a tree");
    }
    assert_eq!(bench.fake.sessions.load(Ordering::SeqCst), 1);
    assert!(
        bench.fake.connections.load(Ordering::SeqCst) <= 1,
        "{} connections",
        bench.fake.connections.load(Ordering::SeqCst)
    );
}

/// A session the runner forgot is opened again, and the verb goes through.
#[test]
fn a_session_the_runner_forgot_is_opened_again() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    run(tree(&bench.handle)).expect("a tree");
    *bench.fake.session.lock().unwrap() = None;
    run(tree(&bench.handle)).expect("a tree again");
    assert_eq!(bench.fake.sessions.load(Ordering::SeqCst), 2);
}

/// 러너 없음 → 다시 띄움; 러너 죽음 복구: the next verb brings it back with no build.
#[test]
fn a_runner_that_is_down_or_died_comes_back_on_the_next_verb() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    assert_eq!(bench.life.phase(), Phase::Down);
    run(tree(&bench.handle)).expect("started on demand");
    assert_eq!(bench.life.phase(), Phase::Up);
    assert_eq!(bench.life.starts(), 1);
    bench.life.stop_and_wait();
    assert_eq!(bench.life.phase(), Phase::Down);
    run(tree(&bench.handle)).expect("started again");
    assert_eq!(bench.life.starts(), 2);
}

/// 터널: a phone's address moved; the next verb reads it again and goes on.
#[test]
fn a_tunnel_that_moved_is_read_again_on_the_next_verb() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    let right = bench.road.base.lock().unwrap().clone();
    *bench.road.base.lock().unwrap() = "http://127.0.0.1:9".to_string();
    let wrong = run(tree(&bench.handle));
    *bench.road.base.lock().unwrap() = right;
    assert_eq!(wrong.unwrap_err().code, PHONE_UNREACHABLE);
    run(tree(&bench.handle)).expect("back on the new address");
}

/// 무인 꺼짐: nobody using it, it ends after the idle time.
#[test]
fn nobody_using_it_stops_the_runner_when_unattended_is_off() {
    let life = Life::new(
        stays(),
        never_unattended(),
        Duration::from_millis(300),
        Vec::new(),
    );
    life.ensure_up(Duration::from_secs(10)).expect("up");
    assert_eq!(life.phase(), Phase::Up);
    let began = Instant::now();
    wait_for(&life, Phase::Down, Duration::from_secs(10));
    assert!(began.elapsed() >= Duration::from_millis(250));
}

/// 무인 켜짐: it does not end for being unused, it comes back when it dies, the
/// waits grow, and past the ceiling it stops trying and says so.
#[test]
fn unattended_keeps_it_up_brings_it_back_and_stops_at_a_ceiling() {
    let alive = Arc::new(AtomicBool::new(true));
    let alive_in = Arc::clone(&alive);
    let spawn: runner::Spawn = Arc::new(move || {
        // It announces itself and then dies at once while `alive` is off.
        let line = if alive_in.load(Ordering::SeqCst) {
            "echo 'ServerURLHere->x'; exec sleep 600"
        } else {
            "echo 'ServerURLHere->x'; exit 3"
        };
        watched("/bin/sh", &["-c".to_string(), line.to_string()]).spawn()
    });
    let life = Life::new(
        spawn,
        Arc::new(|| true),
        Duration::from_millis(100),
        vec![Duration::from_millis(50), Duration::from_millis(100)],
    );
    life.ensure_up(Duration::from_secs(10)).expect("up");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(life.phase(), Phase::Up, "무인인데 끝냈다");

    alive.store(false, Ordering::SeqCst);
    life.stop_and_wait();
    let before = life.starts();
    assert_eq!(life.phase(), Phase::Down, "a person's stop stays stopped");
    let _ = life.ensure_up(Duration::from_secs(10));
    wait_for(&life, Phase::GaveUp, Duration::from_secs(20));
    assert_eq!(life.starts(), before + 3, "one on demand and two restarts");

    alive.store(true, Ordering::SeqCst);
    life.ensure_up(Duration::from_secs(10))
        .expect("a call does not wait: the person may have fixed it");
    assert_eq!(life.phase(), Phase::Up);
    life.stop();
}

fn wait_for(life: &Life, phase: Phase, cap: Duration) {
    let began = Instant::now();
    while life.phase() != phase {
        assert!(
            began.elapsed() < cap,
            "still {:?} after {cap:?}, waiting for {phase:?}",
            life.phase()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A locked phone cannot start a runner, and that start's end is told as the lock.
#[test]
fn a_runner_that_fails_on_a_locked_phone_is_told_as_the_lock() {
    let spawn: runner::Spawn = Arc::new(|| {
        watched(
            "/bin/sh",
            &[
                "-c".to_string(),
                "echo 'Failed to initialize for UI testing: LocalAuthentication -4'; exit 65"
                    .to_string(),
            ],
        )
        .spawn()
    });
    let life = Life::new(
        spawn,
        never_unattended(),
        Duration::from_secs(60),
        Vec::new(),
    );
    assert_eq!(
        life.ensure_up(Duration::from_secs(10)),
        Err(NotUp::Failed(RunnerFailure::Locked))
    );
}

/// 창이 꺼지면: the pipe ends with the window, and the shell ends what runs under it.
/// (A child holding this test's pipe stands in for the window.)
#[test]
fn the_runner_ends_when_its_pipe_ends_however_the_window_ended() {
    let marker = tempfile::tempdir().expect("a folder");
    let pid_file = marker.path().join("pid");
    let script = format!("echo $$ > '{}'; exec sleep 600", pid_file.display());
    let mut child = watched("/bin/sh", &["-c".to_string(), script])
        .spawn()
        .expect("spawned");
    let began = Instant::now();
    while !pid_file.exists()
        || std::fs::read_to_string(&pid_file)
            .unwrap_or_default()
            .is_empty()
    {
        assert!(
            began.elapsed() < Duration::from_secs(10),
            "it never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = std::fs::read_to_string(&pid_file).unwrap();
    let pid = pid.trim();
    let alive = |pid: &str| {
        crate::proc::quiet_command("/bin/kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    assert!(alive(pid));
    drop(child.stdin.take());
    let began = Instant::now();
    while alive(pid) {
        assert!(
            began.elapsed() < Duration::from_secs(8),
            "the runner outlived its window"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.wait();
}

/// 기기당 직렬: two verbs at one phone queue; two phones do not wait for each other.
#[test]
fn one_phone_serves_one_verb_at_a_time_and_two_phones_do_not_wait() {
    let one = phone_on_a_fake();
    let other = phone_on_a_fake();
    for bench in [&one, &other] {
        bench.fake.press_ms.store(300, Ordering::SeqCst);
        bench.fake.set_tree(screen(vec![]));
    }
    let same = run(async {
        let began = Instant::now();
        let (first, second) = tokio::join!(tap(&one.handle, 0.5, 0.5), tap(&one.handle, 0.6, 0.6),);
        first.expect("first");
        second.expect("second");
        began.elapsed()
    });
    assert_eq!(
        one.fake.most_in_press.load(Ordering::SeqCst),
        1,
        "both at once"
    );
    assert!(same >= Duration::from_millis(600), "{same:?}");

    let two = run(async {
        let began = Instant::now();
        let (first, second) =
            tokio::join!(tap(&one.handle, 0.5, 0.5), tap(&other.handle, 0.5, 0.5),);
        first.expect("first");
        second.expect("second");
        began.elapsed()
    });
    // Each tap now waits for its screen too, so one phone's two taps take two
    // taps' time and two phones' one each.
    assert!(
        two * 3 < same * 2,
        "two phones waited: {two:?} against {same:?}"
    );
}

// ---- 공급 ----------------------------------------------------------------------

/// Tools that answer as git and Xcode would, and write down every command.
#[derive(Default)]
struct Script {
    calls: Vec<String>,
    tag: Option<String>,
    head: Option<String>,
    tree: Option<String>,
    patch_fails: bool,
    /// `xcodebuild` writes the plan its build makes.
    build_writes: Option<PathBuf>,
}

impl Tools for Script {
    fn run(&mut self, program: &str, args: &[String], _cwd: &Path) -> Result<String, String> {
        let line = format!("{program} {}", args.join(" "));
        self.calls.push(line.clone());
        if line.contains("ls-remote") {
            return Ok(format!(
                "{}\trefs/tags/{}\n",
                self.tag.as_deref().unwrap_or(WDA_COMMIT),
                words::WDA_TAG
            ));
        }
        if line.contains("FETCH_HEAD^{tree}") {
            return Ok(format!("{}\n", self.tree.as_deref().unwrap_or(WDA_TREE)));
        }
        if line.contains("rev-parse FETCH_HEAD") {
            return Ok(format!("{}\n", self.head.as_deref().unwrap_or(WDA_COMMIT)));
        }
        if line.contains(" apply ") && self.patch_fails {
            return Err("patch does not apply".to_string());
        }
        if program == "xcodebuild"
            && let Some(plan) = &self.build_writes
        {
            std::fs::create_dir_all(plan.parent().unwrap()).unwrap();
            std::fs::write(plan, "plan").unwrap();
        }
        Ok(String::new())
    }
}

fn team() -> Team {
    Team {
        id: "TEAMSYNTH1".to_string(),
        name: "Synthetic".to_string(),
        kind: "Individual".to_string(),
        free: false,
    }
}

fn plan_in(dir: &Path) -> PathBuf {
    dir.join("build/Build/Products/Runner.xctestrun")
}

/// 해시 불일치: nothing after the check runs, so no patch and no build.
#[test]
fn a_fetch_with_other_hashes_patches_nothing_and_builds_nothing() {
    for script in [
        Script {
            tag: Some("1111111111111111111111111111111111111111".to_string()),
            ..Script::default()
        },
        Script {
            head: Some("2222222222222222222222222222222222222222".to_string()),
            ..Script::default()
        },
        Script {
            tree: Some("3333333333333333333333333333333333333333".to_string()),
            ..Script::default()
        },
    ] {
        let mut script = script;
        let dir = tempfile::tempdir().unwrap();
        let why = supply::prepare(&mut script, dir.path(), &team(), "U", "26.5").unwrap_err();
        assert!(why.starts_with(PHONE_SOURCE_MISMATCH), "{why}");
        let all = script.calls.join("\n");
        assert!(!all.contains(" apply "), "{all}");
        assert!(!all.contains("checkout"), "{all}");
        assert!(!all.contains("xcodebuild"), "{all}");
    }
}

/// 패치 불일치: no build, and the sentence is for the person.
#[test]
fn a_patch_that_does_not_fit_builds_nothing() {
    let mut script = Script {
        patch_fails: true,
        ..Script::default()
    };
    let dir = tempfile::tempdir().unwrap();
    let why = supply::prepare(&mut script, dir.path(), &team(), "U", "26.5").unwrap_err();
    assert!(why.starts_with(PHONE_PATCH_MISMATCH), "{why}");
    assert!(why.contains("Tell the person"));
    assert!(!script.calls.join("\n").contains("xcodebuild"));
}

/// The order, the flags, and no rebuild unless the tag, team or phone's OS changed.
#[test]
fn it_checks_then_patches_then_builds_and_builds_once_per_what_it_was_for() {
    let dir = tempfile::tempdir().unwrap();
    let mut script = Script {
        build_writes: Some(plan_in(dir.path())),
        ..Script::default()
    };
    let plan = supply::prepare(&mut script, dir.path(), &team(), "U", "26.5").expect("built");
    assert_eq!(plan, plan_in(dir.path()));
    let position = |needle: &str| {
        script
            .calls
            .iter()
            .position(|call| call.contains(needle))
            .unwrap_or_else(|| panic!("{needle}"))
    };
    assert!(position("rev-parse FETCH_HEAD^{tree}") < position("apply --check"));
    assert!(position("apply --check") < position("xcodebuild"));
    let build = script.calls.last().unwrap();
    assert!(build.contains("DEVELOPMENT_TEAM=TEAMSYNTH1"), "{build}");
    assert!(build.contains("-allowProvisioningUpdates"), "{build}");
    assert!(script.calls[1].contains("core.hooksPath=/dev/null"));

    let built = script.calls.len();
    supply::prepare(&mut script, dir.path(), &team(), "U", "26.5").expect("kept");
    assert_eq!(script.calls.len(), built, "it built again for nothing");

    supply::prepare(&mut script, dir.path(), &team(), "U", "26.6").expect("rebuilt");
    assert!(script.calls.len() > built, "a new OS builds");

    let again = script.calls.len();
    let other = Team {
        id: "TEAMOTHER2".to_string(),
        ..team()
    };
    supply::prepare(&mut script, dir.path(), &other, "U", "26.6").expect("rebuilt");
    assert!(script.calls.len() > again, "a new team builds");
}

/// A start needs nobody: no signing flag, nothing that can stop to ask a person.
#[test]
fn a_start_after_the_first_build_signs_nothing_and_asks_nothing() {
    let args = supply::run_args(Path::new("/x/Runner.xctestrun"), "U");
    assert_eq!(args[0], "test-without-building");
    let line = args.join(" ");
    for word in [
        "allowProvisioningUpdates",
        "DEVELOPMENT_TEAM",
        "CODE_SIGN",
        "build-for-testing",
    ] {
        assert!(!line.contains(word), "{line}");
    }
}

/// 패치가 그 태그의 소스에 깨끗이 붙는지: with the pinned source's own function, by
/// git itself. Run when `git` and the network are there: `--ignored`.
#[test]
#[ignore = "fetches the pinned source from the network"]
fn the_patch_applies_cleanly_to_the_pinned_source() {
    struct Real;
    impl Tools for Real {
        fn run(&mut self, program: &str, args: &[String], cwd: &Path) -> Result<String, String> {
            let out = crate::proc::quiet_command(program)
                .args(args)
                .current_dir(cwd)
                .output()
                .map_err(|error| error.to_string())?;
            if out.status.success() {
                Ok(String::from_utf8_lossy(&out.stdout).into_owned())
            } else if program == "xcodebuild" {
                Err("xcodebuild reached".to_string())
            } else {
                Err(String::from_utf8_lossy(&out.stderr).into_owned())
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    // Stop at the build: that everything before it passed is the test.
    struct UpToBuild(Real);
    impl Tools for UpToBuild {
        fn run(&mut self, program: &str, args: &[String], cwd: &Path) -> Result<String, String> {
            if program == "xcodebuild" {
                return Err("reached the build".to_string());
            }
            self.0.run(program, args, cwd)
        }
    }
    let result = supply::prepare(&mut UpToBuild(Real), dir.path(), &team(), "U", "26.5");
    assert_eq!(result.unwrap_err(), "reached the build");
}

/// A fake's answers carry no hardware id, and `list`'s row carries a hash.
#[test]
fn nothing_an_agent_reads_carries_the_hardware_id() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    let answer = run(tree(&bench.handle)).expect("a tree");
    assert_eq!(answer["app"]["package"], words::SPRINGBOARD);
    let row = zerocode_core::agent_emulator::physical::parse_devicectl(
        &json!({ "result": { "devices": [{
            "hardwareProperties": { "platform": "iOS", "udid": "00008150-SYNTHETIC" },
            "deviceProperties": { "name": "Synthetic Phone" },
            "connectionProperties": { "tunnelState": "connected" },
        }]}})
        .to_string(),
    )
    .expect("a row");
    let said = serde_json::to_string(&row).unwrap();
    assert!(!said.contains("00008150"), "{said}");
    assert!(said.contains(words::REAL_PHONE_PREFIX), "{said}");
}

// ---- 검수 t-91811: what the first round left open ------------------------------

/// 검수 t-91811 P2-8: a lock answer that is neither true nor false is refused, not
/// read as unlocked, and nothing is pressed or typed.
#[test]
fn a_lock_answer_that_is_not_true_or_false_presses_nothing() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    for word in [json!("yes"), Value::Null, json!({ "locked": false })] {
        *bench.fake.locked_word.lock().unwrap() = Some(word.clone());
        for result in [
            run(tap(&bench.handle, 0.5, 0.5)),
            run(text(&bench.handle, "x")),
            run(tree(&bench.handle)),
        ] {
            assert!(result.is_err(), "{word} was read as unlocked");
        }
    }
    assert_eq!(bench.fake.press_count(), 0);
    assert!(bench.fake.typed.lock().unwrap().is_empty());
}

/// 검수 t-91811 P2-3: the simulator's rule for text: a line break (it would submit
/// a form past every guard), another control character, or more than 4096 bytes is
/// refused before anything reaches the phone.
#[test]
fn a_line_break_a_control_or_a_long_text_is_not_typed() {
    let bench = phone_on_a_fake();
    for refused in [
        "1000000\n".to_string(),
        "a\rb".to_string(),
        "tab\there".to_string(),
        "x".repeat(4097),
    ] {
        let said = run(text(&bench.handle, &refused)).unwrap_err();
        assert_eq!(said.code, error_code::INVALID_ARGUMENT, "{refused:?}");
    }
    assert!(bench.fake.typed.lock().unwrap().is_empty());
    assert_eq!(bench.fake.requests.load(Ordering::SeqCst), 0);
    run(text(&bench.handle, &"가".repeat(1365))).expect("4095 bytes go in");
}

/// 검수 t-91811 P2-2: a press on a phone is sent, not seen. No act answers a proof
/// it does not have: a click names no `confirmedBy`; a press that waited for its
/// screen says its landing by that screen (t-93254), and every other act says it is
/// unconfirmed.
#[test]
fn every_act_on_a_phone_says_it_was_sent_not_seen() {
    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![sized(
        "Button", "Go", 10.0, 10.0, 100.0, 100.0,
    )]));
    let (id, mark) = look(&bench, "Go");
    let clicked = press(&bench, &id, mark).expect("pressed");
    assert!(clicked.get("confirmedBy").is_none(), "{clicked}");
    assert_eq!(
        clicked["landed"],
        words::LANDED_SCREEN_UNCHANGED,
        "{clicked}"
    );
    let tapped = run(tap(&bench.handle, 0.9, 0.9)).expect("tap");
    assert_eq!(tapped["landed"], words::LANDED_SCREEN_UNCHANGED, "{tapped}");
    assert!(tapped.get("confirmedBy").is_none(), "{tapped}");
    for answer in [
        run(swipe(&bench.handle, (0.1, 0.1), (0.2, 0.2), None)).expect("swipe"),
        run(text(&bench.handle, "hi")).expect("text"),
        run(button(&bench.handle, "home")).expect("button"),
    ] {
        assert_eq!(answer["landed"], "unconfirmed", "{answer}");
        assert!(answer.get("confirmedBy").is_none(), "{answer}");
        assert_eq!(answer[ROAD_KEY], ROAD_WDA, "{answer}");
        assert_eq!(answer[MAC_POINTER_KEY], false, "{answer}");
    }
}

/// 검수 t-91811 P1-1: a runner is started only bound to the tunnel's address, which
/// `xcodebuild` hands it as `USE_IP`, so its HTTP and its screen stream listen
/// there alone and not on the phone's Wi-Fi. With no address, nothing starts.
#[test]
fn a_runner_listens_on_the_tunnel_alone_and_none_starts_without_one() {
    let plan = Path::new("/x/Runner.xctestrun");
    let command = runner_command(plan, "UDID-SYNTH", Some("fd00::1")).expect("a command");
    let bound: Vec<(String, String)> = command
        .get_envs()
        .filter_map(|(key, value)| {
            Some((
                key.to_string_lossy().into_owned(),
                value?.to_string_lossy().into_owned(),
            ))
        })
        .collect();
    assert!(
        bound.contains(&(words::RUNNER_BIND_ENV.to_string(), "fd00::1".to_string())),
        "{bound:?}"
    );
    assert_eq!(words::RUNNER_BIND_ENV, "TEST_RUNNER_USE_IP");
    let line: Vec<String> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert!(
        line.iter().any(|arg| arg == "test-without-building"),
        "{line:?}"
    );

    for none in [None, Some("")] {
        let refused = runner_command(plan, "UDID-SYNTH", none).unwrap_err();
        assert_eq!(refused.kind(), runner::NO_TUNNEL);
    }

    // A verb on a phone with no tunnel: no runner, no HTTP, and the answer says why.
    let (fake, base) = fake_runner();
    let handle = format!("phone-test{:04}", NEXT_PHONE.fetch_add(1, Ordering::SeqCst));
    let no_tunnel: runner::Spawn =
        Arc::new(|| runner_command(Path::new("/x/Runner.xctestrun"), "UDID-SYNTH", None)?.spawn());
    let life = Life::new(
        no_tunnel,
        never_unattended(),
        Duration::from_secs(600),
        Vec::new(),
    );
    let road = Arc::new(FakeRoad::new(base));
    register(&handle, road as Arc<dyn Road>, Arc::clone(&life)).expect("registered");
    let refused = run(tap(&handle, 0.5, 0.5)).unwrap_err();
    assert_eq!(refused.code, PHONE_UNREACHABLE);
    assert!(refused.message.contains("no tunnel"), "{}", refused.message);
    assert_eq!(life.starts(), 0);
    assert_eq!(fake.requests.load(Ordering::SeqCst), 0);
}

/// 검수 t-91811 P3-4: a real phone's runner is reached where it said it listens on
/// its own ready line, never at a fixed port where another one could answer; before
/// it has said so, there is no address.
#[test]
fn a_real_phone_is_reached_where_its_own_runner_said() {
    let said = |line: &'static str| -> runner::Spawn {
        Arc::new(move || {
            watched(
                "/bin/sh",
                &["-c".to_string(), format!("echo '{line}'; exec sleep 600")],
            )
            .spawn()
        })
    };
    let life = Life::new(
        said("t ServerURLHere->http://fd00::1:8123<-ServerURLHere"),
        never_unattended(),
        Duration::from_secs(600),
        Vec::new(),
    );
    let road = RealRoad {
        handle: "phone-synthetic".to_string(),
        udid: "UDID-SYNTH".to_string(),
        life: Arc::clone(&life),
    };
    assert_eq!(
        road.base().unwrap_err().split(':').next(),
        Some(PHONE_UNREACHABLE)
    );
    life.ensure_up(Duration::from_secs(10)).expect("up");
    assert_eq!(road.base().as_deref(), Ok("http://[fd00::1]:8123"));
    life.stop_and_wait();
    assert!(road.base().is_err(), "a runner that ended has no address");
}

/// 검수 t-91811 P1-2: the person's answer is about one phone. Another phone of the
/// same team has none, and `phone-install` for it puts the card again; a seal moved
/// to another phone's line in the file turns nothing on; an answer about another
/// team takes the earlier ones back.
#[test]
fn an_answer_for_one_phone_is_not_an_answer_for_another() {
    let seal = Seal::with_store(Arc::new(MemorySecretStore::default()) as Arc<dyn SecretStore>);
    let mut document = crate::settings_runtime::SettingsDocument::default();
    assert!(seal.card_needed(&document, "TEAM", "phone-a"));
    seal.allow(&mut document, "TEAM", "phone-a")
        .expect("allowed");

    assert!(seal.allows(&document, "phone-a"));
    assert!(!seal.allows(&document, "phone-b"));
    assert!(!seal.card_needed(&document, "TEAM", "phone-a"));
    assert!(seal.card_needed(&document, "TEAM", "phone-b"));
    assert!(seal.card_needed(&document, "OTHER", "phone-a"));
    assert_eq!(seal.allowed_phones(&document), 1);

    let moved = document.emulator_real_phone_consent["phone-a"].clone();
    let mut forged = document.clone();
    forged
        .emulator_real_phone_consent
        .insert("phone-b".to_string(), moved);
    assert!(
        !seal.allows(&forged, "phone-b"),
        "a seal moved to another phone"
    );

    seal.allow(&mut document, "TEAM", "phone-b")
        .expect("allowed");
    assert_eq!(seal.allowed_phones(&document), 2);
    document.emulator_real_phone_unattended = seal
        .sealed(Said::Unattended, "TEAM", ALL_PHONES)
        .expect("sealed");
    assert!(seal.unattended(&document));

    seal.allow(&mut document, "OTHER", "phone-b")
        .expect("allowed");
    assert!(
        !seal.allows(&document, "phone-a"),
        "another team's answer stands"
    );
    assert!(seal.allows(&document, "phone-b"));
    assert!(!seal.unattended(&document), "unattended outlived its team");

    withdraw(&mut document);
    assert_eq!(seal.allowed_phones(&document), 0);
}

/// 검수 t-91811 P2-1: the team a build was signed with is read off the build, so a
/// road the person already turned off still takes its app off the phone.
#[test]
fn the_team_to_remove_by_is_read_off_the_build_itself() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(supply::built_team(dir.path()), None);
    let mut script = Script {
        build_writes: Some(plan_in(dir.path())),
        ..Script::default()
    };
    supply::prepare(&mut script, dir.path(), &team(), "U", "26.5").expect("built");
    assert_eq!(
        supply::built_team(dir.path()).as_deref(),
        Some("TEAMSYNTH1")
    );
}

/// 검수 t-91811 P2-4: a second `phone-install` while one builds answers that it is
/// installing, and starts no second build, reads no phone and shows no card.
#[test]
fn a_second_install_during_a_build_starts_nothing() {
    let handle = format!("phone-test{:04}", NEXT_PHONE.fetch_add(1, Ordering::SeqCst));
    assert!(claim_build(&handle));
    assert!(!claim_build(&handle), "two builds in one folder");
    let answer = run(install(&handle, None)).expect("still installing");
    assert_eq!(answer["installing"], true);
    set_setup(&handle, Some(Setup::Failed("synthetic".to_string())));
    assert!(claim_build(&handle), "a failed build may be tried again");
    set_setup(&handle, None);
}

/// 검수 t-91811 P2-6: a payment and a transfer stop on a phone whatever the
/// desktop setting says; a delete follows it.
#[test]
fn a_payment_stops_on_a_phone_whatever_the_desktop_setting() {
    use crate::computer_use::confirm::Policy;
    let nothing_asked = Policy {
        payment: false,
        transfer: false,
        delete: false,
    };
    let phone = nothing_asked.money_always_asks();
    assert!(phone.payment && phone.transfer && !phone.delete);

    let bench = phone_on_a_fake();
    bench.fake.set_tree(screen(vec![
        sized("Button", "결제하기", 10.0, 100.0, 100.0, 50.0),
        sized("Button", "송금 확인", 10.0, 300.0, 100.0, 50.0),
    ]));
    let snapshot = run(marks_snapshot(&bench.handle)).expect("a look");
    for (x, y) in [(60.0, 125.0), (60.0, 325.0)] {
        assert!(
            marks::refuse_guarded_at(&snapshot.faces, snapshot.screen, x, y, nothing_asked).is_ok(),
            "the desktop setting asks nothing"
        );
        assert_eq!(
            marks::refuse_guarded_at(&snapshot.faces, snapshot.screen, x, y, phone)
                .unwrap_err()
                .code,
            error_code::CONFIRMATION_REQUIRED
        );
    }
}
