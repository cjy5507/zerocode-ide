use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::*;
use crate::computer_use::{
    MARK_CAP, MARK_DIGIT_GLYPHS, MARK_FILL_RGBA, MARK_INK_RGBA, MARK_PIN_TOLERANCE_POINTS,
};

// ------------------------------------------------------------- fixtures --

fn face(index: usize, role: &str, rect: (f64, f64, f64, f64)) -> ElementFace {
    ElementFace {
        index,
        role: role.to_string(),
        name: None,
        placeholder: None,
        traits: Vec::new(),
        actions: Vec::new(),
        x: rect.0,
        y: rect.1,
        width: rect.2,
        height: rect.3,
        // What a real signature holds: the role and the words, never the
        // index — look-alikes share one.
        signature: role.to_string(),
        visible: None,
        context: None,
    }
}

fn named(mut face: ElementFace, name: &str) -> ElementFace {
    face.signature = format!("{}\u{1f}{name}", face.role);
    face.name = Some(name.to_string());
    face
}

/// A face inside a named element: the row a star sits in.
fn within(mut face: ElementFace, context: &str) -> ElementFace {
    face.context = Some(context.to_string());
    face
}

/// A fixture: faces in one window, at one place on the screen.
struct Fixture {
    name: &'static str,
    window: Rect,
    faces: Vec<ElementFace>,
}

/// Two frames every fixture is planned under: the window's own picture on a
/// Retina capture (1.6 px a point, at the window), and the desktop look on
/// this Mac (1280 px for 1512 points, at the screen's corner).
fn frames(window: Rect) -> [(&'static str, ShotFrame, (u32, u32)); 2] {
    let app = ShotFrame::new((window.x, window.y), 1.6).unwrap();
    let app_px = (
        (window.width * 1.6).ceil() as u32,
        (window.height * 1.6).ceil() as u32,
    );
    let desktop = ShotFrame::new((0.0, 0.0), 1280.0 / 1512.0).unwrap();
    [("app", app, app_px), ("desktop", desktop, (1280, 831))]
}

/// F1: a mail window — toolbar, search, a sidebar, a list whose rows carry a
/// checkbox and a star; and what must not be marked.
fn mail() -> Fixture {
    let mut faces = Vec::new();
    let mut next = 0;
    for (at, word) in ["Get Mail", "Compose", "Reply", "Forward", "Archive"]
        .iter()
        .enumerate()
    {
        next += 1;
        faces.push(named(
            face(
                next,
                "AXButton",
                (20.0 + 36.0 * at as f64, 10.0, 28.0, 24.0),
            ),
            word,
        ));
    }
    next += 1;
    let mut search = face(next, "AXSearchField", (760.0, 11.0, 200.0, 22.0));
    search.placeholder = Some("Search".into());
    faces.push(search);
    for row in 0..6 {
        next += 1;
        faces.push(named(
            face(
                next,
                "AXRow",
                (10.0, 60.0 + 24.0 * f64::from(row), 180.0, 24.0),
            ),
            &format!("Mailbox {row}"),
        ));
    }
    for row in 0..8 {
        let top = 60.0 + 44.0 * f64::from(row);
        next += 1;
        faces.push(named(
            face(next, "AXRow", (200.0, top, 560.0, 44.0)),
            &format!("Message {row}"),
        ));
        next += 1;
        faces.push(within(
            face(next, "AXCheckBox", (208.0, top + 15.0, 14.0, 14.0)),
            &format!("Message {row}"),
        ));
        next += 1;
        faces.push(within(
            named(
                face(next, "AXButton", (730.0, top + 14.0, 16.0, 16.0)),
                "Flag",
            ),
            &format!("Message {row}"),
        ));
    }
    // Not marked: a disabled button, a 1-point splitter, a row below the
    // window, a pressable group over most of the window.
    next += 1;
    let mut disabled = named(face(next, "AXButton", (200.0, 10.0, 28.0, 24.0)), "Junk");
    disabled.traits = vec!["disabled".into()];
    faces.push(disabled);
    next += 1;
    faces.push(face(next, "AXSplitter", (195.0, 60.0, 1.0, 600.0)));
    next += 1;
    faces.push(named(
        face(next, "AXRow", (200.0, 800.0, 560.0, 44.0)),
        "Below",
    ));
    next += 1;
    let mut canvas = face(next, "AXGroup", (0.0, 40.0, 1000.0, 600.0));
    canvas.actions = vec!["AXPress".into()];
    faces.push(canvas);
    Fixture {
        name: "F1 mail",
        window: Rect::new(100.0, 100.0, 1000.0, 700.0),
        faces,
    }
}

/// F2: a form — fields, a popup, checkboxes, radios and two buttons.
fn form() -> Fixture {
    let mut faces = Vec::new();
    for row in 0..5 {
        let top = 20.0 + 36.0 * f64::from(row);
        let mut field = face(faces.len() + 1, "AXTextField", (140.0, top, 240.0, 22.0));
        field.placeholder = Some(format!("Field {row}"));
        faces.push(field);
    }
    faces.push(named(
        face(
            faces.len() + 1,
            "AXPopUpButton",
            (140.0, 200.0, 160.0, 22.0),
        ),
        "Country",
    ));
    for row in 0..3 {
        faces.push(named(
            face(
                faces.len() + 1,
                "AXCheckBox",
                (140.0, 240.0 + 24.0 * f64::from(row), 14.0, 14.0),
            ),
            &format!("Agree {row}"),
        ));
        faces.push(named(
            face(
                faces.len() + 1,
                "AXRadioButton",
                (300.0, 240.0 + 24.0 * f64::from(row), 14.0, 14.0),
            ),
            &format!("Plan {row}"),
        ));
    }
    faces.push(named(
        face(faces.len() + 1, "AXButton", (220.0, 330.0, 80.0, 24.0)),
        "Cancel",
    ));
    faces.push(named(
        face(faces.len() + 1, "AXButton", (310.0, 330.0, 80.0, 24.0)),
        "Submit",
    ));
    Fixture {
        name: "F2 form",
        window: Rect::new(300.0, 200.0, 420.0, 380.0),
        faces,
    }
}

/// F3: a keypad — 5 rows of 4 buttons, 40 points each, 1 point apart.
fn keypad() -> Fixture {
    let mut faces = Vec::new();
    for row in 0..5 {
        for column in 0..4 {
            let (x, y) = (
                10.0 + 41.0 * f64::from(column),
                60.0 + 41.0 * f64::from(row),
            );
            faces.push(named(
                face(faces.len() + 1, "AXButton", (x, y, 40.0, 40.0)),
                &format!("{}", faces.len()),
            ));
        }
    }
    Fixture {
        name: "F3 keypad",
        window: Rect::new(400.0, 300.0, 184.0, 270.0),
        faces,
    }
}

/// F4: a 14x10 grid of 20-point buttons, 2 points apart.
fn grid() -> Fixture {
    let mut faces = Vec::new();
    for row in 0..10 {
        for column in 0..14 {
            let (x, y) = (
                10.0 + 22.0 * f64::from(column),
                40.0 + 22.0 * f64::from(row),
            );
            faces.push(named(
                face(faces.len() + 1, "AXButton", (x, y, 20.0, 20.0)),
                &format!("cell {row},{column}"),
            ));
        }
    }
    Fixture {
        name: "F4 grid",
        window: Rect::new(200.0, 100.0, 330.0, 270.0),
        faces,
    }
}

/// F5: a page of 300 links, one per 18-point line.
fn links() -> Fixture {
    let faces = (0..300)
        .map(|line| {
            let mut link = face(
                line + 1,
                "AXLink",
                (40.0, 10.0 + 18.0 * line as f64, 240.0, 16.0),
            );
            link.name = Some(format!("[Story {line}](https://example.com/{line})"));
            link
        })
        .collect();
    Fixture {
        name: "F5 links",
        window: Rect::new(0.0, 0.0, 1512.0, 5500.0),
        faces,
    }
}

fn planned(fixture: &Fixture, frame: ShotFrame, picture: (u32, u32)) -> MarkPlan {
    plan(&MarkInput {
        faces: &fixture.faces,
        window: fixture.window,
        frame,
        picture,
        occluders: Vec::new(),
    })
}

fn every_fixture() -> Vec<Fixture> {
    vec![mail(), form(), keypad(), grid(), links()]
}

// --------------------------------------------------------------- tables --

fn luminance(rgba: [u8; 4]) -> f64 {
    let channel = |value: u8| {
        let value = f64::from(value) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgba[0]) + 0.7152 * channel(rgba[1]) + 0.0722 * channel(rgba[2])
}

fn contrast(a: [u8; 4], b: [u8; 4]) -> f64 {
    let (light, dark) = {
        let (x, y) = (luminance(a), luminance(b));
        if x > y { (x, y) } else { (y, x) }
    };
    (light + 0.05) / (dark + 0.05)
}

#[test]
fn the_marks_tables_draw_a_readable_two_digit_badge() {
    assert_eq!(badge_size(9), (14, 18));
    assert_eq!(badge_size(99), (26, 18));
    assert_eq!(
        badge_size(MARK_CAP),
        (26, 18),
        "no badge wider than two digits"
    );
    assert!(contrast(MARK_INK_RGBA, MARK_FILL_RGBA) >= 7.0);
    for background in [[255, 255, 255, 255], [0, 0, 0, 255]] {
        let best = contrast(MARK_FILL_RGBA, background).max(contrast(MARK_INK_RGBA, background));
        assert!(best >= 3.0, "{background:?}: {best}");
    }
    let mut closest = u32::MAX;
    for (a, left) in MARK_DIGIT_GLYPHS.iter().enumerate() {
        for right in &MARK_DIGIT_GLYPHS[a + 1..] {
            let differ: u32 = left
                .iter()
                .zip(right)
                .map(|(x, y)| (x ^ y).count_ones())
                .sum();
            closest = closest.min(differ);
        }
    }
    assert!(closest >= 3, "two digits differ in {closest} pixels");
}

// ------------------------------------------------------------ selection --

#[test]
fn only_actionable_visible_enabled_controls_get_marks() {
    let fixture = mail();
    for (space, frame, picture) in frames(fixture.window) {
        let plan = planned(&fixture, frame, picture);
        let marked: Vec<&str> = plan
            .marks
            .iter()
            .filter_map(|mark| {
                fixture
                    .faces
                    .iter()
                    .find(|face| face.index == mark.element_index)
            })
            .map(|face| face.name.as_deref().unwrap_or(face.role.as_str()))
            .collect();
        for word in [
            "Compose",
            "Mailbox 0",
            "Message 7",
            "Flag",
            "AXCheckBox",
            "AXSearchField",
        ] {
            assert!(
                marked.contains(&word),
                "{space}: {word} is marked: {marked:?}"
            );
        }
        for word in ["Junk", "Below", "AXSplitter", "AXGroup"] {
            assert!(!marked.contains(&word), "{space}: {word} is not marked");
        }
    }
}

#[test]
fn one_target_one_mark_the_field_over_its_cell() {
    let mut link = named(
        face(1, "AXLink", (10.0, 10.0, 100.0, 20.0)),
        "[Home](https://a)",
    );
    link.actions = vec!["AXPress".into()];
    let mut image = face(2, "AXImage", (10.0, 10.0, 100.0, 21.0));
    image.actions = vec!["AXPress".into()];
    let field = face(3, "AXTextField", (10.0, 60.0, 200.0, 22.0));
    let cell = face(4, "AXCell", (10.0, 60.0, 200.0, 23.0));
    let row = named(face(5, "AXRow", (10.0, 100.0, 400.0, 30.0)), "a row");
    let star = named(face(6, "AXButton", (380.0, 107.0, 16.0, 16.0)), "Flag");
    let faces = [cell, image, link, field, row, star];
    let plan = plan(&MarkInput {
        faces: &faces,
        window: Rect::new(0.0, 0.0, 500.0, 200.0),
        frame: ShotFrame::UNIT,
        picture: (500, 200),
        occluders: Vec::new(),
    });
    let kept: Vec<usize> = plan.marks.iter().map(|mark| mark.element_index).collect();
    assert!(
        kept.contains(&1) && !kept.contains(&2),
        "the link, not the image it fills: {kept:?}"
    );
    assert!(
        kept.contains(&3) && !kept.contains(&4),
        "the field, not its cell: {kept:?}"
    );
    assert!(
        kept.contains(&5) && kept.contains(&6),
        "a star inside a row keeps both: {kept:?}"
    );
    let home = plan
        .marks
        .iter()
        .find(|mark| mark.element_index == 1)
        .unwrap();
    assert_eq!(
        home.label.as_deref(),
        Some("Home"),
        "a link's words, not its markdown"
    );
}

#[test]
fn a_text_area_that_fills_its_window_is_still_where_the_typing_goes() {
    let faces = [
        face(1, "AXTextArea", (0.0, 30.0, 800.0, 560.0)),
        face(2, "AXScrollArea", (0.0, 30.0, 800.0, 560.0)),
    ];
    let plan = plan(&MarkInput {
        faces: &faces,
        window: Rect::new(0.0, 0.0, 800.0, 600.0),
        frame: ShotFrame::UNIT,
        picture: (800, 600),
        occluders: Vec::new(),
    });
    assert_eq!(
        plan.marks
            .iter()
            .map(|mark| mark.element_index)
            .collect::<Vec<_>>(),
        vec![1]
    );
}

#[test]
fn marks_are_numbered_in_reading_order_from_one() {
    let fixture = form();
    for (space, frame, picture) in frames(fixture.window) {
        let plan = planned(&fixture, frame, picture);
        let numbers: Vec<usize> = plan.marks.iter().map(|mark| mark.mark).collect();
        assert_eq!(
            numbers,
            (1..=plan.marks.len()).collect::<Vec<_>>(),
            "{space}"
        );
        // The three checkboxes and radios share lines: left before right.
        let checkbox = plan
            .marks
            .iter()
            .position(|mark| mark.label.as_deref() == Some("Agree 0"))
            .unwrap();
        assert_eq!(
            plan.marks[checkbox + 1].label.as_deref(),
            Some("Plan 0"),
            "{space}"
        );
        assert_eq!(
            plan,
            planned(&fixture, frame, picture),
            "the same input, the same numbers"
        );
    }
}

// ------------------------------------------------------------ placement --

#[test]
fn no_badge_overlaps_leaves_the_picture_or_sits_on_another_control() {
    for fixture in every_fixture() {
        for (space, frame, picture) in frames(fixture.window) {
            let plan = planned(&fixture, frame, picture);
            let bounds = Rect::new(0.0, 0.0, f64::from(picture.0), f64::from(picture.1));
            let elements: Vec<Rect> = plan.marks.iter().map(|mark| mark.element_px).collect();
            for (at, mark) in plan.marks.iter().enumerate() {
                assert!(
                    bounds.contains_rect(&mark.badge_px),
                    "{} {space}: mark {} leaves the picture",
                    fixture.name,
                    mark.mark
                );
                for other in &plan.marks[at + 1..] {
                    assert!(
                        !mark.badge_px.intersects(&other.badge_px),
                        "{} {space}: {} and {} overlap",
                        fixture.name,
                        mark.mark,
                        other.mark
                    );
                }
                for (index, element) in elements.iter().enumerate() {
                    if index != at && element.intersects(&mark.badge_px) {
                        assert!(
                            element.contains_rect(&mark.element_px),
                            "{} {space}: mark {} sits on another control",
                            fixture.name,
                            mark.mark
                        );
                    }
                }
                assert!(
                    mark.badge_px.intersection(&mark.element_px).is_some(),
                    "{} {space}: mark {} does not touch its control",
                    fixture.name,
                    mark.mark
                );
            }
            println!(
                "{} {space}: candidates {} placed {} omitted {}",
                fixture.name,
                plan.candidates,
                plan.marks.len(),
                plan.omitted
            );
        }
    }
}

#[test]
fn the_coverage_holds_where_a_person_would_count_on_it() {
    for (fixture, floor) in [(form(), 1.0), (keypad(), 1.0), (mail(), 0.9)] {
        for (space, frame, picture) in frames(fixture.window) {
            let plan = planned(&fixture, frame, picture);
            let share = plan.marks.len() as f64 / plan.candidates as f64;
            assert!(
                share >= floor,
                "{} {space}: {}/{} placed",
                fixture.name,
                plan.marks.len(),
                plan.candidates
            );
        }
    }
}

#[test]
fn the_cap_keeps_two_digits_and_says_what_it_left() {
    let fixture = links();
    for (space, frame, picture) in frames(fixture.window) {
        let plan = planned(&fixture, frame, picture);
        assert!(plan.marks.len() <= MARK_CAP, "{space}");
        assert_eq!(plan.omitted, plan.candidates - plan.marks.len(), "{space}");
        assert!(
            plan.marks.iter().all(|mark| mark.badge_px.width <= 26.0),
            "{space}"
        );
    }
    // The whole page in one picture (a window's own capture): the cap, not
    // the picture, is what stops the numbers.
    let (_, whole, picture) = frames(fixture.window)[0];
    let plan = planned(&fixture, whole, picture);
    assert_eq!(plan.marks.len(), MARK_CAP, "a long page fills the cap");
    assert_eq!(plan.omitted, 300 - MARK_CAP);
}

#[test]
fn a_small_control_wears_its_badge_outside_a_row_inside() {
    let faces = [
        named(face(1, "AXButton", (100.0, 100.0, 20.0, 20.0)), "Tiny"),
        named(face(2, "AXRow", (100.0, 200.0, 600.0, 24.0)), "a row"),
        named(
            face(3, "AXRow", (100.0, 300.0, 600.0, 24.0)),
            "a checked row",
        ),
        face(4, "AXCheckBox", (104.0, 305.0, 14.0, 14.0)),
    ];
    let plan = plan(&MarkInput {
        faces: &faces,
        window: Rect::new(0.0, 0.0, 900.0, 500.0),
        frame: ShotFrame::new((0.0, 0.0), 0.85).unwrap(),
        picture: (765, 425),
        occluders: Vec::new(),
    });
    let at = |index: usize| {
        plan.marks
            .iter()
            .find(|mark| mark.element_index == index)
            .unwrap()
    };
    let tiny = at(1);
    assert_eq!(
        tiny.badge_px,
        MarkAnchor::OutsideAboveLeft.place(&tiny.element_px, (14.0, 18.0))
    );
    let row = at(2);
    assert_eq!(
        row.badge_px,
        MarkAnchor::InsideTopLeft.place(&row.element_px, (14.0, 18.0))
    );
    let checked = at(3);
    let size = (f64::from(badge_size(checked.mark).0), 18.0);
    assert_eq!(
        checked.badge_px,
        MarkAnchor::InsideTopRight.place(&checked.element_px, size),
        "the checkbox has the corner"
    );
}

#[test]
fn a_window_in_front_hides_the_marks_under_it() {
    let row = |id: u64, pid: i64, rect: (f64, f64, f64, f64), own: bool| DesktopWindow {
        id,
        pid,
        app: format!("App {pid}"),
        rect: Rect::new(rect.0, rect.1, rect.2, rect.3),
        own,
        layer: 0,
        alpha: 1.0,
        overlay: false,
    };
    let picture = Rect::new(0.0, 0.0, 1512.0, 982.0);
    let windows = [
        row(1, 10, (0.0, 0.0, 800.0, 900.0), true),
        row(2, 20, (2000.0, 0.0, 400.0, 400.0), false),
        row(3, 30, (100.0, 100.0, 900.0, 700.0), false),
    ];
    assert_eq!(
        desktop_target(&windows, picture),
        Some(2),
        "not ZeroCode's own, not off the picture"
    );
    assert_eq!(desktop_target(&windows[..2], picture), None);
    let parsed = DesktopWindow::from_row(&json!({
        "id": 3, "title": "Inbox", "app": { "name": "Mail", "pid": 30 },
        "x": 100, "y": 100, "width": 900, "height": 700, "own": false
    }))
    .unwrap();
    assert_eq!(
        parsed,
        DesktopWindow {
            id: 3,
            pid: 30,
            app: "Mail".into(),
            rect: windows[2].rect,
            own: false,
            layer: 0,
            alpha: 1.0,
            overlay: false,
        }
    );
    let faces = [
        named(face(1, "AXButton", (20.0, 20.0, 60.0, 24.0)), "Under"),
        named(face(2, "AXButton", (800.0, 20.0, 60.0, 24.0)), "Clear"),
    ];
    let plan = plan(&MarkInput {
        faces: &faces,
        window: windows[2].rect,
        frame: ShotFrame::UNIT,
        picture: (1512, 982),
        occluders: vec![windows[0].rect],
    });
    assert_eq!(
        plan.marks
            .iter()
            .map(|mark| mark.element_index)
            .collect::<Vec<_>>(),
        vec![2],
        "the one under ZeroCode's window is hidden"
    );
}

// ------------------------------------------------------------------ pin --

/// The pin matrix — `MarkPinTests.swift` holds the same cases, value for value.
#[test]
fn a_pin_holds_across_a_window_move_and_breaks_on_a_shifted_row() {
    let pin = Pin {
        signature: "AXRow\u{1f}row".into(),
        name: "Message 3".into(),
        context: "Inbox".into(),
        frame: Rect::new(200.0, 192.0, 560.0, 44.0),
        tolerance: MARK_PIN_TOLERANCE_POINTS,
    };
    let sig = Some("AXRow\u{1f}row");
    let inbox = Some("Inbox");
    let same = Some(pin.frame);
    assert!(pin.holds(sig, Some("Message 3"), inbox, same), "identical");
    for (dx, dy, dw, dh) in [
        (2.0, 0.0, 0.0, 0.0),
        (0.0, -2.0, 0.0, 0.0),
        (0.0, 0.0, 2.0, -2.0),
    ] {
        let frame = Rect::new(200.0 + dx, 192.0 + dy, 560.0 + dw, 44.0 + dh);
        assert!(
            pin.holds(sig, Some("Message 3"), inbox, Some(frame)),
            "within the tolerance"
        );
    }
    assert!(
        !pin.holds(
            sig,
            Some("Message 3"),
            inbox,
            Some(Rect::new(200.0, 212.0, 560.0, 44.0))
        ),
        "a 20-point shift"
    );
    assert!(
        !pin.holds(Some("AXRow\u{1f}other"), Some("Message 3"), inbox, same),
        "another signature"
    );
    assert!(
        !pin.holds(sig, Some("Message 4"), inbox, same),
        "the row's text changed under the same frame"
    );
    assert!(
        !pin.holds(sig, Some("Message 3"), Some("Archive"), same),
        "another place in the tree under the same frame"
    );
    assert!(!pin.holds(sig, Some("Message 3"), inbox, None), "no frame");
    assert!(
        !pin.holds(
            sig,
            Some("Message 3"),
            inbox,
            Some(Rect::new(202.01, 192.0, 560.0, 44.0))
        ),
        "+2.01"
    );
    let nameless = Pin {
        name: String::new(),
        context: String::new(),
        ..pin.clone()
    };
    assert!(
        nameless.holds(sig, None, None, same),
        "no words and no context are the empty word"
    );
}

/// A window that moved between the look and the click leaves every mark's
/// window-local frame — the one its pin holds — where it was.
#[test]
fn a_mark_is_pinned_where_it_sits_in_its_window_not_on_the_screen() {
    let fixture = form();
    let at = |x: f64, y: f64| {
        plan(&MarkInput {
            faces: &fixture.faces,
            window: Rect::new(x, y, fixture.window.width, fixture.window.height),
            frame: ShotFrame::UNIT,
            picture: (1512, 982),
            occluders: Vec::new(),
        })
    };
    let (before, after) = (at(100.0, 100.0), at(460.0, 300.0));
    assert_eq!(before.marks.len(), after.marks.len());
    for (then, now) in before.marks.iter().zip(&after.marks) {
        assert_eq!(
            (then.local, &then.signature, &then.name, &then.context),
            (now.local, &now.signature, &now.name, &now.context)
        );
        assert_ne!(
            then.screen, now.screen,
            "the badge follows the window on the screen"
        );
    }
}

/// A control scrolled out of its container, or cut off at its centre, gets
/// no number: its press would land on whatever covers it.
#[test]
fn a_control_cut_off_at_its_centre_is_not_marked() {
    let clipped = |index: usize, visible: (f64, f64, f64, f64)| {
        let mut row = named(
            face(index, "AXRow", (10.0, 100.0, 400.0, 40.0)),
            &format!("Invoice {index}"),
        );
        row.visible = Some(FaceFrame {
            x: visible.0,
            y: visible.1,
            width: visible.2,
            height: visible.3,
        });
        row
    };
    let faces = [
        clipped(1, (10.0, 100.0, 400.0, 40.0)),
        clipped(2, (10.0, 130.0, 400.0, 10.0)),
        clipped(3, (10.0, 100.0, 0.0, 0.0)),
        named(face(4, "AXRow", (10.0, 300.0, 400.0, 40.0)), "Invoice 4"),
    ];
    let plan = plan(&MarkInput {
        faces: &faces,
        window: Rect::new(0.0, 0.0, 600.0, 600.0),
        frame: ShotFrame::UNIT,
        picture: (600, 600),
        occluders: Vec::new(),
    });
    let marked: Vec<usize> = plan.marks.iter().map(|mark| mark.element_index).collect();
    assert_eq!(
        marked,
        vec![1, 4],
        "whole, or never clipped: marked; cut at the centre or cut off: not"
    );
}

/// Tiles no pin could tell apart — the same signature, no words, no named
/// row above them — get no numbers; the same tiles inside named rows do.
#[test]
fn look_alikes_a_pin_cannot_tell_apart_are_not_marked() {
    let tile = |index: usize, x: f64| {
        let mut tile = face(index, "AXGroup", (x, 40.0, 60.0, 60.0));
        tile.actions = vec!["AXPress".into()];
        tile
    };
    let loose: Vec<ElementFace> = (0..4)
        .map(|at| tile(at + 1, 10.0 + 70.0 * at as f64))
        .collect();
    let input = |faces: &[ElementFace]| {
        plan(&MarkInput {
            faces,
            window: Rect::new(0.0, 0.0, 600.0, 400.0),
            frame: ShotFrame::UNIT,
            picture: (600, 400),
            occluders: Vec::new(),
        })
        .marks
        .len()
    };
    assert_eq!(
        input(&loose),
        0,
        "one slot's shift would put another tile under the number"
    );
    let placed: Vec<ElementFace> = loose
        .iter()
        .enumerate()
        .map(|(at, face)| within(face.clone(), &format!("Album {at}")))
        .collect();
    assert_eq!(input(&placed), 4, "each tile is known by its album");
    let mut twin = loose[..1].to_vec();
    let mut offscreen = tile(9, 10.0);
    offscreen.y = 900.0;
    twin.push(offscreen);
    assert_eq!(
        input(&twin),
        0,
        "a twin scrolled out of sight is still a twin"
    );
}

/// Menus, panels and the Dock cover what is under them; an invisible
/// window covers nothing; only a document window is a desktop look's target.
#[test]
fn every_layer_in_front_covers_and_only_a_document_window_is_marked() {
    let row = |id: u64, layer: i64, alpha: f64, rect: (f64, f64, f64, f64)| DesktopWindow {
        id,
        pid: 10 + i64::try_from(id).unwrap(),
        app: format!("App {id}"),
        rect: Rect::new(rect.0, rect.1, rect.2, rect.3),
        own: false,
        layer,
        alpha,
        overlay: false,
    };
    let windows = [
        row(1, 101, 1.0, (100.0, 100.0, 200.0, 300.0)),
        row(2, 25, 0.0, (0.0, 0.0, 1512.0, 982.0)),
        row(3, 0, 1.0, (50.0, 50.0, 900.0, 700.0)),
    ];
    let picture = Rect::new(0.0, 0.0, 1512.0, 982.0);
    assert_eq!(
        desktop_target(&windows, picture),
        Some(2),
        "the menu and the overlay are no targets"
    );
    assert_eq!(
        occluders(&windows, 2),
        vec![windows[0].rect],
        "the invisible overlay covers nothing"
    );
    let parsed = DesktopWindow::from_row(&json!({
        "id": 7, "app": { "name": "Menu", "pid": 1 }, "x": 0, "y": 0, "width": 10, "height": 10,
        "layer": 101, "alpha": 0.5
    }))
    .unwrap();
    assert_eq!((parsed.layer, parsed.alpha), (101, 0.5));
    assert!(!parsed.overlay, "a row that does not say so is no overlay");
}

/// The Dock's window on macOS 26 spans the whole screen at the Dock's layer,
/// fully opaque by the window list's word, and is clear everywhere but the
/// Dock (this Mac, 2026-09-12: `Dock` #11, layer 20, 1512 x 982, alpha 1).
/// The helper calls it an overlay; an overlay hides nothing, so the windows
/// under it are still on the picture and a desktop look still has a target.
#[test]
fn an_overlay_the_size_of_the_screen_hides_nothing() {
    let row = |id: u64, layer: i64, rect: (f64, f64, f64, f64), overlay: bool| DesktopWindow {
        id,
        pid: 10 + i64::try_from(id).unwrap(),
        app: format!("App {id}"),
        rect: Rect::new(rect.0, rect.1, rect.2, rect.3),
        own: false,
        layer,
        alpha: 1.0,
        overlay,
    };
    let windows = [
        row(1, 24, (0.0, 0.0, 1512.0, 33.0), false),
        row(2, 20, (0.0, 0.0, 1512.0, 982.0), true),
        row(3, 0, (0.0, 33.0, 920.0, 600.0), false),
    ];
    let picture = Rect::new(0.0, 0.0, 1512.0, 982.0);
    assert_eq!(
        desktop_target(&windows, picture),
        Some(2),
        "the document window under the Dock's overlay is marked"
    );
    assert_eq!(
        occluders(&windows, 2),
        vec![windows[0].rect],
        "the menu bar covers; the overlay does not"
    );
    let mut opaque = windows.clone();
    opaque[1].overlay = false;
    assert_eq!(
        desktop_target(&opaque, picture),
        None,
        "taken for a window, it would hide everything under it"
    );
    let parsed = DesktopWindow::from_row(&json!({
        "id": 11, "app": { "name": "Dock", "pid": 9 }, "x": 0, "y": 0, "width": 1512, "height": 982,
        "layer": 20, "alpha": 1, "overlay": true
    }))
    .unwrap();
    assert!(parsed.overlay && !parsed.covers());
}

/// A window hidden whole behind the ones in front of it — ZeroCode's own
/// filling the screen, or two windows side by side over it — is not the
/// desktop look's target: nothing of it is on the picture to number.
#[test]
fn a_window_hidden_whole_is_not_walked() {
    let row = |id: u64, own: bool, rect: (f64, f64, f64, f64)| DesktopWindow {
        id,
        pid: i64::try_from(id).unwrap(),
        app: format!("App {id}"),
        rect: Rect::new(rect.0, rect.1, rect.2, rect.3),
        own,
        layer: 0,
        alpha: 1.0,
        overlay: false,
    };
    let picture = Rect::new(0.0, 0.0, 1512.0, 982.0);
    let screen = row(1, true, (0.0, 0.0, 1512.0, 884.0));
    let behind = row(2, false, (0.0, 0.0, 1512.0, 875.0));
    assert_eq!(
        desktop_target(&[screen.clone(), behind.clone()], picture),
        None,
        "ZeroCode fills the screen"
    );
    let peeking = row(3, false, (0.0, 0.0, 1512.0, 900.0));
    assert_eq!(
        desktop_target(&[screen.clone(), peeking], picture),
        Some(1),
        "16 points of it show below ZeroCode"
    );
    let (left, right) = (
        row(4, false, (0.0, 0.0, 760.0, 900.0)),
        row(5, false, (760.0, 0.0, 752.0, 900.0)),
    );
    assert_eq!(
        desktop_target(
            &[
                left.clone(),
                right.clone(),
                row(6, false, (100.0, 100.0, 1200.0, 700.0))
            ],
            picture
        ),
        Some(0),
        "the front window is the target; the one under both halves is hidden whole"
    );
    assert!(fully_covered(
        Rect::new(100.0, 100.0, 1200.0, 700.0),
        &[left.rect, right.rect]
    ));
    assert!(!fully_covered(
        Rect::new(100.0, 100.0, 1200.0, 700.0),
        &[left.rect]
    ));
    assert!(fully_covered(
        Rect::new(10.0, 10.0, 5.0, 5.0),
        &[Rect::new(0.0, 0.0, 100.0, 100.0)]
    ));
    assert!(!fully_covered(Rect::new(10.0, 10.0, 5.0, 5.0), &[]));
}

/// The windows from the front to the target must be the same, in the same
/// order and places, before the picture and after the walk.
#[test]
fn a_desktop_look_is_marked_only_when_its_windows_stood_still() {
    let row = |id: u64, x: f64| DesktopWindow {
        id,
        pid: 1,
        app: "App".into(),
        rect: Rect::new(x, 100.0, 400.0, 300.0),
        own: false,
        layer: 0,
        alpha: 1.0,
        overlay: false,
    };
    let before = [row(1, 0.0), row(2, 100.0), row(3, 500.0)];
    assert!(stood_still(&before, &before, 2, 2.0));
    assert!(
        stood_still(
            &before,
            &[row(1, 0.0), row(2, 101.5), row(4, 900.0)],
            2,
            2.0
        ),
        "behind it does not matter"
    );
    assert!(
        !stood_still(&before, &[row(1, 0.0), row(2, 300.0)], 2, 2.0),
        "the target moved"
    );
    assert!(
        !stood_still(&before, &[row(2, 100.0), row(1, 0.0)], 2, 2.0),
        "the order changed in front of it"
    );
    assert!(
        !stood_still(&before, &[row(1, 0.0), row(5, 0.0), row(2, 100.0)], 2, 2.0),
        "a window came in front"
    );
    assert!(
        !stood_still(&before, &[row(1, 0.0)], 2, 2.0),
        "the target is gone"
    );
}

#[test]
fn a_pin_travels_whole_or_not_at_all() {
    let pin = Pin {
        signature: "sig".into(),
        name: "Save".into(),
        context: "Toolbar".into(),
        frame: Rect::new(1.0, 2.0, 3.0, 4.0),
        tolerance: 2.0,
    };
    let mut params = serde_json::Map::new();
    assert_eq!(Pin::from_params(&params).unwrap(), None);
    pin.write(&mut params);
    assert_eq!(Pin::from_params(&params).unwrap(), Some(pin.clone()));
    let mut partial = params.clone();
    partial.remove(PIN_NAME_KEY);
    assert_eq!(
        Pin::from_params(&partial).unwrap_err().code,
        "invalid_argument"
    );
    for (key, bad) in [
        (PIN_TOLERANCE_KEY, json!(-1.0)),
        (PIN_TOLERANCE_KEY, json!("2")),
        (
            PIN_FRAME_KEY,
            json!({ "x": 1, "y": 2, "width": -3, "height": 4 }),
        ),
        (PIN_FRAME_KEY, json!({ "x": 1, "y": 2, "width": 3 })),
    ] {
        let mut broken = params.clone();
        broken.insert(key.into(), bad.clone());
        assert!(Pin::from_params(&broken).is_err(), "{key}: {bad}");
    }
}

// --------------------------------------------------------------- answer --

#[test]
fn the_answer_speaks_screen_points_and_the_legend_one_line_per_mark() {
    let mut field = face(3, "AXTextField", (30.0, 2.0, 20.0, 20.0));
    field.placeholder = None;
    let faces = [
        named(face(7, "AXButton", (400.0, 76.0, 24.0, 24.0)), "Save"),
        field,
    ];
    let plan = plan(&MarkInput {
        faces: &faces,
        window: Rect::new(0.0, 0.0, 600.0, 200.0),
        frame: ShotFrame::UNIT,
        picture: (600, 200),
        occluders: Vec::new(),
    });
    let window = MarkedWindow {
        look_id: "L1".into(),
        app: "Notes".into(),
        pid: 42,
        window_id: 9,
    };
    let mut answered = answer(&plan, &window);
    assert_eq!(answered[SPACE_KEY], SCREEN_SPACE);
    assert_eq!(answered[LOOK_ID_KEY], "L1");
    let save = answered[ITEMS_KEY]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["elementIndex"] == 7)
        .unwrap()
        .clone();
    assert_eq!(
        (save["centerX"].as_f64(), save["centerY"].as_f64()),
        (Some(412.0), Some(88.0))
    );
    assert_eq!(
        legend_line(&save).as_deref(),
        Some(&*format!("{} button Save @412,88", save["mark"]))
    );
    let nameless = answered[ITEMS_KEY]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["elementIndex"] == 3)
        .unwrap()
        .clone();
    assert_eq!(
        legend_line(&nameless).as_deref(),
        Some(&*format!("{} text field @40,12", nameless["mark"]))
    );
    ShotFrame::new((0.0, 0.0), 0.5)
        .unwrap()
        .pixelize(&mut answered);
    assert_eq!(answered[SPACE_KEY], "shot");
    let halved = answered[ITEMS_KEY]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["elementIndex"] == 7)
        .unwrap();
    assert_eq!(
        (
            halved["x"].as_f64(),
            halved["width"].as_f64(),
            halved["centerX"].as_f64()
        ),
        (Some(200.0), Some(12.0), Some(206.0))
    );
}

#[test]
fn element_faces_carry_what_the_walk_saw() {
    let record = |index: usize, frame: Option<Rect>| RenderedRecord {
        index,
        handle: index,
        local_frame: frame,
        actions: vec!["AXPress".into()],
        signature: format!("sig{index}"),
        role: "AXButton".into(),
        name: Some("OK".into()),
        placeholder: None,
        traits: vec!["selected".into()],
        visible: frame,
        context: Some("Toolbar".into()),
    };
    let records: BTreeMap<usize, RenderedRecord<usize>> = [
        (0, record(0, Some(Rect::new(1.0, 2.0, 30.0, 20.0)))),
        (1, record(1, None)),
        (2, record(2, Some(Rect::new(1.0, 2.0, 0.0, 20.0)))),
    ]
    .into_iter()
    .collect();
    let faces = element_faces(&records);
    assert_eq!(faces.len(), 1, "no frame, or no area, is no face");
    assert_eq!(
        (
            faces[0].signature.as_str(),
            faces[0].name.as_deref(),
            faces[0].actions.len()
        ),
        ("sig0", Some("OK"), 1)
    );
    let wire: Value = serde_json::to_value(&faces[0]).unwrap();
    assert_eq!(wire["index"], 0);
    assert!(wire.get("placeholder").is_none());
    let back: ElementFace = serde_json::from_value(wire).unwrap();
    assert_eq!(back, faces[0]);
}
