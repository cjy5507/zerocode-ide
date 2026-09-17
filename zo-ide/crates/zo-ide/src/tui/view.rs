//! 뷰포트 조립 — 화면 하단에 살아 있는 몇 줄.
//!
//! 캡처 실측 레이아웃(120x40, codex 0.149.1):
//!
//! ```text
//! 유휴  5줄 :  ""              /  ""  /  "› …"  /  ""  /  "  <model> … · <cwd>"
//! 작업중 7줄 이상: "" / "• Working (0s • esc to interrupt)" / detail 0~3줄
//!                 / "" / "" / "› …" / "" / "  <model> … · <cwd>"
//! ```
//!
//! 다이얼로그가 떠 있으면 뷰포트를 통째로 가져간다 — 캡처의 디렉터리 신뢰
//! 다이얼로그가 그렇고, 그때 커서는 숨는다.

use std::time::{Duration, Instant};

use super::ansi::{Color, Line, Span, Style};
use super::composer::Composer;
use super::effort_effect::{EffortEffect, EffortTier};
use super::palette;
use super::shimmer::{activity_marker, fmt_elapsed_compact, shimmer_spans};
use super::{activity::Activity, strings};
use super::wrap::wrap_line;

/// 컴포저 플레이스홀더 — 문안은 zo 답게, 자리와 스타일은 codex 그대로.
pub const PLACEHOLDER: &str = "Ask zo to do anything";
/// 자유 서술 질문이 떠 있을 때의 컴포저 플레이스홀더 — codex
/// `bottom_pane/request_user_input/mod.rs::ANSWER_PLACEHOLDER` 는
/// `"Type your answer (optional)"` 이다. 괄호를 뗀 이유는 문장이 거짓이 되기
/// 때문이다: codex 는 자동 해소(auto-resolution)가 빈 답을 대신 낼 수 있어
/// 정말 선택적이지만, zo 의 질문은 빈 줄을 답으로 받지 않는다
/// (`ide::prompt::parse_question_answer` 는 빈 줄에 `None`).
pub const ANSWER_PLACEHOLDER: &str = "Type your answer";
/// codex 가 `? for shortcuts` 를 두는 자리(푸터 줄)에 우리도 안내를 건다.
/// 컴포저가 비어 있을 때만 보인다 — 타이핑을 시작하면 물러난다.
pub const SHORTCUT_HINT: &str = "? for shortcuts";

/// Fewer than two cells only draw a bare ellipsis, not a recognizable cwd.
const MIN_FOOTER_CWD_WIDTH: usize = 2;

/// codex's status widget defaults to three detail rows.
pub const STATUS_DETAILS_DEFAULT_MAX_LINES: usize = 3;
const DETAILS_PREFIX: &str = "  └ ";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDetailsCapitalization {
    CapitalizeFirst,
    Preserve,
}

/// 진행 중 표시. `header` 는 shimmer 가 훑는 글자다.
#[derive(Debug, Clone)]
pub struct Status {
    pub header: String,
    pub elapsed: Duration,
    pub details: Vec<String>,
    pub details_max_lines: usize,
    inline_message: Option<String>,
    /// The runtime's latest housekeeping word (a context trim, the
    /// auto-compaction heads-up), shown dim in the activity's place until the
    /// next thing happens — content, a tool boundary, the turn's end. A
    /// stream phase (the request left, a retry) is not a happening: the
    /// notice is sent right before the request goes out, and the wait for
    /// the model is exactly the moment it can be read (t-3177).
    housekeeping: Option<String>,
    interrupt_binding: String,
    /// `false` 면 `(Ns)` 만 — 인터럽트가 불가능한 구간.
    pub interruptible: bool,
    /// Where the model request stands, kept as turn-elapsed offsets so the
    /// line can be rendered from `elapsed` alone: the elapsed at which the
    /// current request left (cleared by the first content block), and the
    /// retry being waited out (attempt, elapsed at which it fires).
    request_sent_at: Option<Duration>,
    retry: Option<(u32, Duration)>,
    /// The turn-elapsed at which the model went quiet mid-stream (set by a
    /// `QuietReasoning` phase, cleared by content), so the phrase keeps a
    /// clock of its own.
    quiet_reasoning_since: Option<Duration>,
    active_tools: Vec<ActiveTool>,
    last_tool: Option<Activity>,
    last_event_at: Duration,
    stream_phase_after: Duration,
    quiet_after: Duration,
}

#[derive(Debug, Clone)]
struct ActiveTool {
    id: String,
    activity: Activity,
    started_at: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusActivity {
    Tool {
        activity: Activity,
        elapsed: Duration,
    },
    Waiting {
        elapsed: Duration,
    },
    Reconnecting {
        attempt: u32,
        seconds_left: u64,
    },
    /// The stream is alive and the model is reasoning without visible output.
    ReasoningSilently {
        elapsed: Duration,
    },
    Quiet {
        elapsed: Duration,
        last: Option<Activity>,
    },
}

impl StatusActivity {
    #[must_use]
    pub fn verb(&self) -> String {
        match self {
            Self::Tool { activity, .. } => activity.wire_verb(),
            Self::Waiting { .. } => strings::ACTIVITY_WAITING.to_string(),
            Self::Reconnecting { .. } => strings::ACTIVITY_RECONNECTING.to_string(),
            Self::ReasoningSilently { .. } => strings::ACTIVITY_REASONING_SILENTLY.to_string(),
            Self::Quiet { .. } => strings::ACTIVITY_QUIET.to_string(),
        }
    }

    #[must_use]
    pub fn target(&self) -> Option<String> {
        match self {
            Self::Tool { activity, .. } => activity.target.clone(),
            Self::Waiting { .. } => Some(strings::ACTIVITY_MODEL_TARGET.to_string()),
            Self::Reconnecting { attempt, seconds_left } => {
                Some(strings::reconnecting_fact(*attempt, *seconds_left))
            }
            Self::ReasoningSilently { .. } => None,
            Self::Quiet { last, .. } => last.as_ref().map(strings::last_activity),
        }
    }

    #[must_use]
    pub fn elapsed(&self) -> Duration {
        match self {
            Self::Tool { elapsed, .. }
            | Self::Waiting { elapsed }
            | Self::ReasoningSilently { elapsed }
            | Self::Quiet { elapsed, .. } => *elapsed,
            Self::Reconnecting { seconds_left, .. } => Duration::from_secs(*seconds_left),
        }
    }

    #[must_use]
    pub fn line(&self) -> String {
        match self {
            Self::Tool { activity, elapsed } => strings::current_tool(
                &activity.tool,
                activity.target.as_deref(),
                *elapsed,
            ),
            Self::Waiting { elapsed } => strings::waiting_for_model(*elapsed),
            Self::ReasoningSilently { elapsed } => strings::reasoning_silently(*elapsed),
            Self::Reconnecting { attempt, seconds_left } => {
                strings::reconnecting(*attempt, *seconds_left)
            }
            Self::Quiet { elapsed, last } => strings::quiet(*elapsed, last.as_ref()),
        }
    }
}

impl Status {
    /// The word the shimmer sweeps while nothing more specific is known.
    pub const DEFAULT_HEADER: &'static str = strings::WORKING;

    #[must_use]
    pub fn working(elapsed: Duration) -> Self {
        let limits = crate::autonomy::limits::AutonomyLimits::load();
        Self {
            header: Self::DEFAULT_HEADER.to_string(),
            elapsed,
            details: Vec::new(),
            details_max_lines: STATUS_DETAILS_DEFAULT_MAX_LINES,
            inline_message: None,
            housekeeping: None,
            interrupt_binding: "esc".to_string(),
            interruptible: true,
            request_sent_at: None,
            quiet_reasoning_since: None,
            retry: None,
            active_tools: Vec::new(),
            last_tool: None,
            last_event_at: elapsed,
            stream_phase_after: Duration::from_secs(limits.stream_phase_after_secs),
            quiet_after: Duration::from_secs(limits.quiet_after_secs),
        }
    }

    /// A host event that should occupy the status row for one paint without
    /// claiming that a model turn is running or advertising an interrupt key.
    #[must_use]
    pub fn notice(header: impl Into<String>) -> Self {
        let mut status = Self::working(Duration::ZERO);
        status.header = header.into();
        status.interruptible = false;
        status
    }

    /// The streaming loop's word on the request: it left (start the wait) or
    /// it is being retried (say so, with the attempt and the pause).
    pub fn note_stream_phase(&mut self, phase: runtime::message_stream::types::StreamPhase) {
        use runtime::message_stream::types::StreamPhase;
        self.note_activity_event();
        match phase {
            StreamPhase::RequestSent { .. } => {
                self.request_sent_at = Some(self.elapsed);
                self.retry = None;
            }
            StreamPhase::Retrying { attempt, delay_secs } => {
                self.request_sent_at = None;
                self.retry = Some((attempt, self.elapsed + Duration::from_secs(delay_secs)));
            }
            StreamPhase::QuietReasoning { since_secs } => {
                self.quiet_reasoning_since =
                    Some(self.elapsed.saturating_sub(Duration::from_secs(since_secs)));
            }
        }
    }

    /// Content arrived (text, reasoning or a tool call): the wait is over.
    pub fn note_stream_content(&mut self) {
        self.request_sent_at = None;
        self.retry = None;
        self.quiet_reasoning_since = None;
        self.housekeeping = None;
        self.note_activity_event();
    }

    /// The runtime did some housekeeping: say so in the activity's place,
    /// dim, until the next content or tool boundary.
    pub fn note_housekeeping(&mut self, text: &str) {
        let text = text.trim();
        self.housekeeping = (!text.is_empty()).then(|| text.to_string());
    }

    /// Remember a tool boundary using turn-relative time so the row can be
    /// redrawn from `elapsed` without consulting a wall clock.
    pub fn note_tool_started(&mut self, id: &str, tool: &str, target: Option<&str>) {
        let activity = Activity::new(tool, target);
        self.last_tool = Some(activity.clone());
        self.housekeeping = None;
        self.note_activity_event();
        self.active_tools.retain(|active| active.id != id);
        self.active_tools.push(ActiveTool {
            id: id.to_string(),
            activity,
            started_at: self.elapsed,
        });
    }

    pub fn note_tool_finished(&mut self, id: &str) {
        self.active_tools.retain(|active| active.id != id);
        self.housekeeping = None;
        self.note_activity_event();
    }

    pub fn note_activity_event(&mut self) {
        self.last_event_at = self.elapsed;
    }

    /// Lower the quiet threshold so a test can reach `quiet` in seconds
    /// without depending on the developer machine's settings overlay.
    #[cfg(test)]
    pub(crate) fn set_quiet_after(&mut self, quiet_after: Duration) {
        self.quiet_after = quiet_after;
    }

    #[must_use]
    pub fn last_tool(&self) -> Option<&Activity> {
        self.last_tool.as_ref()
    }

    fn current_tool_activity(&self) -> Option<StatusActivity> {
        let active = self.active_tools.last()?;
        Some(StatusActivity::Tool {
            activity: active.activity.clone(),
            elapsed: self.elapsed.saturating_sub(active.started_at),
        })
    }

    fn quiet_activity(&self) -> Option<StatusActivity> {
        let elapsed = self.elapsed.saturating_sub(self.last_event_at);
        (elapsed >= self.quiet_after).then(|| StatusActivity::Quiet {
            elapsed,
            last: self.last_tool.clone(),
        })
    }

    /// The phrase for the request's standing, or `None` when there is nothing
    /// worth a word — the first token is expected soon, or content is flowing.
    fn phase_activity(&self) -> Option<StatusActivity> {
        if let Some((attempt, fires_at)) = self.retry {
            let left = fires_at.saturating_sub(self.elapsed).as_secs();
            return Some(StatusActivity::Reconnecting {
                attempt,
                seconds_left: left,
            });
        }
        if let Some(since) = self.quiet_reasoning_since {
            return Some(StatusActivity::ReasoningSilently {
                elapsed: self.elapsed.saturating_sub(since),
            });
        }
        let sent_at = self.request_sent_at?;
        let waited = self.elapsed.saturating_sub(sent_at);
        (waited >= self.stream_phase_after).then_some(StatusActivity::Waiting { elapsed: waited })
    }

    #[must_use]
    pub fn activity(&self) -> Option<StatusActivity> {
        self.quiet_activity()
            .or_else(|| self.current_tool_activity())
            .or_else(|| self.phase_activity())
    }

    pub fn set_details(
        &mut self,
        details: impl IntoIterator<Item = String>,
        capitalization: StatusDetailsCapitalization,
    ) {
        self.details = details
            .into_iter()
            .filter_map(|detail| {
                let detail = detail.trim_start();
                if detail.is_empty() {
                    return None;
                }
                Some(match capitalization {
                    StatusDetailsCapitalization::CapitalizeFirst => capitalize_first(detail),
                    StatusDetailsCapitalization::Preserve => detail.to_string(),
                })
            })
            .collect();
    }

    pub fn set_details_max_lines(&mut self, max_lines: usize) {
        self.details_max_lines = max_lines.max(1);
    }

    pub fn set_inline_message(&mut self, message: Option<String>) {
        self.inline_message = message
            .map(|message| message.trim().to_string())
            .filter(|message| !message.is_empty());
    }

    pub fn set_interrupt_binding(&mut self, binding: impl Into<String>) {
        let binding = binding.into();
        if !binding.trim().is_empty() {
            self.interrupt_binding = binding;
        }
    }

    /// Put the model's own heading in the shimmer, or fall back to `Working`.
    ///
    /// codex does exactly this (`chatwidget/streaming.rs::
    /// restore_reasoning_status_header`): while the model reasons, the animated
    /// word is the first bold heading of that reasoning, so a long unattended
    /// run says what it is thinking about instead of only that it is alive.
    pub fn set_header(&mut self, header: Option<&str>) {
        self.header = header
            .map(str::trim)
            .filter(|header| !header.is_empty())
            .map_or_else(|| Self::DEFAULT_HEADER.to_string(), str::to_string);
    }

    /// 상태 줄 한 줄 — `<•> <Working> (0s • esc to interrupt)`.
    #[must_use]
    pub fn line(&self, width: usize) -> Line {
        let mut spans = vec![activity_marker(self.elapsed), Span::raw(" ")];
        spans.extend(shimmer_spans(&self.header, self.elapsed));
        spans.push(Span::raw(" "));
        let elapsed = fmt_elapsed_compact(self.elapsed.as_secs());
        if self.interruptible {
            spans.push(Span::dim(format!("({elapsed} • ")));
            spans.push(Span::dim(self.interrupt_binding.clone()));
            spans.push(Span::dim(" to interrupt)"));
        } else {
            spans.push(Span::dim(format!("({elapsed})")));
        }
        // The housekeeping word takes the activity's place: it is the newer
        // fact, and the activity phrase it hides is the wait it fills.
        if let Some(word) = self
            .housekeeping
            .clone()
            .or_else(|| self.activity().map(|activity| activity.line()))
        {
            spans.push(Span::dim(" · "));
            spans.push(Span::dim(word));
        }
        if let Some(message) = &self.inline_message {
            spans.push(Span::dim(" · "));
            spans.push(Span::dim(message.clone()));
        }
        Line::new(spans).truncated(width)
    }

    /// Wrap all details before applying the row cap. If rows are hidden, the
    /// last visible content span ends in an ellipsis.
    #[must_use]
    pub fn detail_lines(&self, width: usize) -> Vec<Line> {
        if self.details.is_empty() || width == 0 {
            return Vec::new();
        }
        let max_lines = self.details_max_lines.max(1);
        let continuation = Span::dim(" ".repeat(DETAILS_PREFIX.chars().count()));
        let mut rows = Vec::new();
        for detail in &self.details {
            let line = Line::new(vec![Span::dim(DETAILS_PREFIX), Span::dim(detail.clone())]);
            rows.extend(wrap_line(&line, width, &continuation));
        }
        if rows.len() > max_lines {
            rows.truncate(max_lines);
            let content_width = width
                .saturating_sub(DETAILS_PREFIX.chars().count())
                .max(1);
            let max_base_len = content_width.saturating_sub(1);
            if let Some(span) = rows.last_mut().and_then(|line| line.spans.last_mut()) {
                let trimmed: String = span.text.chars().take(max_base_len).collect();
                *span = Span::dim(format!("{trimmed}…"));
            }
        }
        rows
    }
}

fn capitalize_first(text: &str) -> String {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    first.to_uppercase().chain(chars).collect()
}

/// 번호 선택 다이얼로그 — 권한·질문·신뢰 게이트가 모두 이 문법을 쓴다.
#[derive(Debug, Clone)]
pub struct Dialog {
    /// 첫 줄의 볼드 라벨과 그 뒤 값(캡처: `You are in <path>`).
    pub title: String,
    pub subject: String,
    pub body: String,
    pub options: Vec<String>,
    pub selected: usize,
    pub footer: String,
}

impl Dialog {
    #[must_use]
    pub fn lines(&self, width: usize) -> Vec<Line> {
        self.lines_with_focus(width, palette::COMMAND_TOKEN)
    }

    fn lines_with_focus(&self, width: usize, focus: Color) -> Vec<Line> {
        let mut out = Vec::new();
        let mut head = vec![Span::dim("> ")];
        if !self.title.is_empty() {
            head.push(Span::bold(format!("{} ", self.title)));
        }
        head.push(Span::raw(self.subject.clone()));
        out.push(Line::new(head).truncated(width));
        if !self.body.is_empty() {
            out.push(Line::empty());
            for paragraph in self.body.lines() {
                out.extend(wrap_line(
                    &Line::new(vec![Span::raw(paragraph.to_string())]).prefixed(Span::raw("  ")),
                    width,
                    &Span::raw("  "),
                ));
            }
        }
        out.push(Line::empty());
        for (index, option) in self.options.iter().enumerate() {
            let selected = index == self.selected;
            let label = format!("{}. {option}", index + 1);
            let line = if selected {
                let style = Style::new().bold().fg(focus);
                Line::new(vec![
                    Span::new("› ", style),
                    Span::new(label, style),
                ])
            } else {
                Line::new(vec![Span::raw("  "), Span::raw(label)])
            };
            out.push(line.truncated(width));
        }
        if !self.footer.is_empty() {
            out.push(Line::empty());
            out.push(Line::new(vec![Span::dim(self.footer.clone())]).truncated(width));
        }
        out
    }
}

/// 컴포저 자동완성 팝업 한 줄 — 이름과 한 줄 설명.
#[derive(Debug, Clone)]
pub struct PopupRow {
    pub name: String,
    pub description: String,
}

/// `/` 로 시작한 입력에 뜨는 자동완성 팝업. 캡처(model-picker.bin, 8.24s)는
/// 이것을 **푸터 자리**에 놓는다 — 뷰포트 높이는 그대로고 모델·cwd 줄이
/// 물러난다.
#[derive(Debug, Clone)]
pub struct Popup {
    pub rows: Vec<PopupRow>,
    pub selected: usize,
}

impl Popup {
    /// 선택된 항목의 이름.
    #[must_use]
    pub fn selection(&self) -> Option<&str> {
        self.rows
            .get(self.selected)
            .map(|row| row.name.as_str())
    }

    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self) {
        self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
    }

    #[must_use]
    pub fn lines(&self, width: usize) -> Vec<Line> {
        self.lines_with_focus(width, palette::COMMAND_TOKEN)
    }

    fn lines_with_focus(&self, width: usize, focus: Color) -> Vec<Line> {
        let column = self
            .rows
            .iter()
            .map(|row| row.name.chars().count())
            .max()
            .unwrap_or(0);
        self.rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                two_column(
                    "  ",
                    &row.name,
                    column,
                    &row.description,
                    index == self.selected,
                    false,
                    focus,
                )
                .truncated(width)
            })
            .collect()
    }
}

/// 번호 피커 한 줄 — 라벨(`1. <id> (current)`)과 오른쪽 한 줄 설명.
#[derive(Debug, Clone, Default)]
pub struct PickerRow {
    pub label: String,
    pub description: String,
    /// The row is offered but faded — a model its source stopped listing
    /// (the description says since when). Still selectable: the wire decides.
    pub dim: bool,
}

/// 뷰포트를 통째로 가져가는 번호 피커 — 캡처(model-picker.bin, 11.03s)의
/// `Select Model and Effort` 화면 그대로다:
///
/// ```text
/// ""
/// ""
/// "  Select Model and Effort"                       bold
/// "  Access legacy models by running …"             dim
/// ""
/// "  1. gpt-5.6-sol (default)   Latest frontier …"  설명 dim
/// "› 3. gpt-5.6-luna (current)  Fast and afford…"   고른 줄 전체 bold+시안
/// ""
/// "  Press enter to confirm or esc to go back"      dim
/// ```
#[derive(Debug, Clone)]
pub struct Picker {
    pub title: String,
    /// 제목 줄의 스타일. codex 의 두 피커가 서로 다르기 때문에 필드다 —
    /// `Select Model and Effort` 는 bold 뿐이고(`ESC[1m`, model-picker 캡처
    /// 11.03s), `Resume a previous session` 은 bold + 시안이다
    /// (`ESC[1mESC[38;5;6;49m`, resume-picker 캡처 29.19s의 1행).
    pub title_style: Style,
    pub note: String,
    pub rows: Vec<PickerRow>,
    pub selected: usize,
    pub footer: String,
}

/// 피커가 옵션 줄 말고 쓰는 행 수 — 빈 줄 둘·제목·안내·빈 줄·빈 줄·푸터.
///
/// The option window itself is as tall as the screen allows. It was a
/// three-row window once, because every row a popup scrolled away for its
/// room stayed blank after it closed; the painter now draws a popup OVER the
/// rows above the head and prints them back when it closes
/// (`Painter::set_popup_height`), so a tall picker costs nothing afterwards.
const PICKER_CHROME: usize = 7;

impl Picker {
    pub fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn down(&mut self) {
        self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
    }

    /// 숫자 키 — 1 기준. 범위 밖이면 아무것도 안 고른다.
    pub fn select_number(&mut self, digit: usize) -> bool {
        if digit >= 1 && digit <= self.rows.len() {
            self.selected = digit - 1;
            return true;
        }
        false
    }

    /// `max_rows` 는 뷰포트가 쓸 수 있는 최대 높이다. 옵션이 그보다 많으면
    /// 고른 줄이 보이는 창만 그린다 — 잘라 버리면 아래쪽 모델은 영영 못 고른다.
    #[must_use]
    pub fn lines(&self, width: usize, max_rows: usize) -> Vec<Line> {
        self.lines_with_focus(width, max_rows, palette::COMMAND_TOKEN)
    }

    fn lines_with_focus(&self, width: usize, max_rows: usize, focus: Color) -> Vec<Line> {
        let mut out = vec![Line::empty(), Line::empty()];
        out.push(
            Line::new(vec![
                Span::raw("  "),
                Span::new(self.title.clone(), self.title_style),
            ])
            .truncated(width),
        );
        if !self.note.is_empty() {
            out.push(
                Line::new(vec![Span::raw("  "), Span::dim(self.note.clone())]).truncated(width),
            );
        }
        out.push(Line::empty());
        let budget = max_rows.saturating_sub(PICKER_CHROME).max(1);
        out.extend(selection_rows(&self.rows, self.selected, width, budget, focus));
        out.push(Line::empty());
        out.push(Line::new(vec![Span::raw("  "), Span::dim(self.footer.clone())]).truncated(width));
        out
    }
}

/// 번호 선택 행들 — codex `bottom_pane/selection_popup_common.rs::render_rows`
/// 자리다. 그 함수 하나를 codex 의 목록 선택 뷰
/// (`list_selection_view` → 우리 [`Picker`])와 질문 오버레이
/// (`request_user_input` → 우리 [`Question`])가 **함께** 쓴다. 우리도 그렇게
/// 한 벌만 그린다.
///
/// `budget` 은 이 목록이 쓸 수 있는 행 수다. 옵션이 그보다 많으면 고른 줄이
/// 보이는 창만 그린다.
fn selection_rows(
    rows: &[PickerRow],
    selected: usize,
    width: usize,
    budget: usize,
    focus: Color,
) -> Vec<Line> {
    let column = rows
        .iter()
        .map(|row| row.label.chars().count())
        .max()
        .unwrap_or(0);
    // 번호가 먹는 칸(`1. ` / `10. `)까지 세야 설명 열이 캡처처럼 선다.
    let number = rows.len().to_string().chars().count() + 2;
    let continuation_indent = Span::raw(" ".repeat(column + 4));
    let render = |index: usize| {
        let row = &rows[index];
        let label = format!("{}. {}", index + 1, row.label);
        let marker = if index == selected { "› " } else { "  " };
        let line = two_column(
            marker,
            &label,
            column + number,
            &row.description,
            index == selected,
            row.dim,
            focus,
        );
        wrap_line(&line, width, &continuation_indent)
    };
    let mut visible = budget.max(1).min(rows.len());
    let mut start = selected
        .saturating_sub(visible.saturating_sub(1))
        .min(rows.len().saturating_sub(visible));
    // The budget is LINES on screen, and a row whose description wraps is
    // more than one. A window that overflows gives up rows from whichever
    // end does not hold the selection, until it fits — the selected row
    // alone may still exceed the budget, and then it is shown whole.
    let mut lines: Vec<Vec<Line>> = (start..start + visible).map(render).collect();
    while visible > 1 && lines.iter().map(Vec::len).sum::<usize>() > budget.max(1) {
        if start + visible - 1 > selected {
            lines.pop();
        } else {
            lines.remove(0);
            start += 1;
        }
        visible -= 1;
    }
    lines.into_iter().flatten().collect()
}

/// 질문 오버레이 푸터의 힌트 하나 — codex
/// `bottom_pane/request_user_input/mod.rs::FooterTip`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    pub text: String,
    /// 강조된 힌트. codex 는 이것만 `cyan().bold().not_dim()` 으로 내고
    /// 나머지 줄은 통째로 dim 이다(`request_user_input/render.rs` 의 푸터 루프).
    pub highlight: bool,
}

impl Tip {
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            highlight: false,
        }
    }

    /// codex `FooterTip::highlighted` — 지금 눌러야 할 키.
    #[must_use]
    pub fn highlighted(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            highlight: true,
        }
    }
}

/// 힌트 사이의 구분자 — codex `request_user_input::TIP_SEPARATOR`.
pub const TIP_SEPARATOR: &str = " | ";

/// 힌트들을 폭에 맞춰 접은 푸터 줄들. 접는 규칙은 codex
/// `request_user_input::wrap_footer_tips` 그대로다 — 힌트 하나는 쪼개지 않고,
/// 넘치면 통째로 다음 줄로 내린다(실측: 스냅샷
/// `request_user_input_footer_wrap` 이 `enter to submit answer` 뒤에서 끊는다).
#[must_use]
pub fn tip_lines(tips: &[Tip], width: usize) -> Vec<Line> {
    tip_lines_with_focus(tips, width, palette::COMMAND_TOKEN)
}

fn tip_lines_with_focus(tips: &[Tip], width: usize, focus: Color) -> Vec<Line> {
    let max = width.saturating_sub(2).max(1);
    let separator = TIP_SEPARATOR.chars().count();
    let mut out = Vec::new();
    let mut current: Vec<&Tip> = Vec::new();
    let mut used = 0usize;
    for tip in tips {
        let tip_width = tip.text.chars().count().min(max);
        let extra = if current.is_empty() {
            tip_width
        } else {
            separator + tip_width
        };
        if !current.is_empty() && used + extra > max {
            out.push(tip_row(&current, width, focus));
            current.clear();
            used = 0;
        }
        used = if current.is_empty() {
            tip_width
        } else {
            used + separator + tip_width
        };
        current.push(tip);
    }
    out.push(tip_row(&current, width, focus));
    out
}

fn tip_row(tips: &[&Tip], width: usize, focus: Color) -> Line {
    let mut spans = vec![Span::raw("  ")];
    for (index, tip) in tips.iter().enumerate() {
        if index > 0 {
            spans.push(Span::dim(TIP_SEPARATOR));
        }
        if tip.highlight {
            spans.push(Span::new(
                tip.text.clone(),
                Style::new().bold().fg(focus),
            ));
        } else {
            spans.push(Span::dim(tip.text.clone()));
        }
    }
    Line::new(spans).truncated(width)
}

/// `AskUserQuestion` 오버레이 — codex `bottom_pane/request_user_input`.
///
/// 정본 스냅샷(`request_user_input/snapshots/…__options.snap`, 120칸):
///
/// ```text
/// ""
/// "  Question 1/1 (1 unanswered)"                   dim
/// "  Choose an option."                             시안
/// ""
/// "  › 1. Option 1  First choice."                  고른 줄 전체 bold+시안
/// "    2. Option 2  Second choice."
/// ""
/// "  tab to add notes | enter to submit answer | esc to interrupt"
/// ```
///
/// 두 자리만 zo 것이다. (1) 앞 빈 줄이 **둘**이다 — codex 의 이 오버레이는
/// 메뉴 표면(`render_menu_surface`)의 위 패딩 한 줄 위에 앉지만 zo 에는 그
/// 표면이 없고, 우리가 이미 이식한 세 피커([`Picker`], codex
/// `list_selection_view`)의 캡처 실측이 빈 줄 둘이다. 같은 자리에 뜨는 화면이
/// 여백만 다르면 그게 결함이다. (2) 같은 이유로 행의 `›` 는 0열에서 시작한다
/// — codex 도 두 위젯의 행 문법은 같고, 표면 인셋 두 칸만큼 밀려 있을 뿐이다.
#[derive(Debug, Clone)]
pub struct Question {
    /// dim 진행 줄 — `Question 1/1 (1 unanswered)`.
    pub progress: String,
    /// 질문 본문. 아직 답이 없는 동안이라 codex 는 이 줄을 시안으로 낸다
    /// (`render_ui_at`: `if answered { plain } else { cyan }`).
    pub question: String,
    /// 보기. 비면 자유 서술이고 컴포저가 답을 받는다.
    pub rows: Vec<PickerRow>,
    pub selected: usize,
    /// 다중 선택에서 켜진 보기. 단일 선택이면 **비어 있다**.
    pub checked: Vec<bool>,
    /// 고정 보기 바로 뒤에 붙는 자유 입력 행. 옵션 질문에만 있다.
    pub other_index: Option<usize>,
    /// 자유 입력 행을 확정해 인라인 컴포저가 입력을 받고 있는가.
    pub other_input: bool,
    pub tips: Vec<Tip>,
}

impl Question {
    /// 보기가 없는 질문 — 답은 컴포저가 받는다.
    #[must_use]
    pub fn is_freeform(&self) -> bool {
        self.rows.is_empty()
    }

    /// 여러 개를 고를 수 있는 질문.
    #[must_use]
    pub fn is_multi(&self) -> bool {
        !self.checked.is_empty()
    }

    /// 마지막 자유 입력 행에 커서가 있는가.
    #[must_use]
    pub fn is_other_selected(&self) -> bool {
        self.other_index == Some(self.selected)
    }

    /// 옵션 목록 아래의 자유 입력 컴포저가 활성 상태인가.
    #[must_use]
    pub fn is_other_input(&self) -> bool {
        self.other_input
    }

    /// 자유 입력 행으로 이동해 인라인 컴포저를 연다.
    pub fn begin_other_input(&mut self) -> bool {
        let Some(index) = self.other_index else {
            return false;
        };
        self.selected = index;
        self.other_input = true;
        true
    }

    /// 인라인 입력을 닫고 옵션 목록으로 돌아간다.
    pub fn cancel_other_input(&mut self) {
        self.other_input = false;
    }

    /// codex `ScrollState::move_up_wrap` — 목록 위쪽에서 아래로 돈다.
    pub fn up(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.rows.len() - 1
        } else {
            self.selected - 1
        };
    }

    /// codex `ScrollState::move_down_wrap`.
    pub fn down(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.rows.len();
    }

    /// 숫자 키 — 1 기준. codex `option_index_for_digit` 과 같이 범위 밖은 무시.
    pub fn select_number(&mut self, digit: usize) -> bool {
        if digit >= 1 && digit <= self.rows.len() {
            self.selected = digit - 1;
            return true;
        }
        false
    }

    /// 지금 줄을 켜고 끈다(다중 선택 전용).
    pub fn toggle(&mut self) {
        if let Some(slot) = self.checked.get_mut(self.selected) {
            *slot = !*slot;
        }
    }

    /// 지금 고른 답. 다중 선택이면 켜진 보기들이고, 하나도 안 켰으면 커서가
    /// 선 줄 하나다 — responder 계약이 빈 답을 모르기 때문이다
    /// (`ide::prompt::parse_question_answer` 도 빈 답을 내지 않는다).
    #[must_use]
    pub fn answers(&self) -> Vec<String> {
        self.answers_with_custom("")
    }

    /// 고정 보기 답에 인라인 자유 입력을 합친다. 단일 선택의 Other는 커스텀
    /// 텍스트 하나로, 다중 선택은 체크된 보기 뒤에 커스텀 항목 하나로 답한다.
    #[must_use]
    pub fn answers_with_custom(&self, custom: &str) -> Vec<String> {
        let custom = custom.trim();
        if self.is_multi() {
            let mut picked: Vec<String> = self
                .rows
                .iter()
                .zip(self.checked.iter())
                .filter(|(_, checked)| **checked)
                .map(|(row, _)| row.label.clone())
                .collect();
            if !custom.is_empty() {
                picked.push(custom.to_string());
            }
            if !picked.is_empty() {
                return picked;
            }
        }
        if self.is_other_selected() {
            return if custom.is_empty() {
                Vec::new()
            } else {
                vec![custom.to_string()]
            };
        }
        self.rows
            .get(self.selected)
            .map(|row| vec![row.label.clone()])
            .unwrap_or_default()
    }

    /// 화면에 설 행 — 다중 선택이면 라벨 앞에 `[x]`/`[ ]` 가 붙는다. codex
    /// 도 그 표시를 **행을 만드는 쪽**에서 붙인다
    /// (`multi_select_picker.rs::build_rows`: `format!("{prefix} [{marker}] …")`).
    /// 번호는 `request_user_input` 의 행 문법에서 온다 — 둘의 최소 합집합이다.
    fn display_rows(&self) -> Vec<PickerRow> {
        if !self.is_multi() {
            return self.rows.clone();
        }
        self.rows
            .iter()
            .enumerate()
            .map(|(index, row)| PickerRow {
                label: self.checked.get(index).map_or_else(
                    || row.label.clone(),
                    |checked| format!("[{}] {}", if *checked { "x" } else { " " }, row.label),
                ),
                description: row.description.clone(),
                dim: row.dim,
            })
            .collect()
    }

    /// 오버레이의 머리 — 빈 줄 둘, dim 진행 줄, 시안 질문(접힘).
    #[must_use]
    pub fn head(&self, width: usize) -> Vec<Line> {
        self.head_with_focus(width, palette::COMMAND_TOKEN)
    }

    fn head_with_focus(&self, width: usize, focus: Color) -> Vec<Line> {
        let mut out = vec![Line::empty(), Line::empty()];
        out.push(
            Line::new(vec![Span::raw("  "), Span::dim(self.progress.clone())]).truncated(width),
        );
        let question = Line::new(vec![
            Span::raw("  "),
            Span::new(self.question.clone(), Style::new().fg(focus)),
        ]);
        out.extend(wrap_line(&question, width, &Span::raw("  ")));
        out
    }

    /// 보기가 있는 질문의 화면 전체.
    #[must_use]
    pub fn lines(&self, width: usize, max_rows: usize) -> Vec<Line> {
        self.lines_with_focus(width, max_rows, palette::COMMAND_TOKEN)
    }

    fn lines_with_focus(&self, width: usize, max_rows: usize, focus: Color) -> Vec<Line> {
        let footer = self.footer_lines_with_focus(width, focus);
        let mut out = self.option_body_with_focus(width, max_rows, footer.len(), focus);
        out.extend(footer);
        out
    }

    fn footer_lines_with_focus(&self, width: usize, focus: Color) -> Vec<Line> {
        if self.other_input {
            return tip_lines_with_focus(
                &[
                    Tip::new("esc to return to options"),
                    Tip::highlighted("enter to submit answer"),
                ],
                width,
                focus,
            );
        }
        tip_lines_with_focus(&self.tips, width, focus)
    }

    fn option_body_with_focus(
        &self,
        width: usize,
        max_rows: usize,
        reserved_tail: usize,
        focus: Color,
    ) -> Vec<Line> {
        let mut out = self.head_with_focus(width, focus);
        // 머리(빈 줄 둘·진행·접힌 질문)와 목록 앞뒤 빈 줄 둘, 그리고 접힌 푸터.
        // 나머지가 목록의 몫이다 — 넘치면 고른 줄이 보이는 창만 그린다.
        let chrome = out.len() + 2 + reserved_tail;
        let budget = max_rows.saturating_sub(chrome).max(1);
        out.push(Line::empty());
        out.extend(selection_rows(
            &self.display_rows(),
            self.selected,
            width,
            budget,
            focus,
        ));
        out.push(Line::empty());
        out
    }
}

/// 팝업·피커가 함께 쓰는 두 칸 줄 — `<marker><name>` 을 `column` 칸으로 채우고
/// 두 칸 띄운 뒤 설명. 고른 줄은 줄 전체가 bold + 시안이다(캡처 그대로).
fn two_column(
    marker: &str,
    name: &str,
    column: usize,
    description: &str,
    selected: bool,
    dim: bool,
    focus: Color,
) -> Line {
    let pad = " ".repeat(column.saturating_sub(name.chars().count()) + 2);
    if selected {
        // A faded row under the cursor keeps the focus colour but not the
        // weight: it is the row being chosen, and still not a listed one.
        let style = if dim {
            Style::new().dim().fg(focus)
        } else {
            Style::new().bold().fg(focus)
        };
        return Line::new(vec![
            Span::new(marker.to_string(), style),
            Span::new(name.to_string(), style),
            Span::new(pad, style),
            Span::new(description.to_string(), style),
        ]);
    }
    if dim {
        return Line::new(vec![
            Span::dim(marker.to_string()),
            Span::dim(name.to_string()),
            Span::dim(pad),
            Span::dim(description.to_string()),
        ]);
    }
    Line::new(vec![
        Span::raw(marker.to_string()),
        Span::raw(name.to_string()),
        Span::raw(pad),
        Span::dim(description.to_string()),
    ])
}

/// 푸터 — `  <model> <effort> · <cwd>` 왼쪽과 컨텍스트·힌트 오른쪽.
///
/// cwd 는 **남는 자리만큼만** 쓴다. 힌트를 통째 뒤에 붙이고 줄 전체를 자르면
/// 긴 경로가 힌트를 잡아먹어 안내가 영영 안 보인다.
#[must_use]
#[allow(clippy::too_many_arguments)] // This public byte-test seam mirrors the footer's visible facts.
pub fn footer(
    model: &str,
    effort: &str,
    model_note: Option<&str>,
    cwd: &str,
    width: usize,
    hint: Option<&str>,
    context_left: Option<u8>,
    context_used_tokens: Option<u64>,
    plan_mode: bool,
    goal_status: Option<crate::goal::GoalFooterStatus>,
    loop_status: Option<&str>,
) -> Line {
    let head = if effort.is_empty() {
        model.to_string()
    } else {
        format!("{model} {effort}")
    };
    let mut spans = vec![
        Span::raw("  "),
        Span::new(head, footer_theme_style(
            &["entity.name.type", "support.type", "variable"],
            palette::FOOTER_MODEL,
        )),
    ];
    // A swap on the wire names itself right after the model it put there —
    // one warn-tinted word, so a reply another model wrote is never read as
    // the session model's.
    if let Some(note) = model_note {
        spans.push(Span::dim(" · "));
        spans.push(Span::new(note.to_string(), Style::new().fg(palette::NOTICE_WARN)));
    }
    spans.push(Span::dim(" · "));
    let context = crate::status_format::context_footer_text(context_left, context_used_tokens);
    // Match Codex's right-indicator rule: plan wins over goal, then the IDE
    // context value joins it.  The hint is zo-only chrome and has the lowest
    // priority; at narrow widths the order is hint, context, then path.
    let indicator = if plan_mode {
        Some(crate::status_format::PLAN_MODE_FOOTER_TEXT)
    } else {
        goal_status
            .map(crate::status_format::goal_footer_text)
            .or(loop_status)
    };
    let make_tail = |include_context: bool, include_hint: bool| {
        let mut tail = Vec::new();
        if let Some(indicator) = indicator {
            tail.push(Span::new(indicator, Style::new().fg(Color::MAGENTA)));
            if include_context {
                if let Some(context) = context.as_deref() {
                    tail.push(Span::dim(" · "));
                    tail.push(Span::dim(context.to_string()));
                }
            }
        } else if include_context {
            if let Some(context) = context.as_deref() {
                tail.push(Span::dim(context.to_string()));
            }
        }
        if include_hint {
            if let Some(hint) = hint {
                if tail.is_empty() {
                    tail.push(Span::dim(format!("   {hint}")));
                } else {
                    tail.push(Span::dim(" | "));
                    tail.push(Span::dim(hint.to_string()));
                }
            }
        }
        tail
    };
    let context_present = context.is_some();
    let mut tail = make_tail(context_present, hint.is_some());
    let left_width: usize = spans.iter().map(Span::width).sum();
    let tail_width: usize = tail.iter().map(Span::width).sum();
    // Keep this one-way priority independent of the exact width: hint first,
    // then context, then cwd. Reconsidering a previously kept tail at a
    // second threshold is what made the order flip around 40 columns.
    if hint.is_some() && width < left_width + tail_width + MIN_FOOTER_CWD_WIDTH {
        tail = make_tail(context_present, false);
    }
    let tail_width: usize = tail.iter().map(Span::width).sum();
    if indicator.is_some()
        && context_present
        && width < left_width + tail_width + MIN_FOOTER_CWD_WIDTH
    {
        tail = make_tail(false, false);
    }
    let tail_width: usize = tail.iter().map(Span::width).sum();
    // A visible cwd needs one reserved separator before a right indicator.
    // Without it a path that exactly consumes its budget produces
    // `/workPlan mode`, and truncation can no longer recover that boundary.
    let available = width.saturating_sub(left_width + tail_width);
    // A hint without context already begins with its captured three-space
    // gutter. Plan mode and bare context do not, so only those tails reserve
    // a separate boundary here.
    let path_separator = usize::from(
        !tail.is_empty() && available > 0 && (indicator.is_some() || context_present),
    );
    let budget = available.saturating_sub(path_separator);
    let path = Line::new(vec![Span::new(
        cwd.to_string(),
        footer_theme_style(
            &["string", "markup.underline.link"],
            palette::FOOTER_CWD,
        ),
    )])
    .truncated(budget);
    let path_width = path.width();
    spans.extend(path.spans);
    if path_width > 0 && path_separator > 0 {
        spans.push(Span::raw(" "));
    }
    if context_present {
        let gap = width.saturating_sub(left_width + path_width + path_separator + tail_width);
        if gap > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
        }
    }
    spans.extend(tail);
    Line::new(spans).truncated(width)
}

/// Last completed Dreamer pass, kept as one secondary-chrome row above the
/// ordinary turn footer. A narrow pane truncates the detail, never the layout.
fn dreamer_footer(dream: &str, width: usize) -> Line {
    Line::new(vec![Span::dim(format!("  {dream}"))]).truncated(width)
}

fn footer_theme_style(scopes: &[&str], fallback: Color) -> Style {
    super::highlight::foreground_style_for_scopes(scopes)
        .map_or_else(|| Style::new().fg(fallback), palette::soften_status_line_style)
}

/// Join the ordinary cwd with `ZeroCode`'s measured worktree identity.
///
/// The footer renders this as its existing location span, so both halves reuse
/// the same theme-derived, status-line-softened colour instead of creating a
/// fourth colour axis. Worktree identity comes first so a long cwd yields
/// before the new context does. `None` is byte-identical to the Codex footer.
#[must_use]
pub fn footer_location(cwd: &str, worktree_context: Option<&str>) -> String {
    let worktree_context = worktree_context.map(str::trim).filter(|value| !value.is_empty());
    match (cwd.is_empty(), worktree_context) {
        (_, None) => cwd.to_string(),
        (true, Some(context)) => context.to_string(),
        (false, Some(context)) => format!("{context} · {cwd}"),
    }
}

/// 뷰포트 한 프레임의 재료.
pub struct Frame<'a> {
    pub composer: &'a Composer,
    pub effort_tier: Option<EffortTier>,
    pub effort_effect: Option<&'a EffortEffect>,
    pub now: Instant,
    pub status: Option<&'a Status>,
    /// Pending steers and Tab-queued follow-ups. Codex places this between
    /// the status row and the composer; it is live bottom-pane state, never a
    /// history cell.
    pub pending_input: Option<&'a [Line]>,
    /// **활성 셀 칸** — 아직 커밋되지 않은 셀 하나가 사는 자리다. 지금 도는
    /// 도구 셀([`super::tools::ToolGroup`]) 이거나 스트림의 미확정 꼬리
    /// ([`super::cells::MarkdownStream::tail`]) 이고, 둘 다 매 프레임 다시
    /// 그린다 — 뷰포트이므로 append-only 히스토리 계약을 건드리지 않는다.
    ///
    /// 자리는 codex `chatwidget/rendering.rs::as_renderable` 그대로다: 활성
    /// 셀이 먼저, 그 아래 `top: 1` 을 띄운 bottom pane(상태·컴포저·푸터).
    pub active: Option<&'a [Line]>,
    /// 활성 셀이 **스트림 꼬리**인가. 참이면 Working 줄을 감춘다 — codex
    /// `chatwidget/streaming.rs::sync_active_stream_tail` 이 꼬리를 세울 때마다
    /// `bottom_pane.hide_status_indicator()` 를 부른다("Hide the status
    /// indicator while leaving task-running state untouched" — esc 는 그대로
    /// 턴을 끊는다).
    pub tail_owns_slot: bool,
    pub dialog: Option<&'a Dialog>,
    /// 떠 있는 `AskUserQuestion` 오버레이. 보기가 있으면 뷰포트를 통째로 쓰고,
    /// 자유 서술 또는 활성 Other 행이면 컴포저가 답을 받는다.
    pub question: Option<&'a Question>,
    /// 뷰포트를 통째로 가져가는 번호 피커(`/model`).
    pub picker: Option<&'a Picker>,
    /// 뷰포트를 통째로 가져가는 `/resume` 화면. 번호 피커와 달리 자기 크롬
    /// (검색 줄·툴바·진행 구분선·두 줄 힌트)을 들고 있어서 [`Picker`] 가
    /// 아니다 — codex 도 `resume_picker.rs` 를 `list_selection_view` 와 따로
    /// 둔다.
    pub sessions: Option<&'a crate::tui::sessions::SessionPicker>,
    /// 뷰포트를 통째로 가져가는 페이저(Ctrl+T 트랜스크립트). 피커와 달리
    /// 고르는 목록이 아니라 이미 접힌 줄들이라 `Line` 을 그대로 받는다 —
    /// 여기서 다시 자르면 도구 본문을 펴 보러 연 화면이 또 접힌다.
    pub pager: Option<&'a [Line]>,
    /// 컴포저 아래(푸터 자리)의 슬래시 자동완성 팝업.
    pub popup: Option<&'a Popup>,
    /// 컴포저에 `?` 만 있을 때 뜨는 단축키 카드.
    pub shortcuts: Option<&'a [String]>,
    pub model: &'a str,
    pub effort: &'a str,
    /// The footer's word for a model swap on the wire, while one stands.
    pub model_note: Option<&'a str>,
    pub cwd: &'a str,
    pub context_left: Option<u8>,
    pub context_used_tokens: Option<u64>,
    pub plan_mode: bool,
    pub goal_status: Option<crate::goal::GoalFooterStatus>,
    pub loop_status: Option<&'a str>,
    /// Last completed Dreamer pass. `None` preserves the old footer height.
    pub dream: Option<&'a str>,
    pub width: usize,
    /// 뷰포트가 쓸 수 있는 최대 높이 — 피커가 목록 창을 여기에 맞춘다.
    pub max_rows: usize,
}

/// 프레임을 줄들과 커서 좌표로 편다. 커서가 `None` 이면 숨긴다.
#[must_use]
pub fn build(frame: &Frame<'_>) -> (Vec<Line>, Option<(u16, u16)>) {
    build_with_focus(frame, palette::COMMAND_TOKEN)
}

fn build_question(
    frame: &Frame<'_>,
    question: &Question,
    focus: Color,
) -> (Vec<Line>, Option<(u16, u16)>) {
    if !question.is_freeform() && !question.is_other_input() {
        return (question.lines_with_focus(frame.width, frame.max_rows, focus), None);
    }
    // 자유 서술과 Other — codex 도 이 갈래에서는 오버레이 안의 컴포저가
    // 답을 받는다(`…__freeform.snap`: 질문 아래 `› Type your answer`). Other는
    // 목록까지 남기고 같은 컴포저를 그 아래에 둔다.
    let (lines, (col, row)) = frame.composer.render_with_effort(
        frame.width,
        ANSWER_PLACEHOLDER,
        frame.effort_tier,
        frame.effort_effect,
        frame.now,
    );
    let footer = question.footer_lines_with_focus(frame.width, focus);
    let mut rows = if question.is_other_input() {
        question.option_body_with_focus(
            frame.width,
            frame.max_rows,
            lines.len() + 1 + footer.len(),
            focus,
        )
    } else {
        let mut head = question.head_with_focus(frame.width, focus);
        head.push(Line::empty());
        head
    };
    let composer_row = u16::try_from(rows.len()).unwrap_or(0);
    rows.extend(lines);
    rows.push(Line::empty());
    rows.extend(footer);
    (rows, Some((col, composer_row + row)))
}

fn build_with_focus(frame: &Frame<'_>, focus: Color) -> (Vec<Line>, Option<(u16, u16)>) {
    if let Some(dialog) = frame.dialog {
        return (dialog.lines_with_focus(frame.width, focus), None);
    }
    if let Some(picker) = frame.picker {
        return (picker.lines_with_focus(frame.width, frame.max_rows, focus), None);
    }
    if let Some(sessions) = frame.sessions {
        return (sessions.lines(frame.width, frame.max_rows), None);
    }
    if let Some(pager) = frame.pager {
        return (pager.to_vec(), None);
    }
    if let Some(question) = frame.question {
        return build_question(frame, question, focus);
    }
    // The bottom pane first — its height decides how many rows the active
    // cell above it may take (`clip_active`).
    let mut rows: Vec<Line> = Vec::new();
    if let Some(status) = frame.status.filter(|_| !frame.tail_owns_slot) {
        rows.push(Line::empty());
        rows.push(status.line(frame.width));
        rows.extend(status.detail_lines(frame.width));
    }
    if let Some(shortcuts) = frame.shortcuts {
        rows.push(Line::empty());
        for entry in shortcuts {
            rows.push(
                Line::new(vec![Span::raw("  "), Span::dim(entry.clone())])
                    .truncated(frame.width),
            );
        }
    }
    rows.push(Line::empty());
    if let Some(pending_input) = frame.pending_input.filter(|lines| !lines.is_empty()) {
        rows.extend(
            pending_input
                .iter()
                .map(|line| line.clone().truncated(frame.width)),
        );
    }
    rows.push(Line::empty());
    let composer_row = u16::try_from(rows.len()).unwrap_or(0);
    let (lines, (col, row)) = frame.composer.render_with_effort(
        frame.width,
        PLACEHOLDER,
        frame.effort_tier,
        frame.effort_effect,
        frame.now,
    );
    rows.extend(lines);
    rows.push(Line::empty());
    // 팝업은 푸터 자리에 앉는다 — 캡처의 `/model` 프레임이 그 자리에서
    // 모델·cwd 줄을 밀어냈다.
    if let Some(popup) = frame.popup {
        rows.extend(popup.lines_with_focus(frame.width, focus));
    } else {
        if let Some(dream) = frame.dream {
            rows.push(dreamer_footer(dream, frame.width));
        }
        let hint =
            (frame.composer.is_empty() && frame.shortcuts.is_none()).then_some(SHORTCUT_HINT);
        let mut line = footer(
            frame.model,
            frame.effort,
            frame.model_note,
            frame.cwd,
            frame.width,
            hint,
            frame.context_left,
            frame.context_used_tokens,
            frame.plan_mode,
            frame.goal_status,
            frame.loop_status,
        );
        if let Some(effect) = frame
            .effort_effect
            .filter(|effect| !effect.is_finished_at(frame.now))
        {
            line = effect.footer_line_at(&line, frame.width, frame.now);
        }
        rows.push(line);
    }
    // 활성 셀이 먼저 — codex 는 그 위에 빈 줄 하나(`top: 1`)를 띄운다.
    let Some(active) = frame.active.filter(|active| !active.is_empty()) else {
        return (rows, Some((col, composer_row + row)));
    };
    let budget = frame.max_rows.saturating_sub(rows.len() + 1);
    let mut head: Vec<Line> = Vec::with_capacity(budget + 1);
    head.push(Line::empty());
    head.extend(
        clip_active(active, budget)
            .into_iter()
            .map(|line| line.truncated(frame.width)),
    );
    let composer_row = composer_row + u16::try_from(head.len()).unwrap_or(u16::MAX);
    head.extend(rows);
    (head, Some((col, composer_row + row)))
}

/// The active cell's lines that fit above the bottom pane.
///
/// The head is the active cell, then the bottom pane, and the painter caps it
/// at the screen; a cell taller than the rows the bottom pane leaves used to
/// push the composer off the bottom of the pane — `Edited 5 files (+27 -10)`
/// with its five diffs filled a 24-row pane to the last row and the input
/// line was gone (2026-09-07 23:35, t-3063). No cell moves the head: the cell
/// keeps its first rows and says how many it does not show; the whole cell is
/// in history once it commits, and Ctrl+T shows it whole meanwhile.
fn clip_active(active: &[Line], budget: usize) -> Vec<Line> {
    if active.len() <= budget {
        return active.to_vec();
    }
    let shown = budget.saturating_sub(1);
    let mut lines: Vec<Line> = active[..shown].to_vec();
    if budget > 0 {
        lines.push(Line::new(vec![Span::dim(strings::more_lines(
            active.len() - shown,
        ))]));
    }
    lines
}

/// `?` 를 눌렀을 때 보이는 keep-list. codex 의 `? for shortcuts` 자리다.
#[must_use]
pub fn shortcut_card() -> Vec<String> {
    vec![
        "/model [alias]        switch model — no alias opens the model + effort picker".to_string(),
        "/permissions <mode>   read-only · workspace-write · danger-full-access".to_string(),
        format!("/fast                 {}", super::fast::COMMAND_DESCRIPTION),
        "/new [name]           start a new chat — optional name".to_string(),
        "/resume [id]          resume a saved chat — no id opens the picker".to_string(),
        "/compact [focus]      compact the conversation".to_string(),
        "/goal [command]       persistent goal · bounded autonomous gates".to_string(),
        "/loop [command]       bounded count · interval · file-watch loops".to_string(),
        "/status               model · permissions · effort · session · context".to_string(),
        "/help                 this list          /exit  quit (Ctrl-D too)".to_string(),
        "/clear [name]         clear terminal + start a new chat".to_string(),
        "//text                send a literal leading slash".to_string(),
        "esc interrupt · ctrl-c twice to quit · ↑↓ history · shift+tab cycles permissions".to_string(),
    ]
}

/// 마지막 두 경로 조각만 남긴 cwd — 캡처의 푸터도 긴 경로를 줄여 쓴다.
#[must_use]
pub fn short_cwd(cwd: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && cwd.starts_with(&home) {
        return format!("~{}", &cwd[home.len()..]);
    }
    cwd.to_string()
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        build, dreamer_footer, footer, footer_location, palette, selection_rows, shortcut_card,
        Dialog, Frame, Picker, PickerRow, Status, StatusDetailsCapitalization,
    };
    use crate::tui::ansi::{Line, Style};
    use crate::tui::composer::Composer;

    fn frame<'a>(composer: &'a Composer, status: Option<&'a Status>) -> Frame<'a> {
        Frame {
            composer,
            effort_tier: None,
            effort_effect: None,
            now: Instant::now(),
            status,
            pending_input: None,
            active: None,
            tail_owns_slot: false,
            dialog: None,
            question: None,
            picker: None,
            sessions: None,
            pager: None,
            popup: None,
            shortcuts: None,
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
            width: 60,
            max_rows: 39,
        }
    }

    #[test]
    fn the_idle_viewport_contains_the_bordered_composer() {
        let composer = Composer::new();
        let (rows, cursor) = build(&frame(&composer, None));
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain.len(), 7);
        assert_eq!(plain[0], "");
        assert_eq!(plain[1], "");
        assert_eq!(plain[2], format!("╭{}╮", "─".repeat(58)));
        assert_eq!(plain[3], format!("│› Ask zo to do anything{}│", " ".repeat(35)));
        assert_eq!(plain[4], format!("╰{}╯", "─".repeat(58)));
        assert_eq!(plain[5], "");
        assert_eq!(plain[6], "  claude-opus-5 high · /tmp/x   ? for shortcuts");
        assert_eq!(cursor, Some((3, 3)));
    }

    #[test]
    fn a_working_viewport_is_seven_rows_with_the_status_second() {
        let composer = Composer::new();
        let status = Status::working(Duration::from_secs(3));
        let (rows, cursor) = build(&frame(&composer, Some(&status)));
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain.len(), 9);
        assert_eq!(plain[1], "• Working (3s • esc to interrupt)");
        assert_eq!(plain[5], format!("│› Ask zo to do anything{}│", " ".repeat(35)));
        assert_eq!(cursor, Some((3, 5)));
    }

    #[test]
    fn pending_input_sits_between_the_status_and_composer_gaps() {
        let composer = Composer::new();
        let status = Status::working(Duration::from_secs(3));
        let pending = vec![
            Line::from_text("• Queued follow-up inputs"),
            Line::from_text("  ↳ draft"),
            Line::from_text("    ⌥ + ↑ edit last queued message"),
        ];
        let mut frame = frame(&composer, Some(&status));
        frame.pending_input = Some(&pending);
        let (rows, _) = build(&frame);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain[1], "• Working (3s • esc to interrupt)");
        assert_eq!(plain[2], "");
        assert_eq!(plain[3], "• Queued follow-up inputs");
        assert_eq!(plain[5], "    ⌥ + ↑ edit last queued message");
        assert_eq!(plain[6], "");
        assert!(plain[7].starts_with('╭'), "composer border was {plain:?}");
    }

    #[test]
    fn status_details_expand_to_one_row_per_agent() {
        let composer = Composer::new();
        let mut status = Status::working(Duration::from_secs(3));
        status.set_details([
            "scout · reading src/lib.rs · 12s".to_string(),
            "reviewer · thinking · 9s".to_string(),
        ], StatusDetailsCapitalization::Preserve);
        let (rows, _) = build(&frame(&composer, Some(&status)));
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain[2], "  └ scout · reading src/lib.rs · 12s");
        assert_eq!(plain[3], "  └ reviewer · thinking · 9s");
    }

    #[test]
    fn status_details_reserve_the_last_row_for_the_hidden_count() {
        let composer = Composer::new();
        let mut status = Status::working(Duration::from_secs(3));
        status.set_details(
            (1..=5).map(|number| format!("agent-{number}")),
            StatusDetailsCapitalization::Preserve,
        );
        let (rows, _) = build(&frame(&composer, Some(&status)));
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain[2], "  └ agent-1");
        assert_eq!(plain[3], "  └ agent-2");
        assert_eq!(plain[4], "  └ agent-3…");
        assert!(!plain.iter().any(|row| row.contains("+3 more")));
    }

    /// A context trim is noted right before the request leaves, so the
    /// request's own phase must not wipe it: the wait for the model is the
    /// moment it can be read. Content, or a tool boundary, retires it.
    #[test]
    fn a_housekeeping_note_takes_the_activity_place_and_outlives_the_request_phase() {
        use runtime::message_stream::types::StreamPhase;

        let mut status = Status::working(Duration::from_secs(3));
        status.note_housekeeping("Context trim · cleared 3 old tool result(s) (~12k tokens freed)");
        status.note_stream_phase(StreamPhase::RequestSent { attempt: 1 });
        status.elapsed = Duration::from_secs(9);
        assert_eq!(
            status.line(140).plain(),
            "• Working (9s • esc to interrupt) · Context trim · cleared 3 old tool result(s) (~12k tokens freed)",
            "the note hides the waiting phrase it fills the place of"
        );

        status.note_stream_content();
        assert_eq!(status.line(140).plain(), "• Working (9s • esc to interrupt)");

        status.note_housekeeping("Context nearing auto-compaction — threshold 80% of window");
        status.note_tool_started("t1", "bash", Some("cargo test"));
        status.elapsed = Duration::from_secs(10);
        let line = status.line(140).plain();
        assert!(
            !line.contains("Context nearing") && line.contains("Bash"),
            "a tool boundary retires the note: {line}"
        );
    }

    #[test]
    fn status_appends_inline_context_after_the_interrupt_group() {
        let mut status = Status::working(Duration::from_secs(3));
        status.set_inline_message(Some("2 background terminals".to_string()));

        assert_eq!(
            status.line(120).plain(),
            "• Working (3s • esc to interrupt) · 2 background terminals"
        );
    }

    #[test]
    fn status_interrupt_binding_is_its_own_configurable_span() {
        let mut status = Status::working(Duration::ZERO);
        status.set_interrupt_binding("f12");

        let line = status.line(80);

        assert_eq!(line.plain(), "• Working (0s • f12 to interrupt)");
        assert!(line.spans.iter().any(|span| span.text == "f12"));
    }

    #[test]
    fn one_long_status_detail_wraps_before_the_line_cap() {
        let mut status = Status::working(Duration::ZERO);
        status.set_details(
            ["alpha bravo charlie delta".to_string()],
            StatusDetailsCapitalization::Preserve,
        );

        let plain: Vec<String> = status.detail_lines(14).iter().map(Line::plain).collect();

        assert_eq!(plain, ["  └ alpha", "    bravo", "    charlie…"]);
    }

    #[test]
    fn status_detail_capitalization_is_a_surface_policy() {
        let mut status = Status::working(Duration::ZERO);
        status.set_details(
            ["reading src/lib.rs".to_string()],
            StatusDetailsCapitalization::CapitalizeFirst,
        );

        assert_eq!(status.details, ["Reading src/lib.rs"]);
    }

    #[test]
    fn a_dialog_owns_the_viewport_and_hides_the_cursor() {
        let composer = Composer::new();
        let dialog = Dialog {
            title: "You are in".to_string(),
            subject: "/tmp/x".to_string(),
            body: "Do you trust the contents of this directory?".to_string(),
            options: vec!["Yes, continue".to_string(), "No, quit".to_string()],
            selected: 0,
            footer: "Press enter to continue".to_string(),
        };
        let mut frame = frame(&composer, None);
        frame.dialog = Some(&dialog);
        let (rows, cursor) = build(&frame);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(cursor, None);
        assert_eq!(plain[0], "> You are in /tmp/x");
        assert!(plain.contains(&"› 1. Yes, continue".to_string()));
        assert!(plain.contains(&"  2. No, quit".to_string()));
        let selected = rows
            .iter()
            .find(|line| line.plain() == "› 1. Yes, continue")
            .expect("selected dialog row");
        assert!(selected.spans.iter().all(|span| span.style.bold));
        assert!(selected
            .spans
            .iter()
            .all(|span| span.style.fg == Some(crate::tui::palette::COMMAND_TOKEN)));
        assert_eq!(plain.last().expect("footer"), "Press enter to continue");
    }

    #[test]
    fn the_footer_colours_model_and_cwd_apart() {
        let line = footer("m", "high", None, "/tmp", 40, None, None, None, false, None, None);
        assert_eq!(line.plain(), "  m high · /tmp");
        assert_eq!(line.spans[1].style.fg, Some(crate::tui::palette::FOOTER_MODEL));
        assert_eq!(line.spans[3].style.fg, Some(crate::tui::palette::FOOTER_CWD));
    }

    /// The wire note sits between the model and the path, in the warn tint,
    /// and is absent when nothing is swapped (the test above).
    #[test]
    fn a_wire_note_sits_between_the_model_and_the_path_in_warn() {
        let line = footer(
            "claude-opus-5",
            "smart",
            Some("safety fallback"),
            "/tmp",
            60,
            None,
            None,
            None,
            false,
            None,
            None,
        );
        assert_eq!(line.plain(), "  claude-opus-5 smart · safety fallback · /tmp");
        let note = line
            .spans
            .iter()
            .find(|span| span.text == "safety fallback")
            .expect("note span");
        assert_eq!(note.style.fg, Some(crate::tui::palette::NOTICE_WARN));
    }

    #[test]
    fn the_dreamer_turn_footer_is_one_dim_bounded_line() {
        let dream = "dreamer: promoted 2, skipped 0 (now): gate-order, pty-lane-flush";
        let line = dreamer_footer(dream, 44);
        assert!(line.plain().starts_with("  dreamer: promoted 2"));
        assert!(line.width() <= 44, "footer must stay within the pane");
        assert!(
            line.spans.iter().all(|span| span.style.dim),
            "the whole dreamer footer must remain secondary chrome"
        );

        let composer = Composer::new();
        let mut frame = frame(&composer, None);
        frame.dream = Some(dream);
        let (rows, _) = build(&frame);
        assert_eq!(
            rows[rows.len() - 2].plain(),
            dreamer_footer(dream, frame.width).plain()
        );
        assert!(
            rows[rows.len() - 2]
                .spans
                .iter()
                .all(|span| span.style.dim),
            "the integrated row must keep the helper's dim style"
        );
    }

    #[test]
    fn worktree_context_joins_the_footer_location_without_a_new_colour_axis() {
        let location = footer_location("/repo", Some("wt/task ← main"));
        let line = footer(
            "m", "high", None, &location, 80, None, None, None, false, None, None,
        );

        assert_eq!(line.plain(), "  m high · wt/task ← main · /repo");
        let location_span = line
            .spans
            .iter()
            .find(|span| span.text == location)
            .expect("footer location span");
        assert_eq!(location_span.style.fg, Some(crate::tui::palette::FOOTER_CWD));

        let long = footer_location(
            "/private/tmp/a/very/long/repository/path",
            Some("wt/task ← main"),
        );
        let narrow = footer("m", "", None, &long, 32, None, None, None, false, None, None);
        assert!(narrow.plain().contains("wt/task ← main"));
    }

    /// The window's budget is lines on screen: a row whose description wraps
    /// counts for every line it takes, and the window sheds rows from the
    /// end away from the selection until it fits.
    #[test]
    fn a_wrapping_row_costs_the_window_the_lines_it_takes() {
        let picker = Picker {
            title: "Choose".to_string(),
            title_style: Style::new().bold(),
            note: String::new(),
            rows: (1..=4)
                .map(|index| PickerRow {
                    label: format!("choice {index}"),
                    description: "a description long enough to wrap onto a second line at this width for sure".to_string(),
                    dim: false,
                })
                .collect(),
            selected: 0,
            footer: "enter or esc".to_string(),
        };
        // Chrome is six rows here (no note); a budget of four lines holds two
        // wrapped rows, and the selection is the first of them.
        let plain: Vec<String> = picker.lines(60, 11).iter().map(Line::plain).collect();
        assert_eq!(plain.len(), 10, "no more lines than the budget: {plain:?}");
        assert!(plain[4].starts_with("› 1. choice 1"), "{plain:?}");
        assert!(plain[6].starts_with("  2. choice 2"), "{plain:?}");
    }

    /// The option window is as tall as the screen allows: on a tall screen
    /// every choice shows; on a short one the window scrolls to keep the
    /// selection in view.
    /// A row its source stopped listing is offered faded: every span dim
    /// when it is not the selection, the focus colour without the weight
    /// when it is (t-3054). Its neighbours render as before.
    #[test]
    fn an_unlisted_picker_row_is_dim_and_stays_selectable() {
        let rows = vec![
            PickerRow {
                label: "gpt-6-astra (current)".to_string(),
                description: "출처가 목록에서 뺐음 · 09-07 23:25".to_string(),
                dim: true,
            },
            PickerRow {
                label: "gpt-5.6-sol".to_string(),
                description: "Frontier model".to_string(),
                dim: false,
            },
        ];
        let selected_first = selection_rows(&rows, 0, 80, 10, palette::COMMAND_TOKEN);
        let astra = &selected_first[0];
        assert!(astra.plain().starts_with("› 1. gpt-6-astra (current)"));
        assert!(astra.spans.iter().all(|span| span.style.dim && !span.style.bold), "{astra:?}");
        assert!(astra.spans.iter().all(|span| span.style.fg == Some(palette::COMMAND_TOKEN)), "the focus colour stays");
        let sol = &selected_first[1];
        assert!(!sol.spans[1].style.dim, "a listed row's label is not dim");

        let selected_second = selection_rows(&rows, 1, 80, 10, palette::COMMAND_TOKEN);
        let astra = &selected_second[0];
        assert!(astra.spans.iter().all(|span| span.style.dim && !span.style.bold && span.style.fg.is_none()), "{astra:?}");
        let sol = &selected_second[1];
        assert!(sol.spans.iter().all(|span| span.style.bold), "the listed selection keeps its weight");
    }

    #[test]
    fn a_large_picker_fills_the_screen_it_has_and_scrolls_on_a_short_one() {
        let picker = Picker {
            title: "Choose".to_string(),
            title_style: Style::new().bold(),
            note: "Many choices".to_string(),
            rows: (1..=6)
                .map(|index| PickerRow {
                    label: format!("choice {index}"),
                    description: String::new(),
                    dim: false,
                })
                .collect(),
            selected: 4,
            footer: "enter or esc".to_string(),
        };
        let tall: Vec<String> = picker.lines(80, 39).iter().map(Line::plain).collect();
        assert_eq!(
            &tall[5..11],
            [
                "  1. choice 1  ",
                "  2. choice 2  ",
                "  3. choice 3  ",
                "  4. choice 4  ",
                "› 5. choice 5  ",
                "  6. choice 6  ",
            ]
        );
        assert_eq!(tall.len(), 13, "seven chrome rows and all six choices");

        let short: Vec<String> = picker.lines(80, 10).iter().map(Line::plain).collect();
        assert_eq!(
            &short[5..8],
            ["  3. choice 3  ", "  4. choice 4  ", "› 5. choice 5  "]
        );
        assert_eq!(short.len(), 10, "the window gave up rows, not the chrome");
    }

    #[test]
    fn the_hint_never_loses_its_seat_to_a_long_path() {
        let long = "/private/tmp/one/two/three/four/five/six/seven/eight/nine";
        let line = footer(
            "claude-opus-5",
            "high", None,
            long,
            60,
            Some("? for shortcuts"),
            None,
            None,
            false,
            None,
            None,
        );
        assert!(line.width() <= 60);
        assert!(
            line.plain().ends_with("   ? for shortcuts"),
            "footer was {:?}",
            line.plain()
        );
    }

    #[test]
    fn a_path_that_fills_its_budget_still_separates_plan_mode() {
        let line = footer("m", "high", None, "/work", 25, None, None, None, true, None, None);
        assert!(line.plain().contains("… Plan mode"), "footer was {line:?}");
        assert!(!line.plain().contains("…Plan mode"), "footer was {line:?}");
    }

    #[test]
    fn narrow_footer_yields_hint_then_context_then_path() {
        let path = "/private/tmp/project/with/a/long/path";
        for width in 32..=48 {
            let line = footer(
                "m",
                "high", None,
                path,
                width,
                Some("? for shortcuts"),
                Some(80),
                None,
                true,
                None,
                None,
            )
            .plain();
            // The hint is always the first thing to give up. It must never
            // survive while either higher-priority plan/context information
            // was removed to retain cwd.
            if line.contains("? for shortcuts") {
                assert!(line.contains("Plan mode"), "width {width}: {line:?}");
                assert!(line.contains("80% context left"), "width {width}: {line:?}");
            }
            if !line.contains("80% context left") {
                assert!(!line.contains("? for shortcuts"), "width {width}: {line:?}");
            }
        }
    }

    #[test]
    fn a_full_optional_tail_yields_hint_before_discarding_cwd() {
        // 57 is exactly the head plus Plan mode, context, and hint. The old
        // allocator held all of that tail and gave cwd zero columns; adding a
        // cwd reservation must evict the lowest-priority hint instead.
        let line = footer(
            "m",
            "high", None,
            "/private/tmp/project/with/a/long/path",
            57,
            Some("? for shortcuts"),
            Some(80),
            None,
            true,
            None,
            None,
        )
        .plain();
        assert!(!line.contains("? for shortcuts"), "footer was {line:?}");
        assert!(line.contains("80% context left"), "footer was {line:?}");
        assert!(line.contains('/'), "footer was {line:?}");
    }

    #[test]
    fn typing_retires_the_shortcut_hint() {
        let mut composer = Composer::new();
        composer.insert_str("x");
        let (rows, _) = build(&frame(&composer, None));
        let footer = rows.last().expect("footer").plain();
        assert!(!footer.contains("? for shortcuts"), "footer was {footer:?}");
    }

    #[test]
    fn the_shortcut_card_covers_the_keep_list() {
        let card = shortcut_card().join("\n");
        // 목록은 카탈로그에서 읽는다 — 카드와 keep-list 가 갈라지면 여기서 잡힌다.
        for command in crate::slash::Slash::CATALOG {
            assert!(
                card.contains(command.name()),
                "missing {} in {card}",
                command.name()
            );
        }
        assert!(card.contains("//"), "the escape hatch is missing: {card}");
        assert!(!card.contains("/effort"), "the card still offers /effort: {card}");
    }

    /// 활성 셀은 Working 줄 **위**에 빈 줄 하나를 두고 앉는다 — codex
    /// `as_renderable` 의 `top: 1`.
    #[test]
    fn a_running_tool_cell_sits_above_the_working_row() {
        let composer = Composer::new();
        let status = Status::working(Duration::from_secs(1));
        let active = vec![
            Line::from_text("• Exploring"),
            Line::from_text("  └ Read README.md"),
        ];
        let mut frame = frame(&composer, Some(&status));
        frame.active = Some(&active);
        let (rows, _) = build(&frame);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain[0], "");
        assert_eq!(plain[1], "• Exploring");
        assert_eq!(plain[2], "  └ Read README.md");
        assert_eq!(plain[3], "");
        assert_eq!(plain[4], "• Working (1s • esc to interrupt)");
    }

    /// 스트림 꼬리가 칸을 쥐면 Working 줄은 물러난다(codex 는 꼬리를 세울 때
    /// `hide_status_indicator()` 를 부른다). 턴은 계속 돈다 — esc 도 그대로다.
    #[test]
    fn a_stream_tail_hides_the_working_row() {
        let composer = Composer::new();
        let status = Status::working(Duration::from_secs(1));
        let active = vec![Line::from_text("• the sea holds a patience")];
        let mut frame = frame(&composer, Some(&status));
        frame.active = Some(&active);
        frame.tail_owns_slot = true;
        let (rows, _) = build(&frame);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert!(!plain.iter().any(|row| row.contains("Working")), "rows were {plain:?}");
        assert_eq!(plain[1], "• the sea holds a patience");
    }

    /// A model reasoning without visible output is a phrase on the status
    /// line — never a row in the transcript (2026-09-07: one turn left five
    /// `model reasoning silently` rows behind) — and content ends it.
    #[test]
    fn quiet_reasoning_is_a_status_phrase_that_content_ends() {
        use runtime::message_stream::types::StreamPhase;
        // 70 s into the turn the backend says the stream has been quiet 61 s.
        let mut status = Status::working(Duration::from_secs(70));
        status.note_stream_phase(StreamPhase::QuietReasoning { since_secs: 61 });
        let line = status.line(120).plain();
        assert!(line.contains("model reasoning silently") && line.contains("1m 01s"), "{line}");
        status.elapsed += Duration::from_secs(30);
        let line = status.line(120).plain();
        assert!(line.contains("1m 31s"), "the phrase keeps the clock: {line}");
        status.note_stream_content();
        let line = status.line(120).plain();
        assert!(!line.contains("reasoning silently"), "{line}");
    }

    /// The status line names a request that has been out for a while, names
    /// a retry with its attempt and pause, and says nothing once content flows.
    #[test]
    fn the_status_line_says_where_the_request_stands() {
        use crate::autonomy::limits::DEFAULT_STREAM_PHASE_AFTER_SECS;
        use runtime::message_stream::types::StreamPhase;
        let mut status = Status::working(Duration::from_secs(10));
        status.note_stream_phase(StreamPhase::RequestSent { attempt: 1 });
        let line = status.line(120).plain();
        assert!(!line.contains("waiting"), "a request just out is not yet a wait: {line}");
        status.elapsed = Duration::from_secs(10 + DEFAULT_STREAM_PHASE_AFTER_SECS);
        let line = status.line(120).plain();
        assert!(line.contains("waiting for the model 15s"), "{line}");
        status.elapsed = Duration::from_secs(10 + 125);
        let line = status.line(120).plain();
        assert!(line.contains("quiet 2m 05s"), "{line}");

        status.note_stream_phase(StreamPhase::Retrying { attempt: 2, delay_secs: 3 });
        let line = status.line(120).plain();
        assert!(line.contains("reconnecting · attempt 2 in 3s"), "{line}");
        status.elapsed += Duration::from_secs(5);
        let line = status.line(120).plain();
        assert!(line.contains("reconnecting · attempt 2") && !line.contains(" in "), "{line}");

        status.note_stream_phase(StreamPhase::RequestSent { attempt: 2 });
        status.elapsed += Duration::from_secs(DEFAULT_STREAM_PHASE_AFTER_SECS);
        let line = status.line(120).plain();
        assert!(line.contains("waiting for the model 15s"), "{line}");
        status.note_stream_content();
        status.elapsed += Duration::from_secs(60);
        let line = status.line(120).plain();
        assert!(!line.contains("waiting") && !line.contains("reconnecting"), "{line}");
    }

    /// A live cell taller than the rows above the bottom pane keeps its first
    /// rows and says how many it hides; the composer and the footer stay,
    /// and the head never exceeds `max_rows` (t-3063, the 24-row pane whose
    /// `Edited 5 files` diffs pushed the input line off the bottom).
    #[test]
    fn a_tall_active_cell_is_clipped_so_the_composer_stays_in_the_head() {
        let composer = Composer::new();
        let status = Status::working(Duration::from_secs(3));
        let cell: Vec<Line> = (1..=110)
            .map(|index| Line::from_text(format!("diff line {index}")))
            .collect();
        let mut frame = frame(&composer, Some(&status));
        frame.active = Some(&cell);
        frame.max_rows = 23;
        let (rows, cursor) = build(&frame);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain.len(), 23, "{plain:#?}");
        // The bottom pane is the working viewport's 9 rows, the cell above it
        // gets 23 - 9 - 1 = 13 rows: 12 of its own and the count of the rest.
        assert_eq!(plain[1], "diff line 1");
        assert_eq!(plain[12], "diff line 12");
        assert_eq!(plain[13], "… 98 more lines");
        assert_eq!(plain[14], "");
        assert_eq!(plain[15], "• Working (3s • esc to interrupt)");
        assert_eq!(plain[19], format!("│› Ask zo to do anything{}│", " ".repeat(35)));
        assert_eq!(plain[22], "  claude-opus-5 high · /tmp/x   ? for shortcuts");
        assert_eq!(cursor, Some((3, 19)));

        // A cell that fits is drawn whole, as before.
        let short: Vec<Line> = cell[..5].to_vec();
        frame.active = Some(&short);
        let (rows, cursor) = build(&frame);
        let plain: Vec<String> = rows.iter().map(Line::plain).collect();
        assert_eq!(plain.len(), 15, "{plain:#?}");
        assert_eq!(plain[5], "diff line 5");
        assert_eq!(plain[7], "• Working (3s • esc to interrupt)");
        assert_eq!(cursor, Some((3, 11)));
    }
}
