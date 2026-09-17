//! Synthetic input: `SendInput` for the mouse and the keyboard, and the
//! foreground probes the click fence asks.
//!
//! Text is typed as `KEYEVENTF_UNICODE` events, one per UTF-16 unit, which
//! delivers Korean, emoji and every other character without a keyboard
//! layout in the way — the same road the macOS helper takes with
//! `keyboardSetUnicodeString`. A line break or a tab in the text is a
//! CHARACTER on that road, never the Enter or Tab KEY
//! (`super::super::typed_text` says which unit and why). Named keys and
//! hotkeys go through virtual keys; punctuation is looked up on the active
//! layout so `--key ;` means the semicolon key this keyboard has.
//!
//! `SendInput` answers with how many events it inserted, and fewer than
//! asked is the platform refusing (another process holds the input queue
//! with `BlockInput`, the target runs elevated and UIPI drops the events).
//! Every road here carries that answer up as the truth of what was
//! delivered, and releases whatever it had pressed before it says so.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSE_EVENT_FLAGS,
    MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput,
    VIRTUAL_KEY, VK_0, VK_A, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_HOME,
    VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT,
    VK_SPACE, VK_TAB, VK_UP, VkKeyScanW,
};
use zerocode_core::computer_use_protocol::click_plan::{
    self, DeliveryFailure, Recipient, RecipientObservation, Step, partial_delivery_recovery,
};
use zerocode_core::computer_use_protocol::keys::{KeyChord, Modifier};
use zerocode_core::computer_use_protocol::validate::{MouseButton, ScrollDirection};
use zerocode_core::computer_use_protocol::{ProviderError, error_code};

use super::super::geometry::{virtual_desk_coordinate, wheel_delta};
use super::super::typed_text::{characters_within, keystroke_units};
use super::desktop_windows;

/// `SendInput` inserted fewer events than it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InputRefusal {
    pub inserted: usize,
    pub requested: usize,
}

impl InputRefusal {
    fn into_error(self) -> ProviderError {
        ProviderError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!(
                "the input was blocked (another application holds the input queue, or the target runs elevated): {} of {} events were accepted",
                self.inserted, self.requested
            ),
        )
    }
}

fn send(inputs: &[INPUT]) -> Result<(), InputRefusal> {
    // SAFETY: `inputs` is a slice of fully initialised INPUT structs whose
    // size is passed alongside.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) } as usize;
    if sent == inputs.len() {
        Ok(())
    } else {
        Err(InputRefusal {
            inserted: sent,
            requested: inputs.len(),
        })
    }
}

fn mouse_input(dx: i32, dy: i32, data: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn key_input(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn absolute(x: f64, y: f64) -> (i32, i32) {
    virtual_desk_coordinate(x, y, &desktop_windows::virtual_screen())
}

fn absolute_move(x: f64, y: f64) -> INPUT {
    let (dx, dy) = absolute(x, y);
    mouse_input(
        dx,
        dy,
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

/// Move the pointer to a screen point.
pub(super) fn move_to(x: f64, y: f64) -> Result<(), ProviderError> {
    send(&[absolute_move(x, y)]).map_err(InputRefusal::into_error)
}

fn button_flags(button: MouseButton, down: bool) -> MOUSE_EVENT_FLAGS {
    match (button, down) {
        (MouseButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    }
}

/// A button press or release at the pointer's current place.
pub(super) fn button(button: MouseButton, down: bool) -> Result<(), ProviderError> {
    send(&[mouse_input(0, 0, 0, button_flags(button, down))]).map_err(InputRefusal::into_error)
}

/// The virtual key for a modifier on this platform: `CmdOrCtrl` is Ctrl,
/// `Cmd`/`Meta`/`Super`/`Win` is the Windows key.
pub(super) fn modifier_key(modifier: Modifier) -> VIRTUAL_KEY {
    match modifier {
        Modifier::Primary | Modifier::Control => VK_CONTROL,
        Modifier::Command => VK_LWIN,
        Modifier::Alt => VK_MENU,
        Modifier::Shift => VK_SHIFT,
    }
}

/// The keys held around a click. The Windows key is refused: no Windows
/// application reads a Win-click, and releasing the key after nothing but
/// mouse events opens the Start menu — which then owns the focus every
/// keystroke that follows would have gone to the target with.
pub(super) fn click_modifier_keys(
    modifiers: &[Modifier],
) -> Result<Vec<VIRTUAL_KEY>, ProviderError> {
    modifiers
        .iter()
        .map(|modifier| match modifier {
            Modifier::Command => Err(ProviderError::invalid_argument(
                "the Windows key is not a click modifier on Windows; use ctrl (or cmdorctrl), alt or shift",
            )),
            other => Ok(modifier_key(*other)),
        })
        .collect()
}

/// A named key from the shared vocabulary, as a virtual key plus whether it
/// is an extended key and whether the layout wants Shift held for it.
pub(super) fn key_code(name: &str) -> Result<(VIRTUAL_KEY, bool, bool), ProviderError> {
    let named = match name {
        "return" | "enter" => Some((VK_RETURN, false)),
        "tab" => Some((VK_TAB, false)),
        "space" => Some((VK_SPACE, false)),
        // The helper's `delete` is the Backspace key; `forwarddelete` deletes forward.
        "backspace" | "delete" => Some((VK_BACK, false)),
        "forwarddelete" => Some((VK_DELETE, true)),
        "escape" | "esc" => Some((VK_ESCAPE, false)),
        "left" => Some((VK_LEFT, true)),
        "right" => Some((VK_RIGHT, true)),
        "up" => Some((VK_UP, true)),
        "down" => Some((VK_DOWN, true)),
        "insert" => Some((VK_INSERT, true)),
        "home" => Some((VK_HOME, true)),
        "end" => Some((VK_END, true)),
        "pageup" | "page_up" => Some((VK_PRIOR, true)),
        "pagedown" | "page_down" => Some((VK_NEXT, true)),
        _ => None,
    };
    if let Some((vk, extended)) = named {
        return Ok((vk, extended, false));
    }
    let mut chars = name.chars();
    if let (Some(character), None) = (chars.next(), chars.next()) {
        if character.is_ascii_lowercase() {
            return Ok((
                VIRTUAL_KEY(VK_A.0 + (character as u16 - b'a' as u16)),
                false,
                false,
            ));
        }
        if character.is_ascii_digit() {
            return Ok((
                VIRTUAL_KEY(VK_0.0 + (character as u16 - b'0' as u16)),
                false,
                false,
            ));
        }
        let mut unit = [0u16; 2];
        let unit = character.encode_utf16(&mut unit)[0];
        // SAFETY: a plain layout lookup.
        let scan = unsafe { VkKeyScanW(unit) };
        if scan != -1 {
            let vk = VIRTUAL_KEY((scan & 0xff) as u16);
            let shift = (scan >> 8) & 1 == 1;
            return Ok((vk, false, shift));
        }
    }
    Err(ProviderError::invalid_argument(format!(
        "unsupported key '{name}' on this keyboard layout"
    )))
}

fn key_event(vk: VIRTUAL_KEY, extended: bool, down: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !down {
        flags |= KEYEVENTF_KEYUP;
    }
    key_input(vk, 0, flags)
}

/// Release keys that may be down, one event at a time so one refusal does
/// not stop the rest. Best effort: this runs on a road that already failed.
fn release_keys(keys: &[(VIRTUAL_KEY, bool)]) {
    for (vk, extended) in keys.iter().rev() {
        let _ = send(&[key_event(*vk, *extended, false)]);
    }
}

/// `pressKey`/`hotkey`: modifiers down in order, the key down and up, the
/// modifiers up in reverse — one `SendInput` so nothing interleaves. A
/// refusal partway leaves keys down; they are released before the refusal
/// is reported.
pub(super) fn press_chord(chord: &KeyChord) -> Result<(), ProviderError> {
    let (vk, extended, layout_shift) = key_code(&chord.key)?;
    let mut modifiers: Vec<VIRTUAL_KEY> = chord
        .modifiers
        .iter()
        .map(|modifier| modifier_key(*modifier))
        .collect();
    if layout_shift && !modifiers.contains(&VK_SHIFT) {
        modifiers.push(VK_SHIFT);
    }
    let mut inputs = Vec::with_capacity(modifiers.len() * 2 + 2);
    for modifier in &modifiers {
        inputs.push(key_event(*modifier, false, true));
    }
    inputs.push(key_event(vk, extended, true));
    inputs.push(key_event(vk, extended, false));
    for modifier in modifiers.iter().rev() {
        inputs.push(key_event(*modifier, false, false));
    }
    send(&inputs).map_err(|refusal| {
        if refusal.inserted > 0 {
            let mut pressed: Vec<(VIRTUAL_KEY, bool)> =
                modifiers.iter().map(|vk| (*vk, false)).collect();
            pressed.push((vk, extended));
            release_keys(&pressed);
        }
        refusal.into_error()
    })
}

/// The UTF-16 units `typeText` sends for `text`, one key down and up each.
fn unicode_events(units: &[u16]) -> Vec<INPUT> {
    let mut inputs = Vec::with_capacity(units.len() * 2);
    for unit in units {
        inputs.push(key_input(VIRTUAL_KEY(0), *unit, KEYEVENTF_UNICODE));
        inputs.push(key_input(
            VIRTUAL_KEY(0),
            *unit,
            KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
        ));
    }
    inputs
}

/// `typeText`: every keystroke unit of the text as a Unicode key down and
/// up. A refusal partway says how much of the text arrived, because the
/// field now holds exactly that much.
pub(super) fn type_text(text: &str) -> Result<(), ProviderError> {
    let units = keystroke_units(text);
    let inputs = unicode_events(&units);
    // Batches of a few hundred events keep any one SendInput short.
    let mut accepted_events = 0;
    for batch in inputs.chunks(256) {
        match send(batch) {
            Ok(()) => accepted_events += batch.len(),
            Err(refusal) => {
                accepted_events += refusal.inserted;
                // An accepted down whose up was refused: release it.
                if refusal.inserted % 2 == 1 {
                    let _ = send(&batch[refusal.inserted..refusal.inserted + 1]);
                }
                let typed = characters_within(text, accepted_events / 2);
                return Err(ProviderError::new(
                    error_code::ACCESSIBILITY_ERROR,
                    format!(
                        "the input was blocked (another application holds the input queue, or the target runs elevated) after {typed} of {} characters; run get-app-state and verify the field before retrying",
                        text.chars().count()
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// `scroll`: the wheel at a screen point, `pages` pages in `direction`.
pub(super) fn scroll(
    x: f64,
    y: f64,
    direction: ScrollDirection,
    pages: f64,
) -> Result<(), ProviderError> {
    move_to(x, y)?;
    let (flags, delta) = match direction {
        ScrollDirection::Up => (MOUSEEVENTF_WHEEL, wheel_delta(pages, true)),
        ScrollDirection::Down => (MOUSEEVENTF_WHEEL, wheel_delta(pages, false)),
        // Horizontal wheels are signed the other way: positive scrolls right.
        ScrollDirection::Left => (MOUSEEVENTF_HWHEEL, wheel_delta(pages, false)),
        ScrollDirection::Right => (MOUSEEVENTF_HWHEEL, wheel_delta(pages, true)),
    };
    send(&[mouse_input(0, 0, delta, flags)]).map_err(InputRefusal::into_error)
}

/// `drag`: press at the start, ten interpolated moves, release at the end.
/// A refusal after the press releases the button before it is reported.
pub(super) fn drag(from: (f64, f64), to: (f64, f64)) -> Result<(), ProviderError> {
    move_to(from.0, from.1)?;
    button(MouseButton::Left, true)?;
    let moved = (1..=10).try_for_each(|step| {
        let progress = f64::from(step) / 10.0;
        move_to(
            from.0 + (to.0 - from.0) * progress,
            from.1 + (to.1 - from.1) * progress,
        )?;
        std::thread::sleep(std::time::Duration::from_millis(15));
        Ok::<(), ProviderError>(())
    });
    let released = button(MouseButton::Left, false);
    moved.and(released)
}

/// `currentSyntheticClickRecipient`: who owns the foreground, and — when
/// that is the target — who is under the point about to be clicked. A
/// window the target owns (its own context menu, dropdown or tooltip) IS the
/// target: a right-click that opened a menu under the pointer has not been
/// stolen. Any other window, in this process or another, is a change.
pub(super) fn observe_recipient(target: Recipient, x: f64, y: f64) -> RecipientObservation {
    let Some((pid, root)) = desktop_windows::foreground() else {
        return RecipientObservation::Dismissed;
    };
    let focused = Recipient {
        owner_pid: pid,
        window_id: root as u64,
    };
    if focused != target
        && !desktop_windows::is_owned_by_target(
            desktop_windows::hwnd_from_id(focused.window_id),
            target,
        )
    {
        return RecipientObservation::Focused(focused);
    }
    match desktop_windows::root_at(x, y) {
        Some((pid, root)) => {
            let under = Recipient {
                owner_pid: pid,
                window_id: root as u64,
            };
            if under == target
                || desktop_windows::is_owned_by_target(
                    desktop_windows::hwnd_from_id(under.window_id),
                    target,
                )
            {
                RecipientObservation::Focused(target)
            } else {
                RecipientObservation::Focused(under)
            }
        }
        None => RecipientObservation::Unavailable,
    }
}

/// A fenced click at a screen point, with modifiers held around it. Every
/// refusal — a modifier the platform would not press, a button event it
/// dropped, a window that took the pointer — ends with nothing held and the
/// truth of what was delivered.
pub(super) fn deliver_click(
    x: f64,
    y: f64,
    mouse_button: MouseButton,
    count: usize,
    modifiers: &[Modifier],
    target: Recipient,
) -> Result<(), ProviderError> {
    let held = click_modifier_keys(modifiers)?;
    let held_pairs: Vec<(VIRTUAL_KEY, bool)> = held.iter().map(|vk| (*vk, false)).collect();
    if !held.is_empty() {
        let downs: Vec<INPUT> = held.iter().map(|vk| key_event(*vk, false, true)).collect();
        if let Err(refusal) = send(&downs) {
            release_keys(&held_pairs);
            return Err(refusal.into_error());
        }
    }
    let outcome = click_plan::deliver(
        count,
        target,
        || observe_recipient(target, x, y),
        |step| -> Result<INPUT, ProviderError> {
            Ok(match step {
                Step::Move => absolute_move(x, y),
                Step::ButtonDown { .. } => mouse_input(0, 0, 0, button_flags(mouse_button, true)),
                Step::ButtonUp { .. } => mouse_input(0, 0, 0, button_flags(mouse_button, false)),
            })
        },
        |input| send(&[input]).map_err(InputRefusal::into_error),
        |micros| std::thread::sleep(std::time::Duration::from_micros(u64::from(micros))),
    );
    if let Err(DeliveryFailure::Event {
        button_held: true, ..
    }) = &outcome
    {
        let _ = send(&[mouse_input(0, 0, 0, button_flags(mouse_button, false))]);
    }
    release_keys(&held_pairs);
    match outcome {
        Ok(()) => Ok(()),
        Err(DeliveryFailure::Fence(failure)) => Err(ProviderError::new(
            error_code::WINDOW_NOT_FOCUSED,
            failure.message(),
        )),
        Err(DeliveryFailure::Event {
            error,
            delivered_presses,
            ..
        }) => Err(ProviderError::new(
            error.code,
            format!(
                "{}; {}",
                error.message,
                partial_delivery_recovery(delivered_presses)
            ),
        )),
    }
}
