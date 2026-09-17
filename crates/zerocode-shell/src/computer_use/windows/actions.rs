//! The nine actions, each the helper's decision tree over the Windows
//! adapters: an accessibility pattern first, synthetic input when the element
//! has none, and a verification that says which road was taken.
//!
//! Two rules hold on every keyboard road. The element that receives text is
//! the one that holds focus NOW, read live and checked to live in the target
//! window — not the one an observation saw a moment ago. And once the
//! accessibility road may have written the text, no synthetic road follows:
//! `replace_selection` answers [`ReplaceOutcome`], and only
//! `NotApplicable` lets the synthetic road run.

use serde_json::{Map, Value};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use zerocode_core::computer_use_protocol::click_plan::MAX_CLICK_COUNT;
use zerocode_core::computer_use_protocol::coerce::{
    AttributeValue, ReadbackComparison, ValueCoercion,
};
use zerocode_core::computer_use_protocol::keys::{
    ChordWrites, Platform, parse_click_modifiers, parse_key_spec,
};
use zerocode_core::computer_use_protocol::marks::Pin;
use zerocode_core::computer_use_protocol::params;
use zerocode_core::computer_use_protocol::render::{pretty_action, preview};
use zerocode_core::computer_use_protocol::text_field::{ReplaceOutcome, replace_selection};
use zerocode_core::computer_use_protocol::validate::{
    MouseButton, ScrollDirection, SecretEntryFacts, SecretEntryVerdict, positive_integer,
    positive_number, secret_entry, synthetic_input_focus_failure,
};
use zerocode_core::computer_use_protocol::{
    ActionMetadata, ActionPath, ProviderError, Verification, error_code, unverified_reason,
};

use super::clipboard::{RestoreOutcome, Snapshot as ClipboardSnapshot};
use super::desktop_windows;
use super::input;
use super::provider::Provider;
use super::snapshot::Snapshot;
use super::uia::{self, LiveValue, UiaTextField, ValueKind};

fn coordinate(
    params: &Map<String, Value>,
    x_key: &str,
    y_key: &str,
    snapshot: &Snapshot,
) -> Result<(f64, f64), ProviderError> {
    let x = params::required_number(params, x_key)?;
    let y = params::required_number(params, y_key)?;
    Ok(snapshot.screen_point(x, y))
}

/// `requireTargetWindowFocused`.
fn require_focused(snapshot: &Snapshot, params: &Map<String, Value>) -> Result<(), ProviderError> {
    match synthetic_input_focus_failure(
        desktop_windows::is_focused(&snapshot.window),
        params::flag(params, "restoreWindow"),
    ) {
        None => Ok(()),
        Some(failure) => Err(ProviderError::new(
            error_code::WINDOW_NOT_FOCUSED,
            failure.message(&snapshot.app.name),
        )),
    }
}

/// The accessibility road's answer for a replaced selection.
fn replaced(verification: Verification) -> ActionMetadata {
    ActionMetadata::new(ActionPath::Accessibility)
        .named("ReplaceSelection")
        .verified_as(verification)
}

impl Provider {
    /// Keys never write a secret (B3, the core's `secret_entry`, as the macOS
    /// helper runs it): text — or the paste chord, or a printable key — into
    /// the platform's own password field is refused before anything is sent,
    /// and the person types it. Asked of the target's own focus before the
    /// accessibility road, and of whatever holds the focus once the window is
    /// in front before any key; this platform has no keyboard-wide secure
    /// mode to watch.
    fn gate_secret_entry(
        snapshot: &Snapshot,
        focused: Option<&IUIAutomationElement>,
        writes: ChordWrites,
    ) -> Result<(), ProviderError> {
        let verdict = secret_entry(SecretEntryFacts {
            writes_text: writes == ChordWrites::Character,
            pastes: writes == ChordWrites::Clipboard,
            secure_field: focused.is_some_and(uia::cached_is_password),
            secure_input_on: false,
            secure_input_by_receiver: false,
            focus_read: focused.is_some(),
        });
        if verdict == SecretEntryVerdict::PersonsEntry {
            return Err(ProviderError::new(
                error_code::SECURE_INPUT,
                format!("{}: {}", unverified_reason::SECRET_FIELD, snapshot.app.name),
            ));
        }
        Ok(())
    }

    /// The element holding keyboard focus right now, when it belongs to the
    /// target window — by its window handle when it has one, by its process
    /// when it does not. Anything else is not a place the agent's text may go.
    fn live_focused(&self, snapshot: &Snapshot) -> Option<IUIAutomationElement> {
        let element = self.client.focused_element()?;
        let in_target = match uia::cached_root_window(&element) {
            Some(root) => root.0 as isize == snapshot.window.hwnd,
            None => uia::cached_process_id(&element) == Some(snapshot.app.pid),
        };
        in_target.then_some(element)
    }

    pub(super) fn click(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let button = MouseButton::parse(params::optional_str(params, "mouseButton"))?;
        let count = positive_integer(
            params.get("clickCount").and_then(Value::as_f64),
            1,
            "clickCount",
        )? as usize;
        if count > MAX_CLICK_COUNT {
            return Err(ProviderError::invalid_argument(format!(
                "clickCount must be at most {MAX_CLICK_COUNT}"
            )));
        }
        let modifiers = parse_click_modifiers(params::optional_str(params, "modifiers"))?;
        // Refused before anything moves: a modifier the platform will not
        // hold must not cost a window its foreground.
        input::click_modifier_keys(&modifiers)?;
        // A click into the target makes the next keyboard action safe, on
        // either road.
        desktop_windows::bring_to_front(&snapshot.window);
        if let Some(index) = params::optional_index(params, "elementIndex")? {
            let (record, element) = snapshot.element(index)?;
            // A click by a mark's number presses only what a person would hit
            // there: the control, on top at its centre, its window in front.
            if Pin::from_params(params)?.is_some() {
                let centre = snapshot.center_of(record).ok_or_else(|| {
                    ProviderError::new(
                        error_code::ELEMENT_NOT_CLICKABLE,
                        format!("element {index} has no clickable frame"),
                    )
                })?;
                if !self.client.on_top(element, centre)? {
                    return Err(ProviderError::new(
                        error_code::ELEMENT_NOT_FOUND,
                        format!(
                            "element {index} is covered at its centre; look again with --marks"
                        ),
                    ));
                }
            }
            if modifiers.is_empty() && count <= 1 {
                // `performClickAction`: the element's own press for the
                // left button, its own context menu for the right one.
                let performed = match button {
                    MouseButton::Left => uia::primary_click_action(&record.actions)
                        .filter(|action| uia::perform_action(element, action).is_ok())
                        .map(str::to_string),
                    MouseButton::Right => uia::show_context_menu(element)
                        .ok()
                        .map(|()| "ShowContextMenu".to_string()),
                    MouseButton::Middle => None,
                };
                if let Some(action) = performed {
                    return Ok(ActionMetadata::new(ActionPath::Accessibility).named(action));
                }
            }
            let Some((x, y)) = snapshot.center_of(record) else {
                return Err(ProviderError::new(
                    error_code::ELEMENT_NOT_CLICKABLE,
                    format!("element {index} has no clickable frame"),
                ));
            };
            input::deliver_click(x, y, button, count, &modifiers, snapshot.recipient())?;
            return Ok(ActionMetadata::new(ActionPath::Synthetic)
                .fallback("actionUnsupported")
                .verified_as(Verification::synthetic_input()));
        }
        let (x, y) = coordinate(params, "x", "y", &snapshot)?;
        input::deliver_click(x, y, button, count, &modifiers, snapshot.recipient())?;
        Ok(ActionMetadata::new(ActionPath::Synthetic).verified_as(Verification::synthetic_input()))
    }

    pub(super) fn perform_secondary_action(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let index = params::required_index(params, "elementIndex")?;
        let (record, element) = snapshot.element(index)?;
        let requested = params::required_string(params, "action")?;
        let action = record
            .actions
            .iter()
            .find(|action| {
                action.eq_ignore_ascii_case(&requested)
                    || pretty_action(action).eq_ignore_ascii_case(&requested)
            })
            .cloned()
            .ok_or_else(|| {
                ProviderError::new(
                    error_code::ACTION_NOT_SUPPORTED,
                    format!("'{requested}' is not a valid secondary action for element {index}"),
                )
            })?;
        uia::perform_action(element, &action)?;
        Ok(ActionMetadata::new(ActionPath::Accessibility).named(action))
    }

    pub(super) fn set_value(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let index = params::required_index(params, "elementIndex")?;
        let (_, element) = snapshot.element(index)?;
        let secret = uia::cached_is_password(element);
        let expected = params::required_string_allowing_empty(params, "value")?;
        let not_settable = || {
            ProviderError::new(
                error_code::VALUE_NOT_SETTABLE,
                format!("element {index} is not settable"),
            )
        };
        let Some(probe) = uia::probe_value(element) else {
            return Err(not_settable());
        };
        if probe.read_only {
            return Err(not_settable());
        }
        // A value the provider will not read (a password edit) is still
        // settable: as the helper does without a readable current value,
        // the text is written as given and the readback is unsupported.
        let existing = match probe.value {
            Ok(LiveValue::Text(value)) => Some(AttributeValue::String(value)),
            Ok(LiveValue::Range(value)) => Some(AttributeValue::Double(value)),
            Ok(LiveValue::Toggle(on)) => Some(AttributeValue::Boolean(on)),
            Err(_) => None,
        };
        let coercion = ValueCoercion::new(existing.as_ref(), &expected);
        match (probe.kind, &coercion.write_value) {
            (ValueKind::Text, AttributeValue::String(value)) => {
                uia::set_text_value(element, value)?
            }
            (ValueKind::Range, AttributeValue::Double(value)) => {
                uia::set_range_value(element, *value)?
            }
            (ValueKind::Toggle, AttributeValue::Boolean(value)) => uia::toggle_to(element, *value)?,
            (ValueKind::Range, _) => {
                return Err(ProviderError::invalid_argument(format!(
                    "element {index} takes a number, not '{expected}'"
                )));
            }
            (ValueKind::Toggle, _) => {
                return Err(ProviderError::invalid_argument(format!(
                    "element {index} takes true or false, not '{expected}'"
                )));
            }
            (ValueKind::Text, _) => return Err(not_settable()),
        }
        // A password field is never read back, and what was written is not
        // repeated in the answer.
        if secret {
            return Ok(ActionMetadata::new(ActionPath::Accessibility)
                .named("SetValue")
                .verified_as(Verification::unverified(
                    unverified_reason::SECRET_FIELD,
                    None,
                    None,
                )));
        }
        let readback = match uia::live_value(element) {
            Some(LiveValue::Text(value)) => Some(AttributeValue::String(value)),
            Some(LiveValue::Range(value)) => Some(AttributeValue::Double(value)),
            Some(LiveValue::Toggle(on)) => Some(AttributeValue::Boolean(on)),
            None => None,
        };
        let verification = match coercion.compare(readback.as_ref()) {
            ReadbackComparison::Match { actual_preview } => {
                Verification::verified("value", Some(expected), Some(actual_preview))
            }
            ReadbackComparison::Mismatch { actual_preview } => Verification::unverified(
                unverified_reason::VALUE_MISMATCH,
                Some(expected),
                Some(actual_preview),
            ),
            ReadbackComparison::Unsupported => Verification::unverified(
                unverified_reason::READBACK_UNSUPPORTED,
                Some(expected),
                None,
            ),
        };
        Ok(ActionMetadata::new(ActionPath::Accessibility)
            .named("SetValue")
            .verified_as(verification))
    }

    pub(super) fn type_text(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let text = params::required_string(params, "text")?;
        let target = self.live_focused(&snapshot);
        Self::gate_secret_entry(&snapshot, target.as_ref(), ChordWrites::Character)?;
        let mut fallback = None;
        if let Some(element) = target {
            match replace_selection(&UiaTextField(&element), &text) {
                ReplaceOutcome::Applied(verification) => return Ok(replaced(verification)),
                ReplaceOutcome::NotApplicable(reason) => fallback = Some(reason),
            }
        }
        require_focused(&snapshot, params)?;
        Self::gate_secret_entry(
            &snapshot,
            self.client.focused_element().as_ref(),
            ChordWrites::Character,
        )?;
        input::type_text(&text)?;
        let mut metadata = ActionMetadata::new(ActionPath::Synthetic)
            .named("typeText")
            .verified_as(Verification::synthetic_input());
        if let Some(reason) = fallback {
            metadata = metadata.fallback(reason);
        }
        Ok(metadata)
    }

    pub(super) fn press_key(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let chord = parse_key_spec(&params::required_string(params, "key")?)?;
        let writes = chord.writes_on(Platform::Windows);
        Self::gate_secret_entry(&snapshot, self.live_focused(&snapshot).as_ref(), writes)?;
        require_focused(&snapshot, params)?;
        Self::gate_secret_entry(&snapshot, self.client.focused_element().as_ref(), writes)?;
        input::press_chord(&chord)?;
        Ok(ActionMetadata::new(ActionPath::Synthetic)
            .named("pressKey")
            .verified_as(Verification::synthetic_input()))
    }

    pub(super) fn hotkey(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        // One parse decides both roads: a chord that selects all on Windows
        // (`ctrl+a`, `cmdorctrl+a`) takes the Text pattern when the focused
        // element has one and Ctrl+A when it does not; `cmd+a` is Win+A on
        // both roads, as the skill says it is.
        let chord = parse_key_spec(&params::required_string(params, "key")?)?;
        let writes = chord.writes_on(Platform::Windows);
        Self::gate_secret_entry(&snapshot, self.live_focused(&snapshot).as_ref(), writes)?;
        if chord.selects_all_on(Platform::Windows)
            && let Some(element) = self.live_focused(&snapshot)
            && let Some(whole) = uia::select_all_text(&element)
        {
            let verification = if whole {
                Verification::verified(
                    "selection",
                    None,
                    uia::document_text(&element).map(|text| preview(&text, 120)),
                )
            } else {
                Verification::unverified(unverified_reason::PROVIDER_UNAVAILABLE, None, None)
            };
            return Ok(ActionMetadata::new(ActionPath::Accessibility)
                .named("SelectAll")
                .verified_as(verification));
        }
        require_focused(&snapshot, params)?;
        Self::gate_secret_entry(&snapshot, self.client.focused_element().as_ref(), writes)?;
        input::press_chord(&chord)?;
        Ok(ActionMetadata::new(ActionPath::Synthetic)
            .named("hotkey")
            .verified_as(Verification::synthetic_input()))
    }

    pub(super) fn paste_text(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let text = params::required_string(params, "text")?;
        let target = self.live_focused(&snapshot);
        Self::gate_secret_entry(&snapshot, target.as_ref(), ChordWrites::Clipboard)?;
        if let Some(element) = target
            && let ReplaceOutcome::Applied(verification) =
                replace_selection(&UiaTextField(&element), &text)
        {
            return Ok(replaced(verification));
        }
        require_focused(&snapshot, params)?;
        Self::gate_secret_entry(
            &snapshot,
            self.client.focused_element().as_ref(),
            ChordWrites::Clipboard,
        )?;
        // The clipboard road exists only when everything on the clipboard
        // can be put back. Otherwise the text is typed, the clipboard is not
        // touched, and the answer says why the road changed.
        let held = ClipboardSnapshot::take()?;
        let replaced = match held.replace_with_text(&text) {
            Ok(replaced) => replaced,
            Err((_, why)) => {
                input::type_text(&text)?;
                return Ok(ActionMetadata::new(ActionPath::Synthetic)
                    .named("typeText")
                    .fallback("clipboard_not_preservable")
                    .verified_as(Verification::unverified(
                        unverified_reason::SYNTHETIC_INPUT,
                        None,
                        Some(why.message),
                    )));
            }
        };
        let pasted = input::press_chord(&parse_key_spec("ctrl+v")?);
        // The app reads the clipboard when it handles the keystroke, which
        // is after SendInput returns; give it a moment before the text is
        // taken back.
        std::thread::sleep(std::time::Duration::from_millis(250));
        let restored = replaced.restore();
        pasted?;
        let verification = match restored {
            RestoreOutcome::Restored | RestoreOutcome::Superseded => {
                Verification::unverified(unverified_reason::CLIPBOARD_PASTE, None, None)
            }
            RestoreOutcome::Failed(why) => Verification::unverified(
                unverified_reason::CLIPBOARD_RESTORE_FAILED,
                None,
                Some(why),
            ),
        };
        Ok(ActionMetadata::new(ActionPath::Clipboard)
            .named("paste")
            .verified_as(verification))
    }

    pub(super) fn scroll(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let direction = ScrollDirection::parse(&params::required_string(params, "direction")?)?;
        let pages = positive_number(params.get("pages").and_then(Value::as_f64), 1.0, "pages")?;
        if let Some(index) = params::optional_index(params, "elementIndex")? {
            let (record, element) = snapshot.element(index)?;
            if pages.fract() == 0.0
                && let Some(action) = direction.page_action(&record.actions)
            {
                let action = action.to_string();
                for _ in 0..(pages as usize).max(1) {
                    let _ = uia::scroll_page(element, direction);
                }
                return Ok(ActionMetadata::new(ActionPath::Accessibility).named(action));
            }
            let Some((x, y)) = snapshot.center_of(record) else {
                return Err(ProviderError::new(
                    error_code::ELEMENT_NOT_FOUND,
                    format!("element {index} has no scrollable frame"),
                ));
            };
            input::scroll(x, y, direction, pages)?;
            return Ok(ActionMetadata::new(ActionPath::Synthetic).fallback("actionUnsupported"));
        }
        let (x, y) = coordinate(params, "x", "y", &snapshot)?;
        input::scroll(x, y, direction, pages)?;
        Ok(ActionMetadata::new(ActionPath::Synthetic))
    }

    pub(super) fn drag(
        &mut self,
        params: &Map<String, Value>,
    ) -> Result<ActionMetadata, ProviderError> {
        let snapshot = self.current_snapshot(params)?;
        let from_index = params::optional_index(params, "fromElementIndex")?;
        let to_index = params::optional_index(params, "toElementIndex")?;
        let (start, end) = if let (Some(from), Some(to)) = (from_index, to_index) {
            let (from_record, _) = snapshot.element(from)?;
            let (to_record, _) = snapshot.element(to)?;
            match (
                snapshot.center_of(from_record),
                snapshot.center_of(to_record),
            ) {
                (Some(start), Some(end)) => (start, end),
                _ => {
                    return Err(ProviderError::new(
                        error_code::ELEMENT_NOT_FOUND,
                        "drag element has no frame",
                    ));
                }
            }
        } else {
            (
                coordinate(params, "fromX", "fromY", &snapshot)?,
                coordinate(params, "toX", "toY", &snapshot)?,
            )
        };
        input::drag(start, end)?;
        Ok(ActionMetadata::new(ActionPath::Synthetic))
    }
}
