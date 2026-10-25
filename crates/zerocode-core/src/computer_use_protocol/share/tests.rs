use super::*;

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
    assert_eq!(share.gate(act(Some("safari"))), Gate::Ask("safari".into()));
    share.answered("safari", Answer::Shared);
    assert_eq!(share.target(), Some("safari"));
    for _ in 0..3 {
        assert_eq!(share.gate(act(Some("safari"))), Gate::Go);
    }
}

/// While a share stands, everything outside it is refused: another app's
/// look, a whole-screen picture, an act naming no app. Another app's act asks.
#[test]
fn a_share_holds_the_agent_to_its_one_window() {
    let mut share = Share::default();
    share.answered("safari", Answer::Shared);
    assert_eq!(
        code(share.gate(look(Some("notes")))).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        code(share.gate(SCREEN)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        code(share.gate(act(None))).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        share.gate(look(None)),
        Gate::Go,
        "the app list is no picture"
    );
    assert_eq!(share.gate(look(Some("safari"))), Gate::Go);
    assert_eq!(share.gate(act(Some("notes"))), Gate::Ask("notes".into()));
    share.answered("notes", Answer::Shared);
    assert_eq!(share.target(), Some("notes"), "a yes moves the share");
    assert_eq!(
        share.gate(act(Some("safari"))),
        Gate::Go,
        "an app said yes to before is not asked"
    );
    assert_eq!(share.target(), Some("safari"));
}

/// A no refuses that app; silence never widens a share that stands.
#[test]
fn a_no_refuses_and_silence_never_widens_a_share() {
    let mut share = Share::default();
    share.answered("notes", Answer::Declined);
    assert_eq!(
        code(share.gate(act(Some("notes")))).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    share.answered("safari", Answer::Shared);
    share.answered("mail", Answer::Silent);
    assert_eq!(
        code(share.gate(act(Some("mail")))).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(share.target(), Some("safari"));
}

/// Silence with nothing shared — the person away — leaves the agent working
/// as before, and that app is not asked again this session.
#[test]
fn silence_with_nothing_shared_works_as_before_and_is_not_asked_again() {
    let mut share = Share::default();
    share.answered("calculator", Answer::Silent);
    assert_eq!(share.gate(act(Some("calculator"))), Gate::Go);
    assert_eq!(share.target(), None);
    assert_eq!(
        share.gate(SCREEN),
        Gate::Go,
        "nothing shared, nothing narrowed"
    );
    assert_eq!(share.gate(act(None)), Gate::Go);
}

/// Taking control refuses every act, asks nothing, and moves the generation;
/// handing back lets the shared window be worked again.
#[test]
fn taking_control_refuses_every_act_until_the_person_hands_back() {
    let mut share = Share::default();
    share.answered("safari", Answer::Shared);
    let before = share.generation();
    share.take_back();
    assert!(share.taken_back());
    assert!(share.generation() > before);
    for reach in [act(Some("safari")), act(Some("notes")), act(None)] {
        assert_eq!(
            code(share.gate(reach)).as_deref(),
            Some(error_code::PERSON_TOOK_CONTROL),
            "{reach:?}"
        );
    }
    assert_eq!(
        share.gate(look(Some("safari"))),
        Gate::Go,
        "a look is no act"
    );
    let taken = share.generation();
    share.take_back();
    assert_eq!(share.generation(), taken, "a second press moves nothing");
    share.give_back();
    assert!(share.generation() > taken);
    assert_eq!(share.gate(act(Some("safari"))), Gate::Go);
}

#[test]
fn an_app_is_one_key_whatever_its_case() {
    assert_eq!(app_key(" Safari "), app_key("safari"));
}

#[test]
fn required_sharing_refuses_silence_and_unnamed_acts_but_keeps_looks() {
    let mut share = Share::default();
    share.set_required(true);
    assert_eq!(share.gate(act(Some("notes"))), Gate::Ask("notes".into()));
    share.answered("notes", Answer::Silent);
    for reach in [act(Some("notes")), act(None)] {
        let Gate::Refuse(error) = share.gate(reach) else {
            panic!("unshared act was admitted: {reach:?}");
        };
        assert_eq!(error.code, "share_required");
        assert!(error.message.contains("share"));
    }
    assert_eq!(share.gate(look(Some("notes"))), Gate::Go);
    assert_eq!(share.gate(SCREEN), Gate::Go);
    share.set_required(false);
    assert_eq!(share.gate(act(Some("notes"))), Gate::Go);
}

#[test]
fn required_sharing_allows_a_yes_and_keeps_refusal_scope_and_take_back() {
    let mut share = Share::default();
    share.set_required(true);
    share.answered("notes", Answer::Declined);
    assert!(matches!(share.gate(act(Some("notes"))), Gate::Refuse(_)));
    share.answered("safari", Answer::Shared);
    assert_eq!(share.gate(act(Some("safari"))), Gate::Go);
    assert_eq!(
        code(share.gate(look(Some("notes")))).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    assert_eq!(
        code(share.gate(SCREEN)).as_deref(),
        Some(error_code::SHARE_SCOPE)
    );
    share.take_back();
    assert_eq!(
        code(share.gate(act(Some("safari")))).as_deref(),
        Some(error_code::PERSON_TOOK_CONTROL)
    );
}
