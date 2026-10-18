//! The work screen panel's looks (t-59372): a look belongs to the pane that
//! took it and the assignment the pane held, and the window is told of it
//! by number, never by picture.

use super::structured_tests::{frame, with_picture};
use super::*;
use crate::run_evidence::{Actor, acting_for};
use serde_json::json;

fn one_hand() -> std::sync::MutexGuard<'static, ()> {
    crate::tests::computer_desktop_wait::ONE_HAND
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What the window was told by every look here: the pane, the assignment and
/// the look's number.
static HEARD: Mutex<Vec<(String, String, u64)>> = Mutex::new(Vec::new());

fn listen() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        install_notice(Box::new(|pane, scope, seq| {
            HEARD
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((pane.to_string(), scope.to_string(), seq));
        }));
    });
}

fn heard() -> Vec<(String, String, u64)> {
    HEARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn heard_by(pane: &str) -> Vec<(String, u64)> {
    heard()
        .into_iter()
        .filter(|(by, _, _)| by == pane)
        .map(|(_, scope, seq)| (scope, seq))
        .collect()
}

fn actor(pane: &str, scope: Option<&str>) -> Actor {
    Actor {
        pane: Some(pane.to_string()),
        task: None,
        scope: scope.map(str::to_string),
    }
}

fn request(params: Value) -> Map<String, Value> {
    params.as_object().unwrap().clone()
}

/// An app's window seen with its picture.
fn picture_look(viewer: &str) {
    observe_with(
        &request(json!({"app": "Fixture", "viewer": viewer})),
        &super::super::eye::Memory::new(),
        &mut |method, _| {
            assert_eq!(method, "getAppState");
            Ok(with_picture(frame()))
        },
        &mut |_| {},
    )
    .unwrap();
}

/// An app's window read through its tree alone: no picture was taken.
fn tree_look(viewer: &str) {
    observe_with(
        &request(json!({"app": "Fixture", "noScreenshot": true, "viewer": viewer})),
        &super::super::eye::Memory::new(),
        &mut |method, _| {
            assert_eq!(method, "getAppState");
            Ok(frame())
        },
        &mut |_| panic!("a tree-only look does not sleep"),
    )
    .unwrap();
}

fn seq_of(look: &Value) -> u64 {
    look["seq"]
        .as_u64()
        .unwrap_or_else(|| panic!("a numbered look: {look}"))
}

#[test]
fn a_look_belongs_to_the_pane_and_assignment_that_took_it_and_to_no_other() {
    const PANE: &str = "term-5937201";
    let _hand = one_hand();
    listen();
    {
        let _acting = acting_for(actor(PANE, Some("scope-a")));
        picture_look("owned-mine");
    }
    let mine = owned_look(PANE, "scope-a");
    let other_assignment = owned_look(PANE, "scope-b");
    let other_pane = owned_look("term-5937202", "scope-a");
    let mine = mine.expect("the pane's own look");
    assert_eq!(mine["perception"], "screenshot", "{mine}");
    assert_eq!(
        mine["png"],
        with_picture(frame())["screenshot"]["data"],
        "the picture the look saw"
    );
    assert!(mine["at"].as_i64().is_some_and(|at| at > 0), "{mine}");
    assert!(other_assignment.is_none(), "{other_assignment:?}");
    assert!(other_pane.is_none(), "{other_pane:?}");
    assert_eq!(heard_by(PANE), [("scope-a".to_string(), seq_of(&mine))]);
}

/// The same pane given new work (t-59372): the new assignment sees nothing
/// until it looks, a late ask for the old assignment gets nothing once the
/// pane looked again, and the numbers only grow.
#[test]
fn a_pane_given_new_work_shows_only_the_new_assignments_looks() {
    const PANE: &str = "term-5937211";
    let _hand = one_hand();
    listen();
    {
        let _acting = acting_for(actor(PANE, Some("scope-a")));
        picture_look("owned-retask-a");
    }
    let old = owned_look(PANE, "scope-a");
    let before_it_looks = owned_look(PANE, "scope-next");
    {
        let _acting = acting_for(actor(PANE, Some("scope-next")));
        picture_look("owned-retask-next");
    }
    let late = owned_look(PANE, "scope-a");
    let new = owned_look(PANE, "scope-next");
    let old = old.expect("the first assignment's look");
    let new = new.expect("the new assignment's look");
    assert!(before_it_looks.is_none(), "{before_it_looks:?}");
    assert!(late.is_none(), "{late:?}");
    assert!(seq_of(&new) > seq_of(&old), "{old} → {new}");
    assert_eq!(
        heard_by(PANE),
        [
            ("scope-a".to_string(), seq_of(&old)),
            ("scope-next".to_string(), seq_of(&new)),
        ]
    );
}

/// A look taken for no pane, or for a pane whose assignment the window
/// cannot tell, is shown to nobody and told to nobody.
#[test]
fn a_look_no_assignment_owns_is_shown_and_told_to_nobody() {
    const PANE: &str = "term-5937221";
    let _hand = one_hand();
    listen();
    let told = heard().len();
    picture_look("owned-nobody");
    {
        let _acting = acting_for(actor(PANE, None));
        picture_look("owned-unknown-scope");
    }
    assert_eq!(heard().len(), told, "{:?}", heard());
    assert!(owned_look(PANE, "").is_none());
}

/// A look that read only the screen's structure is still the pane's newest
/// look: it is numbered and says it has no picture, and the older picture is
/// not shown in its place.
#[test]
fn a_tree_only_look_is_numbered_and_says_it_saw_no_picture() {
    const PANE: &str = "term-5937231";
    let _hand = one_hand();
    listen();
    let _acting = acting_for(actor(PANE, Some("scope-a")));
    picture_look("owned-tree-picture");
    let pictured = owned_look(PANE, "scope-a");
    tree_look("owned-tree-only");
    let tree = owned_look(PANE, "scope-a");
    let pictured = pictured.expect("the look with a picture");
    let tree = tree.expect("the tree-only look");
    assert_eq!(tree["perception"], "accessibility", "{tree}");
    assert!(tree["png"].is_null(), "{tree}");
    assert!(seq_of(&tree) > seq_of(&pictured), "{pictured} → {tree}");
    assert_eq!(heard_by(PANE).len(), 2, "{:?}", heard_by(PANE));
}

/// The window keeps as many pictures as it keeps frames: a pane whose
/// picture was let go still says it looked, and that its picture is gone.
#[test]
fn a_look_whose_picture_is_no_longer_kept_says_so_and_shows_no_other() {
    const FIRST: &str = "term-5937240";
    let _hand = one_hand();
    listen();
    {
        let _acting = acting_for(actor(FIRST, Some("scope-a")));
        picture_look("owned-gone-first");
    }
    let kept = owned_look(FIRST, "scope-a");
    for pane in 1..=OBSERVE_FRAMES_MAX {
        let pane = format!("term-59372{}", 40 + pane);
        let _acting = acting_for(actor(&pane, Some("scope-a")));
        picture_look(&format!("owned-gone-{pane}"));
    }
    let gone = owned_look(FIRST, "scope-a");
    let newest = owned_look(&format!("term-59372{}", 40 + OBSERVE_FRAMES_MAX), "scope-a");
    let kept = kept.expect("the first look, while kept");
    let gone = gone.expect("the first look, after its picture went");
    assert_eq!(kept["perception"], "screenshot", "{kept}");
    assert_eq!(seq_of(&gone), seq_of(&kept), "the same look");
    assert_eq!(gone["perception"], "gone", "{gone}");
    assert!(gone["png"].is_null(), "{gone}");
    assert!(
        newest.is_some_and(|newest| newest["png"].is_string()),
        "the newest picture is kept"
    );
}

/// A pane that looks while the window cannot tell its assignment (t-63505
/// ④): that look is nobody's, and the pane's older look goes with it — a late
/// ask under the old assignment gets nothing rather than a look older than the
/// pane's newest. The window is told the older look went (t-62872): under its
/// assignment, by a number newer than the look's, so a panel still showing
/// that look asks again, gets nothing, and lets its picture go.
#[test]
fn a_look_under_an_assignment_the_window_cannot_tell_lets_the_panes_older_look_go() {
    const PANE: &str = "term-6350541";
    let _hand = one_hand();
    listen();
    {
        let _acting = acting_for(actor(PANE, Some("scope-a")));
        picture_look("owned-unknown-after-a");
    }
    let before = owned_look(PANE, "scope-a");
    let told = heard_by(PANE);
    {
        let _acting = acting_for(actor(PANE, None));
        picture_look("owned-unknown-after-none");
    }
    let after = owned_look(PANE, "scope-a");
    let heard = heard_by(PANE);
    let before = before.expect("the pane's look under its assignment");
    assert!(
        after.is_none(),
        "a look older than the pane's newest is never shown: {after:?}"
    );
    assert_eq!(
        heard.len(),
        told.len() + 1,
        "the window is told the older look went: {heard:?}"
    );
    let (scope, seq) = heard.last().expect("told");
    assert_eq!(
        scope, "scope-a",
        "told under the assignment whose look went"
    );
    assert!(
        *seq > seq_of(&before),
        "by a number newer than the look it showed: {seq} after {before}"
    );
}

/// A picture past the budget every provider holds a screenshot to — only a
/// full-resolution shot gets there — is not kept for the panel (t-63505 ③):
/// the look is still the pane's newest and numbered, and says its picture was
/// too large; one within the budget is kept as it was.
#[test]
fn a_picture_past_the_screenshot_budget_is_numbered_but_not_kept() {
    use zerocode_core::computer_use_protocol::MAX_SCREENSHOT_PNG_BYTES;
    const PANE: &str = "term-6350531";
    let _hand = one_hand();
    listen();
    let full = request(json!({"viewer": "owned-oversize", "fullRes": true}));
    let _acting = acting_for(actor(PANE, Some("scope-a")));
    remember(&full, &json!({}), vec![7; MAX_SCREENSHOT_PNG_BYTES]);
    let kept = owned_look(PANE, "scope-a").expect("the look within the budget");
    remember(&full, &json!({}), vec![7; MAX_SCREENSHOT_PNG_BYTES + 1]);
    let past = owned_look(PANE, "scope-a").expect("the look past the budget");
    assert_eq!(kept["perception"], "screenshot", "{}", kept["perception"]);
    assert!(kept["png"].is_string(), "the picture within it is kept");
    assert_eq!(past["perception"], "oversize", "{}", past["perception"]);
    assert!(
        past["png"].is_null(),
        "no picture past the budget is kept or sent"
    );
    assert!(seq_of(&past) > seq_of(&kept), "the pane's newest look");
}

/// The screenshot verb's picture is a look its pane took.
#[test]
fn a_screenshot_is_a_look_its_pane_owns() {
    use base64::Engine as _;
    const PANE: &str = "term-5937251";
    let picture = vec![137, 80, 78, 71, 13, 10, 26, 10];
    let _hand = one_hand();
    listen();
    {
        let _acting = acting_for(actor(PANE, Some("scope-a")));
        remember(
            &request(json!({"viewer": "owned-screenshot"})),
            &json!({}),
            picture.clone(),
        );
    }
    let shot = owned_look(PANE, "scope-a").expect("the screenshot's look");
    assert_eq!(shot["perception"], "screenshot", "{shot}");
    assert_eq!(
        shot["png"],
        base64::engine::general_purpose::STANDARD.encode(&picture)
    );
    assert_eq!(heard_by(PANE), [("scope-a".to_string(), seq_of(&shot))]);
}

/// What an assignment is (t-59372): the pane, its seat and dispatch and the
/// conversation it holds, hashed whole — so the same pane given new work, or
/// a new conversation, is another assignment.
#[test]
fn a_new_dispatch_or_conversation_in_the_same_pane_is_another_assignment() {
    use crate::agent_tools_runtime::frame_scope_of;
    let seat = [
        Some("term-7"),
        Some("zo"),
        Some("run-1"),
        Some("w-1"),
        Some("t-1"),
    ];
    let first = frame_scope_of(seat, Some("dp-1"), Some("session-1"))
        .expect("a dispatch and a conversation tell the assignment");
    assert_eq!(first.len(), 64, "{first}");
    assert!(
        first
            .chars()
            .all(|digit| digit.is_ascii_digit() || ('a'..='f').contains(&digit)),
        "{first}"
    );
    let first = Some(first);
    assert_eq!(frame_scope_of(seat, Some("dp-1"), Some("session-1")), first);
    for other in [
        frame_scope_of(seat, Some("dp-2"), Some("session-1")),
        frame_scope_of(seat, Some("dp-1"), Some("session-2")),
        frame_scope_of(
            [
                Some("term-7"),
                Some("zo"),
                Some("run-1"),
                Some("w-1"),
                Some("t-2"),
            ],
            Some("dp-1"),
            Some("session-1"),
        ),
        frame_scope_of(
            [
                Some("term-8"),
                Some("zo"),
                Some("run-1"),
                Some("w-1"),
                Some("t-1"),
            ],
            Some("dp-1"),
            Some("session-1"),
        ),
    ] {
        assert!(other.is_some() && other != first, "{other:?}");
    }
    assert!(frame_scope_of(seat, None, Some("session-1")).is_some());
    assert!(frame_scope_of(seat, Some("dp-1"), None).is_some());
    assert_ne!(
        frame_scope_of(seat, Some("same"), None),
        frame_scope_of(seat, None, Some("same")),
        "a dispatch is not a conversation"
    );
    assert_ne!(
        frame_scope_of(seat, Some("dp-1"), None),
        frame_scope_of(
            [
                Some("term-7"),
                Some("zo"),
                Some("run-1"),
                Some("w-1"),
                Some("t-1d")
            ],
            Some("p-1"),
            None,
        ),
        "the values are told apart, not run together"
    );
    assert_eq!(frame_scope_of(seat, None, None), None);
    assert_eq!(frame_scope_of(seat, Some(""), Some("")), None);
}
