use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::Session;
use crate::computer_use::session::ProviderSession as _;

fn read_state(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("owned fixture state"))
        .expect("fixture JSON")
}

fn point(window: &Value, control: &str) -> Value {
    json!({
        "x": window[format!("{control}X")].as_f64().expect("control x")
            - window["x"].as_f64().expect("window x"),
        "y": window[format!("{control}Y")].as_f64().expect("control y")
            - window["y"].as_f64().expect("window y"),
    })
}

fn require_window_operator() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let folder = std::env::var_os("ZEROCODE_BACKGROUND_OPERATOR_GATE")
        .expect("the authorized runner's operator gate");
    let folder = Path::new(&folder);
    let request = NEXT.fetch_add(1, Ordering::Relaxed);
    std::fs::write(
        folder.join("request.tmp"),
        json!({"id": request}).to_string(),
    )
    .expect("operator request");
    std::fs::rename(folder.join("request.tmp"), folder.join("request.json"))
        .expect("publish operator request");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(bytes) = std::fs::read(folder.join("answer.json"))
            && let Ok(answer) = serde_json::from_slice::<Value>(&bytes)
            && answer["id"] == request
        {
            assert_eq!(
                answer["ready"], true,
                "the window operator is no longer ready"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "operator admission timed out without input"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
#[ignore = "requires an owned BackgroundFixture and the staged signed helper; tools/computer-bench/background_input.py"]
fn window_scoped_input_preserves_the_persons_desktop() {
    assert_background_input(EditingRoute::WindowKey, PointerOracle::Stationary);
}

#[test]
#[ignore = "requires an owned BackgroundWebFixture; inaccessible keyboard receivers must not receive input"]
fn background_accessibility_preserves_the_persons_desktop() {
    assert_background_input(EditingRoute::Accessibility, PointerOracle::Stationary);
}

#[test]
#[ignore = "requires an owned BackgroundFixture and concurrent external pointer activity"]
fn window_scoped_input_while_the_person_moves_the_pointer() {
    assert_background_input(EditingRoute::WindowKey, PointerOracle::Concurrent);
}

#[test]
#[ignore = "requires an owned BackgroundWebFixture and concurrent external pointer activity"]
fn background_accessibility_while_the_person_moves_the_pointer() {
    assert_background_input(EditingRoute::Accessibility, PointerOracle::Concurrent);
}

#[test]
#[ignore = "requires visible owned AppKit windows; does not require a stationary or moving user pointer"]
fn window_pointer_is_visible_without_taking_focus() {
    assert_eq!(
        std::env::var("ZEROCODE_BACKGROUND_POINTER_VISIBLE").as_deref(),
        Ok("1")
    );
    assert_background_input(EditingRoute::WindowKey, PointerOracle::Independent);
}

#[test]
#[ignore = "requires visible owned WebKit windows; does not require a stationary or moving user pointer"]
fn accessibility_pointer_is_visible_without_taking_focus() {
    assert_eq!(
        std::env::var("ZEROCODE_BACKGROUND_POINTER_VISIBLE").as_deref(),
        Ok("1")
    );
    assert_background_input(EditingRoute::Accessibility, PointerOracle::Independent);
}

#[derive(Clone, Copy)]
enum EditingRoute {
    WindowKey,
    Accessibility,
}

#[derive(Clone, Copy)]
enum PointerOracle {
    Stationary,
    Concurrent,
    Independent,
}

fn shared_pointer_events(state: &Value, helper_pid: i32) -> u64 {
    let monitor = &state["sharedPointer"];
    assert_eq!(
        monitor["ready"], true,
        "the passive pointer observer must be ready"
    );
    assert_eq!(
        monitor["overflow"], false,
        "the event evidence must be complete"
    );
    monitor["sources"]
        .as_object()
        .expect("pointer event provenance")
        .get(&helper_pid.to_string())
        .map_or(0, |count| count.as_u64().expect("pointer event count"))
}

fn assert_background_state(
    initial: &Value,
    current: &Value,
    oracle: PointerOracle,
    helper_pid: i32,
) {
    for field in ["foreground", "activationCount", "clipboardChanges"] {
        assert_eq!(current[field], initial[field], "{field} changed");
    }
    match oracle {
        PointerOracle::Stationary => {
            for field in ["cursorX", "cursorY"] {
                assert_eq!(current[field], initial[field], "{field} changed");
            }
        }
        PointerOracle::Concurrent | PointerOracle::Independent => {
            assert_eq!(
                shared_pointer_events(current, helper_pid),
                0,
                "the helper posted shared pointer input"
            );
        }
    }
    assert_eq!(current["active"], false);
}

fn assert_visible_pointer(session: &mut Session, window: &Value, path: &Path, index: usize) {
    use base64::Engine as _;
    let deadline = Instant::now() + Duration::from_millis(500);
    let expected = point(window, "scroll");
    let receipt = loop {
        let status = session
            .request("status", json!({}))
            .expect("pointer status");
        let displayed = &status["backgroundPointer"]["displayed"];
        if displayed["windowId"] == window["id"]
            && displayed["x"].as_f64() == expected["x"].as_f64()
            && displayed["y"].as_f64() == expected["y"].as_f64()
        {
            break status["backgroundPointer"].clone();
        }
        assert!(
            Instant::now() < deadline,
            "virtual pointer was not displayed: {status}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(receipt["displayed"]["x"].as_f64(), expected["x"].as_f64());
    assert_eq!(receipt["displayed"]["y"].as_f64(), expected["y"].as_f64());
    let frame = session
        .request(
            "screenshotDesktop",
            json!({"region": {
                "x": window["scrollX"].as_f64().expect("point x") - 10.0,
                "y": window["scrollY"].as_f64().expect("point y") - 10.0,
                "width": 60, "height": 60,
            }}),
        )
        .expect("visible pointer capture");
    let pixels = base64::engine::general_purpose::STANDARD
        .decode(frame["screenshot"]["data"].as_str().expect("PNG bytes"))
        .expect("PNG data");
    std::fs::write(path.with_file_name(format!("pointer-{index}.png")), pixels)
        .expect("save pointer pixels");
    std::fs::write(
        path.with_file_name(format!("pointer-{index}.json")),
        receipt.to_string(),
    )
    .expect("save pointer receipt");
}

fn assert_background_input(route: EditingRoute, oracle: PointerOracle) {
    let path = std::env::var_os("ZEROCODE_BACKGROUND_FIXTURE_STATE")
        .expect("an explicitly launched, owned fixture");
    let path = Path::new(&path);
    let initial = read_state(path);
    let pid = initial["pid"].as_u64().expect("fixture pid");
    let bundle = initial["bundleId"].as_str().expect("fixture bundle");
    assert!(bundle.starts_with("dev.zerocode.bench.background-input."));
    assert_eq!(initial["active"], false);
    let windows = initial["windows"].as_array().expect("fixture windows");
    assert_eq!(windows.len(), 2);
    let mut session = Session::start().expect("staged signed helper handshake");
    let helper_pid = session.helper_pid().expect("helper pid");
    let status = session
        .request("status", json!({}))
        .expect("operator status");
    assert_eq!(status["stopped"], false);
    assert_eq!(status["hotkeyArmed"], true);
    assert_eq!(status["hotkeyHears"], true);
    let motion_baseline = read_state(path);
    let started = Instant::now();
    let mut provider_time = Duration::ZERO;
    let mut admission_time = Duration::ZERO;
    let visible_pointer =
        std::env::var("ZEROCODE_BACKGROUND_POINTER_VISIBLE").is_ok_and(|value| value == "1");
    for (index, window) in windows.iter().enumerate() {
        require_window_operator();
        let observation_started = Instant::now();
        let look = session
            .request(
                "getAppState",
                json!({
                    "app": format!("pid:{pid}"), "windowId": window["id"],
                    "background": true, "noScreenshot": true,
                }),
            )
            .expect("background window observation before input");
        provider_time += observation_started.elapsed();
        std::fs::write(
            path.with_file_name(format!("observation-{index}.json")),
            serde_json::to_vec(&look).expect("owned observation JSON"),
        )
        .expect("record the observation the input uses");
        assert_eq!(look["snapshot"]["app"]["bundleId"], bundle);
        assert_eq!(look["snapshot"]["window"]["id"], window["id"]);
        let mut guarded = json!({
            "app": format!("pid:{pid}"), "windowId": window["id"], "background": true,
            "confirmGuard": {"payment": ["increment"]}, "noScreenshot": true,
        });
        guarded
            .as_object_mut()
            .expect("guarded params")
            .extend(point(window, "button").as_object().expect("point").clone());
        let refusal = session
            .request("click", guarded.clone())
            .map(drop)
            .expect_err("the target's label needs confirmation even behind another app");
        let refusal_code = refusal.into_error().code;
        if refusal_code == "element_not_found" {
            let current = read_state(path);
            assert_eq!(current["windows"][index]["clicks"], window["clicks"]);
            assert_eq!(current["windows"][index]["text"], window["text"]);
            session
                .request(
                    "waitFor",
                    json!({
                        "app": format!("pid:{pid}"), "windowId": window["id"], "background": true,
                        "label": "Increment", "timeoutMs": 5000, "noScreenshot": true,
                    }),
                )
                .expect("wait for the owned control to become readable without input");
            require_window_operator();
            let refusal = session
                .request("click", guarded)
                .map(drop)
                .expect_err("a readable guarded target still requires confirmation");
            assert_eq!(refusal.into_error().code, "confirmation_required");
        } else {
            assert_eq!(refusal_code, "confirmation_required");
        }
        let mut perform = |method: &str, extra: Value| {
            let admission_started = Instant::now();
            require_window_operator();
            admission_time += admission_started.elapsed();
            let mut params = json!({
                "app": format!("pid:{pid}"), "background": true, "noScreenshot": true,
                "windowId": window["id"],
            });
            params
                .as_object_mut()
                .expect("params")
                .extend(extra.as_object().expect("extra params").clone());
            let provider_started = Instant::now();
            let result = session.request(method, params).expect(method);
            provider_time += provider_started.elapsed();
            let current = read_state(path);
            assert_background_state(&initial, &current, oracle, helper_pid);
            result
        };
        perform("click", point(window, "button"));
        perform("click", point(window, "input"));
        perform("pressKey", json!({"key": "cmd+a"}));
        perform(
            "typeText",
            json!({"text": format!("window {index} 한글🙂")}),
        );
        perform("pressKey", json!({"key": "cmd+a"}));
        perform(
            "pasteText",
            json!({"text": format!("replaced {index} 한글🙂")}),
        );
        match route {
            EditingRoute::WindowKey => {
                perform("pressKey", json!({"key": "left"}));
                perform("typeText", json!({"text": "!"}));
            }
            EditingRoute::Accessibility => {
                let found = perform("findElements", json!({"label": "Owned test field"}));
                let matches = found["matches"].as_array().expect("observed fields");
                assert_eq!(matches.len(), 1);
                perform(
                    "setValue",
                    json!({
                        "elementIndex": matches[0]["index"], "plainInputOnly": true,
                        "expectedPlainValue": format!("replaced {index} 한글🙂"),
                        "value": format!("replaced {index} 한글!🙂"),
                    }),
                );
            }
        }
        let mut scroll = point(window, "scroll");
        scroll["direction"] = json!("down");
        scroll["pages"] = json!(0.5);
        perform("scroll", scroll);
        if matches!(route, EditingRoute::Accessibility) {
            let look = perform("getAppState", json!({}));
            if look["snapshot"]["focusedElementId"].is_null() {
                require_window_operator();
                let refusal = session
                    .request(
                        "pressKey",
                        json!({
                            "app": format!("pid:{pid}"), "windowId": window["id"],
                            "background": true, "key": "left",
                        }),
                    )
                    .map(drop)
                    .expect_err("an unverified keyboard receiver must be refused");
                assert_eq!(refusal.into_error().code, "focus_changed");
            }
        } else {
            require_window_operator();
            let refusal = session
                .request(
                    "pressKey",
                    json!({
                        "app": format!("pid:{pid}"), "windowId": window["id"], "background": true,
                        "key": "return", "confirmGuard": {"payment": ["increment"]}, "noScreenshot": true,
                    }),
                )
                .map(drop).expect_err("the target window's default button needs confirmation");
            assert_eq!(refusal.into_error().code, "confirmation_required");
        }
        if visible_pointer {
            assert_visible_pointer(&mut session, window, path, index);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let final_state = loop {
        let current = read_state(path);
        let complete = current["windows"]
            .as_array()
            .expect("windows")
            .iter()
            .enumerate()
            .all(|(index, row)| row["text"] == format!("replaced {index} 한글!🙂"));
        if complete || Instant::now() >= deadline {
            break current;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let final_state = if matches!(oracle, PointerOracle::Concurrent) {
        std::thread::sleep(Duration::from_millis(100));
        read_state(path)
    } else {
        final_state
    };
    for (index, row) in final_state["windows"]
        .as_array()
        .expect("windows")
        .iter()
        .enumerate()
    {
        assert_eq!(row["text"], format!("replaced {index} 한글!🙂"));
        assert!(
            row["scrollOffset"].as_f64().expect("scroll offset")
                > windows[index]["scrollOffset"]
                    .as_f64()
                    .expect("initial scroll offset")
        );
        assert_eq!(
            row["clicks"].as_u64(),
            Some(windows[index]["clicks"].as_u64().expect("click count") + 1)
        );
    }
    assert_background_state(&initial, &final_state, oracle, helper_pid);
    if matches!(oracle, PointerOracle::Concurrent) {
        let events = final_state["events"]
            .as_array()
            .expect("window event receipts");
        if matches!(route, EditingRoute::WindowKey) {
            assert!(
                !events.is_empty(),
                "the native window route must have receiving-side evidence"
            );
        }
        for event in events {
            assert_eq!(
                event["sourcePid"], helper_pid,
                "window event source identity"
            );
            assert_eq!(event["targetPid"], pid, "window event target identity");
        }
        let before = motion_baseline["sharedPointer"]["mouseMoves"]
            .as_u64()
            .expect("initial mouse activity");
        let after = final_state["sharedPointer"]["mouseMoves"]
            .as_u64()
            .expect("final mouse activity");
        assert!(
            after > before,
            "this concurrent trial requires external pointer movement; a quiet cursor is not proof"
        );
        let cursor_before = motion_baseline["sharedPointer"]["cursorChanges"]
            .as_u64()
            .expect("initial physical cursor changes");
        let cursor_after = final_state["sharedPointer"]["cursorChanges"]
            .as_u64()
            .expect("final physical cursor changes");
        assert!(
            cursor_after > cursor_before,
            "pointer events alone do not prove that the external cursor moved"
        );
        let pointer_status = session
            .request("status", json!({}))
            .expect("virtual pointer receipt");
        let latest = &pointer_status["backgroundPointer"]["latest"];
        let last_window = windows.last().expect("target window");
        let last_point = point(last_window, "scroll");
        assert_eq!(latest["windowId"], last_window["id"]);
        assert_eq!(latest["x"].as_f64(), last_point["x"].as_f64());
        assert_eq!(latest["y"].as_f64(), last_point["y"].as_f64());
        eprintln!(
            "background_shared_pointer {}",
            json!({
                "externalMouseMoves": after - before,
                "externalCursorChanges": cursor_after - cursor_before,
                "helperSharedPointerEvents": shared_pointer_events(&final_state, helper_pid),
                "virtualPointer": pointer_status["backgroundPointer"],
                "cursorChanged": final_state["cursorX"] != initial["cursorX"] || final_state["cursorY"] != initial["cursorY"],
                "scope": "window input with a moving external pointer; no autonomous-task speed comparison",
            })
        );
    }
    eprintln!(
        "background fixture: 2 windows, {:.3} ms total, {:.3} ms provider, {:.3} ms independent operator admission, all oracle values correct",
        started.elapsed().as_secs_f64() * 1000.0,
        provider_time.as_secs_f64() * 1000.0,
        admission_time.as_secs_f64() * 1000.0
    );
    if visible_pointer {
        std::thread::sleep(Duration::from_millis(1100));
        let status = session
            .request("status", json!({}))
            .expect("pointer expiry status");
        assert!(status["backgroundPointer"]["displayed"].is_null());
        assert_background_state(&initial, &read_state(path), oracle, helper_pid);
    }
}

#[test]
fn pointer_provenance_does_not_count_external_input_as_helper_input() {
    let state = json!({"sharedPointer": {
        "ready": true, "overflow": false, "sources": {"0": 200, "400001": 3},
    }});
    assert_eq!(shared_pointer_events(&state, 400002), 0);
    assert_eq!(shared_pointer_events(&state, 400001), 3);
}

#[test]
#[should_panic(expected = "the event evidence must be complete")]
fn a_pointer_observer_that_dropped_evidence_cannot_prove_isolation() {
    shared_pointer_events(
        &json!({"sharedPointer": {
            "ready": true, "overflow": true, "sources": {},
        }}),
        400001,
    );
}

#[test]
fn the_concurrent_oracle_accepts_external_cursor_motion() {
    let initial = json!({
        "foreground": 1, "activationCount": 0, "clipboardChanges": 2,
        "active": false, "cursorX": 10, "cursorY": 20,
        "sharedPointer": {"ready": true, "overflow": false, "sources": {}},
    });
    let mut current = initial.clone();
    current["cursorX"] = json!(300);
    current["sharedPointer"]["sources"]["0"] = json!(100);
    assert_background_state(&initial, &current, PointerOracle::Concurrent, 400001);
}

#[test]
#[should_panic(expected = "the helper posted shared pointer input")]
fn the_concurrent_oracle_rejects_a_helper_event_on_the_shared_pointer() {
    let state = json!({
        "active": false,
        "sharedPointer": {"ready": true, "overflow": false, "sources": {"400001": 1}},
    });
    assert_background_state(&state, &state, PointerOracle::Concurrent, 400001);
}

fn resident_kib(pid: i32) -> u64 {
    let output = crate::proc::quiet_command("/bin/ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .expect("helper memory probe");
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .expect("RSS text")
        .trim()
        .parse()
        .expect("RSS KiB")
}

#[test]
#[ignore = "read-only release-helper measurement against an owned BackgroundFixture"]
fn background_observation_memory_and_latency() {
    let path = std::env::var_os("ZEROCODE_BACKGROUND_FIXTURE_STATE").expect("owned fixture");
    let initial = read_state(Path::new(&path));
    assert!(
        initial["bundleId"]
            .as_str()
            .expect("bundle")
            .starts_with("dev.zerocode.bench.background-input.")
    );
    let params = json!({
        "app": format!("pid:{}", initial["pid"].as_u64().expect("pid")),
        "windowId": initial["windows"][0]["id"], "noScreenshot": true, "background": true,
    });
    let mut session = Session::start().expect("signed helper");
    let pid = session.helper_pid().expect("helper pid");
    for _ in 0..200 {
        session
            .request("getAppState", params.clone())
            .expect("warm observation");
    }
    let initial_memory = resident_kib(pid);
    let mut elapsed = Vec::with_capacity(1000);
    for _ in 0..1000 {
        let started = Instant::now();
        let result = session
            .request("getAppState", params.clone())
            .expect("observation");
        elapsed.push(started.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(
            result["snapshot"]["window"]["id"],
            initial["windows"][0]["id"]
        );
        assert!(result["screenshot"].is_null());
    }
    let final_memory = resident_kib(pid);
    elapsed.sort_by(f64::total_cmp);
    eprintln!(
        "background_observation {}",
        json!({
        "samples": elapsed.len(), "p50_ms": elapsed[(elapsed.len() * 50).div_ceil(100) - 1],
        "p95_ms": elapsed[(elapsed.len() * 95).div_ceil(100) - 1],
            "initial_rss_kib": initial_memory, "final_rss_kib": final_memory,
            "scope": "helper observation only; no model or completed-task comparison",
        })
    );
}
