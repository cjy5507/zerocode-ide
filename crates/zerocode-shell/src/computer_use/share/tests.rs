use std::cell::Cell;
use std::rc::Rc;

use serde_json::{Value, json};
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::error_code;

use super::stand_in;
use super::{Decision, admit, ensure_current, give_back, state, take_control};

fn person(answer: Decision) -> (stand_in::Standing, Rc<Cell<u32>>) {
    let asked = Rc::new(Cell::new(0));
    let counting = Rc::clone(&asked);
    let standing = stand_in::stand(move |_| {
        counting.set(counting.get() + 1);
        answer
    });
    (standing, asked)
}

fn click(app: &str) -> Value {
    json!({ "app": app, "elementIndex": 2 })
}

fn refused(method: ComputerMethod, params: &Value) -> Option<String> {
    admit(method, params).err().map(|error| error.code)
}

/// The person's words: only the target is shared. The first act in an app
/// asks once; a yes holds the agent to that window and the same app is not
/// asked again.
#[test]
fn the_first_act_asks_once_and_a_yes_holds_the_agent_to_that_window() {
    let (_person, asked) = person(Decision::Allowed);
    assert!(admit(ComputerMethod::Click, &click("Safari")).is_ok());
    assert!(admit(ComputerMethod::Click, &click("safari")).is_ok());
    assert!(admit(ComputerMethod::SetValue, &click("Safari")).is_ok());
    assert_eq!(asked.get(), 1, "one card for one app");
    assert_eq!(
        refused(ComputerMethod::Screenshot, &json!({})).as_deref(),
        Some(error_code::SHARE_SCOPE),
        "the whole screen is not the shared window"
    );
    assert_eq!(
        refused(ComputerMethod::GetAppState, &json!({ "app": "Notes" })).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(state()["target"], "safari");
    assert!(
        stand_in::notices()
            .iter()
            .any(|notice| notice["target"] == "safari"),
        "the band hears the share: {:?}",
        stand_in::notices()
    );
}

/// A no refuses that app.
#[test]
fn a_no_refuses_that_app() {
    let (_person, asked) = person(Decision::Refused);
    for _ in 0..2 {
        assert_eq!(
            refused(ComputerMethod::Click, &click("Notes")).as_deref(),
            Some(error_code::SHARE_SCOPE)
        );
    }
    assert_eq!(asked.get(), 1);
}

/// Silence — the person away — leaves the agent working as before, and the
/// app is not asked again: an unattended run never stops at a card.
#[test]
fn silence_leaves_the_agent_working_and_asks_no_more() {
    let (_person, asked) = person(Decision::TimedOut);
    for _ in 0..3 {
        assert!(admit(ComputerMethod::Click, &click("Calculator")).is_ok());
    }
    assert_eq!(asked.get(), 1);
    assert!(admit(ComputerMethod::Screenshot, &json!({})).is_ok());
}

/// "Take control": the request in flight is refused at its next step, every
/// act after it too, and no card is raised; "hand back" lets the shared
/// window be worked again.
#[test]
fn taking_control_stops_the_request_in_flight_and_every_act_until_handed_back() {
    let (_person, asked) = person(Decision::Allowed);
    let in_flight = admit(ComputerMethod::Click, &click("Safari")).expect("admitted");
    assert!(ensure_current().is_ok());
    take_control();
    assert_eq!(
        ensure_current().err().map(|error| error.code).as_deref(),
        Some(error_code::PERSON_TOOK_CONTROL),
        "the request in flight is refused at its next step"
    );
    drop(in_flight);
    for app in ["Safari", "Notes"] {
        assert_eq!(
            refused(ComputerMethod::Click, &click(app)).as_deref(),
            Some(error_code::PERSON_TOOK_CONTROL),
            "{app}"
        );
    }
    assert_eq!(
        asked.get(),
        1,
        "no card is raised after the person took control"
    );
    assert_eq!(state()["takenBack"], true);
    give_back();
    assert!(admit(ComputerMethod::Click, &click("Safari")).is_ok());
    assert_eq!(state()["takenBack"], false);
}

/// A stop, a status, the person's turn — the window's own verbs — are never
/// held to a share.
#[test]
fn the_windows_own_verbs_are_never_held_to_a_share() {
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Safari")).expect("admitted");
    take_control();
    for method in [
        ComputerMethod::Stop,
        ComputerMethod::Status,
        ComputerMethod::Resume,
    ] {
        assert!(admit(method, &json!({})).is_ok(), "{method:?}");
    }
}

#[test]
fn required_sharing_refuses_an_unanswered_card_and_every_unnamed_act() {
    super::with_held(|held| held.share.set_required(true));
    let (_person, asked) = person(Decision::TimedOut);
    for _ in 0..2 {
        assert_eq!(
            refused(ComputerMethod::Click, &click("Notes")).as_deref(),
            Some("share_required")
        );
    }
    assert_eq!(asked.get(), 1);
    assert_eq!(
        refused(ComputerMethod::PressKey, &json!({"key": "Escape"})).as_deref(),
        Some("share_required")
    );
    assert!(admit(ComputerMethod::Screenshot, &json!({})).is_ok());
}

#[test]
fn required_sharing_accepts_a_yes_and_keeps_window_verbs_available() {
    super::with_held(|held| held.share.set_required(true));
    let (_person, asked) = person(Decision::Allowed);
    assert!(admit(ComputerMethod::Click, &click("Notes")).is_ok());
    assert_eq!(asked.get(), 1);
    for method in [
        ComputerMethod::Stop,
        ComputerMethod::Resume,
        ComputerMethod::Status,
        ComputerMethod::Handoff,
    ] {
        assert!(admit(method, &json!({})).is_ok());
    }
}

#[test]
fn required_sharing_blocks_every_unnamed_act_without_asking() {
    super::with_held(|held| held.share.set_required(true));
    let (_person, asked) = person(Decision::Allowed);
    for method in ComputerMethod::ALL
        .iter()
        .copied()
        .filter(|method| method.acts())
    {
        assert_eq!(
            refused(method, &json!({})).as_deref(),
            Some("share_required"),
            "{method:?}"
        );
    }
    assert_eq!(asked.get(), 0);
}

#[test]
fn enabling_required_sharing_stops_an_unshared_act_at_its_next_step() {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let (_person, _) = person(Decision::TimedOut);
    let required = Arc::new(AtomicBool::new(false));
    let read = Arc::clone(&required);
    super::install_required(Box::new(move || read.load(Ordering::SeqCst)));
    let _in_flight =
        admit(ComputerMethod::Click, &click("Notes")).expect("default permits silence");
    required.store(true, Ordering::SeqCst);
    assert_eq!(ensure_current().unwrap_err().code, "share_required");
    required.store(false, Ordering::SeqCst);
    assert!(ensure_current().is_ok());
}

#[test]
fn a_panel_press_takes_the_same_share_and_invalidates_the_inflight_request() {
    let (_person, _) = person(Decision::Allowed);
    let _request = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut panel = state();
    panel["takenBack"] = true.into();
    super::hear_panel(&json!({"shareControl": panel}));
    assert_eq!(state()["takenBack"], true);
    assert_eq!(
        ensure_current().unwrap_err().code,
        error_code::PERSON_TOOK_CONTROL
    );
}

#[test]
fn a_panel_press_survives_a_share_renotice_before_the_window_hears_it() {
    let (_person, asked) = person(Decision::Allowed);
    let request = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut pressed = state();
    pressed["takenBack"] = true.into();
    super::tell();
    super::hear_panel(&json!({"shareControl": pressed}));
    assert_eq!(state()["takenBack"], true);
    assert_eq!(
        ensure_current().unwrap_err().code,
        error_code::PERSON_TOOK_CONTROL
    );
    drop(request);
    for method in ComputerMethod::ALL
        .iter()
        .copied()
        .filter(|method| method.acts())
    {
        assert_eq!(
            refused(method, &click("Sample")).as_deref(),
            Some(error_code::PERSON_TOOK_CONTROL),
            "{method:?}"
        );
    }
    assert_eq!(asked.get(), 1);
    give_back();
    assert!(admit(ComputerMethod::Click, &click("Sample")).is_ok());
}

#[test]
fn a_late_panel_press_cannot_take_a_replaced_or_handed_back_share() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut panel = state();
    panel["takenBack"] = true.into();
    let _ = admit(ComputerMethod::Click, &click("Another")).unwrap();
    super::hear_panel(&json!({"shareControl": panel}));
    assert_eq!(state()["takenBack"], false);
    let mut current = state();
    current["takenBack"] = true.into();
    super::hear_panel(&json!({"shareControl": current.clone()}));
    give_back();
    super::hear_panel(&json!({"shareControl": current}));
    assert_eq!(state()["takenBack"], false);
}

#[test]
fn a_panel_press_survives_a_window_selector_renotice() {
    let (_person, _) = person(Decision::Allowed);
    let _request = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut pressed = state();
    pressed["takenBack"] = true.into();
    let _look = admit(
        ComputerMethod::GetAppState,
        &json!({"app":"Sample", "windowId":41}),
    )
    .unwrap();
    super::hear_panel(&json!({"shareControl": pressed}));
    assert_eq!(state()["takenBack"], true);
    assert_eq!(
        ensure_current().unwrap_err().code,
        error_code::PERSON_TOOK_CONTROL
    );
}

#[test]
fn a_previous_panel_press_cannot_take_the_same_target_after_it_was_replaced() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut pressed = state();
    pressed["takenBack"] = true.into();
    let _ = admit(ComputerMethod::Click, &click("Another")).unwrap();
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    super::hear_panel(&json!({"shareControl": pressed}));
    assert_eq!(state()["takenBack"], false);
}

#[test]
fn a_future_or_missing_panel_revision_cannot_take_control() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut pressed = state();
    pressed["takenBack"] = true.into();
    pressed["revision"] = (pressed["revision"].as_u64().unwrap() + 1).into();
    super::hear_panel(&json!({"shareControl": pressed.clone()}));
    assert_eq!(state()["takenBack"], false);
    pressed.as_object_mut().unwrap().remove("revision");
    super::hear_panel(&json!({"shareControl": pressed}));
    assert_eq!(state()["takenBack"], false);
}

#[test]
fn the_panel_carries_the_requests_explicit_window_selector() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(
        ComputerMethod::Click,
        &json!({"app":"Sample", "windowId":41, "elementIndex":2}),
    )
    .unwrap();
    assert_eq!(state()["windowId"], 41);
    let _ = admit(
        ComputerMethod::GetAppState,
        &json!({"app":"Sample", "windowIndex":1}),
    )
    .unwrap();
    assert_eq!(state()["windowIndex"], 1);
    assert!(state()["windowId"].is_null());
}

#[test]
fn the_panel_reuses_the_renderers_words_without_changing_the_share_stamp() {
    let before = state()["revision"].clone();
    super::panel_label(Some("Take control".into()));
    assert_eq!(state()["buttonLabel"], "Take control");
    assert_eq!(state()["revision"], before);
    super::panel_label(Some("An arbitrary instruction".into()));
    assert_eq!(state()["buttonLabel"], "Take control");
    super::panel_label(Some(String::new()));
    assert_eq!(state()["buttonLabel"], "Take control");
}
