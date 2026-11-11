//! Words an approval card shows that the card did not write.
//!
//! A card that asks the person to allow what an agent asked for — the window's
//! card for a Flow's schedule (`recipe-schedule`), zo's question for a schedule
//! of its own — says its own sentence, and shows beside it words that came
//! from somewhere else: a label the agent gave, a title a document says, the
//! head of a prompt the agent wrote. Those words are data, and this is the one
//! place that says how data is shown, so two cards cannot differ: every run of
//! white space is one space, no other control character, none of the marks
//! that turn the reading direction — which could make a card read as something
//! it does not say — and no longer than its bound.
//!
//! A card that shows an excerpt says how long the whole is; a cut is never
//! passed off as the end.

/// The longest a name is shown and kept: a label an agent gave, a title a
/// document says.
pub const WORD_MAX_CHARS: usize = 80;
/// The longest an excerpt of what would run is shown: the head of a prompt an
/// agent wrote.
pub const EXCERPT_MAX_CHARS: usize = 240;

/// `text` as a card shows it and a record keeps it, at most `max_chars` long.
#[must_use]
pub fn shown(text: &str, max_chars: usize) -> String {
    let kept: String = text
        .chars()
        .filter(|letter| {
            letter.is_whitespace() || !(letter.is_control() || turns_direction(*letter))
        })
        .collect();
    kept.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The marks that embed, override or isolate a run's direction, and the ones
/// that only mark one. No name and no sentence shown as data needs any of
/// them.
const fn turns_direction(letter: char) -> bool {
    matches!(
        letter,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// White space of any kind — a tab, a line break — is one space, and none
    /// stands at either end: a prompt's lines read as one line.
    #[test]
    fn every_run_of_white_space_is_one_space() {
        assert_eq!(
            shown("  Morning\twallet\n check ", WORD_MAX_CHARS),
            "Morning wallet check"
        );
        assert_eq!(shown("a\r\n\r\n  b", WORD_MAX_CHARS), "a b");
        assert_eq!(shown(" \n\t ", WORD_MAX_CHARS), "");
    }

    /// What could make a card read as something it does not say is left out:
    /// the marks that turn the reading direction, and every control character
    /// that is not white space.
    #[test]
    fn direction_marks_and_control_characters_are_left_out() {
        assert_eq!(
            shown("pay\u{202E}dleh\u{2066}x\u{2069}\u{200F}", WORD_MAX_CHARS),
            "paydlehx"
        );
        assert_eq!(shown("\u{061C}\u{200E}name", WORD_MAX_CHARS), "name");
        assert_eq!(shown("a\u{7}b\u{1B}[31mc", WORD_MAX_CHARS), "ab[31mc");
    }

    /// The bound counts characters, not bytes, and a cut leaves no space at
    /// its end.
    #[test]
    fn the_bound_counts_characters_and_a_cut_ends_on_a_letter() {
        assert_eq!(
            shown(&"가".repeat(WORD_MAX_CHARS + 20), WORD_MAX_CHARS)
                .chars()
                .count(),
            WORD_MAX_CHARS
        );
        assert_eq!(shown("ab cd", 3), "ab");
        assert_eq!(shown("abcdef", 0), "");
        let prompt = "line one\nline two ".repeat(40);
        let head = shown(&prompt, EXCERPT_MAX_CHARS);
        assert!(head.chars().count() <= EXCERPT_MAX_CHARS);
        assert!(head.starts_with("line one line two line one"));
    }
}
