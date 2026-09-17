//! 줄바꿈 — 히스토리에 넣기 전에 우리가 직접 접는다.
//!
//! codex 도 스크롤백에 밀어 넣기 전에 미리 접는다(`insert_history.rs` 의
//! pre-wrap). 터미널의 소프트랩에 맡기면 접힌 뒷줄에 셀 접두어(`  `)가 붙지
//! 않아 `• ` 마커의 열이 무너지기 때문이다.
//!
//! 규칙: 공백에서 끊고, 한 낱말이 폭보다 길면 글자 단위로 쪼갠다. CJK 는
//! 낱말 경계가 없으므로 글자 폭 기준으로 자연히 글자 단위가 된다.

use super::ansi::{char_width, Line, Span, Style};

/// 줄을 넘기며 확정할 때 꼬리 공백을 버린다 — 접힌 자리에 공백을 남기면
/// 히스토리 셀의 오른쪽 끝이 들쭉날쭉해지고 골든 비교도 흔들린다.
///
/// 줄 스타일(`style`)은 접힌 조각 **전부**가 물려받는다 — 줄에 깔린 색은
/// 조각마다 다시 깔려야 한다.
fn commit(out: &mut Vec<Line>, mut spans: Vec<Span>, style: Style) {
    while let Some(last) = spans.last_mut() {
        let trimmed = last.text.trim_end_matches(' ').len();
        if trimmed == last.text.len() {
            break;
        }
        last.text.truncate(trimmed);
        if last.text.is_empty() {
            spans.pop();
        } else {
            break;
        }
    }
    out.push(Line::new(spans).styled(style));
}

/// `line` 을 폭 `width` 로 접는다. 두 번째 줄부터는 `indent` 를 앞에 붙인다.
///
/// 줄이 달고 있던 제어 바이트([`Line::lead`]·[`Line::trail`], 접기 마커)는
/// **접힌 조각들의 바깥 끝**으로 간다 — lead 는 첫 조각 앞, trail 은 마지막
/// 조각 뒤. 헤더가 두 행으로 접혀도 `begin` 이 첫 행 앞에 그대로 선다.
#[must_use]
pub fn wrap_line(line: &Line, width: usize, indent: &Span) -> Vec<Line> {
    let mut out = wrap_spans(line, width, indent);
    if let Some(first) = out.first_mut() {
        first.lead.clone_from(&line.lead);
    }
    if let Some(last) = out.last_mut() {
        last.trail.clone_from(&line.trail);
    }
    out
}

/// 스팬만 접는다 — 제어 바이트는 [`wrap_line`] 이 바깥에서 다시 단다.
fn wrap_spans(line: &Line, width: usize, indent: &Span) -> Vec<Line> {
    if width == 0 {
        return vec![line.clone()];
    }
    let style = line.style;
    let indent_width = indent.width();
    let mut out: Vec<Line> = Vec::new();
    let mut current: Vec<Span> = Vec::new();
    let mut used = 0usize;
    // 마지막 공백 뒤에 쌓인 낱말 — 줄을 넘길 때 통째로 다음 줄로 옮긴다.
    let mut word: Vec<Span> = Vec::new();
    let mut word_width = 0usize;

    let limit = |out_len: usize| {
        if out_len == 0 {
            width
        } else {
            width.max(indent_width + 1)
        }
    };
    let start_line = |out_len: usize| -> (Vec<Span>, usize) {
        if out_len == 0 || indent_width == 0 {
            (Vec::new(), 0)
        } else {
            (vec![indent.clone()], indent_width)
        }
    };

    let push_char = |ch: char,
                         span_style,
                         out: &mut Vec<Line>,
                         current: &mut Vec<Span>,
                         used: &mut usize,
                         word: &mut Vec<Span>,
                         word_width: &mut usize| {
        let cell = char_width(ch);
        let is_space = ch == ' ';
        if is_space {
            // 공백은 낱말 경계다 — 쌓인 낱말을 현재 줄에 확정한다.
            current.append(word);
            *used += *word_width;
            *word_width = 0;
        }
        if *used + *word_width + cell > limit(out.len()) && (*used > 0 || *word_width > 0) {
            if is_space {
                // 줄 끝의 공백은 버린다.
                commit(out, std::mem::take(current), style);
                let (fresh, fresh_width) = start_line(out.len());
                *current = fresh;
                *used = fresh_width;
                return;
            }
            // 행에 "내용"이 있는가는 들여쓰기를 뺀 기준이다. 이어지는 행은
            // 들여쓰기만으로 `used > 0` 이라, 폭보다 긴 낱말을 만나면 글자
            // 단위로 쪼개는 대신 들여쓰기만 든 행을 확정하고 새 행을 여는 일을
            // 글자마다 반복했다 — 폭 60 의 불릿 뒤에 빈 행 스무 개가 서고 그
            // 아래에 77 칸짜리 인라인 코드 토큰이 왔다("또 공백", 2026-09-03,
            // 분할 판의 스트리밍 답변).
            let row_indent = start_line(out.len()).1;
            if *used > row_indent {
                commit(out, std::mem::take(current), style);
                let (fresh, fresh_width) = start_line(out.len());
                *current = fresh;
                *used = fresh_width;
            } else {
                // 낱말 하나가 폭보다 길다 — 글자 단위로 쪼갠다.
                current.append(word);
                commit(out, std::mem::take(current), style);
                let (fresh, fresh_width) = start_line(out.len());
                *current = fresh;
                *used = fresh_width;
                *word_width = 0;
            }
        }
        let target = if is_space { &mut *current } else { &mut *word };
        match target.last_mut() {
            Some(last) if last.style == span_style => last.text.push(ch),
            _ => target.push(Span::new(ch.to_string(), span_style)),
        }
        if is_space {
            *used += cell;
        } else {
            *word_width += cell;
        }
    };

    for span in &line.spans {
        for ch in span.text.chars() {
            push_char(
                ch,
                span.style,
                &mut out,
                &mut current,
                &mut used,
                &mut word,
                &mut word_width,
            );
        }
    }
    current.append(&mut word);
    if !current.is_empty() || out.is_empty() {
        out.push(Line::new(current).styled(style));
    }
    out
}

/// 여러 줄을 한 번에 접는다.
#[must_use]
pub fn wrap_lines(lines: &[Line], width: usize, indent: &Span) -> Vec<Line> {
    lines
        .iter()
        .flat_map(|line| wrap_line(line, width, indent))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::wrap_line;
    use crate::tui::ansi::{Line, Span};

    #[test]
    fn short_lines_pass_through() {
        let wrapped = wrap_line(&Line::from_text("hi"), 10, &Span::raw("  "));
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].plain(), "hi");
    }

    #[test]
    fn words_break_at_spaces_and_carry_the_indent() {
        let wrapped = wrap_line(&Line::from_text("alpha beta gamma"), 11, &Span::raw("  "));
        let plain: Vec<String> = wrapped.iter().map(Line::plain).collect();
        assert_eq!(plain, vec!["alpha beta".to_string(), "  gamma".to_string()]);
    }

    #[test]
    fn a_word_longer_than_the_width_is_split() {
        let wrapped = wrap_line(&Line::from_text("abcdefghij"), 4, &Span::raw(""));
        let plain: Vec<String> = wrapped.iter().map(Line::plain).collect();
        assert_eq!(plain, vec!["abcd", "efgh", "ij"]);
    }

    /// A word wider than the width on a CONTINUATION row — one that already
    /// carries the indent — is split by characters like any other, not
    /// preceded by one indent-only blank row per character it overhangs.
    #[test]
    fn a_long_word_on_a_continuation_row_is_split_not_padded_with_blank_rows() {
        let text = "다음 줄의 agent_launch_env_with_lock(crates/zerocode-hookd/src/codex_install.rs:1020)이 덮어씁니다.";
        let wrapped = wrap_line(&Line::from_text(text), 40, &Span::raw("    "));
        let plain: Vec<String> = wrapped.iter().map(Line::plain).collect();
        assert!(
            plain.iter().all(|row| !row.trim().is_empty()),
            "no blank row inside a wrapped line: {plain:?}"
        );
        assert!(wrapped.iter().all(|row| row.width() <= 40), "{plain:?}");
        assert!(plain.len() <= 4, "the token is split across a few rows, not one per char: {plain:?}");
        assert!(plain[1].starts_with("    agent_launch_env_with_lock("), "{plain:?}");
        let joined: String = plain.iter().map(|row| row.trim_start()).collect::<Vec<_>>().join("");
        assert_eq!(joined.replace(' ', ""), text.replace(' ', ""), "every character survives");
    }

    #[test]
    fn wide_glyphs_count_two_cells() {
        let wrapped = wrap_line(&Line::from_text("한글한글한글"), 5, &Span::raw(""));
        let plain: Vec<String> = wrapped.iter().map(Line::plain).collect();
        assert_eq!(plain, vec!["한글", "한글", "한글"]);
    }

    #[test]
    fn control_bytes_ride_the_outer_edges_of_a_fold() {
        let line = Line::from_text("alpha beta gamma")
            .with_lead("<begin>")
            .with_trail("<end>");
        let wrapped = wrap_line(&line, 11, &Span::raw("  "));
        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0].lead.as_deref(), Some("<begin>"));
        assert_eq!(wrapped[0].trail, None);
        assert_eq!(wrapped[1].lead, None);
        assert_eq!(wrapped[1].trail.as_deref(), Some("<end>"));
    }

    #[test]
    fn empty_lines_survive_as_one_empty_line() {
        let wrapped = wrap_line(&Line::empty(), 8, &Span::raw("  "));
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].plain(), "");
    }
}
