//! 접기 마커 — `docs/fold-markers.md` 의 private OSC 7788 계약.
//!
//! 접기/펼치기를 하는 쪽은 **IDE 패인**이다. zo 는 마우스를 잡지 않는다
//! (codex 원본도 `?1004` 포커스 리포팅만 켠다). 대신 도구 셀을 히스토리에
//! 커밋할 때 그 구간의 경계에 OSC 를 찍고, `zerocode-pty` 의 그리드가 행마다
//! `fold: Option<(id, Header|Body)>` 를 들고, `ui/shell.js` 의 `makeTermView`
//! 가 헤더 행에 손잡이를 얹는다.
//!
//! ```text
//! ESC ] 7788 ; begin ; <id> ; <flags> ; <summary> ESC \
//! <헤더 행>                    ← 언제나 보인다
//! <본문 행>…                   ← 접히는 부분
//! ESC ] 7788 ; end ; <id> ESC \
//! ```
//!
//! 마커를 모르는 터미널은 OSC 를 통째로 버리므로 본문이 그냥 펼쳐져 보인다 —
//! 그래서 본문 분량은 codex 의 단일 명령 미리보기 한도를 넘지 않게 해야 하고
//! ([`super::tools`] 의 `OUTPUT_MAX_ROWS`), 답변·서두 셀에는 아예 찍지 않는다.

use std::fmt::Write as _;

use super::ansi::Line;

/// private OSC 번호.
const OSC: &str = "7788";
/// 종결자. 계약은 `BEL` 도 받아주지만 우리는 ST 만 낸다.
const ST: &str = "\u{1b}\\";

/// Private fold markers are an explicit experiment, never a property of the
/// terminal hosting zo.
///
/// 마커를 접어 주는 쪽이 있을 때에만 묶음 셀에 본문을 단다. 맨 터미널에서는
/// 접어 줄 사람이 없으므로 codex 원본 그대로 — `• Ran 2 commands` 한 줄이고
/// 마커도 없다(파이프 골든과 같은 모양).
///
/// 렌더러는 env 를 직접 읽지 않는다. 이 값은 `App::new` 가 한 번 읽어
/// 화면 상태에 실어 주고, 테스트는 그냥 원하는 쪽을 넘긴다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FoldMode {
    /// 마커를 모르는 터미널. codex 원본과 바이트가 같다.
    #[default]
    Bare,
    /// OSC 7788 opt-in. 묶음 셀도 출력 본문을 달고 접힌 채 커밋된다.
    Markers,
}

impl FoldMode {
    /// `ZEROCODE_PANE_KEY` only identifies the transport and deliberately has
    /// no visual effect. A person who wants the private marker protocol must
    /// opt in separately with `ZO_TUI_FOLD_MARKERS=1`.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        match lookup("ZO_TUI_FOLD_MARKERS") {
            Some(value) if value.trim() == "1" => Self::Markers,
            _ => Self::Bare,
        }
    }

    /// Private marker protocol enabled.
    #[must_use]
    pub const fn markers_enabled(self) -> bool {
        matches!(self, Self::Markers)
    }
}

/// 한 세션 안에서 단조 증가하는 fold id 발급기.
#[derive(Debug, Default)]
pub struct FoldIds {
    next: u32,
}

impl FoldIds {
    /// 다음 id. u32 를 다 쓰면 처음으로 돈다 — 패인은 `(term, id)` 로 상태를
    /// 기억하므로 한 바퀴 돈 뒤에 옛 접힘 상태를 물려받는 것이 최악이고,
    /// 그건 42억 개의 도구 셀 뒤의 일이다.
    pub fn mint(&mut self) -> u32 {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        id
    }
}

/// `ESC ] 7788 ; begin ; <id> ; collapsed ; <summary> ESC \`.
#[must_use]
pub fn begin(id: u32, collapsed: bool, summary: &str) -> String {
    begin_with_teaser(id, collapsed, 0, summary)
}

/// [`begin`] with `teaser=<n>` — the first `teaser` body rows stay visible
/// while the region is collapsed and go away when it opens. That is how a
/// folded cell keeps codex's density line (`  └ … +N lines`) without the
/// body under it. `0` writes the plain flags.
#[must_use]
pub fn begin_with_teaser(id: u32, collapsed: bool, teaser: usize, summary: &str) -> String {
    let mut flags = String::from(if collapsed { "collapsed" } else { "expanded" });
    if teaser > 0 {
        let _ = write!(flags, ",teaser={teaser}");
    }
    let mut out = String::new();
    let _ = write!(out, "\u{1b}]{OSC};begin;{id};{flags};{}{ST}", encode(summary));
    out
}

/// `ESC ] 7788 ; end ; <id> ESC \`.
#[must_use]
pub fn end(id: u32) -> String {
    let mut out = String::new();
    let _ = write!(out, "\u{1b}]{OSC};end;{id}{ST}");
    out
}

/// 계약의 퍼센트 인코딩 — `;` `%` 와 0x00–0x1F. 그 밖의 UTF-8 은 그대로다.
#[must_use]
pub fn encode(summary: &str) -> String {
    let mut out = String::with_capacity(summary.len());
    for ch in summary.chars() {
        match ch {
            ';' => out.push_str("%3B"),
            '%' => out.push_str("%25"),
            control if control.is_control() && (control as u32) < 0x20 => {
                let _ = write!(out, "%{:02X}", control as u32);
            }
            other => out.push(other),
        }
    }
    out
}

/// 셀 하나를 접기 구간으로 감싼다 — 첫 행 앞에 `begin`, 마지막 행 뒤에 `end`.
///
/// 본문이 없는 셀(헤더 한 줄뿐인 `• Ran 2 commands` 같은 것)은 **감싸지
/// 않는다**. 접을 것이 없는 자리에 손잡이만 그리면 패인에서 눌러도 아무 일이
/// 일어나지 않는다. 요약 문안은 헤더 행의 평문이다(계약: `summary=<헤더 문안>`).
///
/// 이미 마커가 붙은 줄에는 다시 붙이지 않는다 — 계약이 중첩을 금지한다
/// ("안쪽 begin 은 바깥 end 까지 무시").
#[must_use]
pub fn wrap_cell(lines: Vec<Line>, ids: &mut FoldIds) -> Vec<Line> {
    wrap_cell_with_teaser(lines, 0, ids)
}

/// [`wrap_cell`] whose first `teaser` body rows are the region's collapsed
/// teaser — visible folded, hidden open. The count is clamped to the rows
/// that exist after the header.
#[must_use]
pub fn wrap_cell_with_teaser(mut lines: Vec<Line>, teaser: usize, ids: &mut FoldIds) -> Vec<Line> {
    if lines.len() < 2 {
        return lines;
    }
    if lines.iter().any(|line| line.lead.is_some() || line.trail.is_some()) {
        return lines;
    }
    let id = ids.mint();
    let summary = lines[0].plain();
    let last = lines.len() - 1;
    let teaser = teaser.min(last);
    lines[0].lead = Some(begin_with_teaser(id, /*collapsed*/ true, teaser, &summary).into());
    lines[last].trail = Some(end(id).into());
    lines
}

#[cfg(test)]
mod tests {
    use super::{begin, encode, end, wrap_cell, FoldIds, FoldMode};
    use crate::tui::ansi::Line;

    #[test]
    fn the_begin_sequence_is_the_contract_one() {
        assert_eq!(
            begin(0, true, "• Ran ls -la"),
            "\u{1b}]7788;begin;0;collapsed;• Ran ls -la\u{1b}\\"
        );
        assert_eq!(
            begin(7, false, "x"),
            "\u{1b}]7788;begin;7;expanded;x\u{1b}\\"
        );
        assert_eq!(end(7), "\u{1b}]7788;end;7\u{1b}\\");
        // The teaser rides the flags word list; zero writes nothing extra.
        assert_eq!(
            super::begin_with_teaser(3, true, 1, "• Ran ls"),
            "\u{1b}]7788;begin;3;collapsed,teaser=1;• Ran ls\u{1b}\\"
        );
        assert_eq!(super::begin_with_teaser(3, true, 0, "x"), begin(3, true, "x"));
    }

    /// The teaser count never exceeds the body: a two-row cell asked for two
    /// teaser rows gets one, the only body row there is.
    #[test]
    fn the_teaser_is_clamped_to_the_body() {
        let mut ids = FoldIds::default();
        let cell = super::wrap_cell_with_teaser(
            vec![Line::from_text("• Ran ls"), Line::from_text("  └ … +9 lines")],
            2,
            &mut ids,
        );
        assert_eq!(
            cell[0].lead.as_deref(),
            Some("\u{1b}]7788;begin;0;collapsed,teaser=1;• Ran ls\u{1b}\\")
        );
    }

    #[test]
    fn semicolons_percents_and_controls_are_encoded() {
        assert_eq!(encode("a;b%c\u{1}d\te"), "a%3Bb%25c%01d%09e");
        // 그 밖의 UTF-8 은 건드리지 않는다.
        assert_eq!(encode("Ran 한글"), "Ran 한글");
    }

    #[test]
    fn a_cell_with_a_body_is_wrapped_head_first() {
        let mut ids = FoldIds::default();
        let cell = wrap_cell(
            vec![Line::from_text("• Ran ls"), Line::from_text("  └ total 16")],
            &mut ids,
        );
        assert_eq!(
            cell[0].lead.as_deref(),
            Some("\u{1b}]7788;begin;0;collapsed;• Ran ls\u{1b}\\")
        );
        assert_eq!(cell[0].trail, None);
        assert_eq!(cell[1].lead, None);
        assert_eq!(cell[1].trail.as_deref(), Some("\u{1b}]7788;end;0\u{1b}\\"));
    }

    #[test]
    fn a_header_only_cell_gets_no_handle() {
        let mut ids = FoldIds::default();
        let cell = wrap_cell(vec![Line::from_text("• Ran 2 commands")], &mut ids);
        assert_eq!(cell[0].lead, None);
        assert_eq!(cell[0].trail, None);
        // id 도 소비하지 않는다.
        assert_eq!(ids.mint(), 0);
    }

    #[test]
    fn ids_climb_and_never_nest() {
        let mut ids = FoldIds::default();
        let two = |ids: &mut FoldIds| {
            wrap_cell(vec![Line::from_text("h"), Line::from_text("b")], ids)
        };
        assert!(two(&mut ids)[0].lead.as_deref().is_some_and(|lead| lead.contains(";begin;0;")));
        let second = two(&mut ids);
        assert!(second[0].lead.as_deref().is_some_and(|lead| lead.contains(";begin;1;")));
        // 이미 감싼 셀을 또 감싸도 바깥 마커가 그대로다.
        let again = wrap_cell(second, &mut ids);
        assert!(again[0].lead.as_deref().is_some_and(|lead| lead.contains(";begin;1;")));
    }

    #[test]
    fn fold_markers_require_the_dedicated_opt_in() {
        assert_eq!(FoldMode::from_lookup(|_| None), FoldMode::Bare);
        assert_eq!(
            FoldMode::from_lookup(|_| Some("  ".to_string())),
            FoldMode::Bare
        );
        assert_eq!(
            FoldMode::from_lookup(|name| (name == "ZEROCODE_PANE_KEY").then(|| "p1".to_string())),
            FoldMode::Bare
        );
        assert_eq!(
            FoldMode::from_lookup(|name| {
                (name == "ZO_TUI_FOLD_MARKERS").then(|| "1".to_string())
            }),
            FoldMode::Markers
        );
        assert!(FoldMode::Markers.markers_enabled());
        assert!(!FoldMode::default().markers_enabled());
    }
}
