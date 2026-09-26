//! The `?` card — codex 0.157.1 `bottom_pane/shortcut_overlay.rs` and the
//! column flow of `shortcut_help.rs`, listing zo's own keys.
//!
//! Three groups (Compose · Session · Transcript) flow into three, two or one
//! column by width; zo's keep-list of slash commands
//! ([`super::view::shortcut_card`]) follows where codex prints
//! `/keymap customize`. A key zo does not have is not on the card.

use super::agents::OPEN_KEY_LABEL;
use super::ansi::{Line, Span, Style};
use super::footer_hints::{INDENT, KEY_QUEUE, KEY_WARNINGS};
use super::mention::{KEY_LEFT, KEY_SLASH};
use super::palette;
use super::pending_input::EDIT_BINDING;

/// Codex `shortcut_help.rs::COLUMN_GAP`.
const COLUMN_GAP: usize = 4;
/// Codex `shortcut_help.rs::Group::lines`: two cells between a key and its
/// action.
const KEY_GAP: usize = 2;
const TITLE: &str = "Keyboard shortcuts";
/// Codex `shortcut_overlay.rs::render` when the card is taller than its room.
pub const RESIZE_NOTICE: &str = "… resize to see all";

struct Group {
    title: &'static str,
    entries: Vec<(&'static str, &'static str)>,
}

impl Group {
    /// Codex `Group::lines`: a bold title, then each key in the command
    /// colour, padded to the group's longest key and [`KEY_GAP`].
    fn lines(&self) -> Vec<Line> {
        let key_width = self
            .entries
            .iter()
            .map(|(key, _)| Span::raw(*key).width())
            .max()
            .unwrap_or(0);
        let mut lines = vec![Line::new(vec![Span::bold(self.title)])];
        for (key, action) in &self.entries {
            let key = Span::new(*key, Style::new().fg(palette::COMMAND_TOKEN));
            let padding = " ".repeat(key_width.saturating_sub(key.width()) + KEY_GAP);
            lines.push(Line::new(vec![key, Span::raw(padding), Span::raw(*action)]));
        }
        lines
    }
}

/// zo's keys, in codex's three groups. Only keys zo binds are listed: idle
/// Tab completes the popup rather than sending, so Tab is on the card only
/// while a turn runs and it queues.
fn groups(running: bool) -> [Group; 3] {
    let mut session = Vec::new();
    if running {
        session.push((KEY_QUEUE, "Queue message"));
    }
    session.extend([
        ("shift+tab", "Cycle permissions"),
        (OPEN_KEY_LABEL, "Agents"),
        (KEY_LEFT, "Agents (empty prompt)"),
        (EDIT_BINDING, "Edit last queued message"),
        (KEY_WARNINGS, "Warnings"),
        ("ctrl+c", if running { "Interrupt" } else { "Quit" }),
    ]);
    [
        Group {
            title: "Compose",
            entries: vec![
                (KEY_SLASH, "Commands"),
                ("@", "Mention files"),
                ("ctrl+v", "Paste image"),
                ("↑ / ↓", "History"),
            ],
        },
        Group {
            title: "Session",
            entries: session,
        },
        Group {
            title: "Transcript (open first)",
            entries: vec![
                ("ctrl+t", "Open transcript"),
                ("pgup / pgdn", "Scroll"),
                ("home / end", "Top / latest"),
            ],
        },
    ]
}

/// The card at `width` columns; `running` picks the words a turn changes
/// (Tab queues, Ctrl+C interrupts). The `/help` cell prints the same rows.
#[must_use]
pub fn card(width: usize, running: bool) -> Vec<Line> {
    let inner = width.saturating_sub(INDENT.len()).max(1);
    let mut lines = vec![Line::new(vec![Span::bold(TITLE)]), Line::empty()];
    lines.extend(group_lines(groups(running), inner));
    lines.push(Line::empty());
    lines.extend(
        super::view::shortcut_card()
            .into_iter()
            .map(|entry| Line::new(vec![Span::dim(entry)])),
    );
    lines
        .into_iter()
        .map(|line| {
            if line.width() == 0 {
                line
            } else {
                line.prefixed(Span::raw(INDENT)).truncated(width)
            }
        })
        .collect()
}

/// Codex `shortcut_help.rs::group_lines`: three columns when they fit, else
/// the first group beside the other two stacked, else one column.
fn group_lines(groups: [Group; 3], width: usize) -> Vec<Line> {
    let groups = groups.map(|group| group.lines());
    let widths = groups
        .each_ref()
        .map(|group| group.iter().map(Line::width).max().unwrap_or(0));
    if widths.iter().sum::<usize>() + COLUMN_GAP * 2 <= width {
        return columns(&groups, &widths);
    }
    let [compose, session, transcript] = groups;
    let right_width = widths[1].max(widths[2]);
    if widths[0] + COLUMN_GAP + right_width <= width {
        let mut right = session;
        right.push(Line::empty());
        right.extend(transcript);
        return columns(&[compose, right], &[widths[0], right_width]);
    }
    let mut lines = compose;
    for group in [session, transcript] {
        lines.push(Line::empty());
        lines.extend(group);
    }
    lines
}

/// Codex `shortcut_help.rs::columns`: row by row, each column padded to its
/// width and [`COLUMN_GAP`]; the last column is not padded.
fn columns(groups: &[Vec<Line>], widths: &[usize]) -> Vec<Line> {
    let height = groups.iter().map(Vec::len).max().unwrap_or(0);
    (0..height)
        .map(|row| {
            let mut spans = Vec::new();
            for (column, group) in groups.iter().enumerate() {
                let entry = group.get(row).cloned().unwrap_or_default();
                let padding = widths[column].saturating_sub(entry.width()) + COLUMN_GAP;
                spans.extend(entry.spans);
                if column + 1 < groups.len() {
                    spans.push(Span::raw(" ".repeat(padding)));
                }
            }
            Line::new(spans)
        })
        .collect()
}

/// The card cut to `rows`, its last row saying so — codex keeps the body's
/// head and ends it with [`RESIZE_NOTICE`].
#[must_use]
pub fn fit(card: &[Line], rows: usize) -> Vec<Line> {
    if card.len() <= rows {
        return card.to_vec();
    }
    if rows == 0 {
        return Vec::new();
    }
    let mut lines = card[..rows - 1].to_vec();
    lines.push(Line::new(vec![Span::raw(INDENT), Span::dim(RESIZE_NOTICE)]));
    lines
}

#[cfg(test)]
mod tests {
    use super::{card, fit, RESIZE_NOTICE};
    use crate::slash::Slash;
    use crate::tui::ansi::{Line, Style};

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::plain).collect()
    }

    /// Codex `shortcut_help_above_wide.snap`: three columns, four cells apart,
    /// each key column as wide as its longest key and two.
    #[test]
    fn a_wide_card_flows_into_three_columns() {
        let rows = plain(&card(130, false));
        assert_eq!(rows[0], "  Keyboard shortcuts");
        assert_eq!(rows[1], "");
        assert_eq!(
            rows[2],
            format!("  {:<25}{:<39}{}", "Compose", "Session", "Transcript (open first)")
        );
        assert_eq!(
            rows[3],
            format!(
                "  {:<25}{:<39}{}",
                "/       Commands", "shift+tab  Cycle permissions", "ctrl+t       Open transcript"
            )
        );
        assert!(rows.iter().any(|row| row.contains("←          Agents (empty prompt)")), "{rows:#?}");
        assert!(rows.iter().any(|row| row.contains("f2         Warnings")), "{rows:#?}");
        assert!(rows.iter().any(|row| row.contains("⌥ + ↑      Edit last queued message")));
        assert!(rows.iter().any(|row| row.contains("ctrl+c     Quit")));
        assert!(!rows.iter().any(|row| row.contains("Queue message")), "idle Tab does not queue");
    }

    #[test]
    fn a_narrower_card_takes_two_columns_then_one() {
        let two = plain(&card(80, false));
        assert_eq!(two[2], format!("  {:<25}{}", "Compose", "Session"));
        assert!(two.iter().any(|row| row.ends_with("Transcript (open first)")));
        let one = plain(&card(40, false));
        assert_eq!(one[2], "  Compose");
        let session = one.iter().position(|row| row == "  Session").expect("Session group");
        assert_eq!(one[session - 1], "", "groups stand a blank row apart");
        assert!(one.iter().any(|row| row == "  Transcript (open first)"));
    }

    #[test]
    fn a_running_turn_changes_what_tab_and_ctrl_c_do() {
        let rows = plain(&card(130, true));
        assert!(rows.iter().any(|row| row.contains("tab        Queue message")), "{rows:#?}");
        assert!(rows.iter().any(|row| row.contains("ctrl+c     Interrupt")));
        assert!(!rows.iter().any(|row| row.contains("ctrl+c     Quit")));
    }

    /// The keep-list stays on the card, so every command zo handles is one
    /// `?` away.
    #[test]
    fn the_card_keeps_every_slash_command() {
        let rows = plain(&card(80, false)).join("\n");
        for command in Slash::CATALOG {
            assert!(rows.contains(command.name()), "{} missing: {rows}", command.name());
        }
    }

    #[test]
    fn titles_are_bold_and_keys_wear_the_command_colour() {
        let lines = card(130, false);
        assert!(lines[0].spans.iter().any(|span| span.text == "Keyboard shortcuts"
            && span.style == Style::new().bold()));
        assert!(lines[3].spans.iter().any(|span| span.text == "/"
            && span.style == Style::new().fg(crate::tui::palette::COMMAND_TOKEN)));
    }

    #[test]
    fn a_short_room_keeps_the_head_and_says_so() {
        let lines = card(80, false);
        let cut = plain(&fit(&lines, 6));
        assert_eq!(cut.len(), 6);
        assert_eq!(cut[0], "  Keyboard shortcuts");
        assert_eq!(cut[5], format!("  {RESIZE_NOTICE}"));
        assert_eq!(fit(&lines, lines.len()).len(), lines.len(), "a card that fits is whole");
        assert!(fit(&lines, 0).is_empty());
    }
}
