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
/// pointer, another app's window in front of ZeroCode's at the point, and a
/// list that cannot be read go to the helper as before.
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
    let mut unreadable = || None;
    assert!(
        own_window_first("mouseClick", &at, &mut unreadable).is_none(),
        "no list"
    );
}
