use super::*;

fn refused(facts: &Facts, act: Act) -> Option<String> {
    judge(facts, act).err().map(|error| error.code)
}

const PRESSES: [Act; 3] = [Act::Click, Act::SetValue, Act::Scroll];

/// A control with no mark — a tab, a pane's button — is pressed, scrolled and
/// filled.
#[test]
fn an_ordinary_control_of_zerocodes_window_is_pressed() {
    for act in [Act::Look, Act::Click, Act::SetValue, Act::Scroll] {
        assert_eq!(refused(&Facts::default(), act), None, "{act:?}");
    }
}

/// Every mark that keeps a control the person's refuses every press, and a
/// look still reads it.
#[test]
fn a_marked_control_is_never_pressed_and_still_read() {
    let marked: [(&str, Facts); 6] = [
        (
            "asking",
            Facts {
                asking: true,
                ..Facts::default()
            },
        ),
        (
            "secret",
            Facts {
                secret: true,
                ..Facts::default()
            },
        ),
        (
            "beside a secret",
            Facts {
                beside_secret: true,
                ..Facts::default()
            },
        ),
        (
            "terminal",
            Facts {
                terminal: true,
                ..Facts::default()
            },
        ),
        (
            "person only",
            Facts {
                person_only: true,
                ..Facts::default()
            },
        ),
        (
            "setting",
            Facts {
                setting: true,
                ..Facts::default()
            },
        ),
    ];
    for (mark, facts) in marked {
        for act in PRESSES {
            let error = judge(&facts, act).expect_err(mark);
            assert_eq!(error.code, error_code::SELF_SENSITIVE, "{mark} {act:?}");
        }
        assert_eq!(refused(&facts, Act::Look), None, "{mark} is read");
    }
}

/// The field the person is in, or typed in a moment ago, is theirs: it is not
/// clicked or filled. A scroll moves no focus and no text, and goes.
#[test]
fn the_field_the_person_is_in_is_left_to_them() {
    let focused = Facts {
        person_focus: true,
        ..Facts::default()
    };
    let typing = Facts {
        typed_ms_ago: Some(PERSON_TYPING_QUIET_MS - 1),
        ..Facts::default()
    };
    for facts in [&focused, &typing] {
        for act in [Act::Click, Act::SetValue] {
            assert_eq!(
                refused(facts, act).as_deref(),
                Some(error_code::PERSON_TYPING),
                "{facts:?} {act:?}"
            );
        }
        assert_eq!(refused(facts, Act::Scroll), None);
    }
    let quiet = Facts {
        typed_ms_ago: Some(PERSON_TYPING_QUIET_MS),
        ..Facts::default()
    };
    assert_eq!(refused(&quiet, Act::SetValue), None);
}

/// A sensitive mark wins over the person's focus: the answer names the mark
/// that keeps the control, which no wait changes.
#[test]
fn a_sensitive_mark_is_named_before_the_persons_focus() {
    let facts = Facts {
        secret: true,
        person_focus: true,
        ..Facts::default()
    };
    assert_eq!(
        refused(&facts, Act::SetValue).as_deref(),
        Some(error_code::SELF_SENSITIVE)
    );
}

fn own() -> OwnApp {
    OwnApp {
        pid: 4_242,
        bundle_id: "dev.zerocode.app".into(),
        names: vec!["ZeroCode".into(), "zerocode-shell".into()],
    }
}

/// ZeroCode is named the way the helper names any app: a name, the bundle
/// identifier, any case, or its pid — never a fragment, never another pid.
#[test]
fn zerocode_is_named_by_name_bundle_or_own_pid() {
    let own = own();
    for query in [
        "ZeroCode",
        "zerocode",
        " ZeroCode ",
        "dev.zerocode.app",
        "zerocode-shell",
        "pid:4242",
    ] {
        assert!(own.named_by(query), "{query}");
    }
    for query in [
        "Zero",
        "ZeroCod",
        "Notes",
        "pid:4243",
        "pid:",
        "",
        "dev.zerocode",
    ] {
        assert!(!own.named_by(query), "{query}");
    }
}

#[test]
fn only_the_look_and_the_three_presses_are_the_pages() {
    for method in METHODS {
        assert!(Act::of_method(method).is_some(), "{method}");
    }
    for method in [
        "typeText",
        "pressKey",
        "hotkey",
        "pasteText",
        "drag",
        "mouseClick",
    ] {
        assert_eq!(Act::of_method(method), None, "{method}");
    }
}

/// The person's text leaves a look as its length; a choice, a state and the
/// text of a field nobody marks the person's read as they are (t-74975).
#[test]
fn a_look_shows_the_persons_text_as_its_length() {
    let words = "주소 SENTINEL";
    let length = withheld_text(11);
    for facts in [
        Facts {
            person_focus: true,
            ..Facts::default()
        },
        Facts {
            typed_ms_ago: Some(60_000),
            ..Facts::default()
        },
        Facts {
            setting: true,
            ..Facts::default()
        },
        Facts {
            beside_secret: true,
            ..Facts::default()
        },
    ] {
        for role in TEXT_ENTRY_ROLES {
            assert_eq!(
                shown_value(&facts, role, Some(words), None),
                Some(length.clone()),
                "{role} {facts:?}"
            );
        }
        assert_eq!(
            shown_value(&facts, "AXCheckBox", Some("checked"), None),
            Some("checked".into()),
            "{facts:?}"
        );
        assert_eq!(
            shown_value(&facts, "AXTextField", Some(""), None),
            Some(String::new())
        );
    }
    let plain = Facts::default();
    assert_eq!(
        shown_value(&plain, "AXTextField", Some("main.rs"), None),
        Some("main.rs".into())
    );
    assert_eq!(
        shown_value(&plain, "AXTextArea", None, Some(42)),
        Some(withheld_text(42))
    );
    let secret = Facts {
        secret: true,
        ..Facts::default()
    };
    assert_eq!(shown_value(&secret, "AXTextField", Some(words), None), None);
    assert_eq!(shown_value(&secret, "AXTextField", None, Some(9)), None);
}
