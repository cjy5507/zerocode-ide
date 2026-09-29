//! ZeroCode's own window under a point a request presses at, read by the
//! window before the helper is asked (t-12979).
//!
//! The helper refuses a press that lands on ZeroCode's own window: it reads
//! whose window is frontmost at each point the request names. Where the
//! window server lists the pointer's own picture (a rotated display, 09-30),
//! the frontmost window at the point the pointer rests on is the window
//! server's, so a press there — a second click, a scroll where the pointer
//! is — was read as landing on the window server and let through. The window
//! reads the same list without the pointer's picture and refuses first; the
//! helper's own check stands behind it, unchanged.

use serde_json::Value;
use zerocode_core::computer_use_protocol::identity::{ALLOW_SELF_KEY, own_window};
use zerocode_core::computer_use_protocol::marks::{DesktopWindow, pointed_points, window_under};

use super::ComputerUseError;

/// The refusal a request gets before the helper is asked, when a point it
/// presses at lands on ZeroCode's own window; `None` leaves it to the
/// helper. `list` reads the window list (every layer), only when the
/// request presses at a point and did not say `allowSelf`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(super) fn own_window_first(
    method: &str,
    params: &Value,
    list: &mut dyn FnMut() -> Option<Vec<DesktopWindow>>,
) -> Option<ComputerUseError> {
    if params.get(ALLOW_SELF_KEY).and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let points = pointed_points(method, params);
    if points.is_empty() {
        return None;
    }
    let windows = list()?;
    points
        .into_iter()
        .find(|&(x, y)| window_under(&windows, x, y).is_some_and(|window| window.own))
        .map(|(x, y)| own_window(x, y))
}

#[cfg(test)]
mod tests;
