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
//! it does not say — no letter that draws nothing at all, and no longer than
//! its bound.
//!
//! A letter that draws nothing is not harmless for being unseen: words spelled
//! in such letters are read by whatever is handed the whole text afterwards —
//! the model a scheduled prompt opens a turn for — while the person who
//! allowed it saw none of them. So they are left out and counted
//! ([`shown_counting`]), and a card says how many it left out
//! ([`hidden_said`]).
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
    shown_counting(text, max_chars).text
}

/// What [`shown_counting`] makes of a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// The text as a card shows it and a record keeps it.
    pub text: String,
    /// How many letters were left out because nothing of them would be seen:
    /// control characters that are not white space, and every letter Unicode
    /// says is ignored when drawn. Counted over the whole text, past where
    /// the shown part is cut — a card that shows an excerpt still says what
    /// the rest holds.
    pub hidden: usize,
}

/// `text` as a card shows it, at most `max_chars` long, and how many of its
/// letters draw nothing and were left out.
#[must_use]
pub fn shown_counting(text: &str, max_chars: usize) -> Shown {
    let mut hidden = 0;
    let kept: String = text
        .chars()
        .filter(|letter| {
            let seen = letter.is_whitespace() || !(letter.is_control() || draws_nothing(*letter));
            if !seen {
                hidden += 1;
            }
            seen
        })
        .collect();
    let text = kept
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect::<String>()
        .trim_end()
        .to_string();
    Shown { text, hidden }
}

/// What a card says beside words it left `hidden` letters out of, for a card
/// with no catalog of its own; `None` when it left none out. A card with a
/// catalog says the same in the person's language, from the count.
#[must_use]
pub fn hidden_said(hidden: usize) -> Option<String> {
    match hidden {
        0 => None,
        1 => Some("1 character that draws nothing was left out of what is shown".to_string()),
        many => Some(format!(
            "{many} characters that draw nothing were left out of what is shown"
        )),
    }
}

/// The letters a renderer draws nothing for — Unicode's
/// `Default_Ignorable_Code_Point` (DerivedCoreProperties, 16.0): the soft
/// hyphen, zero-width spaces and joiners, the marks that embed, override,
/// isolate or only mark a run's direction, fillers, variation selectors and
/// tag characters. No name and no sentence shown as data needs any of them,
/// and each can carry words nobody sees.
const fn draws_nothing(letter: char) -> bool {
    matches!(
        letter,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF0}'..='\u{FFF8}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
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

    /// A letter that draws nothing — a zero-width space, a soft hyphen, a tag
    /// character, a variation selector, a filler — could spell words nobody
    /// sees: each is left out, and the count says how many.
    #[test]
    fn letters_that_draw_nothing_are_left_out_and_counted() {
        let read = shown_counting(
            "a\u{200B}b\u{00AD}c\u{E0041}\u{E0042}d\u{FE0F}e\u{3164}f",
            WORD_MAX_CHARS,
        );
        assert_eq!((read.text.as_str(), read.hidden), ("abcdef", 6));
        // The marks that turn the direction and a control character count too.
        let turned = shown_counting("pay\u{202E}dleh\u{0007}", WORD_MAX_CHARS);
        assert_eq!((turned.text.as_str(), turned.hidden), ("paydleh", 2));
        // White space made one space is not a letter left out, and Hangul —
        // syllables and the jamo they are written with — is all kept.
        let plain = shown_counting("  한글 ㄱㄴ\n Wallet  QA ", WORD_MAX_CHARS);
        assert_eq!(
            (plain.text.as_str(), plain.hidden),
            ("한글 ㄱㄴ Wallet QA", 0)
        );
        assert_eq!(shown("a\u{200B}b", WORD_MAX_CHARS), "ab");
    }

    /// The count is of the whole text: letters left out past the cut are
    /// counted, so an excerpt says what the rest holds.
    #[test]
    fn letters_left_out_past_the_cut_are_counted() {
        let long = format!("{}\u{200B}\u{E0041}", "가".repeat(WORD_MAX_CHARS + 5));
        let read = shown_counting(&long, WORD_MAX_CHARS);
        assert_eq!(
            (read.text.chars().count(), read.hidden),
            (WORD_MAX_CHARS, 2)
        );
    }

    /// What a card with no catalog says of the count, and nothing for none.
    #[test]
    fn a_card_says_how_many_letters_it_left_out() {
        assert_eq!(hidden_said(0), None);
        assert_eq!(
            hidden_said(1).as_deref(),
            Some("1 character that draws nothing was left out of what is shown")
        );
        assert_eq!(
            hidden_said(12).as_deref(),
            Some("12 characters that draw nothing were left out of what is shown")
        );
    }
}
