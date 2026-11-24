//! Which window a Windows act's input was seen to reach: the one policy the
//! Windows provider's answer names its window by (`actedWindow`, reviews
//! t-129021 §3, t-129057 §1 and t-129079 §1–2).
//!
//! Letting an act go and naming the window it went to are two judgements.
//! The click fence lets a press through over a window the target owns — its
//! menu, its dropdown — as the target's own, and the focus check lets keys
//! go while the target stands in front; but a step's evidence is a picture
//! of the window its answer names, so an act names a window only where it
//! saw that window take the input, and none where it only aimed there.
//!
//! Every road an act sends input by is a `Road`, and `Road::check` says
//! for each how it sees the window that took it: one `match` with no
//! catch-all, so a road added without a policy does not compile. An act
//! puts each road it took on its record (`Provider::sent_by`), and its
//! answer names the window only as `took` reads that record.
//!
//! The Windows provider is the one user. This half needs no platform, so its
//! tests run on the developer's machine (the `computer_use` module list).

/// Whether a Windows act's answer names a window at all. `false` leaves
/// every Windows step without a picture of its own (`window_unavailable`):
/// the coordinator's standing fallback, should a review find a road this
/// policy misses once more (t-128643 fix 6).
const NAMES_A_WINDOW: bool = true;

/// The windows one look of the click fence read just before a press, by
/// their numbers: the window in front, and the window under the point the
/// press goes to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PressSeen {
    pub(super) front: Option<u64>,
    pub(super) under: Option<u64>,
}

impl PressSeen {
    /// The press went to `window` itself: it stood in front and under the
    /// point. A window it owns in either place — a popup under the point, a
    /// dialog in front that keeps it from input — took the press instead,
    /// or may have.
    fn went_to(&self, window: u64) -> bool {
        self.front == Some(window) && self.under == Some(window)
    }
}

/// Calls a road made on one element: the window the element lives in, by
/// the root of its own window handle — `None` when it has no handle of its
/// own, and only a window's frame draws it — and how many of the calls came
/// back with success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ElementCalls {
    pub(super) window: Option<u64>,
    pub(super) answered: usize,
}

/// Every road a Windows act sends input by, by name: the one list the
/// evidence policy (`Road::check`) is written over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Road {
    /// `click` by number — the left button, once, no modifier — through the
    /// element's press pattern: Invoke, Toggle or Select.
    PressPattern(ElementCalls),
    /// `click` by number otherwise, or after the press pattern did not
    /// answer: presses at the element's centre.
    PressAtElementCentre(Vec<PressSeen>),
    /// `click` at a point of the window.
    PressAtPoint(Vec<PressSeen>),
    /// The modifier keys (Ctrl, Alt, Shift) held down around a click's
    /// presses (`input::deliver_click`).
    ClickModifiers,
    /// The act's own window brought to the front: before a click, and as an
    /// act asked to `restoreWindow` looks (`desktop_windows::bring_to_front`).
    Raise,
    /// `performSecondaryAction`: one action the element advertises — Invoke,
    /// Toggle, Select, Expand, Collapse, ScrollIntoView or a page of scroll.
    SecondaryAction(ElementCalls),
    /// `setValue` on text: the Value pattern's SetValue.
    SetText(ElementCalls),
    /// `setValue` on a number: the RangeValue pattern's SetValue.
    SetRange(ElementCalls),
    /// `setValue` on a check box: Toggles until it stands at the asked
    /// state, at most two (`uia::toggle_to`).
    Toggles(ElementCalls),
    /// `setValue` on a check box whose Toggle was refused, after none or one
    /// answered: the act answers that refusal.
    ToggleRefused,
    /// `typeText` or `pasteText` written into the focused element through
    /// its Value pattern (`replace_selection`).
    FocusedWrite(ElementCalls),
    /// The caret put after that text, on the same element, once the write
    /// answered; its own answer is not read (`TextField::place_caret`).
    CaretAfterWrite,
    /// A `hotkey` that selects all, through the focused element's Text
    /// pattern.
    SelectAllText(ElementCalls),
    /// A `hotkey` that selects all, as Ctrl+A keys: the focused element has
    /// no Text pattern, or its selection could not be read back.
    SelectAllKeys,
    /// `pressKey`, or a `hotkey` that does not select all: virtual keys.
    Keys,
    /// `typeText` typed as characters into whatever holds the focus.
    TypedText,
    /// `pasteText` through the clipboard and Ctrl+V.
    ClipboardPaste,
    /// `pasteText` typed instead: the clipboard holds what cannot be put back.
    TypedPaste,
    /// `scroll` by number through the Scroll pattern, a page a call.
    PageScroll(ElementCalls),
    /// `scroll` by number otherwise: the wheel at the element's centre.
    WheelAtElementCentre,
    /// `scroll` at a point of the window.
    WheelAtPoint,
    /// `drag`, by numbers or by points: a press, moves and a release.
    Drag,
}

/// How a road sees which window took its input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Check<'a> {
    /// The calls answered: the window they went to is the act's own, by the
    /// act's word alone — the element's own window is not read.
    Answered(&'a ElementCalls),
    /// The element its calls went to lives in the act's window — the root
    /// of its own window handle is that window — and a call answered.
    ElementWindow(&'a ElementCalls),
    /// Every press found the act's window itself in front and under the
    /// point, looked at just before it.
    PressLooks(&'a [PressSeen]),
    /// Nothing sees it: its input may have gone to another window.
    Nothing,
    /// It goes with another road of the same act and leaves the window to
    /// that road.
    Beside,
}

/// What one road says of the act's window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sight {
    /// It saw the act's window take its input.
    Took,
    /// It saw nothing take its input, and none of it can have gone to
    /// another window.
    Quiet,
    /// Its input went to another window, or may have.
    Elsewhere,
}

impl Road {
    /// How this road sees which window took its input: the evidence policy
    /// of every Windows road, in one place. No arm is a catch-all — a road
    /// added without its own arm does not compile.
    #[must_use]
    pub(super) fn check(&self) -> Check<'_> {
        match self {
            // Calls on an element of the act's window's tree.
            Self::PressPattern(calls)
            | Self::SecondaryAction(calls)
            | Self::SetText(calls)
            | Self::SetRange(calls)
            | Self::Toggles(calls)
            | Self::PageScroll(calls) => Check::Answered(calls),
            // Calls on the element that holds the focus, whose own window
            // handle the focus check read.
            Self::FocusedWrite(calls) | Self::SelectAllText(calls) => Check::ElementWindow(calls),
            // Presses through the click fence, which looks before each one.
            Self::PressAtElementCentre(presses) | Self::PressAtPoint(presses) => {
                Check::PressLooks(presses)
            }
            // Keys go into the input stream, which says how many events it
            // took and never which window they went to.
            Self::Keys
            | Self::SelectAllKeys
            | Self::TypedText
            | Self::ClipboardPaste
            | Self::TypedPaste => Check::Nothing,
            // The wheel and a drag at points of the screen: whatever window
            // stands there takes them, and nothing looks which.
            Self::WheelAtElementCentre | Self::WheelAtPoint | Self::Drag => Check::Nothing,
            // The act answers the refusal, and an answer that is an error
            // names no window, whatever the Toggle before it did.
            Self::ToggleRefused => Check::Nothing,
            // The modifier keys go into the input stream with the presses:
            // which window took them is not looked at, and the presses name
            // the window (an issue, scored 1 × 2 in review t-129079 §5).
            Self::ClickModifiers => Check::Beside,
            // A call on the very element the write went to.
            Self::CaretAfterWrite => Check::Beside,
            // Moves the act's own window and sends it no input.
            Self::Raise => Check::Beside,
        }
    }

    /// What this road says of `window`, the act's window, by its check.
    fn sight(&self, window: u64) -> Sight {
        let answered = |calls: &ElementCalls| {
            if calls.answered > 0 {
                Sight::Took
            } else {
                Sight::Quiet
            }
        };
        match self.check() {
            Check::Answered(calls) => answered(calls),
            // A call nobody answered may still have acted — in the element's
            // window, which is the act's own or another.
            Check::ElementWindow(calls) => match calls.window {
                Some(own) if own == window => answered(calls),
                Some(_) | None => Sight::Elsewhere,
            },
            Check::PressLooks(presses) => {
                if !presses.is_empty() && presses.iter().all(|press| press.went_to(window)) {
                    Sight::Took
                } else {
                    Sight::Elsewhere
                }
            }
            Check::Nothing => Sight::Elsewhere,
            Check::Beside => Sight::Quiet,
        }
    }
}

/// The window an act aimed at `window` saw take its input, by its number,
/// from the roads it took (`roads`): `window` when a road saw it take the
/// input and no road's input went, or may have gone, anywhere else; `None`
/// otherwise — and always while `NAMES_A_WINDOW` is off. What
/// `Provider::run_action` names, and the act's answer names its window only
/// when it is that one (`ActedWindow::if_it_took`).
#[must_use]
pub(super) fn took(roads: &[Road], window: u64) -> Option<u64> {
    took_while(NAMES_A_WINDOW, roads, window)
}

/// `took`, with `names` in the place of `NAMES_A_WINDOW`.
fn took_while(names: bool, roads: &[Road], window: u64) -> Option<u64> {
    if !names {
        return None;
    }
    let mut seen = false;
    for road in roads {
        match road.sight(window) {
            Sight::Took => seen = true,
            Sight::Quiet => {}
            Sight::Elsewhere => return None,
        }
    }
    seen.then_some(window)
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

    /// An element of window 19's tree that lives in window 19 itself — a
    /// control with a window handle of its own inside it.
    const IN_WINDOW: Option<u64> = Some(19);
    /// An element of window 19's tree that lives in popup 23: its context
    /// menu, a top-level window the tree shows under window 19 (review
    /// t-129079 §1).
    const IN_POPUP: Option<u64> = Some(23);
    /// An element with no window handle of its own: only a window's frame
    /// draws it, and nothing says which.
    const NO_HANDLE: Option<u64> = None;

    /// The window the act's answer names (`actedWindow`) when it took
    /// `roads`, as `Provider::run_action` names it.
    fn named(roads: &[Road]) -> Option<ActedWindow> {
        AIMED.if_it_took(took(roads, AIMED.id))
    }

    /// Calls on an element living in `window`, `answered` of which came back.
    fn calls(window: Option<u64>, answered: usize) -> ElementCalls {
        ElementCalls { window, answered }
    }

    /// One look of the click fence before a press: `front` in front, `under`
    /// under the point.
    fn press(front: u64, under: u64) -> PressSeen {
        PressSeen {
            front: Some(front),
            under: Some(under),
        }
    }

    /// A click by number through the element's press pattern names window 19
    /// only when the element lives in window 19 and the call answered. A
    /// press pattern that did not answer is followed by presses at the
    /// element's centre; the call may still have acted, in the element's
    /// window, so the click names window 19 only when that is window 19.
    #[test]
    fn an_accessibility_press_names_its_window_only_when_its_element_lives_in_it() {
        let pressed = |window, answered| named(&[Road::PressPattern(calls(window, answered))]);
        let then_pressed = |window| {
            named(&[
                Road::PressPattern(calls(window, 0)),
                Road::PressAtElementCentre(vec![press(19, 19)]),
            ])
        };
        let seen = [
            // A button of window 19 answered Invoke.
            pressed(IN_WINDOW, 1),
            // An item of popup 23 answered Invoke: the popup took it.
            pressed(IN_POPUP, 1),
            // An element with no handle of its own answered.
            pressed(NO_HANDLE, 1),
            // A button of window 19 did not answer, and the presses that
            // followed found window 19 itself in front and under the point.
            then_pressed(IN_WINDOW),
            // An item of popup 23 did not answer and may have acted before
            // the popup closed; the presses then found window 19.
            then_pressed(IN_POPUP),
            // The same, for an element with no handle of its own.
            then_pressed(NO_HANDLE),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), None, None, Some(AIMED), None, None],
            "an accessibility press names window 19 only when its element lives in window 19"
        );
    }

    /// `performSecondaryAction` names window 19 only when the element it
    /// went to lives in window 19.
    #[test]
    fn a_secondary_action_names_its_window_only_when_its_element_lives_in_it() {
        let acted = |window| named(&[Road::SecondaryAction(calls(window, 1))]);
        let seen = [
            // A check box of window 19 answered Toggle.
            acted(IN_WINDOW),
            // An item of popup 23 answered Expand.
            acted(IN_POPUP),
            // An element with no handle of its own answered Invoke.
            acted(NO_HANDLE),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), None, None],
            "a secondary action names window 19 only when its element lives in window 19"
        );
    }

    /// A value written — text, a number, a check box's state — or a page
    /// scrolled through the element's own pattern names window 19 only when
    /// the element lives in window 19.
    #[test]
    fn a_value_written_or_a_page_scrolled_names_its_window_only_when_its_element_lives_in_it() {
        let roads: [fn(ElementCalls) -> Road; 4] = [
            Road::SetText,
            Road::SetRange,
            Road::Toggles,
            Road::PageScroll,
        ];
        let seen: Vec<Option<ActedWindow>> = roads
            .iter()
            .flat_map(|road| {
                [IN_WINDOW, IN_POPUP, NO_HANDLE].map(|window| named(&[road(calls(window, 1))]))
            })
            .collect();
        assert_eq!(
            seen,
            [Some(AIMED), None, None].repeat(4),
            "text, a number, a toggle and a page each name window 19 only when their element lives in window 19"
        );
    }

    /// The click fence lets a press through over a window the target owns
    /// as the target's own (`input::observe_recipient`); the answer names
    /// window 19 only when every press went to window 19 itself.
    #[test]
    fn a_click_names_its_window_only_when_every_press_went_to_the_window_itself() {
        let clicked = |presses: Vec<PressSeen>| named(&[Road::PressAtPoint(presses)]);
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
            // A click by number at its element's centre, the same.
            named(&[Road::PressAtElementCentre(vec![press(19, 19)])]),
            named(&[Road::PressAtElementCentre(vec![press(19, 23)])]),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), None, None, None, None, Some(AIMED), None],
            "a click names window 19 only when every press went to it itself"
        );
    }

    /// A page of scroll is a call on an element of window 19; the answer
    /// names window 19 only when one of them answered.
    #[test]
    fn a_page_scroll_names_its_window_only_when_a_page_call_answered() {
        let paged = |answered| named(&[Road::PageScroll(calls(IN_WINDOW, answered))]);
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
        let toggled = |answered| named(&[Road::Toggles(calls(IN_WINDOW, answered))]);
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
    /// which window they went to — so nothing saw window 19 take them, on
    /// any road that types.
    #[test]
    fn keys_name_no_window_even_with_the_window_itself_in_front() {
        let seen = [
            Road::Keys,
            Road::SelectAllKeys,
            Road::TypedText,
            Road::ClipboardPaste,
            Road::TypedPaste,
        ]
        .map(|road| named(&[road]));
        assert_eq!(
            seen, [None; 5],
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
    /// write came back. The caret it puts after the text leaves the window
    /// to the write.
    #[test]
    fn a_write_into_the_focused_element_names_its_window_only_when_the_write_answered() {
        let written = |window: Option<u64>, answer: Result<(), WriteFailure>| {
            let field = Field {
                value: RefCell::new("start ".to_string()),
                answer,
            };
            let answered = replace_selection(&field, "end").write_answered();
            let mut roads = vec![Road::FocusedWrite(calls(window, usize::from(answered)))];
            if answered {
                roads.push(Road::CaretAfterWrite);
            }
            named(&roads)
        };
        let seen = [
            // The field of window 19, by its own handle, answered the write.
            written(IN_WINDOW, Ok(())),
            // The write never came back (a timeout): the text may be in the
            // field, but nothing saw the field take it.
            written(
                IN_WINDOW,
                Err(WriteFailure::Unconfirmed("UIA_E_TIMEOUT".to_string())),
            ),
            // The element names no window of its own, only its process.
            written(NO_HANDLE, Ok(())),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), None, None],
            "a write names its element's window only when the write answered"
        );
    }

    /// Every road, alone, in its plain case — an element of window 19 that
    /// answered, a press that found window 19 itself in front and under the
    /// point — names window 19 exactly where its check sees window 19 take
    /// the input (`Road::check`). A road that only goes with another names
    /// nothing by itself.
    #[test]
    fn every_road_alone_names_its_window_only_where_its_check_sees_it() {
        let own = || calls(IN_WINDOW, 1);
        let at = || vec![press(19, 19)];
        let roads = [
            (Road::PressPattern(own()), Some(AIMED)),
            (Road::PressAtElementCentre(at()), Some(AIMED)),
            (Road::PressAtPoint(at()), Some(AIMED)),
            (Road::ClickModifiers, None),
            (Road::Raise, None),
            (Road::SecondaryAction(own()), Some(AIMED)),
            (Road::SetText(own()), Some(AIMED)),
            (Road::SetRange(own()), Some(AIMED)),
            (Road::Toggles(own()), Some(AIMED)),
            (Road::ToggleRefused, None),
            (Road::FocusedWrite(own()), Some(AIMED)),
            (Road::CaretAfterWrite, None),
            (Road::SelectAllText(own()), Some(AIMED)),
            (Road::SelectAllKeys, None),
            (Road::Keys, None),
            (Road::TypedText, None),
            (Road::ClipboardPaste, None),
            (Road::TypedPaste, None),
            (Road::PageScroll(own()), Some(AIMED)),
            (Road::WheelAtElementCentre, None),
            (Road::WheelAtPoint, None),
            (Road::Drag, None),
        ];
        let wrong: Vec<_> = roads
            .iter()
            .filter(|(road, meant)| named(std::slice::from_ref(road)) != *meant)
            .collect();
        assert!(wrong.is_empty(), "roads named otherwise: {wrong:?}");
    }

    /// An act that took more than one road names window 19 only when a road
    /// saw window 19 take its input and no road's input went, or may have
    /// gone, to another window; a road that only goes with another leaves
    /// the window to it.
    #[test]
    fn an_act_names_its_window_only_when_none_of_its_roads_went_elsewhere() {
        let seen = [
            // A ctrl+click whose press window 19 took.
            named(&[
                Road::ClickModifiers,
                Road::PressAtPoint(vec![press(19, 19)]),
            ]),
            // Window 19 brought to the front, then its button answered.
            named(&[Road::Raise, Road::PressPattern(calls(IN_WINDOW, 1))]),
            // A ctrl+click whose press went to popup 23.
            named(&[
                Road::ClickModifiers,
                Road::PressAtPoint(vec![press(19, 23)]),
            ]),
            // Window 19 brought to the front, and nothing sent.
            named(&[Road::Raise]),
            // No road at all.
            named(&[]),
        ];
        assert_eq!(
            seen,
            [Some(AIMED), Some(AIMED), None, None, None],
            "an act names window 19 only when no road of it went elsewhere"
        );
    }

    /// With windows named at all switched off — the one line the fallback
    /// changes — no road names one: every Windows step is left without a
    /// picture of its own.
    #[test]
    fn no_road_names_a_window_while_windows_name_none() {
        let roads = [
            Road::PressPattern(calls(IN_WINDOW, 1)),
            Road::PressAtPoint(vec![press(19, 19)]),
            Road::FocusedWrite(calls(IN_WINDOW, 1)),
        ];
        let seen = roads
            .iter()
            .map(|road| {
                let alone = std::slice::from_ref(road);
                (took_while(true, alone, 19), took_while(false, alone, 19))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            seen,
            [(Some(19), None); 3],
            "named while windows are named, none once they are not"
        );
    }
}
