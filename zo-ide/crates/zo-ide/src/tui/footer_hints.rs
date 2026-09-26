//! The footer's second row — codex 0.157.1 `bottom_pane/footer.rs`
//! (`left_side_line`, the narrow-width ladder) and
//! `bottom_pane/chat_composer/warning_notice.rs`, on zo's [`Line`].
//!
//! Row one stays [`super::view::footer`] (model · cwd · indicators). This row
//! says what the keys do right now on the left and how many warnings the
//! conversation has shown on the right:
//!
//! ```text
//!   claude-opus-5 high · ~/project
//!   ← for agents · ? for shortcuts                ⚠ 4 warnings · f2 to view
//! ```
//!
//! Keys are bold, their words dim; only the count itself wears the warning
//! colour ([`palette::WARNING_NOTICE`]).

use super::ansi::{Line, Span, Style};
use super::mention::{KEY_ESC, KEY_LEFT};
use super::palette;

/// Codex `ui_consts.rs::FOOTER_INDENT_COLS`.
pub const INDENT: &str = "  ";
/// Codex `warning_notice.rs`: one clear cell at the terminal's right edge.
const BADGE_RIGHT_MARGIN: usize = 1;
/// Codex `chat_composer.rs::render_with_options`: the hint area ends two
/// columns before the badge.
const BADGE_GAP: usize = 2;
/// Codex `warning_notice.rs::warning_notice_layout`: the badge may take half
/// the row, and never less than this.
const BADGE_MIN_BUDGET: usize = 14;

/// The `?` card's key and how the row writes it.
pub const SHORTCUTS_CHAR: char = '?';
pub const KEY_SHORTCUTS: &str = "?";
pub const KEY_QUEUE: &str = "tab";
/// Codex's `app.open_warnings` default, F2, and how the row writes it.
pub const WARNINGS_FUNCTION_KEY: u8 = 2;
pub const KEY_WARNINGS: &str = "f2";
pub const KEY_QUIT: &str = "ctrl + c";
const FOR_AGENTS: &str = " for agents";
const FOR_SHORTCUTS: &str = " for shortcuts";
const TO_QUEUE_MESSAGE: &str = " to queue message";
const TO_QUEUE: &str = " to queue";
const CLOSE_JOIN: &str = " / ";
const CLOSE: &str = " close";
pub const AGAIN_TO_QUIT: &str = " again to quit";
pub const SEPARATOR: &str = " · ";
const WARNING_MARK: &str = "⚠ ";
const TO_VIEW: &str = " to view";

/// What the left half says — codex `FooterMode`, the states zo has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HintMode {
    /// `ComposerEmpty`: `← for agents · ? for shortcuts`.
    #[default]
    Empty,
    /// `ComposerHasDraft` with no turn running: nothing on the left.
    Draft,
    /// `ComposerHasDraft` while a turn runs: `tab to queue message`.
    Queue,
    /// `ShortcutOverlay`: `? / esc close`.
    Overlay,
    /// `QuitShortcutReminder`: `ctrl + c again to quit`.
    QuitReminder,
}

impl HintMode {
    /// Codex `show_warning_notice`: the badge stands beside the passive hints
    /// only — an empty prompt, or a draft with no turn running. A queue hint,
    /// the open card and the quit reminder keep the row to themselves.
    const fn shows_badge(self) -> bool {
        matches!(self, Self::Empty | Self::Draft)
    }
}

/// Everything the second row reads, as one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FooterHints {
    pub mode: HintMode,
    /// Distinct warnings this conversation has shown; `0` draws no badge.
    pub warnings: usize,
}

/// The second footer row at `width` columns.
///
/// Codex's narrow ladder: the whole hint when it fits; for an empty prompt
/// `? for shortcuts` goes first and `← for agents` stays, the queue and close
/// hints take their short words; whatever is left is cut two columns before
/// the badge.
#[must_use]
pub fn hint_row(hints: FooterHints, width: usize) -> Line {
    let badge = if hints.mode.shows_badge() {
        warning_badge(hints.warnings, width)
    } else {
        None
    };
    let badge_at = badge
        .as_ref()
        .map(|badge| width.saturating_sub(badge.width() + BADGE_RIGHT_MARGIN));
    let room = badge_at
        .map_or(width, |at| at.saturating_sub(BADGE_GAP))
        .saturating_sub(INDENT.len());
    let full = left_line(hints.mode, false);
    let left = if full.width() <= room {
        full
    } else {
        left_line(hints.mode, true).truncated(room)
    };
    let mut spans = Vec::new();
    if left.width() > 0 {
        spans.push(Span::raw(INDENT));
        spans.extend(left.spans);
    }
    if let (Some(badge), Some(at)) = (badge, badge_at) {
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(Span::raw(" ".repeat(at.saturating_sub(used))));
        spans.extend(badge.spans);
    }
    Line::new(spans)
}

/// The left half — codex `footer.rs::left_side_line` for the full words,
/// its narrow fallbacks when `short`.
fn left_line(mode: HintMode, short: bool) -> Line {
    let key = |text: &str| Span::bold(text);
    let words = |text: &str| Span::dim(text);
    let spans = match mode {
        HintMode::Empty if short => vec![key(KEY_LEFT), words(FOR_AGENTS)],
        HintMode::Empty => vec![
            key(KEY_LEFT),
            words(FOR_AGENTS),
            words(SEPARATOR),
            key(KEY_SHORTCUTS),
            words(FOR_SHORTCUTS),
        ],
        HintMode::Draft => Vec::new(),
        HintMode::Queue => vec![
            key(KEY_QUEUE),
            words(if short { TO_QUEUE } else { TO_QUEUE_MESSAGE }),
        ],
        HintMode::Overlay if short => vec![key(KEY_ESC), words(CLOSE)],
        HintMode::Overlay => vec![
            key(KEY_SHORTCUTS),
            words(CLOSE_JOIN),
            key(KEY_ESC),
            words(CLOSE),
        ],
        HintMode::QuitReminder => vec![key(KEY_QUIT), words(AGAIN_TO_QUIT)],
    };
    Line::new(spans)
}

/// Codex `warning_notice.rs::warning_notice`: the longest of
/// `⚠ N warnings · f2 to view`, `⚠ N · f2` and `⚠ N` that fits the badge's
/// budget — half the row less its clear cell, at least
/// `BADGE_MIN_BUDGET` — or nothing when not even the count fits.
#[must_use]
pub fn warning_badge(count: usize, width: usize) -> Option<Line> {
    if count == 0 {
        return None;
    }
    let available = width.saturating_sub(BADGE_RIGHT_MARGIN);
    let budget = (available / 2).max(BADGE_MIN_BUDGET).min(available);
    let amber = Style::new().fg(palette::WARNING_NOTICE);
    let plural = if count == 1 { "" } else { "s" };
    let full = Line::new(vec![
        Span::dim(WARNING_MARK),
        Span::new(format!("{count} warning{plural}"), amber),
        Span::dim(SEPARATOR),
        Span::bold(KEY_WARNINGS),
        Span::dim(TO_VIEW),
    ]);
    let compact = Line::new(vec![
        Span::dim(WARNING_MARK),
        Span::new(count.to_string(), amber),
        Span::dim(SEPARATOR),
        Span::bold(KEY_WARNINGS),
    ]);
    let bare = Line::new(vec![Span::dim(WARNING_MARK), Span::new(count.to_string(), amber)]);
    [full, compact, bare]
        .into_iter()
        .find(|line| line.width() <= budget)
}

#[cfg(test)]
mod tests {
    use super::{hint_row, warning_badge, FooterHints, HintMode};
    use crate::tui::ansi::{Color, Style};
    use crate::tui::palette;

    fn row(mode: HintMode, warnings: usize, width: usize) -> String {
        hint_row(FooterHints { mode, warnings }, width).plain()
    }

    #[test]
    fn each_mode_says_what_the_keys_do_now() {
        assert_eq!(row(HintMode::Empty, 0, 80), "  ← for agents · ? for shortcuts");
        assert_eq!(row(HintMode::Draft, 0, 80), "");
        assert_eq!(row(HintMode::Queue, 0, 80), "  tab to queue message");
        assert_eq!(row(HintMode::Overlay, 0, 80), "  ? / esc close");
        assert_eq!(row(HintMode::QuitReminder, 0, 80), "  ctrl + c again to quit");
    }

    /// Codex's narrow ladder: `? for shortcuts` goes first and `← for
    /// agents` stays; the queue and close hints have their short words.
    #[test]
    fn a_narrow_row_drops_the_shortcuts_hint_before_the_agents_hint() {
        assert_eq!(row(HintMode::Empty, 0, 32), "  ← for agents · ? for shortcuts");
        assert_eq!(row(HintMode::Empty, 0, 31), "  ← for agents");
        assert_eq!(row(HintMode::Empty, 0, 8), "  ← for…");
        assert_eq!(row(HintMode::Queue, 0, 22), "  tab to queue message");
        assert_eq!(row(HintMode::Queue, 0, 21), "  tab to queue");
        assert_eq!(row(HintMode::Overlay, 0, 14), "  esc close");
    }

    #[test]
    fn the_badge_stands_right_with_one_clear_cell_and_falls_back_in_three_steps() {
        let wide = row(HintMode::Empty, 3, 80);
        assert!(wide.ends_with("⚠ 3 warnings · f2 to view"), "{wide:?}");
        assert_eq!(wide.chars().count(), 79, "one clear cell at the right edge: {wide:?}");
        assert!(wide.starts_with("  ← for agents · ? for shortcuts "), "{wide:?}");
        assert!(row(HintMode::Draft, 1, 80).ends_with("⚠ 1 warning · f2 to view"));
        // Half of 39 is 19: the long words do not fit, the key does.
        let narrow = row(HintMode::Empty, 2, 40);
        assert!(narrow.ends_with("⚠ 2 · f2"), "{narrow:?}");
        assert!(narrow.starts_with("  ← for agents "), "{narrow:?}");
        assert!(!narrow.contains("shortcuts"), "{narrow:?}");
        assert_eq!(warning_badge(12, 16).map(|line| line.plain()), Some("⚠ 12 · f2".to_string()));
        assert_eq!(warning_badge(12, 8).map(|line| line.plain()), Some("⚠ 12".to_string()));
        assert_eq!(warning_badge(12, 4), None);
    }

    #[test]
    fn the_badge_yields_to_the_queue_the_card_and_the_quit_reminder() {
        assert_eq!(row(HintMode::Queue, 2, 80), "  tab to queue message");
        assert_eq!(row(HintMode::Overlay, 2, 80), "  ? / esc close");
        assert_eq!(row(HintMode::QuitReminder, 2, 80), "  ctrl + c again to quit");
        assert!(row(HintMode::Draft, 2, 80).trim_start().starts_with("⚠ 2 warnings"));
    }

    #[test]
    fn keys_are_bold_words_dim_and_only_the_count_wears_the_warning_colour() {
        let line = hint_row(
            FooterHints {
                mode: HintMode::Empty,
                warnings: 3,
            },
            80,
        );
        let style_of = |text: &str| {
            line.spans
                .iter()
                .find(|span| span.text == text)
                .unwrap_or_else(|| panic!("no span {text:?} in {:?}", line.spans))
                .style
        };
        assert_eq!(style_of("←"), Style::new().bold());
        assert_eq!(style_of(" for agents"), Style::new().dim());
        assert_eq!(style_of("?"), Style::new().bold());
        assert_eq!(style_of("⚠ "), Style::new().dim());
        assert_eq!(style_of("3 warnings"), Style::new().fg(palette::WARNING_NOTICE));
        assert_eq!(style_of("f2"), Style::new().bold());
        assert_eq!(style_of(" to view"), Style::new().dim());
        assert_eq!(palette::WARNING_NOTICE, Color::Rgb(196, 167, 103));
    }
}
