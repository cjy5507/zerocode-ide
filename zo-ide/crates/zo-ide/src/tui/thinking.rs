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

/// The thinking text kept for [`live_line`]: its newest words, which hold the
/// sentence being written. The buffer is halved from the front past this, so
/// a block of any length costs a bounded read per delta.
pub const TAIL_KEEP_BYTES: usize = 2048;

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
    let (last_complete, open) = sentences(text);
    if let Some(sentence) = last_complete {
        let cleaned = clean(sentence);
        if !cleaned.is_empty() {
            return Some(fit(cleaned, max_columns));
        }
    }
    let open = clean(open.trim());
    (open.width() > max_columns).then(|| fit(open, max_columns))
}

/// The newest complete sentence of `text` and what follows it — the
/// sentence still being written. A sentence ends at a newline, at `.`/`!`/`?`
/// followed by whitespace or by the end of the text (so `src/app.rs` and
/// `v1.2` do not end one, and a run that is a bare number — `1. Inspect` —
/// is a list marker, not a sentence), or at a CJK full stop. Neither part
/// holds its terminator.
fn sentences(text: &str) -> (Option<&str>, &str) {
    let mut last_complete: Option<&str> = None;
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((index, ch)) = chars.next() {
        let ends_sentence = match ch {
            '\n' | '。' | '！' | '？' => true,
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
    (last_complete, &text[start..])
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

/// The sentence a thinking block is writing now, as the row under the working
/// row shows it: from its first word, prose on one row with the markdown
/// taken out, no wider than `max_columns` display columns — cut at its end,
/// at a word, with an ellipsis. So the row fills as the sentence is written
/// and then holds still until the next sentence begins; until that one has a
/// word, the sentence just finished stands, and the row never empties
/// between two. `None` while nothing was said.
///
/// The row once showed the block's newest words over two rows (zo 1.1.143),
/// and every delta moved each of them a word along and changed how many rows
/// they took — the work in progress jumped as the model typed (the person,
/// 2026-11-18: 「working부분이 정신없게 바뀜」). The heading on the working row
/// says what the block is about; this row says what it is saying now, and
/// changes once a sentence.
#[must_use]
pub fn live_line(text: &str, max_columns: usize) -> Option<String> {
    if max_columns < 2 {
        return None;
    }
    let (last_complete, open) = sentences(text);
    let said = |segment: &str| {
        let words = clean(segment).split_whitespace().collect::<Vec<_>>().join(" ");
        (!words.is_empty()).then_some(words)
    };
    said(open)
        .or_else(|| last_complete.and_then(said))
        .map(|words| fit(words, max_columns))
}

/// Like [`live_line`], but drops whole-line `**heading**` lines (and an
/// unclosed `**heading` still being written on the last line) because the
/// working row above already displays the heading in the marks look.
#[must_use]
pub fn live_line_without_headings(text: &str, max_columns: usize) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let last = lines.len().saturating_sub(1);
    let body: Vec<&str> = lines
        .into_iter()
        .enumerate()
        .filter(|&(index, line)| !(is_heading_line(line) || (index == last && is_open_heading_line(line))))
        .map(|(_, line)| line)
        .collect();
    if body.is_empty() {
        return None;
    }
    live_line(&body.join("\n"), max_columns)
}

fn is_heading_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed
        .strip_prefix("**")
        .and_then(|rest| rest.strip_suffix("**"))
        .is_some_and(|inner| !inner.trim().is_empty() && !inner.contains("**"))
}

fn is_open_heading_line(line: &str) -> bool {
    line.trim()
        .strip_prefix("**")
        .is_some_and(|rest| !rest.contains("**"))
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
    // One row, cut and not wrapped: at the pane's edge, not the prose column.
    vec![Line::empty(), Line::new(spans).truncated(width)]
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

/// Whether `after`, the thinking row drawn next, still says the sentence
/// `before` said: it grew out of it, or it is the one cut that turns its
/// last word into an ellipsis once the sentence outruns the row. Anything
/// else is the next sentence — which can begin with the letters the last one
/// began with (`The` after `Then I will…`), so a shared start alone is not
/// the same sentence.
#[cfg(test)]
pub(crate) fn same_sentence(before: &str, after: &str) -> bool {
    after.starts_with(before)
        || (after.ends_with('…') && before.starts_with(after.trim_end_matches('…')))
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::{
        bound_scan, cell, first_bold_heading, leading_bold_heading, live_heading, live_line,
        live_line_without_headings, same_sentence, LIVE_HEADING_MAX_COLUMNS,
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

    /// The sentence a block is writing, from its first word: all of it while
    /// it fits, and past that its start, cut at a word behind an ellipsis —
    /// never wider than the room, a Korean letter's two columns counted. A
    /// sentence's end starts the next one; until that has a word the one just
    /// finished stands. Line breaks end a sentence; emphasis and code marks
    /// are not words.
    #[test]
    fn the_live_line_is_the_sentence_being_written_from_its_start() {
        assert_eq!(live_line("", 40), None);
        assert_eq!(live_line(" \n ", 40), None);
        assert_eq!(
            live_line("I should read the parser first", 40).as_deref(),
            Some("I should read the parser first")
        );
        // The sentence being written, not the ones before it.
        assert_eq!(
            live_line("I read the lexer. Then the par", 40).as_deref(),
            Some("Then the par")
        );
        assert_eq!(
            live_line("**Reading the parser**\n\nI should read `lex.rs`\nfirst", 60).as_deref(),
            Some("first")
        );
        assert_eq!(
            live_line("I should read `lex.rs` and  then\tthe   parser", 60).as_deref(),
            Some("I should read lex.rs and then the parser")
        );
        // A sentence just ended and the next has no word yet: it stands.
        assert_eq!(live_line("I read the lexer. ", 40).as_deref(), Some("I read the lexer"));
        assert_eq!(live_line("I read the lexer.", 40).as_deref(), Some("I read the lexer"));
        assert_eq!(live_line("I read the lexer.\n\n", 40).as_deref(), Some("I read the lexer"));

        // Past the room: the sentence's start, cut at a word, and the words
        // that come after do not move it.
        let long = "first the lexer then the parser then the checker and at last the emitter";
        let line = live_line(long, 30).expect("words");
        assert!(line.starts_with("first the lexer") && line.ends_with('…'), "{line}");
        assert!(line.width() <= 30, "{line}");
        let shown = line.trim_end_matches('…');
        assert!(long.starts_with(shown) && long[shown.len()..].starts_with(' '), "it ends at a word: {line}");
        for more in [" and then", " and then the tests", " and then the tests again"] {
            assert_eq!(live_line(&format!("{long}{more}"), 30).as_deref(), Some(line.as_str()), "{more}");
        }

        let korean = "먼저 파서를 읽고 그다음 검사기를 본 뒤에 마지막으로 출력기를 고친다";
        let line = live_line(korean, 24).expect("words");
        assert!(line.width() <= 24, "{line} is {} columns", line.width());
        assert!(line.starts_with("먼저 파서를") && line.ends_with('…'), "{line}");
        assert_eq!(live_line("파서를 읽었다。그다음 검사기", 40).as_deref(), Some("그다음 검사기"));
    }

    /// While the words come the row grows — a word at its end — until the
    /// sentence outruns it; then, once, its last word gives way to the
    /// ellipsis, and the row holds until the next sentence replaces it. Fed a
    /// character at a time, two sentences replace the row once.
    #[test]
    fn the_live_line_grows_and_holds_and_changes_once_a_sentence() {
        let text = "I read the lexer and saw that the tokens carry their spans all the way through. \
                    Then I will check the parser for the same thing before I change it.";
        let mut said = String::new();
        let mut rows: Vec<String> = Vec::new();
        for ch in text.chars() {
            said.push(ch);
            if let Some(row) = live_line(&said, 40) {
                if rows.last() != Some(&row) {
                    rows.push(row);
                }
            }
        }
        assert!(rows.iter().all(|row| row.width() <= 40), "{rows:#?}");
        let sentences = sentence_runs(&rows);
        assert_eq!(sentences.len(), 2, "the row is replaced once, when the second sentence begins: {rows:#?}");
        for run in sentences {
            let cut = run.iter().position(|row| row.ends_with('…')).expect("each sentence outruns 40 columns");
            assert_eq!(cut, run.len() - 1, "once cut the row holds: {run:#?}");
            for pair in run[..cut].windows(2) {
                assert!(pair[1].starts_with(pair[0].as_str()), "the row only grows: {:?} → {:?}", pair[0], pair[1]);
            }
        }
    }

    /// Rows that followed one another, split where one replaced the other —
    /// where a row is not [`same_sentence`] as the one before it.
    fn sentence_runs(rows: &[String]) -> Vec<Vec<String>> {
        let mut runs: Vec<Vec<String>> = Vec::new();
        for row in rows {
            let replaces = runs
                .last()
                .and_then(|run| run.last())
                .is_none_or(|before| !same_sentence(before, row));
            if replaces {
                runs.push(Vec::new());
            }
            runs.last_mut().expect("a run").push(row.clone());
        }
        runs
    }

    #[test]
    fn live_line_without_headings_drops_heading_lines_and_keeps_inline_bold() {
        assert_eq!(
            live_line_without_headings(
                "**Reading the parser**\n\nThe person says settings are not read.",
                80,
            )
            .as_deref(),
            Some("The person says settings are not read")
        );
        assert_eq!(live_line_without_headings("**Reading the parser**", 80), None);
        assert_eq!(
            live_line_without_headings(
                "**Reading the parser**\n\nFirst part.\n\n**Checking the lexer**\n\nSecond part",
                80,
            )
            .as_deref(),
            Some("Second part")
        );
        assert_eq!(
            live_line_without_headings("문장 안의 **강조**는 남는다", 80).as_deref(),
            Some("문장 안의 강조는 남는다")
        );
        assert_eq!(live_line_without_headings("**Reading the par", 80), None);
        assert_eq!(
            live_line_without_headings("앞 문장.\n\n**Checking the le", 80).as_deref(),
            Some("앞 문장")
        );
        assert_eq!(
            live_line_without_headings("**Note** 이어지는 글", 80).as_deref(),
            Some("Note 이어지는 글")
        );
    }
}
