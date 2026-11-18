use serde_json::{Value, json};
use zerocode_core::computer_use::WINDOW_FRAME_PARAM;
use zerocode_core::computer_use_protocol::{NOT_DELIVERED, error_code};

use super::*;

/// The look saw window 41 at (100, 80), 640 × 480.
const LOOK: [f64; 4] = [100.0, 80.0, 640.0, 480.0];

fn frame([x, y, width, height]: [f64; 4]) -> Value {
    json!({ "x": x, "y": y, "width": width, "height": height })
}

fn pinned(method_params: Value) -> Value {
    let mut params = method_params;
    params[WINDOW_FRAME_PARAM] = frame(LOOK);
    params
}

fn point() -> Value {
    pinned(json!({ "app": "Fixture", "windowId": 41, "x": 10.0, "y": 20.0 }))
}

/// The app's windows as the helper lists them: another window first, then
/// window 41 where `now` puts it.
fn listing(now: [f64; 4]) -> Value {
    let [x, y, width, height] = now;
    json!({ "windows": [
        { "id": 52, "x": 0, "y": 0, "width": 300, "height": 200 },
        { "id": 41, "x": x, "y": y, "width": width, "height": height },
    ] })
}

#[test]
fn a_request_without_a_pin_passes_as_it_came_and_reads_no_list() {
    let asked = json!({ "app": "Fixture", "windowId": 41, "x": 10.0, "y": 20.0 });
    let passed = stands_first("click", asked.clone(), |_| {
        panic!("no list for a request with no pin")
    });
    assert_eq!(passed.unwrap(), asked);
}

#[test]
fn a_window_that_stood_where_its_look_saw_it_takes_the_press_and_the_helper_never_sees_the_pin() {
    for now in [LOOK, [101.0, 79.0, 641.0, 480.0]] {
        let mut read = None;
        let passed = stands_first("click", point(), |app| {
            read = Some(app.to_string());
            Ok(listing(now))
        })
        .expect("the window stood within a point of its look");
        assert_eq!(
            read.as_deref(),
            Some("Fixture"),
            "the app's own windows are read"
        );
        assert!(
            passed.get(WINDOW_FRAME_PARAM).is_none(),
            "the pin is the window's: {passed}"
        );
        assert_eq!(passed["x"], 10.0);
        assert_eq!(passed["windowId"], 41);
    }
}

#[test]
fn a_window_that_moved_resized_or_went_refuses_the_press_with_nothing_sent() {
    let moved = listing([160.0, 80.0, 640.0, 480.0]);
    let resized = listing([100.0, 80.0, 800.0, 480.0]);
    let gone = json!({ "windows": [{ "id": 52, "x": 100, "y": 80, "width": 640, "height": 480 }] });
    for (listed, says) in [
        (Ok(moved), "moved or resized"),
        (Ok(resized), "moved or resized"),
        (Ok(gone), "no longer among"),
        (Ok(json!({})), "no longer among"),
        (
            Err(ComputerUseError::new(
                error_code::APP_NOT_FOUND,
                "Fixture is not running",
            )),
            "could not be read",
        ),
    ] {
        let refused = stands_first("drag", point(), |_| listed).expect_err("refused");
        assert_eq!(refused.code, error_code::WINDOW_STALE, "{refused}");
        assert_eq!(
            refused.delivery.as_deref(),
            Some(NOT_DELIVERED),
            "nothing was sent: {refused}"
        );
        assert!(refused.message.contains(says), "{says}: {refused}");
        assert!(
            refused.message.contains("look at the window again"),
            "{refused}"
        );
    }
}

#[test]
fn a_pin_holds_only_an_apps_press_at_a_point_in_one_window() {
    let no_window = pinned(json!({ "app": "Fixture", "x": 10.0, "y": 20.0 }));
    let no_app = pinned(json!({ "windowId": 41, "x": 10.0, "y": 20.0 }));
    let mut flat = point();
    flat[WINDOW_FRAME_PARAM]["width"] = json!(0);
    for (method, params) in [
        ("typeText", point()),
        ("click", no_window),
        ("click", no_app),
        ("click", flat),
    ] {
        let refused = stands_first(method, params.clone(), |_| {
            panic!("no list for a pin that holds nothing")
        })
        .expect_err("refused");
        assert_eq!(
            refused.code,
            error_code::INVALID_ARGUMENT,
            "{method} {params}: {refused}"
        );
        assert_eq!(refused.delivery.as_deref(), Some(NOT_DELIVERED));
    }
}
