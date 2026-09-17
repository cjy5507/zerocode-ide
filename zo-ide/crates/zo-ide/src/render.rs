//! 마크다운 → ANSI 문자열 렌더러 + 스트리밍 경계 상태.
//!
//! 포팅: forge-code `crates/zo-cli/src/render.rs`. 이 트리의 규율(터미널 상태
//! 비접촉)에 맞춰 두 가지를 덜어냈다 — ① `Spinner`(커서 저장/복원·줄 지우기
//! 이스케이프는 append-only 패인 출력에 금지), ② `crossterm` 의존(스타일은
//! SGR 시퀀스를 직접 방출; 색 번호는 crossterm의 매핑과 동일하게 유지해
//! 원본과 바이트 호환). 나머지 로직·테스트는 원본 그대로다.

use std::fmt::Write as FmtWrite;
use std::io::{self, Write};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::util::ansi::strip_ansi;

/// True when `NO_COLOR` is set to a non-empty value (see <https://no-color.org>).
/// The text one-shot path suppresses its ANSI-decorated status indicator when
/// this holds so piped/headless stdout carries no escape sequences.
#[must_use]
pub fn no_color_env() -> bool {
    let disabled = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    if !disabled {
        // App::new calls this after raw mode starts and before its EventStream
        // exists. That is the only safe point in the current TUI boundary for
        // the bounded OSC 10/11 startup probe: plain/cooked callers fail the
        // raw-TTY guard and retain their byte-for-byte output.
        crate::tui::palette::initialize_if_raw_tty();
    }
    disabled
}

/// 16색 팔레트 — 번호는 crossterm의 `Color` 매핑(브라이트=90번대)과 동일하다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Grey,
    DarkGrey,
    DarkCyan,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
}

impl Color {
    const fn fg_code(self) -> u8 {
        match self {
            Color::Grey => 37,
            Color::DarkGrey => 90,
            Color::DarkCyan => 36,
            Color::Green => 92,
            Color::Yellow => 93,
            Color::Blue => 94,
            Color::Magenta => 95,
            Color::Cyan => 96,
            Color::White => 97,
        }
    }
}

/// SGR 코드 목록으로 감싼 문자열. 코드가 없으면 이스케이프 없이 원문 그대로 —
/// crossterm `stylize()`가 속성 없는 텍스트를 맨몸으로 내보내는 것과 동일.
fn sgr(text: &str, codes: &[u8]) -> String {
    if codes.is_empty() {
        return text.to_string();
    }
    let mut seq = String::new();
    for (index, code) in codes.iter().enumerate() {
        if index > 0 {
            seq.push(';');
        }
        let _ = write!(seq, "{code}");
    }
    format!("\u{1b}[{seq}m{text}\u{1b}[0m")
}

fn paint(text: &str, color: Color) -> String {
    sgr(text, &[color.fg_code()])
}

fn paint_bold(text: &str, color: Color) -> String {
    sgr(text, &[1, color.fg_code()])
}

fn paint_underlined(text: &str, color: Color) -> String {
    sgr(text, &[4, color.fg_code()])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColorTheme {
    heading: Color,
    emphasis: Color,
    strong: Color,
    inline_code: Color,
    link: Color,
    quote: Color,
    table_border: Color,
    code_block_border: Color,
}

impl Default for ColorTheme {
    fn default() -> Self {
        Self {
            heading: Color::Cyan,
            emphasis: Color::Magenta,
            strong: Color::Yellow,
            inline_code: Color::Green,
            link: Color::Blue,
            quote: Color::DarkGrey,
            table_border: Color::DarkCyan,
            code_block_border: Color::DarkGrey,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ListKind {
    Unordered,
    Ordered { next_index: u64 },
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct TableState {
    headers: Vec<String>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    in_head: bool,
}

impl TableState {
    fn push_cell(&mut self) {
        let cell = self.current_cell.trim().to_string();
        self.current_row.push(cell);
        self.current_cell.clear();
    }

    fn finish_row(&mut self) {
        if self.current_row.is_empty() {
            return;
        }
        let row = std::mem::take(&mut self.current_row);
        if self.in_head {
            self.headers = row;
        } else {
            self.rows.push(row);
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct RenderState {
    emphasis: usize,
    strong: usize,
    heading_level: Option<u8>,
    quote: usize,
    list_stack: Vec<ListKind>,
    link_stack: Vec<LinkState>,
    table: Option<TableState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkState {
    destination: String,
    text: String,
}

impl RenderState {
    fn style_text(&self, text: &str, theme: ColorTheme) -> String {
        let mut codes: Vec<u8> = Vec::new();

        if matches!(self.heading_level, Some(1 | 2)) || self.strong > 0 {
            codes.push(1);
        }
        if self.emphasis > 0 {
            codes.push(3);
        }

        let mut color = if let Some(level) = self.heading_level {
            Some(match level {
                1 => theme.heading,
                2 => Color::White,
                3 => Color::Blue,
                _ => Color::Grey,
            })
        } else if self.strong > 0 {
            Some(theme.strong)
        } else if self.emphasis > 0 {
            Some(theme.emphasis)
        } else {
            None
        };

        if self.quote > 0 {
            color = Some(theme.quote);
        }

        if let Some(color) = color {
            codes.push(color.fg_code());
        }

        sgr(text, &codes)
    }

    fn append_raw(&mut self, output: &mut String, text: &str) {
        if let Some(link) = self.link_stack.last_mut() {
            link.text.push_str(text);
        } else if let Some(table) = self.table.as_mut() {
            table.current_cell.push_str(text);
        } else {
            output.push_str(text);
        }
    }

    fn append_styled(&mut self, output: &mut String, text: &str, theme: ColorTheme) {
        let styled = self.style_text(text, theme);
        self.append_raw(output, &styled);
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TerminalRenderer {
    color_theme: ColorTheme,
}

impl TerminalRenderer {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn color_theme(&self) -> &ColorTheme {
        &self.color_theme
    }

    #[must_use]
    pub fn render_markdown(&self, markdown: &str) -> String {
        let mut output = String::new();
        let mut state = RenderState::default();
        let mut code_language = String::new();
        let mut code_buffer = String::new();
        let mut in_code_block = false;

        // Everything except smart punctuation: curly quotes/ellipses corrupt
        // copy-pasted identifiers and JSON-ish prose in a coding terminal.
        let mut options = Options::all();
        options.remove(Options::ENABLE_SMART_PUNCTUATION);
        for event in Parser::new_ext(markdown, options) {
            self.render_event(
                event,
                &mut state,
                &mut output,
                &mut code_buffer,
                &mut code_language,
                &mut in_code_block,
            );
        }

        output.trim_end().to_string()
    }

    #[must_use]
    pub fn markdown_to_ansi(&self, markdown: &str) -> String {
        self.render_markdown(markdown)
    }

    #[allow(clippy::too_many_lines)] // flat event-to-render dispatch, one arm per event
    fn render_event(
        &self,
        event: Event<'_>,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        code_language: &mut String,
        in_code_block: &mut bool,
    ) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                Self::start_heading(state, level as u8, output);
            }
            Event::End(TagEnd::Paragraph) => output.push_str("\n\n"),
            Event::Start(Tag::BlockQuote(..)) => self.start_quote(state, output),
            Event::End(TagEnd::BlockQuote(..)) => {
                state.quote = state.quote.saturating_sub(1);
                output.push('\n');
            }
            Event::End(TagEnd::Heading(..)) => {
                state.heading_level = None;
                output.push_str("\n\n");
            }
            Event::End(TagEnd::Item) | Event::SoftBreak | Event::HardBreak => {
                state.append_raw(output, "\n");
            }
            Event::Start(Tag::List(first_item)) => {
                let kind = match first_item {
                    Some(index) => ListKind::Ordered { next_index: index },
                    None => ListKind::Unordered,
                };
                state.list_stack.push(kind);
            }
            Event::End(TagEnd::List(..)) => {
                state.list_stack.pop();
                output.push('\n');
            }
            Event::Start(Tag::Item) => Self::start_item(state, output),
            Event::Start(Tag::CodeBlock(kind)) => {
                *in_code_block = true;
                *code_language = match kind {
                    CodeBlockKind::Indented => String::from("text"),
                    CodeBlockKind::Fenced(lang) => lang.to_string(),
                };
                code_buffer.clear();
                self.start_code_block(code_language, output);
            }
            Event::End(TagEnd::CodeBlock) => {
                self.finish_code_block(code_buffer, code_language, output);
                *in_code_block = false;
                code_language.clear();
                code_buffer.clear();
            }
            Event::Start(Tag::Emphasis) => state.emphasis += 1,
            Event::End(TagEnd::Emphasis) => state.emphasis = state.emphasis.saturating_sub(1),
            Event::Start(Tag::Strong) => state.strong += 1,
            Event::End(TagEnd::Strong) => state.strong = state.strong.saturating_sub(1),
            Event::Code(code) => {
                let rendered = paint(&format!("`{code}`"), self.color_theme.inline_code);
                state.append_raw(output, &rendered);
            }
            Event::Rule => output.push_str("---\n"),
            Event::Text(text) => {
                self.push_text(text.as_ref(), state, output, code_buffer, *in_code_block);
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                state.append_raw(output, &html);
            }
            Event::FootnoteReference(reference) => {
                state.append_raw(output, &format!("[{reference}]"));
            }
            Event::TaskListMarker(done) => {
                state.append_raw(output, if done { "[x] " } else { "[ ] " });
            }
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                state.append_raw(output, &math);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                state.link_stack.push(LinkState {
                    destination: dest_url.to_string(),
                    text: String::new(),
                });
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = state.link_stack.pop() {
                    let label = if link.text.is_empty() {
                        link.destination.clone()
                    } else {
                        link.text
                    };
                    let rendered = paint_underlined(
                        &format!("[{label}]({})", link.destination),
                        self.color_theme.link,
                    );
                    state.append_raw(output, &rendered);
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                let rendered = paint(&format!("[image:{dest_url}]"), self.color_theme.link);
                state.append_raw(output, &rendered);
            }
            Event::Start(Tag::Table(..)) => state.table = Some(TableState::default()),
            Event::End(TagEnd::Table) => {
                if let Some(table) = state.table.take() {
                    output.push_str(&self.render_table(&table));
                    output.push_str("\n\n");
                }
            }
            Event::Start(Tag::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.in_head = true;
                }
            }
            Event::End(TagEnd::TableHead) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                    table.in_head = false;
                }
            }
            Event::Start(Tag::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_row.clear();
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableRow) => {
                if let Some(table) = state.table.as_mut() {
                    table.finish_row();
                }
            }
            Event::Start(Tag::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.current_cell.clear();
                }
            }
            Event::End(TagEnd::TableCell) => {
                if let Some(table) = state.table.as_mut() {
                    table.push_cell();
                }
            }
            Event::Start(Tag::Paragraph | Tag::MetadataBlock(..) | _)
            | Event::End(TagEnd::Image | TagEnd::MetadataBlock(..) | _) => {}
        }
    }

    fn start_heading(state: &mut RenderState, level: u8, output: &mut String) {
        state.heading_level = Some(level);
        if !output.is_empty() {
            output.push('\n');
        }
    }

    fn start_quote(&self, state: &mut RenderState, output: &mut String) {
        state.quote += 1;
        let _ = write!(output, "{}", paint("│ ", self.color_theme.quote));
    }

    fn start_item(state: &mut RenderState, output: &mut String) {
        let depth = state.list_stack.len().saturating_sub(1);
        output.push_str(&"  ".repeat(depth));

        let marker = match state.list_stack.last_mut() {
            Some(ListKind::Ordered { next_index }) => {
                let value = *next_index;
                *next_index += 1;
                format!("{value}. ")
            }
            _ => "• ".to_string(),
        };
        output.push_str(&marker);
    }

    fn start_code_block(&self, code_language: &str, output: &mut String) {
        let label = if code_language.is_empty() {
            "code".to_string()
        } else {
            code_language.to_string()
        };
        let _ = writeln!(
            output,
            "{}",
            paint_bold(&format!("╭─ {label}"), self.color_theme.code_block_border)
        );
    }

    fn finish_code_block(&self, code_buffer: &str, code_language: &str, output: &mut String) {
        output.push_str(&Self::highlight_code(code_buffer, code_language));
        let _ = write!(
            output,
            "{}",
            paint_bold("╰─", self.color_theme.code_block_border)
        );
        output.push_str("\n\n");
    }

    fn push_text(
        &self,
        text: &str,
        state: &mut RenderState,
        output: &mut String,
        code_buffer: &mut String,
        in_code_block: bool,
    ) {
        if in_code_block {
            code_buffer.push_str(text);
        } else {
            state.append_styled(output, text, self.color_theme);
        }
    }

    fn render_table(&self, table: &TableState) -> String {
        let mut rows = Vec::new();
        if !table.headers.is_empty() {
            rows.push(table.headers.clone());
        }
        rows.extend(table.rows.iter().cloned());

        if rows.is_empty() {
            return String::new();
        }

        let column_count = rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths = (0..column_count)
            .map(|column| {
                rows.iter()
                    .filter_map(|row| row.get(column))
                    .map(|cell| visible_width(cell))
                    .max()
                    .unwrap_or(0)
            })
            .collect::<Vec<_>>();

        let border = paint("│", self.color_theme.table_border);
        let separator = widths
            .iter()
            .map(|width| "─".repeat(*width + 2))
            .collect::<Vec<_>>()
            .join(&paint("┼", self.color_theme.table_border));
        let separator = format!("{border}{separator}{border}");

        let mut output = String::new();
        if !table.headers.is_empty() {
            output.push_str(&self.render_table_row(&table.headers, &widths, true));
            output.push('\n');
            output.push_str(&separator);
            if !table.rows.is_empty() {
                output.push('\n');
            }
        }

        for (index, row) in table.rows.iter().enumerate() {
            output.push_str(&self.render_table_row(row, &widths, false));
            if index + 1 < table.rows.len() {
                output.push('\n');
            }
        }

        output
    }

    fn render_table_row(&self, row: &[String], widths: &[usize], is_header: bool) -> String {
        let border = paint("│", self.color_theme.table_border);
        let mut line = String::new();
        line.push_str(&border);

        for (index, width) in widths.iter().enumerate() {
            let cell = row.get(index).map_or("", String::as_str);
            line.push(' ');
            if is_header {
                let _ = write!(line, "{}", paint_bold(cell, self.color_theme.heading));
            } else {
                line.push_str(cell);
            }
            let padding = width.saturating_sub(visible_width(cell));
            line.push_str(&" ".repeat(padding + 1));
            line.push_str(&border);
        }

        line
    }

    #[must_use]
    pub fn highlight_code(code: &str, _language: &str) -> String {
        let mut colored_output = String::new();

        for line in code.split_inclusive('\n') {
            colored_output.push_str(&apply_code_block_background(line));
        }

        colored_output
    }

    pub fn stream_markdown(&self, markdown: &str, out: &mut impl Write) -> io::Result<()> {
        let rendered_markdown = self.markdown_to_ansi(markdown);
        write!(out, "{rendered_markdown}")?;
        if !rendered_markdown.ends_with('\n') {
            writeln!(out)?;
        }
        out.flush()
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MarkdownStreamState {
    pending: String,
}

impl MarkdownStreamState {
    #[must_use]
    pub fn push(&mut self, renderer: &TerminalRenderer, delta: &str) -> Option<String> {
        self.pending.push_str(delta);
        let split = find_stream_safe_boundary(&self.pending)?;
        let ready = self.pending[..split].to_string();
        self.pending.drain(..split);
        let ansi_output = renderer.markdown_to_ansi(&ready);
        if ansi_output.is_empty() {
            return None;
        }
        // `markdown_to_ansi` trims trailing newlines, but this segment ended at
        // a block boundary (blank line / closed fence) in the source — restore
        // the separator so consecutive segments don't glue a heading or bullet
        // onto the previous block's last line.
        Some(format!("{ansi_output}\n\n"))
    }

    #[must_use]
    pub fn flush(&mut self, renderer: &TerminalRenderer) -> Option<String> {
        if self.pending.trim().is_empty() {
            self.pending.clear();
            None
        } else {
            let pending = std::mem::take(&mut self.pending);
            // Final segment of the block: end its line, but add no blank line —
            // whatever follows (tool card, Done footer) owns its own spacing.
            Some(format!("{}\n", renderer.markdown_to_ansi(&pending)))
        }
    }
}

fn apply_code_block_background(line: &str) -> String {
    let trimmed = line.trim_end_matches('\n');
    let trailing_newline = if trimmed.len() == line.len() {
        ""
    } else {
        "\n"
    };
    let with_background = trimmed.replace("\u{1b}[0m", "\u{1b}[0;48;5;236m");
    format!("\u{1b}[48;5;236m{with_background}\u{1b}[0m{trailing_newline}")
}

fn find_stream_safe_boundary(markdown: &str) -> Option<usize> {
    let mut in_fence = false;
    let mut last_boundary = None;

    for (offset, line) in markdown.split_inclusive('\n').scan(0usize, |cursor, line| {
        let start = *cursor;
        *cursor += line.len();
        Some((start, line))
    }) {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            if !in_fence {
                last_boundary = Some(offset + line.len());
            }
            continue;
        }

        if in_fence {
            continue;
        }

        if trimmed.is_empty() {
            last_boundary = Some(offset + line.len());
        }
    }

    last_boundary
}

fn visible_width(input: &str) -> usize {
    strip_ansi(input).chars().count()
}

#[cfg(test)]
mod tests {
    use super::{strip_ansi, MarkdownStreamState, TerminalRenderer};
    use std::time::Instant;

    /// Baseline micro-bench for the streaming markdown path. Kept as a normal
    /// test because the current assertions are intentionally light-weight: the
    /// goal is to exercise the hot path without leaving ignored-test debt in
    /// the renderer lane.
    #[test]
    fn markdown_stream_bench() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();
        let paragraph = "This is a plain prose paragraph with no markdown features, \
                         simulating the common case of a streamed assistant response \
                         that arrives a sentence at a time.\n\n";

        let start = Instant::now();
        for _ in 0..500 {
            let _ = state.push(&renderer, paragraph);
        }
        let _ = state.flush(&renderer);
        let elapsed_plain = start.elapsed();
        eprintln!("markdown_stream_bench plain: 500 paragraphs in {elapsed_plain:?}");

        // Realistic mixed content: heading, list, code block, prose.
        let mut state = MarkdownStreamState::default();
        let mixed = "# Section\n\nSome **bold** text with `inline` code.\n\n\
                     - bullet one\n- bullet two\n\n\
                     ```rust\nfn hi() { println!(\"hi\"); }\n```\n\n\
                     Closing paragraph.\n\n";
        let start = Instant::now();
        for _ in 0..200 {
            let _ = state.push(&renderer, mixed);
        }
        let _ = state.flush(&renderer);
        let elapsed_mixed = start.elapsed();
        eprintln!("markdown_stream_bench mixed: 200 blocks in {elapsed_mixed:?}");
    }

    #[test]
    fn renders_markdown_with_styling_and_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("# Heading\n\nThis is **bold** and *italic*.\n\n- item\n\n`code`");

        assert!(markdown_output.contains("Heading"));
        assert!(markdown_output.contains("• item"));
        assert!(markdown_output.contains("code"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn renders_links_as_colored_markdown_labels() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("See [Zo](https://example.com/docs) now.");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("[Zo](https://example.com/docs)"));
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn highlights_fenced_code_blocks() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.markdown_to_ansi("```rust\nfn hi() { println!(\"hi\"); }\n```");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("╭─ rust"));
        assert!(plain_text.contains("fn hi"));
        assert!(markdown_output.contains('\u{1b}'));
        assert!(markdown_output.contains("[48;5;236m"));
    }

    #[test]
    fn renders_ordered_and_nested_lists() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output =
            terminal_renderer.render_markdown("1. first\n2. second\n   - nested\n   - child");
        let plain_text = strip_ansi(&markdown_output);

        assert!(plain_text.contains("1. first"));
        assert!(plain_text.contains("2. second"));
        assert!(plain_text.contains("  • nested"));
        assert!(plain_text.contains("  • child"));
    }

    #[test]
    fn renders_tables_with_alignment() {
        let terminal_renderer = TerminalRenderer::new();
        let markdown_output = terminal_renderer
            .render_markdown("| Name | Value |\n| ---- | ----- |\n| alpha | 1 |\n| beta | 22 |");
        let plain_text = strip_ansi(&markdown_output);
        let lines = plain_text.lines().collect::<Vec<_>>();

        assert_eq!(lines[0], "│ Name  │ Value │");
        assert_eq!(lines[1], "│───────┼───────│");
        assert_eq!(lines[2], "│ alpha │ 1     │");
        assert_eq!(lines[3], "│ beta  │ 22    │");
        assert!(markdown_output.contains('\u{1b}'));
    }

    #[test]
    fn streaming_state_waits_for_complete_blocks() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        assert_eq!(state.push(&renderer, "# Heading"), None);
        let flushed = state
            .push(&renderer, "\n\nParagraph\n\n")
            .expect("completed block");
        let plain_text = strip_ansi(&flushed);
        assert!(plain_text.contains("Heading"));
        assert!(plain_text.contains("Paragraph"));

        assert_eq!(state.push(&renderer, "```rust\nfn main() {}\n"), None);
        let code = state
            .push(&renderer, "```\n")
            .expect("closed code fence flushes");
        assert!(strip_ansi(&code).contains("fn main()"));
    }

    #[test]
    fn streamed_segments_keep_block_separation() {
        let renderer = TerminalRenderer::new();
        let mut state = MarkdownStreamState::default();

        let first = state
            .push(&renderer, "## 결과 보고\n\n")
            .expect("heading segment");
        assert!(
            first.ends_with("\n\n"),
            "segment must restore its trailing block separator: {first:?}"
        );
        let second = state
            .push(&renderer, "**원인:** 잘못된 계산.\n\n")
            .expect("paragraph segment");

        let plain = strip_ansi(&format!("{first}{second}"));
        assert!(
            !plain.contains("보고**") && !plain.contains("보고원인"),
            "consecutive segments must not glue blocks together: {plain:?}"
        );
    }

    #[test]
    fn markdown_keeps_straight_quotes() {
        let renderer = TerminalRenderer::new();
        let plain = strip_ansi(&renderer.markdown_to_ansi("say \"hello\" and 'bye'..."));
        assert!(
            plain.contains('"') && plain.contains('\''),
            "smart punctuation must stay off so quotes survive copy-paste: {plain:?}"
        );
    }

    /// crossterm 제거 후에도 색 번호가 원본(crossterm 매핑)과 동일함을 고정 —
    /// 헤딩 1은 bright cyan(96), 인라인 코드는 bright green(92).
    #[test]
    fn sgr_codes_match_crossterm_palette() {
        let renderer = TerminalRenderer::new();
        let heading = renderer.render_markdown("# Title");
        assert!(heading.contains("\u{1b}[1;96mTitle\u{1b}[0m"), "{heading:?}");
        let code = renderer.render_markdown("run `ls` now");
        assert!(code.contains("\u{1b}[92m`ls`\u{1b}[0m"), "{code:?}");
    }
}
