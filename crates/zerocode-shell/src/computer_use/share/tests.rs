use std::cell::Cell;
use std::rc::Rc;

use serde_json::{Value, json};
use zerocode_core::computer_use::ComputerMethod;
use zerocode_core::computer_use_protocol::error_code;

use super::stand_in;
use super::{
    Decision, SHARE_ASK_AGAIN_MS, admit, ensure_current, give_back, hand_over, state, take_control,
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

fn click(app: &str) -> Value {
    json!({ "app": app, "elementIndex": 2 })
}

#[test]
fn ending_a_share_forgets_the_answer_and_the_next_act_asks_again() {
    let (_person, asked) = person(Decision::Allowed);
    admit(ComputerMethod::Click, &click("Sample")).expect("shared");
    let before = state();
    let ended = super::end("sample", before["generation"].as_u64().unwrap());
    assert!(ended["target"].is_null(), "{ended}");
    assert!(ended["label"].is_null(), "{ended}");
    assert!(ended["revision"].as_u64() > before["revision"].as_u64());
    assert_eq!(stand_in::notices().last(), Some(&ended));
    admit(ComputerMethod::Click, &click("Sample")).expect("asks again");
    assert_eq!(asked.get(), 2, "ending forgets the earlier yes");
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
    assert_eq!(asked.get(), 2);
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
fn a_panel_press_survives_a_switch_to_an_already_shared_target() {
    let (_person, asked) = person(Decision::Allowed);
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
    super::with_held(|held| held.share.set_required(true));
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
    assert_eq!(asked.get(), 1);
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
    let unshared = admit(ComputerMethod::Click, &click("Calculator")).expect("silence goes on");
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
    assert_eq!(
        ensure_current().err().map(|error| error.code).as_deref(),
        Some(error_code::SHARE_SCOPE),
        "the unshared click in flight"
    );
    drop(unshared);
    assert_eq!(state()["target"], "notes");
    assert_eq!(stand_in::notices().len(), told, "the band heard no move");
}
