//! 마크다운 → 스타일 있는 줄. 규칙은 codex `tui/src/markdown_render.rs` 실측.
//!
//! 핵심은 codex 가 "해시를 지우지 않는다"는 것이다: `### 제목` 은 `### ` 를
//! 그대로 남기고 헤딩 스타일(h3 = bold+italic)을 입힌다. 캡처의
//! `ESC[1mESC[3m### 자기소개` 가 그 증거다.
//!
//! 표도 원본 그대로다 — `Options::ENABLE_TABLES` 를 켜고 열 폭 배분·정렬·칸
//! 접기는 [`super::tables`] 가 맡는다(codex `start_table`/`end_table`).
//!
//! 들여쓰기 규칙도 원본 그대로다 — 깊이 `d` 의 리스트는 `width = 4d - 3`,
//! 불릿 마커는 `" "×(width-1) + "- "`, 번호 마커는 `"{n:width$}. "`, 이어지는
//! 줄은 불릿 `width + 1` · 번호 `width + 2` 칸을 들여쓴다. 바깥 리스트의
//! 접두어는 **쌓이지 않는다**(가장 안쪽 것만 쓴다 — `prefix_spans`).

use std::ops::Range;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};

use super::ansi::{Color, Line, Span, Style};
use super::highlight::{
    MAX_HIGHLIGHT_LINE_BYTES, StreamingCodeHighlighter, highlight_code_to_lines,
};
use super::holdback::{FenceKind, FenceTracker};
use super::tables;

/// 렌더 일감 계수기 — **테스트 빌드에만** 있다.
///
/// 스트리밍이 정말 증분인지는 벤치 없이도 잴 수 있다: "누적 원문을 몇 번,
/// 몇 바이트나 다시 파싱했는가". 이 계수기가 그 자리를 대신한다
/// ([`super::cells`] 의 측정 테스트가 전/후를 나란히 놓는다).
#[cfg(test)]
pub mod probe {
    use std::cell::Cell;

    thread_local! {
        static CALLS: Cell<usize> = const { Cell::new(0) };
        static BYTES: Cell<usize> = const { Cell::new(0) };
        static WRAPPED: Cell<usize> = const { Cell::new(0) };
    }

    /// 계수기를 0 으로.
    pub fn reset() {
        CALLS.with(|cell| cell.set(0));
        BYTES.with(|cell| cell.set(0));
        WRAPPED.with(|cell| cell.set(0));
    }

    /// 파서에 원문 한 조각이 들어갔다.
    pub(super) fn parsed(bytes: usize) {
        CALLS.with(|cell| cell.set(cell.get() + 1));
        BYTES.with(|cell| cell.set(cell.get() + bytes));
    }

    /// 접두어·접기를 다시 탄 마크다운 줄.
    pub fn wrapped(lines: usize) {
        WRAPPED.with(|cell| cell.set(cell.get() + lines));
    }

    /// (파서 호출 수, 파싱한 원문 바이트 합, 다시 접은 줄 합).
    #[must_use]
    pub fn read() -> (usize, usize, usize) {
        (
            CALLS.with(Cell::get),
            BYTES.with(Cell::get),
            WRAPPED.with(Cell::get),
        )
    }
}

/// 헤딩 수준별 스타일 — codex `MarkdownStyles::default`.
fn heading_style(level: HeadingLevel) -> Style {
    match level {
        HeadingLevel::H1 => Style::new().bold().underline(),
        HeadingLevel::H2 => Style::new().bold(),
        HeadingLevel::H3 => Style::new().bold().italic(),
        HeadingLevel::H4 | HeadingLevel::H5 | HeadingLevel::H6 => Style::new().italic(),
    }
}

const CODE_STYLE: Style = Style {
    fg: Some(Color::CYAN),
    bg: None,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strike: false,
};
const LINK_STYLE: Style = Style {
    fg: Some(Color::CYAN),
    bg: None,
    bold: false,
    dim: false,
    italic: false,
    underline: true,
    strike: false,
};
const BLOCKQUOTE_STYLE: Style = Style {
    fg: Some(Color::GREEN),
    bg: None,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strike: false,
};
/// 번호 마커 색 — codex의 `.light_blue()`와 같은 SGR 94.
const ORDERED_MARKER_STYLE: Style = Style {
    fg: Some(Color::LIGHT_BLUE),
    bg: None,
    bold: false,
    dim: false,
    italic: false,
    underline: false,
    strike: false,
};

/// 담장의 언어 이름표 — codex `start_codeblock` 의 정규화 그대로.
///
/// `CommonMark` 의 info string 은 언어 뒤에 메타데이터를 달 수 있다
/// (`rust,no_run`·`rust title=demo`). 문법 조회가 성공하도록 **첫 토큰만**
/// 남기고, 비어 있으면 "언어 없음"이다. 들여쓰기 담장에는 이름표가 없다.
fn fence_language(kind: &CodeBlockKind<'_>) -> Option<String> {
    let CodeBlockKind::Fenced(info) = kind else {
        return None;
    };
    info.split([',', ' ', '\t'])
        .next()
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

/// 한 겹의 들여쓰기 — 이어지는 줄의 접두어와, 첫 줄에만 쓰는 마커.
#[derive(Debug, Clone)]
struct Indent {
    prefix: Vec<Span>,
    marker: Option<Vec<Span>>,
    is_list: bool,
}

/// The indent spans of a stack, as one span — the hanging indent a wrapped
/// row starts with. Spaces take the style of whatever styled prefix they sit
/// beside (a quote's `> `), so the indent paints as one piece; nothing when
/// the stack has no prefix at all.
fn joined_span(spans: &[Span]) -> Option<Span> {
    let text: String = spans.iter().map(|span| span.text.as_str()).collect();
    if text.is_empty() {
        return None;
    }
    let style = spans
        .iter()
        .find(|span| !span.text.trim().is_empty())
        .map_or(Style::new(), |span| span.style);
    Some(Span::new(text, style))
}

/// 마크다운 원문을 스타일 있는 줄로 옮긴다. 표 말고는 줄바꿈을 하지 않는다 —
/// 폭을 아는 쪽(셀 조립부)이 [`super::wrap`] 으로 접는다.
#[must_use]
pub fn render(source: &str) -> Vec<Line> {
    render_wrapped(source, None)
}

/// 폭을 아는 렌더. `width` 는 **내용 폭**(셀 접두어를 뺀 것)이고 codex
/// `render_markdown_text_with_width` 의 `width` 와 같은 자리다 — codex 도
/// 답변 스트림에 `current_stream_width(/*reserved_cols*/ 2)` 를 준다
/// (`chatwidget.rs::on_terminal_resize`).
///
/// 이 값이 필요한 것은 **표**뿐이다: 열 폭 배분과 칸 접기가 폭을 봐야 한다
/// (codex `available_table_width`). 그래서 표가 낸 줄은 이미 접힌 줄이고
/// [`super::cells::prefixed`] 의 접기를 다시 타도 그대로 통과한다. `None` 이면
/// 열은 자연 폭을 쓴다(원본의 `render_markdown_text` 와 같다).
#[must_use]
pub fn render_wrapped(source: &str, width: Option<usize>) -> Vec<Line> {
    write_lines(source, width, true)
}

/// [`render_wrapped`] 과 같은 렌더이되 **꼬리 빈 줄을 남긴다**.
///
/// 증분 렌더가 원문을 조각으로 나눠 이어붙일 때 쓴다: 꼬리 다듬기
/// (`Writer::finish` 의 마지막 `while`)는 문서 **전체의 끝**에서 한 번만
/// 일어나야 하므로, 뒤에 조각이 더 붙는 앞 조각은 자기 끝을 다듬으면 안 된다.
/// 경계는 [`Restarts`] 가 고른다.
#[must_use]
pub fn render_segment(source: &str, width: Option<usize>) -> Vec<Line> {
    write_lines(source, width, false)
}

/// A conservative fast path for one open, top-level, language-tagged fence.
#[derive(Debug)]
pub struct OpenCodeFence {
    marker: u8,
    marker_len: usize,
    language: String,
    source_start: usize,
    content_start: usize,
    source_len: usize,
    /// Initialized only when another chunk arrives, avoiding duplicate work
    /// for a one-shot fence.
    highlighter: Option<StreamingCodeHighlighter>,
}

impl OpenCodeFence {
    /// Find the final top-level fenced block, then apply the conservative
    /// source-preserving detector to that suffix.
    #[must_use]
    pub fn detect_final(source: &str, source_len: usize) -> Option<Self> {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_STRIKETHROUGH);
        options.insert(Options::ENABLE_TABLES);
        let mut depth = 0usize;
        let mut final_fence_start = None;
        for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
            match event {
                Event::Start(tag) => {
                    if depth == 0
                        && matches!(tag, Tag::CodeBlock(CodeBlockKind::Fenced(_)))
                    {
                        final_fence_start = Some(range.start);
                    }
                    depth = depth.saturating_add(1);
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let start = final_fence_start?;
        Self::detect(&source[start..], source_len)
    }

    /// Recognize an append-only fence after its canonical render.
    #[must_use]
    pub fn detect(source: &str, source_len: usize) -> Option<Self> {
        let marker = *source.as_bytes().first()?;
        if marker != b'`' && marker != b'~' {
            return None;
        }
        let (opening, code) = source.split_once('\n')?;
        let marker_len = opening.bytes().take_while(|byte| *byte == marker).count();
        if marker_len < 3
            || !source.ends_with('\n')
            || source.contains(['\r', '\0'])
        {
            return None;
        }
        let info = opening[marker_len..].trim_matches([' ', '\t', '\u{b}', '\u{c}']);
        if info.contains(['&', '\\']) || (marker == b'`' && info.contains('`')) {
            return None;
        }
        let language = info
            .split([',', ' ', '\t'])
            .next()
            .filter(|language| !language.is_empty())?;
        if language.len() > MAX_HIGHLIGHT_LINE_BYTES
            || has_possible_closing_line(code, marker, marker_len)
        {
            return None;
        }
        Some(Self {
            marker,
            marker_len,
            language: language.to_string(),
            source_start: source_len.checked_sub(source.len())?,
            content_start: source_len.checked_sub(code.len())?,
            source_len,
            highlighter: None,
        })
    }

    /// Append only while `committed_source` extends the retained fence.
    #[must_use]
    pub fn append(
        mut self,
        raw_source: &str,
        committed_source: &str,
    ) -> Option<(Self, Vec<Line>)> {
        if self.source_len.checked_add(committed_source.len()) != Some(raw_source.len())
            || !raw_source.ends_with(committed_source)
            || committed_source.contains(['\r', '\0'])
            || has_possible_closing_line(committed_source, self.marker, self.marker_len)
        {
            return None;
        }
        let highlighter = match self.highlighter.take() {
            Some(highlighter) => highlighter,
            None => StreamingCodeHighlighter::new(
                &raw_source[self.content_start..self.source_len],
                &self.language,
            )?,
        };
        let (highlighter, lines) = highlighter.append(committed_source)?;
        self.highlighter = Some(highlighter);
        self.source_len = raw_source.len();
        Some((self, lines))
    }

    /// Blank code lines withheld by the canonical renderer at the mutable
    /// document tail.
    #[must_use]
    pub fn trailing_blank_lines(&self, raw_source: &str) -> usize {
        raw_source[self.content_start..self.source_len]
            .lines()
            .rev()
            .take_while(|line| line.trim().is_empty())
            .count()
    }

    #[must_use]
    pub fn starts_after(&self, source_offset: usize) -> bool {
        self.source_start > source_offset
    }

    #[must_use]
    pub fn has_visible_content(&self, raw_source: &str) -> bool {
        raw_source[self.content_start..self.source_len]
            .lines()
            .any(|line| !line.trim().is_empty())
    }
}

fn has_possible_closing_line(source: &str, marker: u8, marker_len: usize) -> bool {
    source.lines().any(|line| {
        line.trim_start_matches([' ', '\t'])
            .bytes()
            .take_while(|byte| *byte == marker)
            .count()
            >= marker_len
    })
}

fn write_lines(source: &str, width: Option<usize>, trim_tail: bool) -> Vec<Line> {
    #[cfg(test)]
    probe::parsed(source.len());
    // codex `markdown_render.rs` 가 켜는 것 그대로다(`ENABLE_STRIKETHROUGH` +
    // `ENABLE_TABLES`). `ENABLE_TASKLISTS` 는 원본에 없으므로 뺐다 — 체크박스는
    // 평문 `[x] ` 로 흐른다.
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let mut writer = Writer {
        source,
        wrap_width: width,
        ..Writer::default()
    };
    // 표 본문 행이 파이프 문법으로 왔는지는 **원문 범위**로만 알 수 있다
    // (codex `has_table_row_boundary_pipe`) — 그래서 오프셋 이터레이터다.
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        writer.event(&event, range);
    }
    writer.finish(trim_tail)
}

#[derive(Debug, Default)]
struct Writer<'a> {
    /// 원문 — 표 행의 파이프 문법 판정이 범위로 되읽는다.
    source: &'a str,
    /// 내용 폭. 표의 열 배분만 쓴다(`None` 이면 자연 폭).
    wrap_width: Option<usize>,
    lines: Vec<Line>,
    current: Option<Vec<Span>>,
    /// The hanging indent of `current` — the indent stack's continuation
    /// prefix as it stood when the line began. Taken then, not at flush:
    /// `TagEnd::Item` pops the indent before the item's last line is flushed.
    current_continuation: Option<Span>,
    /// 지금 줄에 이미 접두어가 붙었는지 — 두 번 붙이지 않기 위한 표식.
    inline: Vec<Style>,
    indent: Vec<Indent>,
    list_indices: Vec<Option<u64>>,
    needs_newline: bool,
    pending_marker: bool,
    in_code_block: bool,
    /// 열려 있는 담장의 언어 — codex `Writer::code_block_lang`. `Some` 이면
    /// 본문을 [`Self::code_block_buffer`] 에 모아 두었다가 담장이 닫힐 때
    /// 한꺼번에 문법 강조한다.
    code_block_lang: Option<String>,
    /// 강조를 기다리는 담장 본문 — codex `Writer::code_block_buffer`.
    code_block_buffer: String,
    /// 열려 있는 표. codex `Writer::table_state`.
    table: Option<tables::State>,
}

impl Writer<'_> {
    fn event(&mut self, event: &Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.start(tag, range),
            Event::End(tag) => self.end(*tag),
            Event::Text(text) => self.text(text),
            Event::Code(code) => {
                let style = self.style().patch(CODE_STYLE);
                self.push_span(Span::new(code.to_string(), style));
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(cell) = self.table.as_mut().and_then(tables::State::cell_mut) {
                    cell.hard_break();
                    return;
                }
                self.push_line(Vec::new());
            }
            Event::Rule => {
                if self.needs_newline {
                    self.push_blank();
                }
                self.push_line(vec![Span::dim("─".repeat(12))]);
                self.needs_newline = true;
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                self.push_span(Span::new(html.to_string(), self.style()));
            }
            Event::TaskListMarker(_)
            | Event::FootnoteReference(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }

    fn start(&mut self, tag: &Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Table(alignments) => self.start_table(alignments.clone()),
            Tag::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.start_head();
                }
            }
            Tag::TableRow => {
                let has_pipe_syntax = self.row_has_boundary_pipe(range);
                if let Some(table) = self.table.as_mut() {
                    table.start_row(has_pipe_syntax);
                }
            }
            Tag::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.start_cell();
                }
            }
            Tag::Paragraph => {
                if self.needs_newline {
                    self.push_blank();
                }
                self.push_line(Vec::new());
                self.needs_newline = false;
            }
            Tag::Heading { level, .. } => {
                if self.needs_newline {
                    self.push_blank();
                    self.needs_newline = false;
                }
                let style = heading_style(*level);
                let hashes = format!("{} ", "#".repeat(*level as usize));
                self.push_line(vec![Span::new(hashes, style)]);
                self.inline.push(style);
            }
            Tag::BlockQuote(_) => {
                if self.needs_newline {
                    self.push_blank();
                    self.needs_newline = false;
                }
                self.indent.push(Indent {
                    prefix: vec![Span::new("> ", BLOCKQUOTE_STYLE)],
                    marker: None,
                    is_list: false,
                });
                self.inline.push(BLOCKQUOTE_STYLE);
            }
            // codex `start_codeblock` 은 언어 이름을 **찍지 않는다** — 언어
            // 이름표가 있으면 본문을 모아 두었다가 `end_codeblock` 에서
            // 문법 강조해 내고, 언어 없는 담장이면 평문 줄로 흘린다.
            //
            // 들여쓰기 담장은 네 칸을 물려받는다(codex `start_tag` 의
            // `CodeBlockKind::Indented => Some(Span::from(" ".repeat(4)))`).
            // 울타리 담장은 빈 겹이다 — 원본은 빈 스팬을 하나 밀지만 우리
            // 방출기는 빈 스팬에도 SGR 전이를 내므로 스팬 없이 둔다.
            Tag::CodeBlock(kind) => {
                self.flush();
                if !self.lines.is_empty() {
                    self.push_blank();
                }
                self.in_code_block = true;
                self.code_block_lang = fence_language(kind);
                self.code_block_buffer.clear();
                self.indent.push(Indent {
                    prefix: match kind {
                        CodeBlockKind::Fenced(_) => Vec::new(),
                        CodeBlockKind::Indented => vec![Span::raw("    ")],
                    },
                    marker: None,
                    is_list: false,
                });
                self.needs_newline = true;
            }
            Tag::List(first) => {
                if self.list_indices.is_empty() && self.needs_newline {
                    self.push_blank();
                    self.needs_newline = false;
                }
                self.list_indices.push(*first);
            }
            Tag::Item => self.start_item(),
            Tag::Emphasis => self.inline.push(Style::new().italic()),
            Tag::Strong => self.inline.push(Style::new().bold()),
            Tag::Strikethrough => self.inline.push(Style::new().strike()),
            Tag::Link { .. } => self.inline.push(LINK_STYLE),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Table => self.end_table(),
            TagEnd::TableHead => {
                if let Some(table) = self.table.as_mut() {
                    table.end_head();
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = self.table.as_mut() {
                    table.end_row();
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = self.table.as_mut() {
                    table.end_cell();
                }
            }
            TagEnd::Paragraph => self.needs_newline = true,
            TagEnd::Heading(_) => {
                self.inline.pop();
                self.needs_newline = true;
            }
            TagEnd::BlockQuote(_) => {
                self.inline.pop();
                self.indent.pop();
                self.needs_newline = true;
            }
            TagEnd::CodeBlock => self.end_codeblock(),
            TagEnd::List(_) => {
                self.list_indices.pop();
                self.needs_newline = true;
            }
            TagEnd::Item => {
                self.indent.pop();
                self.pending_marker = false;
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Link => {
                self.inline.pop();
            }
            _ => {}
        }
    }

    fn start_item(&mut self) {
        self.flush();
        self.pending_marker = true;
        let depth = self.list_indices.len();
        let width = depth.saturating_mul(4).saturating_sub(3).max(1);
        let ordered = self.list_indices.last().copied().flatten().is_some();
        let marker = match self.list_indices.last_mut() {
            Some(Some(index)) => {
                let shown = *index;
                *index += 1;
                Some(vec![Span::new(
                    format!("{shown:width$}. "),
                    ORDERED_MARKER_STYLE,
                )])
            }
            Some(None) => Some(vec![Span::raw(format!(
                "{}- ",
                " ".repeat(width.saturating_sub(1))
            ))]),
            None => None,
        };
        let indent_len = if ordered { width + 2 } else { width + 1 };
        self.indent.push(Indent {
            prefix: vec![Span::raw(" ".repeat(indent_len))],
            marker,
            is_list: true,
        });
        self.needs_newline = false;
    }

    // ------------------------------------------------------------------
    // 표 — codex `start_table`/`end_table`
    // ------------------------------------------------------------------

    fn start_table(&mut self, alignments: Vec<pulldown_cmark::Alignment>) {
        self.flush();
        if self.needs_newline {
            self.push_blank();
            self.needs_newline = false;
        }
        self.table = Some(tables::State::new(alignments));
    }

    /// 표를 닫고 줄을 낸다. **이미 접힌 줄**(격자·레코드)은 첫 줄에만 리스트
    /// 마커를 주고 접두어를 그대로 붙이고(codex `push_prewrapped_line`),
    /// 파이프 폴백 줄은 보통 줄처럼 흘려 뒤에서 접히게 둔다.
    fn end_table(&mut self) {
        let Some(state) = self.table.take() else {
            return;
        };
        let column_count = state.column_count();
        let prefix_width = Self::spans_width(&self.prefix(self.pending_marker));
        let table_width = self
            .wrap_width
            .map(|width| tables::content_budget(width, prefix_width, column_count));
        let record_width = self
            .wrap_width
            .map(|width| width.saturating_sub(prefix_width));
        let rendered = tables::render(state, table_width, record_width);

        let mut marker_line = self.pending_marker;
        for line in rendered.lines {
            if rendered.prewrapped {
                self.pending_marker = marker_line;
                let mut spans = self.prefix(marker_line);
                let style = line.style;
                spans.extend(line.spans);
                self.lines.push(Line::new(spans).styled(style));
                self.pending_marker = false;
            } else {
                self.push_line(line.spans);
                self.flush();
            }
            marker_line = false;
        }
        self.pending_marker = false;
        for line in rendered.spillover {
            self.push_line(line.spans);
            self.flush();
        }
        self.needs_newline = true;
    }

    /// 이 행이 파이프로 시작하거나 끝났는지 — codex
    /// `has_table_row_boundary_pipe`. 원문 범위를 되읽는 것이 유일한 근거다.
    fn row_has_boundary_pipe(&self, range: Range<usize>) -> bool {
        let Some(source) = self.source.get(range) else {
            return false;
        };
        let source = source.trim();
        source.starts_with('|') || source.ends_with('|')
    }

    fn spans_width(spans: &[Span]) -> usize {
        spans.iter().map(Span::width).sum()
    }

    fn text(&mut self, text: &str) {
        if self.table.is_some() {
            let style = self.style();
            if let Some(cell) = self.table.as_mut().and_then(tables::State::cell_mut) {
                cell.push_span(Span::new(text.to_string(), style));
            }
            return;
        }
        if self.in_code_block {
            // 언어 이름표가 있으면 본문을 그대로 모은다 — codex `text()`:
            // "Append verbatim — pulldown-cmark text events already contain
            // the original line breaks, so inserting separators would double
            // them." 강조는 담장이 닫힐 때 한 번에 한다.
            if self.code_block_lang.is_some() {
                self.code_block_buffer.push_str(text);
                return;
            }
            // 언어 없는 담장의 평문 경로. 담장 본문은 텍스트 이벤트 여럿으로
            // 쪼개져 오므로, 두 번째 이벤트부터는 앞줄을 먼저 닫는다.
            if !self.needs_newline && self.code_line_has_content() {
                self.push_line(Vec::new());
            }
            let style = self.style();
            // `split('\n')` 이 아니라 `lines()` 다 — 꼬리 개행이 유령 빈
            // 조각을 만들지 않는다(codex `highlight_code_to_lines` 의 같은
            // 주석: "avoid a phantom trailing empty element").
            for (index, segment) in text.lines().enumerate() {
                if self.needs_newline {
                    self.push_line(Vec::new());
                    self.needs_newline = false;
                }
                if index > 0 {
                    self.push_line(Vec::new());
                }
                if !segment.is_empty() {
                    self.push_span(Span::new(segment.to_string(), style));
                }
            }
            self.needs_newline = false;
            return;
        }
        let style = self.style();
        self.push_span(Span::new(text.to_string(), style));
    }

    fn style(&self) -> Style {
        self.inline
            .iter()
            .fold(Style::new(), |acc, style| acc.patch(*style))
    }

    /// 지금 스택으로 만든 줄 접두어. 리스트는 가장 안쪽 것만 쓴다.
    fn prefix(&self, marker_line: bool) -> Vec<Span> {
        let last_marker = if marker_line {
            self.indent
                .iter()
                .rposition(|indent| indent.marker.is_some())
        } else {
            None
        };
        let last_list = self.indent.iter().rposition(|indent| indent.is_list);
        let mut out = Vec::new();
        for (index, indent) in self.indent.iter().enumerate() {
            if marker_line {
                if Some(index) == last_marker {
                    if let Some(marker) = &indent.marker {
                        out.extend(marker.iter().cloned());
                        continue;
                    }
                }
                if indent.is_list && last_marker.is_some_and(|marker| marker > index) {
                    continue;
                }
            } else if indent.is_list && Some(index) != last_list {
                continue;
            }
            out.extend(indent.prefix.iter().cloned());
        }
        out
    }

    /// 담장이 닫힌다 — 모아 둔 본문을 강조해 줄로 낸다.
    ///
    /// codex `markdown_render.rs::end_codeblock` 그대로: 언어 이름표가 있었던
    /// 담장만 여기로 온다. 강조기가 언어를 못 알아보면
    /// [`highlight_code_to_lines`] 가 스스로 평문 줄로 떨어지므로 이 자리에
    /// 분기가 없다.
    fn end_codeblock(&mut self) {
        if let Some(lang) = self.code_block_lang.take() {
            let code = std::mem::take(&mut self.code_block_buffer);
            if !code.is_empty() {
                for line in highlight_code_to_lines(&code, &lang) {
                    self.push_line(line.spans);
                }
            }
        }
        self.flush();
        self.indent.pop();
        self.in_code_block = false;
        self.needs_newline = true;
    }

    /// 지금 쌓고 있는 담장 줄에 본문이 있는지 — codex `text()` 의
    /// `has_content`. 열린 줄이 없으면 마지막으로 닫은 줄을 본다.
    fn code_line_has_content(&self) -> bool {
        self.current.as_ref().map_or_else(
            || self.lines.last().is_some_and(|line| !line.spans.is_empty()),
            |spans| !spans.is_empty(),
        )
    }

    fn push_line(&mut self, spans: Vec<Span>) {
        self.flush();
        let marker_line = self.pending_marker;
        self.pending_marker = false;
        let mut line = self.prefix(marker_line);
        line.extend(spans);
        self.current_continuation = joined_span(&self.prefix(false));
        self.current = Some(line);
    }

    fn push_blank(&mut self) {
        self.flush();
        self.lines.push(Line::empty());
    }

    fn push_span(&mut self, span: Span) {
        if let Some(cell) = self.table.as_mut().and_then(tables::State::cell_mut) {
            cell.push_span(span);
            return;
        }
        if self.current.is_none() {
            self.push_line(Vec::new());
        }
        if let Some(current) = self.current.as_mut() {
            current.push(span);
        }
    }

    fn flush(&mut self) {
        if let Some(spans) = self.current.take() {
            let continuation = self.current_continuation.take();
            self.lines
                .push(Line::new(spans).with_continuation(continuation));
        }
    }

    fn finish(mut self, trim_tail: bool) -> Vec<Line> {
        self.flush();
        while trim_tail
            && self
                .lines
                .last()
                .is_some_and(|line| line.plain().trim().is_empty())
        {
            self.lines.pop();
        }
        self.lines
    }
}

// ============================================================================
// 증분 렌더의 재시작 경계
// ============================================================================

/// 증분 렌더가 **다시 시작해도 되는 원문 오프셋**을 찾는 스캐너.
///
/// 마크다운은 뒤에 오는 줄이 앞 줄의 뜻을 바꾼다 — 게으른 문단 이음, 리스트가
/// 이어지며 번호가 밀리는 것, 담장이 뒤를 삼키는 것, 표의 열 폭이 다시 잡히는
/// 것. 그래서 "원문을 아무 데서나 둘로 갈라 따로 렌더한 뒤 이어붙여도 같다" 는
/// 보장은 아무 자리에서나 서지 않는다.
///
/// 이 스캐너는 **보수적으로** 그 자리만 고른다. 오프셋 `c` 가 경계이려면:
///
/// 1. `c` 는 줄 머리이고 바로 앞 줄이 **빈 줄**이다 — 앞의 모든 블록이 닫혔고
///    게으른 이음도 setext 밑줄도 올 수 없다.
/// 2. `c` 는 담장 **밖**이다([`FenceTracker`]).
/// 3. `c` 의 줄은 0열에서 시작하고, 비어 있지 않고, **리스트 표식**(`- `·`1. `)·
///    **인용 표식**(`>`)·`<` 로 시작하지 않는다 — 그 셋만이 앞에서 열린
///    컨테이너를 이어받아 앞 줄의 렌더를 바꿀 수 있다.
/// 4. 원문에 **링크 참조 정의**(`[label]: …`)나 빈 줄로 안 닫히는 **원시 HTML
///    블록**(`<pre`·`<script`·`<style`·`<textarea`·`<!--`·`<?`·`<!`)이 한 번도
///    안 나왔다. 참조 정의는 문서 전역이라 **뒤에 나와도 앞의 렌더를 바꾸고**,
///    저 HTML 블록들은 빈 줄을 삼킨다. 하나라도 보이면 스캐너는 그 스트림
///    동안 아예 꺼지고([`Self::is_poisoned`]) 부르는 쪽은 접두어 보존을 버린다.
///
/// 그 자리에서 [`render_segment`]`(앞) ++ 빈 줄 ++ `[`render_wrapped`]`(뒤)` 는
/// `render_wrapped(전체)` 와 같다. 빈 줄 하나가 끼는 것은 `Writer` 의 모든
/// 블록 시작이 `needs_newline`(또는 `!lines.is_empty()`)일 때 **딱 한 줄**을
/// 밀고, 새로 시작한 조각은 그 자리가 거짓이라 안 밀기 때문이다.
#[derive(Debug)]
pub struct Restarts {
    /// 지금까지 먹은 원문의 길이 — 다음 줄의 시작 오프셋이다.
    offset: usize,
    fence: FenceTracker,
    previous_blank: bool,
    /// 아직 안 쓴 경계들, 오름차순.
    candidates: Vec<usize>,
    poisoned: bool,
}

impl Default for Restarts {
    fn default() -> Self {
        Self::new()
    }
}

impl Restarts {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            offset: 0,
            fence: FenceTracker::new(),
            previous_blank: false,
            candidates: Vec::new(),
            poisoned: false,
        }
    }

    /// 스캐너가 이미 본 원문의 길이.
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// 앞의 렌더까지 바꾸는 문법을 봤는가 — 참이면 증분을 아예 접어야 한다.
    #[must_use]
    pub const fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// 새로 확정된 원문으로 전진한다. 조각은 스트림에 붙는 **그 순서로** 와야
    /// 한다 — 오프셋이 논리 버퍼 기준이기 때문이다([`super::holdback::Scanner`]
    /// 와 같은 규율).
    pub fn push_source_chunk(&mut self, chunk: &str) {
        for line in chunk.split_inclusive('\n') {
            self.push_line(line);
        }
    }

    fn push_line(&mut self, source_line: &str) {
        let line = source_line.strip_suffix('\n').unwrap_or(source_line);
        let start = self.offset;
        self.offset = self.offset.saturating_add(source_line.len());
        let outside = self.fence.kind() == FenceKind::Outside;
        self.fence.advance(line);
        let blank = line.trim().is_empty();
        let after_blank = std::mem::replace(&mut self.previous_blank, blank);
        if self.poisoned || !outside || blank {
            return;
        }
        if mentions_a_link_reference_definition(line) || starts_a_blank_swallowing_html_block(line)
        {
            self.poisoned = true;
            self.candidates.clear();
            return;
        }
        if after_blank && opens_an_independent_block(line) {
            self.candidates.push(start);
        }
    }

    /// `limit` 이하의 **가장 뒤** 경계를 꺼낸다. 그보다 앞의 후보는 이제 쓸모가
    /// 없으므로 함께 버린다. `limit` 은 표 홀드백의 시작 오프셋이다 — 열 폭이
    /// 다시 잡히는 구역은 통째로 다시 렌더해야 지금 바이트가 지켜진다.
    pub fn take_upto(&mut self, limit: usize) -> Option<usize> {
        let at = self.candidates.iter().rposition(|cut| *cut <= limit)?;
        let cut = self.candidates[at];
        self.candidates.drain(..=at);
        Some(cut)
    }
}

/// 이 줄이 앞에서 열린 컨테이너를 이어받지 않고 **새 블록을 여는가**.
fn opens_an_independent_block(line: &str) -> bool {
    let bytes = line.as_bytes();
    match bytes.first() {
        // 0열이 아니거나(들여쓴 이음·코드), 인용·원시 HTML 은 손대지 않는다.
        None | Some(b' ' | b'\t' | b'>' | b'<') => false,
        // `- `·`* `·`+ ` 는 리스트 표식이다. `---`·`***` 는 가로줄이라 안전하다.
        Some(b'-' | b'*' | b'+') => !matches!(bytes.get(1), None | Some(b' ' | b'\t')),
        Some(b'0'..=b'9') => !opens_an_ordered_item(bytes),
        Some(_) => true,
    }
}

/// `12. ` · `12) ` 처럼 번호 리스트 항목을 여는가.
fn opens_an_ordered_item(bytes: &[u8]) -> bool {
    let digits = bytes.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 || digits > 9 {
        return false;
    }
    matches!(bytes.get(digits), Some(b'.' | b')'))
        && matches!(bytes.get(digits + 1), None | Some(b' ' | b'\t'))
}

/// 링크 참조 정의처럼 보이는 것이 줄 어딘가에 있는가.
///
/// 일부러 넓다 — 정의는 리스트·인용 안에도 살고, 문서 전역이라 **뒤에 나와도
/// 앞의 `[label]` 을 링크로 바꾼다**. 넓게 잡아 손해 보는 것은 속도뿐이다.
fn mentions_a_link_reference_definition(line: &str) -> bool {
    line.find("]:")
        .is_some_and(|close| line[..close].contains('['))
}

/// 빈 줄로 안 닫히는 원시 HTML 블록(CommonMark 1~5 형)을 여는가.
fn starts_a_blank_swallowing_html_block(line: &str) -> bool {
    let scan = line.trim_start();
    if line.chars().count() - scan.chars().count() > 3 || !scan.starts_with('<') {
        return false;
    }
    let head: String = scan.chars().take(9).collect::<String>().to_ascii_lowercase();
    ["<pre", "<script", "<style", "<textarea", "<?", "<!"]
        .iter()
        .any(|marker| head.starts_with(marker))
}

#[cfg(test)]
mod tests {
    use super::{render, render_segment, render_wrapped, Restarts};
    use crate::tui::ansi::Line;

    /// 원문을 한 줄씩 먹이고 나온 경계를 순서대로 모은다 — 스트림이 커밋마다
    /// 묻는 것과 같은 순서다.
    fn boundaries(source: &str) -> Vec<usize> {
        let mut scanner = Restarts::new();
        let mut out = Vec::new();
        for line in source.split_inclusive('\n') {
            scanner.push_source_chunk(line);
            let ceiling = scanner.offset();
            if let Some(cut) = scanner.take_upto(ceiling) {
                out.push(cut);
            }
        }
        out
    }

    /// 빈 줄 뒤 0열에서 새로 여는 블록만 경계다.
    #[test]
    fn a_block_that_opens_after_a_blank_line_is_a_restart_boundary() {
        let source = "### 제목\n\n본문 문단\n\n```sh\nls | wc\n```\n\n마지막\n";
        let cuts = boundaries(source);
        let starts: Vec<&str> = cuts
            .iter()
            .map(|cut| source[*cut..].lines().next().unwrap_or_default())
            .collect();
        assert_eq!(starts, vec!["본문 문단", "```sh", "마지막"]);
    }

    /// 담장 안은 경계가 아니다 — 파이프도 빈 줄도 코드다.
    #[test]
    fn nothing_inside_a_fence_is_a_boundary() {
        assert!(boundaries("```sh\n\nls | wc\n\ndone\n```\n").is_empty());
    }

    /// 앞에서 열린 컨테이너를 이어받는 줄은 경계가 아니다 — 리스트 항목은
    /// 빈 줄을 건너 같은 리스트로 이어지고(번호가 밀린다), 인용도 그렇다.
    #[test]
    fn a_line_that_can_continue_an_open_container_is_not_a_boundary() {
        for source in [
            "1. 하나\n\n1. 둘\n",
            "- 하나\n\n- 둘\n",
            "* 하나\n\n* 둘\n",
            "> 인용\n\n> 이어지는 인용\n",
            "문단\n\n  들여쓴 이음\n",
        ] {
            assert!(boundaries(source).is_empty(), "{source:?} 에 경계가 생겼다");
        }
        // 가로줄과 강조는 리스트 표식이 아니다 — 경계가 맞다.
        assert_eq!(boundaries("문단\n\n---\n").len(), 1);
        assert_eq!(boundaries("문단\n\n**굵게**\n").len(), 1);
    }

    /// 링크 참조 정의는 문서 전역이라 **뒤에 나와도 앞의 렌더를 바꾼다** —
    /// 보이는 순간 스캐너가 통째로 꺼진다.
    #[test]
    fn a_link_reference_definition_poisons_the_scanner() {
        let mut scanner = Restarts::new();
        scanner.push_source_chunk("문단\n\n다른 문단\n");
        assert!(!scanner.is_poisoned());
        assert!(scanner.take_upto(usize::MAX).is_some());
        scanner.push_source_chunk("\n[문서]: https://example.com\n");
        assert!(scanner.is_poisoned());
        assert!(scanner.take_upto(usize::MAX).is_none());
    }

    /// 빈 줄을 삼키는 원시 HTML 블록도 마찬가지다.
    #[test]
    fn a_blank_swallowing_html_block_poisons_the_scanner() {
        let mut scanner = Restarts::new();
        scanner.push_source_chunk("문단\n\n<pre>\n한 줄\n\n두 줄\n</pre>\n\n뒤\n");
        assert!(scanner.is_poisoned());
        // 빈 줄로 닫히는 7형(`<br>`)은 무해하다.
        let mut plain = Restarts::new();
        plain.push_source_chunk("문단\n\n<br>\n\n뒤 문단\n");
        assert!(!plain.is_poisoned());
    }

    /// `take_upto` 는 천장 이하의 **가장 뒤**를 주고 그 앞은 버린다 — 천장은 표
    /// 홀드백의 시작이다.
    #[test]
    fn take_upto_returns_the_last_boundary_below_the_ceiling() {
        let source = "하나\n\n둘\n\n셋\n";
        let cuts = boundaries(source);
        assert_eq!(cuts.len(), 2);
        let mut scanner = Restarts::new();
        scanner.push_source_chunk(source);
        assert_eq!(scanner.take_upto(cuts[0]), Some(cuts[0]));
        assert_eq!(scanner.take_upto(cuts[1]), Some(cuts[1]));
        assert_eq!(scanner.take_upto(usize::MAX), None);
    }

    /// 조각 렌더는 꼬리 빈 줄을 남긴다 — 다듬기는 문서 끝에서 한 번만.
    #[test]
    fn a_segment_render_keeps_the_trailing_blank_a_full_render_trims() {
        let source = "```text\n코드\n\n```\n";
        assert_eq!(render_wrapped(source, Some(40)).len(), 1);
        assert_eq!(render_segment(source, Some(40)).len(), 2);
    }

    fn plain(source: &str) -> Vec<String> {
        render(source).iter().map(Line::plain).collect()
    }

    /// 캡처(`codex-tui-v0.149.1-turn.bin`)의 답변 첫 줄: 해시를 남기고
    /// bold+italic 만 입힌다.
    #[test]
    fn headings_keep_their_hashes_and_take_the_level_style() {
        let lines = render("### 자기소개");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].plain(), "### 자기소개");
        assert!(lines[0].spans.iter().all(|span| span.style.bold && span.style.italic));
    }

    #[test]
    fn heading_levels_follow_codex_styles() {
        assert!(render("# a")[0].spans[0].style.underline);
        assert!(render("## a")[0].spans[0].style.bold);
        assert!(!render("## a")[0].spans[0].style.italic);
        assert!(render("#### a")[0].spans[0].style.italic);
        assert!(!render("#### a")[0].spans[0].style.bold);
    }

    #[test]
    fn bullets_use_the_measured_marker_and_indent() {
        assert_eq!(plain("- one\n- two"), vec!["- one", "- two"]);
        assert_eq!(
            plain("- one\n  - nested"),
            vec!["- one", "    - nested"]
        );
    }

    #[test]
    fn ordered_lists_number_from_the_source() {
        assert_eq!(plain("1. one\n2. two"), vec!["1. one", "2. two"]);
    }

    #[test]
    fn ordered_list_markers_use_the_light_blue_sgr_vocabulary() {
        let line = &render("1. one")[0];
        let mut rendered = String::new();
        crate::tui::ansi::write_spans(line, &mut rendered);

        assert!(
            rendered.starts_with("\u{1b}[94;49m1. "),
            "ordered marker should start with SGR 94, got {rendered:?}"
        );
    }

    #[test]
    fn a_blank_line_separates_a_heading_from_the_list_below() {
        assert_eq!(
            plain("### t\n\n- a"),
            vec!["### t".to_string(), String::new(), "- a".to_string()]
        );
    }

    #[test]
    fn inline_styles_land_on_the_right_spans() {
        let line = &render("**bold** and *italic* and `code`")[0];
        let bold = line.spans.iter().find(|span| span.text == "bold").expect("bold");
        assert!(bold.style.bold);
        let italic = line.spans.iter().find(|span| span.text == "italic").expect("italic");
        assert!(italic.style.italic);
        let code = line.spans.iter().find(|span| span.text == "code").expect("code");
        assert_eq!(code.style.fg, Some(crate::tui::ansi::Color::CYAN));
    }

    #[test]
    fn blockquotes_get_the_green_marker() {
        let lines = render("> quoted");
        assert_eq!(lines[0].plain(), "> quoted");
        assert_eq!(lines[0].spans[0].style.fg, Some(crate::tui::ansi::Color::GREEN));
    }

    /// codex 는 담장의 언어 이름을 화면에 안 찍는다(`start_codeblock`) —
    /// 아는 언어면 본문에 색만 입힌다.
    #[test]
    fn fenced_code_blocks_keep_their_lines_without_a_language_header() {
        let lines = render("```sh\necho hi\necho bye\n```");
        assert_eq!(
            lines.iter().map(Line::plain).collect::<Vec<_>>(),
            vec!["echo hi", "echo bye"]
        );
        assert!(
            lines[0].spans.iter().any(|span| span.style.fg.is_some()),
            "a known language is highlighted: {:?}",
            lines[0].spans
        );
    }

    /// 언어 없는 담장은 색 없는 평문이고, 꼬리 개행이 유령 빈 줄을 남기지
    /// 않는다 — codex `text()` 가 `split('\n')` 이 아니라 `lines()` 를 쓴다.
    #[test]
    fn a_language_less_fence_stays_plain_and_leaves_no_phantom_line() {
        let lines = render("```\necho hi\n```\n\nafter");
        assert_eq!(
            lines.iter().map(Line::plain).collect::<Vec<_>>(),
            vec!["echo hi", "", "after"]
        );
        assert!(lines[0].spans.iter().all(|span| span.style.fg.is_none()));
    }

    /// 모르는 언어도 폴백이라 평문이다 — 실측
    /// (`codex-tui-v0.149.1-codeblock.bin` 38행).
    #[test]
    fn an_unknown_language_fence_falls_back_to_plain_text() {
        let lines = render("```nosuchlang\nplain fallback line\n```");
        assert_eq!(
            lines.iter().map(Line::plain).collect::<Vec<_>>(),
            vec!["plain fallback line"]
        );
        assert!(lines[0].spans.iter().all(|span| span.style.fg.is_none()));
    }

    /// info string 의 첫 토큰만 언어다 — `rust,no_run`·`rust title=demo`
    /// (codex `start_codeblock`).
    #[test]
    fn only_the_first_info_token_names_the_language() {
        for fence in ["```rust,no_run", "```rust title=demo"] {
            let lines = render(&format!("{fence}\nfn main() {{}}\n```"));
            assert!(
                lines[0].spans.iter().any(|span| span.style.fg.is_some()),
                "{fence} should still resolve to rust"
            );
        }
    }

    /// 들여쓰기 담장은 네 칸을 물려받는다 — codex `start_tag` 의
    /// `CodeBlockKind::Indented => Some(Span::from(" ".repeat(4)))`.
    #[test]
    fn an_indented_fence_keeps_its_four_columns() {
        assert_eq!(plain("text\n\n    indented code"), vec!["text", "", "    indented code"]);
    }

    /// `ENABLE_TASKLISTS` 는 codex 에 없다 — 체크박스는 평문으로 흐른다.
    #[test]
    fn task_lists_stay_plain_text() {
        assert_eq!(plain("- [x] done\n- [ ] todo"), vec!["- [x] done", "- [ ] todo"]);
    }

    /// 표는 격자로 나온다 — 헤더 아래는 `━`, 본문 행 사이는 `─`
    /// (codex `TABLE_HEADER_SEPARATOR_CHAR`/`TABLE_BODY_SEPARATOR_CHAR`).
    /// 캡처(`codex-tui-v0.149.1-table.bin`)의 열 폭·여백이 그대로다.
    #[test]
    fn tables_render_as_a_separated_grid() {
        let source = concat!(
            "| Name | Count | Status |\n",
            "| --- | --- | --- |\n",
            "| alpha | 1 | ok |\n",
            "| beta | 22 | pending |\n",
        );
        let lines: Vec<String> = render_wrapped(source, Some(118))
            .iter()
            .map(Line::plain)
            .collect();
        assert_eq!(
            lines,
            vec![
                " Name     Count    Status".to_string(),
                "━━━━━━━  ━━━━━━━  ━━━━━━━━━".to_string(),
                " alpha    1        ok".to_string(),
                "───────  ───────  ─────────".to_string(),
                " beta     22       pending".to_string(),
            ]
        );
    }

    /// 정렬은 `:---:`·`---:` 를 따른다 — codex `render_table_row` 의
    /// `Alignment` 분기.
    #[test]
    fn column_alignment_follows_the_delimiter_row() {
        let source = concat!(
            "| left | center | right |\n",
            "|:-----|:------:|------:|\n",
            "| a | b | c |\n",
        );
        let lines: Vec<String> = render_wrapped(source, Some(60))
            .iter()
            .map(Line::plain)
            .collect();
        // 열 폭은 헤더가 정한다(4·6·5). 본문 한 글자가 왼쪽·가운데·오른쪽에 선다.
        assert_eq!(lines[0], " left    center    right");
        assert_eq!(lines[2], " a         b           c");
    }

    /// 헤더 행에는 줄 스타일이 실린다 — codex 는 syntect 스코프 색 + bold 를
    /// `Line::style` 로 준다. 본문 행과 구분선은 그렇지 않다.
    #[test]
    fn the_header_row_carries_the_line_style() {
        let source = concat!("| A | B |\n", "| --- | --- |\n", "| 1 | 2 |\n");
        let lines = render_wrapped(source, Some(60));
        assert_eq!(lines[0].style.fg, Some(crate::tui::palette::TABLE_HEADER));
        assert!(lines[0].style.bold);
        assert_eq!(lines[1].style, crate::tui::ansi::Style::new());
        assert_eq!(lines[2].style, crate::tui::ansi::Style::new());
        // 구분선은 dim 스팬 하나다.
        assert!(lines[1].spans[0].style.dim);
    }

    /// 좁은 폭에서는 **토큰중심 열이 서술 열보다 먼저** 폭을 내놓는다 —
    /// codex `shrink_columns` 의 우선순위(TokenHeavy → Narrative → Compact).
    /// 긴 경로가 읽히는 산문을 짓누르지 않게 하는 자리다.
    #[test]
    fn a_token_heavy_column_gives_up_width_before_prose() {
        let source = concat!(
            "| Path | Note |\n",
            "| --- | --- |\n",
            "| /a/very/long/absolute/path/to/one.rs | 이 열은 문장이 길어서 서술 열로 분류됩니다 그래서 폭을 늦게 내놓습니다 |\n",
            "| /b/another/quite/long/path/two.rs | 두 번째 행도 충분히 길어서 평균 낱말 수가 넉넉히 넘어갑니다 정말로요 |\n",
        );
        let wide = render_wrapped(source, Some(200));
        let narrow = render_wrapped(source, Some(80));
        // 좁아지면 줄 수가 늘어난다(칸이 접힌다).
        assert!(
            narrow.len() > wide.len(),
            "narrow={} wide={}",
            narrow.len(),
            wide.len()
        );
        // 격자는 유지된다 — 아직 레코드로 떨어질 만큼 좁지 않다.
        assert!(narrow.iter().any(|line| line.plain().contains('━')));
    }

    /// 열이 셋도 못 들어가는 폭에서는 키/값 레코드로 세운다 — codex
    /// `table_key_value::render_records`.
    #[test]
    fn a_table_too_narrow_for_columns_becomes_key_value_records() {
        let source = concat!(
            "| Name | Path |\n",
            "| --- | --- |\n",
            "| alpha | /very/long/path/to/a/file.rs |\n",
        );
        let lines: Vec<String> = render_wrapped(source, Some(20))
            .iter()
            .map(Line::plain)
            .collect();
        assert!(
            lines.iter().any(|line| line.trim_start().starts_with("Name")),
            "the header became a label: {lines:?}"
        );
        assert!(
            lines.iter().all(|line| !line.contains('━')),
            "no grid separator survives: {lines:?}"
        );
    }
}
