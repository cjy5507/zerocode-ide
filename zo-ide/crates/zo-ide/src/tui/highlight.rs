//! 코드 담장의 문법 강조 — codex `tui/src/render/highlight.rs` 이식.
//!
//! codex 는 [syntect] 를 [`two_face`] 의 문법·테마 번들(약 250개 문법, 32개
//! 테마)로 감싸 쓴다(`codex-rs/tui/Cargo.toml`: `syntect = "5"` +
//! `two-face = "0.5"`). 우리도 같은 잠금값을 문다 — two-face 0.5.1 · syntect
//! 5.3.0, codex `Cargo.lock` 그대로.
//!
//! # 테마는 하나로 못 박는다
//!
//! codex 는 터미널 배경을 `OSC 10/11` 로 물어 밝으면 `catppuccin-latte`,
//! 어둡거나 **대답이 없으면** `catppuccin-mocha` 를 고른다
//! (`adaptive_default_theme_selection`). 캡처를 뜨는 PTY 는 그 질의에 답하지
//! 않으므로 실측은 언제나 mocha 쪽이고, [`super::palette`] 의 상수들도 그
//! 테마에서 뽑은 값이다(`TABLE_HEADER` = `rgb(249,226,175)`). 그래서 여기서도
//! mocha 를 고정한다 — 결정적이라 골든 바이트가 성립한다.
//!
//! # 스타일 변환 규칙(원본 주석 그대로)
//!
//! - 전경색만 옮긴다. 배경은 "avoid overwriting terminal bg" 로 버린다.
//! - bold 만 옮긴다. italic 은 "many terminals render it poorly or not at
//!   all", underline 은 "themes like Dracula use underline on type scopes …
//!   which produces distracting underlines" 로 버린다.
//! - 알파 채널은 bat/syntect 의 ANSI 팔레트 약속이다: `a=0` 이면 `r` 이 팔레트
//!   번호, `a=1` 이면 "터미널 기본색"(전경 속성을 아예 안 낸다).
//!
//! # 폴백
//!
//! 언어를 못 알아보거나 입력이 안전 한계를 넘으면 **평문 줄**로 떨어진다.
//! 담장에 언어가 아예 없으면 여기까지 오지도 않는다 — codex
//! `markdown_render.rs::start_codeblock` 이 `code_block_lang` 을 `None` 으로
//! 두고, `text()` 가 버퍼 대신 일반 경로로 흘려보낸다([`super::markdown`]).

use std::sync::OnceLock;

use two_face::re_exports::syntect;

use syntect::easy::HighlightLines;
use syntect::highlighting::{
    Color as SyntectColor, FontStyle, HighlightState, Highlighter, Style as SyntectStyle, Theme,
};
use syntect::parsing::{ParseState, Scope, SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;

use super::ansi::{Color, Line, Span, Style};

/// Syntect work counter used by streaming regression tests.
#[cfg(test)]
pub mod probe {
    use std::cell::Cell;

    thread_local! {
        static CALLS: Cell<usize> = const { Cell::new(0) };
        static BYTES: Cell<usize> = const { Cell::new(0) };
    }

    pub fn reset() {
        CALLS.with(|cell| cell.set(0));
        BYTES.with(|cell| cell.set(0));
    }

    pub(super) fn highlighted(bytes: usize) {
        CALLS.with(|cell| cell.set(cell.get() + 1));
        BYTES.with(|cell| cell.set(cell.get() + bytes));
    }

    #[must_use]
    pub fn read() -> (usize, usize) {
        (CALLS.with(Cell::get), BYTES.with(Cell::get))
    }
}

// -- 프로세스 전역 --------------------------------------------------------

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

/// syntect/bat 은 ANSI 팔레트 의미를 알파에 싣는다: `a=0` 이면 RGB 자리에
/// 팔레트 번호가, `a=1` 이면 "터미널 기본색"이 온다. 그 밖(불투명 `0xFF` 와
/// 몇몇 번들 테마가 쓰는 중간값)은 평범한 RGB 다.
const ANSI_ALPHA_INDEX: u8 = 0x00;
const ANSI_ALPHA_DEFAULT: u8 = 0x01;

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(two_face::theme::EmbeddedThemeName::CatppuccinMocha)
            .clone()
    })
}

// -- 안전 한계(원본 `Guardrail constants`) --------------------------------

/// 512 KB 를 넘는 입력은 강조하지 않는다.
const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
/// 10,000 줄을 넘는 입력은 강조하지 않는다.
const MAX_HIGHLIGHT_LINES: usize = 10_000;
/// 한 줄이 4 KiB 를 넘으면 강조하지 않는다.
pub(crate) const MAX_HIGHLIGHT_LINE_BYTES: usize = 4 * 1024;

// -- 색 변환 --------------------------------------------------------------

/// 낮은 ANSI 팔레트 번호(0~7)는 이름 있는 색으로, 8~255 는 `Indexed` 로.
///
/// 원본 주석: "Named variants are preferred over `Indexed(0)`…`Indexed(7)`
/// because many terminals apply bold/bright treatment differently for named
/// vs indexed colors." 우리 [`Color::Base`] 가 그 이름 있는 자리다(30~37).
fn ansi_palette_color(index: u8) -> Color {
    match index {
        0..=7 => Color::Base(index),
        n => Color::Indexed(n),
    }
}

/// syntect 색을 우리 색으로. "터미널 기본 전경" 이면 `None` — 전경 속성을
/// 아예 내지 않는다.
fn convert_syntect_color(color: SyntectColor) -> Option<Color> {
    match color.a {
        ANSI_ALPHA_INDEX => Some(ansi_palette_color(color.r)),
        ANSI_ALPHA_DEFAULT => None,
        _ => Some(Color::Rgb(color.r, color.g, color.b)),
    }
}

/// syntect 스타일 → 우리 스타일. 전경색과 bold 만 살아남는다.
fn convert_style(syn_style: SyntectStyle) -> Style {
    let mut style = Style::new();
    if let Some(fg) = convert_syntect_color(syn_style.foreground) {
        style = style.fg(fg);
    }
    if syn_style.font_style.contains(FontStyle::BOLD) {
        style = style.bold();
    }
    style
}

/// Query the active syntax theme for the first foreground supplied by these
/// `TextMate` scopes. Table headers and footer accents share this resolver with
/// syntax highlighting instead of freezing one theme's RGB values at call sites.
pub(crate) fn foreground_style_for_scopes(scope_names: &[&str]) -> Option<Style> {
    let highlighter = Highlighter::new(theme());
    scope_names.iter().find_map(|scope_name| {
        let scope = Scope::new(scope_name).ok()?;
        let foreground = highlighter.style_mod_for_stack(&[scope]).foreground?;
        convert_syntect_color(foreground).map(|color| Style::new().fg(color))
    })
}

// -- 문법 찾기 ------------------------------------------------------------

/// 언어 이름표로 syntect 문법을 찾는다 — 원본 `find_syntax` 그대로.
///
/// two-face 의 확장 문법 집합이 대부분의 이름·확장자를 그대로 푼다. 못 푸는
/// 별칭만 손으로 기운다.
fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let ss = syntax_set();

    let normalized = lang.to_ascii_lowercase();
    let patched = match normalized.as_str() {
        "csharp" | "c-sharp" => "c#",
        // CUDA 소스(.cu)·헤더(.cuh)와 C++20 모듈 확장자들은 C++ 강조로.
        "cu" | "cuh" | "cppm" | "cxxm" | "ixx" => "cpp",
        "golang" => "go",
        "python3" => "python",
        "shell" => "bash",
        _ => lang,
    };

    // 토큰으로(확장자 대소문자 무시).
    if let Some(syntax) = ss.find_syntax_by_token(patched) {
        return Some(syntax);
    }
    // 문법 이름 그대로(`Rust`·`Python`).
    if let Some(syntax) = ss.find_syntax_by_name(patched) {
        return Some(syntax);
    }
    // 대소문자 무시 이름 (`rust` → `Rust`).
    let lower = patched.to_ascii_lowercase();
    if let Some(syntax) = ss
        .syntaxes()
        .iter()
        .find(|syntax| syntax.name.to_ascii_lowercase() == lower)
    {
        return Some(syntax);
    }
    // 마지막으로 원문을 확장자로.
    ss.find_syntax_by_extension(lang)
}

// -- 강조 -----------------------------------------------------------------

/// 강조된 한 줄을 스팬으로 — 원본 `highlighted_line_spans`.
///
/// 줄바꿈은 스팬이 아니라 줄이 나타내므로 꼬리에서 뗀다. 다 떼고 남는 게
/// 없으면 빈 스팬 하나를 둔다.
fn highlighted_line_spans(ranges: Vec<(SyntectStyle, &str)>) -> Vec<Span> {
    let mut spans = Vec::new();
    for (style, text) in ranges {
        let text = text.trim_end_matches(['\n', '\r']);
        if !text.is_empty() {
            spans.push(Span::new(text.to_string(), convert_style(style)));
        }
    }
    if spans.is_empty() {
        spans.push(Span::raw(String::new()));
    }
    spans
}

/// 줄별 스팬. 언어를 못 알아보거나 한계를 넘으면 `None` — 원본
/// `highlight_to_line_spans_with_theme`.
fn highlight_to_line_spans(code: &str, lang: &str) -> Option<Vec<Vec<Span>>> {
    // 빈 입력은 강조할 것이 없다. 평문 경로가 빈 줄 하나를 옳게 만든다.
    if code.is_empty() {
        return None;
    }

    // 줄 수는 개행 바이트가 아니라 **실제 줄**을 센다 — 끝이 개행이 아닐 때의
    // off-by-one 을 피한다.
    if code.len() > MAX_HIGHLIGHT_BYTES
        || code.lines().count() > MAX_HIGHLIGHT_LINES
        || code
            .lines()
            .any(|line| line.len() > MAX_HIGHLIGHT_LINE_BYTES)
    {
        return None;
    }

    let syntax = find_syntax(lang)?;
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut lines: Vec<Vec<Span>> = Vec::new();

    for line in LinesWithEndings::from(code) {
        #[cfg(test)]
        probe::highlighted(line.len());
        let ranges = highlighter.highlight_line(line, syntax_set()).ok()?;
        lines.push(highlighted_line_spans(ranges));
    }

    Some(lines)
}

/// Whether a complete diff is too large to syntax-highlight without making a
/// redraw visibly expensive.  Diff rendering calls this once for the whole
/// patch, before it starts a parser for each visually separate hunk.
#[must_use]
pub(crate) fn exceeds_highlight_limits(total_bytes: usize, total_lines: usize) -> bool {
    total_bytes > MAX_HIGHLIGHT_BYTES || total_lines > MAX_HIGHLIGHT_LINES
}

/// Highlight a complete logical block, preserving syntect parser state between
/// its lines.  Diff hunks use this instead of [`highlight_code_to_lines`]'s
/// public fallback so an unavailable language can stay distinguishable from a
/// successfully highlighted line.
pub(crate) fn highlight_block_to_lines(code: &str, lang: &str) -> Option<Vec<Line>> {
    highlight_to_line_spans(code, lang).map(|lines| lines.into_iter().map(Line::new).collect())
}

/// Append-only syntax highlighting for complete lines in an open code fence.
#[derive(Debug)]
pub(crate) struct StreamingCodeHighlighter {
    state: Option<HighlightedCode>,
}

#[derive(Debug)]
struct HighlightedCode {
    bytes: usize,
    lines: usize,
    syntax: (HighlightState, ParseState),
}

impl StreamingCodeHighlighter {
    /// Rebuild Syntect's parser state after the canonical renderer emitted
    /// `code`. Unknown languages and oversized input permanently use plain
    /// text until the fence closes.
    pub(crate) fn new(code: &str, lang: &str) -> Option<Self> {
        let lines = code.lines().count();
        let syntax = find_syntax(lang).filter(|_| {
            !exceeds_highlight_limits(code.len(), lines)
                && !code
                    .lines()
                    .any(|line| line.len() > MAX_HIGHLIGHT_LINE_BYTES)
        });
        let Some(syntax) = syntax else {
            return Some(Self { state: None });
        };
        let mut highlighter = HighlightLines::new(syntax, theme());
        for line in LinesWithEndings::from(code) {
            #[cfg(test)]
            probe::highlighted(line.len());
            highlighter.highlight_line(line, syntax_set()).ok()?;
        }
        Some(Self {
            state: Some(HighlightedCode {
                bytes: code.len(),
                lines,
                syntax: highlighter.state(),
            }),
        })
    }

    /// Consume the retained state and highlight only newly committed lines.
    pub(crate) fn append(mut self, appended: &str) -> Option<(Self, Vec<Line>)> {
        if !appended.ends_with('\n') {
            return None;
        }
        let Some(mut state) = self.state.take() else {
            let lines = appended.lines().map(Line::from_text).collect();
            return Some((self, lines));
        };
        let bytes = state.bytes.checked_add(appended.len())?;
        let lines = state.lines.checked_add(appended.lines().count())?;
        if exceeds_highlight_limits(bytes, lines)
            || appended
                .lines()
                .any(|line| line.len() > MAX_HIGHLIGHT_LINE_BYTES)
        {
            return None;
        }
        let (highlight_state, parse_state) = state.syntax;
        let mut highlighter =
            HighlightLines::from_state(theme(), highlight_state, parse_state);
        let mut rendered = Vec::with_capacity(lines - state.lines);
        for line in LinesWithEndings::from(appended) {
            #[cfg(test)]
            probe::highlighted(line.len());
            let ranges = highlighter.highlight_line(line, syntax_set()).ok()?;
            rendered.push(Line::new(highlighted_line_spans(ranges)));
        }
        state.bytes = bytes;
        state.lines = lines;
        state.syntax = highlighter.state();
        self.state = Some(state);
        Some((self, rendered))
    }
}

/// 코드를 강조해 줄로 돌려준다. 언어를 모르거나 한계를 넘으면 **평문** 줄이다
/// — 부르는 쪽은 언제나 결과를 그대로 그리면 된다.
#[must_use]
pub fn highlight_code_to_lines(code: &str, lang: &str) -> Vec<Line> {
    if let Some(lines) = highlight_block_to_lines(code, lang) {
        lines
    } else {
        // 폴백: 원문 한 줄에 우리 줄 하나. `split('\n')` 이 아니라 `lines()` 다
        // — pulldown-cmark 가 주는 담장 본문은 개행으로 끝나므로 `split` 이면
        // 유령 빈 줄이 하나 더 붙는다.
        let mut result: Vec<Line> = code.lines().map(Line::from_text).collect();
        if result.is_empty() {
            result.push(Line::from_text(String::new()));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::{foreground_style_for_scopes, highlight_code_to_lines, Color};

    fn plain(lines: &[super::Line]) -> Vec<String> {
        lines.iter().map(super::Line::plain).collect()
    }

    /// 아는 언어는 색이 붙는다 — 그리고 원문은 한 글자도 잃지 않는다.
    #[test]
    fn a_known_language_gains_colour_without_losing_text() {
        let lines = highlight_code_to_lines("fn main() {}\n", "rust");
        assert_eq!(plain(&lines), vec!["fn main() {}"]);
        assert!(
            lines[0].spans.iter().any(|span| span.style.fg.is_some()),
            "rust should have coloured spans: {:?}",
            lines[0].spans
        );
    }

    /// mocha 는 RGB 테마다 — 팔레트 번호가 아니라 트루컬러가 나가야 한다.
    #[test]
    fn the_theme_is_true_colour() {
        let lines = highlight_code_to_lines("fn main() {}\n", "rust");
        assert!(
            lines[0]
                .spans
                .iter()
                .filter_map(|span| span.style.fg)
                .all(|color| matches!(color, Color::Rgb(..))),
            "catppuccin-mocha is an RGB theme: {:?}",
            lines[0].spans
        );
    }

    #[test]
    fn the_active_theme_exposes_table_model_and_path_foregrounds() {
        assert_eq!(
            foreground_style_for_scopes(&["entity.name.type", "support.type", "variable"])
                .and_then(|style| style.fg),
            Some(Color::Rgb(249, 226, 175))
        );
        assert_eq!(
            foreground_style_for_scopes(&["string", "markup.underline.link"])
                .and_then(|style| style.fg),
            Some(Color::Rgb(166, 227, 161))
        );
    }

    /// 모르는 언어는 평문으로 떨어진다 — 색 없는 스팬 하나씩.
    #[test]
    fn an_unknown_language_falls_back_to_plain_lines() {
        let lines = highlight_code_to_lines("one\ntwo\n", "totally-not-a-language");
        assert_eq!(plain(&lines), vec!["one", "two"]);
        for line in &lines {
            assert!(line.spans.iter().all(|span| span.style == super::Style::new()));
        }
    }

    /// 폴백은 `lines()` 를 쓴다 — 꼬리 개행이 유령 빈 줄을 만들지 않는다.
    #[test]
    fn the_fallback_drops_the_phantom_trailing_line() {
        assert_eq!(plain(&highlight_code_to_lines("only\n", "nope")).len(), 1);
    }

    /// 빈 입력은 빈 줄 하나다.
    #[test]
    fn empty_code_is_one_empty_line() {
        assert_eq!(plain(&highlight_code_to_lines("", "rust")), vec![""]);
    }

    /// 별칭 표 — 원본이 손으로 기운 것들.
    #[test]
    fn patched_aliases_resolve() {
        for lang in ["golang", "python3", "shell", "csharp", "c-sharp", "cu", "ixx"] {
            let lines = highlight_code_to_lines("x\n", lang);
            assert!(
                lines[0].spans.iter().any(|span| span.style.fg.is_some()),
                "{lang} should resolve to a syntax"
            );
        }
    }

    /// 한계를 넘는 줄은 강조를 건너뛴다 — 4 KiB 초과 한 줄.
    #[test]
    fn an_overlong_line_skips_highlighting() {
        let long = "a".repeat(super::MAX_HIGHLIGHT_LINE_BYTES + 1);
        let code = format!("{long}\n");
        let lines = highlight_code_to_lines(&code, "rust");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].spans.iter().all(|span| span.style.fg.is_none()));
    }

    /// 강조된 줄에서도 개행은 스팬에 남지 않는다.
    #[test]
    fn newlines_never_reach_the_spans() {
        let lines = highlight_code_to_lines("let a = 1;\nlet b = 2;\n", "rust");
        assert_eq!(lines.len(), 2);
        for line in &lines {
            for span in &line.spans {
                assert!(!span.text.contains('\n') && !span.text.contains('\r'));
            }
        }
    }

    /// 빈 줄은 빈 스팬 하나를 지킨다(`highlighted_line_spans` 의 마지막 분기).
    #[test]
    fn a_blank_source_line_keeps_one_empty_span() {
        let lines = highlight_code_to_lines("let a = 1;\n\nlet b = 2;\n", "rust");
        assert_eq!(plain(&lines), vec!["let a = 1;", "", "let b = 2;"]);
        assert_eq!(lines[1].spans.len(), 1);
        assert!(lines[1].spans[0].text.is_empty());
    }
}
