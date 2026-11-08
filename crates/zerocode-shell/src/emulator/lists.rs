//! Scrolling and walking a list on one device (`scroll`, `scroll-until`,
//! `collect`), on whichever road the device takes.
//!
//! A swipe flicks: the finger lifts while it moves and the list coasts on past
//! the rows after it, so a walk that swipes skips rows and never says so
//! (Phone Harness measured 5 of 13 chats skipped, 2026-11). A scroll here rests
//! its finger before it lifts, the same on every road, and a walk compares
//! what each screen reads to know when it has reached the end — the words, not
//! the pixels, so a clock that ticks over is not a list that moved.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::agent_emulator::real_android::is_android_phone;
use zerocode_core::agent_emulator::real_phone::is_real_phone;
use zerocode_core::agent_emulator::{
    EMULATOR_COLLECT_END_AFTER, EMULATOR_LIST_BUDGET_MS, EMULATOR_VISIBLE_CHARS_MAX,
};
use zerocode_core::computer_use::{
    EMULATOR_SCROLL_AMOUNT_DEFAULT, EmulatorDirection, EmulatorPlatform,
};
use zerocode_core::computer_use_protocol::ProviderError;

use super::checks::{self, COUNT_KEY, VISIBLE_KEY};
use super::marks::{self, Device, Snapshot};

/// Where a scroll drags when it is not told: the screen's middle.
pub(crate) const SCREEN_MIDDLE: (f64, f64) = (0.5, 0.5);

/// The most lines a `collect` answers: a long chat or feed is still one answer,
/// not a transcript.
const COLLECT_LINES_MAX: usize = 1_000;

/// Why a list walk stopped, as its answer says it.
const STOP_REACHED_END: &str = "reached-end";
const STOP_MAX_SCROLLS: &str = "max-scrolls";
const STOP_TIME: &str = "time";

/// One scroll on the device: `amount` of the screen brought into view in
/// `direction`, dragged at `centre` — through the device's own road and its
/// own guards (on a real phone that states money, only a vertical one goes).
pub(crate) async fn scroll(
    platform: EmulatorPlatform,
    device: &Device,
    direction: EmulatorDirection,
    amount: Option<f64>,
    centre: Option<(f64, f64)>,
) -> Result<Value, ProviderError> {
    let amount = amount.unwrap_or(EMULATOR_SCROLL_AMOUNT_DEFAULT);
    let (from, to) = direction.finger_path(centre.unwrap_or(SCREEN_MIDDLE), amount);
    let address = device.address.clone();
    let mut answer = if is_android_phone(&address) {
        super::android::phone::scroll(&address, from, to).await?
    } else if is_real_phone(&address) {
        super::wda::scroll(&address, from, to).await?
    } else {
        match platform {
            EmulatorPlatform::Ios => super::ios::ios_scroll_direct(address, from, to).await,
            EmulatorPlatform::Android => {
                super::android::android_scroll_direct(address, from, to).await
            }
        }
        .map_err(marks::backend_error)?;
        json!({ "performed": true })
    };
    answer["direction"] = json!(direction.as_str());
    answer["amount"] = json!(amount);
    Ok(answer)
}

/// `scroll-until`: scroll until `subject` is on the screen, the screen stops
/// changing, `max` scrolls are spent or the walk's time is. Found, it answers
/// the count and how many scrolls it took; not, why it stopped and what the
/// last screen showed.
pub(crate) async fn scroll_until(
    platform: EmulatorPlatform,
    device: Device,
    subject: &str,
    direction: EmulatorDirection,
    amount: Option<f64>,
    max: u32,
) -> Result<Value, ProviderError> {
    let started = Instant::now();
    let budget = Duration::from_millis(EMULATOR_LIST_BUDGET_MS);
    let mut screen = marks::snapshot(platform, &device).await?;
    let mut scrolls = 0_u32;
    let mut unmoved = 0_u32;
    let stop = loop {
        let count = checks::found(&screen, subject);
        if count > 0 {
            return Ok(json!({
                "found": true,
                COUNT_KEY: count,
                "scrolls": scrolls,
                "direction": direction.as_str(),
            }));
        }
        if unmoved >= EMULATOR_COLLECT_END_AFTER {
            break STOP_REACHED_END;
        }
        if scrolls >= max {
            break STOP_MAX_SCROLLS;
        }
        if started.elapsed() >= budget {
            break STOP_TIME;
        }
        scroll(platform, &device, direction, amount, None).await?;
        scrolls += 1;
        let next = marks::snapshot(platform, &device).await?;
        if checks::words_of(&next) == checks::words_of(&screen) {
            unmoved += 1;
        } else {
            unmoved = 0;
        }
        screen = next;
    };
    Ok(json!({
        "found": false,
        COUNT_KEY: 0,
        "scrolls": scrolls,
        "stop": stop,
        "direction": direction.as_str(),
        VISIBLE_KEY: checks::visible(&screen),
    }))
}

/// `collect`: walk a list to its end and answer every line it read, once each
/// and in the order it first came into view. It ends when
/// [`EMULATOR_COLLECT_END_AFTER`] scrolls in a row bring no new line (a lazy
/// list had a scroll's grace to load), after `max` scrolls, or when the walk's
/// time is spent — and says which.
pub(crate) async fn collect(
    platform: EmulatorPlatform,
    device: Device,
    direction: EmulatorDirection,
    amount: Option<f64>,
    max: u32,
) -> Result<Value, ProviderError> {
    let started = Instant::now();
    let budget = Duration::from_millis(EMULATOR_LIST_BUDGET_MS);
    let mut lines = Lines::default();
    lines.take(&marks::snapshot(platform, &device).await?);
    let mut scrolls = 0_u32;
    let mut dry = 0_u32;
    let stop = loop {
        if dry >= EMULATOR_COLLECT_END_AFTER {
            break STOP_REACHED_END;
        }
        if scrolls >= max {
            break STOP_MAX_SCROLLS;
        }
        if started.elapsed() >= budget || lines.full() {
            break STOP_TIME;
        }
        scroll(platform, &device, direction, amount, None).await?;
        scrolls += 1;
        if lines.take(&marks::snapshot(platform, &device).await?) == 0 {
            dry += 1;
        } else {
            dry = 0;
        }
    };
    Ok(json!({
        "items": lines.read,
        COUNT_KEY: lines.read.len(),
        "scrolls": scrolls,
        "stop": stop,
        "direction": direction.as_str(),
    }))
}

/// The lines a walk has read: each once, in the order it first came into view.
#[derive(Default)]
struct Lines {
    seen: std::collections::HashSet<String>,
    read: Vec<String>,
}

impl Lines {
    /// Take a screen's lines and say how many were new.
    fn take(&mut self, screen: &Snapshot) -> usize {
        let before = self.read.len();
        for line in checks::words_of(screen) {
            if self.full() {
                break;
            }
            let line: String = line.chars().take(EMULATOR_VISIBLE_CHARS_MAX * 4).collect();
            if self.seen.insert(line.clone()) {
                self.read.push(line);
            }
        }
        self.read.len() - before
    }

    fn full(&self) -> bool {
        self.read.len() >= COLLECT_LINES_MAX
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::computer_use_protocol::render::Rect;

    fn screen(lines: &[&str]) -> Snapshot {
        let children: Vec<Value> = lines
            .iter()
            .enumerate()
            .map(|(at, line)| {
                let top = 40 * at;
                json!({"className": "android.widget.TextView", "text": line, "enabled": true,
                    "clickable": false, "packageName": "com.example.list",
                    "bounds": {"left": 0, "top": top, "right": 300, "bottom": top + 30}})
            })
            .collect();
        Snapshot::new(
            EmulatorPlatform::Android,
            &json!({ "children": children }),
            Rect::new(0.0, 0.0, 390.0, 844.0),
        )
        .unwrap()
    }

    /// A walk keeps each line once, in the order it first came into view; two
    /// screens that overlap add only what is new, and a screen that brings
    /// nothing new says so — the count `collect` ends on.
    #[test]
    fn a_walk_keeps_each_line_once_in_the_order_it_came_into_view() {
        let mut lines = Lines::default();
        assert_eq!(lines.take(&screen(&["Ann", "Ben", "Cid"])), 3);
        assert_eq!(lines.take(&screen(&["Cid", "Dee", "Eve"])), 2);
        assert_eq!(lines.take(&screen(&["Dee", "Eve"])), 0);
        assert_eq!(lines.read, ["Ann", "Ben", "Cid", "Dee", "Eve"]);
    }

    /// The end of a list is a screen whose words did not change — the words,
    /// so the same rows read in the same order are the same screen.
    #[test]
    fn a_screen_that_reads_the_same_words_is_a_list_that_did_not_move() {
        assert_eq!(
            checks::words_of(&screen(&["Ann", "Ben"])),
            checks::words_of(&screen(&["Ann", "Ben"]))
        );
        assert_ne!(
            checks::words_of(&screen(&["Ann", "Ben"])),
            checks::words_of(&screen(&["Ben", "Cid"]))
        );
    }

    /// A walk stops taking lines at its cap, so one answer never grows past it.
    #[test]
    fn a_walk_stops_taking_lines_at_its_cap() {
        let mut lines = Lines::default();
        let many: Vec<String> = (0..COLLECT_LINES_MAX + 5)
            .map(|n| format!("row {n}"))
            .collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        lines.take(&screen(&refs));
        assert!(lines.full());
        assert_eq!(lines.read.len(), COLLECT_LINES_MAX);
    }
}
