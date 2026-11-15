//! The agent's drawn pointer never holds an act up, and no pointer gives way
//! to another. Measured on the installed helper (2026-11-15): a background
//! press waited 240 of its 423 ms for the pointer to arrive, and one agent
//! working in several apps at once showed one pointer. The person's rule is
//! speed, and "several at once" is to be seen as several.
//!
//! Each rule is a function over the two sources, so the last test can write a
//! part-way return into a copy of the text and see the rule refuse it
//! (t-109788: a return of the whole change was caught, four part-way ones
//! were not).

const HELPER: &str =
    include_str!("../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/main.swift");
const OVERLAY: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/BackgroundPointerOverlay.swift"
);

/// What holds a thread where it stands.
const WAITS: [&str; 7] = [
    "sleep(",
    "Thread.sleep",
    "waitOut",
    "DispatchSemaphore",
    "DispatchGroup",
    ".wait(",
    "RunLoop",
];

/// `source` from where `opens` begins to the end of that function's body.
fn body<'a>(source: &'a str, opens: &str) -> Result<&'a str, String> {
    let start = source
        .find(opens)
        .ok_or_else(|| format!("`{opens}` is gone"))?;
    let rest = &source[start..];
    let end = rest
        .find("\n    }\n")
        .ok_or_else(|| format!("`{opens}` has no end"))?;
    Ok(&rest[..end])
}

/// The helper's `travel(to:in:)`.
fn travel(helper: &str) -> Result<&str, String> {
    body(
        helper,
        "private func travel(to point: CGPoint, in snapshot: Snapshot)",
    )
}

/// The background branch of the helper's `click`: from where it turns to the
/// background to its last press.
fn background_click(helper: &str) -> Result<&str, String> {
    let click = helper
        .find("private func click(params: [String: JSONValue], snapshot: Snapshot) throws -> [String: Any] {")
        .map(|start| &helper[start..])
        .ok_or("the helper's click is gone")?;
    let branch = click
        .find("if params[\"background\"]?.bool == true {")
        .map(|start| &click[start..])
        .ok_or("click's background branch is gone")?;
    let end = branch
        .find("return actionMetadata(path: \"window\", verification: unverifiedAction(reason: \"synthetic_input\"))")
        .ok_or("the background branch's window press is gone")?;
    Ok(&branch[..end])
}

/// No act waits for its drawn pointer: not in the helper's `travel`, not in
/// the background press between the pointer's setting off and the press, and
/// nowhere in the overlay, where what waits is drawn later on the main queue.
fn no_act_waits_for_its_pointer(helper: &str, overlay: &str) -> Result<(), String> {
    let travel = travel(helper)?;
    for (place, text) in [
        ("the helper's travel", travel),
        ("the background press", background_click(helper)?),
        ("the overlay", overlay),
    ] {
        if let Some(waits) = WAITS.iter().find(|waits| text.contains(**waits)) {
            return Err(format!(
                "an act waits for its pointer again: `{waits}` in {place}"
            ));
        }
    }
    if !travel.contains("guard !OperatorGuardHost.isStopped() else {") {
        return Err(format!(
            "no stop is read the moment before the act goes:\n{travel}"
        ));
    }
    Ok(())
}

/// The ring of a press waits for the pointer to arrive; the press does not.
fn the_ring_waits_for_the_pointer(helper: &str, overlay: &str) -> Result<(), String> {
    if !travel(helper)?.contains("-> PointerArrival?") {
        return Err("travel no longer says when the pointer arrives".into());
    }
    for held in [
        "let left = arrival.secondsLeft(nowNs: DispatchTime.now().uptimeNanoseconds)",
        "DispatchQueue.main.asyncAfter(deadline: .now() + left) {",
    ] {
        if !overlay.contains(held) {
            return Err(format!("the overlay lost `{held}`"));
        }
    }
    Ok(())
}

/// A pointer is kept by its agent and its app, every key is made of the act's
/// own agent and app, and nothing counts the pointers to hold their number
/// down — under whatever name.
fn a_pointer_for_each_agent_and_app(overlay: &str) -> Result<(), String> {
    if !overlay.contains("private var slots: [PointerKey: Slot] = [:]") {
        return Err("a pointer is no longer kept by its agent and its app".into());
    }
    let made = overlay.matches("PointerKey(").count();
    let of_the_act = overlay
        .matches("PointerKey(agent: agent ?? unnamedAgent, pid: pid)")
        .count()
        + overlay
            .matches("PointerKey(agent: agent ?? unnamedAgent, pid: arrival.pid)")
            .count();
    if made == 0 || made != of_the_act {
        return Err(format!(
            "a pointer's key is made without its act's agent and app: {made} keys made, {of_the_act} of them from the act"
        ));
    }
    for counts in [
        "maxPointers",
        "slots.count",
        "slots.keys.count",
        "slots.values.count",
    ] {
        if overlay.contains(counts) {
            return Err(format!(
                "a bound on how many pointers are drawn is back (`{counts}`)"
            ));
        }
    }
    Ok(())
}

/// With no bound on their number, what keeps the pointers' windows from
/// piling up is that each closes: when its time is up, when the person stops
/// Computer Use, when it cannot be drawn where it was sent. All three go
/// through `retire`, which takes the windows off the screen, closes them, and
/// only then strikes them from the helper's own list — a window off that list
/// while still on the screen reads as somebody's window over the target, and
/// an act that checks what covers its point is refused for it.
fn a_pointer_that_goes_closes_its_windows(overlay: &str) -> Result<(), String> {
    for road in [
        "MainActor.assumeIsolated { self?.retire(key) }",
        "for key in Array(slots.keys) { retire(key) }",
        "DispatchQueue.main.async { shared.clearAll() }",
    ] {
        if !overlay.contains(road) {
            return Err(format!(
                "a pointer whose time is up, or that the person stopped, no longer goes through retire: `{road}` is gone"
            ));
        }
    }
    let present = body(overlay, "private func present(")?;
    let undrawable = present
        .find("let slot = slots[key] ?? Slot()")
        .map(|end| &present[..end])
        .ok_or("present no longer gives a pointer its slot")?;
    if !undrawable.contains("retire(key)\n            return") {
        return Err("a pointer that cannot be drawn is no longer retired".into());
    }
    let retire = body(overlay, "private func retire(_ key: PointerKey)")?;
    let closed = retire
        .find("panel.orderOut(nil)\n            panel.close()")
        .ok_or("retire no longer takes the windows off the screen and closes them")?;
    let struck = retire
        .find("Self.pointerWindowIDs.subtract(")
        .ok_or("retire no longer strikes the windows from the helper's list")?;
    if closed > struck {
        return Err(
            "a pointer is struck from the helper's list while it is still on the screen".into(),
        );
    }
    Ok(())
}

/// The person's stop chord is heard on the helper's main thread, so the
/// pointers are drawn there one a turn, over a window list read once for
/// several of them, and the drawing asks no app anything through
/// accessibility (t-109788): one agent at work in twenty-four apps was
/// twenty-four drawings in a row, each of which could wait 1.5 s on the app
/// under its point.
fn the_pointers_are_drawn_one_a_turn(overlay: &str) -> Result<(), String> {
    let turn = body(overlay, "private static func drawNext(")?;
    if turn.matches("waiting.next()").count() != 1
        || turn.matches("shared.present(").count() != 1
        || overlay.matches("shared.present(").count() != 1
    {
        return Err(format!(
            "a turn takes more than the one pointer that waited longest, or a pointer is drawn outside its turn:\n{turn}"
        ));
    }
    if let Some(row) = ["for ", "while ", "forEach"]
        .iter()
        .find(|row| turn.contains(**row))
    {
        return Err(format!(
            "a turn draws the pointers that wait in a row (`{row}`):\n{turn}"
        ));
    }
    if !turn.contains("DispatchQueue.main.async { drawNext(") {
        return Err(format!(
            "the rest is no longer left to the main queue's next turns:\n{turn}"
        ));
    }
    if overlay.matches("Self.windows()").count() != 1 || !turn.contains("read ?? Self.windows()") {
        return Err("the window list is read again for every pointer".into());
    }
    for asks in ["DesktopSelf.ownerPid(", "AXUIElement"] {
        if overlay.contains(asks) {
            return Err(format!(
                "drawing a pointer asks an app through accessibility again (`{asks}`)"
            ));
        }
    }
    Ok(())
}

fn holds(rule: Result<(), String>) {
    if let Err(why) = rule {
        panic!("{why}");
    }
}

#[test]
fn a_background_act_does_not_wait_for_its_drawn_pointer() {
    holds(no_act_waits_for_its_pointer(HELPER, OVERLAY));
}

#[test]
fn the_press_ring_waits_for_the_pointer_to_arrive_and_the_press_does_not() {
    holds(the_ring_waits_for_the_pointer(HELPER, OVERLAY));
}

#[test]
fn an_agent_has_a_pointer_for_each_app_it_works_in_and_none_gives_way_to_another() {
    holds(a_pointer_for_each_agent_and_app(OVERLAY));
}

#[test]
fn a_pointer_that_goes_leaves_the_screen_before_it_leaves_the_helpers_list() {
    holds(a_pointer_that_goes_closes_its_windows(OVERLAY));
}

#[test]
fn the_pointers_are_drawn_one_a_turn_and_ask_no_app() {
    holds(the_pointers_are_drawn_one_a_turn(OVERLAY));
}

/// `source` with `was` written over by `now` — a part-way return. The place
/// has to be there: a rule tried on an unchanged text proves nothing.
fn returned(source: &str, was: &str, now: &str) -> String {
    assert!(source.contains(was), "nothing to write over: `{was}`");
    source.replacen(was, now, 1)
}

/// What a return of part of the change looks like, each written into a copy
/// of the sources, and the rule that refuses it.
#[test]
fn a_part_way_return_is_refused() {
    // The bound back under another name.
    let bounded = returned(
        OVERLAY,
        "let slot = slots[key] ?? Slot()",
        "if slots[key] == nil, slots.count >= 8 { return }\n        let slot = slots[key] ?? Slot()",
    );
    assert!(a_pointer_for_each_agent_and_app(&bounded).is_err());
    // The key without its app.
    let one_for_all_apps = returned(
        OVERLAY,
        "let key = PointerKey(agent: agent ?? unnamedAgent, pid: pid)",
        "let key = PointerKey(agent: agent ?? unnamedAgent, pid: 0)",
    );
    assert!(a_pointer_for_each_agent_and_app(&one_for_all_apps).is_err());
    // The pointer whose time is up hidden again, its windows kept.
    let hidden = returned(
        OVERLAY,
        "MainActor.assumeIsolated { self?.retire(key) }",
        "MainActor.assumeIsolated { self?.slots[key]?.panel?.orderOut(nil) }",
    );
    assert!(a_pointer_that_goes_closes_its_windows(&hidden).is_err());
    let kept = returned(
        OVERLAY,
        "panel.orderOut(nil)\n            panel.close()",
        "panel.orderOut(nil)",
    );
    assert!(a_pointer_that_goes_closes_its_windows(&kept).is_err());
    // The wait back outside the helper's travel: in the overlay's, and in the
    // press itself.
    let overlay_waits = returned(
        OVERLAY,
        "return PointerArrival(point: point, pid: pid, setOffNs: setOffNs, moveSeconds: seconds)",
        "Thread.sleep(forTimeInterval: seconds)\n        return PointerArrival(point: point, pid: pid, setOffNs: setOffNs, moveSeconds: seconds)",
    );
    assert!(no_act_waits_for_its_pointer(HELPER, &overlay_waits).is_err());
    let press_waits = returned(
        HELPER,
        "let shown = try travel(to: point, in: snapshot)",
        "let shown = try travel(to: point, in: snapshot)\n            usleep(240_000)",
    );
    assert!(no_act_waits_for_its_pointer(&press_waits, OVERLAY).is_err());
    // The pointers that wait drawn in a row again, and a drawing that asks
    // the app under its point.
    let in_a_row = returned(
        OVERLAY,
        "shared.present(next.place.anchor, key: next.key, seconds: next.place.seconds, over: list)",
        "shared.present(next.place.anchor, key: next.key, seconds: next.place.seconds, over: list)\n        while let next = waiting.next() { shared.present(next.place.anchor, key: next.key, seconds: next.place.seconds, over: list) }",
    );
    assert!(the_pointers_are_drawn_one_a_turn(&in_a_row).is_err());
    let asks = returned(
        OVERLAY,
        "let inView = anchor.visible(in: windows, ignoringWindowIDs: Self.windowIDs())",
        "let inView = DesktopSelf.ownerPid(at: anchor.point) == anchor.pid\n            && anchor.visible(in: windows, ignoringWindowIDs: Self.windowIDs())",
    );
    assert!(the_pointers_are_drawn_one_a_turn(&asks).is_err());
    let reads_each_time = returned(
        OVERLAY,
        "let inView = anchor.visible(in: windows, ignoringWindowIDs: Self.windowIDs())",
        "let inView = anchor.visible(in: Self.windows(), ignoringWindowIDs: Self.windowIDs())",
    );
    assert!(the_pointers_are_drawn_one_a_turn(&reads_each_time).is_err());
}
