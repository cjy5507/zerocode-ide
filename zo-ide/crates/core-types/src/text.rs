//! Pure text utilities shared across crates.
//!
//! Single source of truth for small string algorithms that were previously
//! re-implemented per crate (and drifted): edit-distance suggestion ranking,
//! byte-bounded truncation that never splits a UTF-8 codepoint, and stripping
//! a stray trailing `call` tool-call marker. Living in the leaf `core-types`
//! crate, these are reachable from every other crate without a dependency
//! cycle.

/// Levenshtein edit distance between two strings, counting char-level
/// insertions, deletions, and substitutions.
///
/// Uses the standard two-row dynamic-programming algorithm, so memory is
/// proportional to the shorter side rather than the full matrix.
#[must_use]
pub fn levenshtein_distance(left: &str, right: &str) -> usize {
    if left == right {
        return 0;
    }
    if left.is_empty() {
        return right.chars().count();
    }
    if right.is_empty() {
        return left.chars().count();
    }

    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = usize::from(left_char != *right_char);
            current[right_index + 1] = (current[right_index] + 1)
                .min(previous[right_index + 1] + 1)
                .min(previous[right_index] + substitution_cost);
        }
        previous.clone_from(&current);
    }

    previous[right_chars.len()]
}

/// Truncate `text` to at most `max_bytes` bytes, walking back to the nearest
/// char boundary so a multi-byte codepoint is never split, then append
/// `marker` to signal the elision. Returns `text` unchanged (cloned) when it
/// already fits.
#[must_use]
pub fn truncate_on_char_boundary(text: &str, max_bytes: usize, marker: &str) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{marker}", &text[..end])
}

/// Keep the head (2/3) and tail (1/3) of an oversized text, eliding the middle
/// with an explicit char-count notice. Char-based so it never splits a UTF-8
/// boundary. Returns `text` unchanged (cloned) when it already fits.
///
/// The head/tail split preserves what matters in agent results and logs: the
/// framing/answer up front and the conclusion/error at the end — a blind tail
/// cut loses exactly the ending, and mid-JSON cuts break parsers downstream.
#[must_use]
pub fn elide_middle(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let head_len = max_chars * 2 / 3;
    let tail_len = max_chars - head_len;
    let omitted = chars.len() - head_len - tail_len;
    let head: String = chars[..head_len].iter().collect();
    let tail: String = chars[chars.len() - tail_len..].iter().collect();
    format!("{head}\n\n…[{omitted} chars omitted]…\n\n{tail}")
}

/// Strip a stray trailing tool-call marker — a final run of lines that are
/// only the literal `call` (case-insensitive), plus the blank lines around
/// them — that some providers emit after the real assistant text.
///
/// Mutates `text` in place and returns `true` when a marker was removed.
#[must_use]
pub fn strip_trailing_stray_tool_call_marker(text: &mut String) -> bool {
    let trimmed_end = text.trim_end().len();
    if trimmed_end == 0 {
        return false;
    }

    let mut ranges = Vec::new();
    let mut start = 0;
    for (idx, ch) in text[..trimmed_end].char_indices() {
        if ch == '\n' {
            ranges.push((start, idx));
            start = idx + ch.len_utf8();
        }
    }
    ranges.push((start, trimmed_end));

    let mut saw_marker = false;
    let mut remove_from = trimmed_end;
    for (start, end) in ranges.into_iter().rev() {
        let line = text[start..end].trim();
        if line.is_empty() {
            if saw_marker {
                remove_from = start;
            }
            continue;
        }
        if !line.eq_ignore_ascii_case("call") {
            break;
        }
        saw_marker = true;
        remove_from = start;
    }

    if !saw_marker {
        return false;
    }

    let kept = text[..remove_from].trim_end().to_string();
    *text = kept;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_matches_known_distances() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("flaw", "lawn"), 2);
    }

    #[test]
    fn elide_middle_keeps_head_and_tail_with_char_count_notice() {
        assert_eq!(elide_middle("fits", 10), "fits");
        let long: String = "a".repeat(50) + &"z".repeat(50);
        let elided = elide_middle(&long, 30);
        // head 20 (2/3) + tail 10, 70 chars elided.
        assert!(elided.starts_with(&"a".repeat(20)));
        assert!(elided.ends_with(&"z".repeat(10)));
        assert!(elided.contains("…[70 chars omitted]…"));
        // Char-based: multi-byte codepoints never split.
        let cjk: String = "가".repeat(100);
        let elided = elide_middle(&cjk, 30);
        assert!(elided.contains("…[70 chars omitted]…"));
        assert!(elided.starts_with('가') && elided.ends_with('가'));
    }

    #[test]
    fn truncate_keeps_short_text_and_respects_char_boundary() {
        assert_eq!(truncate_on_char_boundary("short", 100, "…"), "short");
        // "héllo": 'h' is byte 0, 'é' spans bytes 1-2, so byte 3 (start of the
        // first 'l') is already a boundary — the cap keeps "hé".
        assert_eq!(truncate_on_char_boundary("héllo world", 3, "…"), "hé…");
        // A 2-byte cap lands mid-'é' and must walk back to byte 1, keeping "h".
        assert_eq!(truncate_on_char_boundary("héllo world", 2, "…"), "h…");
        assert_eq!(
            truncate_on_char_boundary("hello world", 3, "…[truncated]"),
            "hel…[truncated]"
        );
    }

    #[test]
    fn strip_removes_trailing_call_marker_only() {
        let mut text = String::from("real answer\n\ncall");
        assert!(strip_trailing_stray_tool_call_marker(&mut text));
        assert_eq!(text, "real answer");

        let mut clean = String::from("just text");
        assert!(!strip_trailing_stray_tool_call_marker(&mut clean));
        assert_eq!(clean, "just text");
    }
}
