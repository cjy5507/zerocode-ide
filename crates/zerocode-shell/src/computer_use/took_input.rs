//! Which window a Windows act's input was seen to reach: the one rule the
//! Windows provider's answer names its window by (`actedWindow`, reviews
//! t-129021 §3 and t-129057 §1).
//!
//! Letting an act go and naming the window it went to are two judgements.
//! The click fence lets a press through over a window the target owns — its
//! menu, its dropdown — as the target's own, and the focus check lets keys
//! go while the target stands in front; but a step's evidence is a picture
//! of the window its answer names, so a road names a window only where it
//! saw that window take the input, and none where it only aimed there.
//!
//! The Windows provider is the one user. This half needs no platform, so its
//! tests run on the developer's machine (the `computer_use` module list).

/// The windows one look of the click fence read just before a press, by
/// their numbers: the window in front, and the window under the point the
/// press goes to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PressSeen {
    pub(super) front: Option<u64>,
    pub(super) under: Option<u64>,
}

/// What a road of the Windows provider saw of where its input went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Delivered {
    /// Calls on an element of the act's window's own tree — a page of scroll
    /// each, a toggle each — `answered` of which came back with success.
    TreeCalls { answered: usize },
    /// A click's presses, each as the fence saw it just before it.
    Presses(Vec<PressSeen>),
    /// Keys put into the input stream (`SendInput`), which answers how many
    /// events it took and never which window they went to.
    Keys,
    /// A write into the focused element, whose own window handle names
    /// `window` (`None` when only its process ties it to the act's window),
    /// `answered` when the write came back with an answer.
    FocusedWrite { window: Option<u64>, answered: bool },
}

impl Delivered {
    /// The window this road saw take its input, by its number, for an act
    /// aimed at `window` — what `Provider::input_taken_by` keeps, and the act's
    /// answer names its window only when it is that one
    /// (`ActedWindow::if_it_took`). As the roads answer today: each names
    /// the window it was aimed at once its call returned — whatever window a
    /// click's presses went to, whether or not a page of a scroll or a toggle
    /// was answered, whichever window stood in front of keys (here the act's
    /// own) — and a write into the focused element names that element's
    /// window whether or not the write answered.
    #[must_use]
    pub(super) fn took(&self, window: u64) -> Option<u64> {
        match self {
            Self::TreeCalls { .. } | Self::Presses(_) | Self::Keys => Some(window),
            Self::FocusedWrite { window: own, .. } => *own,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use zerocode_core::computer_use_protocol::execution::ActedWindow;
    use zerocode_core::computer_use_protocol::text_field::{
        FieldText, ReadFailure, Selection, TextField, WriteFailure, replace_selection,
    };

    use super::*;

    /// Window 19 of process 501: the window every act below was aimed at
    /// (`current_snapshot`).
    const AIMED: ActedWindow = ActedWindow { id: 19, pid: 501 };

    /// The window the act's answer names (`actedWindow`) when its road
    /// delivered `delivered`, as `Provider::run_action` names it.
    fn named(delivered: &Delivered) -> Option<ActedWindow> {
        AIMED.if_it_took(delivered.took(AIMED.id))
    }

    /// One look of the click fence before a press: `front` in front, `under`
    /// under the point.
    fn press(front: u64, under: u64) -> PressSeen {
        PressSeen {
            front: Some(front),
            under: Some(under),
        }
    }

    /// The click fence lets a press through over a window the target owns
    /// as the target's own (`input::observe_recipient`); the answer names
    /// window 19 only when every press went to window 19 itself.
    #[test]
    fn a_click_names_its_window_only_when_every_press_went_to_the_window_itself() {
        let clicked = |presses: Vec<PressSeen>| named(&Delivered::Presses(presses));
        let seen = [
            // Window 19 stood in front and under the point.
            clicked(vec![press(19, 19)]),
            // Popup 23, window 19's own menu or dropdown, stood under the
            // point: the fence let the press through, and it went to the
            // popup.
            clicked(vec![press(19, 23)]),
            // Dialog 23, which window 19 owns, stood in front; the point was
            // over window 19, which a modal dialog keeps from input.
            clicked(vec![press(23, 19)]),
            // A double click whose first press opened popup 23 under the
            // point: the second press went to the popup.
            clicked(vec![press(19, 19), press(19, 23)]),
            // No press was seen.
            clicked(Vec::new()),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), None, None, None, None],
            "a click names window 19 only when every press went to it itself"
        );
    }

    /// A page of scroll is a call on an element of window 19's tree; the
    /// answer names window 19 only when one of them answered.
    #[test]
    fn a_page_scroll_names_its_window_only_when_a_page_call_answered() {
        let paged = |answered| named(&Delivered::TreeCalls { answered });
        let seen = [
            // Three pages asked, three answered.
            paged(3),
            // One of three answered: window 19 took that page.
            paged(1),
            // None answered — the element no longer scrolls, or refused
            // every call: nothing went into window 19.
            paged(0),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), Some(AIMED), None],
            "a page scroll names window 19 only when a page call answered"
        );
    }

    /// `setValue` on a check box toggles it until it stands at the asked
    /// state (`uia::toggle_to`); the answer names window 19 only when a
    /// Toggle call answered.
    #[test]
    fn a_toggle_names_its_window_only_when_a_toggle_call_answered() {
        let toggled = |answered| named(&Delivered::TreeCalls { answered });
        let seen = [
            // The check box took one Toggle.
            toggled(1),
            // An indeterminate one took two.
            toggled(2),
            // Its state could not be read, so no Toggle was sent, and the
            // write still answers success.
            toggled(0),
            // It already stood at the asked state: no Toggle was sent either.
            toggled(0),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), Some(AIMED), None, None],
            "a toggle names window 19 only when a Toggle call answered"
        );
    }

    /// The focus check let the keys go with window 19 itself in front, and
    /// `SendInput` took every event — which says how many it took, never
    /// which window they went to — so nothing saw window 19 take them.
    #[test]
    fn keys_name_no_window_even_with_the_window_itself_in_front() {
        assert_eq!(
            [named(&Delivered::Keys)],
            [None],
            "keys name no window: the input stream does not say where they went"
        );
    }

    /// A focused field of window 19 that holds "start " and answers a write
    /// with `answer`.
    struct Field {
        value: RefCell<String>,
        answer: Result<(), WriteFailure>,
    }

    impl TextField for Field {
        fn read(&self) -> Result<FieldText, ReadFailure> {
            Ok(FieldText {
                value: self.value.borrow().clone(),
                read_only: false,
            })
        }

        fn around_selection(&self) -> Option<Selection> {
            None
        }

        fn write(&self, text: &str) -> Result<(), WriteFailure> {
            if self.answer.is_ok() {
                *self.value.borrow_mut() = text.to_string();
            }
            self.answer.clone()
        }

        fn place_caret(&self, _utf16_offset: usize) {}
    }

    /// Text written into the focused element through its Value pattern
    /// (`replace_selection`, the accessibility road of `typeText` and
    /// `pasteText`): the answer names the element's window only when the
    /// write came back.
    #[test]
    fn a_write_into_the_focused_element_names_its_window_only_when_the_write_answered() {
        let written = |window: Option<u64>, answer: Result<(), WriteFailure>| {
            let field = Field {
                value: RefCell::new("start ".to_string()),
                answer,
            };
            let answered = replace_selection(&field, "end").write_answered();
            named(&Delivered::FocusedWrite { window, answered })
        };
        let seen = [
            // The field of window 19, by its own handle, answered the write.
            written(Some(19), Ok(())),
            // The write never came back (a timeout): the text may be in the
            // field, but nothing saw the field take it.
            written(
                Some(19),
                Err(WriteFailure::Unconfirmed("UIA_E_TIMEOUT".to_string())),
            ),
            // The element names no window of its own, only its process.
            written(None, Ok(())),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), None, None],
            "a write names its element's window only when the write answered"
        );
    }
}
