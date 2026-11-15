use std::cell::Cell;
use std::rc::Rc;

use serde_json::{Value, json};
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::error_code;

use super::stand_in;
use super::{
    Decision, SHARE_ASK_AGAIN_MS, SHARE_USE_LAPSES_MS, admit, ensure_current, give_back, hand_over,
    lapse, state, take_control,
};

fn person(answer: Decision) -> (stand_in::Standing, Rc<Cell<u32>>) {
    let asked = Rc::new(Cell::new(0));
    let counting = Rc::clone(&asked);
    let standing = stand_in::stand(move |_| {
        counting.set(counting.get() + 1);
        answer
    });
    (standing, asked)
}

/// The person who requires sharing, where cards still ask before every new app.
fn person_requiring(answer: Decision) -> (stand_in::Standing, Rc<Cell<u32>>) {
    let stood = person(answer);
    super::install_required(Box::new(|| true));
    stood
}

fn click(app: &str) -> Value {
    json!({ "app": app, "elementIndex": 2 })
}

#[test]
fn ending_a_share_forgets_the_answer_and_the_next_act_asks_again() {
    let (_person, asked) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    assert_eq!(asked.get(), 0, "no card stands before an app is used");
    let before = state();
    let ended = super::end("sample", before["generation"].as_u64().unwrap());
    assert!(ended["target"].is_null(), "{ended}");
    assert!(ended["label"].is_null(), "{ended}");
    assert!(ended["revision"].as_u64() > before["revision"].as_u64());
    assert_eq!(stand_in::notices().last(), Some(&ended));
    admit(ComputerMethod::Click, &click("Sample")).expect("asks");
    assert_eq!(
        asked.get(),
        1,
        "what the person ended asks before it is used again"
    );
}

#[test]
fn ending_a_share_invalidates_an_admitted_request_even_after_sharing_again() {
    let (_person, _) = person(Decision::Allowed);
    let _request = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let before = state();
    super::end("sample", before["generation"].as_u64().unwrap());
    {
        let _new_request = admit(ComputerMethod::Click, &click("Sample")).unwrap();
        assert!(ensure_current().is_ok());
    }
    assert_eq!(
        ensure_current().unwrap_err().code,
        error_code::PERSON_TOOK_CONTROL
    );
}

#[test]
fn ending_a_taken_back_share_clears_its_controls_and_keeps_old_requests_refused() {
    let (_person, asked) = person(Decision::Allowed);
    let _request = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    take_control();
    let before = state();
    let ended = super::end("sample", before["generation"].as_u64().unwrap());
    assert!(ended["target"].is_null(), "{ended}");
    assert_eq!(ended["takenBack"], false);
    give_back();
    assert_eq!(
        ensure_current().unwrap_err().code,
        error_code::PERSON_TOOK_CONTROL
    );
    admit(ComputerMethod::Click, &click("Sample")).expect("a new card");
    assert_eq!(asked.get(), 1);
}

#[test]
fn ending_a_stale_app_or_generation_preserves_the_current_share() {
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let old = state();
    take_control();
    give_back();
    let current = state();
    assert_eq!(
        super::end("sample", old["generation"].as_u64().unwrap()),
        current
    );
    assert_eq!(
        super::end("other", current["generation"].as_u64().unwrap()),
        current
    );
    assert_eq!(state(), current);
}

#[test]
fn ending_a_share_while_another_card_waits_does_not_restore_a_share_from_its_answer() {
    let _person = stand_in::stand(|card| {
        if card.app == "Other" {
            let current = state();
            super::end("sample", current["generation"].as_u64().unwrap());
        }
        Decision::Allowed
    });
    super::install_required(Box::new(|| true));
    admit(ComputerMethod::Click, &click("Sample")).unwrap();
    assert_eq!(
        refused(ComputerMethod::Click, &click("Other")).as_deref(),
        Some(error_code::PERSON_TOOK_CONTROL)
    );
    assert!(
        state()["target"].is_null(),
        "a late card cannot undo the end"
    );
}

#[test]
fn a_panel_press_from_an_ended_share_cannot_take_a_new_share() {
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let old = state();
    super::end("sample", old["generation"].as_u64().unwrap());
    admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let current = state();
    let mut panel = old;
    panel["takenBack"] = true.into();
    super::hear_panel(&json!({"shareControl": panel}));
    assert_eq!(state(), current);
}

fn refused(method: ComputerMethod, params: &Value) -> Option<String> {
    admit(method, params).err().map(|error| error.code)
}

/// The person's words: only the target is shared. The first act in an app
/// asks once; a yes holds the agent to that window and the same app is not
/// asked again.
#[test]
fn required_the_first_act_asks_once_and_a_yes_holds_the_agent_to_that_window() {
    let (_person, asked) = person_requiring(Decision::Allowed);
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
    let (_person, asked) = person_requiring(Decision::Refused);
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
fn the_agent_uses_any_app_at_once_and_no_card_holds_it_up() {
    let (_person, asked) = person(Decision::TimedOut);
    for app in ["Calculator", "Notes", "KakaoTalk"] {
        assert!(admit(ComputerMethod::Click, &click(app)).is_ok());
    }
    assert_eq!(asked.get(), 0);
    assert!(admit(ComputerMethod::Screenshot, &json!({})).is_ok());
    assert_eq!(
        state()["label"],
        "Calculator, KakaoTalk, Notes",
        "the band names every app in use"
    );
}

/// "Take control": the request in flight is refused at its next step, every
/// act after it too, and no card is raised; "hand back" lets the shared
/// window be worked again.
#[test]
fn taking_control_stops_the_request_in_flight_and_every_act_until_handed_back() {
    let (_person, asked) = person_requiring(Decision::Allowed);
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
    let (_person, asked) = person_requiring(Decision::Allowed);
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
fn a_panel_press_survives_a_switch_to_an_already_shared_target() {
    let (_person, asked) = person_requiring(Decision::Allowed);
    let _ = admit(ComputerMethod::Click, &click("Another")).unwrap();
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut panel = state();
    panel["takenBack"] = true.into();
    let request = admit(ComputerMethod::Click, &click("Another")).unwrap();
    assert_eq!(asked.get(), 2, "returning to an approved app needs no card");
    assert_eq!(state()["generation"], panel["generation"]);
    assert_eq!(state()["target"], "another");
    super::hear_panel(&json!({"shareControl": panel}));
    assert_eq!(state()["takenBack"], true);
    assert_eq!(
        ensure_current().unwrap_err().code,
        error_code::PERSON_TOOK_CONTROL
    );
    drop(request);
    for app in ["Sample", "Another"] {
        for method in ComputerMethod::ALL
            .iter()
            .copied()
            .filter(|method| method.acts())
        {
            assert_eq!(
                refused(method, &click(app)).as_deref(),
                Some(error_code::PERSON_TOOK_CONTROL),
                "{app}: {method:?}"
            );
        }
    }
    give_back();
    for app in ["Sample", "Another"] {
        assert!(admit(ComputerMethod::Click, &click(app)).is_ok());
    }
    assert_eq!(asked.get(), 2);
}

#[test]
fn a_late_panel_press_cannot_take_a_handed_back_share_even_after_target_changes() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut current = state();
    current["takenBack"] = true.into();
    super::hear_panel(&json!({"shareControl": current.clone()}));
    give_back();
    let _ = admit(ComputerMethod::Click, &click("Another")).unwrap();
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
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
fn a_pending_panel_press_survives_target_changes_away_and_back() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut pressed = state();
    pressed["takenBack"] = true.into();
    let _ = admit(ComputerMethod::Click, &click("Another")).unwrap();
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    super::hear_panel(&json!({"shareControl": pressed}));
    assert_eq!(state()["takenBack"], true);
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
fn a_panel_press_without_a_displayed_target_cannot_take_control() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut pressed = state();
    pressed["takenBack"] = true.into();
    pressed["target"] = Value::Null;
    super::hear_panel(&json!({"shareControl": pressed.clone()}));
    assert_eq!(state()["takenBack"], false);
    pressed.as_object_mut().unwrap().remove("target");
    super::hear_panel(&json!({"shareControl": pressed}));
    assert_eq!(state()["takenBack"], false);
}

#[test]
fn a_panel_revision_from_before_the_first_share_cannot_take_control() {
    let (_person, _) = person(Decision::Allowed);
    let before = state()["revision"].clone();
    let _ = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    let mut pressed = state();
    pressed["takenBack"] = true.into();
    pressed["revision"] = before;
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
fn every_app_in_use_gets_its_own_control_on_its_own_window() {
    let (_person, _) = person(Decision::Allowed);
    let _ = admit(
        ComputerMethod::Scroll,
        &json!({"app":"Sample", "windowId":41, "elementIndex":2}),
    )
    .unwrap();
    let one = state()["revision"].clone();
    let _ = admit(
        ComputerMethod::Scroll,
        &json!({"app":"Another", "windowId":52, "elementIndex":3}),
    )
    .unwrap();
    let both = state();
    assert!(
        both["revision"].as_u64() > one.as_u64(),
        "a second app is a new presentation"
    );
    let panels = both["panels"].as_array().expect("one control per app");
    assert_eq!(panels.len(), 2, "{both}");
    for (app, window) in [("sample", 41), ("another", 52)] {
        let panel = panels
            .iter()
            .find(|panel| panel["target"] == app)
            .unwrap_or_else(|| panic!("{app} has its control: {both}"));
        assert_eq!(panel["windowId"], window, "{both}");
    }
    // A press on either control takes control of both.
    let mut pressed = both.clone();
    pressed["takenBack"] = true.into();
    super::hear_panel(&json!({"shareControl": pressed}));
    assert_eq!(state()["takenBack"], true);
    for app in ["Sample", "Another"] {
        assert_eq!(
            refused(ComputerMethod::Scroll, &click(app)).as_deref(),
            Some(error_code::PERSON_TOOK_CONTROL),
            "{app}"
        );
    }
    // Ending the share ends every app's use, and with it every control. The
    // press began a new generation: the band ends the one it shows now.
    let ended = super::end("sample", state()["generation"].as_u64().unwrap());
    assert_eq!(ended["panels"], json!([]), "{ended}");
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

#[test]
fn the_panel_accepts_the_current_control_word_in_every_catalog() {
    let catalog = include_str!("../../../../../ui/shell-i18n.js");
    let renderer = include_str!("../../../../../ui/shell-computer.js");
    let key = "\"computer.band.take\"";
    let mut labels = Vec::new();
    for line in catalog
        .lines()
        .filter(|line| line.trim_start().starts_with(key))
    {
        let (_, value) = line.split_once(':').unwrap();
        labels.push(serde_json::from_str::<String>(value.trim().trim_end_matches(',')).unwrap());
    }
    let (_, fallback) = renderer.split_once("t(\"computer.band.take\", ").unwrap();
    labels.push(serde_json::from_str::<String>(fallback.split_once(')').unwrap().0).unwrap());
    assert_eq!(labels.len(), 5);
    for label in labels {
        super::panel_label(Some(label.clone()));
        assert_eq!(state()["buttonLabel"], label);
    }
}

#[cfg(target_os = "macos")]
#[test]
fn panel_notices_never_open_a_socket_or_spawn_a_helper_on_the_callers_thread() {
    let source = include_str!("../macos.rs");
    let sync = source
        .split_once("pub(super) fn sync_share_panel(")
        .unwrap()
        .1;
    let sync = sync.split_once("\n}\n").unwrap().0;
    assert!(
        !sync.contains("UnixStream::connect"),
        "the caller must only queue the latest state"
    );
    assert!(
        !sync.contains("wire.request("),
        "a helper reply must not delay admission"
    );
    let source = include_str!("../share.rs");
    let tell = source.split_once("fn tell() {").unwrap().1;
    let tell = tell.split_once("\n}\n").unwrap().0;
    assert!(
        !tell.contains(".spawn("),
        "one worker owns helper startup and delivery"
    );
}

/// R1 (t-74826): with sharing required, an unanswered card is no lasting no.
/// The agent's acts in that app are refused at once, with no new card, for
/// the quiet; the first act after it raises the card again.
#[test]
fn required_silence_asks_again_after_the_quiet_and_never_in_it() {
    super::with_held(|held| held.share.set_required(true));
    let (_person, asked) = person(Decision::TimedOut);
    for _ in 0..3 {
        assert_eq!(
            refused(ComputerMethod::Click, &click("Notes")).as_deref(),
            Some(error_code::SHARE_REQUIRED)
        );
    }
    assert_eq!(asked.get(), 1, "no card within the quiet");
    stand_in::advance(SHARE_ASK_AGAIN_MS - 1);
    assert_eq!(
        refused(ComputerMethod::Click, &click("Notes")).as_deref(),
        Some(error_code::SHARE_REQUIRED)
    );
    assert_eq!(asked.get(), 1);
    stand_in::advance(1);
    assert_eq!(
        refused(ComputerMethod::Click, &click("Notes")).as_deref(),
        Some(error_code::SHARE_REQUIRED)
    );
    assert_eq!(asked.get(), 2, "the first act after the quiet asks again");
}

/// A yes on the card raised again shares the app as the first one would.
#[test]
fn a_yes_on_the_card_raised_again_shares_the_waiting_app() {
    super::with_held(|held| held.share.set_required(true));
    let answers = Rc::new(Cell::new(Decision::TimedOut));
    let answering = Rc::clone(&answers);
    let _person = stand_in::stand(move |_| answering.get());
    assert!(admit(ComputerMethod::Click, &click("Notes")).is_err());
    answers.set(Decision::Allowed);
    stand_in::advance(SHARE_ASK_AGAIN_MS);
    assert!(admit(ComputerMethod::Click, &click("Notes")).is_ok());
    assert_eq!(state()["target"], "notes");
    assert!(state()["waiting"].is_null());
}

/// The card says whether silence lets the run go on unshared or keeps the
/// agent waiting, so its clock can say which.
#[test]
fn the_card_says_whether_sharing_is_required() {
    let cards = Rc::new(std::cell::RefCell::new(Vec::new()));
    let seen = Rc::clone(&cards);
    let _person = stand_in::stand(move |card| {
        seen.borrow_mut()
            .push(serde_json::to_value(card).expect("a card"));
        Decision::TimedOut
    });
    let _ = admit(ComputerMethod::Click, &click("Calculator"));
    let current = state();
    super::end("calculator", current["generation"].as_u64().unwrap());
    let _ = admit(ComputerMethod::Click, &click("Calculator"));
    super::install_required(Box::new(|| true));
    let _ = admit(ComputerMethod::Click, &click("Notes"));
    let cards = cards.borrow();
    assert_eq!(cards.len(), 2, "{cards:?}");
    assert_eq!(cards[0]["required"], false);
    assert_eq!(cards[1]["required"], true);
}

/// The person can hand the waiting app over later from the band: no card,
/// that app only, and the band hears what waits and what is shared.
#[test]
fn the_person_hands_the_waiting_app_over_later_from_the_band() {
    super::with_held(|held| held.share.set_required(true));
    let (_person, asked) = person(Decision::TimedOut);
    assert!(admit(ComputerMethod::Click, &click("Notes")).is_err());
    assert_eq!(state()["waiting"], "notes");
    assert_eq!(state()["waitingLabel"], "Notes");
    assert!(
        stand_in::notices()
            .iter()
            .any(|notice| notice["waiting"] == "notes"),
        "the band hears the wait: {:?}",
        stand_in::notices()
    );
    assert!(!hand_over("Safari"), "only the app that waits");
    assert!(state()["target"].is_null());
    assert!(hand_over("notes"));
    assert_eq!(state()["target"], "notes");
    assert_eq!(state()["label"], "Notes");
    assert!(state()["waiting"].is_null());
    assert!(admit(ComputerMethod::Click, &click("Notes")).is_ok());
    assert_eq!(asked.get(), 1, "handing over raises no card");
    assert!(
        stand_in::notices()
            .last()
            .is_some_and(|notice| notice["target"] == "notes" && notice["waiting"].is_null()),
        "{:?}",
        stand_in::notices()
    );
}

/// Without required sharing nothing waits and the band offers nothing new.
#[test]
fn without_required_sharing_nothing_waits() {
    let (_person, asked) = person(Decision::TimedOut);
    assert!(admit(ComputerMethod::Click, &click("Calculator")).is_ok());
    stand_in::advance(SHARE_ASK_AGAIN_MS * 10);
    assert!(admit(ComputerMethod::Click, &click("Calculator")).is_ok());
    assert_eq!(asked.get(), 0);
    assert!(state()["waiting"].is_null());
    assert!(!hand_over("calculator"));
}

/// R1 (t-74826): a request in flight in one app, another app handed over
/// since — the first request's next step leaves the share where the person
/// put it and tells the band nothing.
#[test]
fn a_step_in_flight_never_moves_the_share_back() {
    let (_person, _) = person(Decision::Allowed);
    let in_flight = admit(ComputerMethod::Click, &click("Safari")).expect("admitted");
    // A moment later, when the panel is free to follow.
    stand_in::advance(SHARE_ASK_AGAIN_MS);
    drop(admit(ComputerMethod::Click, &click("Notes")).expect("admitted"));
    assert_eq!(state()["target"], "notes");
    let told = stand_in::notices().len();
    assert!(ensure_current().is_ok());
    assert_eq!(state()["target"], "notes", "the step moved the share back");
    assert_eq!(stand_in::notices().len(), told, "the band heard no move");
    drop(in_flight);
}

/// R3 (t-74826 again): a request in flight outside the share the person
/// makes since — an unanswered app, the whole screen — is refused at its
/// next step, as a new request would be, and the share stays where the
/// person put it.
#[test]
fn handing_over_another_app_refuses_the_next_step_of_a_request_outside_it() {
    let answers = Rc::new(Cell::new(Decision::TimedOut));
    let answering = Rc::clone(&answers);
    let _person = stand_in::stand(move |_| answering.get());
    super::install_required(Box::new(|| true));
    let screen = admit(ComputerMethod::Screenshot, &json!({})).expect("nothing shared yet");
    answers.set(Decision::Allowed);
    drop(admit(ComputerMethod::Click, &click("Notes")).expect("admitted"));
    assert_eq!(state()["target"], "notes");
    let told = stand_in::notices().len();
    assert_eq!(
        ensure_current().err().map(|error| error.code).as_deref(),
        Some(error_code::SHARE_SCOPE),
        "the whole-screen look in flight"
    );
    drop(screen);
    assert_eq!(state()["target"], "notes");
    assert_eq!(stand_in::notices().len(), told, "the band heard no move");
}

/// A helper that names apps as the real one does: every word an app answers
/// to gives the same row. `asked` counts the round trips.
fn naming_helper(asked: &Rc<Cell<u32>>) -> crate::computer_use::test_helper::Standing {
    let asked = Rc::clone(asked);
    crate::computer_use::test_helper::stand(move |method, params| {
        assert_eq!(method, "listApps");
        asked.set(asked.get() + 1);
        let mirroring = json!({"name": "iPhone 미러링", "bundleId": "com.apple.ScreenContinuity", "pid": 4242, "isRunning": true});
        let other = json!({"name": "Sample", "bundleId": "com.example.Sample", "pid": 4343, "isRunning": true});
        let named = params["named"].as_str().unwrap_or_default().to_lowercase();
        let rows = match named.as_str() {
            "iphone 미러링" | "iphone mirroring" | "com.apple.screencontinuity" | "pid:4242" => {
                vec![mirroring]
            }
            "sample" | "com.example.sample" => vec![other],
            _ => vec![],
        };
        Ok(json!({"apps": rows, "named": true}))
    })
}

/// The security edge: what the person ended stays ended under every word the
/// app answers to, not only the one it was first named by.
#[test]
fn an_app_the_person_ended_is_not_entered_again_under_another_name() {
    for other_name in [
        "iPhone Mirroring",
        "com.apple.ScreenContinuity",
        "com.apple.screencontinuity",
        "pid:4242",
    ] {
        let round_trips = Rc::new(Cell::new(0));
        let _helper = naming_helper(&round_trips);
        let (_person, asked) = person(Decision::TimedOut);
        admit(ComputerMethod::Click, &click("iPhone 미러링")).expect("goes at once");
        let before = state();
        super::end(
            before["target"].as_str().expect("in use"),
            before["generation"].as_u64().unwrap(),
        );
        let refused = admit(ComputerMethod::Click, &click(other_name));
        assert_eq!(asked.get(), 1, "{other_name}: asks before going back in");
        assert_eq!(
            refused.err().map(|error| error.code).as_deref(),
            Some(error_code::SHARE_SCOPE),
            "{other_name}: silence keeps what the person ended refused"
        );
        assert!(state()["target"].is_null(), "{other_name}");
        // Each pass starts from a share of its own.
        super::HELD.with(|held| *held.borrow_mut() = super::Held::default());
    }
}

#[test]
fn an_app_handed_over_under_one_name_goes_under_all_of_them() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, asked) = person_requiring(Decision::Allowed);
    drop(admit(ComputerMethod::Click, &click("iPhone 미러링")).expect("handed over"));
    assert_eq!(asked.get(), 1);
    let look = json!({"app": "iPhone Mirroring"});
    drop(admit(ComputerMethod::Observe, &look).expect("a look under the other name"));
    drop(
        admit(ComputerMethod::Click, &click("com.apple.ScreenContinuity"))
            .expect("an act under its identifier"),
    );
    assert_eq!(asked.get(), 1, "one app, one card");
    let held = state();
    assert_eq!(held["target"], "com.apple.screencontinuity");
    assert_eq!(
        held["label"], "iPhone 미러링",
        "the band says the person's word"
    );
}

#[test]
fn two_apps_stay_two_apps() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, asked) = person(Decision::TimedOut);
    admit(ComputerMethod::Click, &click("iPhone Mirroring")).expect("goes");
    let before = state();
    super::end(
        before["target"].as_str().expect("in use"),
        before["generation"].as_u64().unwrap(),
    );
    admit(ComputerMethod::Click, &click("Sample")).expect("another app is not the ended one");
    assert_eq!(asked.get(), 0);
    assert_eq!(state()["target"], "com.example.sample");
}

/// A launch names an app that is not running: the helper reads its bundle, so
/// the person's no still holds while it is closed.
#[test]
fn an_ended_app_that_was_quit_keeps_its_key() {
    let (_person, asked) = person(Decision::TimedOut);
    let round_trips = Rc::new(Cell::new(0));
    let counting = Rc::clone(&round_trips);
    let _helper = crate::computer_use::test_helper::stand(move |_, _| {
        counting.set(counting.get() + 1);
        let running = counting.get() == 1;
        Ok(json!({"apps": [{
            "name": if running { "iPhone 미러링" } else { "iPhone Mirroring" },
            "bundleId": "com.apple.ScreenContinuity",
            "pid": if running { json!(4242) } else { Value::Null },
            "isRunning": running,
        }], "named": true}))
    });
    admit(ComputerMethod::Click, &click("iPhone 미러링")).expect("goes");
    let before = state();
    super::end(
        before["target"].as_str().expect("in use"),
        before["generation"].as_u64().unwrap(),
    );
    assert!(admit(ComputerMethod::Launch, &json!({"app": "iPhone Mirroring"})).is_err());
    assert_eq!(asked.get(), 1, "a launch by the other name asks too");
}

#[test]
fn a_look_costs_no_round_trip_unless_the_person_holds_it_to_the_share() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _asked) = person(Decision::Allowed);
    let look = json!({"app": "iPhone Mirroring"});
    drop(admit(ComputerMethod::Observe, &look).expect("a look"));
    assert_eq!(round_trips.get(), 0, "a free look asks the helper nothing");
    drop(admit(ComputerMethod::Click, &click("iPhone Mirroring")).expect("an act"));
    assert_eq!(round_trips.get(), 1, "an act asks once");
    super::install_required(Box::new(|| true));
    drop(admit(ComputerMethod::Click, &click("iPhone Mirroring")).expect("handed over"));
    round_trips.set(0);
    drop(admit(ComputerMethod::Observe, &look).expect("a held look"));
    assert_eq!(round_trips.get(), 1, "a look under a held share asks once");
}

/// A helper that answers until `up` is cleared, then fails as one restarting.
fn helper_while(up: &Rc<Cell<bool>>) -> crate::computer_use::test_helper::Standing {
    let up = Rc::clone(up);
    crate::computer_use::test_helper::stand(move |_, params| {
        if !up.get() {
            return Err(crate::computer_use::ComputerUseError::new(
                "unavailable",
                "the helper is gone",
            ));
        }
        let named = params["named"].as_str().unwrap_or_default().to_lowercase();
        let rows = match named.as_str() {
            "iphone 미러링" | "iphone mirroring" => vec![
                json!({"name": "iPhone 미러링", "bundleId": "com.apple.ScreenContinuity", "pid": 4242, "isRunning": true}),
            ],
            _ => vec![],
        };
        Ok(json!({"apps": rows, "named": true}))
    })
}

/// With the helper unable to say what a name is, an act cannot be told apart
/// from what the person said no to, or will say no to: an app entered under a
/// word the helper did not read gets a key of that word, and once the person
/// ends it there, the same app under another word is a new app. So an act on
/// an unread name does not go, whether a no stands yet or not.
#[test]
fn an_unread_name_does_not_go_even_before_any_no() {
    let (_person, asked) = person(Decision::TimedOut);
    let up = Rc::new(Cell::new(false));
    let _helper = helper_while(&up);
    let refused = admit(ComputerMethod::Click, &click("Sample"));
    assert_eq!(
        refused.err().map(|error| error.code).as_deref(),
        Some(error_code::SHARE_SCOPE),
        "nothing ended yet, and still no act under a word nobody read"
    );
    assert!(state()["target"].is_null(), "nothing was shared");
    assert_eq!(asked.get(), 0, "no card for a name nobody can read");
}

/// The order the review t-90980 walked: with the helper restarting, an act
/// under the English name; the person ends it; with the helper back, the
/// local name. The first act does not go, so the end is the person's end of
/// the app under its one key, and the other name is refused.
#[test]
fn an_app_entered_while_the_helper_restarts_is_not_entered_again_under_its_other_name() {
    let (_person, asked) = person(Decision::TimedOut);
    let up = Rc::new(Cell::new(false));
    let _helper = helper_while(&up);
    // 1. Nothing ended. 2. The helper is restarting; an act by the English name.
    let first = admit(ComputerMethod::Click, &click("iPhone Mirroring"));
    assert_eq!(
        first.err().map(|error| error.code).as_deref(),
        Some(error_code::SHARE_SCOPE),
        "an unread word is not entered under a key of its own"
    );
    assert!(state()["target"].is_null());
    // The helper answers again; the same act now goes under the app's key.
    up.set(true);
    drop(admit(ComputerMethod::Click, &click("iPhone Mirroring")).expect("goes once read"));
    let before = state();
    assert_eq!(before["target"], "com.apple.screencontinuity");
    // 3. The person ends it from the band.
    super::end(
        before["target"].as_str().expect("in use"),
        before["generation"].as_u64().unwrap(),
    );
    // 4. The local name is the ended app: it asks, and silence refuses.
    let again = admit(ComputerMethod::Click, &click("iPhone 미러링"));
    assert_eq!(
        again.err().map(|error| error.code).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(asked.get(), 1, "the ended app asks before going back in");
    // And with the helper gone again, neither name goes.
    up.set(false);
    for name in ["iPhone Mirroring", "iPhone 미러링"] {
        let refused = admit(ComputerMethod::Click, &click(name));
        assert_eq!(
            refused.err().map(|error| error.code).as_deref(),
            Some(error_code::SHARE_SCOPE),
            "{name}"
        );
    }
    assert_eq!(asked.get(), 1, "no card for a name nobody can read");
}

/// The person, 2026-11-15: "표시가 끝나면 자동 종료되게.. 내가 누르지 않아도
/// 끊기거나 종료되면". A request lets go of the app whose minute is up before
/// it is judged, so the band names what is in use now.
#[test]
fn a_request_lets_go_of_the_app_nothing_reached_for_a_minute() {
    let (_person, asked) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    stand_in::advance(SHARE_USE_LAPSES_MS);
    admit(ComputerMethod::Click, &click("Other")).expect("goes at once");
    let now = state();
    assert_eq!(now["label"], "Other", "{now}");
    assert_eq!(now["panels"].as_array().map(Vec::len), Some(1), "{now}");
    assert_eq!(asked.get(), 0, "letting go asks nobody");
}

/// With no request at all the window's own look lets go: the band closes with
/// no press, and is told so.
#[test]
fn the_windows_look_lets_go_of_a_quiet_app_and_tells_the_band() {
    let (_person, asked) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    assert!(lapse(), "still in use: the look goes on");
    assert_eq!(state()["target"], "sample");
    stand_in::advance(1);
    assert!(!lapse(), "nothing is left to look for");
    let now = state();
    assert!(now["target"].is_null(), "{now}");
    assert!(now["label"].is_null(), "{now}");
    assert_eq!(now["panels"], json!([]), "{now}");
    assert_eq!(stand_in::notices().last(), Some(&now), "the band was told");
    admit(ComputerMethod::Click, &click("Sample")).expect("joins again at once");
    assert_eq!(state()["target"], "sample");
    assert_eq!(asked.get(), 0, "a lapse is no answer of the person's");
}

#[test]
fn an_app_whose_process_ended_leaves_at_once() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    admit(ComputerMethod::Click, &click("iPhone 미러링")).expect("goes at once");
    assert!(lapse());
    assert_eq!(state()["label"], "iPhone 미러링, Sample", "both run");
    stand_in::ended(&[4343]);
    assert!(lapse(), "one is still in use");
    let now = state();
    assert_eq!(now["label"], "iPhone 미러링", "{now}");
    assert_eq!(now["target"], "com.apple.screencontinuity", "{now}");
    stand_in::ended(&[4343, 4242]);
    assert!(!lapse());
    assert!(state()["target"].is_null());
}

#[test]
fn a_request_under_way_keeps_its_app_in_use() {
    let (_person, _) = person(Decision::Allowed);
    let _request = admit(ComputerMethod::Click, &click("Sample")).unwrap();
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    ensure_current().expect("its next step");
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    assert!(lapse());
    assert_eq!(
        state()["target"],
        "sample",
        "a step reached it inside the minute"
    );
    stand_in::advance(1);
    assert!(!lapse());
    assert!(state()["target"].is_null());
    assert!(
        ensure_current().is_ok(),
        "the request itself is not refused"
    );
}

/// What the person took is theirs to hand back: the band stays under their
/// hand whatever ended meanwhile.
#[test]
fn nothing_leaves_while_the_person_holds_control() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).unwrap();
    take_control();
    stand_in::advance(10 * SHARE_USE_LAPSES_MS);
    stand_in::ended(&[4343]);
    assert!(!lapse(), "nothing to look for under the person's hand");
    let taken = state();
    assert_eq!(taken["target"], "com.example.sample", "{taken}");
    assert_eq!(taken["takenBack"], true);
    give_back();
    assert!(!lapse());
    assert!(
        state()["target"].is_null(),
        "handed back, the ended app goes"
    );
}

/// Where the person requires sharing, a quiet minute ends nothing: letting the
/// hand-over go would open every other window to an agent's look mid-task. It
/// goes with its app, and the person's yes stands for when the app is back.
#[test]
fn required_a_hand_over_stands_through_the_quiet_and_goes_with_its_app() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, asked) = person_requiring(Decision::Allowed);
    drop(admit(ComputerMethod::Click, &click("Sample")).expect("handed over"));
    assert_eq!(asked.get(), 1);
    stand_in::advance(10 * SHARE_USE_LAPSES_MS);
    assert!(lapse());
    assert_eq!(state()["target"], "com.example.sample");
    assert_eq!(
        refused(ComputerMethod::Observe, &json!({"app": "iPhone 미러링"})).as_deref(),
        Some(error_code::SHARE_SCOPE),
        "a look outside what was handed over is still refused"
    );
    stand_in::ended(&[4343]);
    assert!(!lapse());
    assert!(state()["target"].is_null());
    drop(admit(ComputerMethod::Click, &click("Sample")).expect("the yes stands"));
    assert_eq!(
        asked.get(),
        1,
        "no second card for an app the person handed over"
    );
}

/// With sharing required, the app that waits for the person stops waiting
/// once no request has asked for it through the quiet. When a request for
/// something else is what finds that, the band is told all the same: its
/// "waiting" line and the hand-over under it go with the wait (t-109811).
#[test]
fn required_a_wait_that_ends_during_another_request_is_told_to_the_band() {
    let (_person, _asked) = person_requiring(Decision::TimedOut);
    assert!(admit(ComputerMethod::Click, &click("Notes")).is_err());
    assert_eq!(state()["waiting"], "notes", "it waits for the person");
    stand_in::advance(SHARE_ASK_AGAIN_MS);
    let told = stand_in::notices().len();
    drop(admit(ComputerMethod::Observe, &json!({})).expect("a look while nothing is handed over"));
    let now = state();
    assert!(now["waiting"].is_null(), "{now}");
    assert!(
        stand_in::notices().len() > told,
        "the wait ended and the band heard nothing: {now}"
    );
    assert_eq!(
        stand_in::notices().last(),
        Some(&now),
        "the band was told what stands now"
    );
}

/// An act holds its app by the bundle identifier the helper names it by, and
/// a free look names it by the agent's word alone. The look is a use of the
/// app all the same — under the word the agent acted by, the app's name, its
/// identifier or its process — so an agent that only reads for more than a
/// minute keeps its app on the band, and the look still asks the helper
/// nothing (t-109811).
#[test]
fn a_look_by_any_word_of_an_app_in_use_keeps_it_in_use() {
    for word in [
        "iPhone Mirroring",
        "iPhone 미러링",
        "com.apple.ScreenContinuity",
        "pid:4242",
    ] {
        let round_trips = Rc::new(Cell::new(0));
        let _helper = naming_helper(&round_trips);
        let (_person, _) = person(Decision::Allowed);
        admit(ComputerMethod::Click, &click("iPhone Mirroring")).expect("goes at once");
        round_trips.set(0);
        stand_in::advance(SHARE_USE_LAPSES_MS - 1);
        drop(admit(ComputerMethod::Observe, &json!({ "app": word })).expect("a look"));
        assert_eq!(
            round_trips.get(),
            0,
            "{word}: a free look asks the helper nothing"
        );
        stand_in::advance(SHARE_USE_LAPSES_MS - 1);
        assert!(lapse(), "{word}: the look reached it inside the minute");
        assert_eq!(state()["target"], "com.apple.screencontinuity", "{word}");
        stand_in::advance(1);
        assert!(
            !lapse(),
            "{word}: a minute after the look nothing is in use"
        );
        assert!(state()["target"].is_null(), "{word}");
        // Each pass starts from a share of its own.
        super::HELD.with(|held| *held.borrow_mut() = super::Held::default());
    }
}

/// A look under way — a wait that lasts — is a use of its app at every step.
#[test]
fn a_look_under_way_keeps_its_app_in_use() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    let _look = admit(ComputerMethod::Observe, &json!({ "app": "Sample" })).expect("a look");
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    ensure_current().expect("its next step");
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    assert!(lapse(), "a step of the look reached it inside the minute");
    assert_eq!(state()["target"], "com.example.sample");
    stand_in::advance(1);
    assert!(!lapse());
    assert!(state()["target"].is_null());
}

/// A word names the app it was acted by only while that app is in use: once
/// the app left, a look by the word brings nothing back.
#[test]
fn a_look_brings_no_app_back_into_use() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    stand_in::advance(SHARE_USE_LAPSES_MS);
    drop(admit(ComputerMethod::Observe, &json!({ "app": "Sample" })).expect("a look"));
    let now = state();
    assert!(now["target"].is_null(), "{now}");
    assert_eq!(now["panels"], json!([]), "{now}");
}

/// A press the person is asked about waits for their answer up to two
/// minutes, and nothing reaches its app meanwhile. The minute does not run
/// while they think: allowed after it, the press would otherwise go with its
/// app — and the band's take-control — gone from the band. The minute begins
/// when the card closes (t-109811).
#[test]
fn nothing_goes_quiet_while_the_person_is_asked() {
    let (_person, _) = person(Decision::Allowed);
    let _request = admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    stand_in::asking(true);
    stand_in::advance(SHARE_USE_LAPSES_MS + SHARE_USE_LAPSES_MS / 2);
    assert!(lapse(), "the look goes on while the card stands");
    assert_eq!(state()["target"], "sample", "its app stays on the band");
    ensure_current().expect("allowed, the press goes with its app on the band");
    stand_in::asking(false);
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    assert!(lapse(), "the minute begins when the card closes");
    stand_in::advance(1);
    assert!(!lapse());
    assert!(state()["target"].is_null());
}

/// An app that quit leaves at once, whatever the person is being asked.
#[test]
fn an_app_that_quit_leaves_while_the_person_is_asked() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    stand_in::asking(true);
    stand_in::ended(&[4343]);
    assert!(!lapse(), "nothing is left to look for");
    assert!(state()["target"].is_null());
}

/// A look's answer names the app it read. A press by a mark names no app at
/// all, and the word of a look may be one no name of the app's spells
/// (`Notes` where the helper says `메모`): the first look answered ties the
/// word to the app in use, and that look and every later one by the word is a
/// use of it — with nothing asked of the helper (t-109874).
#[test]
fn a_looks_answer_ties_its_word_to_the_app_in_use() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _) = person(Decision::Allowed);
    // As a mark's press reaches its app: by its process, no word of the agent's.
    admit(ComputerMethod::Click, &click("pid:4242")).expect("goes at once");
    round_trips.set(0);
    let look = json!({ "app": "Mirroring" });
    let answer = json!({"snapshot": {"app": {
        "name": "iPhone 미러링", "bundleId": "com.apple.ScreenContinuity", "pid": 4242,
    }}});
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    {
        let _look = admit(ComputerMethod::Observe, &look).expect("a look");
        super::heard(&answer);
    }
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    assert!(lapse(), "the look's answer named the app in use");
    drop(admit(ComputerMethod::Observe, &look).expect("a look"));
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    assert!(
        lapse(),
        "from then on the word names it as the look comes in"
    );
    assert_eq!(state()["target"], "com.apple.screencontinuity");
    assert_eq!(round_trips.get(), 0, "no look asked the helper anything");
    stand_in::advance(1);
    assert!(!lapse());
    assert!(state()["target"].is_null());
}

/// An answer about an app that is not in use ties nothing and keeps nothing
/// in use.
#[test]
fn an_answer_about_an_app_that_is_not_in_use_ties_nothing() {
    let round_trips = Rc::new(Cell::new(0));
    let _helper = naming_helper(&round_trips);
    let (_person, _) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("goes at once");
    let other = json!({"snapshot": {"app": {
        "name": "iPhone 미러링", "bundleId": "com.apple.ScreenContinuity", "pid": 4242,
    }}});
    stand_in::advance(SHARE_USE_LAPSES_MS - 1);
    {
        let _look = admit(ComputerMethod::Observe, &json!({ "app": "Mirroring" })).expect("a look");
        super::heard(&other);
    }
    stand_in::advance(1);
    assert!(!lapse(), "a look at another app kept this one in use");
    assert!(state()["target"].is_null());
    assert_eq!(state()["panels"], json!([]), "and brought nothing into use");
}

/// The host's own answer, not a test's: a child that was waited for is gone,
/// and this process is not.
#[cfg(unix)]
#[test]
fn a_process_that_was_waited_for_has_ended_and_this_one_has_not() {
    let mut child = crate::proc::quiet_command("/usr/bin/true")
        .spawn()
        .expect("a child");
    let pid = i64::from(child.id());
    child.wait().expect("the child ended");
    assert!(super::host_says_ended(pid));
    assert!(!super::host_says_ended(i64::from(std::process::id())));
    assert!(!super::host_says_ended(0), "no process is not an ended one");
    assert!(!super::host_says_ended(-1));
    assert!(
        !super::host_says_ended(i64::MAX),
        "a number no process can have proves nothing"
    );
}
