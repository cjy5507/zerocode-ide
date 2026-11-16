//! The take control — the round mark the helper stands on every app in use,
//! the person's one press to take the app back — comes onto the screen and
//! leaves it without the system's window animation. AppKit animates a panel
//! in and out unless told not to, and the process keeps what each animation
//! drew: the pointer's windows were found holding 150 MiB that way
//! (v1.1.157, `computer_pointer.rs`), and the take control is made the same
//! way, one for every app in use. A probe outside the helper (2026-11-16)
//! took the controls of twenty-four apps off the screen once: 86 to 111 MiB
//! resident as they were, 43 with the animation off, and the control's own
//! opening under the person's pointer eased the same in both.
//!
//! The rule is a function over the source, so the second test can write a
//! return into a copy of the text and see the rule refuse it.

const PANELS: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/ShareControlPanel.swift"
);

const MADE: &str = "= ShareTakePanel(";
const QUIET: &str = ".animationBehavior = .none";

/// Each place a take control's panel is made, to the end of the function
/// that makes it.
fn makings(panels: &str) -> Vec<&str> {
    panels
        .match_indices(MADE)
        .map(|(at, _)| {
            let rest = &panels[at..];
            &rest[..rest.find("\n    }\n").unwrap_or(rest.len())]
        })
        .collect()
}

/// Every panel a take control is drawn in is told not to animate where it is
/// made, before it is handed on — told somewhere else, a panel made later
/// would come without it and the count would still hold.
fn no_take_control_is_animated_by_the_system(panels: &str) -> Result<(), String> {
    let makings = makings(panels);
    if makings.is_empty() {
        return Err(format!(
            "no take control is made as `{MADE}` any more: this rule reads nothing"
        ));
    }
    for making in makings {
        let told = making.matches(QUIET).count();
        if told != 1 {
            return Err(format!(
                "a take control's panel is made and told {told} times not to animate"
            ));
        }
        let handed_on = making
            .find("\n        return ")
            .ok_or("a take control's panel is made and never handed on")?;
        if making.find(QUIET).is_none_or(|at| at > handed_on) {
            return Err(
                "a take control's panel is handed on before it is told not to animate".into(),
            );
        }
    }
    Ok(())
}

#[test]
fn a_take_control_comes_and_goes_without_the_systems_animation() {
    if let Err(why) = no_take_control_is_animated_by_the_system(PANELS) {
        panic!("{why}");
    }
}

/// `source` with `was` written over by `now`. The place has to be there: a
/// rule tried on an unchanged text proves nothing.
fn returned(source: &str, was: &str, now: &str) -> String {
    assert!(source.contains(was), "nothing to write over: `{was}`");
    source.replacen(was, now, 1)
}

#[test]
fn a_take_control_left_to_the_systems_animation_is_refused() {
    // The line taken out again.
    let animated = returned(PANELS, "        made.animationBehavior = .none\n", "");
    assert!(no_take_control_is_animated_by_the_system(&animated).is_err());
    // Told where the control is ordered in front instead of where its panel is
    // made: the file still says it once.
    let told_elsewhere = returned(
        &animated,
        "    func orderFront() { panel?.orderFrontRegardless() }",
        "    func orderFront() {\n        panel?.animationBehavior = .none\n        panel?.orderFrontRegardless()\n    }",
    );
    assert!(no_take_control_is_animated_by_the_system(&told_elsewhere).is_err());
    // A second panel made beside the first, without it.
    let another = returned(
        PANELS,
        "    func orderFront() { panel?.orderFrontRegardless() }",
        "    private func spare(frame: CGRect) -> ShareTakePanel {\n        let made = ShareTakePanel(contentRect: frame, styleMask: [.borderless], backing: .buffered, defer: false)\n        return made\n    }\n\n    func orderFront() { panel?.orderFrontRegardless() }",
    );
    assert!(no_take_control_is_animated_by_the_system(&another).is_err());
    // Told after the panel is handed on is told to nobody.
    let told_late = returned(
        &animated,
        "        panel = made\n        return made\n",
        "        panel = made\n        return made\n        made.animationBehavior = .none\n",
    );
    assert!(no_take_control_is_animated_by_the_system(&told_late).is_err());
}
