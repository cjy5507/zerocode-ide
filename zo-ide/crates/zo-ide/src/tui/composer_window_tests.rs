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

use super::Composer;
use crate::tui::ansi::Line;
use crate::tui::footer_hints::FooterHints;
use crate::tui::painter::Painter;
use crate::tui::test_screen::{Glyph, TestScreen};
use crate::tui::view::{self, Frame, Status};

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

    /// `App::paint` 가 아래 창을 그리는 순서: 짓고, 높이를 맞추고, 그리고, 캐럿을 놓는다.
    fn show(&mut self, composer: &Composer, status: Option<&Status>, pending: Option<&[Line]>) {
        let room = usize::from(self.painter.max_height());
        let frame = frame(composer, status, pending, usize::from(self.cols), room);
        let (rows, cursor) = view::build(&frame);
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
    let mut text = String::new();
    let mut number = 0;
    while text.len() < 20 * 1024 {
        number += 1;
        text.push_str(&format!(
            "draft line {number:03}: the quick brown fox jumps over the lazy dog while the notes \
             for step {number} are written down in plain words\n"
        ));
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
/// 해도 화면이 틀리지 않는다.
#[test]
fn walking_the_caret_through_a_long_draft_leaves_the_screen_a_fresh_paint_would_make() {
    let status = Status::working(Duration::from_secs(3));
    let mut rig = Rig::new(60, 12);
    let mut composer = composer_with(&draft(60));
    rig.show(&composer, Some(&status), None);
    let mut steps = 0;
    while composer.cursor() > 0 {
        for _ in 0..7 {
            composer.left();
        }
        rig.show(&composer, Some(&status), None);
        steps += 1;
        rig.assert_fresh(&composer, Some(&status), &format!("walking up, step {steps}"));
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
        rig.assert_fresh(&composer, Some(&status), &format!("walking down, step {steps}"));
    }
}
