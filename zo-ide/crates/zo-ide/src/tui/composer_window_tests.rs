//! 화면보다 키가 큰 작성창 — 캐럿이 있는 행은 언제나 화면에 그려진다 (t-17194).
//!
//! 초안이 화면보다 많은 행을 가지면 뷰는 행을 모두 만들었고 painter 는 화면 높이만큼의 첫
//! 행들만 그렸다. 긴 초안의 끝에서는 캐럿도 방금 친 글자도 그려지지 않은 행에 있었다 —
//! 20 KB 를 붙여 넣은 뒤 글자를 쳐도 화면이 그대로였다. 벤치의 `draft` 상태가 캐럿을
//! Ctrl-A 로 초안의 처음에 옮겨 놓고 잰 까닭이다.
//!
//! 이 시험들은 `view::build` 가 낸 행을 앱이 아래 창을 그리는 길 그대로 painter 에 먹이고
//! (`App::paint`: 짓기, 높이 맞추기, 그리기, 캐럿 놓기) 나간 바이트를 시험용 터미널
//! (`test_screen`)에 되풀이해 **화면이 무엇을 보이는지** 본다. 캐럿의 자리는 터미널이
//! 화면 안으로 끌어오기 전의 값으로 읽는다 — 화면 밖 행으로 간 캐럿이 마지막 행에
//! 있는 것처럼 보이면 안 된다.

use std::cell::RefCell;
use std::io::{self, Write};
use std::rc::Rc;
use std::time::{Duration, Instant};

use runtime::message_stream::{BlockId, QuestionOption, UserQuestionPrompt};

use super::{scrolled_to, Composer};
use crate::tui::ansi::Line;
use crate::tui::footer_hints::FooterHints;
use crate::tui::painter::Painter;
use crate::tui::question;
use crate::tui::test_screen::{Glyph, TestScreen};
use crate::tui::view::{self, Frame, Popup, PopupRow, Status};

/// painter 가 쓰는 바이트를 시험이 꺼내 볼 수 있게 받아 두는 출구.
#[derive(Clone, Debug, Default)]
struct Wire(Rc<RefCell<Vec<u8>>>);

impl Write for Wire {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 화면 하나와 그 아래쪽에 그리는 painter.
struct Rig {
    painter: Painter<Wire>,
    wire: Wire,
    screen: TestScreen,
    cols: u16,
    rows: u16,
    /// 지난 프레임이 놓은 캐럿의 화면 좌표 `(열, 행)` — 터미널이 끌어오기 **전**의 값.
    caret: Option<(u16, u16)>,
}

impl Rig {
    fn new(cols: u16, rows: u16) -> Self {
        let wire = Wire::default();
        Self {
            painter: Painter::new(wire.clone(), cols, rows, 0, true),
            wire,
            screen: TestScreen::new(cols, rows),
            cols,
            rows,
            caret: None,
        }
    }

    /// 아래 창 하나를 그린다 — 팝업도 질문도 없는 프레임으로.
    fn show(&mut self, composer: &Composer, status: Option<&Status>, pending: Option<&[Line]>) {
        let room = usize::from(self.painter.max_height());
        self.show_frame(&frame(composer, status, pending, usize::from(self.cols), room));
    }

    /// `App::paint` 가 아래 창을 그리는 순서: 짓고, 높이를 맞추고, 그리고, 캐럿을 놓는다.
    fn show_frame(&mut self, frame: &Frame<'_>) {
        let (rows, cursor) = view::build(frame);
        self.painter.set_height(u16::try_from(rows.len()).unwrap_or(u16::MAX));
        self.painter.paint(&rows);
        self.caret = cursor.map(|(col, row)| self.painter.absolute(col, row));
        self.painter.end(self.caret);
        let bytes = self.wire.0.take();
        self.screen.feed(&String::from_utf8(bytes).expect("utf-8"));
    }

    /// 화면 행 하나를 글자로.
    fn text(&self, row: usize) -> String {
        self.screen
            .looks(row)
            .iter()
            .filter_map(|(glyph, _)| match glyph {
                Glyph::Blank => Some(' '),
                Glyph::Narrow(ch) | Glyph::WideHead(ch) => Some(*ch),
                Glyph::WideTail => None,
            })
            .collect()
    }

    /// 지난 프레임의 캐럿이 화면 안 행에 있어야 하고, 그 행의 글자를 돌려준다.
    fn caret_row_text(&self, case: &str) -> String {
        let (_, row) = self
            .caret
            .unwrap_or_else(|| panic!("{case}: the frame placed no caret"));
        assert!(
            row < self.rows,
            "{case}: the caret is on screen row {row} of a {}-row screen — off the bottom, on a \
             row nobody drew",
            self.rows
        );
        self.text(usize::from(row))
    }

    /// 지금 화면이 같은 상태를 처음부터 한 번에 그린 화면과 칸 단위로 같다.
    fn assert_fresh(&self, composer: &Composer, status: Option<&Status>, case: &str) {
        let mut fresh = Self::new(self.cols, self.rows);
        fresh.show(composer, status, None);
        for row in 0..usize::from(self.rows) {
            assert_eq!(self.screen.looks(row), fresh.screen.looks(row), "{case}: screen row {row}");
        }
    }
}

/// 이 시험들의 프레임 — 아래 창만 있고 팝업·다이얼로그는 없다.
fn frame<'a>(
    composer: &'a Composer,
    status: Option<&'a Status>,
    pending: Option<&'a [Line]>,
    width: usize,
    max_rows: usize,
) -> Frame<'a> {
    Frame {
        composer,
        effort_tier: None,
        effort_effect: None,
        now: Instant::now(),
        status,
        pending_input: pending,
        active: None,
        tail_owns_slot: false,
        dialog: None,
        question: None,
        picker: None,
        sessions: None,
        pager: None,
        popup: None,
        mention: None,
        shortcuts: None,
        warnings: None,
        hints: FooterHints::default(),
        model: "claude-opus-5",
        effort: "high",
        model_note: None,
        cwd: "/tmp/x",
        context_left: None,
        context_used_tokens: None,
        plan_mode: false,
        goal_status: None,
        loop_status: None,
        dream: None,
        width,
        max_rows,
        // 이 시험들의 프레임에는 팝업이 없어 예산과 화면의 한계가 같다.
        max_height: max_rows,
    }
}

/// 한 행에 다 들어가는 짧은 행 `rows` 개의 초안. 행은 `row NN of the draft` 열아홉 글자다.
fn draft(rows: usize) -> String {
    (1..=rows)
        .map(|number| format!("row {number:02} of the draft"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn composer_with(text: &str) -> Composer {
    let mut composer = Composer::new();
    composer.insert_str(text);
    composer
}

#[derive(Clone, Copy, Debug)]
enum Place {
    Start,
    Middle,
    End,
}

/// 캐럿을 놓고, 그때 캐럿 행에 그려져 있어야 할 초안 행의 번호(1 기준)를 돌려준다.
fn put_caret(composer: &mut Composer, draft_rows: usize, place: Place) -> usize {
    match place {
        Place::Start => {
            composer.home();
            1
        }
        Place::End => {
            composer.end();
            draft_rows
        }
        Place::Middle => {
            let row = draft_rows / 2 + 1;
            // 행마다 열아홉 글자와 줄바꿈 하나 — 스무 바이트. 캐럿은 `row ` 바로 뒤에 선다.
            composer.set_cursor((row - 1) * 20 + 4);
            row
        }
    }
}

#[test]
fn a_draft_taller_than_the_screen_keeps_the_row_of_its_caret_on_screen() {
    let mut rig = Rig::new(60, 10);
    let composer = composer_with(&draft(60));
    rig.show(&composer, None, None);
    let shown = rig.caret_row_text("a 60-row draft on a 10-row screen");
    assert!(
        shown.contains("row 60 of the draft"),
        "the caret's row shows the end of the draft: {shown:?}"
    );
}

#[test]
fn a_letter_typed_at_the_end_of_a_long_draft_is_drawn_at_the_caret() {
    let mut rig = Rig::new(60, 10);
    let mut composer = composer_with(&draft(60));
    rig.show(&composer, None, None);
    composer.insert_char('Z');
    rig.show(&composer, None, None);
    let shown = rig.caret_row_text("after a letter typed at the end of a 60-row draft");
    assert!(
        shown.contains("row 60 of the draftZ"),
        "the letter stands where the caret is: {shown:?}"
    );
    rig.assert_fresh(&composer, None, "a letter typed at the end of a long draft");
}

/// 벤치의 `draft` 상태: 20 KB 를 붙이면 캐럿은 끝에 있고, 그다음 글자가 그 자리에 나타난다.
#[test]
fn after_a_20_kb_paste_the_next_letter_appears_at_the_caret() {
    use std::fmt::Write as _;
    let mut text = String::new();
    let mut number = 0;
    while text.len() < 20 * 1024 {
        number += 1;
        let _ = write!(
            text,
            "draft line {number:03}: the quick brown fox jumps over the lazy dog while the notes \
             for step {number} are written down in plain words\n"
        );
    }
    text.push_str("the very last words");
    let mut rig = Rig::new(120, 40);
    let mut composer = Composer::new();
    composer.insert_pasted(&text);
    rig.show(&composer, None, None);
    let shown = rig.caret_row_text("a 20 KB paste on a 40-row screen");
    assert!(shown.contains("the very last words"), "the caret's row: {shown:?}");
    composer.insert_char('б');
    rig.show(&composer, None, None);
    let shown = rig.caret_row_text("a letter typed after a 20 KB paste");
    assert!(shown.contains("the very last wordsб"), "the letter is drawn: {shown:?}");
}

/// 화면 높이·초안 길이·상태 줄·대기 입력·캐럿 자리의 모든 조합에서 캐럿 행이 그려진다.
/// 화면이 아래 창의 붙박이 줄들(빈 줄·푸터·상태)보다 낮아도 마찬가지다.
#[test]
fn the_caret_row_is_on_screen_at_every_screen_height_and_draft_length() {
    let status = Status::working(Duration::from_secs(3));
    let queued = [Line::from_text("  ↳ queued: run the tests")];
    for screen_rows in 4..=24u16 {
        for draft_rows in [1usize, 2, 3, 8, 25, 60] {
            for (has_status, has_pending) in [(false, false), (true, false), (true, true)] {
                for place in [Place::Start, Place::Middle, Place::End] {
                    let case = format!(
                        "{screen_rows}-row screen, {draft_rows}-row draft, status {has_status}, \
                         queued {has_pending}, caret at {place:?}"
                    );
                    let mut composer = composer_with(&draft(draft_rows));
                    let row = put_caret(&mut composer, draft_rows, place);
                    let mut rig = Rig::new(60, screen_rows);
                    let working = has_status.then_some(&status);
                    rig.show(&composer, working, has_pending.then_some(&queued[..]));
                    let shown = rig.caret_row_text(&case);
                    let expected = format!("row {row:02} of the draft");
                    assert!(shown.contains(&expected), "{case}: the caret's row shows {shown:?}");
                }
            }
        }
    }
}

/// 캐럿을 초안 끝에서 처음까지 걸어 올라갔다가 다시 내려와도, 글자를 끼워도, 화면은 같은 상태를
/// 처음부터 그린 화면과 칸 단위로 같다 — painter 가 바뀐 칸만 쓰고 밀린 행은 터미널이 밀게
/// 해도 화면이 틀리지 않는다. 12행 화면은 상자가 네 행이라 창이 한 행 밀려도 바뀐 행이 둘뿐이고,
/// 24행 화면은 상자가 열여섯 행이라 창이 밀릴 때 painter 가 영역 스크롤을 고른다.
#[test]
fn walking_the_caret_through_a_long_draft_leaves_the_screen_a_fresh_paint_would_make() {
    let status = Status::working(Duration::from_secs(3));
    for screen_rows in [12u16, 24] {
        let mut rig = Rig::new(60, screen_rows);
        let mut composer = composer_with(&draft(60));
        rig.show(&composer, Some(&status), None);
        let mut steps = 0;
        while composer.cursor() > 0 {
            for _ in 0..7 {
                composer.left();
            }
            rig.show(&composer, Some(&status), None);
            steps += 1;
            let case = format!("{screen_rows}-row screen, walking up, step {steps}");
            rig.assert_fresh(&composer, Some(&status), &case);
        }
        composer.insert_char('Q');
        rig.show(&composer, Some(&status), None);
        rig.assert_fresh(&composer, Some(&status), "a letter typed at the start");
        while composer.cursor() < composer.text().len() {
            for _ in 0..7 {
                composer.right();
            }
            rig.show(&composer, Some(&status), None);
            steps += 1;
            let case = format!("{screen_rows}-row screen, walking down, step {steps}");
            rig.assert_fresh(&composer, Some(&status), &case);
        }
    }
}

// ----------------------------------------------------------------------
// 작성창 안에서 초안이 스크롤된다.
// ----------------------------------------------------------------------

fn plain(lines: &[Line]) -> Vec<String> {
    lines.iter().map(Line::plain).collect()
}

fn render(composer: &Composer, width: usize, max_rows: usize) -> (Vec<Line>, (u16, u16)) {
    composer.render_with_effort(width, "", None, None, Instant::now(), max_rows)
}

#[test]
fn the_first_visible_row_moves_only_as_far_as_the_caret_needs() {
    // 초안이 창에 다 들어가면 스크롤은 0 이다 — 기억해 둔 값이 남아 있어도.
    assert_eq!(scrolled_to(7, 4, 3, 4), 0);
    assert_eq!(scrolled_to(0, 1, 0, 3), 0);
    // 캐럿이 창 안이면 글은 제자리다.
    assert_eq!(scrolled_to(10, 60, 10, 4), 10);
    assert_eq!(scrolled_to(10, 60, 13, 4), 10);
    // 위로 나가면 캐럿이 첫 행, 아래로 나가면 마지막 행이 되도록 옮긴다.
    assert_eq!(scrolled_to(10, 60, 9, 4), 9);
    assert_eq!(scrolled_to(10, 60, 14, 4), 11);
    // 초안이 줄어 기억한 값이 끝을 넘어도 끝에서 멈춘다.
    assert_eq!(scrolled_to(50, 10, 9, 4), 6);
    // 처음 그릴 때(스크롤 0) 끝에 선 캐럿은 창의 마지막 행에 놓인다.
    assert_eq!(scrolled_to(0, 60, 59, 4), 56);
}

#[test]
fn a_box_taller_than_its_room_scrolls_the_draft_between_its_lines() {
    let composer = composer_with(&draft(60));
    let (lines, (col, row)) = render(&composer, 40, 6);
    let text = plain(&lines);
    assert_eq!(text.len(), 6, "{text:#?}");
    assert!(text[0].starts_with('╭') && text[5].starts_with('╰'), "{text:#?}");
    // 위아래 선 사이에 초안의 마지막 네 행이 서고, 캐럿은 마지막 행에 있다.
    assert!(text[1].contains("row 57 of the draft"), "{text:#?}");
    assert!(text[4].contains("row 60 of the draft"), "{text:#?}");
    assert_eq!((usize::from(col), usize::from(row)), (1 + 2 + 19, 4));
    // 마커 `›` 는 보이는 첫 행에 선다 — 초안이 스크롤돼도 여기가 입력창이다.
    assert!(text[1].starts_with("│› "), "{text:#?}");
    assert!(text[2].starts_with("│  "), "{text:#?}");
}

#[test]
fn the_box_never_shrinks_below_its_two_lines_and_the_row_of_the_caret() {
    let composer = composer_with(&draft(60));
    for max_rows in [0, 1, 2, 3] {
        let (lines, (_, row)) = render(&composer, 40, max_rows);
        let text = plain(&lines);
        assert_eq!(text.len(), 3, "max_rows {max_rows}: {text:#?}");
        assert!(text[1].contains("row 60 of the draft"), "max_rows {max_rows}: {text:#?}");
        assert_eq!(row, 1, "max_rows {max_rows}");
    }
}

#[test]
fn a_box_too_narrow_for_its_lines_scrolls_too() {
    // 네 칸이면 선이 없고, 글 자리는 두 칸이라 열아홉 글자 행이 열 행으로 접힌다.
    let composer = composer_with(&draft(10));
    let (lines, (_, row)) = render(&composer, 4, 3);
    let text = plain(&lines);
    assert_eq!(text.len(), 3, "{text:#?}");
    assert_eq!(row, 2, "{text:#?}");
    assert_eq!(text[2].trim(), "t", "the draft ends with `draft`: {text:#?}");
}

#[test]
fn the_draft_stays_put_while_the_caret_moves_inside_the_rows_it_can_see() {
    let mut composer = composer_with(&draft(60));
    // 초안의 `number` 번째 행(1 기준)의 `row ` 바로 뒤. 행마다 스무 바이트다.
    let after_row_label = |number: usize| (number - 1) * 20 + 4;
    let top_row = |lines: &[Line]| lines[1].plain();

    // 끝에서 시작하면 마지막 네 행이 보인다.
    let (lines, (_, row)) = render(&composer, 40, 6);
    assert!(top_row(&lines).contains("row 57 of the draft"), "{:#?}", plain(&lines));
    assert_eq!(row, 4);
    // 캐럿이 보이는 행 안에서 올라가는 동안 글은 미끄러지지 않는다.
    composer.set_cursor(after_row_label(57));
    let (lines, (_, row)) = render(&composer, 40, 6);
    assert!(top_row(&lines).contains("row 57 of the draft"), "{:#?}", plain(&lines));
    assert_eq!(row, 1);
    // 창 위로 한 행 나가면 글이 한 행만 밀린다.
    composer.set_cursor(after_row_label(56));
    let (lines, (_, row)) = render(&composer, 40, 6);
    assert!(top_row(&lines).contains("row 56 of the draft"), "{:#?}", plain(&lines));
    assert_eq!(row, 1);
    // 다시 내려오면 캐럿이 창의 마지막 행에 닿을 때까지 글이 그대로다.
    composer.set_cursor(after_row_label(59));
    let (lines, (_, row)) = render(&composer, 40, 6);
    assert!(top_row(&lines).contains("row 56 of the draft"), "{:#?}", plain(&lines));
    assert_eq!(row, 4);
    composer.end();
    let (lines, (_, row)) = render(&composer, 40, 6);
    assert!(top_row(&lines).contains("row 57 of the draft"), "{:#?}", plain(&lines));
    assert_eq!(row, 4);
}

#[test]
fn a_draft_that_shrinks_gives_the_scroll_back() {
    let mut composer = composer_with(&draft(60));
    let (lines, _) = render(&composer, 40, 6);
    assert!(lines[1].plain().contains("row 57 of the draft"));
    composer.clear();
    composer.insert_str("short");
    let (lines, (col, row)) = render(&composer, 40, 6);
    let text = plain(&lines);
    assert_eq!(text.len(), 3, "{text:#?}");
    assert_eq!(text[1], format!("│› short{}│", " ".repeat(31)));
    assert_eq!((col, row), (1 + 2 + 5, 1));
}

// ----------------------------------------------------------------------
// 아래 창이 작성창에 남기는 높이.
// ----------------------------------------------------------------------

/// 화면을 채울 만큼 긴 초안이 아래 창의 다른 줄들과 함께 화면 높이 안에 든다: 활성 셀 위의 빈 줄
/// 하나도 놓지 않는다 — 놓으면 푸터의 마지막 줄이 화면 밑으로 밀려난다.
#[test]
fn an_active_cell_gives_way_to_a_draft_that_fills_the_screen() {
    let composer = composer_with(&draft(60));
    let cell = [Line::from_text("streamed words")];
    let frame = Frame { active: Some(&cell[..]), ..frame(&composer, None, None, 60, 9) };
    let (rows, cursor) = view::build(&frame);
    let text = plain(&rows);
    assert_eq!(rows.len(), 9, "the pane is the screen's height, not a row more: {text:#?}");
    let (_, row) = cursor.expect("a caret");
    assert!(text[usize::from(row)].contains("row 60 of the draft"), "{text:#?}");
    assert!(text[8].contains("for shortcuts"), "the footer's last row is on screen: {text:#?}");
}

/// 예산(`max_rows`)이 작아도 화면(`max_height`)에 들어가는 초안은 팝업이 떠도 쪼그라들지 않는다.
#[test]
fn an_open_popup_does_not_squeeze_a_draft_that_fits_the_screen() {
    let composer = composer_with(&draft(6));
    let popup = Popup {
        rows: ["/model", "/resume", "/status"]
            .iter()
            .map(|name| PopupRow { name: (*name).to_string(), description: "a command".to_string() })
            .collect(),
        selected: 0,
    };
    let frame = Frame { popup: Some(&popup), max_rows: 10, ..frame(&composer, None, None, 60, 39) };
    let (rows, cursor) = view::build(&frame);
    let text = plain(&rows);
    for number in 1..=6 {
        let expected = format!("row {number:02} of the draft");
        assert!(text.iter().any(|row| row.contains(&expected)), "{expected}: {text:#?}");
    }
    let (_, row) = cursor.expect("a caret");
    assert!(text[usize::from(row)].contains("row 06 of the draft"), "{text:#?}");
    assert!(text.iter().any(|row| row.contains("/resume")), "the popup is there: {text:#?}");
}

/// 대기 입력이 화면보다 높이 쌓여도 캐럿 행은 화면에 남는다 — 덜어 내는 것은 그 위의 줄들이다.
#[test]
fn queued_lines_piled_higher_than_the_screen_give_way_to_the_caret_row() {
    let queued: Vec<Line> =
        (1..=30).map(|number| Line::from_text(format!("  ↳ queued {number}"))).collect();
    let composer = composer_with("the answer");
    let mut rig = Rig::new(60, 12);
    rig.show(&composer, None, Some(&queued[..]));
    let shown = rig.caret_row_text("30 queued lines on a 12-row screen");
    assert!(shown.contains("the answer"), "{shown:?}");
}

// ----------------------------------------------------------------------
// 질문 오버레이 안의 작성창.
// ----------------------------------------------------------------------

fn prompt(options: Vec<QuestionOption>) -> UserQuestionPrompt {
    let (responder, _receiver) = tokio::sync::oneshot::channel();
    UserQuestionPrompt {
        id: BlockId(7),
        question: "Say more about the failure.".to_string(),
        header: None,
        options,
        multi_select: false,
        responder,
    }
}

fn option(label: &str, description: &str) -> QuestionOption {
    QuestionOption {
        label: label.to_string(),
        description: Some(description.to_string()),
        preview: None,
    }
}

#[test]
fn a_long_answer_scrolls_inside_the_question_overlay() {
    let prompt = prompt(Vec::new());
    let question = question::view(&prompt);
    let composer = composer_with(&draft(60));
    let mut rig = Rig::new(60, 14);
    let room = usize::from(rig.painter.max_height());
    let frame = Frame { question: Some(&question), ..frame(&composer, None, None, 60, room) };
    rig.show_frame(&frame);
    let shown = rig.caret_row_text("a 60-row answer under a question on a 14-row screen");
    assert!(shown.contains("row 60 of the draft"), "{shown:?}");
    // 질문과 푸터는 남고, 초안이 그 사이에서 스크롤된다.
    let screen: Vec<String> = (0..14).map(|row| rig.text(row)).collect();
    assert!(screen.iter().any(|row| row.contains("Say more about the failure.")), "{screen:#?}");
    assert!(screen.iter().any(|row| row.contains("enter to submit answer")), "{screen:#?}");
}

#[test]
fn a_long_answer_typed_as_other_keeps_the_option_it_belongs_to() {
    let prompt = prompt(vec![option("OAuth", "Browser flow."), option("API key", "Static credential.")]);
    let mut question = question::view(&prompt);
    question.down();
    question.down();
    assert!(question.begin_other_input());
    let composer = composer_with(&draft(60));
    let mut rig = Rig::new(60, 20);
    let room = usize::from(rig.painter.max_height());
    let frame = Frame { question: Some(&question), ..frame(&composer, None, None, 60, room) };
    rig.show_frame(&frame);
    let shown = rig.caret_row_text("a 60-row answer typed as Other on a 20-row screen");
    assert!(shown.contains("row 60 of the draft"), "{shown:?}");
    let screen: Vec<String> = (0..20).map(|row| rig.text(row)).collect();
    assert!(screen.iter().any(|row| row.contains("None of the above")), "{screen:#?}");
}
