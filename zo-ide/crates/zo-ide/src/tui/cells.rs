//! 히스토리 셀 조립 — `RenderBlock` 과 부팅 정보를 codex 문법의 줄로.
//!
//! 접두어 규칙은 codex `tui/src/history_cell/messages.rs` 실측이다:
//! 답변은 `• `(dim) / 이어지는 줄 `  `, reasoning 은 같은 접두어에 본문 전체
//! dim+italic, 유저는 `› `(bold+dim) / `  ` 에 뒤 빈 줄 하나. 도구 셀은
//! [`super::tools`] 가 따로 조립한다(codex `exec_cell/`).
//!
//! 스트리밍은 [`MarkdownStream`] 이 codex 의 두 영역 모델 그대로 돌린다 —
//! 확정 영역은 애니메이션 큐를 타고 스크롤백으로, 미확정 꼬리는 뷰포트의
//! 활성 셀 칸으로.

use std::collections::VecDeque;
use std::time::Instant;

use runtime::message_stream::{SystemLevel, TodoResultItem, TodoResultStatus};

use super::ansi::{Color, Line, Span, Style};
use super::chunking::{DrainPlan, Policy, Snapshot};
use super::holdback;
use super::markdown;
use super::palette;
use super::paths::center_truncate_path;
use super::wrap::wrap_line;

/// 셀 접두어 — 첫 줄과 이어지는 줄.
#[derive(Debug, Clone)]
pub struct Prefix {
    pub initial: Span,
    pub subsequent: Span,
}

impl Prefix {
    #[must_use]
    pub fn new(initial: Span, subsequent: Span) -> Self {
        Self {
            initial,
            subsequent,
        }
    }

    /// 답변·reasoning 셀의 `• ` / `  `.
    #[must_use]
    pub fn bullet() -> Self {
        Self::new(
            Span::new("• ", palette::cell_marker()),
            Span::raw("  "),
        )
    }

    /// 유저 셀의 `› ` / `  `.
    #[must_use]
    pub fn user() -> Self {
        Self::new(Span::new("› ", palette::user_marker()), Span::raw("  "))
    }

    /// 도구 출력의 `  └ ` / `    `.
    #[must_use]
    pub fn tool_output() -> Self {
        Self::new(Span::dim("  └ "), Span::raw("    "))
    }

    /// `  └ ` 없이 네 칸만 — 접힌 묶음 본문에서 `  └ <cmd>` 아래로 이어지는
    /// 출력이 쓴다(`tool_output` 의 subsequent 와 같은 칸).
    #[must_use]
    pub fn gutter() -> Self {
        Self::new(Span::raw("    "), Span::raw("    "))
    }

    fn width(&self) -> usize {
        self.initial.width().max(self.subsequent.width())
    }
}

/// 줄들을 폭 `width` 에 맞춰 접고 접두어를 입힌다. `first` 가 거짓이면
/// 첫 줄에도 이어지는 접두어를 쓴다 — 스트리밍으로 나눠 넣을 때의 이음매다.
#[must_use]
pub fn prefixed(lines: &[Line], width: usize, prefix: &Prefix, first: bool) -> Vec<Line> {
    prefixed_marking(lines, width, prefix, first).0
}

/// [`prefixed`] 와 같되 **마커를 가져갔는지**를 함께 준다.
///
/// 스트리밍이 원문을 조각으로 나눠 렌더할 때 필요한 유일한 이음매 상태다:
/// `• ` 는 셀 전체에서 내용이 있는 **첫 줄** 하나만 가져가므로, 앞 조각이
/// 그것을 이미 썼는지 다음 조각에 넘겨 줘야 한다.
fn prefixed_marking(
    lines: &[Line],
    width: usize,
    prefix: &Prefix,
    first: bool,
) -> (Vec<Line>, bool) {
    #[cfg(test)]
    markdown::probe::wrapped(lines.len());
    let content = width.saturating_sub(prefix.width()).max(1);
    let mut out = Vec::new();
    // 셀 안의 빈 줄(문단 사이)에는 접두어를 붙이지 않는다 — 캡처의 답변 셀도
    // 그 자리가 진짜 빈 줄이다. 마커는 **내용이 있는 첫 줄**이 가져간다.
    let mut marker_pending = first;
    for line in lines {
        // A long list item or quote wraps under its own text: the renderer
        // names the indent it sits under, and every continuation row starts
        // with it (codex wraps with the indent stack as `subsequent_indent`).
        let hanging = line.continuation.clone().unwrap_or_else(|| Span::raw(""));
        // The rows remember the line they came from, with the cell prefix on
        // it, so a screen rebuilt at another width re-wraps the line.
        let mut origin: Option<std::sync::Arc<Line>> = None;
        for wrapped in wrap_line(line, content, &hanging) {
            if wrapped.plain().is_empty() {
                out.push(Line::empty());
                continue;
            }
            let marker = if marker_pending {
                marker_pending = false;
                prefix.initial.clone()
            } else {
                prefix.subsequent.clone()
            };
            let origin = origin.get_or_insert_with(|| {
                std::sync::Arc::new(origin_line(line, &marker, &prefix.subsequent, &hanging))
            });
            let mut row = wrapped.prefixed(marker);
            row.origin = Some(std::sync::Arc::clone(origin));
            out.push(row);
        }
    }
    (out, first && !marker_pending)
}

/// The logical line a cell's rows were wrapped from, as the ring keeps it:
/// the marker its first row wore in front of the renderer's spans, and a
/// continuation of the cell's subsequent prefix plus the line's own hanging
/// indent — wrapped at any width this gives the rows the cell would print.
fn origin_line(line: &Line, marker: &Span, subsequent: &Span, hanging: &Span) -> Line {
    let mut spans = Vec::with_capacity(line.spans.len() + 1);
    spans.push(marker.clone());
    spans.extend(line.spans.iter().cloned());
    let mut continuation = subsequent.clone();
    continuation.text.push_str(&hanging.text);
    if !hanging.text.trim().is_empty() {
        continuation.style = hanging.style;
    }
    let mut origin = Line::new(spans).styled(line.style);
    origin.lead.clone_from(&line.lead);
    origin.trail.clone_from(&line.trail);
    origin.with_continuation(Some(continuation))
}

/// How many logical lines these rows belong to: rows sharing an origin are
/// one line, a row without one is a line of its own. Width-independent, which
/// is what lets a width change carry "how much went out" across the render.
fn logical_lines(rows: &[Line]) -> usize {
    let mut count = 0;
    let mut last: Option<*const Line> = None;
    for row in rows {
        if let Some(origin) = &row.origin {
            let pointer = std::sync::Arc::as_ptr(origin);
            if last != Some(pointer) {
                count += 1;
                last = Some(pointer);
            }
        } else {
            count += 1;
            last = None;
        }
    }
    count
}

/// How many rows the first `lines` logical lines of `rows` take.
fn rows_of_first_lines(rows: &[Line], lines: usize) -> usize {
    let mut seen = 0;
    let mut last: Option<*const Line> = None;
    for (index, row) in rows.iter().enumerate() {
        let starts_line = if let Some(origin) = &row.origin {
            let pointer = std::sync::Arc::as_ptr(origin);
            let starts = last != Some(pointer);
            last = Some(pointer);
            starts
        } else {
            last = None;
            true
        };
        if starts_line {
            if seen == lines {
                return index;
            }
            seen += 1;
        }
    }
    rows.len()
}

/// 원문 조각 하나를 렌더한 결과 — [`MarkdownStream`] 의 증분 렌더가 쓴다.
struct Segment {
    /// 꼬리 빈 줄을 **다듬은** 표시 행들. 조각 하나짜리 문서라면 이것이 곧
    /// `prefixed(markdown::render_wrapped(…))` 다.
    lines: Vec<Line>,
    /// 다듬기가 떼어 낸 꼬리 빈 줄들. 뒤에 조각이 더 붙으면 도로 끼워야 한다.
    pad: Vec<Line>,
    /// 이 조각이 `• ` 마커를 가져갔는지.
    marker: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkdownStreamKind {
    Answer,
    Reasoning,
}

/// Split structured reasoning-summary parts into the status header and
/// renderable content. This follows codex 0.151's part grammar exactly.
#[must_use]
pub(crate) fn split_reasoning_summary_parts(reasoning_parts: &[String]) -> (String, String) {
    let mut leading_empty_part_header = None;
    let mut content_parts = Vec::with_capacity(reasoning_parts.len());

    for part in reasoning_parts {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }

        let header_end = part.strip_prefix("**").and_then(|after_open| {
            after_open
                .find("**")
                .and_then(|close| (close > 0).then_some(close + 4))
        });
        let body = header_end.map_or(part, |header_end| &part[header_end..]);
        if body.trim() == "<!-- -->" {
            if content_parts.is_empty() && leading_empty_part_header.is_none() {
                if let Some(header_end) = header_end {
                    leading_empty_part_header = Some(part[..header_end].to_string());
                }
            }
            continue;
        }

        content_parts.push(part);
    }

    let content = content_parts.join("\n\n");
    if content.is_empty() {
        return (leading_empty_part_header.unwrap_or_default(), content);
    }

    if let Some(after_open) = content.strip_prefix("**") {
        if let Some(close) = after_open.find("**") {
            let after_close_idx = 2 + close + 2;
            let after_close = &content[after_close_idx..];
            if after_close.starts_with('\n') || after_close.starts_with('\r') {
                return (
                    content[..after_close_idx].to_string(),
                    after_close.to_string(),
                );
            }
        }
    }

    (leading_empty_part_header.unwrap_or_default(), content)
}

pub(crate) fn user_message_style() -> Style {
    user_message_style_for_palette(palette::terminal_palette())
}

fn user_message_style_for_palette(
    terminal_palette: Option<palette::TerminalPalette>,
) -> Style {
    let Some(terminal_palette) = terminal_palette else {
        return Style::new();
    };
    let (top, alpha) = if terminal_palette.has_light_background() {
        ((0, 0, 0), 0.04)
    } else {
        ((255, 255, 255), 0.12)
    };
    let background = super::shimmer::blend(top, terminal_palette.background(), alpha);
    Style::new().bg(Color::Rgb(
        background.0,
        background.1,
        background.2,
    ))
}

/// 유저 셀 — 캡처는 앞 빈 줄 둘, 뒤 빈 줄 하나를 함께 넣는다.
#[must_use]
pub fn user_cell(text: &str, width: usize) -> Vec<Line> {
    user_cell_for_palette(text, width, palette::terminal_palette())
}

fn user_cell_for_palette(
    text: &str,
    width: usize,
    terminal_palette: Option<palette::TerminalPalette>,
) -> Vec<Line> {
    let body: Vec<Line> = text.lines().map(Line::from_text).collect();
    let body = if body.is_empty() {
        vec![Line::empty()]
    } else {
        body
    };
    // The first blank is the untinted breathing gap between cells; the tinted
    // band below it is codex's shape exactly — one leading blank + body + one
    // trailing blank (history_cell/messages.rs:265-270) — so the text sits in
    // the band's vertical center instead of hugging its top edge.
    let mut out = vec![Line::empty()];
    let band_start = out.len();
    out.push(Line::empty());
    out.extend(prefixed(&body, width, &Prefix::user(), true));
    out.push(Line::empty());
    let style = user_message_style_for_palette(terminal_palette);
    for line in &mut out[band_start..] {
        line.style = style;
    }
    out
}

/// 시스템 통지 셀. 경고는 캡처의 `⚠ …` 노랑 줄이고, 그 외는 dim `• `.
#[must_use]
pub fn system_cell(level: SystemLevel, text: &str, width: usize) -> Vec<Line> {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let (marker, style) = match level {
        SystemLevel::Error => (
            Span::new("⚠ ", Style::new().fg(Color::RED).bold()),
            Style::new().fg(Color::RED),
        ),
        SystemLevel::Warn => (
            Span::new("⚠ ", Style::new().fg(palette::NOTICE_WARN)),
            Style::new().fg(palette::NOTICE_WARN),
        ),
        SystemLevel::Success => (Span::dim("• "), Style::new()),
        SystemLevel::Info | SystemLevel::Housekeeping => (Span::dim("• "), Style::new().dim()),
    };
    let body: Vec<Line> = trimmed
        .lines()
        .map(|line| Line::new(vec![Span::new(line.to_string(), style)]))
        .collect();
    let prefix = Prefix::new(marker, Span::raw("  "));
    let mut out = vec![Line::empty()];
    out.extend(prefixed(&body, width, &prefix, true));
    out
}

/// A message a peer agent sent with `SendMessage` — not the person's own words,
/// so not the tinted user band: an envelope header naming the sender, and the
/// body indented under it. codex has no such cell (it has no teammate panes);
/// Claude Code's teammate message row is the shape followed. Before this the
/// framed text stood in a user cell, frame and all ("메시지 표시도 깔끔하게",
/// 2026-09-06).
#[must_use]
pub fn peer_message_cell(from: Option<&str>, body: &str, width: usize) -> Vec<Line> {
    let mut header = vec![
        Span::new("✉ ", Style::new().fg(Color::CYAN).bold()),
        Span::bold(super::strings::PEER_MESSAGE),
    ];
    if let Some(from) = from.map(str::trim).filter(|from| !from.is_empty()) {
        header.push(Span::dim(format!(" · {}", super::strings::peer_message_from(from))));
    }
    let lines: Vec<Line> = body.trim().lines().map(Line::from_text).collect();
    let prefix = Prefix::new(Span::raw("  "), Span::raw("  "));
    let mut out = vec![Line::empty(), Line::new(header).truncated(width)];
    out.extend(prefixed(&lines, width, &prefix, true));
    out
}

#[cfg(test)]
mod peer_message_cell_tests {
    use super::{peer_message_cell, Line};

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::plain).collect()
    }

    /// The envelope names the sender when the frame did, and the body sits
    /// indented under it — no `› ` prompt marker, no `[message via …]` frame.
    #[test]
    fn a_peer_message_wears_an_envelope_not_the_persons_prompt() {
        assert_eq!(
            plain(&peer_message_cell(Some("reviewer"), "first line\nsecond line\n", 80)),
            vec!["", "✉ Message · from reviewer", "  first line", "  second line"]
        );
        assert_eq!(
            plain(&peer_message_cell(None, "hello", 80)),
            vec!["", "✉ Message", "  hello"]
        );
    }
}

/// 제출된 플랜의 히스토리 셀 — codex `history_cell/plans.rs::ProposedPlanCell`.
///
/// ```text
/// • Proposed Plan
///
///   <마크다운 본문, 두 칸 들여쓰기>
///
///   ↳ /plan off to approve · <경로>
/// ```
///
/// 마지막 줄만 우리 것이다. codex 의 셀에는 승인 힌트가 없지만 우리 계약은
/// 다르다: [`crate`] 의 `ExitPlanModeV2` 는 **쓰기 권한을 절대 돌려주지
/// 않는다**(모델이 자기 플랜을 자기가 승인하는 길이 되므로), 승인은 사람이
/// `/plan off` 로 하는 별개의 행동이다. 그 사실이 화면에 없으면 승인해야 할
/// 사람이 무엇을 해야 하는지 모른다.
///
/// 본문 줄바꿈 폭이 `width - 4` 인 것과 빈 줄이 앞뒤로 하나씩인 것은 codex
/// 그대로다. startup palette가 배경을 알면 본문 영역에 같은 저대비 패널색을
/// 입히고, 응답이 없으면 `Style::default()`로 남긴다.
#[must_use]
pub fn proposed_plan_cell(plan: &str, plan_path: Option<&str>, width: usize) -> Vec<Line> {
    proposed_plan_cell_for_palette(plan, plan_path, width, palette::terminal_palette())
}

fn proposed_plan_cell_for_palette(
    plan: &str,
    plan_path: Option<&str>,
    width: usize,
    terminal_palette: Option<palette::TerminalPalette>,
) -> Vec<Line> {
    let mut out = vec![
        Line::empty(),
        Line::new(vec![Span::dim("• "), Span::new("Proposed Plan", Style::new().bold())]),
        Line::empty(),
    ];

    let wrap = width.saturating_sub(4).max(1);
    let mut body = super::markdown::render_wrapped(plan.trim(), Some(wrap));
    if body.is_empty() {
        // codex 도 빈 플랜을 빈 화면으로 두지 않는다.
        body.push(Line::new(vec![Span::new(
            "(empty)".to_string(),
            Style::new().dim().italic(),
        )]));
    }
    for line in body {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(line.spans);
        out.push(Line::new(spans));
    }

    out.push(Line::empty());
    let mut hint = vec![
        Span::dim("  ↳ "),
        Span::new("/plan off".to_string(), Style::new().fg(Color::CYAN)),
        Span::dim(" to approve"),
    ];
    if let Some(path) = plan_path {
        hint.push(Span::dim(" · "));
        hint.push(Span::dim(path.to_string()));
    }
    out.push(Line::new(hint));
    let style = user_message_style_for_palette(terminal_palette);
    let hint_index = out.len().saturating_sub(1);
    for line in out.iter_mut().take(hint_index).skip(2) {
        line.style = style;
    }
    out
}

/// Completed `update_plan` / `TodoWrite` checklist — codex
/// `history_cell/plans.rs::PlanUpdateCell::display_lines`.
#[must_use]
pub fn plan_update_cell(items: &[TodoResultItem], width: usize) -> Vec<Line> {
    let mut out = vec![Line::new(vec![Span::dim("• "), Span::bold("Updated Plan")])];
    let content_width = width.saturating_sub(4).max(1);
    let mut body = Vec::new();

    if items.is_empty() {
        body.push(Line::new(vec![Span::new(
            "(no steps provided)",
            Style::new().dim().italic(),
        )]));
    } else {
        for item in items {
            let (box_str, step_style) = match item.status {
                TodoResultStatus::Completed => ("✔ ", Style::new().strike().dim()),
                TodoResultStatus::InProgress => {
                    ("□ ", Style::new().fg(Color::CYAN).bold())
                }
                TodoResultStatus::Pending => ("□ ", Style::new().dim()),
            };
            let step = Line::new(vec![
                Span::raw(box_str),
                Span::new(
                    crate::util::ansi::sanitize_inline(&item.content),
                    step_style,
                ),
            ]);
            body.extend(wrap_line(&step, content_width, &Span::raw("  ")));
        }
    }

    out.extend(prefixed(&body, width, &Prefix::tool_output(), true));
    out
}

/// 답한(또는 끊긴) 질문의 히스토리 셀 — codex
/// `history_cell/request_user_input.rs::RequestUserInputResultCell` 그대로다.
/// 질문 오버레이는 뷰포트에 사는 화면이라 답과 함께 사라진다. codex 도 그
/// 자리에서 이 셀을 스크롤백에 남긴다:
///
/// ```text
/// • Questions 1/1 answered
///   • Which auth method?
///     answer: OAuth
/// ```
///
/// 끊겼으면 머리에 `(interrupted)`(시안)가 붙고 질문 뒤에 `(unanswered)`(dim),
/// 마지막에 `  ↳ interrupted with 1 unanswered` 가 선다.
#[must_use]
pub fn question_cell(question: &str, answers: &[String], width: usize) -> Vec<Line> {
    let answered = usize::from(!answers.is_empty());
    let mut header = vec![
        Span::dim("• "),
        Span::bold("Questions"),
        Span::dim(format!(" {answered}/1 answered")),
    ];
    if answered == 0 {
        header.push(Span::new(
            " (interrupted)",
            Style::new().fg(palette::COMMAND_TOKEN),
        ));
    }
    let mut out = vec![Line::empty(), Line::new(header).truncated(width)];

    let mut asked = vec![Span::raw(question.to_string())];
    if answered == 0 {
        asked.push(Span::dim(" (unanswered)"));
    }
    out.extend(wrap_line(
        &Line::new(asked).prefixed(Span::raw("  • ")),
        width,
        &Span::raw("    "),
    ));

    for answer in answers {
        let line = Line::new(vec![
            Span::dim("    answer: "),
            Span::new(answer.clone(), Style::new().fg(palette::COMMAND_TOKEN)),
        ]);
        out.extend(wrap_line(&line, width, &Span::dim("            ")));
    }
    if answered == 0 {
        let line = Line::new(vec![
            Span::new("  ↳ ", Style::new().fg(palette::COMMAND_TOKEN).dim()),
            Span::new(
                "interrupted with 1 unanswered",
                Style::new().fg(palette::COMMAND_TOKEN).dim(),
            ),
        ]);
        out.extend(wrap_line(&line, width, &Span::dim("    ")));
    }
    out
}

/// 장식 없는 본문 셀 — 슬래시 카드나 `send_to_user` 메모처럼 codex 에 대응
/// 문법이 없는 것들. 접두어만 주고 원문을 그대로 흘린다.
#[must_use]
pub fn verbatim_cell(text: &str, width: usize) -> Vec<Line> {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let body: Vec<Line> = trimmed.lines().map(Line::from_text).collect();
    let mut out = vec![Line::empty()];
    out.extend(prefixed(&body, width, &Prefix::bullet(), true));
    out
}

/// 장식 없는 셀 — codex `chatwidget::add_plain_history_lines` 가 미는
/// `PlainHistoryCell` 이다. [`verbatim_cell`] 과 달리 **접두어가 없어** 줄이
/// 0열에서 시작한다. 앞 빈 줄 하나는 다른 셀과 같다.
///
/// 실측(`docs/captures/codex-tui-v0.149.1-resume-summary.bin`, 84~86행):
///
/// ```text
/// ESC[39;49m ESC[K                                         ← 앞 빈 줄
/// ESC[39;49m ESC[K Token usage: total=7,627 …
/// ESC[39;49m ESC[K To continue this session, run ESC[38;5;6;49m…
/// ```
#[must_use]
pub fn plain_cell(body: &[Line], width: usize) -> Vec<Line> {
    if body.is_empty() {
        return Vec::new();
    }
    let mut out = vec![Line::empty()];
    for line in body {
        out.extend(wrap_line(line, width.max(1), &Span::raw("")));
    }
    out
}

/// One live-slot row for a streak of loop iterations that reported no work.
/// The caller keeps it out of scrollback while the streak remains current and
/// replaces this row in place as the count changes.
#[must_use]
pub fn quiet_loop_cell(loop_id: &str, count: u32, last: &str, width: usize) -> Vec<Line> {
    vec![Line::new(vec![
        Span::dim("• "),
        Span::new(loop_id.to_string(), Style::new().bold()),
        Span::dim(format!(" · quiet ×{count} (last {last})")),
    ])
    .truncated(width)]
}

/// 부팅 카드 안쪽 폭의 상한 — codex
/// `history_cell/session.rs::SESSION_HEADER_MAX_INNER_WIDTH` 그대로다
/// ("Just an eyeballed value"). 하한은 **없다**: codex `with_border` 는
/// `content_width = max_line_width` 로 상자를 내용에 맞춰 줄인다.
const CARD_MAX_INNER: usize = 56;

/// 부팅 카드 — 캡처의 둥근 모서리 상자.
#[must_use]
pub fn boot_card(product: &str, version: &str, model: &str, effort: &str, cwd: &str, width: usize) -> Vec<Line> {
    boot_card_with_product_style(
        product,
        version,
        model,
        effort,
        cwd,
        width,
        Style::new().bold(),
    )
}

fn boot_card_with_product_style(
    product: &str,
    version: &str,
    model: &str,
    effort: &str,
    cwd: &str,
    width: usize,
    product_style: Style,
) -> Vec<Line> {
    let title = vec![
        Span::dim(">_ "),
        Span::new(product.to_string(), product_style),
        Span::dim(format!(" (v{version})")),
    ];
    let mut model_row = vec![
        Span::dim("model:     "),
        Span::raw(model.to_string()),
    ];
    if !effort.is_empty() {
        model_row.push(Span::raw(" "));
        model_row.push(Span::raw(effort.to_string()));
    }
    model_row.push(Span::dim("   "));
    model_row.push(Span::new("/model", Style::new().fg(palette::COMMAND_TOKEN)));
    model_row.push(Span::dim(" to change"));
    // codex `card_inner_width(width, 56)` — 화면에서 테두리 넷을 뺀 만큼이
    // 천장이다. 그 다음 `with_border` 가 상자를 **가장 긴 줄**에 맞춘다.
    let ceiling = CARD_MAX_INNER.min(width.max(4).saturating_sub(4));
    // 경로는 오른쪽이 아니라 **가운데**를 버린다 — codex
    // `history_cell/session.rs::format_directory_inner` 는 폭이 넘칠 때
    // `text_formatting::center_truncate_path` 를 부르고, 그 폭은 안쪽 폭에서
    // 라벨 접두어(`directory: `, 11칸)를 뺀 만큼이다. 꼬리(폴더 이름)가 남는
    // 쪽이 사람에게 쓸모 있다.
    let dir_prefix = "directory: ";
    let dir = center_truncate_path(cwd, ceiling.saturating_sub(dir_prefix.len()));
    let dir_row = vec![Span::dim(dir_prefix), Span::raw(dir)];

    card(
        vec![
            Line::new(title),
            Line::empty(),
            Line::new(model_row),
            Line::new(dir_row),
        ],
        ceiling,
    )
}

/// 카드 상자 한 벌 — codex `with_border` 그대로다. 부팅 카드와 `/status`
/// 카드가 같은 상자를 쓴다 — 캡처의 둘도 같은 문법이다.
///
/// `ceiling` 은 **천장일 뿐 하한이 아니다**: 줄은 먼저 `ceiling` 으로 잘리고,
/// 상자는 그 뒤 **가장 긴 줄**에 맞춰 줄어든다. 바깥변은 안쪽 폭 + 2 칸이고
/// (양쪽 패딩 한 칸씩), 여백까지 dim 이다.
#[must_use]
pub fn card(rows: Vec<Line>, ceiling: usize) -> Vec<Line> {
    let rows: Vec<Line> = rows.into_iter().map(|row| row.truncated(ceiling)).collect();
    let inner = rows.iter().map(Line::width).max().unwrap_or(0);

    let border = |left: &str, right: &str| {
        Line::new(vec![Span::dim(format!(
            "{left}{}{right}",
            "─".repeat(inner + 2)
        ))])
    };
    let mut out = vec![border("╭", "╮")];
    for row in rows {
        let pad = inner.saturating_sub(row.width());
        let mut spans = vec![Span::dim("│ ")];
        spans.extend(row.spans);
        // 여백도 dim 이고, 폭이 0이면 스팬을 만들지 않는다 — codex
        // `with_border_internal` 의 `if used_width < content_width` 그대로다.
        // 빈 스팬을 밀면 `write_spans` 가 쓸데없는 SGR 을 흘린다.
        if pad > 0 {
            spans.push(Span::dim(" ".repeat(pad)));
        }
        spans.push(Span::dim(" │"));
        out.push(Line::new(spans));
    }
    out.push(border("╰", "╯"));
    out
}

/// 스트리밍 마크다운 셀 — codex 의 **두 영역 모델**을 그대로 돌린다.
///
/// codex `tui/src/streaming/controller.rs` 머리말: 스트림을 "a *stable region*
/// (committed to scrollback via the animation queue in `StreamState`)" 과
/// "a *tail region* (mutable, displayed in the active-cell slot as a transient
/// stream-tail cell)" 으로 나눈다. 우리도 그 둘을 갖는다 —
/// [`Self::tick`] 이 확정 영역을 히스토리로 흘려보내고, [`Self::tail`] 이
/// 미확정 영역을 뷰포트의 활성 셀 칸에 그린다. 히스토리는 여전히 append-only 다
/// (뷰포트는 매 프레임 다시 그리는 자리이므로 꼬리를 고쳐도 계약을 깨지 않는다).
///
/// 확정 경계는 **원문 개행**이다 — codex
/// `markdown_stream.rs::commit_complete_source`: "Calling it after a delta
/// without a newline returns `None`, which prevents the live stream from
/// rendering incomplete markdown blocks that may change meaning when the rest
/// of the line arrives."
///
/// 개행이 오면 확정 원문을 통째로 다시 렌더하고(`StreamingRender::append`),
/// 새로 안정된 **표시 행**을 애니메이션 큐에 넣는다. 큐는
/// [`super::chunking`] 의 케이던스로 빠진다 — Smooth 는 틱마다 한 행,
/// 백로그가 쌓이면 `CatchUp` 이 한 번에 비운다.
///
/// # 꼬리에는 확정된 것만 들어간다
///
/// codex 의 꼬리 셀에는 **개행 안 온 원문이 들어가지 않는다**. 두 군데가 그
/// 말을 한다:
///
/// - `chatwidget/streaming.rs:485` — "Unterminated source is buffered by the
///   controller and cannot change the visible tail."
/// - `streaming/controller.rs::has_tail` 은 `enqueued_stable_len <
///   render.lines.len()` 이고, 그 `render` 는 `push_delta` 가 **개행이 온
///   델타에서만** `committed_source` 로 키운다. 미확정 원문은 어느 렌더에도
///   들어가지 않는다.
///
/// 그러니 개행 없는 긴 문단은 codex 에서 침묵한다 — 그 침묵이 원본의 모양이다.
/// 4라운드는 그 자리를 갈라 미확정 줄까지 꼬리에 그렸는데, 5라운드의 기준은
/// 바이트라 되돌렸다. 꼬리가 담는 것은 codex 와 같이 **확정됐지만 아직 큐에서
/// 안 빠진 행**뿐이다.
///
/// # 표 홀드백
///
/// 꼬리가 실제로 사는 자리는 **표**다. codex `streaming/controller.rs` 머리말:
/// "Table rendering is inherently non-incremental: adding a new row can change
/// every column's width and reshape all prior rows. The holdback mechanism
/// (`table_holdback_state`) detects pipe-table patterns (header + delimiter
/// pair) in the accumulated source and keeps content from the table header
/// onward as mutable tail until the stream finalizes."
///
/// [`super::holdback::Scanner`] 가 확정 원문을 훑어 `Confirmed`/`PendingHeader`
/// 를 내고, `Self::active_tail_budget_lines` 가 그 바이트 오프셋을 렌더 행 수로
/// 옮긴다(codex `tail_budget_from_source_start` — "the only place where those
/// coordinate systems are bridged"). 표가 없으면 예산은 0 이고 지난 라운드와
/// 같이 꼬리는 늘 빈다.
///
/// # 렌더는 증분이다
///
/// 개행이 올 때마다 **누적 원문 전체**를 다시 파싱하면 n 줄짜리 답변이 누적분을
/// n 번 훑어 일감이 n² 로 자란다. 그래서 렌더는 두 조각으로 나눠 산다:
///
/// - `rendered[..stable_lines]` — 원문 `committed[..stable_cut]` 의 표시 행.
///   한 번 만든 뒤로는 **손대지 않는다**. 폭이 바뀔 때만 다시 만든다.
/// - 그 뒤 — 가변 꼬리. 개행마다 이 조각만 다시 렌더한다.
///
/// 경계 `stable_cut` 을 고르는 것은 [`markdown::Restarts`] 다(왜 그 자리만
/// 안전한지는 그 문서에 있다). 표는 행이 늘 때마다 **지난 행까지 다시 잡히므로**
/// 홀드백이 잡은 시작 오프셋이 경계의 천장이다 — 표 구역은 늘 통째로 다시
/// 렌더된다. 그래서 지금 바이트가 그대로 지켜진다.
#[derive(Debug)]
pub struct MarkdownStream {
    /// 개행까지 확정된 원문 — codex `MarkdownStreamCollector::committed_source`.
    committed: String,
    /// 마지막 개행 뒤의 원문. 아직 확정되지 않았다.
    pending: String,
    /// 확정 원문을 지금 폭으로 렌더한 표시 행 — codex `StreamingRender::lines`.
    rendered: Vec<Line>,
    /// 큐에 넣은 행 수 — codex `enqueued_stable_len`.
    enqueued: usize,
    /// 스크롤백으로 나간 행 수 — codex `emitted_stable_len`.
    emitted: usize,
    /// 확정됐지만 아직 안 나간 행들. 넣은 시각을 함께 들고 있어야
    /// 케이던스 정책이 "가장 오래 기다린 행의 나이" 를 볼 수 있다.
    queue: VecDeque<(Line, Instant)>,
    policy: Policy,
    width: usize,
    style: Option<Style>,
    started: bool,
    /// 표 홀드백 스캐너 — codex `StreamCore::holdback_scanner`.
    holdback: holdback::Scanner,
    /// 표 앞 확정 구간의 행 수 캐시 — codex `StablePrefixLenCache`. 표가
    /// 한 행씩 들어오는 동안 같은 앞부분을 되풀이해 렌더하지 않기 위한 것이다.
    stable_prefix: Option<(usize, usize, usize)>,
    /// 다시 시작해도 되는 원문 경계를 찾는 스캐너.
    restarts: markdown::Restarts,
    /// 가변 꼬리가 시작하는 원문 오프셋. 그 앞은 `rendered` 에 그대로 산다.
    stable_cut: usize,
    /// `rendered` 앞쪽에서 확정 접두어가 차지한 행 수 — 정의상
    /// `render(&committed[..stable_cut]).len()` 과 같은 값이다. 접두어와 꼬리
    /// 사이의 이음매 빈 줄은 여기 안 든다(꼬리를 그릴 때마다 새로 밀린다).
    stable_lines: usize,
    /// 확정 접두어에서 **다듬겨 나간 꼬리 빈 줄**. `rendered` 밖에 둔다
    /// ([`Self::render_mutable_tail`] 이 왜인지 적는다).
    stable_pad: Vec<Line>,
    /// 확정 접두어가 이미 `• ` 마커를 가져갔는지.
    stable_marker: bool,
    /// Retained Syntect state for an append-only top-level code fence.
    open_code_fence: Option<markdown::OpenCodeFence>,
    /// Blank lines at the open fence's mutable tail. Canonical rendering
    /// withholds them until later non-blank content proves they are internal.
    open_fence_pad: Vec<Line>,
    kind: MarkdownStreamKind,
    reasoning_transcript: Option<String>,
}

impl MarkdownStream {
    /// 답변 셀.
    #[must_use]
    pub fn answer(width: usize) -> Self {
        Self {
            committed: String::new(),
            pending: String::new(),
            rendered: Vec::new(),
            enqueued: 0,
            emitted: 0,
            queue: VecDeque::new(),
            policy: Policy::default(),
            width: width.max(1),
            style: None,
            started: false,
            holdback: holdback::Scanner::new(),
            stable_prefix: None,
            restarts: markdown::Restarts::new(),
            stable_cut: 0,
            stable_lines: 0,
            stable_pad: Vec::new(),
            stable_marker: false,
            open_code_fence: None,
            open_fence_pad: Vec::new(),
            kind: MarkdownStreamKind::Answer,
            reasoning_transcript: None,
        }
    }

    /// reasoning 셀 — 본문 전체에 dim+italic 을 덧입힌다.
    #[must_use]
    pub fn reasoning(width: usize) -> Self {
        Self {
            style: Some(Style::new().dim().italic()),
            kind: MarkdownStreamKind::Reasoning,
            ..Self::answer(width)
        }
    }

    /// 아직 아무것도 스크롤백으로 내보내지 않았는지.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        !self.started
    }

    /// Pending rendered lines, read only by the opt-in frame probe.
    pub(super) fn queued_lines(&self) -> usize {
        self.queue.len()
    }

    /// 큐에 대기 중인 행이 있는지 — 커밋 틱을 돌릴 이유가 있는지의 판단.
    #[must_use]
    pub fn is_draining(&self) -> bool {
        !self.queue.is_empty()
    }

    /// 델타를 먹인다. 개행이 왔으면 확정 원문을 다시 렌더하고 큐를 채운다.
    pub fn push(&mut self, text: &str) {
        if self.kind == MarkdownStreamKind::Reasoning {
            self.pending.push_str(text);
            return;
        }
        self.pending.push_str(text);
        if !text.contains('\n') {
            return;
        }
        let Some(cut) = self.pending.rfind('\n') else {
            return;
        };
        let cut_end = cut + 1;
        let complete = self.pending[..cut_end].to_string();
        self.pending.drain(..cut_end);
        self.committed.push_str(&complete);
        // 스캐너는 **커밋되는 그 조각**만 순서대로 받는다 — codex
        // `push_delta` 가 `commit_complete_source()` 의 범위를 그대로 넘기는
        // 자리다. 끝나지 않은 행은 여기 오지 않는다.
        self.holdback.push_source_chunk(&complete);
        self.restarts.push_source_chunk(&complete);
        self.sync_render(Some(&complete));
        self.sync_stable_queue();
    }

    /// 폭이 바뀌었다 — codex `streaming/controller.rs::set_width` 의 자리.
    /// 새 폭으로 한 번 다시 렌더하고 큐를 `emitted` 에서 재구성한다.
    ///
    /// 이미 스크롤백으로 나간 행은 그대로다. 그 행들이 **무엇을 말했는가**가
    /// 기준이고, 그것은 행이 아니라 **줄**로 센다: 어느 한 행이라도 나간 줄은
    /// 통째로 나간 것이다 — painter 의 링이 그 줄을 들고 있어 새 폭으로 화면
    /// 위에 다시 세운다([`Line::origin`]). 그래서 새 폭에서 아직 남은 것은
    /// 정확히 **안 나간 줄들의 행**이다. codex 는 행 수를 그대로 옮기고 "keep
    /// at least one line un-emitted" 로 한 행을 강제로 되돌렸는데, 그 한 행이
    /// 링의 재구성과 겹쳐 문단이 화면에 두 번 섰다(2026-09-03, 리사이즈 e2e).
    pub fn set_width(&mut self, width: usize) {
        let width = width.max(1);
        if width == self.width {
            return;
        }
        self.width = width;
        if self.committed.is_empty() {
            return;
        }
        let lines_out = logical_lines(&self.rendered[..self.emitted.min(self.rendered.len())]);
        self.stable_prefix = None;
        self.open_code_fence = None;
        self.open_fence_pad.clear();
        // 경계는 폭과 무관한 **원문 좌표**라 그대로 쓴다 — 새 폭으로 확정
        // 접두어를 한 번, 꼬리를 한 번 세우면 끝이다.
        self.rebuild_render();
        self.emitted = rows_of_first_lines(&self.rendered, lines_out);
        self.queue.clear();
        self.enqueued = self.emitted;
        self.sync_stable_queue();
    }

    /// 미확정 꼬리로 붙잡아 둘 행 수 — codex `active_tail_budget_lines`.
    ///
    /// 표가 잡히면(`Confirmed` 나 `PendingHeader`) 그 시작 오프셋부터 전부
    /// 꼬리다: 행이 늘면 열 폭이 다시 잡히기 때문이다. `PendingHeader` 는
    /// 헤더 후보 줄부터만 붙잡아 그 앞 산문은 계속 흐르게 한다. 표가 없으면
    /// 0 이고 모든 것이 곧바로 확정된다.
    fn active_tail_budget_lines(&mut self) -> usize {
        match self.holdback.state() {
            holdback::State::Confirmed { table_start: start }
            | holdback::State::PendingHeader {
                header_start: start,
            } => self.tail_budget_from_source_start(start),
            holdback::State::None => 0,
        }
    }

    /// 원문 오프셋을 꼬리 행 수로 옮긴다 — 스캐너는 바이트로, 큐는 렌더 행으로
    /// 셈하므로 codex 는 이 함수를 "the only place where those coordinate
    /// systems are bridged" 라고 적는다.
    fn tail_budget_from_source_start(&mut self, source_start: usize) -> usize {
        if source_start == 0 {
            return self.rendered.len();
        }
        let source_start = source_start.min(self.committed.len());
        let prefix_len = self.stable_prefix_len(source_start);
        self.rendered.len().saturating_sub(prefix_len)
    }

    /// 표 앞 구간을 지금 폭으로 렌더한 행 수. 캐시가 있는 이유는 원본과 같다 —
    /// 표가 빽빽하게 흘러올 때 확정 줄마다 이 경로를 밟는다.
    fn stable_prefix_len(&mut self, source_start: usize) -> usize {
        if let Some((cached_start, cached_width, len)) = self.stable_prefix {
            if cached_start == source_start && cached_width == self.width {
                return len;
            }
        }
        let cut = source_start.min(self.committed.len());
        let len = self.prefix_len(cut);
        self.stable_prefix = Some((source_start, self.width, len));
        len
    }

    /// `committed[..cut]` 을 지금 폭으로 렌더했을 때의 **행 수**.
    ///
    /// 확정 경계가 `cut` 보다 앞이면 그 앞부분의 행 수는 이미 안다
    /// (`stable_lines + stable_pad`) — 경계 뒤만 렌더하고 이음매 빈 줄 하나를
    /// 더하면 된다. 표가 아직 확정 전(`PendingHeader`)이라 이 경로가 줄마다
    /// 도는 원문이 있어서(파이프 하나 낀 산문) 그 자리가 아프다.
    fn prefix_len(&self, cut: usize) -> usize {
        if cut == self.stable_cut && self.stable_cut > 0 {
            // `stable_lines` 는 정의상 `render(&committed[..stable_cut]).len()`
            // 이다 — 다듬긴 빈 줄은 `stable_pad` 로 빠져 있다.
            return self.stable_lines;
        }
        if self.stable_cut == 0 || cut < self.stable_cut {
            return self.render(&self.committed[..cut]).len();
        }
        let middle = self.segment(&self.committed[self.stable_cut..cut], !self.stable_marker);
        if middle.lines.is_empty() {
            // 뒤 조각이 비면 문서 끝의 다듬기가 이음매를 넘어간다 — 그때는 세지
            // 말고 그냥 렌더한다.
            return self.render(&self.committed[..cut]).len();
        }
        self.stable_lines + self.stable_pad.len() + 1 + middle.lines.len()
    }

    /// 커밋 애니메이션 한 틱 — 이번에 스크롤백으로 갈 행들.
    pub fn tick(&mut self, now: Instant) -> Vec<Line> {
        let snapshot = Snapshot {
            queued_lines: self.queue.len(),
            oldest_age: self
                .queue
                .front()
                .map(|(_, at)| now.saturating_duration_since(*at)),
        };
        let plan = self.policy.decide(snapshot, now);
        self.drain(match plan {
            DrainPlan::Single => 1,
            // 대량 백로그 유입 시에도 한 틱에 최대 16행으로 스무딩하여
            // 터미널 스크롤 점프를 방지하고 8.33ms 고빈도 티커와 조화롭게 수렴한다.
            DrainPlan::Batch(count) => count.clamp(1, 16),
        })
    }

    /// 남은 원문까지 확정하고 큐를 비운다 — codex `finalize_remaining` 은
    /// "re-renders from the full raw source" 하고 아직 안 나간 부분을 준다.
    pub fn finish(&mut self) -> Vec<Line> {
        if self.kind == MarkdownStreamKind::Reasoning {
            let mut source = std::mem::take(&mut self.committed);
            source.push_str(&std::mem::take(&mut self.pending));
            let (header, content) = split_reasoning_summary_parts(&[source]);
            let title_only = content
                .strip_prefix("**")
                .and_then(|content| content.strip_suffix("**"))
                .is_some_and(|content| !content.is_empty() && !content.contains("**"));
            let transcript_only = header.is_empty() && !title_only;
            let transcript_content = content.trim().to_string();
            self.reasoning_transcript = (!transcript_content.is_empty()).then_some(transcript_content);
            self.kind = MarkdownStreamKind::Answer;
            if transcript_only {
                self.policy.reset();
                return Vec::new();
            }
            self.pending = content;
        }
        let mut source = std::mem::take(&mut self.committed);
        source.push_str(&std::mem::take(&mut self.pending));
        if source.is_empty() {
            return Vec::new();
        }
        if !source.ends_with('\n') {
            source.push('\n');
        }
        self.committed = source;
        // 경계 스캐너에 아직 안 준 꼬리를 마저 준다 — 오프셋이 `committed` 와
        // 어긋나면 다음 경계가 엉뚱한 자리를 가리킨다.
        let seen = self.restarts.offset();
        if seen < self.committed.len() {
            self.restarts.push_source_chunk(&self.committed[seen..]);
        }
        self.sync_render(None);
        self.queue.clear();
        let start = self.emitted.min(self.rendered.len());
        let out = self.rendered[start..].to_vec();
        self.emitted = self.rendered.len();
        self.enqueued = self.rendered.len();
        self.policy.reset();
        self.open_code_fence = None;
        self.open_fence_pad.clear();
        // 표가 열려 있던 채로 끝나도 여기서 전부 커밋된다 — codex
        // `finalize_remaining` 은 "re-renders from the full raw source" 하고
        // 아직 안 나간 부분을 준다. 홀드백 판정은 비우되 스캐너 오프셋은
        // `committed` 끝에 맞춘다 — 같은 스트림에 이어 push 해도 다음 표
        // 경계가 원문 좌표를 그대로 가리킨다.
        self.holdback.settle(self.committed.len());
        self.stable_prefix = None;
        self.opened(out)
    }

    /// Take the cleaned reasoning body destined for Ctrl+T transcript history.
    pub fn take_reasoning_transcript(&mut self) -> Option<String> {
        self.reasoning_transcript.take()
    }

    /// 뷰포트의 활성 셀 칸에 그릴 꼬리 — **확정 렌더**에서 큐에 들어간 만큼을
    /// 뺀 나머지다. codex `StreamCore::has_tail` 이 `enqueued_stable_len <
    /// render.lines.len()` 인 것과 같은 경계다(`emitted` 에서 시작하면 큐에
    /// 있는 행이 꼬리에도 나와 두 번 보인다).
    ///
    /// `Self::pending` 은 여기 들어오지 않는다 — 위 머리말의 두 인용이 그
    /// 규칙이다. 큐 정책이 확정 행을 전부 흘려보내면 꼬리는 비고, 다음 개행이
    /// 올 때까지 화면은 `Working` 만 보여 준다.
    #[must_use]
    pub fn tail(&self) -> Vec<Line> {
        if self.enqueued >= self.rendered.len() {
            return Vec::new();
        }
        self.rendered[self.enqueued..].to_vec()
    }

    /// 확정 영역의 경계를 밀고 새 행을 큐에 넣는다 — codex
    /// `sync_stable_queue` + `compute_target_stable_len`. 목표는 렌더 전체에서
    /// **꼬리 예산**을 뺀 것이고, 이미 나간 만큼보다는 내려가지 않는다.
    ///
    /// 구조가 다시 쓰이면(표 하나가 확정되며 경계가 뒤로 밀리면) 목표가
    /// `enqueued` 보다 작아진다 — 그때는 큐를 버리고 지금 스냅숏에서 다시
    /// 세운다. 이미 스크롤백으로 나간 행은 건드릴 수 없으므로 `emitted` 가
    /// 바닥이다.
    fn sync_stable_queue(&mut self) {
        let budget = self.active_tail_budget_lines();
        let target = self
            .rendered
            .len()
            .saturating_sub(budget)
            .max(self.emitted);
        if target < self.enqueued {
            self.queue.clear();
            if self.emitted < target {
                let now = Instant::now();
                for line in &self.rendered[self.emitted..target] {
                    self.queue.push_back((line.clone(), now));
                }
            }
            self.enqueued = target;
            return;
        }
        if target == self.enqueued {
            return;
        }
        let now = Instant::now();
        for line in &self.rendered[self.enqueued..target] {
            self.queue.push_back((line.clone(), now));
        }
        self.enqueued = target;
    }

    fn drain(&mut self, rows: usize) -> Vec<Line> {
        let take = rows.min(self.queue.len());
        if take == 0 {
            return Vec::new();
        }
        let out: Vec<Line> = self.queue.drain(..take).map(|(line, _)| line).collect();
        self.emitted += out.len();
        self.opened(out)
    }

    /// 셀의 첫 방출에는 앞 빈 줄을 하나 붙인다 — 캡처의 답변 셀도 그렇다.
    fn opened(&mut self, out: Vec<Line>) -> Vec<Line> {
        if out.is_empty() {
            return out;
        }
        if self.started {
            return out;
        }
        self.started = true;
        let mut with_gap = vec![Line::empty()];
        with_gap.extend(out);
        with_gap
    }

    /// 마크다운에 줄 예산으로 주는 **내용 폭** — 접두어를 뺀 나머지다.
    /// [`prefixed`] 가 접을 때 쓰는 것과 같은 값이라, 표가 그 폭에 맞춰 나오면
    /// 뒤의 접기를 그대로 통과한다. codex 도 답변 스트림에
    /// `current_stream_width(/*reserved_cols*/ 2)` 를 준다.
    fn content_width(&self) -> usize {
        self.width.saturating_sub(Prefix::bullet().width()).max(1)
    }

    /// 원문을 지금 폭의 표시 행으로. 마커(`• `)는 내용이 있는 첫 행이 가져가고
    /// 나머지는 두 칸 들여쓴다 — 원문 전체를 매번 같은 규칙으로 렌더하므로
    /// 행 번호가 곧 안정된 좌표가 된다.
    fn render(&self, source: &str) -> Vec<Line> {
        self.segment(source, true).lines
    }

    /// 원문 **조각** 하나를 표시 행으로. `first` 는 `• ` 마커가 아직 안 나갔다는
    /// 뜻이다.
    fn segment(&self, source: &str, first: bool) -> Segment {
        let mut md = markdown::render_segment(source, Some(self.content_width()));
        if let Some(style) = self.style {
            md = md.into_iter().map(|line| line.patched(style)).collect();
        }
        // 꼬리 빈 줄 다듬기는 **마크다운 줄** 층에서 자른다 — `render_wrapped`
        // 가 자르는 그 층이다. 접두어를 입히고 접은 뒤에 자르면 긴 줄이 접힌
        // 마지막 조각 하나가 어긋날 수 있다.
        let keep = md
            .iter()
            .rposition(|line| !line.plain().trim().is_empty())
            .map_or(0, |at| at + 1);
        let trailing = md.split_off(keep);
        let (lines, marker) = prefixed_marking(&md, self.width, &Prefix::bullet(), first);
        let (pad, _) =
            prefixed_marking(&trailing, self.width, &Prefix::bullet(), first && !marker);
        Segment { lines, pad, marker }
    }

    /// 확정 접두어는 그대로 두고 **가변 꼬리만** 다시 렌더한다 — 개행이 올
    /// 때마다 여기를 지난다.
    fn sync_render(&mut self, committed_chunk: Option<&str>) {
        if let Some(committed_chunk) = committed_chunk {
            if self.append_open_code_fence(committed_chunk) {
                return;
            }
        }
        if self.restarts.is_poisoned() && self.stable_cut > 0 {
            // 링크 참조 정의가 뒤늦게 왔다 — 그것은 문서 전역이라 이미 그린
            // 앞줄의 뜻까지 바꾼다. 접두어 보존을 버리고 처음부터 다시 세운다.
            self.forget_stable();
        }
        self.advance_stable();
        self.render_mutable_tail();
        self.open_code_fence = markdown::OpenCodeFence::detect_final(
            &self.committed[self.stable_cut..],
            self.committed.len(),
        );
        self.open_fence_pad = self.open_code_fence.as_ref().map_or_else(Vec::new, |fence| {
            vec![Line::empty(); fence.trailing_blank_lines(&self.committed)]
        });
    }

    fn append_open_code_fence(&mut self, committed_chunk: &str) -> bool {
        let Some(fence) = self.open_code_fence.take() else {
            return false;
        };
        if fence.starts_after(self.stable_cut)
            && !fence.has_visible_content(&self.committed)
            && committed_chunk
                .lines()
                .any(|line| !line.trim().is_empty())
        {
            self.open_fence_pad.clear();
            return false;
        }
        let Some((fence, mut lines)) = fence.append(&self.committed, committed_chunk) else {
            self.open_fence_pad.clear();
            return false;
        };
        if let Some(style) = self.style {
            lines = lines
                .into_iter()
                .map(|line| line.patched(style))
                .collect();
        }
        let mut visible = std::mem::take(&mut self.open_fence_pad);
        visible.append(&mut lines);
        let keep = visible
            .iter()
            .rposition(|line| !line.plain().trim().is_empty())
            .map_or(0, |at| at + 1);
        self.open_fence_pad = visible.split_off(keep);
        if !visible.is_empty() {
            if self.rendered.len() == self.stable_lines && self.stable_cut > 0 {
                self.rendered.extend(self.stable_pad.iter().cloned());
                self.rendered.push(Line::empty());
            }
            let first = !self.stable_marker && self.rendered.is_empty();
            self.rendered
                .extend(prefixed(&visible, self.width, &Prefix::bullet(), first));
        }
        self.open_code_fence = Some(fence);
        true
    }

    fn forget_stable(&mut self) {
        self.open_code_fence = None;
        self.open_fence_pad.clear();
        self.stable_cut = 0;
        self.stable_lines = 0;
        self.stable_pad.clear();
        self.stable_marker = false;
        self.rendered.clear();
    }

    /// 경계를 지금 놓을 수 있는 데까지 민다. 새로 확정된 구간은 딱 한 번
    /// 렌더돼 `rendered` 앞쪽에 눌러앉는다.
    fn advance_stable(&mut self) {
        // 표가 잡히면 그 시작이 천장이다 — codex 가 표 구역을 통째로 미확정
        // 꼬리로 붙잡는 것과 같은 이유로, 증분도 그 앞에서 멈춰야 한다.
        let ceiling = match self.holdback.state() {
            holdback::State::None => self.committed.len(),
            holdback::State::PendingHeader { header_start: at }
            | holdback::State::Confirmed { table_start: at } => at,
        };
        let Some(cut) = self.restarts.take_upto(ceiling) else {
            return;
        };
        if cut <= self.stable_cut {
            return;
        }
        let mut fresh = self.segment(&self.committed[self.stable_cut..cut], !self.stable_marker);
        if fresh.lines.is_empty() {
            // 새 구간이 한 행도 안 냈다면 이음매 빈 줄이 하나여야 하는지 둘이어야
            // 하는지 알 수 없다. 그런 원문은 증분에서 뺀다(전체 렌더가 정답이다).
            return;
        }
        self.rendered.truncate(self.stable_lines);
        if self.stable_cut > 0 {
            let mut pad = std::mem::take(&mut self.stable_pad);
            self.rendered.append(&mut pad);
            self.rendered.push(Line::empty());
        }
        self.rendered.append(&mut fresh.lines);
        self.stable_lines = self.rendered.len();
        self.stable_pad = fresh.pad;
        self.stable_marker |= fresh.marker;
        self.stable_cut = cut;
    }

    /// `rendered` 의 꼬리를 지금 원문으로 다시 그린다.
    ///
    /// 꼬리가 한 행도 안 낼 때가 있다 — 담장이 열린 채로 끝난 자리가 그렇다.
    /// 그때 원본의 꼬리 다듬기는 이음매를 넘어 확정 구간의 꼬리 빈 줄까지
    /// 먹는다. 그래서 그 빈 줄들은 `rendered` 밖([`Self::stable_pad`])에 두고
    /// **꼬리가 살아 있을 때만** 도로 끼운다.
    fn render_mutable_tail(&mut self) {
        let tail = self.segment(&self.committed[self.stable_cut..], !self.stable_marker);
        self.rendered.truncate(self.stable_lines);
        if tail.lines.is_empty() {
            return;
        }
        if self.stable_cut > 0 {
            self.rendered.extend(self.stable_pad.iter().cloned());
            self.rendered.push(Line::empty());
        }
        self.rendered.extend(tail.lines);
    }

    /// 증분 렌더가 전체 렌더와 어긋난 **첫 행**. 같으면 `None`.
    ///
    /// 증분화의 계약은 하나다 — 화면 바이트가 그대로일 것. 그것을 지키는 것은
    /// [`markdown::Restarts`] 가 고르는 경계이고, 이 함수가 그 경계를 매 커밋
    /// 검산한다(테스트 전용).
    #[cfg(test)]
    fn disagreement_with_a_full_render(&self) -> Option<(usize, String, String)> {
        let full = self.render(&self.committed);
        let rows = |lines: &[Line]| -> Vec<String> {
            lines
                .iter()
                .map(|line| {
                    let mut out = String::new();
                    super::ansi::write_spans(line, &mut out);
                    out
                })
                .collect()
        };
        let (want, got) = (rows(&full), rows(&self.rendered));
        (0..want.len().max(got.len())).find_map(|at| {
            let (left, right) = (want.get(at), got.get(at));
            (left != right).then(|| {
                (
                    at,
                    left.cloned().unwrap_or_else(|| "<없음>".to_string()),
                    right.cloned().unwrap_or_else(|| "<없음>".to_string()),
                )
            })
        })
    }

    /// 지금 폭으로 `rendered` 를 처음부터 세운다 — 폭이 바뀔 때만.
    fn rebuild_render(&mut self) {
        self.rendered.clear();
        self.stable_lines = 0;
        self.stable_pad.clear();
        self.stable_marker = false;
        if self.stable_cut > 0 {
            let stable = self.segment(&self.committed[..self.stable_cut], true);
            if stable.lines.is_empty() {
                self.stable_cut = 0;
            } else {
                self.rendered = stable.lines;
                self.stable_lines = self.rendered.len();
                self.stable_pad = stable.pad;
                self.stable_marker = stable.marker;
            }
        }
        self.render_mutable_tail();
    }
}

#[cfg(test)]
mod tests {
    use super::super::markdown;
    use super::super::wrap::wrap_line;
    use super::prefixed;
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    use runtime::message_stream::{TodoResultItem, TodoResultStatus};

    use super::{
        boot_card, plan_update_cell, proposed_plan_cell_for_palette,
        split_reasoning_summary_parts, user_cell, user_cell_for_palette,
        MarkdownStream, Prefix,
    };
    use crate::tui::ansi::Line;

    /// The 2026-09-03 screenshot, pinned at the stream: a bullet whose inline
    /// code token is wider than a narrow split pane stood as one row, twenty
    /// blank rows, then the token. The bullet's rows must be contiguous.
    #[test]
    fn a_bullet_with_an_overwide_token_wraps_without_blank_rows() {
        let md = "- 하지만 바로 다음 줄의 `agent_launch_env_with_lock(crates/zerocode-hookd/src/codex_install.rs:1020)`이 ZeroCode 공용 미러 디렉터리(`codex-mirror`)를 `CODEX_HOME`으로 반환하여 앞서 설정한 계정별 경로를 덮어씁니다.\n\n다음 문단.\n";
        let mut stream = MarkdownStream::answer(60);
        stream.push(md);
        let mut rows: Vec<Line> = Vec::new();
        for _ in 0..200 {
            rows.extend(stream.tick(Instant::now()));
        }
        rows.extend(stream.finish());
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        let first = plain.iter().position(|row| row.contains("하지만 바로 다음 줄의")).expect("the bullet");
        let last = plain.iter().position(|row| row.contains("덮어씁니다")).expect("its last row");
        assert!(
            plain[first..=last].iter().all(|row| !row.trim().is_empty()),
            "the bullet's rows are contiguous:\n{}",
            plain.join("\n")
        );
        assert!(last - first <= 5, "a few rows, not one per character: {}", plain.join("\n"));
    }

    /// 커밋 틱을 원하는 만큼 돌려 나온 행들을 모은다.
    fn drain(stream: &mut MarkdownStream, ticks: usize) -> Vec<String> {
        let start = Instant::now();
        let mut out = Vec::new();
        for step in 0..ticks {
            let now = start + Duration::from_millis(step as u64);
            out.extend(stream.tick(now).iter().map(Line::plain));
        }
        out
    }

    #[test]
    fn the_user_cell_matches_the_captured_shape() {
        let lines = user_cell("안녕!", 40);
        let plain: Vec<String> = lines.iter().map(Line::plain).collect();
        assert_eq!(
            plain,
            vec![String::new(), String::new(), "› 안녕!".to_string(), String::new()]
        );
        let marker = &lines[2].spans[0];
        assert_eq!(marker.text, "› ");
        assert!(marker.style.bold && marker.style.dim);
    }

    #[test]
    fn a_long_user_line_wraps_under_the_two_space_indent() {
        let lines = user_cell("alpha beta gamma delta", 14);
        let plain: Vec<String> = lines.iter().map(Line::plain).collect();
        assert_eq!(plain[2], "› alpha beta");
        assert_eq!(plain[3], "  gamma delta");
    }

    #[test]
    fn user_and_proposed_plan_cells_use_the_dark_background_tint() {
        use crate::tui::ansi::Color;
        use crate::tui::palette::{ColorLevel, TerminalPalette};

        let terminal = TerminalPalette::new(
            (255, 255, 255),
            (0, 0, 0),
            ColorLevel::TrueColor,
        );
        let expected = Some(Color::Rgb(30, 30, 30));
        let user = user_cell_for_palette("hello", 40, Some(terminal));
        // codex history_cell/messages.rs:265-270: the tinted band is ONE
        // leading blank + body + ONE trailing blank, so the text sits in the
        // band's vertical center. The first blank is the untinted breathing
        // gap between cells and stays outside the band.
        assert_eq!(user[0].style.bg, None, "inter-cell gap stays untinted");
        assert!(
            user[1..].iter().all(|line| line.style.bg == expected),
            "band = blank + body + blank, all tinted"
        );

        let plan = proposed_plan_cell_for_palette("one step", None, 40, Some(terminal));
        assert_eq!(plan[1].style.bg, None, "plan header stays outside the panel");
        assert!(plan[2..plan.len() - 1]
            .iter()
            .all(|line| line.style.bg == expected));
        assert_eq!(plan.last().expect("approval hint").style.bg, None);
    }

    #[test]
    fn an_empty_plan_names_the_missing_steps() {
        let plain: Vec<String> = plan_update_cell(&[], 40).iter().map(Line::plain).collect();

        assert_eq!(
            plain,
            vec![
                "• Updated Plan".to_string(),
                "  └ (no steps provided)".to_string()
            ]
        );
    }

    #[test]
    fn plan_steps_use_the_codex_status_styles() {
        let items = vec![
            TodoResultItem {
                content: "done".to_string(),
                active_form: "doing".to_string(),
                status: TodoResultStatus::Completed,
            },
            TodoResultItem {
                content: "current".to_string(),
                active_form: "doing current".to_string(),
                status: TodoResultStatus::InProgress,
            },
            TodoResultItem {
                content: "later".to_string(),
                active_form: "doing later".to_string(),
                status: TodoResultStatus::Pending,
            },
        ];
        let lines = plan_update_cell(&items, 40);
        let completed = lines[1].spans.last().expect("completed step").style;
        let current = lines[2].spans.last().expect("current step").style;
        let pending = lines[3].spans.last().expect("pending step").style;

        assert!(completed.strike && completed.dim);
        assert!(current.bold && current.fg == Some(crate::tui::ansi::Color::CYAN));
        assert!(pending.dim);
    }

    /// 확정은 개행에서만. 그 전에는 큐도 꼬리도 비어 있다 — codex
    /// `push_delta` 가 개행이 온 델타에서만 렌더를 키우기 때문이다.
    #[test]
    fn markdown_streams_commit_only_at_source_newlines() {
        let mut stream = MarkdownStream::answer(40);
        stream.push("### 자기");
        assert!(drain(&mut stream, 4).is_empty(), "nothing is committed yet");
        assert!(stream.tail().is_empty(), "unterminated source is invisible");

        stream.push("소개\n");
        let rows = drain(&mut stream, 4);
        assert_eq!(rows, vec![String::new(), "• ### 자기소개".to_string()]);
        assert!(stream.tail().is_empty());

        stream.push("\n- 저는 개발자입니다\n");
        let rows = drain(&mut stream, 4);
        assert_eq!(rows, vec![String::new(), "  - 저는 개발자입니다".to_string()]);
    }

    /// Smooth 기어는 틱마다 한 행이다 — codex `chunking.rs` 의 기본 케이던스.
    #[test]
    fn smooth_mode_lets_one_row_out_per_tick() {
        let mut stream = MarkdownStream::answer(12);
        // 폭 12 → `• ` 를 뺀 본문 열 칸. 세 행으로 접힌다.
        stream.push("alpha beta gamma delta\n");
        let start = Instant::now();
        let first: Vec<String> = stream.tick(start).iter().map(Line::plain).collect();
        assert_eq!(first, vec![String::new(), "• alpha beta".to_string()]);
        let second: Vec<String> = stream.tick(start).iter().map(Line::plain).collect();
        assert_eq!(second, vec!["  gamma".to_string()]);
        let third: Vec<String> = stream.tick(start).iter().map(Line::plain).collect();
        assert_eq!(third, vec!["  delta".to_string()]);
        assert!(stream.tick(start).is_empty());
    }

    /// 개행 없는 문단은 큐에도 꼬리에도 없다 — codex
    /// `chatwidget/streaming.rs:485` "Unterminated source is buffered by the
    /// controller and cannot change the visible tail." 그 침묵이 원본의 모양이고
    /// (4라운드가 여기를 갈라 뒀던 자리), 개행이 오면 한꺼번에 내려온다.
    #[test]
    fn an_unterminated_paragraph_is_invisible_until_its_newline() {
        let mut stream = MarkdownStream::answer(12);
        stream.push("alpha beta ");
        assert!(drain(&mut stream, 4).is_empty());
        assert!(stream.tail().is_empty());

        stream.push("gamma");
        assert!(stream.tail().is_empty());

        // 개행이 오면 그 줄이 큐를 타고 히스토리로 내려가고 꼬리는 빈다.
        stream.push("\n");
        let rows = drain(&mut stream, 8);
        assert_eq!(
            rows,
            vec![
                String::new(),
                "• alpha beta".to_string(),
                "  gamma".to_string(),
            ]
        );
        assert!(stream.tail().is_empty());
    }

    /// 표가 없는 원문에서 꼬리는 **언제나 빈다** — codex
    /// `active_tail_budget_lines` 가 `TableHoldbackState::None` 에서 0 을
    /// 돌려주므로 확정 렌더가 전부 큐로 가고 `enqueued == rendered.len()` 이
    /// 유지된다. 그래서 활성 셀 칸은 답변 스트림에 쓰이지 않고 `Working` 이
    /// 그 자리를 지킨다.
    #[test]
    fn without_a_table_the_tail_is_always_empty() {
        let mut stream = MarkdownStream::answer(12);
        stream.push("alpha beta gamma\n");
        assert!(stream.tail().is_empty(), "everything is queued");
        // 큐를 한 틱 비워도 — 꼬리 경계는 `emitted` 가 아니라 `enqueued` 다.
        assert_eq!(stream.tick(Instant::now()).len(), 2);
        assert!(stream.tail().is_empty(), "the queue owns the stable region");
        stream.push("delta");
        assert!(stream.tail().is_empty(), "delta is still buffered");
    }

    /// 표가 열리면 꼬리가 산다 — 이 어휘에서 `active_tail_budget_lines` 가
    /// 처음으로 0 이 아닌 값을 낸다. 헤더 + 구분 줄이 짝을 이룬 뒤로는 표
    /// 구역이 통째로 미확정이고, 그 앞 산문은 그대로 확정된다(codex 의
    /// `PendingHeader` → `Confirmed` 경계가 "content from the table header
    /// onward" 만 붙잡기 때문이다).
    #[test]
    fn a_table_holds_its_region_in_the_tail_and_lets_earlier_prose_commit() {
        let mut stream = MarkdownStream::answer(40);
        stream.push("여기 표가 있습니다\n\n");
        let prose = drain(&mut stream, 8);
        assert_eq!(
            prose,
            vec![String::new(), "• 여기 표가 있습니다".to_string()],
            "prose before the table still commits"
        );

        stream.push("| A | B |\n| --- | --- |\n");
        assert!(
            drain(&mut stream, 8).is_empty(),
            "the confirmed table region is held back"
        );
        let tail: Vec<String> = stream.tail().iter().map(Line::plain).collect();
        assert_eq!(
            tail,
            vec![
                String::new(),
                "   A      B".to_string(),
                "  ━━━━━  ━━━━━".to_string(),
            ]
        );

        // 행이 늘면 앞선 행까지 다시 잡힌다 — 그것이 붙잡아 두는 이유다.
        stream.push("| alpha | beta |\n");
        assert!(drain(&mut stream, 8).is_empty());
        let tail: Vec<String> = stream.tail().iter().map(Line::plain).collect();
        assert_eq!(
            tail,
            vec![
                String::new(),
                "   A        B".to_string(),
                "  ━━━━━━━  ━━━━━━".to_string(),
                "   alpha    beta".to_string(),
            ]
        );

        // 끝나면 붙잡아 둔 것이 한 번에 내려간다.
        let flushed: Vec<String> = stream.finish().iter().map(Line::plain).collect();
        assert_eq!(
            flushed,
            vec![
                String::new(),
                "   A        B".to_string(),
                "  ━━━━━━━  ━━━━━━".to_string(),
                "   alpha    beta".to_string(),
            ]
        );
        assert!(stream.tail().is_empty());
    }

    /// `finish` 뒤에 같은 스트림으로 계속 흘려도 경계는 **누적 원문의 절대
    /// 좌표**여야 한다. 스캐너 오프셋을 0 으로 되감던 시절에는 두 번째 표의
    /// `table_start` 가 새 조각 안의 좌표라 누적 원문에서는 첫 셀 한가운데를
    /// 가리켰고, 그 뒤 전부가 홀드백으로 묶여 표 앞 산문이 스크롤백에
    /// 커밋되지 않았다.
    #[test]
    fn a_table_after_finish_keeps_absolute_boundaries() {
        let mut stream = MarkdownStream::answer(40);
        stream.push("첫 셀은 제법 긴 한 문단이다 여기까지가 첫 셀\n");
        let _ = stream.finish();

        stream.push("표 앞 산문 한 줄\n\n");
        stream.push("| a | b |\n");
        stream.push("|---|---|\n");
        let committed: Vec<String> = stream.tick(Instant::now()).iter().map(Line::plain).collect();

        assert!(
            committed.iter().any(|row| row.contains("표 앞 산문")),
            "표 앞 산문은 커밋된다: {committed:?}"
        );
        assert!(
            !committed.iter().any(|row| row.contains('│') || row.contains('┌')),
            "끝나지 않은 표는 커밋되지 않는다: {committed:?}"
        );
    }

    #[test]
    fn finishing_flushes_the_unterminated_tail() {
        let mut stream = MarkdownStream::answer(40);
        stream.push("one two three");
        let rows: Vec<String> = stream.finish().iter().map(Line::plain).collect();
        assert_eq!(rows, vec![String::new(), "• one two three".to_string()]);
        assert!(stream.finish().is_empty());
    }

    #[test]
    fn a_reasoning_stream_dims_and_italicises_its_body() {
        let mut stream = MarkdownStream::reasoning(40);
        stream.push("thinking about it\n");
        let lines = stream.finish();
        assert!(lines.is_empty(), "headerless reasoning is transcript-only");
        let transcript = stream.take_reasoning_transcript().expect("transcript body");
        let mut visible = MarkdownStream::reasoning(40);
        visible.push(&format!("**Thinking**\n\n{transcript}\n"));
        let lines = visible.finish();
        let body = lines.last().expect("body");
        assert!(body.spans[1].style.dim && body.spans[1].style.italic);
    }

    #[test]
    fn a_reasoning_heading_is_not_repeated_in_main_history() {
        let mut stream = MarkdownStream::reasoning(40);
        stream.push("**Checking the tests**\n\nThey pass.\n");

        let lines: Vec<String> = stream.finish().iter().map(Line::plain).collect();

        assert_eq!(lines, vec![String::new(), "• They pass.".to_string()]);
    }

    #[test]
    fn headerless_reasoning_is_withheld_from_main_history() {
        let mut stream = MarkdownStream::reasoning(40);
        stream.push("Detailed reasoning goes here.\n");

        assert!(stream.finish().is_empty());
        assert_eq!(
            stream.take_reasoning_transcript().as_deref(),
            Some("Detailed reasoning goes here.")
        );
    }

    #[test]
    fn a_bold_only_reasoning_title_stays_in_main_history() {
        let mut stream = MarkdownStream::reasoning(40);
        stream.push("**Confirming backend JSONL source**\n");

        let lines: Vec<String> = stream.finish().iter().map(Line::plain).collect();

        assert_eq!(
            lines,
            vec![
                String::new(),
                "• Confirming backend JSONL source".to_string()
            ]
        );
    }

    #[test]
    fn reasoning_parts_drop_standalone_empty_html_comments() {
        let parts = vec![
            "**Status**\n\n<!-- -->".to_string(),
            "**Checking tests**\n\nTests passed".to_string(),
            "<!-- -->".to_string(),
        ];

        assert_eq!(
            split_reasoning_summary_parts(&parts),
            ("**Checking tests**".to_string(), "\n\nTests passed".to_string())
        );
    }

    #[test]
    fn reasoning_parts_preserve_literal_html_comment_text() {
        let parts = vec!["**Plan**\n\nUse `<!-- -->` in JSX.".to_string()];

        assert_eq!(
            split_reasoning_summary_parts(&parts),
            ("**Plan**".to_string(), "\n\nUse `<!-- -->` in JSX.".to_string())
        );
    }

    /// 넓어진 폭에서 남은 내용이 이미 나간 행 안으로 접혀 들어가도 잃지
    /// 않는다 — 다만 스트림이 다시 내는 것이 아니라, painter 의 링이 그 줄을
    /// 새 폭으로 다시 세운다([`Line::origin`]): 어느 행이든 나간 줄은 통째로
    /// 나간 줄이다. codex 가 "keep at least one line un-emitted" 로 한 행을
    /// 되돌려 새 폭으로 다시 낸 것은 그 재구성과 겹쳐 문단이 두 번 섰다
    /// (2026-09-03, 리사이즈 e2e). 그 뒤의 줄은 그대로 온다.
    #[test]
    fn a_resize_leaves_a_line_that_had_begun_to_go_out_to_the_ring() {
        let mut stream = MarkdownStream::answer(12);
        stream.push("alpha beta gamma delta\n\nnext one\n");
        let first: Vec<String> = stream.tick(Instant::now()).iter().map(Line::plain).collect();
        assert_eq!(first, vec![String::new(), "• alpha beta".to_string()]);
        stream.set_width(40);
        let rest = drain(&mut stream, 8);
        assert_eq!(rest, vec![String::new(), "  next one".to_string()]);
    }

    /// 큐도 비었고 꼬리도 없었으면 이미 나간 것을 다시 내지 않는다 — codex
    /// "avoid replaying already-emitted content after resize".
    #[test]
    fn a_resize_after_a_full_drain_replays_nothing() {
        let mut stream = MarkdownStream::answer(40);
        stream.push("alpha beta\n");
        assert_eq!(
            drain(&mut stream, 8),
            vec![String::new(), "• alpha beta".to_string()]
        );
        stream.set_width(60);
        assert!(drain(&mut stream, 8).is_empty());
    }

    /// 캡처(`codex-tui-v0.149.1-boot-trust.bin`)의 히스토리 1행은 카드
    /// 윗변이다 — 카드 **앞에 빈 줄이 없다**. 상자는 내용에 맞춰 줄어들고
    /// (`with_border`), 여백까지 dim 이다.
    #[test]
    fn the_boot_card_is_a_rounded_box_sized_to_its_content() {
        let lines = boot_card("zo", "0.1.0", "claude-opus-5", "high", "/tmp/x", 60);
        let plain: Vec<String> = lines.iter().map(Line::plain).collect();
        assert!(plain[0].starts_with('╭') && plain[0].ends_with('╮'));
        assert!(plain[1].contains(">_ zo (v0.1.0)"));
        assert!(plain[3].contains("model:     claude-opus-5 high"));
        assert!(plain[3].contains("/model to change"));
        assert!(plain[4].contains("directory: /tmp/x"));
        assert!(plain[5].starts_with('╰') && plain[5].ends_with('╯'));
        assert_eq!(lines.len(), 6);
        // 윗변 = 가장 긴 줄 + 2 — 40칸 하한이 없다.
        let inner = plain[1].chars().count() - 4;
        assert_eq!(plain[0].chars().count(), inner + 4);
        assert!(inner < 56, "the card shrank to its content: {inner}");
        // 빈 행은 dim 한 덩어리다 — codex 처럼 SGR 이 한 번만 나간다.
        let mut blank = String::new();
        crate::tui::ansi::write_spans(&lines[2], &mut blank);
        assert_eq!(blank, format!("\u{1b}[2m│ {} │", " ".repeat(inner)));
    }

    #[test]
    fn boot_card_effort_is_plain_not_magenta() {
        let lines = boot_card("zo", "0.1.0", "gpt-5", "high", "/tmp", 60);
        let effort = lines
            .iter()
            .flat_map(|line| &line.spans)
            .find(|span| span.text == "high")
            .expect("effort span");

        assert_eq!(effort.style.fg, None);
    }

    #[test]
    fn error_notice_bolds_only_its_warning_marker() {
        let lines = super::system_cell(runtime::message_stream::SystemLevel::Error, "boom", 40);
        let marker = &lines[1].spans[0];
        let body = &lines[1].spans[1];

        assert!(marker.style.bold);
        assert!(!body.style.bold);
    }

    #[test]
    fn success_notice_uses_a_dim_marker_and_plain_body() {
        let lines = super::system_cell(runtime::message_stream::SystemLevel::Success, "done", 40);
        let marker = &lines[1].spans[0];
        let body = &lines[1].spans[1];

        assert!(marker.style.dim && marker.style.fg.is_none());
        assert_eq!(body.style, crate::tui::ansi::Style::new());
    }

    // ------------------------------------------------------------------
    // 증분 렌더 — 근거와 등가성
    // ------------------------------------------------------------------

    /// 측정용 원문. 흔한 답변의 모양(헤딩·문단·리스트·담장)을 `blocks` 번
    /// 되풀이한다 — 줄 수가 늘 때 일감이 어떻게 자라는지를 재는 자다.
    fn corpus(blocks: usize) -> String {
        let mut out = String::new();
        for index in 0..blocks {
            writeln!(out, "## 구역 {index}\n").expect("문자열 쓰기는 실패하지 않는다");
            out.push_str(
                "바다는 뭍의 무엇도 따라오지 못할 참을성을 가졌고, 같은 물을 같은 \
                 돌 위로 수천 년 동안 서두르지도 불평하지도 않고 접어 넘긴다.\n\n",
            );
            out.push_str("- 첫째 항목입니다\n- 둘째 항목입니다\n- 셋째 항목입니다\n\n");
            out.push_str("```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n");
        }
        out
    }

    /// 증분 경계가 걸려 넘어질 만한 자리를 모은 것. 마크다운에서 **뒤 줄이 앞
    /// 줄의 뜻을 바꾸는** 문법이 하나씩 들어 있다.
    fn equivalence_corpus() -> Vec<&'static str> {
        vec![
            // 산문·헤딩 — 가장 흔한 모양.
            "### 자기소개\n\n저는 개발자입니다.\n\n둘째 문단입니다.\n",
            // 빈 줄로 갈린 같은 리스트 — 갈라 렌더하면 번호가 1 로 되감긴다.
            "1. 하나\n\n1. 둘\n\n1. 셋\n",
            "1. 하나\n\n2. 둘\n\n3. 셋\n",
            // 촘촘한 리스트와 중첩.
            "- 하나\n- 둘\n  - 안쪽\n- 셋\n\n뒤 문단\n",
            "- 하나\n\n- 둘\n\n뒤 문단\n",
            // 빈 줄 뒤에 들여쓴 이음 — 리스트가 계속된다.
            "- 하나\n\n  같은 항목의 둘째 문단\n\n- 둘\n",
            // 담장 — 빈 줄과 파이프를 삼킨다.
            "앞 문단\n\n```rust\nfn main() {\n\n    let a = 1 | 2;\n}\n```\n\n뒤 문단\n",
            "```\n| A | B |\n| --- | --- |\n```\n\n뒤 문단\n",
            "```md\n| A | B |\n| --- | --- |\n| 1 | 2 |\n```\n\n뒤 문단\n",
            // 들여쓰기 담장은 빈 줄을 건너 이어진다.
            "앞 문단\n\n    코드 한 줄\n\n    이어지는 코드\n\n뒤 문단\n",
            // 표 — 열 폭이 행마다 다시 잡힌다. 앞뒤 산문이 함께 있다.
            "앞 문단\n\n| Name | Count |\n| --- | --- |\n| alpha | 1 |\n| beta | 22 |\n\n뒤 문단\n",
            "| A | B |\n| --- | --- |\n| 1 | 2 |\n",
            // 인용 안의 표.
            "> | A | B |\n> | --- | --- |\n> | 1 | 2 |\n\n뒤 문단\n",
            // 인용과 가로줄.
            "인용 앞\n\n> 인용문\n> 이어지는 줄\n\n---\n\n***\n\n뒤 문단\n",
            // 게으른 이음 — 빈 줄이 없으면 한 문단이다.
            "첫 줄\n| A | B |\n둘째 줄\n\n뒤 문단\n",
            // setext 밑줄.
            "제목\n===\n\n본문\n\n다른 제목\n---\n\n본문\n",
            // 링크 참조 정의는 문서 전역이라 **뒤에 나와도 앞을 바꾼다**.
            "여기 [문서] 를 보라\n\n다른 문단\n\n[문서]: https://example.com\n",
            // 빈 줄을 삼키는 원시 HTML 블록.
            "앞 문단\n\n<pre>\n한 줄\n\n두 줄\n</pre>\n\n뒤 문단\n",
            "앞 문단\n\n<!-- 주석\n\n이어지는 주석 -->\n\n뒤 문단\n",
            // 담장 안의 빈 줄로 끝나는 원문(꼬리 다듬기 자리).
            "앞 문단\n\n```text\n코드\n\n```\n\n뒤 문단\n",
            // 담장 줄 끝의 공백은 살아남는다 — 좁은 폭에서 접히면 마지막 조각이
            // 통째로 공백이 될 수 있는 자리다(다듬기를 어느 층에서 하느냐의 시험).
            "앞 문단\n\n```text\n코드                                        \n```\n\n뒤\n",
            // 파이프 하나가 낀 산문 — 홀드백이 줄마다 `PendingHeader` 를 낸다.
            "여기 a | b 가 있다\n\n또 c | d 가 있다\n\n마지막 문단\n",
            // 빈 줄만 잔뜩.
            "문단\n\n\n\n다른 문단\n\n\n",
            // 체크박스·강조·취소선.
            "- [x] 했음\n- [ ] 아직\n\n**굵게** *기울임* ~~취소~~ `코드`\n",
        ]
    }

    /// 한 줄씩 흘려 넣고 계수기를 읽는다.
    fn stream_and_measure(source: &str, width: usize) -> (usize, usize, usize) {
        crate::tui::markdown::probe::reset();
        let mut stream = MarkdownStream::answer(width);
        let start = Instant::now();
        let mut step = 0u64;
        for line in source.split_inclusive('\n') {
            stream.push(line);
            step += 1;
            stream.tick(start + Duration::from_millis(step));
        }
        stream.finish();
        crate::tui::markdown::probe::read()
    }

    /// 증분 렌더의 **근거**. 벤치가 없으므로 파서에 들어간 원문 바이트를 직접
    /// 센다: 누적 원문을 개행마다 통째로 다시 파싱하면 그 합은 원문 길이의
    /// 제곱에 비례해 자라고(줄이 두 배면 일감은 네 배), 꼬리만 다시 파싱하면
    /// **선형**으로 자란다.
    ///
    /// 이 테스트가 재는 것은 비율이다 — 원문을 두 배로 늘렸을 때 일감이 세 배를
    /// 넘지 않는 것. 증분화 전에는 3.7 배였다(보고의 측정표).
    #[test]
    fn the_reparsed_bytes_grow_with_the_source_not_its_square() {
        let small = corpus(6);
        let large = corpus(12);
        let (_, small_bytes, small_wrapped) = stream_and_measure(&small, 80);
        let (_, large_bytes, large_wrapped) = stream_and_measure(&large, 80);
        assert!(
            large_bytes * 10 < small_bytes * 30,
            "원문이 두 배인데 파싱 바이트가 세 배를 넘었다: {small_bytes} → {large_bytes}"
        );
        assert!(
            large_wrapped * 10 < small_wrapped * 30,
            "다시 접은 줄도 함께 선형이어야 한다: {small_wrapped} → {large_wrapped}"
        );
        // 꼬리만 다시 읽으므로 총 파싱량은 원문 길이의 몇 배 안쪽이다.
        assert!(
            large_bytes < large.len() * 6,
            "원문 {} 바이트에 파싱 {large_bytes} 바이트는 꼬리만 읽은 것이 아니다",
            large.len()
        );
    }

    /// An open language-tagged fence retains Syntect's parse state. Appending
    /// one complete line must parse that line, not every line before it again.
    #[test]
    fn an_open_fence_highlights_only_the_appended_lines() {
        let mut stream = MarkdownStream::answer(120);
        stream.push("```rust\n");
        crate::tui::highlight::probe::reset();

        let mut code = String::new();
        for index in 0..64 {
            let line = format!("let value_{index} = {index};\n");
            code.push_str(&line);
            stream.push(&line);
        }

        let (calls, bytes) = crate::tui::highlight::probe::read();
        assert!(
            calls < 64 * 3,
            "64 appended lines caused {calls} Syntect parses"
        );
        assert!(
            bytes < code.len() * 3,
            "{} bytes of code caused {bytes} highlighted bytes",
            code.len()
        );
        assert_eq!(stream.disagreement_with_a_full_render(), None);
    }

    #[test]
    fn a_fence_that_interrupts_a_paragraph_still_highlights_incrementally() {
        let mut stream = MarkdownStream::answer(120);
        stream.push("Before\n```rust\n");
        crate::tui::highlight::probe::reset();

        let mut code = String::new();
        for index in 0..32 {
            let line = format!("let value_{index} = {index};\n");
            code.push_str(&line);
            stream.push(&line);
        }

        let (calls, bytes) = crate::tui::highlight::probe::read();
        assert!(calls < 32 * 3, "32 appended lines caused {calls} Syntect parses");
        assert!(
            bytes < code.len() * 3,
            "{} bytes of code caused {bytes} highlighted bytes",
            code.len()
        );
        assert_eq!(stream.disagreement_with_a_full_render(), None);
    }

    /// 파이프 하나가 낀 산문도 선형이어야 한다.
    ///
    /// `is_table_header_line` 은 바깥 파이프 없는 `a | b` 도 헤더 후보로 받으므로
    /// (codex 그대로) 그런 줄마다 홀드백이 `PendingHeader` 를 낸다. 그러면
    /// `stable_prefix_len` 이 표 앞 구간의 행 수를 다시 세는데, 그 자리가 확정
    /// 경계와 겹치면 셈은 공짜다 — `stable_lines` 가 곧 그 값이다.
    #[test]
    fn prose_with_a_stray_pipe_stays_linear_too() {
        let mut source = String::new();
        for index in 0..40 {
            writeln!(source, "문단 {index} 에는 a | b 처럼 파이프가 하나 있다\n")
                .expect("문자열 쓰기는 실패하지 않는다");
        }
        let (_, bytes, _) = stream_and_measure(&source, 120);
        assert!(
            bytes < source.len() * 6,
            "원문 {} 바이트에 파싱 {bytes} 바이트 — 접두어 셈이 매번 처음부터 돈다",
            source.len()
        );
    }

    /// 증분 렌더의 **계약**: 어느 커밋에서 멈춰 봐도 누적 원문을 통째로 렌더한
    /// 것과 바이트까지 같다. 경계를 잘못 고르면(리스트가 이어지는 자리, 담장
    /// 안, 표 한가운데, 링크 참조 정의 뒤) 여기서 갈린다.
    #[test]
    fn the_incremental_render_matches_a_full_render_at_every_commit() {
        for source in equivalence_corpus() {
            for width in [16, 40, 120] {
                let mut stream = MarkdownStream::answer(width);
                for line in source.split_inclusive('\n') {
                    stream.push(line);
                    assert_eq!(
                        stream.disagreement_with_a_full_render(),
                        None,
                        "폭 {width}, 원문 {source:?} 의 {line:?} 뒤에서 갈렸다"
                    );
                }
                // 개행 없는 꼬리와 마무리도 같은 계약이다.
                stream.push("끝나지 않은 꼬리");
                stream.finish();
                assert_eq!(
                    stream.disagreement_with_a_full_render(),
                    None,
                    "폭 {width}, 원문 {source:?} 의 마무리에서 갈렸다"
                );
            }
        }
    }

    /// 폭이 바뀌어도 같다 — 경계는 원문 좌표라 살아남고, 확정 접두어만 새 폭으로
    /// 한 번 다시 세운다(codex `set_width` 의 자리).
    #[test]
    fn a_resize_mid_stream_still_matches_a_full_render() {
        for source in equivalence_corpus() {
            let mut stream = MarkdownStream::answer(40);
            let lines: Vec<&str> = source.split_inclusive('\n').collect();
            for (index, line) in lines.iter().enumerate() {
                stream.push(line);
                if index == lines.len() / 2 {
                    stream.set_width(72);
                }
                assert_eq!(
                    stream.disagreement_with_a_full_render(),
                    None,
                    "원문 {source:?} 의 {line:?} 뒤에서 갈렸다"
                );
            }
            stream.set_width(28);
            assert_eq!(stream.disagreement_with_a_full_render(), None, "{source:?}");
        }
    }

    /// 델타가 줄 단위로 안 올 때도 같다 — 실제 스트림은 토큰 단위라 한 번의
    /// `push` 에 개행이 여럿 들어오기도 하고 하나도 안 들어오기도 한다.
    #[test]
    fn ragged_deltas_render_the_same_as_line_deltas() {
        for source in equivalence_corpus() {
            for chunk in [1usize, 3, 7, 64] {
                let mut stream = MarkdownStream::answer(40);
                let bytes: Vec<char> = source.chars().collect();
                for piece in bytes.chunks(chunk) {
                    stream.push(&piece.iter().collect::<String>());
                    assert_eq!(
                        stream.disagreement_with_a_full_render(),
                        None,
                        "원문 {source:?} 을 {chunk} 글자씩 흘렸을 때 갈렸다"
                    );
                }
                stream.finish();
                assert_eq!(stream.disagreement_with_a_full_render(), None, "{source:?}");
            }
        }
    }

    /// 접두어 행 수를 세는 지름길도 전체 렌더와 같아야 한다 — 이 값이 곧 표
    /// 홀드백의 꼬리 예산이라, 하나만 어긋나도 큐와 꼬리의 경계가 밀린다
    /// (codex `tail_budget_from_source_start`).
    #[test]
    fn counting_a_prefix_matches_rendering_it() {
        for source in equivalence_corpus() {
            for width in [16, 40, 120] {
                let mut stream = MarkdownStream::answer(width);
                for line in source.split_inclusive('\n') {
                    stream.push(line);
                    for cut in 0..=stream.committed.len() {
                        if !stream.committed.is_char_boundary(cut) {
                            continue;
                        }
                        assert_eq!(
                            stream.prefix_len(cut),
                            stream.render(&stream.committed[..cut]).len(),
                            "폭 {width}, 원문 {source:?} 의 {cut} 바이트 앞에서 갈렸다"
                        );
                    }
                }
            }
        }
    }

    /// 표는 홀드백 시작부터 통째로 다시 렌더된다 — 경계가 그 앞에서 멈추기
    /// 때문이다. 행이 늘 때마다 지난 행의 열 폭이 다시 잡히는 것이 그 증거다.
    #[test]
    fn a_table_keeps_the_boundary_in_front_of_itself() {
        let mut stream = MarkdownStream::answer(40);
        stream.push("앞 문단\n\n");
        stream.push("| A | B |\n| --- | --- |\n");
        let narrow: Vec<String> = stream.tail().iter().map(Line::plain).collect();
        stream.push("| 아주긴셀값 | 2 |\n");
        let wide: Vec<String> = stream.tail().iter().map(Line::plain).collect();
        assert_ne!(narrow[2], wide[2], "행이 늘면 지난 구분선이 다시 잡힌다");
        assert_eq!(stream.disagreement_with_a_full_render(), None);
    }

    #[test]
    fn prefix_widths_reserve_the_marker_column() {
        assert_eq!(Prefix::bullet().width(), 2);
        assert_eq!(Prefix::tool_output().width(), 4);
    }

    /// A long list item wraps under its own text, not under its marker —
    /// the renderer names the indent it sits under and every continuation
    /// row starts with it, as codex wraps with the indent stack as
    /// `subsequent_indent` ("codex cli 스타일로 정갈하게", 2026-09-03).
    #[test]
    fn a_wrapped_list_item_continues_under_its_text() {
        let lines = markdown::render_wrapped("- alpha beta gamma delta epsilon", None);
        assert_eq!(
            lines[0].continuation.as_ref().map(|span| span.text.as_str()),
            Some("  "),
            "the renderer names the item's hanging indent"
        );
        let rows = prefixed(&lines, 14, &Prefix::bullet(), true);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain, ["• - alpha beta", "    gamma", "    delta", "    epsilon"]);
    }

    /// A quote keeps its bar on every row it wraps into.
    #[test]
    fn a_wrapped_quote_keeps_its_bar() {
        let lines = markdown::render_wrapped("> quoted words that wrap", None);
        let rows = prefixed(&lines, 14, &Prefix::bullet(), true);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain, ["• > quoted", "  > words that", "  > wrap"]);
        assert!(
            markdown::render_wrapped("plain words", None)[0].continuation.is_none(),
            "a paragraph has no hanging indent"
        );
    }

    /// The rows of a wrapped line remember the line — with the cell prefix
    /// on it — so a screen rebuilt at another width can wrap it again and
    /// get exactly the rows the cell would print there.
    #[test]
    fn wrapped_rows_remember_the_line_they_came_from() {
        let lines = markdown::render_wrapped("- alpha beta gamma delta epsilon", None);
        let rows = prefixed(&lines, 14, &Prefix::bullet(), true);
        let origin = rows[0].origin.as_deref().expect("a wrapped row names its line");
        assert!(rows.iter().all(|row| row.origin.as_deref() == Some(origin)));
        let hanging = origin.continuation.as_ref().expect("the line carries its hanging indent");
        assert_eq!(hanging.text, "    ", "the cell's indent, then the item's");
        let again: Vec<String> = wrap_line(origin, 14, hanging).iter().map(Line::plain).collect();
        let printed: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(again, printed, "the same width gives back the same rows");
        assert_eq!(wrap_line(origin, 40, hanging).len(), 1, "a wider screen gives one row");
        assert!(
            prefixed(&[Line::from_text("short")], 40, &Prefix::bullet(), true)[0]
                .origin
                .is_some(),
            "even a line that fits names itself, so a narrower rebuild can wrap it"
        );
    }

    /// A width change carries "how much went out" in LINES: a paragraph any
    /// row of which reached scrollback is out whole — the painter's ring
    /// rebuilds it at the new width — so the stream re-emits nothing of it,
    /// and everything after it still comes.
    #[test]
    fn a_width_change_re_emits_only_the_lines_that_never_went_out() {
        let mut stream = MarkdownStream::answer(20);
        stream.push("alpha beta gamma delta epsilon zeta eta theta\n\nsecond paragraph here\n\n");
        // The first paragraph is three rows at twenty columns; two of them go
        // out, then the pane widens before the third does.
        let first: Vec<String> = stream.tick(Instant::now()).iter().map(Line::plain).collect();
        let second: Vec<String> = stream.tick(Instant::now()).iter().map(Line::plain).collect();
        assert!(first.concat().contains("alpha"), "{first:?}");
        assert!(second.concat().contains("delta") || second.concat().contains("gamma"), "{second:?}");
        stream.set_width(80);
        let rest: Vec<String> = drain(&mut stream, 8);
        assert!(
            rest.iter().all(|row| !row.contains("alpha")),
            "the paragraph that had begun to go out is not printed again: {rest:?}"
        );
        assert!(
            rest.iter().any(|row| row.contains("second paragraph here")),
            "the paragraph after it still comes: {rest:?}"
        );
    }
}
