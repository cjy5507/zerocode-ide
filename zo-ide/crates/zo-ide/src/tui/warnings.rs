//! The conversation's warnings, kept, and the F2 viewer that pages through
//! them — codex 0.157.1 `history_cell/warnings.rs` (a set keyed by the
//! message, so a repeat counts once), `bottom_pane/warnings_view.rs` (the
//! keys) and `warnings_view_render.rs` (the page).
//!
//! A warning is still a transcript cell ([`super::cells::system_cell`]); this
//! is what remains of it after it scrolled away. Seeing one does not lower the
//! count — only a new conversation (`/new`, `/resume`) clears it.

use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use runtime::message_stream::SystemLevel;

use super::ansi::{Line, Span};
use super::footer_hints::{INDENT, SEPARATOR};
use super::mention::{KEY_ESC, KEY_LEFT, KEY_RIGHT};
use super::wrap::wrap_line;

/// Codex `warnings_view_render.rs::desired_height`.
const VIEWER_ROWS: usize = 12;
/// Header, the blank under it, and the footer — the rows that are not body.
const VIEWER_CHROME_ROWS: usize = 3;
const TITLE: &str = "Warnings";
const EMPTY: &str = "No warnings";
const KEY_DOWN: &str = "↓";
const KEY_JOIN: &str = "/";
const BACK: &str = "back";
const WARNING: &str = "warning";
const SCROLL: &str = "scroll";

/// Where a warning came from — the header's last word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Warning,
    Error,
    /// Said before the first prompt was taken — codex `StartupWarningsCell`.
    Startup,
}

impl Source {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Warning => "Warning",
            Self::Error => "Error",
            Self::Startup => "Startup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub source: Source,
    pub details: String,
}

/// Every distinct warning this conversation has shown, in first-seen order.
#[derive(Debug, Default)]
pub struct Retained {
    entries: Vec<Entry>,
    keys: BTreeSet<String>,
    started: bool,
}

impl Retained {
    /// Keep `text` when `level` is a warning or an error; a repeat is one.
    pub fn record(&mut self, level: SystemLevel, text: &str) {
        let source = match level {
            SystemLevel::Warn if self.started => Source::Warning,
            SystemLevel::Error if self.started => Source::Error,
            SystemLevel::Warn | SystemLevel::Error => Source::Startup,
            SystemLevel::Info | SystemLevel::Success | SystemLevel::Housekeeping => return,
        };
        let details = text.trim();
        if details.is_empty() || !self.keys.insert(details.to_string()) {
            return;
        }
        self.entries.push(Entry {
            source,
            details: details.to_string(),
        });
    }

    /// The boot is over: later warnings are the session's own.
    pub fn start(&mut self) {
        self.started = true;
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.keys.len()
    }

    /// A new conversation starts with none.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.keys.clear();
    }

    /// The viewer on the list as it stands now — codex freezes it on open.
    #[must_use]
    pub fn viewer(&self) -> Viewer {
        Viewer::new(self.entries.clone())
    }
}

/// What a key did to the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Handled,
    Close,
}

/// The F2 page: one warning at a time, its details wrapped and scrolled.
#[derive(Debug)]
pub struct Viewer {
    entries: Vec<Entry>,
    current: usize,
    offset: usize,
    page: usize,
    max_offset: usize,
}

impl Viewer {
    #[must_use]
    pub fn new(entries: Vec<Entry>) -> Self {
        Self {
            entries,
            current: 0,
            offset: 0,
            page: 1,
            max_offset: 0,
        }
    }

    /// Codex `WarningsView::handle_key`: every key is the viewer's; esc, F2
    /// and Ctrl+C close it.
    pub fn key(&mut self, key: KeyEvent) -> Outcome {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc | KeyCode::F(2) => return Outcome::Close,
            KeyCode::Char('c') if control => return Outcome::Close,
            KeyCode::Left | KeyCode::Right => {
                let previous = self.current;
                self.current = if key.code == KeyCode::Left {
                    self.current.saturating_sub(1)
                } else {
                    (self.current + 1).min(self.entries.len().saturating_sub(1))
                };
                if self.current != previous {
                    self.offset = 0;
                }
            }
            KeyCode::Up => self.offset = self.offset.saturating_sub(1),
            KeyCode::Down => self.offset = (self.offset + 1).min(self.max_offset),
            KeyCode::PageUp => self.offset = self.offset.saturating_sub(self.page),
            KeyCode::PageDown => {
                self.offset = self.offset.saturating_add(self.page).min(self.max_offset);
            }
            KeyCode::Home => self.offset = 0,
            KeyCode::End => self.offset = self.max_offset,
            _ => {}
        }
        Outcome::Handled
    }

    /// The page at `width` × at most `max_rows`, recording the page size the
    /// scroll keys move by.
    pub fn lines(&mut self, width: usize, max_rows: usize) -> Vec<Line> {
        let rows = VIEWER_ROWS.min(max_rows);
        let inner = width.saturating_sub(2 * INDENT.len()).max(1);
        let entry = self.entries.get(self.current);
        let title = entry.map_or_else(
            || TITLE.to_string(),
            |entry| {
                format!(
                    "{TITLE}{SEPARATOR}{} of {}{SEPARATOR}{}",
                    self.current + 1,
                    self.entries.len(),
                    entry.source.label()
                )
            },
        );
        let details = entry.map_or(EMPTY, |entry| entry.details.as_str());
        let body: Vec<Line> = details
            .lines()
            .flat_map(|line| wrap_line(&Line::from_text(line), inner, &Span::raw("")))
            .collect();
        let page = rows.saturating_sub(VIEWER_CHROME_ROWS);
        self.page = page.max(1);
        self.max_offset = body.len().saturating_sub(page);
        self.offset = self.offset.min(self.max_offset);
        let mut lines = vec![indented(Line::new(vec![Span::bold(title)])), Line::empty()];
        lines.extend(
            body.into_iter()
                .skip(self.offset)
                .take(page)
                .map(indented),
        );
        lines.resize(rows.saturating_sub(1), Line::empty());
        lines.push(indented(Self::footer(inner)));
        lines.truncate(rows);
        lines.into_iter().map(|line| line.truncated(width)).collect()
    }

    /// Codex `warnings_view_render.rs`: `esc back · ←/→ warning · ↓ scroll`,
    /// each item that does not fit the row left out. zo has no clipboard
    /// road, so codex's `ctrl+o copy` is not offered.
    fn footer(width: usize) -> Line {
        let navigation = format!("{KEY_LEFT}{KEY_JOIN}{KEY_RIGHT}");
        let mut items: Vec<(&str, &str)> = Vec::new();
        for item in [(KEY_ESC, BACK), (navigation.as_str(), WARNING), (KEY_DOWN, SCROLL)] {
            items.push(item);
            if hint_items(&items).width() > width {
                items.pop();
            }
        }
        hint_items(&items)
    }
}

/// Codex `footer.rs::footer_hint_items_line`: `key label` pairs, the key bold,
/// joined by ` · `.
fn hint_items(items: &[(&str, &str)]) -> Line {
    let mut spans = Vec::with_capacity(items.len() * 3);
    for (index, (key, label)) in items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::dim(SEPARATOR));
        }
        spans.push(Span::bold(*key));
        spans.push(Span::dim(format!(" {label}")));
    }
    Line::new(spans)
}

fn indented(line: Line) -> Line {
    line.prefixed(Span::raw(INDENT))
}

#[cfg(test)]
mod tests {
    use super::{Entry, Outcome, Retained, Source, Viewer};
    use crate::tui::ansi::Line;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use runtime::message_stream::SystemLevel;

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn warnings_and_errors_are_kept_once_by_their_words() {
        let mut kept = Retained::default();
        kept.record(SystemLevel::Warn, "quota low");
        kept.start();
        kept.record(SystemLevel::Info, "interrupted");
        kept.record(SystemLevel::Success, "done");
        kept.record(SystemLevel::Housekeeping, "trimmed");
        kept.record(SystemLevel::Warn, "quota low");
        kept.record(SystemLevel::Error, "'/new' is disabled");
        kept.record(SystemLevel::Warn, "   ");
        assert_eq!(kept.count(), 2);
        let viewer = kept.viewer();
        assert_eq!(
            viewer.entries,
            vec![
                Entry {
                    source: Source::Startup,
                    details: "quota low".to_string()
                },
                Entry {
                    source: Source::Error,
                    details: "'/new' is disabled".to_string()
                },
            ]
        );
        kept.clear();
        assert_eq!(kept.count(), 0);
        kept.record(SystemLevel::Warn, "quota low");
        assert_eq!(kept.count(), 1, "a cleared conversation counts afresh");
        assert_eq!(kept.viewer().entries[0].source, Source::Warning);
    }

    /// Codex `warnings_view_tests.rs::warnings_narrow`, on zo's rows.
    #[test]
    fn the_page_names_its_place_and_wraps_the_details() {
        let mut viewer = Viewer::new(vec![
            Entry {
                source: Source::Warning,
                details: "Long diagnostic\nDetails retained in full, including remediation instructions."
                    .to_string(),
            },
            Entry {
                source: Source::Error,
                details: "second".to_string(),
            },
        ]);
        let rows: Vec<String> = viewer.lines(40, 9).iter().map(Line::plain).collect();
        assert_eq!(
            rows,
            vec![
                "  Warnings · 1 of 2 · Warning",
                "",
                "  Long diagnostic",
                "  Details retained in full, including",
                "  remediation instructions.",
                "",
                "",
                "",
                "  esc back · ←/→ warning · ↓ scroll",
            ]
        );
        assert_eq!(viewer.lines(80, 30).len(), 12, "codex's pane is twelve rows");
        assert_eq!(viewer.key(press(KeyCode::Right)), Outcome::Handled);
        let header = viewer.lines(80, 12)[0].plain();
        assert_eq!(header, "  Warnings · 2 of 2 · Error");
        viewer.key(press(KeyCode::Right));
        assert_eq!(viewer.lines(80, 12)[0].plain(), header, "the last page holds");
        viewer.key(press(KeyCode::Left));
        viewer.key(press(KeyCode::Left));
        assert_eq!(viewer.lines(80, 12)[0].plain(), "  Warnings · 1 of 2 · Warning");
        assert_eq!(
            Viewer::new(Vec::new()).lines(80, 12)[..3]
                .iter()
                .map(Line::plain)
                .collect::<Vec<_>>(),
            vec!["  Warnings", "", "  No warnings"]
        );
    }

    #[test]
    fn the_body_scrolls_within_its_page_and_resets_on_a_new_warning() {
        let details: Vec<String> = (1..=20).map(|n| format!("line {n}")).collect();
        let mut viewer = Viewer::new(vec![
            Entry {
                source: Source::Warning,
                details: details.join("\n"),
            },
            Entry {
                source: Source::Warning,
                details: "other".to_string(),
            },
        ]);
        let body = |viewer: &mut Viewer| viewer.lines(40, 12)[2].plain();
        assert_eq!(body(&mut viewer), "  line 1");
        viewer.key(press(KeyCode::Down));
        assert_eq!(body(&mut viewer), "  line 2");
        viewer.key(press(KeyCode::PageDown));
        assert_eq!(body(&mut viewer), "  line 11", "a page is the nine body rows");
        viewer.key(press(KeyCode::End));
        assert_eq!(body(&mut viewer), "  line 12", "the last row stands on the footer");
        viewer.key(press(KeyCode::PageDown));
        assert_eq!(body(&mut viewer), "  line 12");
        viewer.key(press(KeyCode::Up));
        viewer.key(press(KeyCode::PageUp));
        assert_eq!(body(&mut viewer), "  line 2");
        viewer.key(press(KeyCode::Home));
        assert_eq!(body(&mut viewer), "  line 1");
        viewer.key(press(KeyCode::Down));
        viewer.key(press(KeyCode::Right));
        viewer.key(press(KeyCode::Left));
        assert_eq!(body(&mut viewer), "  line 1", "a new warning starts at its top");
    }

    #[test]
    fn esc_f2_and_ctrl_c_close_and_nothing_else_does() {
        let mut viewer = Viewer::new(Vec::new());
        for code in [KeyCode::Enter, KeyCode::Char('q'), KeyCode::Char('?'), KeyCode::Tab] {
            assert_eq!(viewer.key(press(code)), Outcome::Handled, "{code:?}");
        }
        assert_eq!(viewer.key(press(KeyCode::Esc)), Outcome::Close);
        assert_eq!(viewer.key(press(KeyCode::F(2))), Outcome::Close);
        assert_eq!(
            viewer.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Outcome::Close
        );
    }

    #[test]
    fn the_footer_drops_what_does_not_fit() {
        let mut viewer = Viewer::new(Vec::new());
        let footer = |viewer: &mut Viewer, width| {
            viewer.lines(width, 12).last().map(Line::plain).unwrap_or_default()
        };
        assert_eq!(footer(&mut viewer, 30), "  esc back · ←/→ warning");
        assert_eq!(footer(&mut viewer, 23), "  esc back · ↓ scroll");
        assert_eq!(footer(&mut viewer, 12), "  esc back");
    }
}
