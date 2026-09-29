use std::cell::Cell;

use serde_json::json;
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::render::Rect;

use super::*;

fn window(id: u64, pid: i64, app: &str, layer: i64, rect: (f64, f64, f64, f64)) -> DesktopWindow {
    DesktopWindow {
        id,
        pid,
        app: app.to_string(),
        rect: Rect::new(rect.0, rect.1, rect.2, rect.3),
        own: false,
        layer,
        alpha: 1.0,
        overlay: false,
    }
}

/// ZeroCode's window over most of the screen.
fn zerocode() -> DesktopWindow {
    let mut own = window(1, 500, "ZeroCode", 0, (0.0, 0.0, 1_200.0, 800.0));
    own.own = true;
    own
}

/// The pointer's own picture as read off the window list on 09-30, resting
/// at (600, 400).
fn pointer() -> DesktopWindow {
    window(
        2,
        399,
        "Window Server",
        2_147_483_630,
        (590.0, 390.0, 23.0, 22.0),
    )
}

/// Ask with `windows` as the list, counting how often it was read.
fn asked(
    method: &str,
    params: &Value,
    windows: Vec<DesktopWindow>,
) -> (Option<ComputerUseError>, u32) {
    let reads = Cell::new(0);
    let mut list = || {
        reads.set(reads.get() + 1);
        Some(windows.clone())
    };
    (own_window_first(method, params, &mut list), reads.get())
}

/// A click or a scroll where the pointer rests over ZeroCode's own window:
/// the frontmost window there is the pointer's picture, and under it is
/// ZeroCode's — refused before the helper is asked, in the helper's words.
#[test]
fn a_press_where_the_pointer_rests_on_zerocodes_window_is_refused_first() {
    for method in ["mouseClick", "mouseScroll"] {
        let (refused, reads) = asked(
            method,
            &json!({ "x": 600, "y": 400 }),
            vec![pointer(), zerocode()],
        );
        let refused = refused.unwrap_or_else(|| panic!("{method} went to the helper"));
        assert_eq!(refused.code, error_code::APP_BLOCKED, "{method}");
        assert_eq!(
            refused.message,
            "(600, 400) lands on ZeroCode's own window; the operator does not drive the app it lives in"
        );
        assert_eq!(reads, 1);
    }
}

/// A drag is read at both ends: one that ends where the pointer rests on
/// ZeroCode's window is refused, from wherever it starts.
#[test]
fn a_drag_ending_where_the_pointer_rests_on_zerocodes_window_is_refused_first() {
    let other = window(3, 20, "Other", 0, (1_300.0, 0.0, 400.0, 400.0));
    let params = json!({ "fromX": 1_400, "fromY": 100, "toX": 600, "toY": 400 });
    let (refused, _) = asked("mouseDrag", &params, vec![pointer(), other, zerocode()]);
    assert_eq!(
        refused.map(|refused| refused.code),
        Some(error_code::APP_BLOCKED.to_string())
    );
}

/// What stays the helper's: a request that said `allowSelf` and one that
/// presses at no point are not even read; another app's window under the
/// pointer and another app's window in front of ZeroCode's at the point go
/// to the helper as before.
#[test]
fn what_is_not_zerocodes_window_under_the_point_is_left_to_the_helper() {
    let at = json!({ "x": 600, "y": 400 });
    let (refused, reads) = asked(
        "mouseClick",
        &json!({ "x": 600, "y": 400, "allowSelf": true }),
        vec![pointer(), zerocode()],
    );
    assert!(refused.is_none() && reads == 0, "allowSelf");
    let (refused, reads) = asked(
        "key",
        &json!({ "key": "return" }),
        vec![pointer(), zerocode()],
    );
    assert!(refused.is_none() && reads == 0, "a key presses at no point");
    let other = window(3, 20, "Other", 0, (0.0, 0.0, 1_200.0, 800.0));
    let (refused, _) = asked("mouseClick", &at, vec![pointer(), other.clone()]);
    assert!(refused.is_none(), "another app's window under the pointer");
    let (refused, _) = asked("mouseClick", &at, vec![pointer(), other, zerocode()]);
    assert!(
        refused.is_none(),
        "another app's window in front of ZeroCode's"
    );
}

/// When whose window a point lands on cannot be read — the list does not
/// come, or comes empty, as the window server's list does when it cannot be
/// had — the press is refused by the window, closed: it may land on
/// ZeroCode's own window, and the helper's own check reads the same list.
/// A request that presses at no point, or said `allowSelf`, is not read.
#[test]
fn a_press_whose_point_cannot_be_read_is_refused_closed() {
    let at = json!({ "x": 600, "y": 400 });
    let mut unreadable = || None;
    let empty = asked("mouseClick", &at, Vec::new()).0;
    for (refused, why) in [
        (
            own_window_first("mouseClick", &at, &mut unreadable),
            "no list",
        ),
        (empty, "an empty list"),
    ] {
        let refused = refused.unwrap_or_else(|| panic!("{why}: went to the helper"));
        assert_eq!(refused.code, error_code::WINDOW_NOT_FOUND, "{why}");
        assert!(
            refused.message.contains("(600, 400)"),
            "{why}: {}",
            refused.message
        );
    }
    let (refused, reads) = asked(
        "mouseClick",
        &json!({ "x": 600, "y": 400, "allowSelf": true }),
        Vec::new(),
    );
    assert!(refused.is_none() && reads == 0, "allowSelf");
    let (refused, reads) = asked("key", &json!({ "key": "return" }), Vec::new());
    assert!(refused.is_none() && reads == 0, "a key");
}

/// The verbs read for their points are the helper's own (`pointedMethods`
/// in its dispatch), entry for entry: a verb the helper starts reading a
/// point of, or a point it renames, fails here before the window's check and
/// the helper's part ways.
#[test]
fn the_verbs_pressed_at_points_are_the_helpers_own() {
    let main = include_str!(
        "../../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/main.swift"
    );
    let table = &main[main
        .find("static let pointedMethods")
        .expect("the helper's table")..];
    let table = &table[..table.find("\n    ]").expect("its end")];
    let helpers: Vec<&str> = table
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('"'))
        .map(|line| line.trim_end_matches(','))
        .collect();
    let ours: Vec<String> = zerocode_core::computer_use_protocol::marks::POINTED_METHODS
        .iter()
        .map(|(method, points)| {
            let points: Vec<String> = points
                .iter()
                .map(|(x, y)| format!("(\"{x}\", \"{y}\")"))
                .collect();
            format!("\"{method}\": [{}]", points.join(", "))
        })
        .collect();
    assert_eq!(helpers, ours);
}
