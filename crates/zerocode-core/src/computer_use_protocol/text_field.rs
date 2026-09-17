//! Writing text into the focused field through accessibility, decided once
//! for every platform: `TextInput.replaceSelection` from the macOS helper,
//! with the two answers that Swift folded into one `nil` kept apart.
//!
//! The helper answers `nil` both when the accessibility road was never taken
//! (the value could not be read, the element is not settable) and when a
//! write happened but the readback did not match. Its caller falls through to
//! synthetic typing on `nil`, which is right for the first and wrong for the
//! second: a value that WAS set and then typed again is the text twice, and a
//! value that could not be read is not an empty field to overwrite. On Windows
//! the two cases are structurally likelier than on macOS (a password edit
//! refuses the read; the Text and Value patterns are two views of one field),
//! so the outcome is a type here — [`ReplaceOutcome`] — and a caller may only
//! type synthetically on [`ReplaceOutcome::NotApplicable`].

use super::render::preview;
use super::{Verification, unverified_reason};

/// A field's current text and whether it may be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldText {
    pub value: String,
    pub read_only: bool,
}

/// Why the field's value could not be read. Neither is an empty field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadFailure {
    /// The element has no value to read through accessibility.
    Unsupported,
    /// The provider refused — a password field, an access-denied proxy, an
    /// element that went away.
    Denied(String),
}

/// The text around the selection, as the field's text view reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub prefix: String,
    pub selected: String,
    pub suffix: String,
}

/// How a write ended when it did not plainly succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteFailure {
    /// The provider refused before touching the value; nothing changed.
    Refused(String),
    /// The call did not come back with an answer — a timeout, a broken
    /// connection — and the value MAY have changed.
    Unconfirmed(String),
}

/// One focused text field as a platform exposes it.
pub trait TextField {
    fn read(&self) -> Result<FieldText, ReadFailure>;
    /// `None` when the field cannot say where its selection is; the text is
    /// then appended at the end, as the helper does without a selected range.
    fn around_selection(&self) -> Option<Selection>;
    fn write(&self, text: &str) -> Result<(), WriteFailure>;
    /// Put the caret after the inserted text (UTF-16 offset into the new
    /// value). Best effort: a field that cannot is still verified by its text.
    fn place_caret(&self, utf16_offset: usize);
}

/// What `replace_selection` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceOutcome {
    /// The accessibility road was not taken and the field is untouched; the
    /// caller may type synthetically. The reason is for the action's
    /// `fallbackReason`.
    NotApplicable(&'static str),
    /// A write happened (or may have). The caller reports the accessibility
    /// road with this verification and must not type the text again.
    Applied(Verification),
}

/// Replace the selection of `field` with `text` and read the result back.
pub fn replace_selection(field: &impl TextField, text: &str) -> ReplaceOutcome {
    let current = match field.read() {
        Ok(current) => current,
        Err(ReadFailure::Unsupported) => return ReplaceOutcome::NotApplicable("no_value"),
        Err(ReadFailure::Denied(_)) => return ReplaceOutcome::NotApplicable("value_unreadable"),
    };
    if current.read_only {
        return ReplaceOutcome::NotApplicable("read_only");
    }
    let (prefix, suffix) = match field.around_selection() {
        Some(selection) => {
            // The selection view and the value view are two reads of one
            // field; when they disagree (a trailing carriage return the text
            // pattern reports and the value pattern does not, an embedded
            // object), composing from one and writing to the other would put
            // text the person never saw into the field. Synthetic typing
            // honours the field's own selection instead.
            let composed = format!(
                "{}{}{}",
                selection.prefix, selection.selected, selection.suffix
            );
            if composed != current.value {
                return ReplaceOutcome::NotApplicable("selection_inconsistent");
            }
            (selection.prefix, selection.suffix)
        }
        None => (current.value.clone(), String::new()),
    };
    let next = format!("{prefix}{text}{suffix}");
    match field.write(&next) {
        Ok(()) => {}
        Err(WriteFailure::Refused(_)) => return ReplaceOutcome::NotApplicable("write_refused"),
        Err(WriteFailure::Unconfirmed(why)) => {
            return ReplaceOutcome::Applied(Verification::unverified(
                unverified_reason::WRITE_UNCONFIRMED,
                Some(text.to_string()),
                Some(why),
            ));
        }
    }
    field.place_caret(prefix.encode_utf16().count() + text.encode_utf16().count());
    ReplaceOutcome::Applied(match field.read() {
        Ok(after) if after.value == next => Verification::verified(
            "focusedText",
            Some(text.to_string()),
            Some(preview(&next, 120)),
        ),
        Ok(after) => Verification::unverified(
            unverified_reason::VALUE_MISMATCH,
            Some(text.to_string()),
            Some(preview(&after.value, 120)),
        ),
        Err(_) => Verification::unverified(
            unverified_reason::READBACK_UNSUPPORTED,
            Some(text.to_string()),
            None,
        ),
    })
}

/// What keys typed into a field did, read back after the settle: `before`
/// is what it held, `expected` what it should hold now, `after` what it
/// reads (`None`: it could not be read). A field that held nothing and holds
/// nothing again consumed the keys or never got them — a terminal's key sink
/// clears itself — so that is not called unchanged.
#[must_use]
pub fn judge_typed(before: &str, expected: &str, after: Option<&str>) -> Verification {
    let reason = match after {
        None => unverified_reason::READBACK_UNSUPPORTED,
        Some(after) if after == expected => {
            return Verification::verified(
                "focusedText",
                Some(expected.to_string()),
                Some(preview(after, 120)),
            );
        }
        Some(after) if after == before && before.is_empty() => {
            unverified_reason::READBACK_UNSUPPORTED
        }
        Some(after) if after == before => unverified_reason::VALUE_UNCHANGED,
        Some(_) => unverified_reason::VALUE_MISMATCH,
    };
    Verification::unverified(
        reason,
        Some(expected.to_string()),
        after.map(|after| preview(after, 120)),
    )
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    /// The one table the Swift helper's landing judge runs too.
    #[test]
    fn typed_keys_are_judged_by_the_shared_case_table() {
        let table = include_str!("cases/typed_landing.tsv");
        let cell = |word: &'static str| match word {
            "<empty>" => Some(""),
            "<unread>" => None,
            other => Some(other),
        };
        let mut rows = 0;
        for line in table
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        {
            let cells: Vec<&'static str> = line.split('\t').collect();
            let judged = super::judge_typed(
                cell(cells[0]).unwrap(),
                cell(cells[1]).unwrap(),
                cell(cells[2]),
            );
            let word = match &judged {
                super::Verification::Verified { .. } => "verified".to_string(),
                super::Verification::Unverified { reason, .. } => reason.clone(),
            };
            assert_eq!(word, cells[3], "{line}");
            rows += 1;
        }
        assert!(rows >= 6, "the table was read: {rows}");
    }

    use super::*;

    /// A field that answers what the test tells it to and records every write.
    struct FakeField {
        reads: RefCell<Vec<Result<FieldText, ReadFailure>>>,
        selection: Option<Selection>,
        write_answer: Result<(), WriteFailure>,
        writes: RefCell<Vec<String>>,
        caret: RefCell<Option<usize>>,
    }

    impl FakeField {
        fn new(value: &str) -> Self {
            Self {
                reads: RefCell::new(vec![Ok(FieldText {
                    value: value.to_string(),
                    read_only: false,
                })]),
                selection: None,
                write_answer: Ok(()),
                writes: RefCell::new(Vec::new()),
                caret: RefCell::new(None),
            }
        }

        /// The value the field reports AFTER a write.
        fn reads_back(mut self, value: &str) -> Self {
            self.reads.get_mut().push(Ok(FieldText {
                value: value.to_string(),
                read_only: false,
            }));
            self
        }

        fn readback_fails(mut self) -> Self {
            self.reads
                .get_mut()
                .push(Err(ReadFailure::Denied("E_ACCESSDENIED".into())));
            self
        }

        fn selecting(mut self, prefix: &str, selected: &str, suffix: &str) -> Self {
            self.selection = Some(Selection {
                prefix: prefix.into(),
                selected: selected.into(),
                suffix: suffix.into(),
            });
            self
        }
    }

    impl TextField for FakeField {
        fn read(&self) -> Result<FieldText, ReadFailure> {
            let mut reads = self.reads.borrow_mut();
            if reads.len() > 1 {
                reads.remove(0)
            } else {
                reads[0].clone()
            }
        }

        fn around_selection(&self) -> Option<Selection> {
            self.selection.clone()
        }

        fn write(&self, text: &str) -> Result<(), WriteFailure> {
            self.writes.borrow_mut().push(text.to_string());
            self.write_answer.clone()
        }

        fn place_caret(&self, utf16_offset: usize) {
            *self.caret.borrow_mut() = Some(utf16_offset);
        }
    }

    #[test]
    fn a_denied_read_is_not_an_empty_field_and_nothing_is_written() {
        let field = FakeField {
            reads: RefCell::new(vec![Err(ReadFailure::Denied("E_ACCESSDENIED".into()))]),
            selection: None,
            write_answer: Ok(()),
            writes: RefCell::new(Vec::new()),
            caret: RefCell::new(None),
        };
        assert_eq!(
            replace_selection(&field, "hunter2"),
            ReplaceOutcome::NotApplicable("value_unreadable")
        );
        assert!(
            field.writes.borrow().is_empty(),
            "a password field was overwritten"
        );
        let unsupported = FakeField {
            reads: RefCell::new(vec![Err(ReadFailure::Unsupported)]),
            ..FakeField::new("")
        };
        assert_eq!(
            replace_selection(&unsupported, "x"),
            ReplaceOutcome::NotApplicable("no_value")
        );
    }

    #[test]
    fn a_read_only_field_is_left_to_synthetic_input() {
        let field = FakeField {
            reads: RefCell::new(vec![Ok(FieldText {
                value: "fixed".into(),
                read_only: true,
            })]),
            ..FakeField::new("")
        };
        assert_eq!(
            replace_selection(&field, "x"),
            ReplaceOutcome::NotApplicable("read_only")
        );
        assert!(field.writes.borrow().is_empty());
    }

    #[test]
    fn the_selection_is_replaced_exactly_and_the_caret_lands_after_the_text() {
        let field = FakeField::new("start OLD end")
            .selecting("start ", "OLD", " end")
            .reads_back("start 한글 end");
        assert_eq!(
            replace_selection(&field, "한글"),
            ReplaceOutcome::Applied(Verification::verified(
                "focusedText",
                Some("한글".into()),
                Some("start 한글 end".into())
            ))
        );
        assert_eq!(*field.writes.borrow(), vec!["start 한글 end".to_string()]);
        assert_eq!(
            *field.caret.borrow(),
            Some("start 한글".encode_utf16().count())
        );
    }

    #[test]
    fn without_a_selection_view_the_text_is_appended_at_the_end() {
        let field = FakeField::new("start ").reads_back("start end");
        assert!(matches!(
            replace_selection(&field, "end"),
            ReplaceOutcome::Applied(Verification::Verified { .. })
        ));
        assert_eq!(*field.writes.borrow(), vec!["start end".to_string()]);
    }

    #[test]
    fn a_write_whose_readback_differs_is_applied_and_unverified_never_retyped() {
        let field = FakeField::new("abc")
            .selecting("abc", "", "")
            .reads_back("abc\r");
        let outcome = replace_selection(&field, "d");
        // The preview is sanitized like every other preview: the carriage
        // return the field grew reads as a space.
        assert_eq!(
            outcome,
            ReplaceOutcome::Applied(Verification::unverified(
                "value_mismatch",
                Some("d".into()),
                Some("abc ".into())
            ))
        );
        assert_eq!(field.writes.borrow().len(), 1);
    }

    #[test]
    fn a_readback_that_fails_after_a_write_is_still_applied() {
        let field = FakeField::new("").readback_fails();
        assert_eq!(
            replace_selection(&field, "typed"),
            ReplaceOutcome::Applied(Verification::unverified(
                "readback_unsupported",
                Some("typed".into()),
                None
            ))
        );
    }

    #[test]
    fn a_selection_view_that_disagrees_with_the_value_is_not_trusted() {
        // The text pattern sees a trailing carriage return the value pattern
        // does not: composing from one and writing to the other is refused.
        let field = FakeField::new("abc").selecting("ab", "c", "\r");
        assert_eq!(
            replace_selection(&field, "x"),
            ReplaceOutcome::NotApplicable("selection_inconsistent")
        );
        assert!(field.writes.borrow().is_empty());
    }

    #[test]
    fn a_refused_write_falls_through_and_an_unconfirmed_one_does_not() {
        let refused = FakeField {
            write_answer: Err(WriteFailure::Refused("E_NOTIMPL".into())),
            ..FakeField::new("v")
        };
        assert_eq!(
            replace_selection(&refused, "x"),
            ReplaceOutcome::NotApplicable("write_refused")
        );
        let unconfirmed = FakeField {
            write_answer: Err(WriteFailure::Unconfirmed("UIA_E_TIMEOUT".into())),
            ..FakeField::new("v")
        };
        assert_eq!(
            replace_selection(&unconfirmed, "x"),
            ReplaceOutcome::Applied(Verification::unverified(
                "write_unconfirmed",
                Some("x".into()),
                Some("UIA_E_TIMEOUT".into())
            ))
        );
        assert_eq!(unconfirmed.writes.borrow().len(), 1);
    }
}
