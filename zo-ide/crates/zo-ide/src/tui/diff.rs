//! 통합 diff 를 줄번호·부호·색칠한 배경으로 그린다.
//!
//! 정본은 codex `tui/src/diff_render.rs` 다. 한 줄의 모양이 그 파일의
//! `push_wrapped_diff_line_inner_with_theme_and_color_level` 그대로다:
//!
//! ```text
//! <줄번호 오른쪽 맞춤><공백><부호><본문>
//! <줄번호 폭만큼 공백><공백 둘><이어지는 본문>
//! ```
//!
//! 부호는 `+`·`-`·` ` 이고, 줄 전체에 배경 tint 가 깔린다. 이어지는 줄에는
//! 부호가 없고 두 칸 들여쓴다 — 번호가 두 번 나오면 그 번호가 거짓이 된다.
//!
//! **배경 조회 결정.** 이제 화면 전체의 ANSI writer와 diff가 같은
//! [`TerminalPalette`]를 받으므로 OSC 10/11 조회를 startup에 한 번 도입한다.
//! redraw/행마다 조회하지 않고 100ms 공통 기한으로 제한하며, 실패하면 `None`을
//! 주입한다. `None`은 이전과 같은 dark + 환경 기반 깊이이므로 조용한 터미널의
//! 골든 바이트는 그대로다. ANSI-16은 Codex처럼 전경만 쓴다.

use runtime::message_stream::{DiffHunk, DiffLineKind, DiffView};

use super::ansi::{Color, Line, Span, Style};
use super::highlight::{exceeds_highlight_limits, highlight_block_to_lines};
use super::palette::{self, ColorLevel, TerminalPalette};

// codex `DARK_TC_*` / `LIGHT_TC_*` palettes. The startup background chooses
// one table for the whole render pass; a missing reply keeps the old Dark row.
const DARK_TC_ADD_LINE_BG: Color = Color::Rgb(33, 58, 43);
const DARK_TC_DEL_LINE_BG: Color = Color::Rgb(74, 34, 29);
const LIGHT_TC_ADD_LINE_BG: Color = Color::Rgb(218, 251, 225);
const LIGHT_TC_DEL_LINE_BG: Color = Color::Rgb(255, 235, 233);
const DARK_256_ADD_LINE_BG: Color = Color::Indexed(22);
const DARK_256_DEL_LINE_BG: Color = Color::Indexed(52);
const LIGHT_256_ADD_LINE_BG: Color = Color::Indexed(194);
const LIGHT_256_DEL_LINE_BG: Color = Color::Indexed(224);
// The changed words inside a changed line: the line's tint, one step stronger.
const DARK_TC_ADD_WORD_BG: Color = Color::Rgb(44, 105, 64);
const DARK_TC_DEL_WORD_BG: Color = Color::Rgb(122, 46, 38);
const LIGHT_TC_ADD_WORD_BG: Color = Color::Rgb(172, 238, 187);
const LIGHT_TC_DEL_WORD_BG: Color = Color::Rgb(255, 206, 203);
const DARK_256_ADD_WORD_BG: Color = Color::Indexed(28);
const DARK_256_DEL_WORD_BG: Color = Color::Indexed(88);
const LIGHT_256_ADD_WORD_BG: Color = Color::Indexed(157);
const LIGHT_256_DEL_WORD_BG: Color = Color::Indexed(217);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiffTheme { Dark, Light }

#[derive(Clone, Copy)]
struct DiffStyles {
    theme: DiffTheme,
    color_level: ColorLevel,
}

fn diff_theme() -> DiffTheme {
    // See the module documentation: querying OSC 11 here would make only one
    // part of zo adaptive.  Dark is the same safe fallback codex uses when a
    // terminal does not reply.
    diff_theme_for_bg(None)
}

fn diff_theme_for_bg(background: Option<(u8, u8, u8)>) -> DiffTheme {
    let Some((red, green, blue)) = background else { return DiffTheme::Dark; };
    // Kept as a pure policy helper for the eventual whole-screen palette
    // migration; production passes None until that migration exists.
    if u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114 > 128_000 {
        DiffTheme::Light
    } else {
        DiffTheme::Dark
    }
}

fn diff_styles(terminal_palette: Option<TerminalPalette>) -> DiffStyles {
    terminal_palette.map_or_else(
        || DiffStyles {
            theme: diff_theme(),
            color_level: palette::color_level_from_env(),
        },
        |terminal_palette| DiffStyles {
            theme: diff_theme_for_bg(Some(terminal_palette.background())),
            color_level: terminal_palette.color_level(),
        },
    )
}

fn line_background(kind: DiffLineKind, theme: DiffTheme, level: ColorLevel) -> Option<Color> {
    match (kind, theme, level) {
        (DiffLineKind::Added, DiffTheme::Dark, ColorLevel::TrueColor) => Some(DARK_TC_ADD_LINE_BG),
        (DiffLineKind::Removed, DiffTheme::Dark, ColorLevel::TrueColor) => Some(DARK_TC_DEL_LINE_BG),
        (DiffLineKind::Added, DiffTheme::Light, ColorLevel::TrueColor) => Some(LIGHT_TC_ADD_LINE_BG),
        (DiffLineKind::Removed, DiffTheme::Light, ColorLevel::TrueColor) => Some(LIGHT_TC_DEL_LINE_BG),
        (DiffLineKind::Added, DiffTheme::Dark, ColorLevel::Ansi256) => Some(DARK_256_ADD_LINE_BG),
        (DiffLineKind::Removed, DiffTheme::Dark, ColorLevel::Ansi256) => Some(DARK_256_DEL_LINE_BG),
        (DiffLineKind::Added, DiffTheme::Light, ColorLevel::Ansi256) => Some(LIGHT_256_ADD_LINE_BG),
        (DiffLineKind::Removed, DiffTheme::Light, ColorLevel::Ansi256) => Some(LIGHT_256_DEL_LINE_BG),
        _ => None, // ANSI-16 is deliberately foreground-only.
    }
}

fn word_background(kind: DiffLineKind, theme: DiffTheme, level: ColorLevel) -> Option<Color> {
    match (kind, theme, level) {
        (DiffLineKind::Added, DiffTheme::Dark, ColorLevel::TrueColor) => Some(DARK_TC_ADD_WORD_BG),
        (DiffLineKind::Removed, DiffTheme::Dark, ColorLevel::TrueColor) => Some(DARK_TC_DEL_WORD_BG),
        (DiffLineKind::Added, DiffTheme::Light, ColorLevel::TrueColor) => Some(LIGHT_TC_ADD_WORD_BG),
        (DiffLineKind::Removed, DiffTheme::Light, ColorLevel::TrueColor) => Some(LIGHT_TC_DEL_WORD_BG),
        (DiffLineKind::Added, DiffTheme::Dark, ColorLevel::Ansi256) => Some(DARK_256_ADD_WORD_BG),
        (DiffLineKind::Removed, DiffTheme::Dark, ColorLevel::Ansi256) => Some(DARK_256_DEL_WORD_BG),
        (DiffLineKind::Added, DiffTheme::Light, ColorLevel::Ansi256) => Some(LIGHT_256_ADD_WORD_BG),
        (DiffLineKind::Removed, DiffTheme::Light, ColorLevel::Ansi256) => Some(LIGHT_256_DEL_WORD_BG),
        _ => None,
    }
}

fn sign_style(kind: DiffLineKind, background: Option<Color>) -> Style {
    let mut style = match kind {
        DiffLineKind::Added => Style::new().fg(Color::GREEN),
        DiffLineKind::Removed => Style::new().fg(Color::RED),
        DiffLineKind::Context => Style::new(),
    };
    if let Some(background) = background { style = style.bg(background); }
    style
}

fn content_style(kind: DiffLineKind, background: Option<Color>) -> Style {
    sign_style(kind, background)
}

/// 한 diff 셀이 그리는 최대 줄 수.
///
/// 자율 실행이 파일을 크게 고치면 diff 하나가 화면을 통째로 먹는다. codex 도
/// 큰 패치에서는 하이라이팅을 끄고(`exceeds_highlight_limits`) 양을 줄인다 —
/// 우리는 줄 수로 자르고 몇 줄을 접었는지 말한다. 잘린 것을 조용히 감추는
/// 것이 가장 나쁘다.
const MAX_RENDERED_LINES: usize = 120;

/// `hunks` 를 화면 줄로. 빈 diff 는 빈 벡터다.
#[must_use]
pub fn hunk_lines(view: &DiffView, width: usize) -> Vec<Line> {
    hunk_lines_for_palette(view, width, palette::terminal_palette())
}

/// 명시한 팔레트로 `hunks`를 화면 줄로 만든다.
///
/// production과 골든이 같은 순수 정책을 쓰는 주입 이음매다. `None`이면 기존
/// dark 테마와 환경 기반 색 깊이를 그대로 써 조회 실패가 바이트를 바꾸지 않는다.
#[must_use]
pub fn hunk_lines_for_palette(
    view: &DiffView,
    width: usize,
    terminal_palette: Option<TerminalPalette>,
) -> Vec<Line> {
    let gutter = gutter_width(&view.hunks);
    let total_bytes = view.hunks.iter().flat_map(|hunk| &hunk.lines).map(|line| line.text.len()).sum();
    let total_lines = view.hunks.iter().map(|hunk| hunk.lines.len()).sum();
    let language = (!exceeds_highlight_limits(total_bytes, total_lines))
        .then_some(view.language.as_deref())
        .flatten();
    let styles = diff_styles(terminal_palette);
    let mut out = Vec::new();
    let mut folded = 0usize;
    let mut previous_end: Option<(usize, usize)> = None;
    for hunk in &view.hunks {
        if let Some((old_end, new_end)) = previous_end {
            let skipped = (hunk.old_start as usize).saturating_sub(old_end)
                .min((hunk.new_start as usize).saturating_sub(new_end));
            if skipped > 0 && out.len() < MAX_RENDERED_LINES {
                out.push(Line::new(vec![Span::dim(format!(
                    "{:cell$}{}",
                    "",
                    super::strings::unchanged_lines(skipped),
                    cell = numbers_width(gutter)
                ))]));
            }
        }
        previous_end = Some(hunk_end(hunk));
        let highlighted = language.and_then(|language| hunk_highlights(hunk, language));
        let emphasis = changed_words(hunk);
        for (index, (old, new, kind, text)) in numbered_lines(hunk).into_iter().enumerate() {
            if out.len() >= MAX_RENDERED_LINES {
                folded += 1;
                continue;
            }
            out.extend(rendered_line(
                (old, new),
                kind,
                text,
                highlighted.as_ref().and_then(|lines| lines.get(index)),
                emphasis[index],
                width,
                gutter,
                styles,
            ));
        }
    }
    if folded > 0 {
        out.push(Line::new(vec![Span::dim(format!(
            "{:cell$}… {folded} more line{}",
            "",
            if folded == 1 { "" } else { "s" },
            cell = numbers_width(gutter)
        ))]));
    }
    out
}

/// Both number columns and the space after them: `old  new `.
fn numbers_width(gutter: usize) -> usize {
    gutter * 2 + 3
}

/// The first old and new line numbers after a hunk.
fn hunk_end(hunk: &DiffHunk) -> (usize, usize) {
    let mut old = hunk.old_start as usize;
    let mut new = hunk.new_start as usize;
    for line in &hunk.lines {
        match line.kind {
            DiffLineKind::Added => new += 1,
            DiffLineKind::Removed => old += 1,
            DiffLineKind::Context => { old += 1; new += 1; }
        }
    }
    (old, new)
}

/// What changed inside each changed line, as a byte range of its text: a run
/// of removed lines followed by a run of added ones pairs up row by row, and a
/// pair's changed stretch is the words between their common head and tail. A
/// pair sharing neither is a rewrite, and a rewrite points at nothing.
fn changed_words(hunk: &DiffHunk) -> Vec<Option<(usize, usize)>> {
    let lines = &hunk.lines;
    let mut out = vec![None; lines.len()];
    let mut index = 0usize;
    while index < lines.len() {
        let removed_start = index;
        while index < lines.len() && lines[index].kind == DiffLineKind::Removed { index += 1; }
        let added_start = index;
        while index < lines.len() && lines[index].kind == DiffLineKind::Added { index += 1; }
        if removed_start == added_start || added_start == index {
            index = index.max(removed_start + 1);
            continue;
        }
        let pairs = (added_start - removed_start).min(index - added_start);
        for offset in 0..pairs {
            let (old_row, new_row) = (removed_start + offset, added_start + offset);
            if let Some((old, new)) = word_change(&lines[old_row].text, &lines[new_row].text) {
                out[old_row] = old;
                out[new_row] = new;
            }
        }
    }
    out
}

type WordRange = Option<(usize, usize)>;

fn word_change(old: &str, new: &str) -> Option<(WordRange, WordRange)> {
    let old_words = word_bounds(old);
    let new_words = word_bounds(new);
    let same = |a: &(usize, usize), b: &(usize, usize)| old[a.0..a.1] == new[b.0..b.1];
    let head = old_words.iter().zip(&new_words).take_while(|(a, b)| same(a, b)).count();
    let tail = old_words[head..].iter().rev()
        .zip(new_words[head..].iter().rev())
        .take_while(|(a, b)| same(a, b))
        .count();
    if head == 0 && tail == 0 {
        return None;
    }
    let span = |words: &[(usize, usize)]| -> WordRange {
        Some((words.first()?.0, words.last()?.1))
    };
    Some((
        span(&old_words[head..old_words.len() - tail]),
        span(&new_words[head..new_words.len() - tail]),
    ))
}

/// A line's words as byte ranges: a run of letters, digits and `_`, a run of
/// whitespace, or any other single character.
fn word_bounds(text: &str) -> Vec<(usize, usize)> {
    let class = |ch: char| {
        if ch.is_alphanumeric() || ch == '_' { 1 } else if ch.is_whitespace() { 2 } else { 0 }
    };
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut previous = None;
    for (at, ch) in text.char_indices() {
        let kind = class(ch);
        let end = at + ch.len_utf8();
        match out.last_mut() {
            Some(last) if kind != 0 && previous == Some(kind) => last.1 = end,
            _ => out.push((at, end)),
        }
        previous = Some(kind);
    }
    out
}

/// Paints `range` of the row's text with `background`, splitting the spans at
/// its edges, over whatever tint the spans already carry.
fn emphasize(spans: Vec<Span>, range: (usize, usize), background: Color) -> Vec<Span> {
    let mut out = Vec::with_capacity(spans.len() + 2);
    let mut at = 0usize;
    for span in spans {
        let (start, end) = (at, at + span.text.len());
        at = end;
        let from = range.0.clamp(start, end) - start;
        let to = range.1.clamp(start, end) - start;
        if from == to || !span.text.is_char_boundary(from) || !span.text.is_char_boundary(to) {
            out.push(span);
            continue;
        }
        let (before, rest) = span.text.split_at(from);
        let (inside, after) = rest.split_at(to - from);
        for (piece, style) in [(before, span.style), (inside, span.style.bg(background)), (after, span.style)] {
            if !piece.is_empty() { out.push(Span::new(piece.to_string(), style)); }
        }
    }
    out
}

/// 줄번호 한 칸의 폭 — 옛 번호와 새 번호 중 가장 큰 것, 최소 1.
fn gutter_width(hunks: &[DiffHunk]) -> usize {
    let widest = hunks
        .iter()
        .flat_map(|hunk| numbered_lines(hunk).into_iter().flat_map(|(old, new, _, _)| old.into_iter().chain(new)))
        .max()
        .unwrap_or(0);
    if widest == 0 {
        1
    } else {
        widest.to_string().len()
    }
}

/// Highlight one hunk as **two** sources: the file as it was, and as it is.
///
/// Joining every row into one source — context, removed, and added together —
/// is what breaks. An edit that touches a quote makes the parser open the
/// string twice, and the corruption then runs to the end of the hunk. Measured
/// on a real edit: the added row came back with code in string color and the
/// string in code color, and the three context rows after it were wrong too.
/// A hunk is not one text; it is two texts interleaved.
///
/// So each row is highlighted inside the file it actually belongs to — removed
/// rows with the old projection, added rows with the new one, context rows with
/// the new (they are the same in both, and the new one is the file that exists
/// now). Parser state is still carried across the rows of each projection,
/// which is the property the single-source version was reaching for.
///
/// Returns `None` when either projection fails to line up, so a surprise leaves
/// the diff uncolored rather than miscolored.
fn hunk_highlights(hunk: &DiffHunk, language: &str) -> Option<Vec<Line>> {
    fn project_without(hunk: &DiffHunk, omitted: DiffLineKind) -> (String, Vec<usize>) {
        let mut hunk_rows = Vec::new();
        let mut source = String::new();
        for (hunk_row, line) in hunk.lines.iter().enumerate() {
            if line.kind == omitted {
                continue;
            }
            if !hunk_rows.is_empty() {
                source.push('\n');
            }
            source.push_str(&line.text);
            hunk_rows.push(hunk_row);
        }
        (source, hunk_rows)
    }

    fn highlight_projection(
        source: &str,
        projected_rows: usize,
        language: &str,
    ) -> Option<Vec<Line>> {
        if projected_rows == 0 {
            return Some(Vec::new());
        }
        let mut lines = highlight_block_to_lines(source, language)?;
        // syntect's `LinesWithEndings` deliberately treats a terminal newline
        // as the ending of the preceding line, not an additional empty line.
        // In a diff, though, the final empty `DiffLine` is a real row. Restore
        // that known row before checking the projection so one blank addition
        // cannot silently discard syntax highlighting for the whole hunk.
        if source.ends_with('\n') && lines.len() + 1 == projected_rows {
            lines.push(Line::empty());
        }
        (lines.len() == projected_rows).then_some(lines)
    }

    let (old_source, old_hunk_rows) = project_without(hunk, DiffLineKind::Added);
    let (new_source, new_hunk_rows) = project_without(hunk, DiffLineKind::Removed);
    let old_lines = highlight_projection(&old_source, old_hunk_rows.len(), language)?;
    let new_lines = highlight_projection(&new_source, new_hunk_rows.len(), language)?;

    let mut highlighted = vec![Line::empty(); hunk.lines.len()];
    for (line, hunk_row) in old_lines.into_iter().zip(old_hunk_rows) {
        if hunk.lines[hunk_row].kind == DiffLineKind::Removed {
            highlighted[hunk_row] = line;
        }
    }
    // Context rows appear in both projections; the new one wins so the colors
    // describe the file that exists now.
    for (line, hunk_row) in new_lines.into_iter().zip(new_hunk_rows) {
        highlighted[hunk_row] = line;
    }
    Some(highlighted)
}

/// 한 hunk 의 줄에 양쪽 번호를 매긴다: 삭제는 옛 번호만, 추가는 새 번호만,
/// 문맥 줄은 둘 다 달고 양쪽 커서를 함께 민다.
fn numbered_lines(hunk: &DiffHunk) -> Vec<(Option<usize>, Option<usize>, DiffLineKind, &str)> {
    let mut old_line = hunk.old_start as usize;
    let mut new_line = hunk.new_start as usize;
    let mut out = Vec::with_capacity(hunk.lines.len());
    for line in &hunk.lines {
        let (old, new) = match line.kind {
            DiffLineKind::Added => (None, Some(new_line)),
            DiffLineKind::Removed => (Some(old_line), None),
            DiffLineKind::Context => (Some(old_line), Some(new_line)),
        };
        if old.is_some() { old_line += 1; }
        if new.is_some() { new_line += 1; }
        out.push((old, new, line.kind, line.text.as_str()));
    }
    out
}

/// 한 줄 — 넘치면 이어지는 줄로 접힌다.
#[allow(clippy::too_many_arguments)]
fn rendered_line(
    (old, new): (Option<usize>, Option<usize>),
    kind: DiffLineKind,
    text: &str,
    syntax: Option<&Line>,
    emphasis: Option<(usize, usize)>,
    width: usize,
    gutter: usize,
    styles: DiffStyles,
) -> Vec<Line> {
    let sign = match kind {
        DiffLineKind::Added => '+', DiffLineKind::Removed => '-', DiffLineKind::Context => ' ',
    };
    let background = line_background(kind, styles.theme, styles.color_level);
    let sign_style = sign_style(kind, background);
    let style = content_style(kind, background);
    // 두 번호 칸 + 부호와 그 뒤 한 칸이 앞에 서므로 본문은 그만큼 좁다.
    let content_width = width.saturating_sub(numbers_width(gutter) + 2).max(1);
    let mut out = Vec::new();
    let mut spans = syntax.map_or_else(|| vec![Span::new(text.to_string(), style)], |line| {
        line.spans.iter().map(|span| {
            let mut style = span.style;
            if let Some(background) = background { style = style.bg(background); }
            if matches!(kind, DiffLineKind::Removed) { style = style.dim(); }
            Span::new(span.text.clone(), style)
        }).collect()
    });
    let word_bg = word_background(kind, styles.theme, styles.color_level);
    if let (Some(range), Some(word_bg)) = (emphasis, word_bg) {
        if spans.iter().map(|span| span.text.len()).sum::<usize>() == text.len() {
            spans = emphasize(spans, range, word_bg);
        }
    }
    for (index, chunk) in wrapped_spans(&spans, content_width).into_iter().enumerate() {
        let mut row = if index == 0 {
            let column = |number: Option<usize>| number.map_or_else(String::new, |number| number.to_string());
            vec![
                Span::new(format!("{:>gutter$}  {:>gutter$} ", column(old), column(new)), Style::new().dim()),
                Span::new(format!("{sign} "), sign_style),
            ]
        } else {
            // 이어지는 줄은 번호도 부호도 없다. 빈 칸이 번호 칸과 부호 자리를
            // 대신해 본문이 같은 열에서 이어지게 한다.
            vec![Span::new(format!("{:cell$}", "", cell = numbers_width(gutter) + 2), Style::new().dim())]
        };
        row.extend(chunk);
        out.push(Line::new(row));
    }
    out
}

fn wrapped_spans(spans: &[Span], width: usize) -> Vec<Vec<Span>> {
    let mut out = vec![Vec::new()];
    let mut used = 0usize;
    for span in spans {
        let mut piece = String::new();
        for ch in span.text.chars() {
            let cols = super::ansi::char_width(ch);
            if used + cols > width && !piece.is_empty() {
                out.last_mut().expect("one row").push(Span::new(std::mem::take(&mut piece), span.style));
                out.push(Vec::new());
                used = 0;
            }
            piece.push(ch);
            used += cols;
        }
        if !piece.is_empty() { out.last_mut().expect("one row").push(Span::new(piece, span.style)); }
    }
    out
}

/// 표시 폭 기준으로 자른다. 한 글자가 남은 칸보다 넓으면 그 **앞에서** 끊어
/// 항상 전진한다 — CJK 나 탭에서 멈추지 않기 위한 codex 의 규칙이다.
#[cfg(test)]
mod tests {
    use super::{Color, MAX_RENDERED_LINES, diff_styles, hunk_lines, line_background};
    use runtime::message_stream::{DiffHunk, DiffLine, DiffLineKind, DiffView};

    fn line(kind: DiffLineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            text: text.to_string(),
        }
    }

    fn view(hunks: Vec<DiffHunk>) -> DiffView {
        DiffView {
            old_path: Some("src/lib.rs".into()),
            new_path: Some("src/lib.rs".into()),
            language: Some("rust".into()),
            hunks,
        }
    }

    fn hunk(old_start: u32, new_start: u32, lines: Vec<DiffLine>) -> DiffHunk {
        DiffHunk {
            old_start,
            old_lines: 0,
            new_start,
            new_lines: 0,
            lines,
        }
    }

    /// codex numbers additions from the NEW file and deletions from the OLD
    /// one (`diff_render.rs`, the `max_line_number` walk). Getting that
    /// backwards points the reader at a line that does not hold the change.
    #[test]
    fn additions_count_in_the_new_file_and_deletions_in_the_old() {
        let lines = hunk_lines(
            &view(vec![hunk(
                100,
                200,
                vec![
                    line(DiffLineKind::Context, "kept"),
                    line(DiffLineKind::Removed, "gone"),
                    line(DiffLineKind::Added, "fresh"),
                    line(DiffLineKind::Context, "kept too"),
                ],
            )]),
            80,
        );
        let plain: Vec<String> = lines.iter().map(super::Line::plain).collect();

        assert_eq!(plain[0], "100  200   kept");
        assert_eq!(plain[1], "101      - gone");
        assert_eq!(plain[2], "     201 + fresh");
        // Context advanced both cursors, so the next context line is 102/202.
        assert_eq!(plain[3], "102  202   kept too");
    }

    /// The gutter is as wide as the widest number in the whole diff, so the
    /// sign column stays put across hunks.
    #[test]
    fn every_hunk_shares_one_gutter_width() {
        let lines = hunk_lines(
            &view(vec![
                hunk(1, 1, vec![line(DiffLineKind::Added, "a")]),
                hunk(9_000, 9_000, vec![line(DiffLineKind::Added, "b")]),
            ]),
            80,
        );
        let plain: Vec<String> = lines.iter().map(super::Line::plain).collect();
        assert_eq!(plain[0], "         1 + a");
        assert_eq!(plain[2], "      9000 + b");
    }

    /// Added and removed lines carry the tinted background; context does not.
    #[test]
    fn the_tint_marks_only_the_changed_lines() {
        let lines = hunk_lines(
            &view(vec![hunk(
                1,
                1,
                vec![
                    line(DiffLineKind::Added, "a"),
                    line(DiffLineKind::Removed, "b"),
                    line(DiffLineKind::Context, "c"),
                ],
            )]),
            80,
        );
        let body = |index: usize| lines[index].spans.last().expect("body span").style;
        let styles = diff_styles(None);
        assert_eq!(body(0).bg, line_background(DiffLineKind::Added, styles.theme, styles.color_level));
        assert_eq!(body(1).bg, line_background(DiffLineKind::Removed, styles.theme, styles.color_level));
        assert_eq!(body(2).bg, None, "context must not be painted");
    }

    #[test]
    fn context_content_is_plain_while_its_gutter_stays_dim() {
        let mut diff = view(vec![hunk(
            1,
            1,
            vec![line(DiffLineKind::Context, "plain context")],
        )]);
        diff.language = None;
        let lines = hunk_lines(&diff, 80);

        assert!(lines[0].spans[0].style.dim, "line-number gutter stays dim");
        assert!(!lines[0].spans[1].style.dim, "context sign is plain");
        assert!(!lines[0].spans[2].style.dim, "context content is plain");
    }

    /// A wrapped line repeats neither its number nor its sign — a second copy
    /// of either would point at a line that does not exist.
    #[test]
    fn a_wrapped_line_does_not_repeat_its_number_or_sign() {
        let lines = hunk_lines(
            &view(vec![hunk(
                1,
                1,
                vec![line(DiffLineKind::Added, "abcdefghij")],
            )]),
            // two number columns (1 each) and three spaces, the sign and one
            // space = 7, leaving 4 columns of content.
            11,
        );
        let plain: Vec<String> = lines.iter().map(super::Line::plain).collect();
        assert_eq!(plain, vec!["   1 + abcd", "       efgh", "       ij"]);
    }

    /// A huge diff is folded, and says how much it folded. Silently dropping
    /// lines would make the cell lie about what changed.
    #[test]
    fn an_oversized_diff_says_how_much_it_folded() {
        let body: Vec<DiffLine> = (0..MAX_RENDERED_LINES + 5)
            .map(|index| line(DiffLineKind::Added, &format!("line {index}")))
            .collect();
        let lines = hunk_lines(&view(vec![hunk(1, 1, body)]), 80);
        assert_eq!(lines.len(), MAX_RENDERED_LINES + 1);
        assert!(
            lines
                .last()
                .expect("fold notice")
                .plain()
                .contains("… 5 more lines"),
            "got {:?}",
            lines.last().map(super::Line::plain)
        );
    }

    /// A hunk is two texts interleaved, not one. Highlighting the concatenation
    /// of context+removed+added makes an edit that touches a quote open the
    /// string twice, and the corruption runs to the end of the hunk — measured
    /// as code in string color, string in code color, and three context rows
    /// after it wrong. Each row must be colored inside the file it belongs to.
    #[test]
    fn a_quote_edit_does_not_poison_the_rest_of_the_hunk() {
        let hunk = hunk(
            1,
            1,
            vec![
                line(DiffLineKind::Context, "fn banner() -> String {"),
                line(DiffLineKind::Removed, "    let text = \"OPEN-OLD"),
                line(DiffLineKind::Added, "    let text = \"OPEN-NEW"),
                line(DiffLineKind::Context, "    still inside the string"),
                line(DiffLineKind::Context, "\";"),
                line(DiffLineKind::Context, "    let count = 1;"),
            ],
        );

        let Some(colored) = super::hunk_highlights(&hunk, "rust") else {
            // No highlighter available in this build: the uncolored path is the
            // documented fallback, and miscoloring is what we are preventing.
            return;
        };
        assert_eq!(colored.len(), hunk.lines.len());

        // The projections must each be internally consistent: the removed row
        // is colored inside the OLD file, the added row inside the NEW one. The
        // bug showed itself as those two rows sharing one parser run, so the
        // decisive check is that they do not agree by accident on the trailing
        // context — `let count = 1;` is code in the real file, so it must not
        // come back as a single string-colored span.
        let tail = colored.last().expect("trailing context row");
        assert!(
            tail.spans.len() > 1,
            "`let count = 1;` came back as one span — the parser is still inside a string: {:?}",
            tail.plain()
        );
    }

    /// An empty diff draws nothing rather than an empty frame.
    #[test]
    fn nothing_to_show_shows_nothing() {
        assert!(hunk_lines(&view(Vec::new()), 80).is_empty());
    }

    #[test]
    fn a_hunk_keeps_syntax_state_across_multiline_strings() {
        let lines = hunk_lines(
            &view(vec![hunk(
                1,
                1,
                vec![
                    line(DiffLineKind::Context, "let message = \"open"),
                    line(DiffLineKind::Added, "still string\";"),
                ],
            )]),
            80,
        );
        let added = &lines[1].spans[2..];
        assert!(added.iter().any(|span| span.style.fg.is_some()), "the second row must inherit string syntax: {added:?}");
    }

    #[test]
    fn a_final_empty_diff_row_does_not_discard_the_whole_hunk_highlight() {
        let hunk = hunk(
            1,
            1,
            vec![
                line(DiffLineKind::Context, "let visible = 1;"),
                line(DiffLineKind::Added, ""),
            ],
        );
        let Some(colored) = super::hunk_highlights(&hunk, "rust") else {
            panic!("the projection must retain a final empty diff row");
        };
        assert_eq!(colored.len(), 2);
        assert!(colored[0].spans.iter().any(|span| span.style.fg.is_some()));
        assert_eq!(colored[1].plain(), "");
    }

    #[test]
    fn an_oversized_diff_skips_syntax_colours() {
        let body = vec![line(DiffLineKind::Added, &"x".repeat(512 * 1024 + 1))];
        let lines = hunk_lines(&view(vec![hunk(1, 1, body)]), 80);
        assert!(lines[0].spans.iter().skip(2).all(|span| span.style.fg == Some(Color::GREEN)));
    }

    #[test]
    fn deleted_syntax_is_dimmed_without_recolouring_the_sign() {
        let lines = hunk_lines(&view(vec![hunk(1, 1, vec![line(DiffLineKind::Removed, "let value = 1;")])]), 80);
        assert!(!lines[0].spans[1].style.dim, "sign keeps the diff style");
        assert!(lines[0].spans[2].style.dim, "syntax content receives DIM");
    }

    #[test]
    fn explicit_light_background_selects_the_light_palette_policy() {
        assert_eq!(super::diff_theme_for_bg(Some((255, 255, 255))), super::DiffTheme::Light);
    }
}

/// Two number columns, a marker for the lines a hunk gap skipped, and the
/// changed words painted over the line tint (t-48216).
#[cfg(test)]
mod two_column_tests {
    use runtime::message_stream::{DiffHunk, DiffLine, DiffLineKind, DiffView};

    use super::{hunk_lines, hunk_lines_for_palette};
    use crate::tui::ansi::{Color, Line};
    use crate::tui::palette::{ColorLevel, TerminalPalette};

    fn line(kind: DiffLineKind, text: &str) -> DiffLine {
        DiffLine {
            kind,
            text: text.to_string(),
        }
    }

    fn hunk(old_start: u32, new_start: u32, lines: Vec<DiffLine>) -> DiffHunk {
        DiffHunk {
            old_start,
            old_lines: 0,
            new_start,
            new_lines: 0,
            lines,
        }
    }

    fn view(hunks: Vec<DiffHunk>) -> DiffView {
        DiffView {
            old_path: Some("src/app.py".into()),
            new_path: Some("src/app.py".into()),
            language: None,
            hunks,
        }
    }

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::plain).collect()
    }

    #[test]
    fn old_and_new_numbers_stand_in_their_own_columns() {
        let lines = hunk_lines(
            &view(vec![hunk(
                100,
                200,
                vec![
                    line(DiffLineKind::Context, "kept"),
                    line(DiffLineKind::Removed, "gone"),
                    line(DiffLineKind::Added, "fresh"),
                    line(DiffLineKind::Context, "kept too"),
                ],
            )]),
            80,
        );
        assert_eq!(
            plain(&lines),
            vec![
                "100  200   kept",
                "101      - gone",
                "     201 + fresh",
                "102  202   kept too",
            ]
        );
    }

    #[test]
    fn a_gap_between_two_hunks_says_how_many_lines_it_skipped() {
        let lines = hunk_lines(
            &view(vec![
                hunk(
                    1,
                    1,
                    vec![line(DiffLineKind::Context, "a"), line(DiffLineKind::Context, "b")],
                ),
                hunk(
                    7,
                    7,
                    vec![line(DiffLineKind::Context, "g"), line(DiffLineKind::Added, "h")],
                ),
            ]),
            80,
        );
        assert_eq!(
            plain(&lines),
            vec![
                "1  1   a",
                "2  2   b",
                "     ⋯ 4 unchanged lines",
                "7  7   g",
                "   8 + h",
            ]
        );
    }

    #[test]
    fn one_skipped_line_is_one_unchanged_line() {
        let lines = hunk_lines(
            &view(vec![
                hunk(1, 1, vec![line(DiffLineKind::Context, "a")]),
                hunk(3, 3, vec![line(DiffLineKind::Added, "c")]),
            ]),
            80,
        );
        assert_eq!(plain(&lines)[1], "     ⋯ 1 unchanged line");
    }

    #[test]
    fn the_changed_words_are_painted_over_the_line_tint() {
        let palette = TerminalPalette::new((220, 220, 220), (20, 20, 20), ColorLevel::TrueColor);
        let lines = hunk_lines_for_palette(
            &view(vec![hunk(
                9,
                9,
                vec![
                    line(DiffLineKind::Removed, "        self.wfile.write(str(TODOS).encode())"),
                    line(DiffLineKind::Added, "        self.wfile.write(json.dumps(TODOS).encode())"),
                ],
            )]),
            100,
            Some(palette),
        );
        let background_of = |row: &Line, needle: &str| -> Option<Color> {
            let mut at = 0usize;
            let start = row.plain().find(needle).expect("needle on the row");
            for span in &row.spans {
                let end = at + span.text.len();
                if at <= start && start < end {
                    return span.style.bg;
                }
                at = end;
            }
            None
        };
        for (row, changed) in [(&lines[0], "str"), (&lines[1], "json.dumps")] {
            let tint = background_of(row, "self.wfile");
            let emphasis = background_of(row, changed);
            assert!(tint.is_some(), "the line keeps its tint: {row:?}");
            assert!(emphasis.is_some(), "the changed words keep a background: {row:?}");
            assert_ne!(emphasis, tint, "`{changed}` must stand out from its line: {row:?}");
        }
        let unchanged_tail = background_of(&lines[1], ".encode()");
        assert_eq!(
            unchanged_tail,
            background_of(&lines[1], "self.wfile"),
            "only the changed words are emphasised"
        );
    }
}
