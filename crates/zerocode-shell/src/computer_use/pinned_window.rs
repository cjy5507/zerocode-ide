//! A press at a point pinned to the window its look saw (CU speed 163, A1).
//!
//! zo reads the points of a batch on one picture of one app window and pins
//! each point step to the frame that window had in that picture
//! (`--window-frame`). A point in an app is counted inside its window, so a
//! window that moved, grew or shrank — or another window under the same id —
//! puts the point on something the picture never showed. The window reads
//! the app's window list before it asks the helper, and takes a pinned press
//! only while the window still stands where, and as large as, the picture
//! saw it: else the press is refused with nothing sent, and a batch stops at
//! that step. The helper never sees the pin, and is not changed.

use serde_json::Value;
use zerocode_core::computer_use::{WINDOW_FRAME_PARAM, WINDOW_FRAME_TOLERANCE_POINTS};
use zerocode_core::computer_use_protocol::render::Rect;
use zerocode_core::computer_use_protocol::{NOT_DELIVERED, error_code};

use super::ComputerUseError;

/// The helper's methods a pin rides on: an app's presses at a point.
const PINNED_METHODS: [&str; 3] = ["click", "scroll", "drag"];

/// `params` without its pin, once the pinned window stands where its look
/// saw it; a refusal with nothing sent when it moved, resized or went, or
/// when the list that would say so did not come. `list` reads the app's
/// windows (`listWindows`), only for a request that carries a pin: one
/// without passes as it came and costs nothing.
pub(super) fn stands_first(
    method: &str,
    mut params: Value,
    list: impl FnOnce(&str) -> Result<Value, ComputerUseError>,
) -> Result<Value, ComputerUseError> {
    let Some(pin) = params
        .as_object_mut()
        .and_then(|params| params.remove(WINDOW_FRAME_PARAM))
    else {
        return Ok(params);
    };
    let app = params
        .get("app")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let window_id = params.get("windowId").and_then(Value::as_u64);
    let (Some(pin), Some(window_id)) = (rect(&pin), window_id) else {
        return Err(misplaced());
    };
    if app.trim().is_empty() || !PINNED_METHODS.contains(&method) {
        return Err(misplaced());
    }
    let listed = list(app).map_err(|error| {
        stale(&format!(
            "its window could not be read to check it still stands where your look saw it ({}: {})",
            error.code, error.message
        ))
    })?;
    match standing(&listed, window_id) {
        Some(now) if now.matches_within(&pin, WINDOW_FRAME_TOLERANCE_POINTS) => Ok(params),
        Some(now) => Err(stale(&format!(
            "window {window_id} moved or resized since the look its point was read on (then {}, now {})",
            spelled(&pin),
            spelled(&now)
        ))),
        None => Err(stale(&format!(
            "window {window_id} is no longer among the app's windows"
        ))),
    }
}

/// A pin on something it cannot hold: nothing was sent.
fn misplaced() -> ComputerUseError {
    ComputerUseError::invalid_argument(format!(
        "a window frame pins an app's press at a point ({}) to one window: it needs the app, the window id and x,y,width,height",
        PINNED_METHODS.join(", ")
    ))
    .with_delivery(Some(NOT_DELIVERED))
}

/// The refusal of a pinned press whose window does not stand as its look
/// saw it: nothing was sent, and the point needs a new look.
fn stale(why: &str) -> ComputerUseError {
    ComputerUseError::new(
        error_code::WINDOW_STALE,
        format!("{why}; nothing was pressed — look at the window again and read the point on the new picture"),
    )
    .with_delivery(Some(NOT_DELIVERED))
}

/// The frame window `window_id` has in an app's window list.
fn standing(listed: &Value, window_id: u64) -> Option<Rect> {
    listed
        .get("windows")?
        .as_array()?
        .iter()
        .find(|row| row.get("id").and_then(Value::as_u64) == Some(window_id))
        .and_then(rect)
}

fn rect(value: &Value) -> Option<Rect> {
    let side = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_f64)
            .filter(|side| side.is_finite())
    };
    let rect = Rect::new(side("x")?, side("y")?, side("width")?, side("height")?);
    (rect.width > 0.0 && rect.height > 0.0).then_some(rect)
}

fn spelled(rect: &Rect) -> String {
    format!("{},{},{},{}", rect.x, rect.y, rect.width, rect.height)
}

#[cfg(test)]
mod tests;
