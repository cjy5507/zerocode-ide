//! The pointer's own picture on a window list (t-12979).
//!
//! On some displays the window server draws the cursor as a window of its
//! own and lists it with every other: measured 09-30 with a 1080×1920
//! display beside a 1920×1080 one, a 23×22 window of the window server's at
//! the cursor's level stood first on the list, under the pointer, wherever
//! it rested. It hides nothing a hand presses — a press lands where the
//! pointer points, through it — so what covers a window, and a check that
//! nothing stands over a fixture, leave it out.
//!
//! It is known by three facts together, never by one: the cursor's level,
//! the window server as its owner, and a size no bigger than a pointer. A
//! window at that level that is bigger, or another app's, is a cover all the
//! same. Nothing here reads a title or what a window shows.

/// The cursor's window level: `CGWindowLevelForKey(kCGCursorWindowLevelKey)`
/// on macOS, the level the pointer's window was listed at when measured.
pub const CURSOR_WINDOW_LAYER: i64 = 2_147_483_630;

/// The window server's name as a window list gives a window's owner
/// (`kCGWindowOwnerName`).
pub const WINDOW_SERVER_OWNER: &str = "WindowServer";

/// The most points a side of the pointer's picture spans: a cursor image is
/// at most 32 points a side, and the pointer is drawn at most four times its
/// size (Accessibility › Display › Pointer size). Measured at the normal
/// size: 23×22.
pub const POINTER_PICTURE_MAX_SIDE_PT: f64 = 128.0;

/// Whether a listed window — its `layer`, its owner's name `owner`, and its
/// size in points — is the pointer's own picture: all three facts, or not.
#[must_use]
pub fn is_pointer_picture(layer: i64, owner: &str, width: f64, height: f64) -> bool {
    layer == CURSOR_WINDOW_LAYER
        && owner == WINDOW_SERVER_OWNER
        && width <= POINTER_PICTURE_MAX_SIDE_PT
        && height <= POINTER_PICTURE_MAX_SIDE_PT
}

#[cfg(test)]
mod tests;
