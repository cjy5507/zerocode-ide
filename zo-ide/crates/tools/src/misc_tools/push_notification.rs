//! `PushNotification` — call the person only when they are away (t-2943).
//!
//! Claude Code's tool of the same name sends a desktop notification and, when
//! Remote Control is connected, a phone push; when the person is at the
//! terminal it sends nothing and says so ("a 'not sent' result is expected").
//! zo has no phone service, so its roads are the ones it already owns: the
//! `ZeroCode` window's OS notification (whose click focuses the pane) and the
//! terminal's own notification protocols. The design table is
//! `docs/design/zo-push-notification.md`.
//!
//! This module holds what every surface shares: the one table of limits, the
//! one judgement of which road a surface takes, the shape of a message, and
//! the stamp a frontend writes on every key so "attended" is a measured fact.
//! Delivery itself lives with the surface (`user_question_bridge.rs`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::ToolError;

/// The tool's registered name.
pub const PUSH_NOTIFICATION_TOOL: &str = "PushNotification";

/// The one table of limits. Every number this tool obeys is a field here,
/// and every field has one env override, so an e2e can open the terminal
/// road by lowering the attended window instead of waiting a minute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PushLimits {
    /// Longest body that goes out. Claude Code only hints at 200 ("mobile
    /// OSes truncate"); this cuts on a char boundary and says so in the
    /// receipt, so what the person sees is what the model was told went out.
    pub max_message_chars: usize,
    /// How recently a key must have been pressed at this zo for the person to
    /// count as at the keyboard. The same one-breath minute the window gives a
    /// notification click (`RING_CLICK_WINDOW_MS`): somebody who typed within
    /// it is still reading the screen the answer lands on.
    pub attended_window: Duration,
}

impl Default for PushLimits {
    fn default() -> Self {
        Self {
            max_message_chars: 200,
            attended_window: Duration::from_secs(60),
        }
    }
}

impl PushLimits {
    /// Env override of [`Self::max_message_chars`]; `0` or garbage keeps the default.
    pub const MAX_MESSAGE_CHARS_ENV: &str = "ZO_PUSH_MAX_MESSAGE_CHARS";
    /// Env override of [`Self::attended_window`] in milliseconds; `0` is a
    /// real value — nobody ever counts as attended — and garbage keeps the default.
    pub const ATTENDED_WINDOW_MS_ENV: &str = "ZO_PUSH_ATTENDED_WINDOW_MS";

    /// The table with one environment's overrides applied. A closure rather
    /// than `std::env` so a test can state an environment without touching
    /// the process every other test shares.
    pub fn read(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut limits = Self::default();
        if let Some(chars) = lookup(Self::MAX_MESSAGE_CHARS_ENV)
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|chars| *chars > 0)
        {
            limits.max_message_chars = chars;
        }
        if let Some(millis) =
            lookup(Self::ATTENDED_WINDOW_MS_ENV).and_then(|value| value.trim().parse::<u64>().ok())
        {
            limits.attended_window = Duration::from_millis(millis);
        }
        limits
    }

    /// From the real environment.
    #[must_use]
    pub fn from_env() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }
}

/// Which road a push took — the word the receipt says, always. Spelled in the
/// runtime beside the render block that carries it.
pub use runtime::message_stream::NotificationRoad as PushRoad;

/// The sentence the model reads beside the road — why a skip is fine, and
/// what a delivery means.
#[must_use]
pub const fn road_note(road: PushRoad) -> &'static str {
    match road {
        PushRoad::Window => {
            "Handed to the ZeroCode window; it notifies unless the person is already looking at this pane."
        }
        PushRoad::Terminal => "Rang the terminal (bell + OSC 9/777); the turn continues.",
        PushRoad::SkippedAttended => {
            "The person is at the keyboard, so your output already reaches them. Not sent, as expected."
        }
        PushRoad::SkippedNowhere => {
            "No surface can carry a notification here; the message is returned inline."
        }
    }
}

/// The facts one surface can state about itself, gathered before judging.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PushSurface {
    /// A key was pressed at this zo within [`PushLimits::attended_window`].
    pub attended: bool,
    /// Somebody is subscribed to this session's events channel — a window.
    pub window: bool,
    /// Stdout is a terminal that can carry a bell.
    pub terminal: bool,
}

/// The one judgement: a person at the keyboard beats every road, a window
/// beats a terminal, a terminal beats nothing. Pure, so the receipt table is a
/// unit test rather than a hope.
#[must_use]
pub const fn push_road(surface: PushSurface) -> PushRoad {
    if surface.attended {
        PushRoad::SkippedAttended
    } else if surface.window {
        PushRoad::Window
    } else if surface.terminal {
        PushRoad::Terminal
    } else {
        PushRoad::SkippedNowhere
    }
}

/// What goes out: a title for the terminal protocols and the window, and the
/// one-line body the model wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushNotice {
    pub title: String,
    pub body: String,
}

/// One line, capped at the table. Newlines and runs of whitespace fold to one
/// space — a notification has no second line anywhere it lands — and the cut
/// is on a char boundary so a multi-byte body is never split mid-codepoint.
/// Returns the body and whether it was cut; an empty body is refused because
/// a notification that says nothing is noise with a bell on it.
pub fn shape_message(raw: &str, limits: &PushLimits) -> Result<(String, bool), ToolError> {
    let one_line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.is_empty() {
        return Err(ToolError::InvalidInput(
            "'message' must not be empty".to_string(),
        ));
    }
    if one_line.chars().count() <= limits.max_message_chars {
        return Ok((one_line, false));
    }
    let kept: String = one_line.chars().take(limits.max_message_chars).collect();
    Ok((kept, true))
}

/// The moment a person last pressed a key at this zo — a stamp the frontend
/// writes on every key event and the push bridge reads. Shared by handle so a
/// test can hold its own; production holds one per process
/// ([`Self::process`]), because one process serves one keyboard.
#[derive(Debug, Clone, Default)]
pub struct KeyboardPresence {
    /// Milliseconds since the process epoch, plus one so `0` stays "never".
    last_input_ms: Arc<AtomicU64>,
}

impl KeyboardPresence {
    /// The one presence of this process.
    #[must_use]
    pub fn process() -> &'static Self {
        static PRESENCE: OnceLock<KeyboardPresence> = OnceLock::new();
        PRESENCE.get_or_init(Self::default)
    }

    /// A key was pressed now.
    pub fn note_input(&self) {
        self.note_input_at(process_millis());
    }

    /// Whether a key was pressed within `window` of now.
    #[must_use]
    pub fn attended_within(&self, window: Duration) -> bool {
        self.attended_within_at(window, process_millis())
    }

    /// [`Self::note_input`] with the clock stated, for tests.
    pub fn note_input_at(&self, now_ms: u64) {
        self.last_input_ms
            .store(now_ms.saturating_add(1), Ordering::Relaxed);
    }

    /// [`Self::attended_within`] with the clock stated, for tests. A window of
    /// zero never counts as attended, whatever was pressed — that is the knob
    /// an e2e turns to open the terminal road.
    #[must_use]
    pub fn attended_within_at(&self, window: Duration, now_ms: u64) -> bool {
        let stamped = self.last_input_ms.load(Ordering::Relaxed);
        if stamped == 0 || window.is_zero() {
            return false;
        }
        let window_ms = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
        now_ms.saturating_add(1).saturating_sub(stamped) <= window_ms
    }
}

/// Milliseconds since this process first asked — a monotonic clock the stamp
/// and the reader share.
fn process_millis() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(Instant::now);
    u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_reads_its_overrides_and_keeps_defaults_for_garbage() {
        let none = PushLimits::read(|_| None);
        assert_eq!(none, PushLimits::default());

        let set = PushLimits::read(|name| match name {
            PushLimits::MAX_MESSAGE_CHARS_ENV => Some("120".to_string()),
            PushLimits::ATTENDED_WINDOW_MS_ENV => Some("0".to_string()),
            _ => None,
        });
        assert_eq!(set.max_message_chars, 120);
        assert_eq!(set.attended_window, Duration::ZERO);

        let garbage = PushLimits::read(|name| match name {
            PushLimits::MAX_MESSAGE_CHARS_ENV => Some("0".to_string()),
            PushLimits::ATTENDED_WINDOW_MS_ENV => Some("soon".to_string()),
            _ => None,
        });
        assert_eq!(garbage, PushLimits::default());
    }

    #[test]
    fn a_key_within_the_window_means_attended_and_a_zero_window_never_does() {
        let presence = KeyboardPresence::default();
        let window = Duration::from_secs(60);
        assert!(!presence.attended_within_at(window, 5_000), "never typed");

        presence.note_input_at(10_000);
        assert!(presence.attended_within_at(window, 10_000));
        assert!(presence.attended_within_at(window, 70_000), "at the edge");
        assert!(!presence.attended_within_at(window, 70_001), "past it");
        assert!(!presence.attended_within_at(Duration::ZERO, 10_000));
    }

    #[test]
    fn the_wire_words_round_trip() {
        for road in [
            PushRoad::Window,
            PushRoad::Terminal,
            PushRoad::SkippedAttended,
            PushRoad::SkippedNowhere,
        ] {
            assert_eq!(PushRoad::parse(road.as_str()), Some(road));
        }
        assert_eq!(PushRoad::parse("phone"), None);
    }

    #[test]
    fn the_body_folds_to_one_line_and_cuts_on_a_char_boundary() {
        let limits = PushLimits {
            max_message_chars: 5,
            ..PushLimits::default()
        };
        assert_eq!(
            shape_message(" a \n b\t\tc ", &limits).expect("shaped"),
            ("a b c".to_string(), false)
        );
        assert_eq!(
            shape_message("가나다라마바사", &limits).expect("shaped"),
            ("가나다라마".to_string(), true)
        );
        assert!(matches!(
            shape_message(" \n\t", &limits),
            Err(ToolError::InvalidInput(_))
        ));
    }
}
