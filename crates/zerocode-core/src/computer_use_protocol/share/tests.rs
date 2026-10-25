use super::*;

/// The window's clock when a test does not care what it reads.
const NOW: u64 = 0;

const fn act(app: Option<&str>) -> Reach<'_> {
    Reach {
        app,
        acts: true,
        whole_screen: false,
    }
}

const fn look(app: Option<&str>) -> Reach<'_> {
    Reach {
        app,
        acts: false,
        whole_screen: false,
    }
}

const SCREEN: Reach<'static> = Reach {
    app: None,
    acts: false,
    whole_screen: true,
};

fn code(gate: Gate) -> Option<String> {
    match gate {
        Gate::Refuse(error) => Some(error.code),
        Gate::Go | Gate::Ask(_) => None,
    }
}

/// The first act in an app asks; a yes shares it, and the same app is never
/// asked again.
#[test]
fn the_first_act_in_an_app_asks_once_and_a_yes_shares_it() {
    let mut share = Share::default();
    assert_eq!(share.gate(act(Some("safari")), NOW), Gate::Ask("safari".into()));
    share.answered("safari", Answer::Shared, NOW);
    assert_eq!(share.target(), Some("safari"));
    for _ in 0..3 {
        assert_eq!(share.gate(act(Some("safari")), NOW), Gate::Go);
    }
}

/// While a share stands, everything outside it is refused: another app's
/// look, a whole-screen picture, an act naming no app. Another app's act asks.
#[test]
fn a_share_holds_the_agent_to_its_one_window() {
    let mut share = Share::default();
    share.answered("safari", Answer::Shared, NOW);
    assert_eq!(
        code(share.gate(look(Some("notes")), NOW)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        code(share.gate(SCREEN, NOW)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        code(share.gate(act(None), NOW)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        share.gate(look(None), NOW),
        Gate::Go,
        "the app list is no picture"
    );
    assert_eq!(share.gate(look(Some("safari")), NOW), Gate::Go);
    assert_eq!(share.gate(act(Some("notes")), NOW), Gate::Ask("notes".into()));
    share.answered("notes", Answer::Shared, NOW);
    assert_eq!(share.target(), Some("notes"), "a yes moves the share");
    assert_eq!(
        share.gate(act(Some("safari")), NOW),
        Gate::Go,
        "an app said yes to before is not asked"
    );
    assert_eq!(share.target(), Some("safari"));
}

/// A no refuses that app; silence never widens a share that stands.
#[test]
fn a_no_refuses_and_silence_never_widens_a_share() {
    let mut share = Share::default();
    share.answered("notes", Answer::Declined, NOW);
    assert_eq!(
        code(share.gate(act(Some("notes")), NOW)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    share.answered("safari", Answer::Shared, NOW);
    share.answered("mail", Answer::Silent, NOW);
    assert_eq!(
        code(share.gate(act(Some("mail")), NOW)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(share.target(), Some("safari"));
}

/// Silence with nothing shared — the person away — leaves the agent working
/// as before, and that app is not asked again this session.
#[test]
fn silence_with_nothing_shared_works_as_before_and_is_not_asked_again() {
    let mut share = Share::default();
    share.answered("calculator", Answer::Silent, NOW);
    assert_eq!(share.gate(act(Some("calculator")), NOW), Gate::Go);
    assert_eq!(share.target(), None);
    assert_eq!(
        share.gate(SCREEN, NOW),
        Gate::Go,
        "nothing shared, nothing narrowed"
    );
    assert_eq!(share.gate(act(None), NOW), Gate::Go);
}

/// Taking control refuses every act, asks nothing, and moves the generation;
/// handing back lets the shared window be worked again.
#[test]
fn taking_control_refuses_every_act_until_the_person_hands_back() {
    let mut share = Share::default();
    share.answered("safari", Answer::Shared, NOW);
    let before = share.generation();
    share.take_back();
    assert!(share.taken_back());
    assert!(share.generation() > before);
    for reach in [act(Some("safari")), act(Some("notes")), act(None)] {
        assert_eq!(
            code(share.gate(reach, NOW)).as_deref(),
            Some(error_code::PERSON_TOOK_CONTROL),
            "{reach:?}"
        );
    }
    assert_eq!(
        share.gate(look(Some("safari")), NOW),
        Gate::Go,
        "a look is no act"
    );
    let taken = share.generation();
    share.take_back();
    assert_eq!(share.generation(), taken, "a second press moves nothing");
    share.give_back();
    assert!(share.generation() > taken);
    assert_eq!(share.gate(act(Some("safari")), NOW), Gate::Go);
}

#[test]
fn an_app_is_one_key_whatever_its_case() {
    assert_eq!(app_key(" Safari "), app_key("safari"));
}

#[test]
fn required_sharing_refuses_silence_and_unnamed_acts_but_keeps_looks() {
    let mut share = Share::default();
    share.set_required(true);
    assert_eq!(share.gate(act(Some("notes")), NOW), Gate::Ask("notes".into()));
    share.answered("notes", Answer::Silent, NOW);
    for reach in [act(Some("notes")), act(None)] {
        let Gate::Refuse(error) = share.gate(reach, NOW) else {
            panic!("unshared act was admitted: {reach:?}");
        };
        assert_eq!(error.code, "share_required");
        assert!(error.message.contains("share"));
    }
    assert_eq!(share.gate(look(Some("notes")), NOW), Gate::Go);
    assert_eq!(share.gate(SCREEN, NOW), Gate::Go);
    share.set_required(false);
    assert_eq!(share.gate(act(Some("notes")), NOW), Gate::Go);
}

#[test]
fn required_sharing_allows_a_yes_and_keeps_refusal_scope_and_take_back() {
    let mut share = Share::default();
    share.set_required(true);
    share.answered("notes", Answer::Declined, NOW);
    assert!(matches!(share.gate(act(Some("notes")), NOW), Gate::Refuse(_)));
    share.answered("safari", Answer::Shared, NOW);
    assert_eq!(share.gate(act(Some("safari")), NOW), Gate::Go);
    assert_eq!(
        code(share.gate(look(Some("notes")), NOW)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        code(share.gate(SCREEN, NOW)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    share.take_back();
    assert_eq!(
        code(share.gate(act(Some("safari")), NOW)).as_deref(),
        Some(error_code::PERSON_TOOK_CONTROL)
    );
}

/// R1 (t-74826): with sharing required, silence is no lasting no. The app
/// waits for the person: its acts are refused at once, with no card, for
/// [`SHARE_ASK_AGAIN_MS`]; the first act after that asks again.
#[test]
fn required_silence_waits_then_the_next_act_after_the_quiet_asks_again() {
    let mut share = Share::default();
    share.set_required(true);
    let asked = 1_000;
    assert_eq!(share.gate(act(Some("notes")), asked), Gate::Ask("notes".into()));
    share.answered("notes", Answer::Silent, asked);
    assert_eq!(share.waiting(), Some("notes"));
    for now in [asked, asked + SHARE_ASK_AGAIN_MS - 1] {
        assert_eq!(
            code(share.gate(act(Some("notes")), now)).as_deref(),
            Some(error_code::SHARE_REQUIRED),
            "no card within the quiet, at {now} ms"
        );
    }
    assert_eq!(
        share.gate(act(Some("notes")), asked + SHARE_ASK_AGAIN_MS),
        Gate::Ask("notes".into()),
        "the first act after the quiet asks again"
    );
    share.answered("notes", Answer::Shared, asked + SHARE_ASK_AGAIN_MS);
    assert_eq!(share.target(), Some("notes"));
    assert_eq!(share.waiting(), None, "a yes ends the wait");
}

/// The refusal while an app waits tells the agent when the window asks
/// again and that the person can hand it over from the window.
#[test]
fn a_waiting_refusal_says_when_the_window_asks_again() {
    let mut share = Share::default();
    share.set_required(true);
    share.answered("notes", Answer::Silent, 0);
    let Gate::Refuse(error) = share.gate(act(Some("notes")), SHARE_ASK_AGAIN_MS / 2) else {
        panic!("a waiting app was admitted");
    };
    assert_eq!(error.code, error_code::SHARE_REQUIRED);
    let seconds = (SHARE_ASK_AGAIN_MS / 2).div_ceil(1_000).to_string();
    assert!(error.message.contains("notes"), "{}", error.message);
    assert!(error.message.contains(&seconds), "{}", error.message);
    assert!(error.message.contains("band"), "{}", error.message);
}

/// The person can hand a waiting app over later, from the window, without
/// waiting for a card: that app only, and only while it waits.
#[test]
fn the_person_can_hand_the_waiting_app_over_later() {
    let mut share = Share::default();
    share.set_required(true);
    assert!(!share.hand_over("notes"), "nothing waits yet");
    share.answered("notes", Answer::Silent, 0);
    assert!(!share.hand_over("safari"), "only the app that waits");
    assert!(share.hand_over("notes"));
    assert_eq!(share.target(), Some("notes"));
    assert_eq!(share.waiting(), None);
    assert_eq!(share.gate(act(Some("notes")), 0), Gate::Go);
    assert!(!share.hand_over("notes"), "a share is handed over once");
}

/// With a share standing, another app's silence under required sharing
/// waits the same way and never widens the share.
#[test]
fn required_silence_beside_a_share_waits_and_never_widens_it() {
    let mut share = Share::default();
    share.set_required(true);
    share.answered("safari", Answer::Shared, 0);
    assert_eq!(share.gate(act(Some("notes")), 0), Gate::Ask("notes".into()));
    share.answered("notes", Answer::Silent, 0);
    assert_eq!(share.target(), Some("safari"));
    assert_eq!(share.waiting(), Some("notes"));
    assert_eq!(
        code(share.gate(act(Some("notes")), 1)).as_deref(),
        Some(error_code::SHARE_REQUIRED)
    );
    assert_eq!(share.target(), Some("safari"), "silence never widens a share");
    assert_eq!(
        share.gate(act(Some("notes")), SHARE_ASK_AGAIN_MS),
        Gate::Ask("notes".into())
    );
    assert!(share.hand_over("notes"));
    assert_eq!(share.target(), Some("notes"));
}

/// Without required sharing nothing waits: silence lets the agent work
/// unshared, and that app is never asked again, however long after.
#[test]
fn without_required_sharing_silence_never_waits_and_is_never_asked_again() {
    let mut share = Share::default();
    share.answered("calculator", Answer::Silent, 0);
    assert_eq!(share.waiting(), None);
    assert!(!share.hand_over("calculator"));
    assert_eq!(
        share.gate(act(Some("calculator")), SHARE_ASK_AGAIN_MS * 10),
        Gate::Go
    );
    share.set_required(true);
    assert_eq!(share.waiting(), Some("calculator"), "requiring it makes it wait");
    share.set_required(false);
    assert_eq!(share.waiting(), None);
}

/// A no stays a no under required sharing: it is no silence to ask again.
#[test]
fn a_no_under_required_sharing_is_not_asked_again_and_does_not_wait() {
    let mut share = Share::default();
    share.set_required(true);
    share.answered("notes", Answer::Declined, 0);
    assert_eq!(share.waiting(), None);
    assert!(!share.hand_over("notes"));
    assert_eq!(
        code(share.gate(act(Some("notes")), SHARE_ASK_AGAIN_MS * 10)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
}
