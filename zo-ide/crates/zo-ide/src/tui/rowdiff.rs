//! 행 차분 — 바뀐 칸 구간만 다시 쓴다.
//!
//! [`super::painter::Painter::paint`] 는 바뀐 행을 통째로 다시 쓴다: `ESC[0m ESC[K`,
//! 스팬들, `ESC[0m`. 그런데 한 프레임에 화면에서 바뀌는 것은 대개 한 행의 몇 칸이다
//! — shimmer 가 한 칸 옮겨 갈 때 상태 줄의 서너 글자 색, 도는 도구 셀의 머리 행에서는
//! 불릿 한 칸의 색, 글자를 칠 때는 컴포저의 글자 하나. 실측(설치본 1.1.42, 2026-09-30):
//! 기다리는 동안 프레임마다 행 하나(258 B)에 바뀐 칸 3.3개, 도구가 도는 동안은 구문
//! 강조된 명령 행(약 700 B)을 불릿 한 칸 때문에 다시 썼다.
//!
//! 여기서는 행을 **칸의 나열**로 본다. 칸은 글자·너비·스타일이다. 직전에 이 painter 가
//! 쓴 행의 칸들과 새 행의 칸들을 견주어, 다른 칸의 구간만 찾아가(CUP) 쓴다. 눈에 보이는
//! 화면은 행을 통째로 다시 썼을 때와 **같아야** 한다 — 이 모듈의 계약은 그것 하나고,
//! 시험이 두 화면을 칸 단위로 견준다(`test_screen`).
//!
//! 칸으로 나눌 수 없는 행(제어 문자, 너비 0인 글자, 화면보다 넓은 행)은 [`RowCells::fill`]
//! 이 거절하고, painter 는 그 행을 예전처럼 통째로 쓴다. 넓은 글자의 반쪽만 바뀐 구간은
//! 글자 전체로 넓혀 쓴다.

use super::ansi::{char_width, Line, Style, StyleWriter};
use super::painter::cup;

/// 한 글자가 차지한 자리.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// 한 칸짜리 글자.
    Narrow,
    /// 넓은 글자의 앞 칸.
    Head,
    /// 넓은 글자의 뒤 칸 — 글자는 앞 칸이 쓴다.
    Tail,
}

/// 한 칸이 화면에 만드는 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cell {
    ch: char,
    style: Style,
    part: Part,
}

/// 쓴 적 없는 칸이자 지워진 칸 — 기본 스타일의 공백.
const BLANK: Cell = Cell { ch: ' ', style: Style::new(), part: Part::Narrow };

/// 빈칸 몇 개 이상이면 하나씩 쓰지 않고 지우기(`ESC[K`)로 지운다.
const ERASE_FROM: usize = 4;
/// 바뀐 두 구간 사이의 안 바뀐 칸이 이만큼 이하일 때만 이어 쓰는 것을 따져 본다.
const MERGE_REACH: usize = 6;

/// 터미널마다 폭이 같은 글자인가.
///
/// 바뀐 칸을 찾아가려면 우리가 센 열이 터미널이 센 열이어야 한다. 행을 통째로 쓸 때는
/// 그럴 필요가 없었다 — 터미널이 제 폭으로 차례로 놓을 뿐이라 폭이 어긋나도 오른쪽
/// 가장자리만 틀어진다. 이모지·기호·결합 문자는 터미널과 유니코드 판에 따라 한 칸이기도
/// 두 칸이기도 하다(같은 글자를 xterm.js 의 6판 표는 한 칸으로, `unicode-width` 는 두
/// 칸으로 센다). 그런 글자가 든 행은 칸으로 나누지 않고 예전처럼 통째로 쓴다. 남기는 것은
/// 어느 표에서나 같은 글자다: ASCII·라틴·그리스·키릴, 문장부호, 화살표, 상자 그림과
/// 도형, 그리고 폭 2가 늘 2인 한글·한자·가나·전각.
const fn stable_width(ch: char) -> bool {
    matches!(
        ch as u32,
        0x20..=0x7e
            | 0xa0..=0x24f
            | 0x370..=0x52f
            | 0x2010..=0x2027
            | 0x2030..=0x205e
            | 0x2190..=0x21ff
            | 0x2500..=0x25ff
            | 0x3000..=0x30ff
            | 0x4e00..=0x9fff
            | 0xac00..=0xd7a3
            | 0xff01..=0xff60
            | 0xffe0..=0xffe6
    )
}

/// 눈에 보이는 스타일. 공백에서는 글자색·굵기·희미함·기울임이 안 보이므로 떼어
/// 둔다 — "희미한 공백"과 "그냥 공백"이 같은 칸이 되어야 다시 쓰지 않는다.
fn seen(ch: char, style: Style) -> Style {
    if ch == ' ' {
        Style { fg: None, bold: false, dim: false, italic: false, ..style }
    } else {
        style
    }
}

/// 한 행이 화면에서 만드는 칸들 — 언제나 화면 폭만큼, 오른쪽은 빈칸으로 채운다.
#[derive(Debug, Clone, Default)]
pub(super) struct RowCells {
    cells: Vec<Cell>,
}

impl RowCells {
    /// `cols` 칸 전부 빈칸인 행 — 스크롤이 새로 드러낸 행이 이렇다.
    pub(super) fn blank(cols: usize) -> Self {
        Self { cells: vec![BLANK; cols] }
    }

    /// 칸의 수 — 화면 폭이 바뀌었는데 옛 행이 남았다면 견주지 않는다.
    pub(super) fn width(&self) -> usize {
        self.cells.len()
    }

    /// `line` 이 `cols` 폭 화면에서 만드는 칸들로 채운다(`color` 가 꺼져 있으면 스타일
    /// 없이). 칸으로 나눌 수 없으면 — 제어 문자, 터미널마다 폭이 다를 수 있는 글자
    /// ([`stable_width`]), 화면보다 넓은 행 — 거짓이고 내용은 쓸 수 없다. 그 행은 통째로 쓴다.
    pub(super) fn fill(&mut self, line: &Line, cols: usize, color: bool) -> bool {
        self.cells.clear();
        for span in &line.spans {
            let style = if color { span.style.patch(line.style) } else { Style::new() };
            for ch in span.text.chars() {
                if !stable_width(ch) {
                    return false;
                }
                let used = self.cells.len();
                let style = seen(ch, style);
                match char_width(ch) {
                    1 if used < cols => self.cells.push(Cell { ch, style, part: Part::Narrow }),
                    2 if used + 2 <= cols => {
                        self.cells.push(Cell { ch, style, part: Part::Head });
                        self.cells.push(Cell { ch, style, part: Part::Tail });
                    }
                    _ => return false,
                }
            }
        }
        self.cells.resize(cols, BLANK);
        true
    }
}

/// 새 행에서 다시 써야 하는 칸 구간 `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Run {
    start: usize,
    end: usize,
}

/// 두 행에서 다른 칸의 구간들. 넓은 글자는 반쪽만 다르더라도 글자 전체로 넓힌다 —
/// 터미널은 한 칸만 덮어쓰면 남은 반쪽을 비운다.
fn changed_runs(old: &[Cell], new: &[Cell], runs: &mut Vec<Run>) {
    runs.clear();
    let cols = old.len().min(new.len());
    let mut col = 0;
    while col < cols {
        if old[col] == new[col] {
            col += 1;
            continue;
        }
        let mut start = col;
        let mut end = col;
        while end < cols && old[end] != new[end] {
            end += 1;
        }
        while start > 0 && (new[start].part == Part::Tail || old[start].part == Part::Tail) {
            start -= 1;
        }
        while end < cols && (new[end].part == Part::Tail || old[end].part == Part::Tail) {
            end += 1;
        }
        match runs.last_mut() {
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => runs.push(Run { start, end }),
        }
        col = end;
    }
}

/// 터미널의 커서를 `(col, row)` 로 — 이미 거기 있다고 알면 아무것도 안 낸다.
fn move_to(out: &mut String, at: &mut Option<(u16, u16)>, col: usize, row: u16) {
    let col = u16::try_from(col).unwrap_or(u16::MAX);
    if *at != Some((col, row)) {
        cup(out, col, row);
    }
    *at = Some((col, row));
}

/// 한 칸의 글자를, 필요한 만큼만 스타일을 바꾸고 쓴다. 공백은 지금 켜 둔 스타일로
/// 써도 같아 보이면 스타일을 건드리지 않는다.
fn write_cell(out: &mut String, writer: &mut StyleWriter, cell: Cell) {
    if cell.ch != ' ' || !writer.blank_looks_alike(cell.style) {
        writer.transition(cell.style, out);
    }
    out.push(cell.ch);
}

/// `runs` 를 새 행 `cells` 대로 쓴다. 터미널의 커서가 어디 있다고 아는지는 `at`
/// 에 두고 오가며, 쓴 뒤의 자리를 알면 채워 둔다(오른쪽 끝 칸을 쓴 뒤는 다음 글자가
/// 줄을 넘기는 자리라 모른다고 친다).
fn emit(
    out: &mut String,
    writer: &mut StyleWriter,
    at: &mut Option<(u16, u16)>,
    row: u16,
    cells: &[Cell],
    runs: &[Run],
) {
    let cols = cells.len();
    // 여기부터 오른쪽은 새 행이 전부 빈칸이다.
    let content_end = cells.iter().rposition(|cell| *cell != BLANK).map_or(0, |last| last + 1);
    for run in runs {
        move_to(out, at, run.start, row);
        let glyph_end = run.end.min(content_end).max(run.start);
        for cell in &cells[run.start..glyph_end] {
            if cell.part != Part::Tail {
                write_cell(out, writer, *cell);
            }
        }
        let mut end = glyph_end;
        if glyph_end < run.end {
            // 바뀐 칸이 새 행의 빈 꼬리까지 닿는다. 지우기는 커서에서 줄 끝까지 지우지만,
            // 이 구간 오른쪽의 칸은 옛 행도 새 행과 같은 빈칸이다(다르면 이 구간에
            // 들었을 것이다).
            let width = run.end - glyph_end;
            if !writer.leaves_blank() {
                writer.transition(Style::new(), out);
            }
            if width >= ERASE_FROM {
                out.push_str("\u{1b}[K");
            } else {
                out.extend(std::iter::repeat_n(' ', width));
                end = run.end;
            }
        }
        *at = u16::try_from(end).ok().filter(|_| end < cols).map(|col| (col, row));
    }
}

/// 사이가 가까운 두 구간은, 사이의 안 바뀐 칸을 다시 쓰는 편이 새로 찾아가는 것(CUP)
/// 보다 짧으면 하나로 잇는다. 실제로 써 본 길이로 견준다.
fn merge_by_cost(
    runs: &mut Vec<Run>,
    writer: &StyleWriter,
    at: Option<(u16, u16)>,
    row: u16,
    cells: &[Cell],
) {
    let cost = |set: &[Run]| {
        let mut trial = String::new();
        let mut pen = writer.clone();
        let mut place = at;
        emit(&mut trial, &mut pen, &mut place, row, cells, set);
        trial.len()
    };
    let mut index = 0;
    while index + 1 < runs.len() {
        let (first, second) = (runs[index], runs[index + 1]);
        if second.start - first.end <= MERGE_REACH {
            let joined = Run { start: first.start, end: second.end };
            if cost(&[joined]) <= cost(&[first, second]) {
                runs[index] = joined;
                runs.remove(index + 1);
                continue;
            }
        }
        index += 1;
    }
}

/// `old` 이 화면에 있을 때 `new` 로 바꾸는 바이트를 `out` 에 붙인다. `writer` 는 터미널이
/// 켜 둔 스타일을, `at` 는 커서가 있다고 아는 자리를 따라간다. 바뀐 칸이 없으면 아무것도
/// 안 낸다. `runs` 는 쓸 자리를 비워 두는 작업 공간이다.
pub(super) fn write_diff(
    out: &mut String,
    writer: &mut StyleWriter,
    at: &mut Option<(u16, u16)>,
    row: u16,
    old: &RowCells,
    new: &RowCells,
    runs: &mut Vec<Run>,
) {
    changed_runs(&old.cells, &new.cells, runs);
    if runs.is_empty() {
        return;
    }
    merge_by_cost(runs, writer, *at, row, &new.cells);
    emit(out, writer, at, row, &new.cells, runs);
}

#[cfg(test)]
mod tests {
    use super::{changed_runs, write_diff, Cell, Part, RowCells, Run, BLANK};
    use crate::tui::ansi::{write_spans, Color, Line, Span, Style, StyleWriter, RESET};
    use crate::tui::painter::cup;
    use crate::tui::test_screen::{mutate, random_line, Lcg, TestScreen};

    const COLS: u16 = 40;

    fn cells(line: &Line) -> RowCells {
        let mut row = RowCells::default();
        assert!(row.fill(line, usize::from(COLS), true), "{:?}", line.plain());
        row
    }

    fn text(text: &str) -> Line {
        Line::from_text(text)
    }

    fn runs_of(old: &Line, new: &Line) -> Vec<(usize, usize)> {
        let mut runs: Vec<Run> = Vec::new();
        changed_runs(&cells(old).cells, &cells(new).cells, &mut runs);
        runs.iter().map(|run| (run.start, run.end)).collect()
    }

    /// The bytes the painter has always written for a row: a move, a reset, an erase,
    /// the spans, a reset.
    fn whole_row(line: &Line, row: u16) -> String {
        let mut out = String::new();
        cup(&mut out, 0, row);
        out.push_str("\u{1b}[0m\u{1b}[K");
        write_spans(line, &mut out);
        out.push_str(RESET);
        out
    }

    /// The bytes that change `old` into `new` on `row`, from a terminal whose pen is plain.
    fn diff_bytes(old: &Line, new: &Line, row: u16) -> String {
        let mut out = String::new();
        let mut writer = StyleWriter::new();
        let mut at = None;
        let mut runs = Vec::new();
        write_diff(&mut out, &mut writer, &mut at, row, &cells(old), &cells(new), &mut runs);
        if !writer.is_plain() {
            out.push_str(RESET);
        }
        out
    }

    /// `old` is on the screen (written whole), then `new` arrives as a diff: the screen
    /// must look like a screen that only ever had `new`, written whole.
    fn assert_same_look(old: &Line, new: &Line) {
        let mut through_diff = TestScreen::new(COLS, 4);
        through_diff.feed(&whole_row(old, 1));
        through_diff.feed(&diff_bytes(old, new, 1));
        let mut whole = TestScreen::new(COLS, 4);
        whole.feed(&whole_row(new, 1));
        assert_eq!(
            through_diff.looks(1),
            whole.looks(1),
            "old {:?} new {:?}",
            old.plain(),
            new.plain()
        );
    }

    fn rgb(r: u8, g: u8, b: u8) -> Style {
        Style::new().fg(Color::Rgb(r, g, b)).bold()
    }

    #[test]
    fn equal_rows_have_no_runs() {
        assert!(runs_of(&text("same"), &text("same")).is_empty());
    }

    #[test]
    fn one_changed_glyph_is_one_column() {
        assert_eq!(runs_of(&text("abcdef"), &text("abXdef")), vec![(2, 3)]);
    }

    #[test]
    fn a_row_that_shrinks_changes_up_to_where_it_used_to_end() {
        assert_eq!(runs_of(&text("abcdef"), &text("abc")), vec![(3, 6)]);
        assert_eq!(runs_of(&text("abc"), &text("abcdef")), vec![(3, 6)]);
    }

    #[test]
    fn half_a_wide_glyph_changing_rewrites_the_glyph() {
        // "한" is two columns; the same style and glyph on the left, a different glyph
        // on the right: both halves belong to one glyph.
        assert_eq!(runs_of(&text("a한b"), &text("a글b")), vec![(1, 3)]);
        // Two narrow glyphs where a wide one stood.
        assert_eq!(runs_of(&text("a한b"), &text("axyb")), vec![(1, 3)]);
        assert_eq!(runs_of(&text("axyb"), &text("a한b")), vec![(1, 3)]);
    }

    #[test]
    fn touching_runs_are_one_run() {
        assert_eq!(runs_of(&text("abcdef"), &text("XYcdeZ")), vec![(0, 2), (5, 6)]);
        assert_eq!(runs_of(&text("abcdef"), &text("XYZdef")), vec![(0, 3)]);
    }

    #[test]
    fn a_space_that_only_changes_what_cannot_be_seen_is_not_a_change() {
        let dim_gap = Line::new(vec![Span::raw("a"), Span::dim(" "), Span::raw("b")]);
        assert!(runs_of(&dim_gap, &text("a b")).is_empty());
        // A background can be seen on a space.
        let tinted = Line::new(vec![
            Span::raw("a"),
            Span::new(" ", Style::new().bg(Color::Rgb(9, 9, 9))),
            Span::raw("b"),
        ]);
        assert_eq!(runs_of(&tinted, &text("a b")), vec![(1, 2)]);
    }

    #[test]
    fn glyphs_whose_width_depends_on_the_terminal_keep_their_row_whole() {
        let mut row = RowCells::default();
        for risky in ["🚀 launch", "warn ⚠", "done ✓", "⚡ fast", "a\u{200d}b", "flag 🇰🇷", "e\u{301}"] {
            assert!(!row.fill(&text(risky), 40, true), "{risky:?}");
        }
    }

    #[test]
    fn glyphs_that_are_the_same_width_everywhere_are_cells() {
        let mut row = RowCells::default();
        for steady in ["plain ascii ~", "café ñ ×·°", "привет мир", "αβγ", "• … — ›", "← → ↑ ↓", "╭──╮│ ●▶", "한글 日本語 ＡＢ", "ｈｉ　"] {
            assert!(row.fill(&text(steady), 40, true), "{steady:?}");
        }
    }

    #[test]
    fn a_line_that_cannot_be_cells_is_refused() {
        let mut row = RowCells::default();
        assert!(!row.fill(&text("tab\there"), 40, true), "a control character");
        assert!(!row.fill(&text("e\u{301}"), 40, true), "a combining mark has no cell of its own");
        assert!(!row.fill(&text(&"x".repeat(41)), 40, true), "wider than the screen");
        assert!(!row.fill(&text(&format!("{}한", "x".repeat(39))), 40, true), "a wide glyph on the last column");
        assert!(row.fill(&text(&format!("{}한", "x".repeat(38))), 40, true));
        assert!(row.fill(&text(&"x".repeat(40)), 40, true));
    }

    #[test]
    fn a_row_is_as_wide_as_the_screen() {
        let row = cells(&text("hi"));
        assert_eq!(row.cells.len(), usize::from(COLS));
        assert_eq!(row.cells[2], BLANK);
        assert!(RowCells::blank(3).cells.iter().all(|cell| *cell == BLANK));
        let wide = cells(&text("한"));
        assert_eq!((wide.cells[0].part, wide.cells[1].part), (Part::Head, Part::Tail));
    }

    #[test]
    fn no_color_has_no_styles_to_compare() {
        let mut row = RowCells::default();
        assert!(row.fill(&Line::new(vec![Span::bold("x"), Span::dim("y")]), 10, false));
        assert!(row.cells[..2].iter().all(|cell| cell.style == Style::new()));
    }

    #[test]
    fn one_glyph_costs_a_move_and_a_glyph() {
        let bytes = diff_bytes(&text("abcdef"), &text("abXdef"), 3);
        assert_eq!(bytes, "\u{1b}[4;3HX");
    }

    #[test]
    fn a_colour_change_costs_a_colour_and_a_glyph() {
        let old = Line::new(vec![Span::new("W", rgb(100, 100, 100)), Span::raw("orking")]);
        let new = Line::new(vec![Span::new("W", rgb(200, 200, 200)), Span::raw("orking")]);
        let bytes = diff_bytes(&old, &new, 0);
        assert_eq!(bytes, "\u{1b}[1;1H\u{1b}[1m\u{1b}[38;2;200;200;200;49mW\u{1b}[0m");
        assert!(bytes.len() < whole_row(&new, 0).len());
    }

    #[test]
    fn a_shorter_row_erases_its_old_tail() {
        let bytes = diff_bytes(&text("abcdefghij"), &text("abc"), 0);
        assert_eq!(bytes, "\u{1b}[1;4H\u{1b}[K");
        // A short tail is cheaper as spaces than as an erase.
        assert_eq!(diff_bytes(&text("abcde"), &text("abc"), 0), "\u{1b}[1;4H  ");
    }

    #[test]
    fn an_erase_is_not_painted_with_a_background_that_is_on() {
        let tinted = Style::new().bg(Color::Rgb(30, 30, 30));
        let old = Line::new(vec![Span::new("abcdefgh", tinted)]);
        let new = Line::new(vec![Span::new("Xbc", tinted)]);
        let bytes = diff_bytes(&old, &new, 0);
        // The glyph was written with the tint on; back to the plain pen before the
        // erase, or the erase would paint the rest of the row with the tint.
        assert!(bytes.contains("\u{1b}[39;49m\u{1b}[K"), "{bytes:?}");
        assert_same_look(&old, &new);
        // With the pen already plain the erase goes out as it is.
        let shorter = Line::new(vec![Span::new("abc", tinted)]);
        assert_eq!(diff_bytes(&old, &shorter, 0), "\u{1b}[1;4H\u{1b}[K");
        assert_same_look(&old, &shorter);
    }

    #[test]
    fn two_changes_a_few_columns_apart_are_one_write_when_that_is_shorter() {
        let bytes = diff_bytes(&text("abcdefgh"), &text("Xbcdefgh").with_tail("Y"), 0);
        assert!(bytes.matches("\u{1b}[").count() <= 2, "{bytes:?}");
        let far = diff_bytes(&text(&"a".repeat(30)), &text(&format!("X{}Y", "a".repeat(28))), 0);
        assert_eq!(far.matches('H').count(), 2, "far apart runs each get their own move: {far:?}");
    }

    trait WithTail {
        fn with_tail(self, tail: &str) -> Self;
    }

    impl WithTail for Line {
        fn with_tail(mut self, tail: &str) -> Self {
            let mut plain = self.plain();
            plain.truncate(plain.len() - 1);
            plain.push_str(tail);
            self.spans = vec![Span::raw(plain)];
            self
        }
    }

    // ------------------------------------------------------------------
    // The contract: the screen looks the same as when the row is written whole.
    // ------------------------------------------------------------------

    #[test]
    fn a_diffed_row_looks_like_the_row_written_whole() {
        let mut random = Lcg(0x5eed);
        let cols = usize::from(COLS);
        for _ in 0..3000 {
            let old = random_line(&mut random, cols);
            let new = if random.below(4) == 0 {
                random_line(&mut random, cols)
            } else {
                mutate(&old, &mut random, cols)
            };
            assert_same_look(&old, &new);
        }
    }

    #[test]
    fn a_run_of_diffs_keeps_looking_like_the_last_row_written_whole() {
        // Each row after the first is a diff from the one before, on a pen that carries over.
        let mut random = Lcg(0xfeed);
        let cols = usize::from(COLS);
        for _ in 0..200 {
            let mut screen = TestScreen::new(COLS, 3);
            let mut line = random_line(&mut random, cols);
            screen.feed(&whole_row(&line, 1));
            let mut writer = StyleWriter::new();
            let mut at = None;
            let mut runs = Vec::new();
            for _ in 0..12 {
                let next = mutate(&line, &mut random, cols);
                let mut bytes = String::new();
                write_diff(&mut bytes, &mut writer, &mut at, 1, &cells(&line), &cells(&next), &mut runs);
                // The frame closes with a reset whenever a pen is left on.
                if !writer.is_plain() {
                    bytes.push_str(RESET);
                    writer.reset();
                }
                at = None;
                screen.feed(&bytes);
                let mut whole = TestScreen::new(COLS, 3);
                whole.feed(&whole_row(&next, 1));
                assert_eq!(screen.looks(1), whole.looks(1), "{:?} -> {:?}", line.plain(), next.plain());
                line = next;
            }
        }
    }

    #[test]
    fn a_diff_is_never_written_past_the_last_column() {
        // TestScreen stops on a write past the last column; a full-width row diffed at its end must not.
        let old = text(&"x".repeat(40));
        let new = text(&format!("{}y", "x".repeat(39)));
        assert_same_look(&old, &new);
        assert_eq!(cells(&new).cells.last().map(|cell| cell.ch), Some('y'));
        let _ = Cell { ch: 'a', style: Style::new(), part: Part::Narrow };
    }
}
