//! painter 가 화면에서 바뀐 것만 쓴다는 시험 (t-17056).
//!
//! 두 종류다. 하나는 **바이트를 센다**: 바뀐 칸 하나가 행 하나 값이 아니고, 한 줄 밀린
//! 목록이 그 줄 하나 값이며, 터미널이 아는 커서 모양·자리를 다시 보내지 않는다. 다른
//! 하나는 **화면을 견준다**: 어떻게 줄였든 화면은 뷰포트 행을 통째로 쓴 화면과 칸
//! 단위로 같아야 하고(글자·너비·색·속성), 커서는 프레임이 말한 자리에 있어야 하며,
//! 밀어낸 행은 스크롤백으로 새지 않아야 한다. 줄이는 일은 화면이 같을 때만 줄이는 일이다.
//!
//! 기준 수치는 설치본 1.1.42 의 실측이다(`tools/tui-bench`, 2026-09-30): 기다리는 동안
//! 프레임 258 B, 도구가 도는 동안 구문 강조된 명령 행을 불릿 한 칸 때문에 다시 써 프레임
//! 931~988 B, 트랜스크립트를 한 줄 넘길 때 1,980 B.

use super::{cup, Painter};
use crate::tui::ansi::{write_spans, Color, Line, Span, Style, RESET};
use crate::tui::test_screen::{mutate, random_line, Lcg, TestScreen};

const COLS: u16 = 40;
const ROWS: u16 = 24;

/// 터미널 하나와 그 아래쪽에 그리는 painter.
struct Rig {
    painter: Painter<Vec<u8>>,
    screen: TestScreen,
}

impl Rig {
    fn new(height: u16) -> Self {
        let mut painter = Painter::new(Vec::new(), COLS, ROWS, 0, true);
        painter.set_height(height);
        Self { painter, screen: TestScreen::new(COLS, ROWS) }
    }

    /// 뷰포트의 첫 화면 행.
    fn top(&self) -> u16 {
        self.painter.top
    }

    /// 한 프레임: 그리고, 끝내고, 나간 바이트를 터미널에 먹인다. 나간 바이트를 돌려준다.
    fn frame(&mut self, rows: &[Line], cursor: Option<(u16, u16)>) -> String {
        self.painter.paint(rows);
        self.painter.end(cursor);
        let out = String::from_utf8(std::mem::take(&mut self.painter.out)).expect("utf-8");
        self.screen.feed(&out);
        out
    }

    /// 화면이 `rows` 를 뷰포트에 통째로 쓴 화면과 같아 보이고, 그 밖은 비어 있다.
    fn assert_shows(&self, rows: &[Line]) {
        let mut whole = TestScreen::new(COLS, ROWS);
        for (index, line) in rows.iter().enumerate() {
            whole.feed(&whole_row(line, self.top() + u16::try_from(index).unwrap_or(0)));
        }
        for row in 0..whole.rows() {
            assert_eq!(self.screen.looks(row), whole.looks(row), "screen row {row}");
        }
    }
}

/// painter 가 늘 써 온 행 바이트: 찾아가기, 리셋, 지우기, 스팬들, 리셋.
fn whole_row(line: &Line, row: u16) -> String {
    let mut out = String::new();
    cup(&mut out, 0, row);
    out.push_str("\u{1b}[0m\u{1b}[K");
    write_spans(line, &mut out);
    out.push_str(RESET);
    out
}

fn plain_rows(texts: &[&str]) -> Vec<Line> {
    texts.iter().map(|text| Line::from_text(*text)).collect()
}

fn grey(level: u8, text: &str) -> Span {
    Span::new(text, Style::new().fg(Color::Rgb(level, level, level)).bold())
}

/// 상태 줄이 그려지는 모양: 웨이브가 글자마다 다른 회색을 주는 단어와 흐려진 카운터.
fn status_row(levels: [u8; 7]) -> Line {
    let mut spans = vec![Span::raw("  ")];
    for (letter, level) in "Working".chars().zip(levels) {
        spans.push(grey(level, &letter.to_string()));
    }
    spans.push(Span::dim(" (3s • esc to interrupt)"));
    Line::new(spans)
}

fn numbered(range: std::ops::Range<usize>) -> Vec<Line> {
    range.map(|number| Line::from_text(format!("line {number:02} of a list that scrolls"))).collect()
}

// ----------------------------------------------------------------------
// 터미널이 아는 커서는 다시 보내지 않는다.
// ----------------------------------------------------------------------

#[test]
fn an_unchanged_caret_is_not_sent_again() {
    let mut rig = Rig::new(2);
    let first = rig.frame(&plain_rows(&["a", "b"]), Some((2, 22)));
    assert!(
        first.contains("\u{1b}[0 q\u{1b}[?25h"),
        "the first frame puts the caret on: {first:?}"
    );
    let second = rig.frame(&plain_rows(&["a", "c"]), Some((2, 22)));
    assert!(
        !second.contains("\u{1b}[0 q") && !second.contains("\u{1b}[?25h"),
        "the terminal already shows the caret in its default shape: {second:?}"
    );
    assert_eq!(rig.screen.cursor(), (2, 22));
}

#[test]
fn a_hidden_caret_is_hidden_once_and_shown_again_when_it_returns() {
    let mut rig = Rig::new(2);
    rig.frame(&plain_rows(&["a", "b"]), Some((2, 22)));
    let hidden = rig.frame(&plain_rows(&["a", "c"]), None);
    assert!(hidden.contains("\u{1b}[?25l"), "{hidden:?}");
    let still = rig.frame(&plain_rows(&["a", "d"]), None);
    assert!(!still.contains("\u{1b}[?25l"), "already hidden: {still:?}");
    let shown = rig.frame(&plain_rows(&["a", "e"]), Some((2, 22)));
    assert!(shown.contains("\u{1b}[0 q\u{1b}[?25h"), "{shown:?}");
    assert_eq!(rig.screen.cursor(), (2, 22));
}

#[test]
fn after_a_clear_the_caret_is_put_on_again() {
    let mut rig = Rig::new(2);
    rig.frame(&plain_rows(&["a", "b"]), Some((2, 22)));
    rig.painter.clear_terminal();
    let after = rig.frame(&plain_rows(&["a", "b"]), Some((2, 22)));
    assert!(after.contains("\u{1b}[0 q\u{1b}[?25h"), "{after:?}");
}

// ----------------------------------------------------------------------
// 바뀐 칸은 칸 값이다.
// ----------------------------------------------------------------------

/// 도는 도구 셀의 머리 행: 불릿 하나와 구문 강조된 긴 명령. 설치본은 불릿의 색이
/// 바뀔 때마다 이 행 전체(약 700 B)를 다시 썼다.
fn command_row(bullet: u8) -> Line {
    let mut spans = vec![grey(bullet, "•"), Span::raw(" "), Span::bold("Running"), Span::raw(" ")];
    for (index, word) in ["for", "i", "in", "{1..120};", "do", "echo", "\"tool output line\"", "sleep", "0.025;", "done"]
        .iter()
        .enumerate()
    {
        spans.push(Span::new(*word, Style::new().fg(Color::Rgb(100 + u8::try_from(index).unwrap_or(0), 150, 200))));
        spans.push(Span::raw(" "));
    }
    Line::new(spans).truncated(usize::from(COLS))
}

#[test]
fn a_bullet_that_changes_colour_costs_the_bullet_and_not_its_command() {
    let mut rig = Rig::new(3);
    let mut rows = vec![command_row(100), Line::from_text("status"), Line::from_text("footer")];
    rig.frame(&rows, Some((1, 23)));
    rows[0] = command_row(200);
    let step = rig.frame(&rows, Some((1, 23)));
    assert_eq!(
        step,
        concat!(
            "\u{1b}[?2026h",
            "\u{1b}[0m",
            "\u{1b}[22;1H\u{1b}[1m\u{1b}[38;2;200;200;200;49m•",
            "\u{1b}[0m",
            "\u{1b}[24;2H",
            "\u{1b}[?2026l",
        ),
        "a byte golden: one cell of one row"
    );
    rig.assert_shows(&rows);
    assert_eq!(rig.screen.cursor(), (1, 23));
}

/// The quiet-loop cell `tests/tui_bytes.rs` pins as a byte golden: one live row whose counter
/// ticks. It used to be the whole row again; now it is the digit.
#[test]
fn a_counter_that_ticks_in_a_live_row_costs_the_digit() {
    let cell = |count: u32| {
        Line::new(vec![
            Span::dim("• "),
            Span::new("loop-1", Style::new().bold()),
            Span::dim(format!(" · quiet ×{count} (last 04:12)")),
        ])
    };
    let mut painter = Painter::new(Vec::new(), 80, 20, 19, true);
    let mut screen = TestScreen::new(80, 20);
    painter.set_height(5);
    painter.paint(&[cell(1)]);
    screen.feed(&painter.take_frame());
    painter.paint(&[cell(5)]);
    let delta = painter.take_frame();
    assert_eq!(
        delta,
        "\u{1b}[?2026h\u{1b}[0m\u{1b}[16;19H\u{1b}[2m5",
        "a move to the digit, its style, the digit"
    );
    screen.feed(&delta);
    let mut whole = TestScreen::new(80, 20);
    whole.feed(&whole_row(&cell(5), 15));
    assert_eq!(screen.looks(15), whole.looks(15));
}

#[test]
fn a_wave_step_writes_the_letters_whose_colour_moved() {
    let mut rig = Rig::new(3);
    let mut rows = plain_rows(&["", "", "footer"]);
    // The wave's edge moves over the last three letters; the first four keep their grey.
    rows[1] = status_row([128, 128, 128, 128, 138, 167, 128]);
    rig.frame(&rows, Some((1, 23)));
    rows[1] = status_row([128, 128, 128, 128, 128, 138, 167]);
    let step = rig.frame(&rows, Some((1, 23)));
    // Three letters at about 22 B each and the frame around them. Written whole, the row
    // was 144 B and its frame 188.
    assert!(step.len() < 125, "{} bytes for a step that moves three letters: {step:?}", step.len());
    assert!(!step.contains("Wor"), "letters that kept their grey are not written again: {step:?}");
    rig.assert_shows(&rows);
    assert_eq!(rig.screen.cursor(), (1, 23));
}

#[test]
fn a_letter_typed_at_the_caret_costs_the_letter() {
    let mut rig = Rig::new(3);
    let composer = |typed: &str| {
        Line::new(vec![Span::raw("│› "), Span::raw(typed.to_string()), Span::raw(" ".repeat(36 - typed.len())), Span::raw("│")])
    };
    let mut rows = vec![Line::empty(), composer("abc"), Line::from_text("footer")];
    // The caret is after the "c", on the composer's row.
    let caret = |typed: &str| Some((u16::try_from(3 + typed.len()).unwrap_or(0), 22));
    rig.frame(&rows, caret("abc"));
    rows[1] = composer("abcd");
    let typed = rig.frame(&rows, caret("abcd"));
    // The caret was where the letter goes and is where it ends: nothing moves it.
    assert_eq!(typed, "\u{1b}[?2026h\u{1b}[0md\u{1b}[?2026l");
    rig.assert_shows(&rows);
    assert_eq!(rig.screen.cursor(), (7, 22));
}

#[test]
fn a_shorter_row_is_erased_from_where_it_ends() {
    let mut rig = Rig::new(3);
    let mut rows = plain_rows(&["", "a long line that gets shorter", "footer"]);
    rig.frame(&rows, Some((1, 23)));
    rows[1] = Line::from_text("a long");
    let step = rig.frame(&rows, Some((1, 23)));
    // The space after "long" is a space in both rows: the first column that differs is the "l" of "line".
    assert!(step.contains("\u{1b}[23;8H\u{1b}[K"), "{step:?}");
    rig.assert_shows(&rows);
}

#[test]
fn rows_the_painter_never_drew_are_written_whole() {
    let mut rig = Rig::new(3);
    let rows = plain_rows(&["one", "two", "three"]);
    let first = rig.frame(&rows, Some((1, 23)));
    assert_eq!(first.matches("\u{1b}[0m\u{1b}[K").count(), 3, "{first:?}");
    // A cleared terminal has forgotten them.
    rig.painter.clear_terminal();
    rig.screen = TestScreen::new(COLS, ROWS);
    let again = rig.frame(&rows, Some((1, 23)));
    assert_eq!(again.matches("\u{1b}[0m\u{1b}[K").count(), 3, "{again:?}");
    rig.assert_shows(&rows);
}

// ----------------------------------------------------------------------
// 프레임 안에서 남이 건드린 것을 믿지 않는다.
// ----------------------------------------------------------------------

/// 어떤 스타일도 켜 두지 않았다는 것은 painter 가 낸 바이트에 대한 앎이다. 프레임에 남의
/// 바이트가 끼어(알림 바이트가 그렇게 간다) 굵기를 켜 둔 채 두어도 그 굵기가 새로 쓰는
/// 칸에 묻어서는 안 된다 — 칸을 쓰기 전에 리셋이 한 번 나가는 것이 그 보험이다.
#[test]
fn a_pen_someone_else_left_on_does_not_reach_the_cells_that_are_written() {
    let mut rig = Rig::new(3);
    let mut rows = plain_rows(&["", "status", "footer"]);
    rig.frame(&rows, Some((1, 23)));
    rig.painter.emit_raw("\u{1b}[1m");
    rows[1] = Line::from_text("stXtus");
    let step = rig.frame(&rows, Some((1, 23)));
    assert!(step.contains("\u{1b}[1m\u{1b}[0m"), "the reset comes before the first cell: {step:?}");
    rig.assert_shows(&rows);
}

/// 커서를 어디 두었다는 앎은 `paint` 가 프레임의 마지막 쓰기였을 때만 맞다. 그 뒤에 히스토리 같은
/// 것이 바이트를 더했다면 터미널의 커서는 그 바이트가 끝난 자리에 있다.
#[test]
fn a_caret_is_put_back_when_something_wrote_after_the_paint() {
    let mut rig = Rig::new(3);
    let composer = |typed: &str| Line::new(vec![Span::raw("│› "), Span::raw(typed.to_string())]);
    let mut rows = vec![Line::empty(), composer("ab"), Line::from_text("footer")];
    rig.frame(&rows, Some((5, 22)));
    rows[1] = composer("abc");
    rig.painter.paint(&rows);
    // The last cell written ends on the caret's column ...
    rig.painter.insert_history(&[Line::from_text("late history")]);
    rig.painter.end(Some((6, 22)));
    let out = String::from_utf8(std::mem::take(&mut rig.painter.out)).expect("utf-8");
    rig.screen.feed(&out);
    // ... but history came after it, and the terminal's cursor is not there any more.
    assert_eq!(rig.screen.cursor(), (6, 22), "the caret goes back to where the frame said: {out:?}");
}

// ----------------------------------------------------------------------
// 밀린 행은 터미널이 민다.
// ----------------------------------------------------------------------

#[test]
fn rows_that_only_moved_up_cost_the_row_that_came_in() {
    let mut rig = Rig::new(12);
    rig.frame(&numbered(0..12), Some((1, 23)));
    let scrolled = rig.frame(&numbered(1..13), Some((1, 23)));
    assert_eq!(scrolled.matches("line ").count(), 1, "only the row that came in is written: {scrolled:?}");
    assert!(scrolled.len() < 160, "{} bytes: {scrolled:?}", scrolled.len());
    rig.assert_shows(&numbered(1..13));
    assert!(rig.screen.scrollback.is_empty(), "a region below the first row keeps nothing");
    assert_eq!(rig.screen.cursor(), (1, 23));
}

#[test]
fn rows_that_only_moved_down_cost_the_row_that_came_in() {
    let mut rig = Rig::new(12);
    rig.frame(&numbered(1..13), Some((1, 23)));
    let scrolled = rig.frame(&numbered(0..12), Some((1, 23)));
    assert_eq!(scrolled.matches("line ").count(), 1, "{scrolled:?}");
    assert!(scrolled.len() < 160, "{} bytes: {scrolled:?}", scrolled.len());
    rig.assert_shows(&numbered(0..12));
}

#[test]
fn a_header_that_changes_beside_rows_that_moved_is_written_by_itself() {
    let mut rig = Rig::new(12);
    let page = |first: usize| {
        let mut rows = vec![Line::from_text(format!("{}-{} of 100 lines", first + 1, first + 10))];
        rows.extend(numbered(first..first + 10));
        rows.push(Line::from_text("↑↓ scroll"));
        rows
    };
    rig.frame(&page(20), Some((1, 23)));
    let scrolled = rig.frame(&page(21), Some((1, 23)));
    assert!(scrolled.len() < 220, "{} bytes: {scrolled:?}", scrolled.len());
    assert_eq!(scrolled.matches("line ").count(), 1, "{scrolled:?}");
    rig.assert_shows(&page(21));
    // And a page further, held down.
    for first in 22..40 {
        let rows = page(first);
        let frame = rig.frame(&rows, Some((1, 23)));
        assert!(frame.len() < 220, "{} bytes at {first}: {frame:?}", frame.len());
        rig.assert_shows(&rows);
    }
}

#[test]
fn a_shift_that_saves_less_than_it_costs_is_not_taken() {
    let mut rig = Rig::new(5);
    rig.frame(&plain_rows(&["a", "b", "c", "d", "e"]), Some((1, 23)));
    let step = rig.frame(&plain_rows(&["b", "c", "d", "e", "f"]), Some((1, 23)));
    assert!(!step.contains('\n'), "five one-letter rows are cheaper written than scrolled: {step:?}");
    rig.assert_shows(&plain_rows(&["b", "c", "d", "e", "f"]));
}

#[test]
fn a_page_turn_has_no_row_to_keep_and_scrolls_nothing() {
    let mut rig = Rig::new(12);
    rig.frame(&numbered(0..12), Some((1, 23)));
    let turned = rig.frame(&numbered(30..42), Some((1, 23)));
    assert!(!turned.contains('\n') && !turned.contains("\u{1b}M"), "{turned:?}");
    rig.assert_shows(&numbered(30..42));
}

// ----------------------------------------------------------------------
// The contract: whatever it saves, the screen is the screen written whole.
// ----------------------------------------------------------------------

#[test]
fn frames_of_random_rows_look_like_the_rows_written_whole() {
    let cols = usize::from(COLS);
    for seed in 0..60u64 {
        let mut random = Lcg(seed * 7919 + 1);
        let height = 3 + random.below(9);
        let mut rig = Rig::new(u16::try_from(height).unwrap_or(3));
        let mut rows: Vec<Line> = (0..height).map(|_| random_line(&mut random, cols)).collect();
        for _ in 0..80 {
            match random.below(6) {
                0 => {
                    // The list moved by a few rows and new rows came in at the end.
                    let by = 1 + random.below(3.min(height - 1));
                    if random.below(2) == 0 {
                        rows.rotate_left(by);
                        for row in rows.iter_mut().rev().take(by) {
                            *row = random_line(&mut random, cols);
                        }
                    } else {
                        rows.rotate_right(by);
                        for row in rows.iter_mut().take(by) {
                            *row = random_line(&mut random, cols);
                        }
                    }
                }
                1 | 2 => {
                    let at = random.below(height);
                    rows[at] = mutate(&rows[at], &mut random, cols);
                }
                3 => {
                    let at = random.below(height);
                    rows[at] = random_line(&mut random, cols);
                }
                4 => {
                    for row in &mut rows {
                        if random.below(3) == 0 {
                            *row = mutate(row, &mut random, cols);
                        }
                    }
                }
                _ => {}
            }
            let cursor = match random.below(3) {
                0 => None,
                _ => Some((
                    u16::try_from(random.below(cols - 1)).unwrap_or(0),
                    rig.top() + u16::try_from(random.below(height)).unwrap_or(0),
                )),
            };
            rig.frame(&rows, cursor);
            rig.assert_shows(&rows);
            if let Some((col, row)) = cursor {
                assert_eq!(
                    rig.screen.cursor(),
                    (usize::from(col), usize::from(row)),
                    "seed {seed}: the caret is where the frame said"
                );
            }
        }
        assert!(rig.screen.scrollback.is_empty(), "seed {seed}: nothing may leave through the top");
    }
}
