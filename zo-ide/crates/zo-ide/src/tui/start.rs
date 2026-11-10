//! The empty conversation's page — where a session starts.
//!
//! Until the first prompt is sent the viewport is this page: a heading, the
//! composer, the row that says where the session runs, the conversations of
//! this folder to continue, and the keys worth knowing. It is the window's
//! own new-conversation page in the terminal's cells, in the same order.
//!
//! The page is a popup to the painter — it covers the rows above the head and
//! gives them back when the first prompt goes, so nothing of the terminal's
//! own scrollback is pushed away to draw it. When the pane is too small, or
//! the rows the painter can give back are too few, [`lines`] answers `None`
//! and the ordinary bottom pane is drawn.
//!
//! # The field keeps the shape the window reads
//!
//! The window reads zo's composer off the screen before it types at it
//! (`zerocode-shell` `composer_empty::observed`): `│› ` then the dim
//! placeholder, the borders on the rows above and below. The page changes
//! where the box stands, never its bytes. Standing in from the left edge is
//! something only a window that says it reads an inset field is shown
//! ([`inset_allowed`]); an older window keeps the field at column 0.

use super::ansi::{Line, Span, Style};
use super::palette;
use super::sessions::{self, SessionRow};
use super::strings;

/// How many conversations the page offers to continue.
pub const RECENT_ROWS: usize = 3;
/// The narrowest pane the page is drawn in.
pub const MIN_COLUMNS: usize = 60;
/// The fewest rows the page is drawn in: the heading, the field, the where
/// row and a blank row above and below.
pub const MIN_ROWS: usize = 9;
/// The page's text column, in from the field's left border.
const TEXT_INDENT: usize = 2;

/// A window that reads an inset composer says so in its panes' environment.
pub const READS_INSET_ENV: &str = zerocode_core::hook::READS_INSET_COMPOSER_ENV;
/// A pane of the window carries its key; a terminal of the person's own has
/// none, and nothing there reads the screen.
const PANE_KEY_ENV: &str = zerocode_core::hook::PANE_KEY_ENV;

/// Whether the field may stand in from the pane's left edge: always outside
/// the window, and inside it only when the window says it reads that.
#[must_use]
pub fn inset_allowed(pane_key: Option<&str>, reads_inset: Option<&str>) -> bool {
    let in_a_pane = pane_key.is_some_and(|key| !key.trim().is_empty());
    !in_a_pane || reads_inset.is_some_and(|word| word.trim() == "1")
}

/// [`inset_allowed`] for this process.
#[must_use]
pub fn inset_allowed_here() -> bool {
    inset_allowed(
        std::env::var(PANE_KEY_ENV).ok().as_deref(),
        std::env::var(READS_INSET_ENV).ok().as_deref(),
    )
}

/// The empty conversation's page: what it lists and which row is chosen.
#[derive(Debug, Clone, Default)]
pub struct StartPage {
    recent: Vec<SessionRow>,
    selected: Option<usize>,
    reference_millis: u128,
    inset: bool,
}

impl StartPage {
    #[must_use]
    pub fn new(inset: bool) -> Self {
        Self { inset, ..Self::default() }
    }

    /// The folder's conversations, newest first; the page keeps the first
    /// [`RECENT_ROWS`].
    pub fn set_recent(&mut self, mut rows: Vec<SessionRow>, reference_millis: u128) {
        rows.truncate(RECENT_ROWS);
        self.recent = rows;
        self.reference_millis = reference_millis;
        self.selected = self.selected.filter(|index| *index < self.recent.len());
    }

    #[must_use]
    pub fn is_choosing(&self) -> bool {
        self.selected.is_some()
    }

    /// The chosen conversation's id.
    #[must_use]
    pub fn selection(&self) -> Option<&str> {
        self.selected
            .and_then(|index| self.recent.get(index))
            .map(|row| row.id.as_str())
    }

    /// ↓: into the list, then down it. `false` when there is nothing to choose.
    pub fn down(&mut self) -> bool {
        if self.recent.is_empty() {
            return false;
        }
        self.selected = Some(match self.selected {
            None => 0,
            Some(index) => (index + 1).min(self.recent.len() - 1),
        });
        true
    }

    /// ↑: up the list, and from its first row back to the field. `false`
    /// when the field already has the keys.
    pub fn up(&mut self) -> bool {
        match self.selected {
            None => false,
            Some(0) => {
                self.selected = None;
                true
            }
            Some(index) => {
                self.selected = Some(index - 1);
                true
            }
        }
    }

    /// Back to the field.
    pub fn leave_list(&mut self) {
        self.selected = None;
    }

    /// The page's column in a pane `width` wide: where it starts and how wide
    /// it is. An inset column is the reading width, centred; otherwise the
    /// field keeps the pane's left edge and whole width, as the window reads
    /// it there.
    #[must_use]
    pub fn column(&self, width: usize) -> (usize, usize) {
        if !self.inset {
            return (0, width);
        }
        let column = super::style::current().reading_width(width);
        ((width - column) / 2, column)
    }
}

/// What the page is drawn from, already laid out for its column.
pub struct Parts<'a> {
    /// The pane's width.
    pub width: usize,
    /// The rows the painter can give the page.
    pub rows: usize,
    /// The composer at the column's width, and its caret inside it.
    pub composer: Vec<Line>,
    pub caret: (u16, u16),
    /// The suggestion list under the composer while one is open; it takes
    /// the place of the page's own rows there.
    pub popup: Option<Vec<Line>>,
    /// Where the session runs: the permission mark, the model, the folder.
    pub where_row: Line,
    /// What was said before the first frame — a startup warning. Drawn last,
    /// in the rows the page has left.
    pub notes: &'a [Line],
    pub version: &'static str,
}

/// Rows to draw and where the caret stands on them, when it shows.
pub type Drawn = (Vec<Line>, Option<(u16, u16)>);

/// The page's rows and the caret, or `None` when the pane cannot hold it.
#[must_use]
pub fn lines(page: &StartPage, parts: Parts<'_>) -> Option<Drawn> {
    let (left, column) = page.column(parts.width);
    if parts.width < MIN_COLUMNS || parts.rows < MIN_ROWS {
        return None;
    }
    let heading = heading_row(column, parts.version);
    let field_rows = parts.composer.len();
    // The rows the page stands on when nothing is open: they decide where the
    // field is, so a suggestion list opening under it moves nothing.
    let mut recent = recent_rows(page, column);
    let mut tips = vec![Line::empty(), tips_row()];
    let fixed = 2 + field_rows + 1;
    if fixed + recent.len() + tips.len() + 2 > parts.rows {
        tips.clear();
    }
    if fixed + recent.len() + tips.len() + 2 > parts.rows {
        recent.clear();
    }
    let standing = fixed + recent.len() + tips.len();
    if standing + 2 > parts.rows {
        return None;
    }
    let top = ((parts.rows - standing) / 2).saturating_sub(1).max(1);

    let mut rows: Vec<Line> = vec![Line::empty(); top];
    rows.push(heading);
    rows.push(Line::empty());
    let field_at = rows.len();
    rows.extend(parts.composer);
    if let Some(popup) = parts.popup {
        rows.extend(popup);
    } else {
        rows.push(parts.where_row.truncated(column));
        rows.extend(recent);
        rows.extend(tips);
        rows.extend(parts.notes.iter().map(|note| note.clone().truncated(column)));
    }
    rows.truncate(parts.rows);
    rows.resize(parts.rows, Line::empty());
    let rows = if left == 0 {
        rows
    } else {
        rows.into_iter()
            .map(|row| if row == Line::empty() { row } else { row.prefixed(Span::raw(" ".repeat(left))) })
            .collect()
    };
    let caret = (!page.is_choosing()).then(|| {
        let (col, row) = parts.caret;
        (
            col + u16::try_from(left).unwrap_or(0),
            row + u16::try_from(field_at).unwrap_or(0),
        )
    });
    Some((rows, caret))
}

fn heading_row(column: usize, version: &str) -> Line {
    let mut spans = vec![
        Span::raw(" ".repeat(TEXT_INDENT)),
        Span::new("✻ ", Style::new().fg(palette::COMPOSER_CARET).bold()),
        Span::bold(strings::START_HEADING),
    ];
    let tail = format!("{} {version}", crate::status_format::PRODUCT);
    let used: usize = spans.iter().map(Span::width).sum();
    let tail_width = unicode_width::UnicodeWidthStr::width(tail.as_str());
    if used + tail_width + 2 * TEXT_INDENT <= column {
        spans.push(Span::raw(" ".repeat(column - used - tail_width - TEXT_INDENT)));
        spans.push(Span::dim(tail));
    }
    Line::new(spans)
}

/// The conversations to continue: a blank row, their head, a row each. Empty
/// when the folder has none.
fn recent_rows(page: &StartPage, column: usize) -> Vec<Line> {
    if page.recent.is_empty() {
        return Vec::new();
    }
    let mut rows = vec![Line::empty()];
    let mut head = vec![
        Span::raw(" ".repeat(TEXT_INDENT)),
        Span::bold(strings::START_CONTINUE),
    ];
    let keys = [
        Span::bold("↓"),
        Span::dim(" choose · "),
        Span::bold("enter"),
        Span::dim(" resume · "),
        Span::bold("/resume"),
        Span::dim(" all"),
    ];
    let used: usize = head.iter().map(Span::width).sum();
    let keys_width: usize = keys.iter().map(Span::width).sum();
    if used + keys_width + 2 * TEXT_INDENT <= column {
        head.push(Span::raw(" ".repeat(column - used - keys_width - TEXT_INDENT)));
        head.extend(keys);
    }
    rows.push(Line::new(head));
    for (index, row) in page.recent.iter().enumerate() {
        rows.push(recent_row(row, page.selected == Some(index), page.reference_millis, column));
    }
    rows
}

fn recent_row(row: &SessionRow, selected: bool, reference_millis: u128, column: usize) -> Line {
    let age = sessions::relative_time(reference_millis, row.updated_millis);
    let age_width = unicode_width::UnicodeWidthStr::width(age.as_str());
    let title_room = column.saturating_sub(TEXT_INDENT + age_width + 2 + TEXT_INDENT);
    let held = if row.held_by.is_some() { "● " } else { "" };
    let title = Line::new(vec![Span::raw(format!("{held}{}", row.preview))]).truncated(title_room);
    let title_width = title.width();
    let text_style = if selected { Style::new().bold() } else { Style::new() };
    let mut spans = if selected {
        vec![Span::new("❯ ", Style::new().fg(palette::COMPOSER_CARET).bold())]
    } else {
        vec![Span::raw(" ".repeat(TEXT_INDENT))]
    };
    spans.push(Span::new(title.plain(), text_style));
    spans.push(Span::raw(" ".repeat(
        column.saturating_sub(TEXT_INDENT + title_width + age_width + TEXT_INDENT),
    )));
    spans.push(Span::dim(age));
    let mut line = Line::new(spans);
    if selected {
        if let Some(terminal_palette) = palette::terminal_palette() {
            let pad = column.saturating_sub(line.width());
            line.spans.push(Span::raw(" ".repeat(pad)));
            let band = Style::new().bg(palette::row_background(terminal_palette, true));
            for span in &mut line.spans {
                span.style = span.style.patch(band);
            }
        }
    }
    line
}

/// The keys worth knowing before the first prompt.
fn tips_row() -> Line {
    let mut spans = vec![Span::raw(" ".repeat(TEXT_INDENT))];
    for (index, (key, words)) in strings::START_TIPS.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("     "));
        }
        spans.push(Span::bold(*key));
        spans.push(Span::dim(format!(" {words}")));
    }
    Line::new(spans)
}

#[cfg(test)]
mod tests {
    use super::{inset_allowed, lines, Parts, StartPage, MIN_ROWS};
    use crate::tui::ansi::{Line, Span};
    use crate::tui::sessions::SessionRow;

    fn row(id: &str, title: &str, updated: u128) -> SessionRow {
        SessionRow {
            name: None,
            id: id.to_string(),
            cwd: None,
            preview: title.to_string(),
            created_millis: updated,
            updated_millis: updated,
            held_by: None,
        }
    }

    fn field(width: usize) -> Vec<Line> {
        vec![
            Line::from_text(format!("╭{}╮", "─".repeat(width - 2))),
            Line::new(vec![Span::raw("│› "), Span::dim("Ask zo to do anything")]),
            Line::from_text(format!("╰{}╯", "─".repeat(width - 2))),
        ]
    }

    fn parts(width: usize, rows: usize, column: usize) -> Parts<'static> {
        Parts {
            width,
            rows,
            composer: field(column),
            caret: (3, 1),
            popup: None,
            where_row: Line::from_text("  ⏵⏵ Full Access   model high · ~/work/acme-api"),
            notes: &[],
            version: "1.1.139",
        }
    }

    fn page(inset: bool, count: usize) -> StartPage {
        let mut page = StartPage::new(inset);
        page.set_recent(
            (0..count).map(|n| row(&format!("s{n}"), &format!("conversation {n}"), 1_000)).collect(),
            61_000,
        );
        page
    }

    #[test]
    fn an_older_window_keeps_the_field_at_the_left_edge() {
        assert!(inset_allowed(None, None), "a terminal of the person's own");
        assert!(inset_allowed(Some(""), None));
        assert!(!inset_allowed(Some("pane-7"), None), "a window that does not say it reads an inset field");
        assert!(!inset_allowed(Some("pane-7"), Some("0")));
        assert!(inset_allowed(Some("pane-7"), Some("1")));
    }

    #[test]
    fn the_field_stands_in_the_middle_with_the_list_under_it() {
        let page = page(false, 3);
        let (rows, caret) = lines(&page, parts(100, 30, 100)).expect("the page fits");
        assert_eq!(rows.len(), 30, "the page is as tall as the rows it was given");
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        let field = plain.iter().position(|row| row.starts_with("│› ")).expect("the field's row");
        assert!(plain[field - 1].starts_with('╭') && plain[field + 1].starts_with('╰'));
        assert!((8..=14).contains(&field), "about the middle: row {field}");
        assert_eq!(caret, Some((3, u16::try_from(field).unwrap())));
        assert!(plain[field - 3].contains("What should we do?"));
        assert!(plain[field + 2].contains("Full Access"));
        let head = plain.iter().position(|row| row.contains("Continue a conversation")).expect("the list's head");
        assert!(head > field);
        assert!(plain[head + 1].contains("conversation 0") && plain[head + 1].ends_with("1m ago"));
        assert_eq!(rows[head + 1].width(), 98, "the age ends two columns in from the column's edge");
        assert!(plain[head + 3].contains("conversation 2"));
        assert!(plain.iter().any(|row| row.contains("/ commands")));
    }

    #[test]
    fn an_inset_column_is_centred_and_carries_the_caret_with_it() {
        // The reading width is the look's, and the look is fixed once a process.
        if !crate::tui::style::in_fresh_run(
            "tui::start::tests::an_inset_column_is_centred_and_carries_the_caret_with_it",
        ) {
            return;
        }
        crate::tui::style::init(&crate::preferences::Preferences::default());
        let page = page(true, 0);
        assert_eq!(page.column(201), (50, 100));
        assert_eq!(page.column(80), (0, 80));
        let (rows, caret) = lines(&page, parts(201, 40, 100)).expect("the page fits");
        let field = rows.iter().position(|row| row.plain().trim_start().starts_with("│› ")).unwrap();
        assert!(rows[field].plain().starts_with(&" ".repeat(50)));
        assert_eq!(caret, Some((53, u16::try_from(field).unwrap())));
        assert!(rows.iter().all(|row| row.width() <= 150));
    }

    #[test]
    fn down_goes_into_the_list_and_up_comes_back_to_the_field() {
        let mut page = page(false, 2);
        assert!(!page.up(), "the field has the keys");
        assert!(page.down());
        assert_eq!(page.selection(), Some("s0"));
        let (rows, caret) = lines(&page, parts(100, 30, 100)).unwrap();
        assert_eq!(caret, None, "no caret in the field while a row is chosen");
        assert!(rows.iter().any(|row| row.plain().starts_with("❯ conversation 0")));
        assert!(page.down());
        assert!(page.down(), "the last row holds");
        assert_eq!(page.selection(), Some("s1"));
        assert!(page.up());
        assert!(page.up());
        assert_eq!(page.selection(), None);
        assert!(!StartPage::new(false).down(), "nothing to choose in a folder with no conversations");
    }

    #[test]
    fn a_short_pane_drops_the_keys_then_the_list_then_the_page() {
        let page = page(false, 3);
        let tall = lines(&page, parts(100, 30, 100)).unwrap().0;
        assert!(tall.iter().any(|row| row.plain().contains("/ commands")));
        let shorter = lines(&page, parts(100, 14, 100)).unwrap().0;
        assert!(!shorter.iter().any(|row| row.plain().contains("/ commands")));
        assert!(shorter.iter().any(|row| row.plain().contains("Continue a conversation")));
        let short = lines(&page, parts(100, 10, 100)).unwrap().0;
        assert!(!short.iter().any(|row| row.plain().contains("Continue a conversation")));
        assert!(short.iter().any(|row| row.plain().starts_with("│› ")));
        assert!(lines(&page, parts(100, MIN_ROWS - 1, 100)).is_none());
        assert!(lines(&page, parts(40, 30, 40)).is_none(), "too narrow");
    }

    #[test]
    fn a_suggestion_list_takes_the_rows_under_the_field_and_moves_nothing() {
        let page = page(false, 3);
        let standing = lines(&page, parts(100, 30, 100)).unwrap();
        let mut open = parts(100, 30, 100);
        open.popup = Some(vec![Line::empty(), Line::from_text("  /model   choose a model")]);
        let (rows, caret) = lines(&page, open).unwrap();
        assert_eq!(caret, standing.1, "the field stays where it stood");
        assert!(rows.iter().any(|row| row.plain().contains("/model   choose a model")));
        assert!(!rows.iter().any(|row| row.plain().contains("Continue a conversation")));
        assert_eq!(rows.len(), 30);
    }
}
