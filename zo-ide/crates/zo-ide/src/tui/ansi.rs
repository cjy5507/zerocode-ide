//! 스타일 있는 텍스트 모델과 SGR 방출.
//!
//! ratatui 를 쓰지 않는다. codex 가 `Line`/`Span`/`Style` 로 표현하는 것을 이
//! 파일이 최소한으로 되풀이하고, 그 표현을 터미널 바이트로 옮긴다. 값 타입이
//! 작고 순수해서 캡처 기반 바이트 테스트가 painter 를 통과하지 않고도
//! 스타일 규칙 하나를 핀할 수 있다.
//!
//! 방출 규율은 codex `insert_history.rs::write_spans` + `ModifierDiff::queue`
//! **그대로**다. 스팬마다 (1) 속성 차분을 끄는 것부터 켜는 것 순서로,
//! (2) 그 다음 색을 `SetColors` 한 방에 낸다. `ESC[0m` 되감기는 없다 — 줄
//! 끝에서만 한 번 나간다([`LINE_TAIL`]).
//!
//! 그 차분에는 눈에 띄는 지문 하나가 있다: bold 를 끄는데 새 스타일이 dim 이면
//! `ESC[22m ESC[2m` 을 내고, 이어지는 "dim 을 켠다" 가 `ESC[2m` 을 **한 번 더**
//! 낸다. 캡처의 부팅 카드 제목줄이 바로 그 모양이다 —
//! `ESC[2m│ >_ ESC[22mESC[1mOpenAI CodexESC[22mESC[2mESC[2m (v0.149.1)`
//! (`docs/captures/codex-tui-v0.149.1-boot-trust.bin`, 히스토리 2행). 원본의
//! 군더더기지만 바이트가 기준이므로 그대로 복제한다.

use std::fmt::Write as _;

use unicode_width::UnicodeWidthStr;

use super::palette::{self, TerminalPalette};

/// 전경색. 30번대 기본색·256색·트루컬러 셋 다 캡처에 나온다
/// (`ESC[38;5;6m` 은 부팅 카드의 `/model`, `ESC[38;2;…m` 은 shimmer·푸터).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// 30~37 기본색.
    Base(u8),
    /// 90~97 밝은 기본색. ratatui의 `Light*` 어휘와 같은 SGR이다.
    BrightBase(u8),
    /// `ESC[38;5;<n>m`.
    Indexed(u8),
    /// `ESC[38;2;<r>;<g>;<b>m`.
    Rgb(u8, u8, u8),
}

impl Color {
    pub const RED: Self = Self::Base(1);
    pub const GREEN: Self = Self::Base(2);
    pub const YELLOW: Self = Self::Base(3);
    pub const MAGENTA: Self = Self::Base(5);
    pub const CYAN: Self = Self::Base(6);
    pub const LIGHT_BLUE: Self = Self::BrightBase(4);

    /// 이 색의 SGR **파라미터**만. codex 는 전경·배경을 `SetColors` 하나로
    /// 묶어 `ESC[<fg>;<bg>m` 을 내므로(crossterm `SetColors::write_ansi` 의
    /// "one command resulted in about 20% more FPS" 주석) 색 하나가 완결된
    /// 이스케이프를 만들지 않는다.
    fn params(self, out: &mut String) {
        match self {
            Self::Base(index) => {
                let _ = write!(out, "{}", 30 + u16::from(index));
            }
            Self::BrightBase(index) => {
                let _ = write!(out, "{}", 90 + u16::from(index));
            }
            Self::Indexed(index) => {
                let _ = write!(out, "38;5;{index}");
            }
            Self::Rgb(r, g, b) => {
                let _ = write!(out, "38;2;{r};{g};{b}");
            }
        }
    }

    /// The same color as a background parameter — every foreground base
    /// shifted by ten, which is the whole of the SGR rule (30→40, 38→48).
    fn background_params(self, out: &mut String) {
        match self {
            Self::Base(index) => {
                let _ = write!(out, "{}", 40 + u16::from(index));
            }
            Self::BrightBase(index) => {
                let _ = write!(out, "{}", 100 + u16::from(index));
            }
            Self::Indexed(index) => {
                let _ = write!(out, "48;5;{index}");
            }
            Self::Rgb(r, g, b) => {
                let _ = write!(out, "48;2;{r};{g};{b}");
            }
        }
    }
}

/// 한 스팬의 표시 속성. 캡처에 나오는 것만 있다(90번대 없음).
///
/// 불리언이 넷인 건 SGR 속성이 넷이기 때문이다 — 묶어 봐야 비트필드 이름을
/// 새로 지어야 하고, 그러면 `Style::new().bold().dim()` 이 읽히지 않는다.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    /// Background. `None` is the terminal's own — the only value the screen
    /// had until diffs needed a tinted line, so every existing pin still
    /// renders `;49m`.
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

impl Style {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            strike: false,
        }
    }

    #[must_use]
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    #[must_use]
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    #[must_use]
    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    #[must_use]
    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    #[must_use]
    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    #[must_use]
    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    #[must_use]
    pub const fn strike(mut self) -> Self {
        self.strike = true;
        self
    }

    fn resolved_for(mut self, palette: Option<TerminalPalette>) -> Self {
        if let Some(palette) = palette {
            self.fg = self.fg.and_then(|color| palette.resolve(color));
            self.bg = self.bg.and_then(|color| palette.resolve(color));
        }
        self
    }

    /// `other` 의 켜진 속성·색을 덮어씌운다 (ratatui `patch` 대응).
    #[must_use]
    pub fn patch(mut self, other: Self) -> Self {
        if other.fg.is_some() {
            self.fg = other.fg;
        }
        if other.bg.is_some() {
            self.bg = other.bg;
        }
        self.bold |= other.bold;
        self.dim |= other.dim;
        self.italic |= other.italic;
        self.underline |= other.underline;
        self.strike |= other.strike;
        self
    }

}

/// 한 스팬 — 같은 스타일의 연속 텍스트.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

impl Span {
    #[must_use]
    pub fn new(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    #[must_use]
    pub fn raw(text: impl Into<String>) -> Self {
        Self::new(text, Style::new())
    }

    #[must_use]
    pub fn dim(text: impl Into<String>) -> Self {
        Self::new(text, Style::new().dim())
    }

    #[must_use]
    pub fn bold(text: impl Into<String>) -> Self {
        Self::new(text, Style::new().bold())
    }

    #[must_use]
    pub fn width(&self) -> usize {
        UnicodeWidthStr::width(self.text.as_str())
    }
}

/// 한 줄 — 스팬의 나열과 **줄 자체의 스타일**.
///
/// [`Line::style`] 은 ratatui `Line::style` 자리다. codex `insert_history.rs`
/// 가 행을 쓸 때 두 군데서 쓴다: (1) 행 머리의 `SetColors` 가 스팬이 아니라
/// **줄**의 색을 낸다(`ESC[38;2;…;49m ESC[K`), (2) 스팬마다
/// `s.style.patch(line.style)` 로 줄 스타일을 접어 넣는다 — "Merge line-level
/// style into each span so that ANSI colors reflect line styles (e.g.
/// blockquotes with green fg)". 표 헤더 행이 그 두 규칙의 실측이다
/// (`docs/captures/codex-tui-v0.149.1-table.bin`).
///
/// [`Line::lead`]·[`Line::trail`] 은 **표시되지 않는 제어 바이트**다. 폭에도
/// [`Line::plain`] 에도 안 들어가고, 히스토리로 나갈 때만 그 행의 앞뒤에 그대로
/// 흘러간다 — 접기 마커([`super::folds`], OSC 7788)의 자리다. 뷰포트 그리기는
/// 매 프레임 다시 쓰는 자리라 마커를 내지 않는다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
    /// 줄 전체에 깔리는 스타일 — 스팬 스타일이 이것을 `patch` 한다.
    pub style: Style,
    /// 이 행을 쓰기 직전에 흘릴 제어 바이트.
    pub lead: Option<Box<str>>,
    /// 이 행을 쓴 직후에 흘릴 제어 바이트.
    pub trail: Option<Box<str>>,
    /// The indent a wrapped continuation row of this line starts with — the
    /// list or quote prefix the line sits under, so a long item wraps under
    /// its own text rather than under its marker (codex's `subsequent_indent`).
    /// `None` wraps flush left. Not shown, not counted in [`Self::width`].
    pub continuation: Option<Span>,
    /// The logical line this row was wrapped from, when it is a wrapped row
    /// of a longer line — carrying the cell prefix and its own
    /// [`Self::continuation`], so a rebuild at another width re-wraps the
    /// line instead of reprinting rows cut for the old one. `None` for a
    /// line that is its own row.
    pub origin: Option<std::sync::Arc<Line>>,
}

impl Line {
    #[must_use]
    pub fn new(spans: Vec<Span>) -> Self {
        Self {
            spans,
            style: Style::new(),
            lead: None,
            trail: None,
            continuation: None,
            origin: None,
        }
    }

    /// 줄 스타일을 입힌다 — ratatui `Line::style`.
    #[must_use]
    pub const fn styled(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }

    /// 앞에 흘릴 제어 바이트를 단다.
    #[must_use]
    pub fn with_lead(mut self, control: impl Into<Box<str>>) -> Self {
        self.lead = Some(control.into());
        self
    }

    /// 뒤에 흘릴 제어 바이트를 단다.
    #[must_use]
    pub fn with_trail(mut self, control: impl Into<Box<str>>) -> Self {
        self.trail = Some(control.into());
        self
    }

    /// Name the indent a wrapped continuation row of this line starts with.
    #[must_use]
    pub fn with_continuation(mut self, indent: Option<Span>) -> Self {
        self.continuation = indent;
        self
    }

    /// `source` 가 달고 있던 제어 바이트를 그대로 물려받는다 — 줄을 다시
    /// 만드는 자리(자르기·접기)에서 마커가 증발하지 않게 하는 이음매다.
    #[must_use]
    fn inheriting(mut self, source: &Self) -> Self {
        self.style = source.style;
        self.lead.clone_from(&source.lead);
        self.trail.clone_from(&source.trail);
        self.continuation.clone_from(&source.continuation);
        self
    }

    #[must_use]
    pub fn from_text(text: impl Into<String>) -> Self {
        Self::new(vec![Span::raw(text)])
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    /// 장식을 벗긴 본문 — 테스트와 폭 계산의 기준.
    #[must_use]
    pub fn plain(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// 모든 스팬에 `style` 을 덧입힌다(reasoning 셀의 dim+italic 처리).
    #[must_use]
    pub fn patched(mut self, style: Style) -> Self {
        for span in &mut self.spans {
            span.style = span.style.patch(style);
        }
        self
    }

    /// 첫 줄에는 `initial`, 그 뒤로는 `subsequent` 접두어를 붙인다.
    #[must_use]
    pub fn prefixed(mut self, prefix: Span) -> Self {
        self.spans.insert(0, prefix);
        self
    }

    /// 표시 폭이 `width` 를 넘으면 `…` 로 자른다.
    #[must_use]
    pub fn truncated(self, width: usize) -> Self {
        if width == 0 {
            return Self::empty().inheriting(&self);
        }
        if self.width() <= width {
            return self;
        }
        let budget = width.saturating_sub(1);
        let (style, lead, trail) = (self.style, self.lead, self.trail);
        let mut used = 0usize;
        let mut spans: Vec<Span> = Vec::new();
        for span in self.spans {
            if used >= budget {
                break;
            }
            let mut text = String::new();
            for ch in span.text.chars() {
                let cell = char_width(ch);
                if used + cell > budget {
                    break;
                }
                used += cell;
                text.push(ch);
            }
            if !text.is_empty() {
                spans.push(Span::new(text, span.style));
            }
        }
        spans.push(Span::dim("…"));
        Self {
            spans,
            style,
            lead,
            trail,
            continuation: self.continuation,
            origin: None,
        }
    }
}

/// 문자열의 표시 폭.
///
/// 문자별 폭을 더하는 판이 따로 있었는데(경로 축약), 결합 문자·ZWJ 이모지·
/// 가변 선택자에서 문자열 단위 계산보다 크게 나온다 — 표 격자와 경로 축약이
/// 서로 다른 폭 모델로 같은 화면에 붙던 자리다. 폭은 여기 한 곳에서 잰다.
#[must_use]
pub fn str_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

/// 한 글자의 표시 폭. 제어문자는 0 으로 친다.
#[must_use]
pub fn char_width(ch: char) -> usize {
    let mut buffer = [0u8; 4];
    UnicodeWidthStr::width(ch.encode_utf8(&mut buffer) as &str)
}

/// 스팬 사이의 SGR 전이를 흘리는 작은 상태기계 — codex
/// `insert_history.rs` 의 `write_spans` + `ModifierDiff::queue` 그대로다.
///
/// 두 단계다. 먼저 **속성** 차분을 codex 의 순서대로 낸다(끄는 것 전부, 그
/// 다음 켜는 것 전부), 그 다음 색이 하나라도 바뀌었으면 전경·배경을 한
/// `SetColors` SGR 로 낸다. 순서가 뒤바뀌면 캡처와 바이트가 어긋난다 —
/// 부팅 카드 `model:` 줄의 `ESC[2mESC[39;49m` 가 속성-먼저의 증거다.
#[derive(Debug)]
pub struct StyleWriter {
    current: Style,
    palette: Option<TerminalPalette>,
}

impl Default for StyleWriter {
    fn default() -> Self {
        Self {
            current: Style::new(),
            palette: palette::terminal_palette(),
        }
    }
}

impl StyleWriter {
    /// 줄 머리의 상태 — 색은 터미널 기본값, 속성은 전부 꺼짐.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn with_palette(palette: Option<TerminalPalette>) -> Self {
        Self {
            current: Style::new(),
            palette,
        }
    }

    /// `style` 로 전이하는 데 필요한 바이트를 `out` 에 붙인다.
    pub fn transition(&mut self, style: Style, out: &mut String) {
        let style = style.resolved_for(self.palette);
        let previous = self.current;
        self.current = style;
        Self::modifier_diff(previous, style, out);
        if previous.fg != style.fg || previous.bg != style.bg {
            Self::set_colors(style.fg, style.bg, out);
        }
    }

    /// codex `ModifierDiff::queue` — 끄는 속성 먼저, 켜는 속성 나중.
    ///
    /// bold 를 끄면서 새 스타일이 dim 이면 `ESC[22m ESC[2m` 이 나가고, 아래
    /// "dim 을 켠다" 가 `ESC[2m` 을 한 번 더 낸다. 원본의 중복이지만 캡처에
    /// 그대로 찍혀 있으므로 지우지 않는다.
    fn modifier_diff(from: Style, to: Style, out: &mut String) {
        if from.bold && !to.bold {
            out.push_str("\u{1b}[22m");
            if to.dim {
                out.push_str("\u{1b}[2m");
            }
        }
        if from.italic && !to.italic {
            out.push_str("\u{1b}[23m");
        }
        if from.underline && !to.underline {
            out.push_str("\u{1b}[24m");
        }
        if from.dim && !to.dim {
            out.push_str("\u{1b}[22m");
        }
        if from.strike && !to.strike {
            out.push_str("\u{1b}[29m");
        }
        if to.bold && !from.bold {
            out.push_str("\u{1b}[1m");
        }
        if to.italic && !from.italic {
            out.push_str("\u{1b}[3m");
        }
        if to.underline && !from.underline {
            out.push_str("\u{1b}[4m");
        }
        if to.dim && !from.dim {
            out.push_str("\u{1b}[2m");
        }
        if to.strike && !from.strike {
            out.push_str("\u{1b}[9m");
        }
    }

    /// crossterm `SetColors` — 전경과 배경을 한 SGR 로. 없으면 각각 기본값
    /// (`39`·`49`)이다. 배경이 `None` 인 동안 바이트는 예전과 한 글자도
    /// 다르지 않다 — diff 의 색칠한 줄만 새 꼬리를 낸다.
    pub fn set_colors(fg: Option<Color>, bg: Option<Color>, out: &mut String) {
        out.push_str("\u{1b}[");
        Self::foreground_params(fg, out);
        out.push(';');
        Self::background_params(bg, out);
        out.push('m');
    }

    /// 주입한 팔레트에 맞게 색을 낮춘 뒤 한 `SetColors` SGR로 쓴다.
    ///
    /// `palette == None`은 기존 [`Self::set_colors`]와 완전히 같은 바이트를
    /// 낸다. 실제 터미널을 만지지 않는 골든이 이 이음매로 실패·적응 갈래를
    /// 각각 고정한다.
    pub fn set_colors_for_palette(
        palette: Option<TerminalPalette>,
        fg: Option<Color>,
        bg: Option<Color>,
        out: &mut String,
    ) {
        let fg = palette.map_or(fg, |palette| fg.and_then(|color| palette.resolve(color)));
        let bg = palette.map_or(bg, |palette| bg.and_then(|color| palette.resolve(color)));
        Self::set_colors(fg, bg, out);
    }

    /// crossterm `SetForegroundColor` — 전경만 켜고 끄는 한 SGR
    /// (`ESC[36m` · `ESC[39m`). 화면 차분기 **밖**의 자리가 이것을 쓴다:
    /// codex `main.rs::format_exit_messages` 는 TUI 를 놓은 뒤 재개 명령을
    /// 이 문법으로 감싼다([`super::summary`]).
    pub fn set_foreground(fg: Option<Color>, out: &mut String) {
        out.push_str("\u{1b}[");
        Self::foreground_params(fg, out);
        out.push('m');
    }

    fn foreground_params(fg: Option<Color>, out: &mut String) {
        match fg {
            Some(color) => color.params(out),
            None => out.push_str("39"),
        }
    }

    /// Background half of the same SGR. `Color::params` writes the foreground
    /// form (`38;…`/`3x`), so the background base is shifted by ten — the SGR
    /// convention that makes 30→40 and 38→48.
    fn background_params(bg: Option<Color>, out: &mut String) {
        match bg {
            Some(color) => color.background_params(out),
            None => out.push_str("49"),
        }
    }
}

/// SGR 0.
pub const RESET: &str = "\u{1b}[0m";

/// 히스토리 한 행의 꼬리 — codex `write_spans` 가 스팬을 다 쓰고 내는
/// `SetForegroundColor(Reset)` · `SetBackgroundColor(Reset)` ·
/// `SetAttribute(Reset)` 세 명령이다. `SetColors` 와 달리 묶이지 않는다.
pub const LINE_TAIL: &str = "\u{1b}[39m\u{1b}[49m\u{1b}[0m";

/// 행 머리의 `SetColors` — codex `write_history_line` 이 `ESC[K` 앞에 내는
/// 것으로, **줄 스타일**의 전경색이다(스팬의 것이 아니다). 색이 없으면
/// `ESC[39;49m` 이고, 표 헤더처럼 줄에 색이 있으면 그 색이 여기 나온다.
#[must_use]
pub fn line_lead(line: &Line) -> String {
    let mut out = String::new();
    StyleWriter::set_colors_for_palette(
        palette::terminal_palette(),
        line.style.fg,
        line.style.bg,
        &mut out,
    );
    out
}

/// 한 줄의 스팬을 바이트로. 줄 끝의 [`LINE_TAIL`] 은 호출자가 붙인다.
///
/// 스팬 스타일은 **줄 스타일을 `patch` 한 것**이다 — codex `write_history_line`
/// 의 "Merge line-level style into each span" 그대로다. 차분의 시작 상태는
/// 줄 스타일이 아니라 빈 스타일이라(`write_spans` 의 `fg = Color::Reset`),
/// 줄에 색이 있으면 첫 스팬에서 `SetColors` 가 **한 번 더** 나간다. 표 헤더
/// 행의 실측이 그렇다.
pub fn write_spans(line: &Line, out: &mut String) {
    write_spans_for_palette(line, palette::terminal_palette(), out);
}

/// 명시한 터미널 팔레트로 한 줄의 스팬을 바이트화한다.
///
/// production은 [`write_spans`]가 startup probe 캐시를 주입하고, 골든은
/// `Some`/`None`을 직접 넣는다. 환경 변수를 바꾸는 테스트 하네스보다 이 순수
/// 인자가 병렬 테스트에서도 결정적이다.
pub fn write_spans_for_palette(
    line: &Line,
    palette: Option<TerminalPalette>,
    out: &mut String,
) {
    let mut writer = StyleWriter::with_palette(palette);
    for span in &line.spans {
        writer.transition(span.style.patch(line.style), out);
        out.push_str(&span.text);
    }
}

/// 색을 지운 평문 — `NO_COLOR` 경로.
pub fn write_plain(line: &Line, out: &mut String) {
    for span in &line.spans {
        out.push_str(&span.text);
    }
}

#[cfg(test)]
mod tests {
    use super::{write_spans, Color, Line, Span, Style};

    #[test]
    fn plain_spans_emit_no_escapes() {
        let mut out = String::new();
        write_spans(&Line::from_text("hello"), &mut out);
        assert_eq!(out, "hello");
    }

    #[test]
    fn additive_transitions_only_add() {
        let mut out = String::new();
        write_spans(
            &Line::new(vec![
                Span::new("a", Style::new().bold()),
                Span::new("b", Style::new().bold().italic()),
            ]),
            &mut out,
        );
        assert_eq!(out, "\u{1b}[1ma\u{1b}[3mb");
    }

    /// 캡처(`codex-tui-v0.149.1-turn.bin`, 히스토리 19행)의 유저 셀:
    /// `ESC[1mESC[2m› ESC[22mESC[22m<본문>`. bold 와 dim 을 각각 끄므로
    /// `ESC[22m` 이 **두 번** 나가고 `ESC[0m` 은 나가지 않는다 — 색을 건드리지
    /// 않아 앞 스팬의 전경색이 살아 있는 것이 원본의 의도다.
    #[test]
    fn turning_attributes_off_uses_the_captured_per_attribute_codes() {
        let mut out = String::new();
        write_spans(
            &Line::new(vec![
                Span::new("› ", Style::new().bold().dim()),
                Span::new("b", Style::new()),
            ]),
            &mut out,
        );
        assert_eq!(out, "\u{1b}[1m\u{1b}[2m› \u{1b}[22m\u{1b}[22mb");
    }

    /// 캡처(`codex-tui-v0.149.1-boot-trust.bin`, 히스토리 2행)의 지문:
    /// bold → dim 전이가 `ESC[22mESC[2mESC[2m` 을 낸다.
    #[test]
    fn bold_to_dim_emits_the_captured_duplicate_dim() {
        let mut out = String::new();
        write_spans(
            &Line::new(vec![
                Span::new("OpenAI Codex", Style::new().bold()),
                Span::new(" (v0.149.1)", Style::new().dim()),
            ]),
            &mut out,
        );
        assert_eq!(
            out,
            "\u{1b}[1mOpenAI Codex\u{1b}[22m\u{1b}[2m\u{1b}[2m (v0.149.1)"
        );
    }

    /// codex 는 전경·배경을 한 `SetColors` SGR 로 낸다 — 캡처의
    /// `ESC[38;5;5;49mfast` · `ESC[38;5;6;49m/model` · 되돌림 `ESC[39;49m`.
    #[test]
    fn colors_ride_one_setcolors_sgr_with_the_default_background() {
        let mut out = String::new();
        write_spans(
            &Line::new(vec![
                Span::new("x", Style::new().fg(Color::CYAN)),
                Span::new("y", Style::new().fg(Color::Indexed(6))),
                Span::new("z", Style::new().fg(Color::Rgb(246, 226, 183))),
                Span::raw("w"),
            ]),
            &mut out,
        );
        assert_eq!(
            out,
            "\u{1b}[36;49mx\u{1b}[38;5;6;49my\u{1b}[38;2;246;226;183;49mz\u{1b}[39;49mw"
        );
    }

    #[test]
    fn bright_named_colours_use_the_ninety_series_sgr() {
        let mut out = String::new();
        write_spans(
            &Line::new(vec![Span::new("blue", Style::new().fg(Color::LIGHT_BLUE))]),
            &mut out,
        );
        assert_eq!(out, "\u{1b}[94;49mblue");
    }

    /// 캡처(`codex-tui-v0.149.1-boot-trust.bin`, 히스토리 4행) 그대로:
    /// `ESC[38;5;5;49mfast ESC[2m ESC[39;49m` — 속성이 색보다 먼저다.
    #[test]
    fn the_boot_card_model_row_is_the_captured_one() {
        let mut out = String::new();
        write_spans(
            &Line::new(vec![
                Span::new("fast", Style::new().fg(Color::Indexed(5))),
                Span::dim("   "),
                Span::new("/model", Style::new().fg(Color::Indexed(6))),
                Span::dim(" to change"),
            ]),
            &mut out,
        );
        assert_eq!(
            out,
            "\u{1b}[38;5;5;49mfast\u{1b}[2m\u{1b}[39;49m   \u{1b}[22m\u{1b}[38;5;6;49m/model\u{1b}[2m\u{1b}[39;49m to change"
        );
    }

    #[test]
    fn truncation_keeps_display_width_and_marks_the_cut() {
        let line = Line::from_text("abcdefgh").truncated(4);
        assert_eq!(line.plain(), "abc…");
        let wide = Line::from_text("한글한글").truncated(5);
        assert_eq!(wide.plain(), "한글…");
    }
}

#[cfg(test)]
mod background_tests {
    use super::{write_spans_for_palette, Color, Line, Span, Style, StyleWriter};

    /// Adding a background must not move a single byte of what the screen
    /// already draws: every span without one keeps the `;49m` tail the
    /// captures were pinned against.
    #[test]
    fn a_span_without_a_background_writes_the_bytes_it_always_did() {
        let mut out = String::new();
        StyleWriter::set_colors(Some(Color::Rgb(246, 226, 183)), None, &mut out);
        assert_eq!(out, "\u{1b}[38;2;246;226;183;49m");

        let mut out = String::new();
        StyleWriter::set_colors(None, None, &mut out);
        assert_eq!(out, "\u{1b}[39;49m");
    }

    /// A background rides the same SGR as the foreground — one sequence, the
    /// base shifted by ten (30→40, 38→48).
    #[test]
    fn a_background_rides_the_same_sgr_with_the_base_shifted_by_ten() {
        let mut out = String::new();
        StyleWriter::set_colors(Some(Color::GREEN), Some(Color::Rgb(33, 58, 43)), &mut out);
        assert_eq!(out, "\u{1b}[32;48;2;33;58;43m");

        let mut out = String::new();
        StyleWriter::set_colors(None, Some(Color::Base(1)), &mut out);
        assert_eq!(out, "\u{1b}[39;41m");

        let mut out = String::new();
        StyleWriter::set_colors(None, Some(Color::Indexed(22)), &mut out);
        assert_eq!(out, "\u{1b}[39;48;5;22m");
    }

    /// The diff writes runs of same-foreground spans that differ only in
    /// background; a transition that only watched `fg` would drop the tint.
    #[test]
    fn a_background_change_alone_still_emits_a_transition() {
        let mut writer = StyleWriter::new();
        let mut out = String::new();
        writer.transition(Style::new().fg(Color::GREEN), &mut out);
        out.clear();
        writer.transition(
            Style::new().fg(Color::GREEN).bg(Color::Rgb(33, 58, 43)),
            &mut out,
        );
        assert_eq!(out, "\u{1b}[32;48;2;33;58;43m");
    }

    #[test]
    fn a_line_background_is_merged_into_each_span() {
        let line = Line::new(vec![Span::new("x", Style::new().fg(Color::GREEN))])
            .styled(Style::new().bg(Color::Rgb(48, 48, 48)));
        let mut out = String::new();
        write_spans_for_palette(&line, None, &mut out);
        assert_eq!(out, "\u{1b}[32;48;2;48;48;48mx");
    }
}
