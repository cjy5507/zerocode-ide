use serde_json::{Map, Value, json};
use zerocode_core::computer_use::{
    MARK_BADGE_BORDER_PX, MARK_BADGE_PAD_PX, MARK_DIGIT_GLYPHS, MARK_FILL_RGBA, MARK_GLYPH_COLUMNS,
    MARK_GLYPH_GAP_PX, MARK_GLYPH_ROWS, MARK_GLYPH_SCALE, MARK_INK_RGBA, MARK_PIN_TOLERANCE_POINTS,
    MARK_SNAPSHOT_SESSION,
};
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::{
    self as plan, DesktopWindow, ELEMENTS_KEY, ElementFace, MarkInput, PIN_CONTEXT_KEY,
    PIN_FRAME_KEY, PIN_NAME_KEY, PIN_SIGNATURE_KEY, PIN_TOLERANCE_KEY,
};
use zerocode_core::computer_use_protocol::render::Rect;

use super::*;
use crate::computer_use::screenshot_png::RgbaImage;

fn face(index: usize, role: &str, rect: (f64, f64, f64, f64), name: &str) -> ElementFace {
    ElementFace {
        index,
        role: role.into(),
        name: Some(name.into()),
        placeholder: None,
        traits: Vec::new(),
        actions: vec!["AXPress".into()],
        x: rect.0,
        y: rect.1,
        width: rect.2,
        height: rect.3,
        signature: format!("{role}\u{1f}{name}"),
        visible: None,
        context: Some("Keypad".into()),
    }
}

/// A keypad of `count` 40-point buttons, four to a row.
fn keypad(count: usize) -> Vec<ElementFace> {
    (0..count)
        .map(|at| {
            let (column, row) = ((at % 4) as f64, (at / 4) as f64);
            face(
                at + 1,
                "AXButton",
                (10.0 + 44.0 * column, 40.0 + 44.0 * row, 40.0, 40.0),
                &format!("key {at}"),
            )
        })
        .collect()
}

fn picture(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&paint(x, y));
        }
    }
    RgbaImage::new(width, height, pixels).unwrap()
}

fn pixel(image: &RgbaImage, x: i64, y: i64) -> [u8; 4] {
    let at = ((y * i64::from(image.width) + x) * 4) as usize;
    image.pixels[at..at + 4].try_into().unwrap()
}

/// A test's own reader: each glyph cell of a badge, sampled and matched to
/// the digit table — the numbers as a model would see them.
fn read_badge(image: &RgbaImage, badge: &Rect, digits: usize) -> Option<usize> {
    let inset = i64::from(MARK_BADGE_BORDER_PX + MARK_BADGE_PAD_PX);
    let scale = i64::from(MARK_GLYPH_SCALE);
    let step = i64::from(MARK_GLYPH_COLUMNS * MARK_GLYPH_SCALE + MARK_GLYPH_GAP_PX);
    let (x0, y0) = (badge.x.round() as i64, badge.y.round() as i64);
    let mut number = 0;
    for at in 0..digits as i64 {
        let mut rows = [0u8; 7];
        for (row, bits) in rows.iter_mut().enumerate() {
            for column in 0..i64::from(MARK_GLYPH_COLUMNS) {
                let (x, y) = (
                    x0 + inset + at * step + column * scale,
                    y0 + inset + row as i64 * scale,
                );
                if pixel(image, x, y) == MARK_INK_RGBA {
                    *bits |= 1 << (i64::from(MARK_GLYPH_COLUMNS) - 1 - column);
                }
            }
        }
        number = number * 10 + MARK_DIGIT_GLYPHS.iter().position(|glyph| *glyph == rows)?;
    }
    Some(number)
}

#[test]
fn drawn_marks_read_back_as_their_numbers() {
    let faces = keypad(24);
    let window = Rect::new(0.0, 0.0, 200.0, 320.0);
    type Paint = Box<dyn Fn(u32, u32) -> [u8; 4]>;
    let backgrounds: [(&str, Paint); 3] = [
        ("white", Box::new(|_, _| [255, 255, 255, 255])),
        ("black", Box::new(|_, _| [0, 0, 0, 255])),
        (
            "noise",
            Box::new(|x, y| {
                let mut seed =
                    (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663)).wrapping_add(1);
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let [a, b, c, _] = seed.to_le_bytes();
                [a, b, c, 255]
            }),
        ),
    ];
    for scale in [1.6, 1280.0 / 1512.0] {
        let size = (
            (window.width * scale) as u32,
            (window.height * scale) as u32,
        );
        let planned = plan::plan(&MarkInput {
            faces: &faces,
            window,
            frame: ShotFrame::new((0.0, 0.0), scale).unwrap(),
            picture: size,
            occluders: Vec::new(),
        });
        assert_eq!(planned.marks.len(), 24, "{scale}");
        for (name, paint) in &backgrounds {
            let mut image = picture(size.0, size.1, paint);
            draw(&mut image, &planned.marks);
            for mark in &planned.marks {
                let digits = plan::digits(mark.mark).len();
                assert_eq!(
                    read_badge(&image, &mark.badge_px, digits),
                    Some(mark.mark),
                    "{name} at {scale}"
                );
                let (x0, y0, x1, y1) = edges(&mark.badge_px);
                for y in y0..y1 {
                    for x in x0..x1 {
                        let seen = pixel(&image, x, y);
                        assert!(
                            seen == MARK_FILL_RGBA || seen == MARK_INK_RGBA,
                            "{name}: a badge is fill and ink only"
                        );
                    }
                }
            }
        }
    }
    assert_eq!(MARK_GLYPH_ROWS as usize, MARK_DIGIT_GLYPHS[0].len());
}

fn app_answer(faces: &[ElementFace], app: &str) -> Value {
    json!({
        "snapshot": {
            "id": "S1",
            "app": { "name": app, "bundleId": null, "pid": 4321 },
            "window": { "id": 77, "title": "Keys", "x": 100, "y": 50, "width": 200, "height": 320 },
            "treeText": "App=x",
            ELEMENTS_KEY: faces,
        },
        "screenshot": { "data": "", "width": 320, "height": 512, "scale": 1.6 },
    })
}

fn no_helper() -> impl FnMut(&str, Value) -> Result<Value, ComputerUseError> {
    |method: &str, _params: Value| panic!("no helper call expected, got {method}")
}

fn shot<'a>(
    clean: &'a RgbaImage,
    fingerprint: u64,
    placed: ShotFrame,
    place: &'a str,
) -> Picture<'a> {
    Picture {
        clean,
        fingerprint,
        placed,
        place,
    }
}

#[test]
fn a_mark_click_is_the_pinned_element_of_the_looks_own_window() {
    let faces = keypad(8);
    let answer = app_answer(&faces, "Calculator-click");
    let placed = ShotFrame::new((100.0, 50.0), 1.6).unwrap();
    let clean = picture(320, 512, |_, _| [255, 255, 255, 255]);
    let mut params = Map::new();
    params.insert("app".into(), "Calculator-click".into());
    let marked = mark_look(
        &params,
        &answer,
        &shot(&clean, 1, placed, "place-click"),
        None,
        &mut no_helper(),
    );
    let look = marked.answer["lookId"].as_str().unwrap().to_string();
    assert!(marked.png.is_some(), "the picture carries the badges");
    assert_eq!(marked.answer["items"].as_array().unwrap().len(), 8);
    let third = &marked.answer["items"][2];
    let click = pinned_click(&json!({ "mark": 3, "look": look, "mouseButton": "left", "clickCount": 1,
                                      "modifiers": "shift", "confirming": "delete", "viewer": "zo-1" }))
        .unwrap();
    assert_eq!(click.params["app"], "pid:4321");
    assert_eq!(click.params["windowId"], 77);
    assert_eq!(click.params["elementIndex"], third["elementIndex"]);
    assert_eq!(click.params["session"], MARK_SNAPSHOT_SESSION);
    assert_eq!(click.params["noScreenshot"], true);
    assert_eq!(click.params[PIN_TOLERANCE_KEY], MARK_PIN_TOLERANCE_POINTS);
    assert_eq!(click.params[PIN_SIGNATURE_KEY], "AXButton\u{1f}key 2");
    assert_eq!(click.params[PIN_NAME_KEY], "key 2");
    assert_eq!(click.params[PIN_CONTEXT_KEY], "Keypad");
    assert_eq!(
        click.params[PIN_FRAME_KEY],
        json!({ "x": 98.0, "y": 40.0, "width": 40.0, "height": 40.0 }),
        "the window-local frame"
    );
    for key in ["mouseButton", "clickCount", "modifiers", "confirming"] {
        assert!(click.params.contains_key(key), "{key} is carried");
    }
    assert!(!click.params.contains_key("viewer") && !click.params.contains_key("look"));
    let beyond = pinned_click(&json!({ "mark": 9, "look": look })).unwrap_err();
    assert_eq!(beyond.code, "invalid_argument");
    assert!(beyond.message.contains("1–8"), "{}", beyond.message);
    let unknown = pinned_click(&json!({ "mark": 1, "look": "0.0" })).unwrap_err();
    assert_eq!(unknown.code, "element_not_found");
}

/// A look keeps a table's numbers only for the very pixels they were drawn
/// on: another screen at the same place — another app come to the front —
/// is walked again, never served an old window's badges.
#[test]
fn a_look_keeps_its_numbers_only_for_the_very_same_picture() {
    let faces = keypad(4);
    let answer = app_answer(&faces, "Calculator-same");
    let placed = ShotFrame::new((100.0, 50.0), 1.6).unwrap();
    let clean = picture(320, 512, |_, _| [255, 255, 255, 255]);
    let mut params = Map::new();
    params.insert("app".into(), "Calculator-same".into());
    let first = mark_look(
        &params,
        &answer,
        &shot(&clean, 7, placed, "place-same"),
        None,
        &mut no_helper(),
    );
    let again = mark_look(
        &params,
        &json!({}),
        &shot(&clean, 7, placed, "place-same"),
        None,
        &mut no_helper(),
    );
    assert_eq!(
        again.answer["lookId"], first.answer["lookId"],
        "the same pixels: the same numbers, with no walk"
    );
    assert_eq!(again.answer["sameAsLastLook"], true);
    assert_eq!(again.png, first.png, "drawn again on the new clean frame");
    let changed = mark_look(
        &params,
        &answer,
        &shot(&clean, 8, placed, "place-same"),
        None,
        &mut no_helper(),
    );
    assert_ne!(
        changed.answer["lookId"], first.answer["lookId"],
        "other pixels at the same place: a new walk"
    );
    assert!(changed.answer.get("sameAsLastLook").is_none());
    let other = mark_look(
        &params,
        &answer,
        &shot(&clean, 7, placed, "place-other"),
        None,
        &mut no_helper(),
    );
    assert_ne!(
        other.answer["lookId"], first.answer["lookId"],
        "a table for another place is not this one's"
    );
}

fn list(rows: &[Value]) -> Result<Vec<DesktopWindow>, ComputerUseError> {
    Ok(rows.iter().filter_map(DesktopWindow::from_row).collect())
}

#[test]
fn desktop_marks_skip_zerocodes_own_windows_and_say_why_when_none_is_left() {
    let faces = vec![
        face(1, "AXButton", (20.0, 20.0, 60.0, 24.0), "Under"),
        face(2, "AXButton", (600.0, 20.0, 60.0, 24.0), "Clear"),
        face(3, "AXButton", (600.0, 400.0, 60.0, 24.0), "Menu covers"),
    ];
    let window = json!({ "id": 9, "title": "Mail", "app": { "name": "Mail", "pid": 30 }, "x": 100, "y": 100, "width": 800, "height": 600, "own": false, "layer": 0, "alpha": 1.0 });
    let own = json!({ "id": 1, "title": "ZeroCode", "app": { "name": "ZeroCode", "pid": 10 }, "x": 0, "y": 0, "width": 400, "height": 400, "own": true, "layer": 0, "alpha": 1.0 });
    let menu = json!({ "id": 5, "title": "", "app": { "name": "Mail", "pid": 30 }, "x": 650, "y": 450, "width": 200, "height": 300, "own": false, "layer": 101, "alpha": 1.0 });
    let rows = [menu.clone(), own.clone(), window.clone()];
    let walked = json!({ "snapshot": { "id": "S", "app": { "name": "Mail", "pid": 30 },
        "window": { "id": 9, "x": 100, "y": 100, "width": 800, "height": 600 }, ELEMENTS_KEY: faces } });
    let desk = ShotFrame::new((0.0, 0.0), 1280.0 / 1512.0).unwrap();
    let clean = picture(1280, 831, |_, _| [240, 240, 240, 255]);
    let mut asked = Vec::new();
    let mut call = |method: &str, params: Value| -> Result<Value, ComputerUseError> {
        asked.push((method.to_string(), params.clone()));
        Ok(match method {
            "listAllWindows" => json!({ "windows": rows }),
            "getAppState" => walked.clone(),
            other => panic!("unexpected {other}"),
        })
    };
    let before = desktop_windows(&mut call);
    let marked = mark_look(
        &Map::new(),
        &json!({}),
        &shot(&clean, 1, desk, "desk-own"),
        Some(before),
        &mut call,
    );
    let indexes: Vec<u64> = marked.answer["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["elementIndex"].as_u64().unwrap())
        .collect();
    assert_eq!(
        indexes,
        vec![2],
        "under ZeroCode's window and under the menu: hidden: {}",
        marked.answer
    );
    let lists: Vec<&Value> = asked
        .iter()
        .filter(|(method, _)| method == "listAllWindows")
        .map(|(_, params)| params)
        .collect();
    assert_eq!(lists.len(), 2, "before the picture and after the walk");
    assert!(
        lists.iter().all(|params| params["everyLayer"] == true),
        "every layer covers"
    );
    let (_, walk) = asked
        .iter()
        .find(|(method, _)| method == "getAppState")
        .unwrap();
    assert_eq!(walk["app"], "pid:30");
    assert_eq!(walk["windowId"], 9, "the document window, not its menu");
    assert_eq!(walk["session"], MARK_SNAPSHOT_SESSION);
    assert_eq!(walk["noScreenshot"], true);
    assert_eq!(walk["elementFrames"], true);

    let none = mark_look(
        &Map::new(),
        &json!({}),
        &shot(&clean, 2, desk, "desk-alone"),
        Some(list(std::slice::from_ref(&own))),
        &mut no_helper(),
    );
    assert!(
        none.answer["unavailable"]
            .as_str()
            .unwrap()
            .starts_with("window_not_found"),
        "{}",
        none.answer
    );
    assert!(none.png.is_none());
    // ZeroCode fills the screen and Mail sits whole behind it: nothing of
    // Mail is on the picture, so it is not walked at all.
    let filling = json!({ "id": 1, "title": "ZeroCode", "app": { "name": "ZeroCode", "pid": 10 }, "x": 0, "y": 0, "width": 1512, "height": 982, "own": true, "layer": 0, "alpha": 1.0 });
    let hidden = mark_look(
        &Map::new(),
        &json!({}),
        &shot(&clean, 5, desk, "desk-hidden"),
        Some(list(&[filling, window.clone()])),
        &mut no_helper(),
    );
    assert!(
        hidden.answer["unavailable"]
            .as_str()
            .unwrap()
            .starts_with("window_not_found"),
        "a window hidden whole is not walked: {}",
        hidden.answer
    );
    let unlisted = mark_look(
        &Map::new(),
        &json!({}),
        &shot(&clean, 3, desk, "desk-unlisted"),
        Some(Err(ComputerUseError::new("accessibility_error", "no list"))),
        &mut no_helper(),
    );
    assert_eq!(
        unlisted.answer["unavailable"],
        "accessibility_error: no list"
    );

    let mut refused = |method: &str, _: Value| -> Result<Value, ComputerUseError> {
        match method {
            "listAllWindows" => Ok(json!({ "windows": [window.clone()] })),
            _ => Err(ComputerUseError::new("app_blocked", "not that one")),
        }
    };
    let blocked = mark_look(
        &Map::new(),
        &json!({}),
        &shot(&clean, 4, desk, "desk-blocked"),
        Some(list(std::slice::from_ref(&window))),
        &mut refused,
    );
    assert_eq!(
        blocked.answer["unavailable"], "app_blocked: not that one",
        "a look never fails because of its marks"
    );

    // The window moved after the picture: the list before it and the list
    // after the walk disagree, even though the walk agrees with the second.
    let moved_row = json!({ "id": 9, "title": "Mail", "app": { "name": "Mail", "pid": 30 }, "x": 300, "y": 100, "width": 800, "height": 600, "own": false });
    let moved_walk = json!({ "snapshot": { "id": "S", "app": { "name": "Mail", "pid": 30 },
        "window": { "id": 9, "x": 300, "y": 100, "width": 800, "height": 600 }, ELEMENTS_KEY: [] } });
    let mut raced = |method: &str, _: Value| -> Result<Value, ComputerUseError> {
        Ok(match method {
            "listAllWindows" => json!({ "windows": [moved_row.clone()] }),
            _ => moved_walk.clone(),
        })
    };
    let stale = mark_look(
        &Map::new(),
        &json!({}),
        &shot(&clean, 5, desk, "desk-moved"),
        Some(list(std::slice::from_ref(&window))),
        &mut raced,
    );
    assert!(
        stale.answer["unavailable"]
            .as_str()
            .unwrap()
            .starts_with("window_stale"),
        "{}",
        stale.answer
    );
}

#[test]
fn a_mark_click_answers_what_it_pressed_and_keeps_the_window_for_evidence() {
    let click = PinnedClick {
        params: Map::new(),
        mark: 7,
        look: "1.2".into(),
        role: "button".into(),
        label: Some("Save".into()),
        element_index: 12,
        app: "TextEdit".into(),
        frame: zerocode_core::computer_use_protocol::render::Rect::new(10.0, 20.0, 30.0, 40.0),
    };
    let answered = click_answer(
        json!({ "snapshot": { "window": { "id": 3 }, "treeText": "long" }, "screenshot": { "data": "x" },
                "action": { "path": "accessibility" } }),
        &click,
    );
    assert!(answered["snapshot"].get("treeText").is_none() && answered.get("screenshot").is_none());
    assert_eq!(
        answered.pointer("/snapshot/window/id"),
        Some(&json!(3)),
        "evidence frames by the window"
    );
    assert_eq!(answered["mark"]["label"], "Save");
    assert_eq!(
        answered["mark"]["frame"],
        json!({ "x": 10.0, "y": 20.0, "width": 30.0, "height": 40.0 }),
        "the pressed mark's rectangle rides the answer, for the walk's ring"
    );
    let refused = click_refusal(
        ComputerUseError::new("element_not_found", "element 12 is no longer…"),
        &click,
    );
    assert_eq!(
        refused.message,
        "mark 7 (button Save) moved or changed since look 1.2; look again with --marks"
    );
    let other = click_refusal(
        ComputerUseError::new("stopped", "the operator is stopped"),
        &click,
    );
    assert_eq!(other.code, "stopped", "only the pin's refusal is rewritten");
}

#[test]
fn a_marked_look_keeps_the_clean_frame_for_the_diff() {
    use base64::Engine as _;
    let faces = keypad(4);
    let clean = picture(320, 512, |_, _| [250, 250, 250, 255])
        .encode()
        .unwrap();
    let data = base64::engine::general_purpose::STANDARD.encode(&clean);
    let mut answer = app_answer(&faces, "Calculator-diff");
    answer["screenshot"]["data"] = data.clone().into();
    let mut call = |method: &str, params: Value| -> Result<Value, ComputerUseError> {
        assert_eq!(method, "getAppState");
        assert_eq!(
            params["elementFrames"], true,
            "a marked look asks for the faces"
        );
        Ok(answer.clone())
    };
    let mut params = Map::new();
    params.insert("app".into(), "Calculator-diff".into());
    params.insert("viewer".into(), "marks-diff-test".into());
    params.insert("diff".into(), true.into());
    params.insert("marks".into(), true.into());
    let memory = crate::computer_use::eye::Memory::new();
    let mut pause = |_: std::time::Duration| panic!("a look without --settle does not wait");
    let first = crate::computer_use::observe::observe_with(&params, &memory, &mut call, &mut pause)
        .unwrap();
    assert_ne!(
        first["screenshot"]["data"], data,
        "the agent is shown the marked picture"
    );
    let second =
        crate::computer_use::observe::observe_with(&params, &memory, &mut call, &mut pause)
            .unwrap();
    assert_eq!(
        second["changed"],
        json!([]),
        "the badges are not a change: the diff's baseline is the clean frame"
    );
    assert_eq!(second["marks"]["sameAsLastLook"], true);
    assert_eq!(second["marks"]["lookId"], first["marks"]["lookId"]);
}

/// What the badges cost on a real frame — the work `--no-screenshot` skips.
///
/// Not part of the gate: it needs a frame a real look left on disk, named by
/// `ZEROCODE_MARK_BENCH_PNG`, and how many badges to draw in
/// `ZEROCODE_MARK_BENCH_MARKS`. Run it with `--ignored` beside a capture.
#[test]
#[ignore = "measures a real frame named by the environment"]
fn what_drawing_the_badges_costs_on_a_real_frame() {
    let Ok(path) = std::env::var("ZEROCODE_MARK_BENCH_PNG") else {
        panic!("ZEROCODE_MARK_BENCH_PNG names a png a real marked look left");
    };
    let count: usize = std::env::var("ZEROCODE_MARK_BENCH_MARKS")
        .ok()
        .and_then(|marks| marks.parse().ok())
        .unwrap_or(99);
    let bytes = std::fs::read(&path).expect("the frame");
    let rounds = 20;

    let began = std::time::Instant::now();
    for _ in 0..rounds {
        crate::computer_use::compare::decode_png(&bytes).expect("a frame decodes");
    }
    let decode = began.elapsed() / rounds;

    let clean = crate::computer_use::compare::decode_png(&bytes).expect("a frame decodes");
    let marks: Vec<plan::PlacedMark> = (1..=count)
        .map(|mark| {
            #[allow(clippy::cast_precision_loss)]
            let at = (mark % 30) as f64 * 24.0;
            #[allow(clippy::cast_precision_loss)]
            let down = (mark / 30) as f64 * 24.0;
            plan::PlacedMark {
                mark,
                element_index: mark,
                role: "button".into(),
                label: None,
                screen: Rect::new(at, down, 16.0, 16.0),
                local: Rect::new(at, down, 16.0, 16.0),
                signature: String::new(),
                name: String::new(),
                context: String::new(),
                element_px: Rect::new(at, down, 16.0, 16.0),
                badge_px: Rect::new(at, down, 14.0, 12.0),
            }
        })
        .collect();

    let began = std::time::Instant::now();
    for _ in 0..rounds {
        let mut picture = clean.clone();
        draw(&mut picture, &marks);
        picture.encode().expect("a marked frame encodes");
    }
    let drawn = began.elapsed() / rounds;

    println!(
        "frame {}x{} · {count} badges · decode {:.1} ms · clone+draw+encode {:.1} ms · both {:.1} ms",
        clean.width,
        clean.height,
        decode.as_secs_f64() * 1_000.0,
        drawn.as_secs_f64() * 1_000.0,
        (decode + drawn).as_secs_f64() * 1_000.0,
    );
}
