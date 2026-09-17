//! 화면 관리 — codex 의 인라인 뷰포트 문법으로, Claude Code 의 자리에.
//!
//! alt-screen 도, 히스토리 재그리기도 없다. 화면 하단 몇 줄만 살아 있는
//! 뷰포트(머리: 상태·컴포저·푸터)로 소유하고, 완성된 줄은 **스크롤 영역 +
//! 역인덱스(ESC M)** 문법으로 그 위에 쓴다. codex `tui/src/insert_history.rs`
//! 의 Standard 경로를 옮긴 것이고, `docs/captures/codex-tui-v0.149.1-*.bin` 이
//! 그 바이트를 증언한다:
//!
//! ```text
//! ESC[?2026h
//! ESC[13;40r ESC[13;1H ESC M ESC M ESC[r     ← 뷰포트를 아래로 민다
//! ESC[1;14r ESC[12;1H                        ← 위쪽만 스크롤 영역으로 묶고
//! \r\n ESC[39;49m ESC[K <줄> ESC[39m ESC[49m ESC[0m   ← 줄마다
//! ESC[r                                      ← 영역 복구
//! ```
//!
//! 자리는 codex 와 다르다(사용자 결정 2026-09-06). codex 의 머리는 커서 행에서
//! 열려 트랜스크립트를 따라 내려오지만, 여기 머리는 **언제나 바닥**(`rows -
//! height`)에 있고 트랜스크립트가 화면 **위에서부터** 채운다 — Claude Code
//! 처럼. 둘 사이의 빈 행이 띠(`gap`)다: 히스토리는 띠를 위에서부터 채우고,
//! 자라는 머리는 띠를 아래에서부터 먹고, 띠가 다 없어져야 머리 위 영역이
//! 스크롤한다. 위쪽 영역의 상단 마진이 1행이라 그때 밀려난 줄은 터미널이
//! 스크롤백에 저장한다 — 히스토리가 남는 이유고, 짧은 트랜스크립트는 첫 행을
//! 잃지 않는 이유다.

use std::collections::VecDeque;
use std::io::Write;

use super::ansi::{write_plain, write_spans, Line, RESET};
use super::wrap::wrap_line;

/// 프레임 동기화 시작/끝 — 캡처는 프레임마다 이 짝을 낸다.
const SYNC_BEGIN: &str = "\u{1b}[?2026h";
const SYNC_END: &str = "\u{1b}[?2026l";
/// 커서 숨김/보임과 DECSCUSR 기본 모양 — 캡처의 프레임 꼬리 그대로.
const CURSOR_HIDE: &str = "\u{1b}[?25l";
const CURSOR_SHOW: &str = "\u{1b}[0 q\u{1b}[?25h";
/// 역인덱스 — 스크롤 영역 맨 위에서 내용을 한 줄 아래로 민다.
const REVERSE_INDEX: &str = "\u{1b}M";
/// 스크롤 영역 해제.
const SCROLL_REGION_RESET: &str = "\u{1b}[r";

/// 뷰포트가 살아 있으려면 화면에 최소한 이만큼은 있어야 한다 — 위쪽에 삽입
/// 영역이 한 줄도 없으면 히스토리를 밀어 넣을 자리가 없다.
pub const MIN_ROWS: u16 = 4;

/// 인라인 뷰포트 painter.
#[derive(Debug)]
pub struct Painter<W: Write> {
    out: W,
    cols: u16,
    rows: u16,
    /// 뷰포트 첫 행(0 기준). 언제나 1 이상이다 — 0 이면 위에 삽입할 자리가 없다.
    top: u16,
    height: u16,
    color: bool,
    /// 마지막으로 실제 방출한 행 페이로드, **화면 절대 행**으로 색인한다
    /// (길이는 `rows`). 안 바뀐 행은 다시 그리지 않는다 — shimmer 프레임이
    /// 초당 30번 도는데 매번 전면 재그리기를 하면 느린 링크에서 눈에 보인다.
    ///
    /// 뷰포트 인덱스가 아니라 절대 행인 것이 핵심이다. 뷰포트 **안에** 줄이
    /// 하나 끼면(상태 행, 도구 셀) 그 아래는 인덱스가 전부 한 칸씩 밀리는데,
    /// 화면에서는 **같은 절대 행에 그대로 있다** — 머리가 바닥에 붙어 있어
    /// 위로 자라기 때문이다. 인덱스로 색인하면 그 전부가 캐시 미스가 된다.
    /// 실측(스크립트 턴 하나, `ZO_PROBE_PAINT`): draw 104 중 43(41%)이 컴포저
    /// 입력 줄 `│› ` 였다 — 아무도 타이핑하지 않는 턴에서.
    ///
    /// 그래서 행을 옮기는 연산은 캐시를 **밀어야** 한다: [`Self::scroll_band`]
    /// 의 영역 스크롤은 머리 위만 위로, [`Self::insert_history`] 의 머리 밀기는
    /// 아래로, [`Self::scroll_screen`] 은 화면 전체를 위로.
    rendered: Vec<String>,
    /// 한 행의 페이로드를 조립하는 자리. 프레임마다·행마다 새로 만들지 않고
    /// 이 버퍼를 비워 다시 쓴다 — 32ms 틱은 아무것도 안 바뀌어도 돌고, 그
    /// 프레임은 뷰포트 행 수만큼 페이로드를 만들어 **전부 버린다**(캐시가
    /// 스킵하므로). 실측: 40행 화면·뷰포트 16 에서 2,479 ns/프레임.
    scratch: String,
    frame: String,
    /// 마지막으로 놓은 커서. 프레임이 아무것도 바꾸지 않았고 커서도 그대로면
    /// 한 바이트도 내보내지 않는다 — 32ms 틱이 유휴에서도 도는데, 매번 동기화
    /// 짝과 CUP 을 흘리면 원격 패인에서 그것만으로 초당 1KB 를 먹는다.
    last_cursor: Option<(u16, u16)>,
    failed: bool,
    /// The popup covering the head, if one is up — see [`Self::set_popup_height`].
    popup: Option<Popup>,
    /// The newest history lines this painter printed — as they were handed
    /// in, before wrapping — newest last, at most [`RING_LINES`] of them.
    /// Wrapped at the screen's width they are the transcript's rows on
    /// screen: the last `known_above` of those rows stand in that order
    /// directly above the band (or the head, when the band is gone). That is
    /// what lets a popup draw over those rows and put them back afterwards,
    /// and a resize rebuild them at the new width.
    history_tail: VecDeque<Line>,
    /// The line the ring's newest entry came from, held so the rows of one
    /// line arriving in SEPARATE `insert_history` calls — one row per 8 ms
    /// commit tick, since the 120 fps ticker — still fold into one ring entry.
    /// Held as the `Arc` itself, not its address: a dropped line's address can
    /// be reused by the next one, and an address match would then skip a new
    /// line. Before this a two-row paragraph committed across two ticks stood
    /// twice in the ring, and a resize rebuilt it twice on screen (the
    /// pane-resize reflow e2e, red 2 of 3 runs after 490d5bdd).
    last_ring_origin: Option<std::sync::Arc<Line>>,
    /// Rows the ring can vouch for, directly above the band: screen rows
    /// `top - gap - known_above .. top - gap` are the ring's last rows, in
    /// order.
    known_above: u16,
    /// The band: blank rows directly above the head, `top - gap .. top`.
    ///
    /// The head sits on the floor and the transcript fills the screen from
    /// the top, so the rows between the transcript's last row and the head
    /// are blank until history fills them from the top — one row after the
    /// last — a growing head takes them from the bottom, or a shrinking head
    /// gives its own rows back to them. Nothing scrolls while the band has a
    /// row to give; only when it is gone does history scroll the region above
    /// the head, and that is when the transcript's first rows reach
    /// scrollback (a short transcript never does). A blank band scrolled up
    /// instead would stand in the transcript, and in scrollback — the
    /// 2026-09-03 "또 공백" band under a cell that had filled the pane.
    ///
    /// The band never reaches row zero (`top - gap >= 1`): the row above it
    /// is where the walk that writes into it starts (`ESC[12;1H` then
    /// `\r\n` per line, the captured grammar).
    gap: u16,
    /// Whether the transcript has reached the head since the screen was
    /// built: history closed the band, or a growing head scrolled the
    /// region above it. Before that the band is a short transcript's margin
    /// and the head keeps the floor whatever it does — the product owner's
    /// rule (2026-09-06 morning). After it, the rows a shrinking head gives
    /// up are a HOLE, not a margin: left above the head they stood as a break
    /// under the last reply, taller the more commands the finished group had
    /// folded away ("ran 8 commands 가 숫자에 따라 간격이 더 떨어짐",
    /// 2026-09-06). So the head keeps its top instead and the hole goes under
    /// its footer, where the history that follows closes it — pushing the
    /// head down only as far as it needs. Claude Code's spinner row does the
    /// same. A head that covered the screen is the exception (see
    /// [`Self::set_height`]), and it hands the band back as a margin.
    reached: bool,
    /// Rows under the footer that a shrinking head gave up since it last
    /// stood on the floor — never more than the room under it. This is the
    /// room [`Self::settle_on_the_floor`] closes between turns; the room a
    /// popup's scroll left is not (a settle there would open the very void the
    /// model-picker e2e guards), and history pushing the head down or a head
    /// growing in place spends it first.
    slack: u16,
}

/// Lines the ring keeps — several screens, so a resize can refill the rows
/// above the head from it. A line is a few spans; this is kilobytes.
const RING_LINES: usize = 400;

/// Rows a popup may scroll the screen for beyond what the ring can put back —
/// the void it may leave under the head when it closes. Three is the inline
/// chrome the e2e harness allows between a cell and the composer.
const POPUP_SCROLL_ALLOWANCE: u16 = 3;
/// The least a popup is offered whatever the ring knows: the ten rows the
/// captured `/model` surface had (seven chrome rows and a three-row window).
const POPUP_FLOOR_ROWS: u16 = 10;

/// A surface that takes the bottom of the screen for a while — a picker, a
/// dialog, a question — drawn OVER the rows above the head instead of
/// scrolling them away, so closing it puts the screen back as it was.
#[derive(Debug, Clone, Copy)]
struct Popup {
    /// Where the head was, and how tall, before the popup went up.
    base_top: u16,
    base_height: u16,
}

impl<W: Write> Painter<W> {
    /// 화면 크기와 지금 커서 행(0 기준)을 알고 연다.
    ///
    /// The head opens on the floor — with no height yet, the last row — and
    /// the rows from the cursor down to it are the band: the shell left the
    /// cursor on a fresh line, and everything under it is blank. The rows
    /// above the cursor are the shell's, and stay. Before this the head
    /// opened AT the cursor and followed the transcript down, codex's way; a
    /// short transcript left the composer mid-pane with blank rows under it
    /// (the 2026-09-06 screenshot: "pinned to the bottom, like Claude Code").
    pub fn new(out: W, cols: u16, rows: u16, cursor_row: u16, color: bool) -> Self {
        let rows = rows.max(MIN_ROWS);
        let top = rows - 1;
        let first = cursor_row.max(1).min(top);
        Self {
            out,
            cols: cols.max(1),
            rows,
            top,
            height: 0,
            color,
            // 화면 한 장 크기로 열어 둔다. 캐시가 절대 행 색인이므로 길이는
            // 뷰포트가 아니라 **화면**을 따른다 — 예전에는 `set_height` 의
            // resize 가 키워 줬는데, 이제 거기서 자리를 늘리지 않는다.
            rendered: vec![String::from('\u{0}'); rows as usize],
            scratch: String::new(),
            frame: String::new(),
            last_cursor: None,
            failed: false,
            popup: None,
            history_tail: VecDeque::new(),
            last_ring_origin: None,
            known_above: 0,
            gap: top - first,
            reached: top == first,
            slack: 0,
        }
    }

    /// 출력 쓰기가 한 번이라도 실패했는지 — 패인이 닫혔다는 뜻이다.
    pub const fn failed(&self) -> bool {
        self.failed
    }

    pub const fn cols(&self) -> u16 {
        self.cols
    }

    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// 뷰포트가 쓸 수 있는 최대 높이 — 위쪽 삽입 자리 한 줄은 늘 남긴다.
    pub const fn max_height(&self) -> u16 {
        self.rows.saturating_sub(1)
    }

    // ------------------------------------------------------------------
    // 프레임
    // ------------------------------------------------------------------

    /// 프레임을 (아직 안 열렸으면) 연다.
    ///
    /// 여는 일은 방출하는 쪽이 스스로 한다. 호출자가 `begin` 을 잊고 히스토리를
    /// 넣으면 그 바이트가 다음 `begin` 의 `clear` 에 통째로 사라지기 때문이다 —
    /// 실제로 그렇게 유저 셀 하나가 조용히 증발했었다.
    fn open(&mut self) {
        if self.frame.is_empty() {
            self.frame.push_str(SYNC_BEGIN);
        }
    }

    /// 프레임을 닫고 흘려보낸다. `cursor` 가 `None` 이면 커서를 숨긴다
    /// (다이얼로그가 떠 있을 때 — 캡처의 신뢰 다이얼로그 프레임이 그렇다).
    pub fn end(&mut self, cursor: Option<(u16, u16)>) {
        if (self.frame.is_empty() || self.frame.as_str() == SYNC_BEGIN)
            && cursor == self.last_cursor
        {
            self.frame.clear();
            return;
        }
        self.open();
        self.last_cursor = cursor;
        self.frame.push_str(RESET);
        match cursor {
            Some((col, row)) => {
                self.frame.push_str(CURSOR_SHOW);
                cup(&mut self.frame, col, row);
            }
            None => self.frame.push_str(CURSOR_HIDE),
        }
        self.frame.push_str(SYNC_END);
        let payload = std::mem::take(&mut self.frame);
        self.write(&payload);
    }

    /// 아직 흘려보내지 않은 프레임 바이트를 꺼낸다.
    ///
    /// 테스트 이음매다 — 캡처 대조는 터미널에 실제로 쓰지 않고 **한 프레임의
    /// 바이트**를 봐야 하는데, `end` 는 그걸 쓰고 버린다.
    pub fn take_frame(&mut self) -> String {
        std::mem::take(&mut self.frame)
    }

    /// Bytes that are not a cell — a terminal notification (`super::bell`) —
    /// ride the next frame through the one write door, so they never
    /// interleave with a half-written row.
    pub fn emit_raw(&mut self, bytes: &str) {
        if bytes.is_empty() {
            return;
        }
        self.open();
        self.frame.push_str(bytes);
    }

    fn write(&mut self, payload: &str) {
        if payload.is_empty() || self.failed {
            return;
        }
        if self.out.write_all(payload.as_bytes()).is_err() || self.out.flush().is_err() {
            self.failed = true;
        }
    }

    // ------------------------------------------------------------------
    // 기하
    // ------------------------------------------------------------------

    /// The screen changed size.
    ///
    /// The head goes to the new floor, taller or shorter — even from a row
    /// above the OLD floor: Zed can report a new grid while DSR and the live
    /// streaming head still describe the old absolute rows. Preserving those
    /// rows leaves the composer and footer mid-screen with every new row blank
    /// underneath (reproduced at 40→60 rows with a live spinner, 2026-09-05).
    ///
    /// The transcript above is rebuilt from the ring at the new width, from
    /// the row it started on when it fits above the head — a short transcript
    /// stays at the top of a pane that grew — and bottom-aligned when it does
    /// not: the last rows are the ones a person was reading. The band is what
    /// is left between the two. Re-anchoring the head alone left rows wrapped
    /// for the old width ("반응형 창에 맞게 출력되게", 2026-09-03).
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.open();
        // A popup up at this moment is closed on paper only — its rows are
        // repainted by the invalidation below.
        let head_top = self.popup.take().map_or(self.top, |popup| popup.base_top);
        let old_rows = self.rows;
        let old_cols = self.cols;
        let old_top = self.top;
        // Where the transcript starts: the first row the ring vouches for,
        // or the band's first row when it vouches for none.
        let anchor = head_top
            .saturating_sub(self.gap)
            .saturating_sub(self.known_above);
        self.cols = cols.max(1);
        self.rows = rows.max(MIN_ROWS);
        let height = self.height.min(self.max_height());
        // With no height yet the floor is the last row, as at `new`.
        self.top = self.rows.saturating_sub(height.max(1)).max(1);
        self.height = height;
        if self.rows == old_rows && self.cols == old_cols && self.top == old_top {
            // The size did not change after all: the head repaints, in case
            // the signal meant the terminal redrew.
            cup(&mut self.frame, 0, self.top);
            self.frame.push_str("\u{1b}[J");
            self.invalidate();
            return;
        }
        // codex rebuilds the rows above the viewport from the transcript after
        // a resize; the ring is this painter's transcript. The screen is
        // cleared whole first — a terminal that grew either kept its rows at
        // the top (this window's grid) or pulled scrollback down to the bottom
        // (Zed's), and the rebuild owes nothing to either.
        cup(&mut self.frame, 0, 0);
        self.frame.push_str("\u{1b}[J");
        self.invalidate();
        let tail = self.tail_rows(usize::from(self.top));
        let count = u16::try_from(tail.len()).unwrap_or(u16::MAX).min(self.top);
        let from = if anchor.saturating_add(count) <= self.top {
            anchor
        } else {
            self.top - count
        };
        for (offset, payload) in (0..count).zip(&tail) {
            cup(&mut self.frame, 0, from + offset);
            self.frame.push_str(payload);
        }
        self.known_above = count;
        self.gap = self.top - (from + count).max(1);
        self.reached = self.gap == 0;
        self.slack = 0;
    }

    /// 뷰포트 높이를 `height` 로 맞춘다.
    ///
    /// The head stays on the floor whichever way it goes. Growing, it takes
    /// rows from the band above it first — they are blank, so nothing moves
    /// and nothing is written — and scrolls the region above it only for
    /// what the band cannot give: the transcript's first rows leave for
    /// scrollback, and the head's own rows stay where they are. Shrinking
    /// under a short transcript, it gives the rows at its top back to the
    /// band: they are cleared, nothing scrolls, and the rows under them —
    /// composer, footer — keep their absolute rows, so a status row coming
    /// and going costs one row's paint and moves the transcript not at all.
    ///
    /// Shrinking once the transcript has REACHED it (`reached`), the head
    /// keeps its top and gives the rows up under its footer instead: given
    /// back above, they stood as a hole between the transcript and the head
    /// — eight blank rows after a group of eight commands folded to one
    /// ("ran 8 commands 가 숫자에 따라 간격이 더 떨어짐", 2026-09-06) — and
    /// the transcript cannot come back down to close it. Under the footer
    /// they are the room the next history pushes the head down into, only
    /// as far as it needs ([`Self::insert_history`]).
    ///
    /// A head that covered the screen — a picker, a cell as tall as the pane
    /// — shrinks back to the floor whatever `reached` says, and hands the
    /// band back as a margin: what stood above it went to scrollback when it
    /// grew, so there is no transcript on screen to stay attached to, and
    /// anchored at its top it stayed on row one with the rest of the screen
    /// blank under it ("resume하고 화면이 깨지는 버그", 2026-09-02).
    ///
    /// A head short of the floor — after such a shrink, or a popup that
    /// scrolled the screen for rows the ring could not put back — keeps its
    /// row and grows down toward the floor; the history that follows pushes
    /// it the rest of the way it needs.
    pub fn set_height(&mut self, height: u16) {
        if self.popup.is_some() {
            self.close_popup();
        }
        let height = height.min(self.max_height()).max(1);
        if height == self.height {
            return;
        }
        probe_paint(usize::from(height), false, "HEIGHT-CHANGE");
        self.open();
        let old_height = self.height;
        let on_the_floor = self.top + old_height == self.rows;
        if height < old_height {
            let covered = old_height >= self.max_height();
            if on_the_floor && (!self.reached || covered) {
                // The band is a margin again: the rows the head gives up are
                // blank under a transcript that either never reached it or
                // left the screen while it covered it.
                self.reached = false;
                let given_up = old_height - height;
                let new_top = self.top + given_up;
                for row in self.top..new_top {
                    cup(&mut self.frame, 0, row);
                    self.frame.push_str("\u{1b}[K");
                }
                for row in self
                    .rendered
                    .iter_mut()
                    .take(usize::from(new_top))
                    .skip(usize::from(self.top))
                {
                    *row = Self::unknown_row();
                }
                self.gap += given_up;
                self.top = new_top;
            } else {
                cup(&mut self.frame, 0, self.top + height);
                self.frame.push_str("\u{1b}[J");
                self.forget_from(self.top + height);
                self.slack = self
                    .slack
                    .saturating_add(old_height - height)
                    .min(self.rows - (self.top + height));
            }
            self.height = height;
            return;
        }
        let deficit = (self.top + height).saturating_sub(self.rows);
        if deficit > 0 {
            let eaten = self.gap.min(deficit);
            let scrolled = deficit - eaten;
            self.gap -= eaten;
            if scrolled > 0 {
                self.scroll_band(scrolled);
                // The transcript is at the head now, and its first rows are
                // in scrollback: whatever the head gives up from here is a
                // hole, not a margin.
                self.reached = true;
            }
            // `height <= rows - 1`, so the head's new top is at least row one.
            self.top -= deficit;
            self.known_above = self.known_above.min(self.top - self.gap);
        } else {
            // Short of the floor with room under it: the rows the head takes
            // are cleared and forgotten, and it grows down in place.
            cup(&mut self.frame, 0, self.top + old_height);
            self.frame.push_str("\u{1b}[J");
            self.forget_from(self.top + old_height);
            self.slack = self.slack.saturating_sub(height - old_height);
        }
        self.height = height;
    }

    /// How tall a popup may be before it starts costing rows: the head and
    /// whatever is under it, the band and the transcript rows above it the
    /// ring can put back, and the allowance — never less than the old
    /// ten-row surface, never more than the screen. The view sizes a
    /// picker's window from this.
    #[must_use]
    pub fn popup_budget(&self) -> u16 {
        let (base_top, base_height) = self
            .popup
            .map_or((self.top, self.height), |popup| (popup.base_top, popup.base_height));
        let below = self.rows.saturating_sub(base_top.saturating_add(base_height));
        base_height
            .saturating_add(below)
            .saturating_add(self.gap)
            .saturating_add(self.known_above)
            .saturating_add(POPUP_SCROLL_ALLOWANCE)
            .max(POPUP_FLOOR_ROWS)
            .min(self.max_height())
    }

    /// Size the head for a popup — a picker, a dialog, a question — that takes
    /// the bottom of the screen for a while.
    ///
    /// The popup is drawn OVER the rows above the head instead of scrolling
    /// them into scrollback for its room, and [`Self::set_height`] puts those
    /// rows back when it closes — the band blank, the transcript from the
    /// ring of history this painter printed — so the screen is exactly what
    /// it was. That is why a popup can be as tall as the screen at no lasting
    /// cost: the old three-row `/model` window existed because every row a
    /// popup scrolled away stayed blank after it closed ("자꾸 빈공백이
    /// 발생해", 2026-09-02).
    ///
    /// Only rows the ring remembers can be covered. A row it does not know —
    /// the shell's rows above a fresh session, the screen was resized and
    /// history has not scrolled through since — may hold something the
    /// person can see, and covering it would erase what nothing could
    /// redraw; that popup takes the old road instead, and scrolls for its
    /// room.
    pub fn set_popup_height(&mut self, height: u16) {
        let height = height.min(self.max_height()).max(1);
        let new_top = self.rows.saturating_sub(height).max(1);
        let mut base = self
            .popup
            .unwrap_or(Popup { base_top: self.top, base_height: self.height });
        self.open();
        // Rows above the head the ring cannot put back are scrolled away for
        // the room instead — the same road every growth took before, and the
        // void it leaves when the popup closes is exactly that many rows.
        let excess = base
            .base_top
            .saturating_sub(new_top)
            .saturating_sub(self.gap)
            .saturating_sub(self.known_above);
        if excess > 0 {
            probe_paint(usize::from(excess), false, "POPUP-SCROLL");
            self.scroll_screen(excess);
            base.base_top = base.base_top.saturating_sub(excess).max(1);
            self.top = self.top.saturating_sub(excess).max(1);
            self.known_above = self
                .known_above
                .min(base.base_top.saturating_sub(self.gap));
        }
        self.popup = Some(base);
        if (self.top, self.height) == (new_top, height) {
            return;
        }
        probe_paint(usize::from(height), false, "POPUP-HEIGHT");
        // Rows a shorter popup uncovers are given back at once …
        if new_top > self.top {
            self.restore_rows(self.top, new_top.min(base.base_top), base.base_top);
        }
        // … and rows of the base viewport it no longer draws over are cleared,
        // so nothing of the old composer shows between the two.
        for row in base.base_top.max(self.top)..new_top {
            cup(&mut self.frame, 0, row);
            self.frame.push_str("\u{1b}[K");
        }
        // Rows newly covered are only forgotten by the cache: the popup
        // repaints them, and what they held is in the ring.
        let forget_to = self.top.max(new_top);
        for row in self
            .rendered
            .iter_mut()
            .take(usize::from(forget_to))
            .skip(usize::from(new_top))
        {
            *row = Self::unknown_row();
        }
        for row in self
            .rendered
            .iter_mut()
            .take(usize::from(new_top.max(base.base_top)))
            .skip(usize::from(base.base_top.max(self.top)))
        {
            *row = Self::unknown_row();
        }
        self.top = new_top;
        self.height = height;
    }

    /// Put the screen back the way it was before the popup: the rows it
    /// covered are reprinted — the band blank, the transcript from the ring —
    /// the head returns to where it was, and whatever the popup drew below
    /// the head's old extent is cleared.
    fn close_popup(&mut self) {
        let Some(base) = self.popup.take() else {
            return;
        };
        self.open();
        let covered_to = base.base_top.min(self.rows);
        if self.top < covered_to {
            self.restore_rows(self.top, covered_to, base.base_top);
        }
        self.top = base.base_top;
        self.height = base.base_height;
        // The base viewport was drawn over: every row of it repaints next
        // frame, and what the popup left under it goes.
        for row in self.rendered.iter_mut().skip(usize::from(self.top)) {
            *row = Self::unknown_row();
        }
        let below = self.top + self.height;
        if below < self.rows {
            cup(&mut self.frame, 0, below);
            self.frame.push_str("\u{1b}[J");
        }
    }

    /// Reprint rows `from..to` — screen rows above `base_top`, the head's
    /// row — as they were: the band's rows blank, the rows above it from the
    /// ring; a row the ring does not reach is left blank.
    fn restore_rows(&mut self, from: u16, to: u16, base_top: u16) {
        let transcript_bottom = base_top.saturating_sub(self.gap);
        let tail = self.tail_rows(usize::from(self.known_above));
        for row in from..to {
            let payload = (row < transcript_bottom)
                .then(|| {
                    let back = usize::from(transcript_bottom - row);
                    (back <= usize::from(self.known_above))
                        .then(|| tail.len().checked_sub(back).and_then(|index| tail.get(index)))
                        .flatten()
                })
                .flatten();
            cup(&mut self.frame, 0, row);
            match payload {
                Some(payload) => self.frame.push_str(payload),
                None => self.frame.push_str("\u{1b}[0m\u{1b}[K"),
            }
            if let Some(slot) = self.rendered.get_mut(usize::from(row)) {
                *slot = Self::unknown_row();
            }
        }
    }

    fn invalidate(&mut self) {
        self.rendered.clear();
        self.rendered
            .resize(self.rows as usize, Self::unknown_row());
    }

    /// "아직 안 그림" 표식. 빈 문자열이면 **빈 줄을 그렸다**와 구별되지 않는다 —
    /// 실제 페이로드는 언제나 `ESC[0m ESC[K` 로 시작하므로 NUL 하나면 족하다.
    fn unknown_row() -> String {
        String::from('\u{0}')
    }

    /// 캐시를 화면 절대 좌표로 `delta` 행만큼 민다(양수면 아래로). 밀려 들어온
    /// 자리는 "안 그림"이 된다 — 화면에서도 새로 드러난 자리다.
    fn shift_cache(&mut self, delta: i32) {
        if delta == 0 || self.rendered.is_empty() {
            return;
        }
        let len = self.rendered.len();
        let step = delta.unsigned_abs() as usize;
        if step >= len {
            self.invalidate();
            return;
        }
        if delta > 0 {
            self.rendered.rotate_right(step);
            for row in &mut self.rendered[..step] {
                *row = Self::unknown_row();
            }
        } else {
            self.rendered.rotate_left(step);
            for row in &mut self.rendered[len - step..] {
                *row = Self::unknown_row();
            }
        }
    }

    /// Push the head `rows` down toward the floor: the region from its first
    /// row to the screen's last scrolls DOWN — the captured grammar
    /// (`ESC[13;40r ESC[13;1H ESC M …`) — and the rows that open above it
    /// join the band. The cache follows, being absolute rows.
    fn push_head_down(&mut self, rows: u16) {
        if rows == 0 {
            return;
        }
        set_scroll_region(&mut self.frame, self.top + 1, self.rows);
        cup(&mut self.frame, 0, self.top);
        for _ in 0..rows {
            self.frame.push_str(REVERSE_INDEX);
        }
        self.frame.push_str(SCROLL_REGION_RESET);
        self.top += rows;
        self.gap += rows;
        self.slack = self.slack.saturating_sub(rows);
        self.shift_cache(i32::from(rows));
    }

    /// Between turns the head goes back onto the floor.
    ///
    /// Mid-turn a shrinking head keeps its top (`reached`) so no hole opens
    /// above it; the rows it gave up wait under its footer. Once nobody is
    /// working, those rows become the band above the head instead — a blank
    /// row or two over the composer, the spacing an idle screen has anyway —
    /// so the composer rests on the last row whenever the turn is over: the
    /// rule of 2026-09-06 morning, and what the fresh-session e2es pin.
    ///
    /// Only the rows a shrink gave up (`slack`) are closed this way. Room a
    /// popup's scroll left under the head is NOT: the transcript's last rows
    /// stand directly above that head, and pushing it down would open a blank
    /// band between them — the vertical void the model-picker e2e guards.
    /// That room is closed by the history that follows, as before. A popup
    /// on the screen keeps its base; it settles when it closes.
    pub fn settle_on_the_floor(&mut self) {
        if self.popup.is_some() || self.height == 0 || self.slack == 0 {
            return;
        }
        let room = self.rows.saturating_sub(self.top + self.height);
        let rows = room.min(self.slack);
        if rows == 0 {
            return;
        }
        self.open();
        self.push_head_down(rows);
    }

    /// Scroll the region above the head — screen rows `0..top` — up by
    /// `rows`, for a head growing past the floor once the band is gone. The
    /// region's top margin is the first screen row, so what leaves it is what
    /// the terminal keeps in scrollback: the transcript's first rows. The
    /// rows directly above the head come up blank for the head to take, and
    /// the head's own rows do not move — composer and footer keep their
    /// absolute rows, and the cache keeps them. The head is at least two
    /// rows down when it can still grow, so the region is never one row.
    fn scroll_band(&mut self, rows: u16) {
        set_scroll_region(&mut self.frame, 1, self.top);
        cup(&mut self.frame, 0, self.top - 1);
        for _ in 0..rows {
            self.frame.push('\n');
        }
        self.frame.push_str(SCROLL_REGION_RESET);
        let end = usize::from(self.top).min(self.rendered.len());
        let by = usize::from(rows).min(end);
        if by == 0 {
            return;
        }
        self.rendered[..end].rotate_left(by);
        for row in &mut self.rendered[end - by..end] {
            *row = Self::unknown_row();
        }
    }

    /// Scroll the whole screen up by `rows` — for a popup taking rows the
    /// ring cannot put back. Everything on screen moves up together: the
    /// cache follows, and the band above the head keeps its size.
    fn scroll_screen(&mut self, rows: u16) {
        cup(&mut self.frame, 0, self.rows.saturating_sub(1));
        for _ in 0..rows {
            self.frame.push('\n');
        }
        self.shift_cache(-i32::from(rows));
    }

    /// `from` 행부터 화면 끝까지를 "안 그림" 으로 표시한다 — `ESC[J` 로 실제로
    /// 지운 구간과 짝을 이룬다.
    fn forget_from(&mut self, from: u16) {
        let from = from as usize;
        if from >= self.rendered.len() {
            return;
        }
        for row in &mut self.rendered[from..] {
            *row = Self::unknown_row();
        }
    }

    // ------------------------------------------------------------------
    // 히스토리 삽입
    // ------------------------------------------------------------------

    /// 완성된 줄들을 뷰포트 **위**에 넣는다 — 띠를 위에서부터 채우고, 띠가
    /// 다 차야 스크롤백으로 민다.
    pub fn insert_history(&mut self, lines: &[Line]) {
        let wrapped = self.wrap_for_screen(lines);
        self.open();
        let Ok(count) = u16::try_from(wrapped.len()) else {
            return;
        };
        if count == 0 || self.height == 0 {
            return;
        }
        // History goes under a popup, never into it: the popup comes down
        // first and the next frame draws it again over the new rows.
        if self.popup.is_some() {
            self.close_popup();
        }
        // History arriving while a full-screen viewport is up — a picker just
        // closed and the resumed transcript is being replayed before the next
        // frame has sized the composer — has nowhere to go: the region above
        // the head is one row, and every line but the last would be lost
        // ("resume 내용이 안보이네", 2026-09-02). Fold the viewport to one row on
        // the floor first (the shrink above gives the rows to the band); the
        // next frame grows it back from the bottom, taking them back.
        if self.height >= self.max_height() {
            self.set_height(1);
        }
        // A head short of the floor — a popup scrolled the screen for rows
        // the ring could not put back — is pushed down onto it first: the
        // region from its first row to the screen's last scrolls DOWN, the
        // captured grammar (`ESC[13;40r ESC[13;1H ESC M …`), and the rows
        // that open above it join the band.
        let bottom = self.top + self.height;
        if bottom < self.rows {
            // Under a transcript that reached the head, the room under the
            // footer is the rows a shrink gave up: the head goes down only as
            // far as these lines need, so no row reopens above it as a hole.
            let room = self.rows - bottom;
            let fresh = if self.reached { room.min(count) } else { room };
            self.push_head_down(fresh);
        }
        let mut rest = wrapped.iter();
        // 1. The band is filled from its first row: the cursor starts on the
        //    row above it and walks down — `ESC[1;14r ESC[12;1H`, then `\r\n`
        //    before each line, the captured grammar — and nothing scrolls.
        //    The band never reaches row zero, so there is always a row to
        //    start on above it.
        let walk = count.min(self.gap);
        if walk > 0 {
            let band_top = self.top - self.gap;
            set_scroll_region(&mut self.frame, 1, self.top);
            cup(&mut self.frame, 0, band_top - 1);
            for line in rest.by_ref().take(usize::from(walk)) {
                self.frame.push_str("\r\n");
                self.history_line(line);
            }
            self.gap -= walk;
            self.known_above = self.known_above.saturating_add(walk);
        }
        // 2. What is left scrolls the region above the head — the band is
        //    gone, so the rows directly above the head are the transcript's
        //    last. 상단 마진이 1행이라 여기서 밀려난 줄은 터미널이 스크롤백에
        //    저장한다 — 히스토리가 남는 이유고, 밀려나는 것은 언제나
        //    트랜스크립트의 첫 행이지 빈 띠가 아니다.
        if !rest.as_slice().is_empty() {
            set_scroll_region(&mut self.frame, 1, self.top);
            cup(&mut self.frame, 0, self.top - 1);
            for line in rest {
                self.frame.push_str("\r\n");
                self.history_line(line);
            }
            self.known_above = self.known_above.saturating_add(count - walk);
        }
        if self.gap == 0 {
            self.reached = true;
        }
        self.frame.push_str(SCROLL_REGION_RESET);
        // The ring keeps the LINE a row came from, once, not the rows cut
        // from it for this width — that is what lets a resize rebuild the
        // rows above the head at the new width instead of reprinting rows
        // wrapped for the old one ("반응형 창에 맞게 출력되게", 2026-09-03).
        for line in lines {
            let kept = if let Some(origin) = &line.origin {
                if self
                    .last_ring_origin
                    .as_ref()
                    .is_some_and(|last| std::sync::Arc::ptr_eq(last, origin))
                {
                    continue;
                }
                self.last_ring_origin = Some(std::sync::Arc::clone(origin));
                Line::clone(origin)
            } else {
                self.last_ring_origin = None;
                line.clone()
            };
            if self.history_tail.len() >= RING_LINES {
                self.history_tail.pop_front();
            }
            self.history_tail.push_back(kept);
        }
        // 머리 위는 히스토리가 썼거나 위쪽 스크롤 영역이 통째로 밀어 올렸다 —
        // 뷰포트가 그리는 자리는 아니지만 캐시가 화면 전체를 담으므로 낡은
        // 값을 남기지 않는다.
        for row in self.rendered.iter_mut().take(self.top as usize) {
            *row = Self::unknown_row();
        }
        // The rows the ring can vouch for, directly above the band: at most
        // every row from the top of the screen down to it.
        self.known_above = self.known_above.min(self.top - self.gap);
    }

    /// 히스토리 한 행. [`Line::lead`]·[`Line::trail`] 의 제어 바이트는 여기서만
    /// 흘러간다 — 접기 마커(OSC 7788)는 **스크롤백에 커밋되는 행**의 속성이지
    /// 매 프레임 다시 그리는 뷰포트 행의 것이 아니다.
    fn history_line(&mut self, line: &Line) {
        if let Some(lead) = line.lead.as_deref() {
            self.frame.push_str(lead);
        }
        let shown = self.render_row(line);
        self.frame.push_str(&shown);
        if let Some(trail) = line.trail.as_deref() {
            self.frame.push_str(trail);
        }
    }

    /// One wrapped history row's visual bytes — what the row's commit printed
    /// between its lead and trail, and what a reprint of it prints again. A
    /// fold marker is a property of the row's one commit to scrollback, not of
    /// a row put back under a closing popup or rebuilt after a resize.
    fn render_row(&self, line: &Line) -> String {
        let mut shown = String::new();
        if self.color {
            // 행 머리의 색은 **줄 스타일**의 것이다 — codex
            // `write_history_line` 은 `SetColors(line.style.fg, …)` 를 `ESC[K`
            // 앞에 낸다. 색 없는 줄이면 지금까지처럼 `ESC[39;49m` 이다.
            shown.push_str(&super::ansi::line_lead(line));
            shown.push_str("\u{1b}[K");
            write_spans(line, &mut shown);
            shown.push_str("\u{1b}[39m\u{1b}[49m\u{1b}[0m");
        } else {
            shown.push_str("\u{1b}[K");
            write_plain(line, &mut shown);
        }
        shown
    }

    /// The last `rows` rows above the head as the ring prints them at the
    /// current width — oldest first, fewer when the ring does not reach.
    fn tail_rows(&self, rows: usize) -> Vec<String> {
        let mut newest_first: Vec<String> = Vec::new();
        for line in self.history_tail.iter().rev() {
            if newest_first.len() >= rows {
                break;
            }
            for row in self.wrap_for_screen(std::slice::from_ref(line)).iter().rev() {
                newest_first.push(self.render_row(row));
            }
        }
        newest_first.truncate(rows);
        newest_first.reverse();
        newest_first
    }

    fn wrap_for_screen(&self, lines: &[Line]) -> Vec<Line> {
        let width = self.cols as usize;
        lines
            .iter()
            .flat_map(|line| {
                if line.width() <= width {
                    vec![line.clone()]
                } else {
                    // A line's continuation rows start with its own hanging
                    // indent — the same wrap the cell gave it.
                    let hanging = line
                        .continuation
                        .clone()
                        .unwrap_or_else(|| super::ansi::Span::raw(""));
                    wrap_line(line, width, &hanging)
                }
            })
            .collect()
    }

    // ------------------------------------------------------------------
    // 뷰포트
    // ------------------------------------------------------------------

    /// 뷰포트를 그린다. `rows` 길이는 [`Painter::set_height`] 로 맞춰 둔 높이여야
    /// 한다 — 넘치면 잘리고 모자라면 빈 줄로 채운다.
    pub fn paint(&mut self, rows: &[Line]) {
        self.open();
        // 루프 밖에 한 번. 안에 두면 행마다 새로 세운다.
        let blank = Line::empty();
        // 버퍼를 self 에서 빌려 나온다 — 안에서 `self.rendered` 를 함께
        // 빌려야 하므로, 소유를 잠시 옮기는 쪽이 분리 빌림보다 읽기 쉽다.
        let mut payload = std::mem::take(&mut self.scratch);
        for index in 0..self.height as usize {
            let line = rows.get(index).unwrap_or(&blank);
            payload.clear();
            payload.push_str("\u{1b}[0m\u{1b}[K");
            if self.color {
                write_spans(line, &mut payload);
            } else {
                write_plain(line, &mut payload);
            }
            payload.push_str(RESET);
            // 캐시는 화면 절대 행으로 본다 — 뷰포트 안에서 인덱스가 밀렸을
            // 뿐인 행은 화면의 같은 자리에 이미 옳게 그려져 있다.
            let row = self.top as usize + index;
            let skipped = self.rendered.get(row).is_some_and(|old| *old == payload);
            probe_paint(index, skipped, &payload);
            if skipped {
                continue;
            }
            #[allow(clippy::cast_possible_truncation)] // height 는 u16 이다.
            cup(&mut self.frame, 0, self.top + index as u16);
            self.frame.push_str(&payload);
            if let Some(slot) = self.rendered.get_mut(row) {
                // 슬롯의 할당도 재사용한다 — 대입은 버리고 새로 잡는다.
                slot.clear();
                slot.push_str(&payload);
            }
        }
        self.scratch = payload;
    }

    /// 뷰포트 안 `(col, row)` 상대 좌표를 화면 절대 좌표로.
    pub const fn absolute(&self, col: u16, row: u16) -> (u16, u16) {
        (col, self.top + row)
    }

    /// `/clear` — visible screen and scrollback are one terminal operation.
    ///
    /// The sequence and order match Codex's inline terminal backend. Queued
    /// paint bytes and every cached row are discarded first, so no pre-clear
    /// transcript can be flushed or restored after the purge.
    pub fn clear_terminal(&mut self) {
        self.frame.clear();
        self.open();
        self.frame
            .push_str("\u{1b}[r\u{1b}[0m\u{1b}[H\u{1b}[2J\u{1b}[3J\u{1b}[H");
        // The head stays on the floor; the whole screen above it is the band.
        self.top = self.rows.saturating_sub(self.height.max(1)).max(1);
        self.gap = self.top - 1;
        self.reached = false;
        self.slack = 0;
        self.rendered.fill(String::from('\u{0}'));
        self.last_cursor = None;
        self.popup = None;
        self.history_tail.clear();
        self.last_ring_origin = None;
        self.known_above = 0;
    }

    /// 종료 정리 — 스크롤 영역을 풀고 띠와 뷰포트를 지운 뒤 커서를
    /// 트랜스크립트 바로 아래에 둔다: 셸의 프롬프트가 마지막 행 다음에 온다.
    pub fn leave(&mut self) {
        let mut payload = String::new();
        payload.push_str(SCROLL_REGION_RESET);
        let from = self.popup.map_or(self.top - self.gap, |popup| {
            self.top.min(popup.base_top.saturating_sub(self.gap))
        });
        cup(&mut payload, 0, from);
        payload.push_str("\u{1b}[J");
        payload.push_str(RESET);
        payload.push_str("\u{1b}[?25h");
        self.write(&payload);
    }
}

/// Per-row paint decisions, appended to the file `ZO_PROBE_PAINT` names.
///
/// Off unless that variable is set, and the lookup happens ONCE — this sits
/// inside the per-row loop of every frame, so an `env::var` here would be a
/// real cost paid by every session to serve a measurement nobody asked for.
///
/// What it answers: whether a repaint is the cache failing or the row genuinely
/// changing. Measured 2026-08-29 on a scripted turn: 147 draws against 1,335
/// skips — the cache is fine, and the draws cluster on indices whose CONTENT
/// changes identity between frames (the composer sits at index 3 for a while,
/// then the shimmer does). Rows are shifting position, not churning.
fn probe_paint(index: usize, skipped: bool, payload: &str) {
    use std::io::Write as _;
    use std::sync::OnceLock;
    static PATH: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let Some(path) = PATH
        .get_or_init(|| {
            std::env::var_os("ZO_PROBE_PAINT")
                .filter(|value| !value.is_empty())
                .map(std::path::PathBuf::from)
        })
        .as_ref()
    else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let head: String = payload.chars().take(400).collect();
        let _ = writeln!(
            file,
            "{} idx={index} {:?}",
            if skipped { "skip" } else { "draw" },
            head.replace('\u{1b}', "^")
        );
    }
}

/// `ESC[{row+1};{col+1}H` — 0 기준 좌표를 1 기준 CUP 로.
fn cup(out: &mut String, col: u16, row: u16) {
    use std::fmt::Write as _;
    let _ = write!(out, "\u{1b}[{};{}H", row + 1, col + 1);
}

/// `ESC[{top};{bottom}r` — codex `SetScrollRegion(Range)` 과 같은 표기다
/// (Range 의 start/end 를 그대로 쓴다).
fn set_scroll_region(out: &mut String, top: u16, bottom: u16) {
    use std::fmt::Write as _;
    let _ = write!(out, "\u{1b}[{top};{bottom}r");
}

#[cfg(test)]
mod tests {
    use super::{Painter, SYNC_BEGIN};
    use crate::tui::ansi::{Line, Span, Style};

    fn painter(rows: u16, cursor_row: u16) -> Painter<Vec<u8>> {
        Painter::new(Vec::new(), 40, rows, cursor_row, true)
    }

    /// 유휴 프레임 하나가 얼마나 드는가.
    ///
    /// ```sh
    /// cargo test -p zo-ide --lib idle_frame_cost -- --ignored --nocapture
    /// ```
    ///
    /// 32ms 틱은 아무것도 안 바뀌어도 돈다. 그 프레임은 **한 바이트도** 안
    /// 내보내는데(캐시가 전부 스킵), 그 답을 얻기까지 뷰포트 행마다 페이로드를
    /// 새로 만들어 비교하고 버린다. 초당 31프레임 × 뷰포트 높이만큼이다.
    #[ignore = "measurement, not a contract"]
    #[test]
    fn idle_frame_cost() {
        use super::super::ansi::{Line, Span};
        const FRAMES: usize = 20_000;
        let mut painter = painter(40, 20);
        painter.set_height(16);
        let rows: Vec<Line> = (0..16)
            .map(|i| {
                Line::new(vec![
                    Span::raw("  "),
                    Span::dim(format!("row {i} of a settled viewport")),
                ])
            })
            .collect();
        // 첫 프레임이 캐시를 채운다 — 재는 것은 그 뒤의 유휴다.
        painter.paint(&rows);
        let _ = drain(&mut painter);

        let started = std::time::Instant::now();
        for _ in 0..FRAMES {
            painter.paint(&rows);
        }
        let elapsed = started.elapsed();
        let per_frame = elapsed.as_nanos() / FRAMES as u128;
        eprintln!(
            "[idle] {FRAMES} frames · {elapsed:?} · {per_frame} ns/frame · \
             {} ns/row",
            per_frame / 16
        );
    }

    fn drain(painter: &mut Painter<Vec<u8>>) -> String {
        painter.end(None);
        // `end` 가 이미 흘려보냈다 — 버퍼를 통째로 꺼내 본다.
        String::from_utf8(std::mem::take(&mut painter.out)).expect("utf-8")
    }

    #[test]
    fn clear_terminal_purges_scrollback_and_forgets_the_old_screen() {
        let mut painter = painter(40, 12);
        painter.set_height(7);
        painter.paint(&[Line::from_text("old viewport")]);
        let _ = painter.take_frame();
        painter.insert_history(&[Line::from_text("old history")]);
        let _ = painter.take_frame();
        assert!(!painter.history_tail.is_empty());
        assert!(painter.known_above > 0);

        painter.clear_terminal();

        assert_eq!(
            painter.frame,
            format!("{SYNC_BEGIN}\u{1b}[r\u{1b}[0m\u{1b}[H\u{1b}[2J\u{1b}[3J\u{1b}[H")
        );
        assert_eq!((painter.top, painter.gap), (33, 32), "the head is on the floor, the screen above it is the band");
        assert!(painter.popup.is_none());
        assert!(painter.history_tail.is_empty());
        assert_eq!(painter.known_above, 0);
        assert!(painter.rendered.iter().all(|row| row == "\u{0}"));
    }

    /// A height change that does not move the viewport's head keeps the rows
    /// that survive it.
    ///
    /// A head short of the floor — a popup scrolled the screen for rows the
    /// ring could not put back — grows down in place, and every surviving
    /// row is at the SAME absolute coordinate, already drawn correctly.
    /// Erasing the whole viewport and dropping the cache — which is what this
    /// did — repaints rows the terminal is already showing. Measured on a
    /// scripted turn: the height oscillated 5→7→9→12→9→16→9→7, and blanket
    /// invalidation charged 74 row draws for it out of 147 in the whole turn.
    #[test]
    fn growing_in_place_keeps_the_rows_that_survive() {
        let mut painter = Painter::new(Vec::new(), 120, 40, 22, true);
        painter.set_height(4);
        let _ = drain(&mut painter);
        painter.set_popup_height(39);
        let _ = drain(&mut painter);
        painter.set_height(4);
        let _ = drain(&mut painter);
        assert_eq!(painter.top, 15, "precondition: the popup left the head short of the floor");
        let four: Vec<Line> = (0..4).map(|i| Line::from_text(format!("row {i}"))).collect();
        painter.paint(&four);
        let _ = painter.take_frame();
        let top_before = painter.top;

        painter.set_height(6);
        assert_eq!(painter.top, top_before, "precondition: the head did not move");
        let _ = painter.take_frame();

        let six: Vec<Line> = (0..6).map(|i| Line::from_text(format!("row {i}"))).collect();
        painter.paint(&six);
        let frame = painter.take_frame();
        let drawn = frame.matches("\u{1b}[K").count();
        assert_eq!(
            drawn, 2,
            "only the two NEW rows may draw; the four that survived are already \
             on screen at the same coordinates: {frame:?}"
        );
    }

    /// Growing on the FLOOR keeps the cache too — the band gone, the region
    /// above the head scrolls for the rows, and the head's own rows do not
    /// move: composer and footer stay on their absolute rows.
    ///
    /// This is the common case, not an edge one: the viewport lives on the
    /// floor, so nearly every growth takes this path. Treating it as
    /// "everything moved" is what invalidated the cache on almost every height
    /// change.
    #[test]
    fn growing_at_the_bottom_keeps_the_rows_the_scroll_carried() {
        let mut painter = Painter::new(Vec::new(), 120, 12, 8, true);
        painter.set_height(3);
        painter.insert_history(&[Line::from_text("r1")]);
        assert_eq!((painter.top, painter.gap), (9, 0), "precondition: the band is gone");
        let three = lines(["composer 0", "composer 1", "footer"]);
        painter.paint(&three);
        let _ = painter.take_frame();
        let top_before = painter.top;

        painter.set_height(6);
        assert!(painter.top < top_before, "precondition: the head grew up the screen");
        let frame = painter.take_frame();
        assert!(
            frame.contains("\u{1b}[1;9r\u{1b}[9;1H\n\n\n\u{1b}[r"),
            "the region above the head scrolled by three: {frame:?}"
        );

        let six = lines(["cell 0", "cell 1", "cell 2", "composer 0", "composer 1", "footer"]);
        painter.paint(&six);
        let frame = painter.take_frame();
        assert_eq!(
            frame.matches("\u{1b}[K").count(),
            3,
            "only the three NEW rows may draw: {frame:?}"
        );
    }

    /// The head can ALWAYS reach the floor, at every allowed height.
    ///
    /// `set_height` keeps the cache only while the head moved up by exactly
    /// the rows the band gave and the region above it scrolled. `top` has a
    /// floor of 1, so in principle the screen could scroll further than the
    /// head can follow — and then nothing is where the cache says. This walks
    /// every height `max_height()` permits, from every starting cursor row,
    /// and shows that case never arises: the cap is what makes it
    /// unreachable.
    ///
    /// It is a property, not a formality. If `max_height` ever loosens, the
    /// defensive invalidation in `set_height` becomes live again — and this
    /// test is what says so instead of a garbled screen.
    #[test]
    fn every_allowed_height_lets_the_head_follow_the_scroll() {
        for rows in [8u16, 12, 24, 40] {
            for start_top in 1..rows {
                let mut painter = Painter::new(Vec::new(), 80, rows, start_top, false);
                let cap = painter.max_height();
                for height in 1..=cap {
                    let before = painter.top;
                    let previous = painter.height;
                    painter.set_height(height);
                    let scrolled = if before + height.max(previous) > rows {
                        before + height - rows
                    } else {
                        0
                    };
                    if height != previous {
                        assert_eq!(
                            before.saturating_sub(painter.top),
                            before.saturating_sub(before.saturating_sub(scrolled).max(1)),
                            "rows={rows} top={before} height={height}: the head and the \
                             scroll disagree, so the cache would be stale"
                        );
                        assert!(painter.top >= 1, "the head must stay on screen");
                        assert!(
                            painter.top + painter.height <= rows,
                            "the viewport must fit: top={} height={}",
                            painter.top,
                            painter.height
                        );
                    }
                    let _ = painter.take_frame();
                }
            }
        }
    }

    /// After a history insertion, an unchanged viewport must repaint nothing.
    ///
    /// This is the property the whole per-row cache exists for. An insertion
    /// moves the viewport down and rewrites the region ABOVE it; the rows
    /// inside the viewport did not change, so the next paint should be silent.
    #[test]
    fn probe_insert_then_repaint_is_silent() {
        let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
        painter.set_height(6);
        let rows: Vec<Line> = (0..6)
            .map(|i| Line::from_text(format!("row {i}")))
            .collect();
        painter.paint(&rows);
        let first = painter.take_frame();
        assert!(first.contains("\u{1b}[K"), "first paint draws");

        painter.insert_history(&[Line::from_text("committed")]);
        let _ = painter.take_frame();

        painter.paint(&rows);
        let second = painter.take_frame();
        let els = second.matches("\u{1b}[K").count();
        eprintln!("PROBE after-insert repaint ELs = {els}");
        assert_eq!(els, 0, "unchanged viewport repainted: {second:?}");
    }

    /// 캡처(`codex-tui-v0.149.1-turn.bin`, 2.83s 프레임)의 삽입 문법 —
    /// `ESC[1;14r ESC[12;1H \r\n …`: 위쪽만 스크롤 영역으로 묶고, 첫 줄이 갈
    /// 행 바로 위에서 시작해 줄마다 `\r\n` 으로 내려간다. codex 는 그 앞에
    /// 머리를 두 줄 밀지만(`ESC[13;40r ESC[13;1H ESC M ESC M ESC[r`), 여기
    /// 머리는 이미 바닥에 있어 밀 것이 없다 — 밀기 문법은
    /// [`a_head_short_of_the_floor_is_pushed_down_onto_it_by_history`] 가 핀한다.
    #[test]
    fn history_insertion_matches_the_captured_sequence() {
        let mut painter = painter(40, 12);
        painter.set_height(7);
        painter.rendered.clear();
        painter.frame.clear();
        painter.insert_history(&[Line::empty(), Line::from_text("warn")]);
        let frame = painter.frame.clone();
        assert!(
            frame.starts_with("\u{1b}[?2026h\u{1b}[1;33r\u{1b}[12;1H"),
            "frame was {frame:?}"
        );
        assert!(!frame.contains("\u{1b}M"), "a head on the floor is not pushed: {frame:?}");
        assert!(frame.contains("\r\n\u{1b}[39;49m\u{1b}[K\u{1b}[39m\u{1b}[49m\u{1b}[0m"));
        assert!(frame.contains("\r\n\u{1b}[39;49m\u{1b}[Kwarn\u{1b}[39m\u{1b}[49m\u{1b}[0m"));
        assert!(frame.ends_with("\u{1b}[r"));
    }

    #[test]
    fn a_viewport_at_the_screen_bottom_only_scrolls_the_region_above_it() {
        let mut painter = painter(24, 23);
        painter.set_height(5);
        // set_height 가 바닥에 붙였다 — top = 24 - 5 = 19.
        painter.frame.clear();
        painter.insert_history(&[Line::from_text("a")]);
        let frame = painter.frame.clone();
        assert!(!frame.contains("\u{1b}M"), "no reverse index expected: {frame:?}");
        assert!(
            frame.starts_with("\u{1b}[?2026h\u{1b}[1;19r\u{1b}[19;1H"),
            "frame was {frame:?}"
        );
    }

    #[test]
    fn unchanged_viewport_rows_are_not_repainted() {
        let mut painter = painter(24, 10);
        painter.set_height(2);
        painter.paint(&[Line::from_text("a"), Line::from_text("b")]);
        let first = painter.frame.clone();
        assert!(first.contains('a') && first.contains('b'));
        painter.frame.clear();
        painter.paint(&[Line::from_text("a"), Line::from_text("c")]);
        let second = painter.frame.clone();
        assert!(!second.contains('a'));
        assert!(second.contains('c'));
        let _ = drain(&mut painter);
    }

    #[test]
    fn a_frame_is_wrapped_in_the_synchronised_update_pair() {
        let mut painter = painter(24, 10);
        painter.set_height(1);
        painter.paint(&[Line::new(vec![Span::new("x", Style::new().bold())])]);
        painter.end(Some((2, 3)));
        let out = String::from_utf8(std::mem::take(&mut painter.out)).expect("utf-8");
        assert!(out.starts_with("\u{1b}[?2026h"));
        assert!(out.ends_with("\u{1b}[0m\u{1b}[0 q\u{1b}[?25h\u{1b}[4;3H\u{1b}[?2026l"));
    }

    /// 접기 마커는 히스토리 행에만 실리고 뷰포트 행에는 실리지 않는다.
    #[test]
    fn fold_markers_ride_history_rows_only() {
        let mut painter = painter(24, 10);
        painter.set_height(3);
        painter.frame.clear();
        painter.insert_history(&[Line::from_text("head").with_lead("<b>").with_trail("<e>")]);
        assert!(
            painter.frame.contains("<b>\u{1b}[39;49m\u{1b}[Khead"),
            "frame was {:?}",
            painter.frame
        );
        assert!(painter.frame.ends_with("\u{1b}[39m\u{1b}[49m\u{1b}[0m<e>\u{1b}[r"));

        painter.frame.clear();
        painter.paint(&[Line::from_text("head").with_lead("<b>")]);
        assert!(!painter.frame.contains("<b>"), "frame was {:?}", painter.frame);
        let _ = drain(&mut painter);
    }

    #[test]
    fn height_never_eats_the_last_insertion_row() {
        let mut painter = painter(6, 0);
        painter.set_height(99);
        assert_eq!(painter.height, 5);
        assert!(painter.top >= 1);
    }

    /// A full-screen viewport (a picker) shrinking back goes to the FLOOR:
    /// the rows it vacated above are cleared, the head lands at the bottom,
    /// and the history that follows has the whole screen above it again.
    #[test]
    fn a_full_screen_viewport_shrinks_back_to_the_floor() {
        let mut painter = painter(30, 5);
        painter.set_height(99);
        assert_eq!((painter.top, painter.height), (1, 29), "the picker took the screen");
        let _ = drain(&mut painter);
        painter.frame.clear();
        painter.set_height(5);
        assert_eq!(painter.top, 25, "the head is back on the floor");
        assert_eq!(painter.height, 5);
        let frame = painter.frame.clone();
        assert_eq!(
            frame.matches("\u{1b}[K").count(),
            24,
            "every vacated row above is cleared once: {frame:?}"
        );
        assert!(!frame.contains("\u{1b}[J"), "nothing below the head to clear: {frame:?}");
        painter.frame.clear();
        assert_eq!((painter.gap, painter.top - painter.gap), (24, 1), "the vacated rows are the band, from row one");
        painter.insert_history(&[Line::from_text("a")]);
        assert!(
            painter.frame.starts_with("\u{1b}[?2026h\u{1b}[1;25r\u{1b}[1;1H\r\n"),
            "history fills the band's first row, scrolling nothing: {:?}",
            painter.frame
        );
        assert_eq!(painter.gap, 23, "one row of the band is filled");
        let _ = drain(&mut painter);
    }

    /// A popup over rows the ring knows scrolls nothing away: it draws over
    /// them, and closing it prints those rows back from the ring with the
    /// head where it was and nothing of the popup left under it.
    #[test]
    fn a_popup_covers_the_history_it_knows_and_puts_it_back() {
        let mut painter = painter(30, 25);
        painter.set_height(5);
        let lines: Vec<Line> = (1..=10).map(|n| Line::from_text(format!("h{n}"))).collect();
        painter.insert_history(&lines);
        assert_eq!((painter.top, painter.known_above), (25, 10));
        let _ = drain(&mut painter);
        painter.frame.clear();
        painter.set_popup_height(15);
        assert_eq!((painter.top, painter.height), (15, 15), "the popup hugs the floor");
        assert!(!painter.frame.contains('\n'), "nothing was scrolled for its room: {:?}", painter.frame);
        painter.paint(&(0..15).map(|n| Line::from_text(format!("p{n}"))).collect::<Vec<_>>());
        let _ = drain(&mut painter);
        painter.frame.clear();
        painter.set_height(5);
        assert_eq!((painter.top, painter.height), (25, 5), "the head is back where it was");
        let frame = painter.frame.clone();
        for n in 1..=10 {
            assert!(frame.contains(&format!("h{n}")), "h{n} was not put back: {frame:?}");
        }
        assert!(!frame.contains("\u{1b}[J"), "nothing below the head to clear: {frame:?}");
        let _ = drain(&mut painter);
    }

    /// A popup taller than the band the ring can vouch for scrolls the
    /// screen for exactly the rows it cannot put back — a row the ring does
    /// not know may hold something the person can still see — and closing
    /// it leaves the head that many rows short of the floor, no more.
    #[test]
    fn a_popup_scrolls_only_for_the_rows_the_ring_cannot_put_back() {
        let mut painter = painter(30, 25);
        painter.set_height(5);
        let lines: Vec<Line> = (1..=4).map(|n| Line::from_text(format!("h{n}"))).collect();
        painter.insert_history(&lines);
        assert_eq!(painter.known_above, 4);
        let _ = drain(&mut painter);
        painter.frame.clear();
        painter.set_popup_height(15);
        assert_eq!((painter.top, painter.height), (15, 15));
        assert_eq!(painter.frame.matches('\n').count(), 6, "scrolled for the six unknown rows only: {:?}", painter.frame);
        assert!(painter.popup.is_some());
        let _ = drain(&mut painter);
        painter.frame.clear();
        painter.set_height(5);
        assert_eq!((painter.top, painter.height), (19, 5), "six rows short of the floor, the four known ones put back");
        for n in 1..=4 {
            assert!(painter.frame.contains(&format!("h{n}")), "h{n} was not put back: {:?}", painter.frame);
        }
    }

    /// The budget grows with what the ring can put back, never under the
    /// old ten-row surface and never past the screen.
    #[test]
    fn the_popup_budget_follows_the_ring() {
        let mut painter = painter(30, 25);
        painter.set_height(5);
        assert_eq!(painter.popup_budget(), 10, "nothing known: the old surface");
        let lines: Vec<Line> = (1..=12).map(|n| Line::from_text(format!("h{n}"))).collect();
        painter.insert_history(&lines);
        assert_eq!(painter.popup_budget(), 5 + 12 + 3);
        let more: Vec<Line> = (1..=30).map(|n| Line::from_text(format!("m{n}"))).collect();
        painter.insert_history(&more);
        assert_eq!(painter.popup_budget(), 29, "capped at the screen");
        let _ = drain(&mut painter);
    }

    /// The order the app relies on for a finished cell: shrink first, then
    /// insert its lines. The cell had scrolled the transcript up to the head
    /// when it grew, so the shrink keeps the head's top — the rows it gives
    /// up are cleared UNDER it, never a hole above — and the cell's own lines
    /// push the head back down onto the floor by exactly the rows they need.
    /// Nothing above the head scrolls, and no blank row is left anywhere.
    #[test]
    fn history_after_a_shrink_pushes_the_head_back_down_by_what_it_needs() {
        let mut painter = painter(30, 25);
        painter.set_height(20);
        assert_eq!(painter.top, 10, "the cell grew the head up the screen — and scrolled the transcript to it");
        let _ = drain(&mut painter);
        painter.set_height(5);
        let frame = painter.frame.clone();
        assert_eq!((painter.top, painter.gap), (10, 0), "the shrink keeps the head's top: the room is under it");
        assert!(frame.contains("\u{1b}[16;1H\u{1b}[J"), "the rows under the shorter head are cleared: {frame:?}");
        let _ = drain(&mut painter);
        let lines: Vec<Line> = (1..=15).map(|n| Line::from_text(format!("c{n}"))).collect();
        painter.insert_history(&lines);
        let frame = painter.frame.clone();
        assert_eq!(frame.matches("\u{1b}M").count(), 15, "the head comes down by the fifteen rows they need: {frame:?}");
        assert!(
            !frame.replace("\r\n", "").contains('\n'),
            "the region above the head did not scroll: {frame:?}"
        );
        assert_eq!((painter.top, painter.gap), (25, 0), "the cell's own lines stand between the transcript and the head on the floor");
        let _ = drain(&mut painter);
    }


    /// History inserted while the viewport still covers the screen — the
    /// picker closed and the transcript replays before the next frame —
    /// folds the viewport to the floor first, so every line lands and the
    /// next frame's growth carries them up instead of losing them.
    #[test]
    fn history_under_a_full_screen_viewport_folds_it_to_the_floor_first() {
        let mut painter = painter(30, 5);
        painter.set_height(99);
        let _ = drain(&mut painter);
        painter.frame.clear();
        painter.insert_history(&[Line::from_text("one"), Line::from_text("two"), Line::from_text("three")]);
        assert_eq!((painter.top, painter.height), (29, 1), "folded to one row on the floor");
        let frame = painter.frame.clone();
        assert!(
            frame.contains("\u{1b}[1;29r\u{1b}[1;1H\r\n"),
            "the lines fill the band's first three rows: {frame:?}"
        );
        for word in ["one", "two", "three"] {
            assert!(frame.contains(word), "{word} was lost: {frame:?}");
        }
        painter.frame.clear();
        painter.set_height(6);
        assert_eq!(painter.top, 24, "the composer grew back from the floor");
        let _ = drain(&mut painter);
    }

    /// A head on the floor of a screen that grows follows the floor down and
    /// takes the transcript above it along — the rows between the old head
    /// and the new one are the transcript, not a gap. (codex
    /// `update_inline_viewport_for_resize_reflow`: bottom-aligned viewports
    /// follow the floor; the rows above are rebuilt.)
    #[test]
    fn a_head_on_the_floor_follows_it_down_with_its_transcript() {
        let mut painter = painter(18, 1);
        painter.set_height(5);
        let lines: Vec<Line> = (1..=30).map(|n| Line::from_text(format!("h{n}"))).collect();
        painter.insert_history(&lines);
        assert_eq!((painter.top, painter.gap, painter.known_above), (13, 0, 13), "precondition: the transcript filled the band");
        let _ = drain(&mut painter);

        painter.resize(40, 46);
        assert_eq!(painter.top, 41, "the head followed the floor down");
        let frame = painter.take_frame();
        assert!(
            frame.contains("\u{1b}[1;1H\u{1b}[J\u{1b}[1;1H"),
            "the screen is cleared whole before the rebuild:\n{frame:?}"
        );
        // The ring held all thirty lines and the transcript had reached the
        // first screen row, so it stands from row 1 again — h1 on row 1, h30
        // on row 30 — with the band under it and the head on the floor.
        assert_eq!((painter.known_above, painter.gap), (30, 11));
        assert!(frame.contains("\u{1b}[1;1H\u{1b}[39;49m\u{1b}[Kh1\u{1b}[39m"), "{frame:?}");
        assert!(frame.contains("\u{1b}[30;1H\u{1b}[39;49m\u{1b}[Kh30\u{1b}[39m"), "{frame:?}");
        assert!(!frame.contains("\u{1b}M"), "nothing is scrolled — the rows are rebuilt");
    }

    /// A width change re-wraps the transcript at the new width — codex's
    /// resize reflow — instead of leaving rows wrapped for the old one.
    #[test]
    fn a_narrower_screen_rewraps_the_transcript_above_the_head() {
        let mut painter = Painter::new(Vec::new(), 40, 30, 1, true);
        painter.set_height(5);
        painter.insert_history(&[Line::from_text("a".repeat(60))]);
        let _ = drain(&mut painter);
        assert_eq!(painter.known_above, 2, "sixty columns wrapped to two rows at forty");

        painter.resize(20, 30);
        let frame = painter.take_frame();
        assert_eq!(painter.known_above, 3, "and to three rows at twenty");
        assert!(frame.contains(&"a".repeat(20)), "{frame:?}");
        assert!(!frame.contains(&"a".repeat(21)), "a row wider than the screen:\n{frame:?}");
    }

    /// The head is on the floor from its first frame, so the DSR/size
    /// ordering Zed can deliver — a grid reported while the cursor row still
    /// describes the old one — cannot strand the footer above newly added
    /// rows: a grow rebuilds the screen with the head on the new floor.
    #[test]
    fn a_head_above_the_floor_follows_it_when_the_screen_grows() {
        let mut painter = painter(30, 5);
        painter.set_height(5);
        assert_eq!((painter.top, painter.gap), (25, 20), "opened on the floor, the band from the cursor row");
        let _ = drain(&mut painter);
        painter.resize(40, 46);
        assert_eq!((painter.top, painter.gap), (41, 36), "the live head must reach the new floor");
        let frame = painter.take_frame();
        assert!(
            frame.starts_with("\u{1b}[?2026h\u{1b}[1;1H\u{1b}[J"),
            "the screen is rebuilt independently of its grow behavior:\n{frame:?}"
        );
        assert!(!frame.contains("\u{1b}M"), "nothing scrolled:\n{frame:?}");
        let _ = drain(&mut painter);
    }

    /// A head that no longer fits a shrunken screen moves up onto its floor.
    #[test]
    fn a_head_that_no_longer_fits_moves_up_onto_the_floor() {
        let mut painter = painter(46, 40);
        painter.set_height(5);
        assert_eq!(painter.top, 41);
        let _ = drain(&mut painter);
        painter.resize(40, 30);
        assert_eq!(painter.top, 25);
        let _ = drain(&mut painter);
    }

    /// A small terminal for the tests that care where rows END UP rather
    /// than which bytes moved them: the painter's grammar — CUP, EL, ED,
    /// DECSTBM, line feed, reverse index — replayed onto a grid with a
    /// scrollback. A row that leaves a region whose top margin is the first
    /// screen row goes to scrollback; a row leaving any other region is
    /// dropped. That is xterm's rule and this window's grid's.
    struct Screen {
        cols: usize,
        rows: Vec<Vec<char>>,
        scrollback: Vec<String>,
        row: usize,
        col: usize,
        top_margin: usize,
        bottom_margin: usize,
    }

    impl Screen {
        fn new(cols: u16, rows: u16) -> Self {
            let (cols, rows) = (usize::from(cols), usize::from(rows));
            Self {
                cols,
                rows: vec![vec![' '; cols]; rows],
                scrollback: Vec::new(),
                row: 0,
                col: 0,
                top_margin: 0,
                bottom_margin: rows - 1,
            }
        }

        fn feed(&mut self, bytes: &str) {
            let mut chars = bytes.chars();
            while let Some(ch) = chars.next() {
                match ch {
                    '\u{1b}' => self.escape(&mut chars),
                    '\r' => self.col = 0,
                    '\n' => self.line_feed(),
                    ch if ch.is_control() => {}
                    ch => {
                        if self.col < self.cols {
                            self.rows[self.row][self.col] = ch;
                            self.col += 1;
                        }
                    }
                }
            }
        }

        fn escape(&mut self, chars: &mut std::str::Chars<'_>) {
            match chars.next() {
                Some('[') => {
                    let mut params = String::new();
                    let mut final_byte = None;
                    for ch in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&ch) {
                            final_byte = Some(ch);
                            break;
                        }
                        params.push(ch);
                    }
                    self.csi(&params, final_byte);
                }
                Some(']') => {
                    let mut previous = '\0';
                    for ch in chars.by_ref() {
                        if ch == '\u{7}' || (previous == '\u{1b}' && ch == '\\') {
                            break;
                        }
                        previous = ch;
                    }
                }
                Some('M') => self.reverse_index(),
                _ => {}
            }
        }

        fn csi(&mut self, params: &str, final_byte: Option<char>) {
            // Private modes and DECSCUSR move nothing.
            if params.starts_with('?') || params.contains(' ') {
                return;
            }
            let values: Vec<usize> = params
                .split(';')
                .map(|value| value.parse().unwrap_or(0))
                .collect();
            let arg = |index: usize, default: usize| {
                values.get(index).copied().filter(|value| *value > 0).unwrap_or(default)
            };
            let last = self.rows.len() - 1;
            match final_byte {
                Some('H') => {
                    self.row = (arg(0, 1) - 1).min(last);
                    self.col = (arg(1, 1) - 1).min(self.cols - 1);
                }
                Some('K') => match values.first().copied().unwrap_or(0) {
                    0 => self.rows[self.row][self.col..].fill(' '),
                    1 => self.rows[self.row][..=self.col.min(self.cols - 1)].fill(' '),
                    2 => self.rows[self.row].fill(' '),
                    _ => {}
                },
                Some('J') => match values.first().copied().unwrap_or(0) {
                    0 => {
                        self.rows[self.row][self.col..].fill(' ');
                        for row in &mut self.rows[self.row + 1..] {
                            row.fill(' ');
                        }
                    }
                    2 => {
                        for row in &mut self.rows {
                            row.fill(' ');
                        }
                    }
                    3 => self.scrollback.clear(),
                    _ => {}
                },
                Some('r') => {
                    if params.is_empty() {
                        self.top_margin = 0;
                        self.bottom_margin = last;
                    } else {
                        let top = (arg(0, 1) - 1).min(last);
                        let bottom = (arg(1, last + 1) - 1).min(last);
                        if top < bottom {
                            self.top_margin = top;
                            self.bottom_margin = bottom;
                        }
                    }
                    self.row = 0;
                    self.col = 0;
                }
                _ => {}
            }
        }

        fn line_feed(&mut self) {
            if self.row == self.bottom_margin {
                let gone = self.rows.remove(self.top_margin);
                if self.top_margin == 0 {
                    self.scrollback.push(Self::text(&gone));
                }
                self.rows.insert(self.bottom_margin, vec![' '; self.cols]);
            } else if self.row < self.rows.len() - 1 {
                self.row += 1;
            }
        }

        fn reverse_index(&mut self) {
            if self.row == self.top_margin {
                self.rows.remove(self.bottom_margin);
                self.rows.insert(self.top_margin, vec![' '; self.cols]);
            } else if self.row > 0 {
                self.row -= 1;
            }
        }

        fn text(row: &[char]) -> String {
            row.iter().collect::<String>().trim_end().to_owned()
        }

        /// Every row a person can scroll to, oldest first: the scrollback,
        /// then the screen.
        fn transcript(&self) -> Vec<String> {
            self.scrollback
                .iter()
                .cloned()
                .chain(self.rows.iter().map(|row| Self::text(row)))
                .collect()
        }
    }

    fn lines(texts: impl IntoIterator<Item = impl Into<String>>) -> Vec<Line> {
        texts.into_iter().map(Line::from_text).collect()
    }

    /// The rows from `from` to the last row of the head, joined for a message.
    fn shown(transcript: &[String]) -> String {
        transcript.join("\n")
    }

    /// Whether a frame moved rows: a line feed outside a history line's
    /// `\r\n`, or a reverse index.
    fn scrolled(frame: &str) -> bool {
        frame.contains("\u{1b}M") || frame.replace("\r\n", "").contains('\n')
    }

    /// A tool cell tall enough to fill the screen — an edit's diff in a split
    /// pane — hands its lines to history when it finishes, and the head shrinks
    /// back in the same frame (`app::Ui::paint`: size, flush history, size).
    /// The lines must land right under the transcript. The shrink puts the
    /// head on the floor and clears the rows it gave up; scrolling the whole
    /// region above the head for the lines carried that blank band up between
    /// the transcript and the cell — and into scrollback, where it stayed
    /// ("edited 나올때 그러네", 2026-09-03, a 47-line edit in a split pane).
    #[test]
    fn a_cell_that_filled_the_screen_leaves_no_gap_when_its_lines_become_history() {
        let mut painter = painter(24, 1);
        let mut screen = Screen::new(40, 24);
        let head = lines(["head 0", "head 1", "head 2"]);
        painter.set_height(3);
        painter.paint(&head);
        screen.feed(&drain(&mut painter));
        painter.insert_history(&lines(["» fix the bug", "Working on it."]));
        painter.set_height(3);
        painter.paint(&head);
        screen.feed(&drain(&mut painter));

        // The edit's cell streams in and grows the head to the ceiling.
        let cell: Vec<Line> = lines(
            std::iter::once("Edited src/lib.rs (+47 -40)".to_owned())
                .chain((1..=20).map(|n| format!("+ line {n}"))),
        );
        let ceiling = painter.max_height();
        painter.set_height(ceiling);
        let live: Vec<Line> = cell.iter().chain(&head).cloned().collect();
        painter.paint(&live);
        screen.feed(&drain(&mut painter));
        assert_eq!((painter.top, painter.height), (1, ceiling), "precondition: the cell took the screen");

        // It finishes: the head is sized first, the cell's lines follow.
        painter.set_height(3);
        painter.insert_history(&cell);
        painter.set_height(3);
        painter.paint(&head);
        screen.feed(&drain(&mut painter));

        let transcript = screen.transcript();
        let reply = transcript
            .iter()
            .position(|row| row == "Working on it.")
            .expect("the reply is still there");
        let edited = transcript
            .iter()
            .position(|row| row.starts_with("Edited"))
            .expect("the cell landed");
        assert_eq!(
            edited,
            reply + 1,
            "blank rows sit between the reply and the cell:\n{}",
            shown(&transcript)
        );
        let last = transcript
            .iter()
            .rposition(|row| row == "+ line 20")
            .expect("the cell's last line landed");
        assert_eq!(transcript[last + 1], "head 0", "the head is right under the cell:\n{}", shown(&transcript));
        assert!(
            transcript[reply..].iter().all(|row| !row.is_empty()),
            "no blank row from the reply down to the head:\n{}",
            shown(&transcript)
        );
        assert_eq!(painter.top, 21, "the head is on the floor");
    }

    /// A band the transcript did not fill — a short session resumed under a
    /// folded head — is filled by the lines that follow, one row after the
    /// last, and nothing scrolls until it is full; then the region above the
    /// head scrolls and what leaves is the transcript's first row, never a
    /// blank one.
    #[test]
    fn later_lines_fill_the_band_under_the_transcript_before_anything_scrolls() {
        let mut painter = painter(12, 1);
        let mut screen = Screen::new(40, 12);
        painter.set_height(11);
        painter.paint(&lines((0..11).map(|n| format!("surface {n}"))));
        screen.feed(&drain(&mut painter));
        painter.insert_history(&lines(["r1", "r2", "r3"]));
        painter.paint(&lines(["head"]));
        screen.feed(&drain(&mut painter));
        assert_eq!((painter.top, painter.height), (11, 1), "folded to the floor");
        assert_eq!((painter.gap, painter.known_above), (7, 3), "r1..r3 on rows 1..3, the band under them");
        assert_eq!(&screen.transcript()[1..4], &["r1", "r2", "r3"]);

        painter.insert_history(&lines(["r4", "r5"]));
        let frame = painter.frame.clone();
        assert!(
            frame.contains("\u{1b}[1;11r\u{1b}[4;1H\r\n"),
            "the lines start on the row after r3: {frame:?}"
        );
        screen.feed(&drain(&mut painter));
        assert!(
            screen.scrollback.is_empty(),
            "no row was scrolled into scrollback: {:?}",
            screen.scrollback
        );
        let visible = screen.transcript();
        assert_eq!(&visible[1..6], &["r1", "r2", "r3", "r4", "r5"]);
        assert!(visible[6..11].iter().all(String::is_empty), "the band shrank by two: {visible:?}");
        assert_eq!(visible[11], "head");
        assert_eq!((painter.gap, painter.known_above), (5, 5));

        // More than the band holds: the band goes first, the rest scrolls the
        // region above the head as ever, and what leaves the screen is the
        // transcript — into scrollback.
        painter.insert_history(&lines((6..=12).map(|n| format!("r{n}"))));
        screen.feed(&drain(&mut painter));
        assert_eq!(painter.gap, 0, "the band is gone");
        assert_eq!(painter.known_above, 11);
        let transcript = screen.transcript();
        let first = transcript.iter().position(|row| row == "r1").expect("r1 is in scrollback");
        let expected: Vec<String> = (1..=12).map(|n| format!("r{n}")).chain(["head".to_owned()]).collect();
        assert_eq!(&transcript[first..], expected.as_slice(), "{}", shown(&transcript));
    }


    /// The rows of one line reach the ring in separate calls — one row per
    /// commit tick — and still fold into one entry, so a wider screen stands
    /// the paragraph once. Before, the fold only spanned one call: two rows
    /// committed across two ticks made two ring entries, and the rebuild
    /// printed the paragraph twice (the pane-resize reflow e2e after 490d5bdd).
    #[test]
    fn rows_of_one_line_arriving_across_calls_fold_into_one_ring_entry() {
        use super::super::cells::{prefixed, Prefix};
        let mut painter = Painter::new(Vec::new(), 40, 20, 1, true);
        painter.set_height(3);
        let _ = drain(&mut painter);
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
        let rows = prefixed(&[Line::from_text(text)], 40, &Prefix::bullet(), true);
        assert_eq!(rows.len(), 2, "precondition: two rows at forty columns");
        painter.insert_history(&rows[..1]);
        painter.insert_history(&rows[1..]);
        let _ = drain(&mut painter);
        assert_eq!(painter.history_tail.len(), 1, "one line, however its rows arrived");
        assert_eq!(painter.known_above, 2);

        painter.resize(100, 20);
        let frame = painter.take_frame();
        assert_eq!(frame.matches(text).count(), 1, "the paragraph stands once: {frame:?}");

        // A different line follows and is its own entry; a line without an
        // origin resets the fold.
        let next = prefixed(&[Line::from_text("next paragraph")], 40, &Prefix::bullet(), true);
        painter.insert_history(&next);
        painter.insert_history(&lines(["plain"]));
        assert_eq!(painter.history_tail.len(), 3);
    }

    /// A screen that grows rebuilds the rows above the head from the LINES
    /// the ring keeps, wrapped for the new width — a paragraph cut into two
    /// rows for a narrow pane is one row again in a wide one ("반응형 창에
    /// 맞게 출력되게", 2026-09-03). Before, the ring held the rows themselves
    /// and a wider pane showed the same two short rows.
    #[test]
    fn a_wider_screen_rewraps_the_transcript_from_the_lines_it_came_from() {
        use super::super::cells::{prefixed, Prefix};
        let mut painter = Painter::new(Vec::new(), 40, 20, 1, true);
        painter.set_height(3);
        let _ = drain(&mut painter);
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
        let rows = prefixed(&[Line::from_text(text)], 40, &Prefix::bullet(), true);
        assert_eq!(rows.len(), 2, "precondition: two rows at forty columns");
        painter.insert_history(&rows);
        let _ = drain(&mut painter);
        assert_eq!(painter.history_tail.len(), 1, "the ring keeps the line, not its rows");
        assert_eq!(painter.known_above, 2, "two rows stand above the head");

        painter.resize(100, 20);
        let frame = painter.take_frame();
        assert_eq!(
            frame.matches(text).count(),
            1,
            "the paragraph is one row again at a hundred columns: {frame:?}"
        );
        assert_eq!(painter.known_above, 1, "one row of transcript above the head");
    }

    // ------------------------------------------------------------------
    // The head sits on the floor; history fills the band above it from
    // the top (user decision 2026-09-06: "like Claude Code, not codex").
    // ------------------------------------------------------------------

    /// A fresh screen: the head opens on the floor, the band above it is
    /// blank, and nothing scrolls to put it there.
    #[test]
    fn a_head_opens_on_the_floor_of_an_empty_screen() {
        let mut painter = painter(60, 0);
        painter.set_height(5);
        assert_eq!(painter.top, 55, "the head is on the floor: rows - height");
        let frame = painter.take_frame();
        assert!(!frame.contains('\n'), "nothing scrolled for it: {frame:?}");
        assert!(!frame.contains("\u{1b}M"), "nothing was pushed: {frame:?}");
        assert_eq!(painter.gap, 54, "rows 1..55 are the band the transcript will fill");
        assert_eq!(painter.top - painter.gap, 1, "the band starts on row one");
    }

    /// Three committed rows occupy rows 1..3 — the top of the band — with no
    /// blank row among them, and the head is still on the floor with the rest
    /// of the band blank between them.
    #[test]
    fn committed_rows_fill_the_band_from_the_top_and_the_head_stays_on_the_floor() {
        let mut painter = painter(60, 0);
        let mut screen = Screen::new(40, 60);
        painter.set_height(5);
        let head = lines(["head 0", "head 1", "head 2", "head 3", "footer"]);
        painter.paint(&head);
        screen.feed(&drain(&mut painter));
        painter.insert_history(&lines(["r1", "r2", "r3"]));
        let frame = painter.frame.clone();
        assert!(!frame.contains("\u{1b}M"), "the head was not pushed: {frame:?}");
        assert!(
            frame.starts_with("\u{1b}[?2026h\u{1b}[1;55r\u{1b}[1;1H\r\n"),
            "the lines are written from the row above the band: {frame:?}"
        );
        painter.paint(&head);
        screen.feed(&drain(&mut painter));

        assert!(screen.scrollback.is_empty(), "nothing reached scrollback: {:?}", screen.scrollback);
        let visible = screen.transcript();
        assert_eq!(&visible[1..4], &["r1", "r2", "r3"], "{}", shown(&visible));
        assert!(visible[4..55].iter().all(String::is_empty), "the rest of the band is blank: {}", shown(&visible));
        assert_eq!(&visible[55..60], &["head 0", "head 1", "head 2", "head 3", "footer"], "{}", shown(&visible));
        assert_eq!((painter.top, painter.gap, painter.known_above), (55, 51, 3));
    }

    /// The band is filled before anything scrolls: the region above the head
    /// scrolls — and the first rows reach scrollback — only once the band is
    /// gone.
    #[test]
    fn the_region_scrolls_only_once_the_band_is_full() {
        let mut painter = painter(12, 0);
        let mut screen = Screen::new(40, 12);
        painter.set_height(3);
        assert_eq!((painter.top, painter.gap), (9, 8), "eight band rows: 1..8");
        painter.paint(&lines(["head 0", "head 1", "footer"]));
        screen.feed(&drain(&mut painter));

        painter.insert_history(&lines((1..=8).map(|n| format!("r{n}"))));
        let frame = painter.frame.clone();
        assert!(!scrolled(&frame), "eight rows fill the band without a scroll: {frame:?}");
        screen.feed(&drain(&mut painter));
        assert!(screen.scrollback.is_empty(), "{:?}", screen.scrollback);
        assert_eq!(&screen.transcript()[1..9], &(1..=8).map(|n| format!("r{n}")).collect::<Vec<_>>()[..]);
        assert_eq!((painter.gap, painter.known_above), (0, 8));

        painter.insert_history(&lines(["r9", "r10"]));
        let frame = painter.frame.clone();
        assert!(
            frame.starts_with("\u{1b}[?2026h\u{1b}[1;9r\u{1b}[9;1H\r\n"),
            "the region above the head scrolls now: {frame:?}"
        );
        screen.feed(&drain(&mut painter));
        assert_eq!(screen.scrollback, vec!["", "r1"], "the anchor row and r1 left the top");
        let transcript = screen.transcript();
        assert_eq!(&transcript[2..11], &(2..=10).map(|n| format!("r{n}")).collect::<Vec<_>>()[..], "{}", shown(&transcript));
        assert_eq!(&transcript[11..14], &["head 0", "head 1", "footer"], "the head did not move: {}", shown(&transcript));
        assert_eq!((painter.top, painter.gap, painter.known_above), (9, 0, 9));
    }

    /// A head growing past the floor takes rows from the band first — no
    /// scroll, no bytes moving rows — and scrolls the region above it only
    /// for what the band cannot give; what leaves the screen then is the
    /// transcript, never a blank row.
    #[test]
    fn a_head_growing_past_the_floor_eats_the_band_before_it_scrolls() {
        let mut painter = painter(12, 0);
        let mut screen = Screen::new(40, 12);
        painter.set_height(1);
        painter.paint(&lines(["footer"]));
        screen.feed(&drain(&mut painter));
        painter.insert_history(&lines(["r1", "r2"]));
        painter.paint(&lines(["footer"]));
        screen.feed(&drain(&mut painter));
        assert_eq!((painter.top, painter.gap), (11, 8), "rows 3..10 are the band");

        painter.set_height(4);
        let frame = painter.frame.clone();
        assert!(!frame.contains('\n') && !frame.contains("\u{1b}M"), "the band gave three rows for free: {frame:?}");
        painter.paint(&lines(["cell 0", "cell 1", "cell 2", "footer"]));
        screen.feed(&drain(&mut painter));
        assert_eq!((painter.top, painter.gap, painter.known_above), (8, 5, 2));
        assert!(screen.scrollback.is_empty());
        let visible = screen.transcript();
        assert_eq!(&visible[1..3], &["r1", "r2"], "{}", shown(&visible));
        assert!(visible[3..8].iter().all(String::is_empty), "five band rows are left: {}", shown(&visible));
        assert_eq!(&visible[8..12], &["cell 0", "cell 1", "cell 2", "footer"], "{}", shown(&visible));

        // More than the band holds: the band goes first, the rest scrolls the
        // region above the head — the head's own rows stay where they are.
        painter.set_height(11);
        let frame = painter.frame.clone();
        assert!(
            frame.contains("\u{1b}[1;8r\u{1b}[8;1H\n\n\u{1b}[r"),
            "the two rows the band could not give scroll the region above the head: {frame:?}"
        );
        let cell: Vec<Line> = lines((0..10).map(|n| format!("cell {n}")).chain(["footer".to_owned()]));
        painter.paint(&cell);
        screen.feed(&drain(&mut painter));
        assert_eq!((painter.top, painter.gap, painter.known_above), (1, 0, 1));
        assert_eq!(screen.scrollback, vec!["", "r1"], "the transcript left, not a blank band: {:?}", screen.scrollback);
        let visible = screen.transcript();
        assert_eq!(visible[2], "r2", "{}", shown(&visible));
        assert_eq!(visible[3], "cell 0", "{}", shown(&visible));
        assert_eq!(visible[13], "footer", "{}", shown(&visible));
    }

    /// A head on the floor that shrinks stays on the floor: the rows it gives
    /// up at its top join the band, and nothing scrolls.
    #[test]
    fn an_ordinary_shrink_keeps_the_head_on_the_floor() {
        let mut painter = painter(30, 20);
        painter.set_height(6);
        assert_eq!((painter.top, painter.gap), (24, 4));
        let _ = drain(&mut painter);
        painter.set_height(5);
        assert_eq!((painter.top, painter.gap), (25, 5), "one row down, one more band row");
        let frame = painter.frame.clone();
        assert!(!frame.contains('\n') && !frame.contains("\u{1b}M"), "nothing scrolled: {frame:?}");
        assert_eq!(frame.matches("\u{1b}[K").count(), 1, "the row given up is cleared once: {frame:?}");
        assert!(!frame.contains("\u{1b}[J"), "nothing below the head to clear: {frame:?}");
        let _ = drain(&mut painter);
    }

    /// A status row coming and going costs one row's paint: the rows under it
    /// — composer, footer — keep their absolute rows on the floor, so the
    /// cache keeps them.
    #[test]
    fn a_status_row_toggling_repaints_only_itself() {
        let mut painter = Painter::new(Vec::new(), 120, 40, 12, true);
        painter.set_height(3);
        let idle = lines(["composer 0", "composer 1", "footer"]);
        painter.paint(&idle);
        let _ = drain(&mut painter);

        painter.set_height(4);
        let busy = lines(["status", "composer 0", "composer 1", "footer"]);
        painter.paint(&busy);
        let frame = painter.take_frame();
        assert_eq!(painter.top, 36);
        assert_eq!(frame.matches("\u{1b}[K").count(), 1, "only the status row draws: {frame:?}");
        assert!(frame.contains("\u{1b}[37;1H\u{1b}[0m\u{1b}[Kstatus"), "on the row above the composer: {frame:?}");

        painter.set_height(3);
        painter.paint(&idle);
        let frame = painter.take_frame();
        assert_eq!(painter.top, 37);
        assert_eq!(frame.matches("\u{1b}[K").count(), 1, "only the vacated row is cleared: {frame:?}");
        assert!(!frame.contains("composer"), "the rows that stayed did not repaint: {frame:?}");
    }

    /// A popup covers the band and the history above it alike, and closing
    /// it puts both back: the history where it was, the band blank, the head
    /// on the floor.
    #[test]
    fn a_popup_covers_the_band_and_puts_the_screen_back() {
        let mut painter = painter(30, 0);
        let mut screen = Screen::new(40, 30);
        painter.set_height(5);
        let head = lines(["head 0", "head 1", "head 2", "head 3", "footer"]);
        painter.paint(&head);
        screen.feed(&drain(&mut painter));
        painter.insert_history(&lines((1..=10).map(|n| format!("h{n}"))));
        painter.paint(&head);
        screen.feed(&drain(&mut painter));
        assert_eq!((painter.top, painter.gap, painter.known_above), (25, 14, 10));
        assert_eq!(painter.popup_budget(), 29, "the band counts toward the budget: 5 + 14 + 10 + 3, capped");

        painter.set_popup_height(29);
        assert_eq!((painter.top, painter.height), (1, 29));
        assert!(!painter.frame.contains('\n'), "nothing scrolled for its room: {:?}", painter.frame);
        painter.paint(&lines((0..29).map(|n| format!("p{n}"))));
        screen.feed(&drain(&mut painter));
        assert_eq!(screen.transcript()[1], "p0");

        painter.set_height(5);
        painter.paint(&head);
        screen.feed(&drain(&mut painter));
        assert_eq!((painter.top, painter.gap, painter.known_above), (25, 14, 10));
        let visible = screen.transcript();
        assert_eq!(&visible[1..11], &(1..=10).map(|n| format!("h{n}")).collect::<Vec<_>>()[..], "{}", shown(&visible));
        assert!(visible[11..25].iter().all(String::is_empty), "the band is blank again: {}", shown(&visible));
        assert_eq!(&visible[25..30], &["head 0", "head 1", "head 2", "head 3", "footer"], "{}", shown(&visible));
        assert!(screen.scrollback.is_empty());
    }

    /// A resize keeps the transcript at the top and the head on the new
    /// floor — growing and shrinking alike — with the band between them.
    #[test]
    fn a_resize_keeps_the_transcript_at_the_top_and_the_head_on_the_floor() {
        let mut painter = painter(24, 0);
        painter.set_height(5);
        painter.insert_history(&lines(["h1", "h2", "h3"]));
        let _ = drain(&mut painter);
        assert_eq!((painter.top, painter.gap, painter.known_above), (19, 15, 3));

        painter.resize(40, 46);
        let frame = painter.take_frame();
        assert_eq!((painter.top, painter.gap, painter.known_above), (41, 37, 3));
        assert!(frame.starts_with("\u{1b}[?2026h\u{1b}[1;1H\u{1b}[J"), "{frame:?}");
        assert!(frame.contains("\u{1b}[2;1H\u{1b}[39;49m\u{1b}[Kh1\u{1b}[39m"), "h1 is back on row one: {frame:?}");
        assert!(frame.contains("\u{1b}[4;1H\u{1b}[39;49m\u{1b}[Kh3\u{1b}[39m"), "h3 on row three: {frame:?}");
        assert!(!frame.contains("\u{1b}M"), "nothing scrolled: {frame:?}");

        painter.resize(40, 12);
        let frame = painter.take_frame();
        assert_eq!((painter.top, painter.gap, painter.known_above), (7, 3, 3));
        assert!(frame.contains("\u{1b}[2;1H\u{1b}[39;49m\u{1b}[Kh1\u{1b}[39m"), "{frame:?}");
    }

    /// A transcript taller than the new band is bottom-aligned above the head
    /// by a resize — the last rows are the ones a person was reading.
    #[test]
    fn a_resize_bottom_aligns_a_transcript_taller_than_the_band() {
        let mut painter = painter(12, 0);
        painter.set_height(3);
        painter.insert_history(&lines((1..=20).map(|n| format!("h{n}"))));
        let _ = drain(&mut painter);
        assert_eq!((painter.top, painter.gap, painter.known_above), (9, 0, 9));

        painter.resize(40, 10);
        let frame = painter.take_frame();
        assert_eq!((painter.top, painter.gap, painter.known_above), (7, 0, 7));
        assert!(frame.contains("\u{1b}[1;1H\u{1b}[39;49m\u{1b}[Kh14\u{1b}[39m"), "{frame:?}");
        assert!(frame.contains("\u{1b}[7;1H\u{1b}[39;49m\u{1b}[Kh20\u{1b}[39m"), "{frame:?}");
    }

    /// History arriving under a head short of the floor — a popup scrolled
    /// for rows the ring could not put back — pushes the head down onto the
    /// floor first (the captured reverse-index grammar), then fills the band
    /// from the top.
    #[test]
    fn a_head_short_of_the_floor_is_pushed_down_onto_it_by_history() {
        let mut painter = painter(40, 22);
        painter.set_height(7);
        assert_eq!((painter.top, painter.gap), (33, 11));
        let _ = drain(&mut painter);
        painter.set_popup_height(39);
        assert_eq!(painter.frame.matches('\n').count(), 21, "scrolled for the rows the ring cannot put back");
        let _ = drain(&mut painter);
        painter.set_height(7);
        let _ = drain(&mut painter);
        assert_eq!((painter.top, painter.gap, painter.known_above), (12, 11, 0), "21 rows short of the floor");

        painter.insert_history(&[Line::empty(), Line::from_text("warn")]);
        let frame = painter.frame.clone();
        let push = format!("\u{1b}[?2026h\u{1b}[13;40r\u{1b}[13;1H{}\u{1b}[r", "\u{1b}M".repeat(21));
        assert!(frame.starts_with(&push), "the head is pushed to the floor: {frame:?}");
        assert!(
            frame[push.len()..].starts_with("\u{1b}[1;33r\u{1b}[1;1H\r\n"),
            "then the band is filled from its first row: {frame:?}"
        );
        assert_eq!((painter.top, painter.gap, painter.known_above), (33, 30, 2));
    }

    /// A finished command group folds to one row while the model is still
    /// working, and nothing follows it yet. The head shrinks by the rows the
    /// group gave up — and the transcript had reached the head long before,
    /// so those rows are a hole, not a margin: given back above the head they
    /// stood as a break under the last reply, taller the more commands the
    /// group had held ("ran 8 commands 가 숫자에 따라 간격이 더 떨어짐",
    /// 2026-09-06). The head keeps its top instead, the vacated rows go under
    /// its footer, and the history that follows pushes it back down only as
    /// far as it needs.
    #[test]
    fn a_head_the_transcript_reached_keeps_its_top_when_it_shrinks() {
        let mut painter = painter(30, 5);
        painter.set_height(4);
        assert_eq!((painter.top, painter.gap), (26, 21));
        let lines: Vec<Line> = (1..=21).map(|n| Line::from_text(format!("t{n}"))).collect();
        painter.insert_history(&lines);
        assert_eq!((painter.top, painter.gap), (26, 0), "precondition: the transcript reached the head");
        let _ = drain(&mut painter);
        // Eight commands run: the live group grows the head by eight rows.
        painter.set_height(12);
        assert_eq!((painter.top, painter.gap), (18, 0), "the band was gone, so the region above scrolled");
        let _ = drain(&mut painter);
        // They finish and fold to one row; the model is still working.
        painter.set_height(5);
        let frame = painter.frame.clone();
        assert_eq!(
            (painter.top, painter.height, painter.gap),
            (18, 5, 0),
            "the head keeps its top and no hole opens above it: {frame:?}"
        );
        assert!(
            frame.contains("\u{1b}[24;1H\u{1b}[J"),
            "the vacated rows are cleared under the footer: {frame:?}"
        );
        assert!(!frame.contains("\u{1b}[K"), "no row above the head is cleared: {frame:?}");
        assert!(!frame.contains('\n') && !frame.contains("\u{1b}M"), "nothing scrolled: {frame:?}");
        let _ = drain(&mut painter);
        // The model's next two rows push the head down two rows, no further.
        painter.insert_history(&[Line::from_text("a"), Line::from_text("b")]);
        let frame = painter.frame.clone();
        assert!(
            frame.starts_with("\u{1b}[?2026h\u{1b}[19;30r\u{1b}[19;1H\u{1b}M\u{1b}M\u{1b}[r"),
            "pushed down two rows only: {frame:?}"
        );
        assert_eq!((painter.top, painter.gap), (20, 0), "the two rows sit between the transcript and the head");
        let _ = drain(&mut painter);
        // A status row toggling now costs the rows under it, not a hole: it
        // grows down into the room and gives it back.
        painter.set_height(6);
        assert_eq!((painter.top, painter.height), (20, 6), "grew down in place");
        let _ = drain(&mut painter);
        painter.set_height(5);
        assert_eq!((painter.top, painter.height, painter.gap), (20, 5, 0), "shrank in place");
        let _ = drain(&mut painter);
        // The turn is over: the head settles back onto the floor and the room
        // under it becomes the band above — the idle screen's spacing.
        assert_eq!(painter.slack, 5, "the rows the shrinks gave up, less what history took back");
        painter.settle_on_the_floor();
        let frame = painter.frame.clone();
        assert_eq!((painter.top, painter.height, painter.gap), (25, 5, 5), "on the floor, five band rows above");
        assert_eq!(frame.matches("\u{1b}M").count(), 5, "pushed down by the five rows: {frame:?}");
        assert_eq!(painter.slack, 0);
        let _ = drain(&mut painter);
        painter.settle_on_the_floor();
        assert!(painter.frame.is_empty(), "a head on the floor settles for free");
    }

    /// Room under the head that a POPUP's scroll left — not a shrink — is not
    /// the settle's to close: the transcript's last rows stand right above the
    /// head, and pushing it down would open a blank band between them (the
    /// void the model-picker e2e guards). History closes that room, as before.
    #[test]
    fn a_head_left_short_by_a_popup_does_not_settle_into_a_void() {
        let mut painter = painter(40, 22);
        painter.set_height(7);
        let _ = drain(&mut painter);
        painter.set_popup_height(39);
        let _ = drain(&mut painter);
        painter.set_height(7);
        let _ = drain(&mut painter);
        assert_eq!((painter.top, painter.slack), (12, 0), "precondition: 21 rows short, none of them a shrink's");
        painter.settle_on_the_floor();
        assert!(painter.frame.is_empty(), "nothing to settle: {:?}", painter.frame);
        assert_eq!(painter.top, 12, "the head keeps its row for the history to push");
    }

    /// A screen the shell had already filled — the cursor on the last row
    /// when zo opened — has no margin to give: the first shrink after a
    /// growth keeps the head's top as well.
    #[test]
    fn a_head_opened_on_a_full_screen_has_no_margin_to_shrink_into() {
        let mut painter = Painter::new(Vec::new(), 40, 24, 23, false);
        painter.set_height(3);
        assert_eq!((painter.top, painter.gap), (21, 0));
        let _ = drain(&mut painter);
        painter.set_height(7);
        assert_eq!(painter.top, 17);
        let _ = drain(&mut painter);
        painter.set_height(3);
        assert_eq!((painter.top, painter.gap), (17, 0), "the head keeps its top");
        let _ = drain(&mut painter);
    }

    /// `/clear` leaves the head on the floor with the whole screen above it
    /// as the band.
    #[test]
    fn clear_terminal_leaves_the_head_on_the_floor() {
        let mut painter = painter(40, 12);
        painter.set_height(7);
        painter.insert_history(&lines(["old"]));
        let _ = drain(&mut painter);
        painter.clear_terminal();
        assert_eq!((painter.top, painter.gap, painter.known_above), (33, 32, 0));
    }

    /// Leaving clears the band and the head and parks the cursor right under
    /// the transcript, so the shell's prompt follows the last row printed.
    #[test]
    fn leave_parks_the_cursor_under_the_transcript() {
        let mut painter = painter(24, 0);
        painter.set_height(5);
        painter.insert_history(&lines(["h1", "h2", "h3"]));
        let _ = drain(&mut painter);
        painter.leave();
        let out = String::from_utf8(std::mem::take(&mut painter.out)).expect("utf-8");
        assert!(out.ends_with("\u{1b}[r\u{1b}[5;1H\u{1b}[J\u{1b}[0m\u{1b}[?25h"), "{out:?}");
    }
}
