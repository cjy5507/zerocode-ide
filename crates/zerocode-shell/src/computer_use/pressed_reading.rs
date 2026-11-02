//! What a press by words read of a window, kept a moment (t-89036): the screen
//! as it stood before the keys that follow, read already, so a value it showed
//! then proves nothing about those keys and no more pictures are taken for it.
//!
//! It stays in this process's memory for one typing or [`KEPT_FOR`], is never
//! written anywhere, and a screen that asks for a secret is not kept.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::Value;
use zerocode_core::computer_use_protocol::typed_check::shows_a_secret_field;
use zerocode_core::computer_use_protocol::words::ReadLine;

/// Longer than a batch's press and keys take; a reading this old is another screen.
const KEPT_FOR: Duration = Duration::from_secs(30);

struct Kept {
    window: String,
    at: Instant,
    lines: Vec<ReadLine>,
}

static KEPT: Mutex<Vec<Kept>> = Mutex::new(Vec::new());

/// Keep what `window`, a reading's own words, just read.
pub(crate) fn keep(window: &Value, lines: &[ReadLine]) {
    let window = window.to_string();
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|reading| reading.window != window && reading.at.elapsed() < KEPT_FOR);
    if !shows_a_secret_field(lines) {
        kept.push(Kept {
            window,
            at: Instant::now(),
            lines: lines.to_vec(),
        });
    }
}

/// What `window` was kept reading, which goes with this call; none when nothing was.
pub(crate) fn take(window: &Value) -> Vec<ReadLine> {
    let window = window.to_string();
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    kept.retain(|reading| reading.at.elapsed() < KEPT_FOR);
    kept.iter()
        .position(|reading| reading.window == window)
        .map(|at| kept.swap_remove(at).lines)
        .unwrap_or_default()
}
