//! The agent's drawn pointer never holds an act up, and no pointer gives way
//! to another. Measured on the installed helper (2026-11-15): a background
//! press waited 240 of its 423 ms for the pointer to arrive, and one agent
//! working in several apps at once showed one pointer. The person's rule is
//! speed, and "several at once" is to be seen as several.

const HELPER: &str =
    include_str!("../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/main.swift");
const OVERLAY: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/BackgroundPointerOverlay.swift"
);

/// The helper's `travel(to:in:)`, from its signature to the end of its body.
fn travel() -> &'static str {
    let start = HELPER
        .find("private func travel(to point: CGPoint, in snapshot: Snapshot)")
        .expect("the helper's travel");
    let rest = &HELPER[start..];
    &rest[..rest.find("\n    }\n").expect("the end of travel")]
}

#[test]
fn a_background_act_does_not_wait_for_its_drawn_pointer() {
    let travel = travel();
    for waits in ["sleep", "waitOut", "DispatchSemaphore"] {
        assert!(
            !travel.contains(waits),
            "the act waits for its pointer again (`{waits}`):\n{travel}"
        );
    }
    assert!(
        travel.contains("guard !OperatorGuardHost.isStopped() else {"),
        "a stop is read the moment before the act goes:\n{travel}"
    );
}

#[test]
fn the_press_ring_waits_for_the_pointer_to_arrive_and_the_press_does_not() {
    assert!(
        travel().contains("-> PointerArrival?"),
        "travel says when the pointer arrives"
    );
    for held in [
        "let left = arrival.secondsLeft(nowNs: DispatchTime.now().uptimeNanoseconds)",
        "DispatchQueue.main.asyncAfter(deadline: .now() + left) {",
    ] {
        assert!(OVERLAY.contains(held), "the overlay lost `{held}`");
    }
}

#[test]
fn an_agent_has_a_pointer_for_each_app_it_works_in_and_none_gives_way_to_another() {
    assert!(
        OVERLAY.contains("private var slots: [PointerKey: Slot] = [:]"),
        "a pointer is kept by its agent and its app"
    );
    assert!(
        !OVERLAY.contains("maxPointers"),
        "a bound on how many pointers are drawn is back"
    );
}
