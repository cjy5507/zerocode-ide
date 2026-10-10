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
    assert_background_input(EditingRoute::WindowKey);
}

#[test]
#[ignore = "requires an owned BackgroundWebFixture; inaccessible keyboard receivers must not receive input"]
fn background_accessibility_preserves_the_persons_desktop() {
    assert_background_input(EditingRoute::Accessibility);
}

#[derive(Clone, Copy)]
enum EditingRoute {
    WindowKey,
    Accessibility,
}

fn assert_background_input(route: EditingRoute) {
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
    let status = session
        .request("status", json!({}))
        .expect("operator status");
    assert_eq!(status["stopped"], false);
    assert_eq!(status["hotkeyArmed"], true);
    assert_eq!(status["hotkeyHears"], true);
    let started = Instant::now();
    let mut provider_time = Duration::ZERO;
    let mut admission_time = Duration::ZERO;
    for (index, window) in windows.iter().enumerate() {
        require_window_operator();
        let mut guarded = json!({
            "app": format!("pid:{pid}"), "windowId": window["id"], "background": true,
            "confirmGuard": {"payment": ["increment"]}, "noScreenshot": true,
        });
        guarded
            .as_object_mut()
            .expect("guarded params")
            .extend(point(window, "button").as_object().expect("point").clone());
        let refusal = session
            .request("click", guarded)
            .map(drop)
            .expect_err("the target's label needs confirmation even behind another app");
        assert_eq!(refusal.into_error().code, "confirmation_required");
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
            for field in [
                "foreground",
                "cursorX",
                "cursorY",
                "activationCount",
                "clipboardChanges",
            ] {
                assert_eq!(
                    current[field], initial[field],
                    "{field} changed during {method}"
                );
            }
            assert_eq!(current["active"], false);
            result
        };
        let look = perform("getAppState", json!({}));
        assert_eq!(look["snapshot"]["app"]["bundleId"], bundle);
        assert_eq!(look["snapshot"]["window"]["id"], window["id"]);
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
    for field in [
        "foreground",
        "cursorX",
        "cursorY",
        "activationCount",
        "clipboardChanges",
    ] {
        assert_eq!(final_state[field], initial[field], "{field} changed");
    }
    assert_eq!(final_state["active"], false);
    eprintln!(
        "background fixture: 2 windows, {:.3} ms total, {:.3} ms provider, {:.3} ms independent operator admission, all oracle values correct",
        started.elapsed().as_secs_f64() * 1000.0,
        provider_time.as_secs_f64() * 1000.0,
        admission_time.as_secs_f64() * 1000.0
    );
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
