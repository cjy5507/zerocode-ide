//! Thinking on screen (t-5872): the status word while the model reasons, and
//! the folded cell its thinking becomes when the block ends.
//!
//! codex takes the shimmer word from the first `**bold**` heading of a
//! reasoning summary (`chatwidget.rs::extract_first_bold`) — OpenAI summaries
//! always open with one. Anthropic thinking has no headings at all, so a turn
//! on Fable or Opus said `Working` for minutes while the model was writing
//! sentences about what it was doing. [`live_heading`] is the one function
//! that turns either stream into the word: codex's rule first, and when no
//! heading closes, the last complete sentence (the first one, until a second
//! ends), cut to a width that leaves the interrupt hint on the row.
//!
//! The body is codex's too — a dim italic cell — but Anthropic's thinking is
//! raw reasoning, pages of it, so the committed cell is a title row and a
//! preview: in a bare terminal the first [`THINKING_PREVIEW_ROWS`] rows and
//! `… +N lines`, in a `ZeroCode` pane the whole body folded under the title
//! ([`super::folds`], the same markers a tool cell commits with).

use unicode_width::UnicodeWidthStr;

use super::ansi::{Line, Span, Style};
use super::folds::{self, FoldIds, FoldMode};
use super::{palette, strings};

/// Whether the TUI shows thinking bodies when neither `--show-thinking` nor
/// the `showThinking` setting says otherwise. codex shows reasoning summaries
/// by default (`hide_agent_reasoning` is false), and the person's question —
/// "what is it doing?" — is answered by the body far more often than by a
/// spinner.
pub const SHOW_BY_DEFAULT: bool = true;

/// The columns a derived heading may take on the status row. The row is
/// `• <heading> (12s • esc to interrupt) · <activity>`, and a heading that
/// swallowed the interrupt hint would hide the one thing a person needs while
/// a turn runs. codex has no cap: its headings are titles the model wrote.
/// Measured in display columns, not characters — a Korean sentence of
/// forty-eight characters is ninety-six columns wide.
pub const LIVE_HEADING_MAX_COLUMNS: usize = 48;

/// The reasoning text kept for the heading scan. A sentence closes long
/// before this, and once one has, only the tail is read — so the buffer is
/// halved from the front whenever it grows past this, and a block of any
/// length costs a bounded scan per delta.
pub const SCAN_KEEP_BYTES: usize = 4096;

/// The thinking text kept for [`live_tail`]: its newest words. Two rows of
/// the widest pane hold fewer, and the buffer is halved from the front past
/// this, so a block of any length costs a bounded read per delta.
pub const TAIL_KEEP_BYTES: usize = 2048;

/// The rows under the working row a thinking block's newest words may take.
pub const TAIL_ROWS: usize = 2;

/// Body rows a thinking cell keeps in a bare terminal before `… +N lines` —
/// codex's tool-output preview cap, so the two previews read as one rule.
pub const THINKING_PREVIEW_ROWS: usize = super::tools::OUTPUT_MAX_ROWS;

/// The first `**bold**` heading in `text`, if one has closed yet.
///
/// codex's rule (`chatwidget.rs::extract_first_bold`) verbatim, including the
/// two refusals that matter while a stream is still arriving: an **unclosed**
/// `**` returns `None` so the header waits for more deltas instead of flashing
/// a half-written phrase, and an empty `****` returns `None` rather than
/// blanking the shimmer.
#[must_use]
pub fn first_bold_heading(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            let start = i + 2;
            let mut j = start;
            while j + 1 < bytes.len() {
                if bytes[j] == b'*' && bytes[j + 1] == b'*' {
                    let trimmed = text[start..j].trim();
                    return (!trimmed.is_empty()).then_some(trimmed);
                }
                j += 1;
            }
            // No closing marker yet — wait rather than guess.
            return None;
        }
        i += 1;
    }
    None
}

/// codex's heading only where codex's summaries put it: a `**bold**` run
/// that opens a line. An inline bold word inside Anthropic's prose
/// (`the **key** point is`) is emphasis, not the block's title, and must not
/// freeze the row on one word for the rest of the block.
#[must_use]
pub fn leading_bold_heading(text: &str) -> Option<&str> {
    let open = text.find("**")?;
    let line_start = text[..open].rfind('\n').map_or(0, |at| at + 1);
    if !text[line_start..open].trim().is_empty() {
        return None;
    }
    first_bold_heading(&text[open..])
}

/// The status word for a reasoning block so far, or `None` to keep the
/// current one.
///
/// 1. A closed `**heading**` opening a line wins — codex's own word for the
///    block ([`leading_bold_heading`]).
/// 2. Otherwise the last complete sentence: a run ended by a newline, by
///    `.`/`!`/`?` followed by whitespace (so `src/app.rs` and `v1.2` do not
///    end one), or by a CJK full stop. The first sentence is the heading
///    until a second one closes; from then on the newest complete sentence
///    is, so a long block keeps saying what it is doing now.
/// 3. A first sentence that has run past `max_columns` without ending is
///    cut there, at a word boundary, so a long opening still gets a word.
///
/// The word carries no terminator and no markdown noise (a list marker, a
/// heading hash, emphasis stars), and never exceeds `max_columns`.
#[must_use]
pub fn live_heading(text: &str, max_columns: usize) -> Option<String> {
    if let Some(bold) = leading_bold_heading(text) {
        return Some(fit(clean(bold), max_columns));
    }
    let mut last_complete: Option<&str> = None;
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let ends_sentence = match ch {
            '\n' | '。' | '！' | '？' => true,
            // A stop followed by whitespace, unless the run before it is a
            // bare number: `1. Inspect` is a list marker, not a sentence.
            '.' | '!' | '?' => {
                let run = text[start..index].trim_start();
                chars
                    .peek()
                    .is_none_or(|(_, next)| next.is_whitespace())
                    && !run.is_empty()
                    && !run.bytes().all(|byte| byte.is_ascii_digit())
            }
            _ => false,
        };
        if !ends_sentence {
            continue;
        }
        let segment = text[start..index].trim();
        if !segment.is_empty() {
            last_complete = Some(segment);
        }
        start = index + ch.len_utf8();
    }
    if let Some(sentence) = last_complete {
        let cleaned = clean(sentence);
        if !cleaned.is_empty() {
            return Some(fit(cleaned, max_columns));
        }
    }
    let open = clean(text[start..].trim());
    (open.width() > max_columns).then(|| fit(open, max_columns))
}

/// Strip the markdown a reasoning line opens with — `- `, `* `, `1. `, `# `,
/// `> ` — and the emphasis stars inside it, so the status word is prose.
fn clean(segment: &str) -> String {
    let mut rest = segment.trim();
    loop {
        let trimmed = rest
            .trim_start_matches(['#', '>', '-', '*', '•'])
            .trim_start();
        let numbered = trimmed
            .find(|ch: char| !ch.is_ascii_digit())
            .filter(|digits| *digits > 0)
            .and_then(|digits| trimmed[digits..].strip_prefix(". ").or_else(|| trimmed[digits..].strip_prefix(") ")))
            .map(str::trim_start);
        let next = numbered.unwrap_or(trimmed);
        if next == rest {
            break;
        }
        rest = next;
    }
    rest.replace("**", "").replace('`', "").trim().to_string()
}

/// `text` within `max_columns` display columns, cut at the last word boundary
/// that keeps at least half the room, with an ellipsis; unchanged when it
/// already fits.
fn fit(text: String, max_columns: usize) -> String {
    if text.width() <= max_columns {
        return text;
    }
    let room = max_columns.saturating_sub(1);
    let mut used = 0usize;
    let mut cut = 0usize;
    for (index, ch) in text.char_indices() {
        let width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4]) as &str);
        if used + width > room {
            break;
        }
        used += width;
        cut = index + ch.len_utf8();
    }
    let head = &text[..cut];
    let word_boundary = head
        .rfind(char::is_whitespace)
        .filter(|boundary| *boundary * 2 >= cut)
        .unwrap_or(cut);
    let mut out = head[..word_boundary].trim_end().to_string();
    out.push('…');
    out
}

/// The newest words of a thinking block that is still being written, as the
/// rows under the working row show them: prose on one line with the emphasis
/// marks taken out, no wider than `max_columns` display columns, led by an
/// ellipsis when earlier words were left out. `None` while nothing was said.
///
/// The heading on the working row says what the block is about; these are
/// its words as they come, so a model that reasons for half a minute before
/// its first word shows that it is doing so.
#[must_use]
pub fn live_tail(text: &str, max_columns: usize) -> Option<String> {
    let said = text.split_whitespace().collect::<Vec<_>>().join(" ").replace("**", "").replace('`', "");
    let said = said.trim();
    if said.is_empty() || max_columns < 2 {
        return None;
    }
    if said.width() <= max_columns {
        return Some(said.to_string());
    }
    // The newest columns that fit beside the ellipsis, from the end back.
    let room = max_columns - 1;
    let mut used = 0usize;
    let mut cut = said.len();
    for (index, ch) in said.char_indices().rev() {
        let width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0u8; 4]) as &str);
        if used + width > room {
            break;
        }
        used += width;
        cut = index;
    }
    let newest = &said[cut..];
    // From a word's start, when that keeps at least half of what fits.
    let from_a_word = newest
        .find(char::is_whitespace)
        .filter(|boundary| *boundary * 2 <= newest.len())
        .map_or(newest, |boundary| newest[boundary..].trim_start());
    Some(format!("…{from_a_word}"))
}

/// Halve `buffer` from the front, on a character boundary, once it grows past
/// `keep` bytes. The tail is where the newest sentence is.
pub fn bound_scan(buffer: &mut String, keep: usize) {
    if buffer.len() <= keep {
        return;
    }
    let mut cut = buffer.len() / 2;
    while !buffer.is_char_boundary(cut) {
        cut += 1;
    }
    buffer.drain(..cut);
}

/// The committed cell for a headerless thinking block: a dim italic
/// `• Thinking` title over the body [`super::cells::MarkdownStream::finish`]
/// rendered, previewed in a bare terminal and folded whole under a
/// `ZeroCode` pane's markers.
///
/// `body` is the stream's finished output: one leading blank row (the gap
/// between cells, kept outside the fold like a tool cell's) and the rendered
/// rows, the first carrying the `• ` marker the title takes over.
#[must_use]
pub fn cell(body: Vec<Line>, mode: FoldMode, ids: &mut FoldIds) -> Vec<Line> {
    let mut rows = body.into_iter().peekable();
    let mut out = Vec::new();
    if rows.peek().is_some_and(|row| row.spans.is_empty()) {
        out.push(rows.next().unwrap_or_default());
    }
    let mut rows: Vec<Line> = rows.collect();
    if rows.is_empty() {
        return Vec::new();
    }
    let glyphs = super::style::glyphs();
    if let Some(marker) = rows[0].spans.first_mut().filter(|span| span.text == glyphs.answer) {
        *marker = Span::raw("  ");
    }
    let title = Line::new(vec![
        Span::new(glyphs.thinking, palette::cell_marker()),
        Span::new(strings::THINKING, Style::new().dim().italic().bold()),
    ]);
    let mut cell = vec![title];
    if mode.markers_enabled() {
        // The pane folds the body: keep every row, teaser the count.
        let folded = rows.len();
        cell.push(teaser(folded));
        cell.append(&mut rows);
        out.extend(folds::wrap_cell_with_teaser(cell, 1, ids));
    } else {
        cell.extend(super::tools::limit_from_start(&rows, THINKING_PREVIEW_ROWS).into_iter().map(indent_marker));
        out.extend(cell);
    }
    out
}

/// A finished thinking block as one row: the gap every cell opens with, then
/// `✻ Thought for 4s · Reading the parser` — how long it took when that was a
/// second or more, and `about`: the words the working row showed for it, its
/// bold heading or its last whole sentence. A block that ended before the
/// working row had a word for it is read once more, `whole`, through the same
/// rule ([`live_heading`]).
#[must_use]
pub fn one_line(
    about: Option<&str>,
    whole: Option<&str>,
    took: Option<std::time::Duration>,
    width: usize,
) -> Vec<Line> {
    let read_whole = match about {
        Some(_) => None,
        None => whole.and_then(|text| live_heading(text, LIVE_HEADING_MAX_COLUMNS)),
    };
    let about = about.or(read_whole.as_deref());
    let glyphs = super::style::glyphs();
    let said = match took.map(|took| took.as_secs()).filter(|seconds| *seconds > 0) {
        Some(seconds) => format!("{} {}", strings::THOUGHT_FOR, super::shimmer::fmt_elapsed_compact(seconds)),
        None => strings::THOUGHT.to_string(),
    };
    let mut spans = vec![Span::dim(glyphs.thinking), Span::dim(said)];
    if let Some(about) = about.map(str::trim).filter(|about| !about.is_empty()) {
        spans.push(Span::dim(" · "));
        spans.push(Span::new(about.to_string(), Style::new().dim().italic()));
    }
    vec![Line::empty(), Line::new(spans).truncated(super::style::current().reading_width(width))]
}

/// `  … +N lines` under the title while the fold is closed — the tool
/// cell's teaser grammar, under an answer cell's indent.
fn teaser(folded_rows: usize) -> Line {
    Line::new(vec![Span::raw("  "), Span::dim(format!("… +{folded_rows} lines"))])
}

/// The bare-terminal elision row comes back flush left; give it the cell's
/// continuation indent.
fn indent_marker(row: Line) -> Line {
    if row.spans.len() == 1 && row.spans[0].text.starts_with("… +") {
        row.prefixed(Span::raw("  "))
    } else {
        row
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::{
        bound_scan, cell, first_bold_heading, leading_bold_heading, live_heading, live_tail,
        LIVE_HEADING_MAX_COLUMNS,
    };
    use crate::tui::ansi::Line;
    use crate::tui::folds::{FoldIds, FoldMode};

    /// codex takes the shimmer word from the first bold heading of the model's
    /// reasoning (`chatwidget.rs::extract_first_bold`). Two refusals matter
    /// while deltas are still arriving: an unclosed `**` waits instead of
    /// flashing half a phrase, and an empty `****` never blanks the shimmer.
    #[test]
    fn a_finished_block_stands_as_one_row() {
        use std::time::Duration;
        let rows = super::one_line(Some("Reading the parser"), None, Some(Duration::from_millis(2_400)), 80);
        assert_eq!(rows[0].plain(), "", "the gap every cell opens with");
        assert_eq!(rows[1].plain(), "• Thought for 2s · Reading the parser");
        let quick = super::one_line(Some("Checking the test"), None, Some(Duration::from_millis(300)), 80);
        assert_eq!(quick[1].plain(), "• Thought · Checking the test", "under a second says no seconds");
        assert_eq!(super::one_line(None, None, None, 80)[1].plain(), "• Thought");
        assert_eq!(super::one_line(Some("  "), None, None, 80)[1].plain(), "• Thought");
        // A block the working row never had a word for is read whole, by the
        // rule the working row reads with; the word it did show stands.
        let whole = "**Reading the parser**\n\nThe loop ends early.";
        assert_eq!(super::one_line(None, Some(whole), None, 80)[1].plain(), "• Thought · Reading the parser");
        assert_eq!(super::one_line(Some("Shown"), Some(whole), None, 80)[1].plain(), "• Thought · Shown");
    }

    #[test]
    fn the_shimmer_word_waits_for_a_closed_heading() {
        assert_eq!(first_bold_heading("**Checking the test**\nbody"), Some("Checking the test"));
        assert_eq!(first_bold_heading("intro **Second** and **Third**"), Some("Second"));
        assert_eq!(first_bold_heading("  **  padded  **"), Some("padded"));

        assert_eq!(first_bold_heading("**still writing the head"), None);
        assert_eq!(first_bold_heading("****"), None);
        assert_eq!(first_bold_heading("no markers at all"), None);
        assert_eq!(first_bold_heading(""), None);
    }

    /// A closed bold heading opening a line is the word, whatever sentences
    /// stand around it; a bold word inside a sentence is emphasis and the
    /// sentence rule keeps going.
    #[test]
    fn a_bold_heading_outranks_the_sentences_only_where_codex_puts_it() {
        assert_eq!(
            live_heading("Hmm.\n**Polishing tool display**\nThen more.", LIVE_HEADING_MAX_COLUMNS),
            Some("Polishing tool display".to_string())
        );
        assert_eq!(leading_bold_heading("**Title**\nbody"), Some("Title"));
        assert_eq!(leading_bold_heading("  **Title** body"), Some("Title"));
        assert_eq!(leading_bold_heading("the **key** point"), None);
        assert_eq!(leading_bold_heading("**still open"), None);
        assert_eq!(
            live_heading("The **key** point is the cache. Next", LIVE_HEADING_MAX_COLUMNS),
            Some("The key point is the cache".to_string())
        );
    }

    /// Anthropic thinking: nothing until the first sentence ends, then that
    /// sentence, then the newest complete one — a half-written sentence never
    /// flashes on the row.
    #[test]
    fn headerless_thinking_says_its_newest_complete_sentence() {
        let max = LIVE_HEADING_MAX_COLUMNS;
        assert_eq!(live_heading("The user wants me to", max), None);
        assert_eq!(
            live_heading("The user wants me to read the test. Let me", max),
            Some("The user wants me to read the test".to_string())
        );
        assert_eq!(
            live_heading("The user wants me to read the test. Let me open src/app.rs first.\nNow", max),
            Some("Let me open src/app.rs first".to_string())
        );
        // A newline closes a sentence too; the markdown a line opens with
        // does not reach the row.
        assert_eq!(
            live_heading("- **Plan**: read the failing test\n", max),
            Some("Plan: read the failing test".to_string())
        );
        assert_eq!(live_heading("1. Inspect the wiring\n2. Fix", max), Some("Inspect the wiring".to_string()));
        // A dot inside a path or a version is not a full stop.
        assert_eq!(live_heading("Reading src/app.rs and v1.2 now", max), None);
    }

    /// Korean sentences end the same way, and the cap is columns, not
    /// characters: a wide sentence is cut at a word so the interrupt hint
    /// keeps its place.
    #[test]
    fn korean_sentences_close_and_wide_text_is_cut_by_columns() {
        let max = LIVE_HEADING_MAX_COLUMNS;
        assert_eq!(
            live_heading("먼저 실패하는 시험을 읽어야 한다. 그다음", max),
            Some("먼저 실패하는 시험을 읽어야 한다".to_string())
        );
        assert_eq!(
            live_heading("파일을 읽고 있다。다음은", max),
            Some("파일을 읽고 있다".to_string())
        );
        let long = "이 문장은 아주 길어서 상태 줄의 인터럽트 힌트를 밀어낼 만큼 넓은 폭을 차지하므로 잘려야 한다.";
        let word = live_heading(long, max).expect("a long sentence still yields a word");
        assert!(unicode_width::UnicodeWidthStr::width(word.as_str()) <= max, "{word}");
        assert!(word.ends_with('…'), "{word}");
    }

    /// A first sentence that runs past the cap without ending is not left as
    /// `Working` — it is cut at a word boundary — and a short open sentence
    /// still waits.
    #[test]
    fn a_long_open_sentence_is_cut_rather_than_withheld() {
        let max = 20;
        assert_eq!(live_heading("short and still open", max), None);
        assert_eq!(
            live_heading("this opening sentence goes on well past the cap without a stop", max),
            Some("this opening…".to_string())
        );
    }

    #[test]
    fn the_scan_buffer_is_bounded_from_the_front() {
        let mut buffer = "가나다라".repeat(100);
        let before = buffer.len();
        bound_scan(&mut buffer, 64);
        assert!(buffer.len() < before && buffer.len() <= before / 2 + 3);
        assert!(buffer.starts_with('가') || buffer.starts_with('나') || buffer.starts_with('다') || buffer.starts_with('라'));
        let mut small = "abc".to_string();
        bound_scan(&mut small, 64);
        assert_eq!(small, "abc");
    }

    fn body(rows: usize) -> Vec<Line> {
        let mut out = vec![Line::empty()];
        for index in 0..rows {
            let prefix = if index == 0 { "• " } else { "  " };
            out.push(Line::new(vec![
                crate::tui::ansi::Span::raw(prefix),
                crate::tui::ansi::Span::raw(format!("row {index}")),
            ]));
        }
        out
    }

    /// A bare terminal gets the title, the preview rows and the elision
    /// count; the blank gap stays above the cell.
    #[test]
    fn a_bare_terminal_previews_the_thinking_under_a_title() {
        let mut ids = FoldIds::default();
        let rows: Vec<String> = cell(body(8), FoldMode::Bare, &mut ids).iter().map(Line::plain).collect();
        assert_eq!(
            rows,
            vec![
                String::new(),
                "• Thinking".to_string(),
                "  row 0".to_string(),
                "  row 1".to_string(),
                "  row 2".to_string(),
                "  row 3".to_string(),
                "  row 4".to_string(),
                "  … +3 lines".to_string(),
            ]
        );
        assert_eq!(ids.mint(), 0, "no fold id is spent without markers");
    }

    /// Under a pane the whole body is kept and folded, the teaser counting it.
    #[test]
    fn a_pane_folds_the_whole_body_under_the_title() {
        let mut ids = FoldIds::default();
        let lines = cell(body(8), FoldMode::Markers, &mut ids);
        let plain: Vec<String> = lines.iter().map(Line::plain).collect();
        assert_eq!(plain.len(), 1 + 1 + 1 + 8);
        assert_eq!(plain[1], "• Thinking");
        assert_eq!(plain[2], "  … +8 lines");
        assert_eq!(plain[10], "  row 7");
        assert!(lines[1].lead.as_deref().is_some_and(|lead| lead.contains("begin;0;collapsed,teaser=1;• Thinking")));
        assert!(lines[10].trail.as_deref().is_some_and(|trail| trail.contains("end;0")));
        assert_eq!(lines[0].lead, None, "the gap row stays outside the fold");
    }

    #[test]
    fn an_empty_body_makes_no_cell() {
        let mut ids = FoldIds::default();
        assert!(cell(vec![Line::empty()], FoldMode::Bare, &mut ids).is_empty());
        assert!(cell(Vec::new(), FoldMode::Markers, &mut ids).is_empty());
    }

    /// The newest words of a block still being written: all of them while
    /// they fit, and past that the newest, from a word's start, behind an
    /// ellipsis — never wider than the room, a Korean letter's two columns
    /// counted. Line breaks and emphasis marks are not words.
    #[test]
    fn the_live_tail_is_the_newest_words_that_fit() {
        assert_eq!(live_tail("", 40), None);
        assert_eq!(live_tail(" \n ", 40), None);
        assert_eq!(
            live_tail("I should read the parser first", 40).as_deref(),
            Some("I should read the parser first")
        );
        assert_eq!(
            live_tail("**Reading the parser**\n\nI should read `lex.rs`\nfirst", 60).as_deref(),
            Some("Reading the parser I should read lex.rs first")
        );

        let long = "first the lexer then the parser then the checker and at last the emitter";
        let tail = live_tail(long, 30).expect("words");
        assert!(tail.starts_with('…') && tail.ends_with("at last the emitter"), "{tail}");
        assert!(tail.width() <= 30, "{tail}");
        let newest = tail.trim_start_matches('…');
        assert!(long.contains(&format!(" {newest}")), "it starts at a word: {tail}");

        let korean = "먼저 파서를 읽고 그다음 검사기를 본 뒤에 마지막으로 출력기를 고친다";
        let tail = live_tail(korean, 24).expect("words");
        assert!(tail.width() <= 24, "{tail} is {} columns", tail.width());
        assert!(tail.starts_with('…') && tail.ends_with("출력기를 고친다"), "{tail}");
    }
}
