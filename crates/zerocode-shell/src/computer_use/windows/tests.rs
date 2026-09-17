//! Functional tests against the disposable fixture window, and the
//! performance baseline (`ZEROCODE_COMPUTER_PERF=1`).
//!
//! These run only on Windows and only serially: one fixture, one provider,
//! one COM apartment on the test thread. Anything that needs the fixture in
//! the foreground says so and steps aside when the desktop refuses (a
//! headless CI session cannot always be made to) instead of pretending.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use zerocode_core::computer_use_protocol::keys::Modifier;

use super::clipboard::{
    RestoreOutcome, Snapshot as ClipboardSnapshot, fixture as clipboard_fixture,
};
use super::dpi::ThreadDpiAwareness;
use super::fixture::Fixture;
use super::provider::Provider;
use super::{ComApartment, ComHandler, desktop_windows, input};

static SERIAL: Mutex<()> = Mutex::new(());

/// The large-tree fixture: enough rows that the tree must be cut.
const LARGE_TREE_ROWS: usize = 5_000;

fn with_fixture(run: impl FnOnce(&mut Provider, &Fixture)) {
    with_fixture_rows(0, run);
}

/// The test thread speaks the provider's coordinate contract (physical
/// pixels) for the fixture's lifetime, the way the provider thread does.
fn with_fixture_rows(rows: usize, run: impl FnOnce(&mut Provider, &Fixture)) {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let _dpi = ThreadDpiAwareness::per_monitor_v2();
    assert!(
        ThreadDpiAwareness::is_per_monitor_v2(),
        "the test thread could not become per-monitor-v2 aware"
    );
    let _apartment = ComApartment::enter().expect("COM apartment");
    let mut provider = Provider::new().expect("UI Automation client");
    let fixture = Fixture::launch_with_rows("ZeroCode Computer Use Fixture", rows);
    std::thread::sleep(Duration::from_millis(400));
    run(&mut provider, &fixture);
}

fn observe(provider: &mut Provider, fixture: &Fixture, screenshot: bool) -> Value {
    provider
        .handle(
            "getAppState",
            json!({ "app": fixture.app_query(), "noScreenshot": !screenshot }),
        )
        .expect("getAppState")
}

fn tree_text(observation: &Value) -> &str {
    observation["snapshot"]["treeText"]
        .as_str()
        .unwrap_or_default()
}

/// The index at the head of the first tree line containing `needle`.
fn index_of(observation: &Value, needle: &str) -> usize {
    nth_index_of(observation, needle, 0)
}

/// The index at the head of the `nth` (zero-based) tree line containing
/// `needle` — the fixture has two edits, and the second is the password.
fn nth_index_of(observation: &Value, needle: &str, nth: usize) -> usize {
    tree_text(observation)
        .lines()
        .filter(|line| line.contains(needle))
        .nth(nth)
        .and_then(|line| {
            line.trim_start_matches('\t')
                .split(' ')
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or_else(|| {
            panic!(
                "no tree line #{nth} contains {needle:?}:\n{}",
                tree_text(observation)
            )
        })
}

fn foreground_or_skip(fixture: &Fixture, test: &str) -> bool {
    let candidate = desktop_windows::candidates(fixture.pid)
        .into_iter()
        .find(|candidate| candidate.hwnd == fixture.hwnd)
        .expect("fixture candidate");
    if desktop_windows::bring_to_front(&candidate) && desktop_windows::is_focused(&candidate) {
        return true;
    }
    eprintln!("skipping {test}: this desktop would not bring the fixture to the foreground");
    false
}

#[test]
fn the_fixture_is_listed_and_its_window_resolves() {
    with_fixture(|provider, fixture| {
        let apps = provider.handle("listApps", json!({})).expect("listApps");
        let pids: Vec<u64> = apps["apps"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|app| app["pid"].as_u64())
            .collect();
        assert!(pids.contains(&u64::from(fixture.pid)), "{apps}");
        let windows = provider
            .handle("listWindows", json!({ "app": fixture.app_query() }))
            .expect("listWindows");
        let listed = windows["windows"].as_array().unwrap();
        assert!(!listed.is_empty(), "{windows}");
        let own = listed
            .iter()
            .find(|window| window["id"].as_u64() == Some(fixture.hwnd as u64))
            .unwrap_or_else(|| panic!("fixture window not listed: {windows}"));
        assert_eq!(own["title"], "ZeroCode Computer Use Fixture");
        assert!(own["width"].as_i64().unwrap() >= 48);
        assert_eq!(own["isMinimized"], false);
        assert_eq!(windows["app"]["pid"], fixture.pid);
    });
}

#[test]
fn get_app_state_renders_the_controls_and_bounds_the_screenshot() {
    with_fixture(|provider, fixture| {
        let observed = observe(provider, fixture, true);
        let text = tree_text(&observed);
        assert!(text.starts_with("App="), "{text}");
        assert!(text.contains("(pid "));
        assert!(text.contains(" OK"), "button missing:\n{text}");
        assert!(text.contains(" Enable"), "checkbox missing:\n{text}");
        assert!(text.contains(" ready"), "status missing:\n{text}");
        assert_eq!(observed["snapshot"]["coordinateSpace"], "window");
        assert!(observed["snapshot"]["elementCount"].as_u64().unwrap() >= 4);
        assert_eq!(
            observed["screenshotStatus"]["state"], "captured",
            "{observed}"
        );
        let shot = &observed["screenshot"];
        assert_eq!(shot["format"], "png");
        assert!(shot["width"].as_u64().unwrap() > 0);
        assert!(shot["scale"].as_f64().unwrap() > 0.0);
        assert!(
            shot["data"].as_str().unwrap().len() < 1_300_000,
            "screenshot over budget"
        );

        let quiet = observe(provider, fixture, false);
        assert_eq!(quiet["screenshotStatus"]["state"], "skipped");
        assert!(quiet["screenshot"].is_null());
    });
}

#[test]
fn set_value_writes_korean_into_the_edit_and_reads_it_back() {
    with_fixture(|provider, fixture| {
        let observed = observe(provider, fixture, false);
        let edit = index_of(&observed, "edit");
        let value = "안녕하세요 ZeroCode ✓";
        let answer = provider
            .handle(
                "setValue",
                json!({ "app": fixture.app_query(), "elementIndex": edit, "value": value }),
            )
            .expect("setValue");
        assert_eq!(answer["action"]["path"], "accessibility");
        assert_eq!(answer["action"]["actionName"], "SetValue");
        assert_eq!(
            answer["action"]["verification"]["state"], "verified",
            "{answer}"
        );
        assert_eq!(answer["action"]["verification"]["expected"], value);
        assert_eq!(fixture.edit_text(), value);
        assert!(
            tree_text(&answer).contains(value),
            "the observation after the action shows the value"
        );
    });
}

#[test]
fn clicking_by_index_invokes_the_button_and_toggles_the_checkbox() {
    with_fixture(|provider, fixture| {
        let before = Fixture::clicks();
        let observed = observe(provider, fixture, false);
        let button = index_of(&observed, " OK");
        let answer = provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "elementIndex": button }),
            )
            .expect("click");
        assert_eq!(answer["action"]["path"], "accessibility", "{answer}");
        assert_eq!(answer["action"]["actionName"], "Invoke");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(Fixture::clicks(), before + 1);
        assert_eq!(fixture.status_text(), "clicked");

        let observed = observe(provider, fixture, false);
        let checkbox = index_of(&observed, " Enable");
        assert!(!fixture.checkbox_checked());
        let answer = provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "elementIndex": checkbox }),
            )
            .expect("click checkbox");
        assert_eq!(answer["action"]["actionName"], "Toggle", "{answer}");
        assert!(fixture.checkbox_checked());
        assert!(
            tree_text(&answer).contains("Enable, Value: 1")
                || tree_text(&answer).contains("Enable Value: 1"),
            "{}",
            tree_text(&answer)
        );
    });
}

#[test]
fn a_secondary_action_is_matched_by_its_pretty_name() {
    with_fixture(|provider, fixture| {
        let observed = observe(provider, fixture, false);
        let checkbox = index_of(&observed, " Enable");
        assert!(
            tree_text(&observed).contains("Secondary Actions: toggle"),
            "{}",
            tree_text(&observed)
        );
        let answer = provider
            .handle(
                "performSecondaryAction",
                json!({ "app": fixture.app_query(), "elementIndex": checkbox, "action": "toggle" }),
            )
            .expect("performSecondaryAction");
        assert_eq!(answer["action"]["actionName"], "Toggle");
        assert!(fixture.checkbox_checked());
        let refused = provider
            .handle(
                "performSecondaryAction",
                json!({ "app": fixture.app_query(), "elementIndex": checkbox, "action": "zoom the window" }),
            )
            .unwrap_err();
        assert_eq!(refused.code, "action_not_supported");
    });
}

#[test]
fn stale_or_unknown_indexes_are_refused_with_the_helpers_words() {
    with_fixture(|provider, fixture| {
        let fresh = provider
            .handle("click", json!({ "app": fixture.app_query(), "elementIndex": 0, "session": "never-observed" }))
            .unwrap_err();
        assert_eq!(fresh.code, "element_not_found");
        assert!(
            fresh.message.contains("fresh get-app-state"),
            "{}",
            fresh.message
        );

        let _ = observe(provider, fixture, false);
        let gone = provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "elementIndex": 999 }),
            )
            .unwrap_err();
        assert_eq!(gone.code, "element_not_found");
        assert!(
            gone.message.contains("element 999 is stale"),
            "{}",
            gone.message
        );
    });
}

#[test]
fn type_text_replaces_the_focused_edits_selection_through_the_value_pattern() {
    with_fixture(|provider, fixture| {
        let observed = observe(provider, fixture, false);
        let edit = index_of(&observed, "edit");
        provider
            .handle(
                "setValue",
                json!({ "app": fixture.app_query(), "elementIndex": edit, "value": "start " }),
            )
            .expect("seed");
        if !foreground_or_skip(fixture, "type-text through the focused edit") {
            return;
        }
        // Focus the edit with a real click so the provider sees it focused.
        provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "elementIndex": edit }),
            )
            .expect("focus the edit");
        std::thread::sleep(Duration::from_millis(200));
        let answer = provider
            .handle(
                "typeText",
                json!({ "app": fixture.app_query(), "text": "한글 end" }),
            )
            .expect("typeText");
        std::thread::sleep(Duration::from_millis(200));
        // Exactly once, after the seed: a `contains` would let the text
        // arrive twice (written by SetValue and typed again) and still pass.
        assert_eq!(
            fixture.edit_text(),
            "start 한글 end",
            "the edit holds the seed and the text once ({answer})"
        );
    });
}

/// H1 (B3): keys never write a secret. A password edit — UIA's `IsPassword`,
/// whose value no read may see — refuses typed text and the paste chord
/// before anything is sent (the person types it) and keeps what was there;
/// a Tab still leaves it. A secret the user gave goes in by `setValue`, which
/// is never read back and never repeated in the answer.
#[test]
fn keys_never_write_into_a_password_edit_and_set_value_is_not_read_back() {
    with_fixture(|provider, fixture| {
        use zerocode_core::computer_use_protocol::{error_code, unverified_reason};
        fixture.set_password_text("pre");
        let observed = observe(provider, fixture, false);
        let password = nth_index_of(&observed, "edit", 1);
        if !foreground_or_skip(fixture, "type-text into the password edit") {
            return;
        }
        provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "elementIndex": password }),
            )
            .expect("focus the password edit");
        std::thread::sleep(Duration::from_millis(200));
        for (method, field, value) in [
            ("typeText", "text", "hunter2"),
            ("pasteText", "text", "hunter2"),
            ("pressKey", "key", "ctrl+v"),
            ("hotkey", "key", "CmdOrCtrl+V"),
        ] {
            let refused = provider
                .handle(method, json!({ "app": fixture.app_query(), field: value }))
                .expect_err("keys never write a secret");
            assert_eq!(refused.code, error_code::SECURE_INPUT, "{method}");
            assert!(
                refused
                    .message
                    .starts_with(&format!("{}: ", unverified_reason::SECRET_FIELD)),
                "{method}: {}",
                refused.message
            );
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            fixture.password_text(),
            "pre",
            "nothing was typed or pasted"
        );
        let answer = provider
            .handle(
                "setValue",
                json!({ "app": fixture.app_query(), "elementIndex": password, "value": "hunter2" }),
            )
            .expect("setValue");
        assert_eq!(
            answer["action"]["verification"]["reason"],
            unverified_reason::SECRET_FIELD,
            "{answer}"
        );
        assert_eq!(fixture.password_text(), "hunter2");
        assert!(
            !tree_text(&answer).contains("hunter2") && !answer.to_string().contains("hunter2"),
            "the answer leaks the password:\n{answer}"
        );
    });
}

/// H3: a right-click on an element asks the element for its own menu
/// (`ShowContextMenu`) and, when the proxy has none, right-clicks it with the
/// pointer — and the menu that opens under the pointer is the target's own,
/// not a change of recipient. Either road ends in a menu and an `Ok`.
#[test]
fn a_right_click_on_an_element_opens_its_context_menu_without_a_false_failure() {
    with_fixture(|provider, fixture| {
        if !foreground_or_skip(fixture, "right-click on an element") {
            return;
        }
        let before = Fixture::menus();
        let observed = observe(provider, fixture, false);
        let button = index_of(&observed, " OK");
        let answer = provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "elementIndex": button, "mouseButton": "right" }),
            )
            .expect("right-click");
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(
            Fixture::menus(),
            before + 1,
            "no context menu was shown ({answer})"
        );
        match answer["action"]["path"].as_str() {
            Some("accessibility") => {
                assert_eq!(
                    answer["action"]["actionName"], "ShowContextMenu",
                    "{answer}"
                );
            }
            Some("synthetic") => {
                assert_eq!(
                    answer["action"]["fallbackReason"], "actionUnsupported",
                    "{answer}"
                );
            }
            other => panic!("unexpected path {other:?}: {answer}"),
        }
    });
}

/// H3, the coordinate road: the `#32768` popup that appears at the pointer
/// on mouse-up is owned by the target, so the after-release observation
/// reports the target and the click succeeds.
#[test]
fn a_coordinate_right_click_is_not_failed_by_the_menu_it_opens() {
    with_fixture(|provider, fixture| {
        if !foreground_or_skip(fixture, "coordinate right-click") {
            return;
        }
        let windows = provider
            .handle("listWindows", json!({ "app": fixture.app_query() }))
            .expect("listWindows");
        let own = windows["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|window| window["id"].as_u64() == Some(fixture.hwnd as u64))
            .unwrap()
            .clone();
        let (screen_x, screen_y) = fixture.bare_frame_point_screen();
        let x = screen_x - own["x"].as_f64().unwrap();
        let y = screen_y - own["y"].as_f64().unwrap();
        let before = Fixture::menus();
        let _ = observe(provider, fixture, false);
        let answer = provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "x": x, "y": y, "mouseButton": "right" }),
            )
            .expect("a right-click whose own menu must not read as a stolen recipient");
        assert_eq!(answer["action"]["path"], "synthetic");
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(Fixture::menus(), before + 1, "{answer}");
    });
}

/// A 2×2 32-bit DIB: the bytes `CF_DIB` carries for a tiny image.
fn tiny_dib() -> Vec<u8> {
    let mut bytes = Vec::new();
    // biSize, biWidth, biHeight, biPlanes (low word) | biBitCount (high
    // word), biCompression, biSizeImage, the two resolutions, biClrUsed,
    // biClrImportant — then the pixel rows, bottom-up, BGRA.
    let header: [u32; 10] = [40, 2, 2, (32 << 16) | 1, 0, 16, 2835, 2835, 0, 0];
    for word in header {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    for pixel in [
        [0u8, 0, 255, 0],
        [0, 255, 0, 0],
        [255, 0, 0, 0],
        [255, 255, 255, 0],
    ] {
        bytes.extend_from_slice(&pixel);
    }
    bytes
}

fn utf16_bytes(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

/// M1: every global-block format on the clipboard — text, a registered
/// format an app owns the meaning of, an image — comes back byte for byte
/// after `paste-text` has put its own text there and taken it away again.
#[test]
fn the_clipboard_round_trip_puts_every_format_back() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = ClipboardSnapshot::take().expect("read the host clipboard");
    let registered = clipboard_fixture::register("ZeroCode.ComputerUse.Fixture");
    let payload = vec![1u8, 2, 3, 4, 5, 250, 251];
    let dib = tiny_dib();
    clipboard_fixture::put(&[
        (13, utf16_bytes("before")),
        (registered, payload.clone()),
        (8, dib.clone()),
    ]);

    let held = ClipboardSnapshot::take().expect("snapshot");
    assert!(held.is_complete(), "unpreserved: {:?}", held.unpreserved());
    assert_eq!(held.text().as_deref(), Some("before"));
    assert_eq!(
        held.entry(registered).map(|entry| &entry.bytes),
        Some(&payload)
    );
    assert_eq!(held.entry(8).map(|entry| &entry.bytes), Some(&dib));

    let replaced = held
        .replace_with_text("secret")
        .unwrap_or_else(|(_, why)| panic!("{why}"));
    assert_eq!(
        ClipboardSnapshot::take().unwrap().text().as_deref(),
        Some("secret")
    );
    assert!(
        !clipboard_fixture::has_format(registered),
        "the paste text alone is on the clipboard"
    );
    assert_eq!(replaced.restore(), RestoreOutcome::Restored);

    let after = ClipboardSnapshot::take().expect("snapshot after restore");
    assert_eq!(after.text().as_deref(), Some("before"));
    assert_eq!(
        after.entry(registered).map(|entry| &entry.bytes),
        Some(&payload)
    );
    assert_eq!(after.entry(8).map(|entry| &entry.bytes), Some(&dib));
    assert!(
        clipboard_fixture::has_format(2),
        "CF_BITMAP is synthesized from the restored DIB"
    );
    if host.is_complete() {
        host.put_back();
    }
}

/// M1: a clipboard carrying something that cannot be read back (an enhanced
/// metafile is a GDI handle with no global twin) has no road to
/// `EmptyClipboard` at all: the snapshot is incomplete, the replacement is
/// refused, and the metafile is still there.
#[test]
fn a_clipboard_with_an_unpreservable_format_is_never_emptied() {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Graphics::Gdi::{CloseEnhMetaFile, CreateEnhMetaFileW};

    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let host = ClipboardSnapshot::take().expect("read the host clipboard");
    // SAFETY: a memory metafile with no drawing in it, closed into a handle
    // the clipboard takes ownership of.
    let metafile = unsafe {
        let dc = CreateEnhMetaFileW(None, None, None, None);
        CloseEnhMetaFile(dc)
    };
    clipboard_fixture::put_handle(14, HANDLE(metafile.0));

    let held = ClipboardSnapshot::take().expect("snapshot");
    assert!(!held.is_complete());
    assert!(
        held.unpreserved()
            .iter()
            .any(|name| name == "CF_ENHMETAFILE"),
        "{:?}",
        held.unpreserved()
    );
    let (_, why) = held
        .replace_with_text("secret")
        .err()
        .expect("an incomplete snapshot must not replace the clipboard");
    assert!(why.message.contains("CF_ENHMETAFILE"), "{why}");
    assert!(
        clipboard_fixture::has_format(14),
        "the metafile was emptied"
    );
    if host.is_complete() {
        host.put_back();
    }
}

/// Click a control by its tree line so the fixture's own focus lands on it.
fn focus_by_click(provider: &mut Provider, fixture: &Fixture, needle: &str, nth: usize) {
    let observed = observe(provider, fixture, false);
    let index = nth_index_of(&observed, needle, nth);
    provider
        .handle(
            "click",
            json!({ "app": fixture.app_query(), "elementIndex": index }),
        )
        .expect("focus the control");
    std::thread::sleep(Duration::from_millis(200));
}

/// M6: the fixture really is a dialog — the Tab KEY moves focus and the
/// Enter KEY presses the default button — so the literal-text tests below
/// are measuring something.
#[test]
fn the_fixture_navigates_on_the_tab_key_and_submits_on_the_enter_key() {
    with_fixture(|provider, fixture| {
        if !foreground_or_skip(fixture, "dialog key navigation") {
            return;
        }
        focus_by_click(provider, fixture, "edit", 0);
        assert_eq!(fixture.focused_control_id(), super::fixture::EDIT_ID);
        input::press_chord(
            &zerocode_core::computer_use_protocol::keys::parse_key_spec("tab").unwrap(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_ne!(
            fixture.focused_control_id(),
            super::fixture::EDIT_ID,
            "the Tab key did not move focus: the loop is not a dialog's"
        );
        focus_by_click(provider, fixture, "edit", 0);
        let clicks = Fixture::clicks();
        input::press_chord(
            &zerocode_core::computer_use_protocol::keys::parse_key_spec("return").unwrap(),
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            Fixture::clicks(),
            clicks + 1,
            "the Enter key did not press the default button"
        );
    });
}

/// M6: synthetic text with a line break and a tab lands in the focused
/// field exactly, moves no focus and presses no button — on the synthetic
/// road (`input::type_text`, the road a field without a Value pattern
/// takes) and on the accessibility road (`typeText` through SetValue).
#[test]
fn literal_text_stays_in_its_field_and_a_line_break_is_a_line_break() {
    with_fixture(|provider, fixture| {
        if !foreground_or_skip(fixture, "literal text into the edits") {
            return;
        }
        // The single-line edit: nothing typed may leave it or press OK.
        focus_by_click(provider, fixture, "edit", 0);
        let clicks = Fixture::clicks();
        input::type_text("x\ty\nz").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            fixture.focused_control_id(),
            super::fixture::EDIT_ID,
            "typed text moved the focus"
        );
        assert_eq!(
            Fixture::clicks(),
            clicks,
            "typed text pressed the default button"
        );
        let single = fixture.edit_text();
        assert!(
            single.starts_with('x') && single.ends_with('z'),
            "{single:?}"
        );

        // The multi-line edit: exact content, the platform's line break.
        focus_by_click(provider, fixture, "edit", 2);
        assert_eq!(fixture.focused_control_id(), super::fixture::MULTILINE_ID);
        input::type_text("a\nb\tc").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(fixture.multiline_text(), "a\r\nb\tc");
        assert_eq!(fixture.focused_control_id(), super::fixture::MULTILINE_ID);
        assert_eq!(Fixture::clicks(), clicks);

        // The accessibility road writes the literal text as given.
        let multiline = nth_index_of(&observe(provider, fixture, false), "edit", 2);
        let answer = provider
            .handle(
                "setValue",
                json!({ "app": fixture.app_query(), "elementIndex": multiline, "value": "one\r\ntwo\tthree" }),
            )
            .expect("setValue");
        assert_eq!(
            answer["action"]["verification"]["state"], "verified",
            "{answer}"
        );
        assert_eq!(fixture.multiline_text(), "one\r\ntwo\tthree");
        assert_eq!(Fixture::clicks(), clicks);
    });
}

/// H2: `Cmd`/`Meta`/`Win` is the Windows key on every road, and the Windows
/// key is never held around a click.
#[test]
fn the_windows_key_is_the_windows_key_and_never_a_click_modifier() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_LWIN};
    assert_eq!(input::modifier_key(Modifier::Command), VK_LWIN);
    assert_eq!(input::modifier_key(Modifier::Primary), VK_CONTROL);
    assert_eq!(input::modifier_key(Modifier::Control), VK_CONTROL);
    let refused = input::click_modifier_keys(&[Modifier::Shift, Modifier::Command]).unwrap_err();
    assert_eq!(refused.code, "invalid_argument");
    assert!(
        refused.message.contains("not a click modifier"),
        "{}",
        refused.message
    );
    assert_eq!(
        input::click_modifier_keys(&[Modifier::Primary, Modifier::Shift]).unwrap(),
        vec![
            VK_CONTROL,
            windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT
        ]
    );
}

#[test]
fn a_coordinate_click_lands_on_the_button_when_the_fixture_is_in_front() {
    with_fixture(|provider, fixture| {
        if !foreground_or_skip(fixture, "coordinate click") {
            return;
        }
        let windows = provider
            .handle("listWindows", json!({ "app": fixture.app_query() }))
            .expect("listWindows");
        let own = windows["windows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|window| window["id"].as_u64() == Some(fixture.hwnd as u64))
            .unwrap()
            .clone();
        let (screen_x, screen_y) = fixture.button_center_screen();
        let x = screen_x - own["x"].as_f64().unwrap();
        let y = screen_y - own["y"].as_f64().unwrap();
        let before = Fixture::clicks();
        let _ = observe(provider, fixture, false);
        let answer = provider
            .handle(
                "click",
                json!({ "app": fixture.app_query(), "x": x, "y": y }),
            )
            .expect("coordinate click");
        assert_eq!(answer["action"]["path"], "synthetic");
        assert_eq!(
            answer["action"]["verification"]["reason"],
            "synthetic_input"
        );
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(Fixture::clicks(), before + 1, "{answer}");
    });
}

/// M2: a window with thousands of rows is cut at the node cap instead of
/// materialised whole — the snapshot says so, the count stays under the
/// cap, and the fetch count is the visited parents, not the rows.
#[test]
fn a_large_tree_is_cut_at_the_cap_within_a_bounded_number_of_fetches() {
    with_fixture_rows(LARGE_TREE_ROWS, |provider, fixture| {
        let started = Instant::now();
        let observed = observe(provider, fixture, false);
        let elapsed = started.elapsed();
        assert_eq!(
            observed["snapshot"]["truncation"]["truncated"],
            true,
            "{}",
            tree_text(&observed)
        );
        let count = observed["snapshot"]["elementCount"].as_u64().unwrap();
        assert!(count <= 1200, "{count} elements");
        assert!(count >= 1000, "{count} elements — the cap was not reached");
        assert!(
            tree_text(&observed).contains(" OK"),
            "the controls before the list still render:\n{}",
            tree_text(&observed)
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "a cut tree must not cost UI Automation's timeout: {elapsed:?}"
        );
        // The provider's own count of cross-process fetches, read from a
        // fresh build: one per visited parent, never one per row.
        let app = provider.apps.resolve(&fixture.app_query()).unwrap();
        let snapshot = super::snapshot::build(
            &provider.client,
            super::snapshot::BuildRequest {
                app: &app,
                include_screenshot: false,
                window_id: None,
                window_index: None,
                restore_window: false,
            },
        )
        .unwrap();
        assert!(snapshot.truncated);
        assert!(
            snapshot.fetches < 1300,
            "{} fetches for a {LARGE_TREE_ROWS}-row list",
            snapshot.fetches
        );
        assert!(
            snapshot.elements.len() <= 2400,
            "{} live elements held",
            snapshot.elements.len()
        );
    });
}

/// M3: the provider thread is per-monitor-v2 aware by its own doing, so
/// every coordinate it reads and posts is a physical pixel — the contract
/// UI Automation's rectangles already speak.
#[test]
fn the_provider_thread_speaks_physical_pixels() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let aware = std::thread::spawn(|| {
        let handler = ComHandler::start().expect("provider");
        let aware = ThreadDpiAwareness::is_per_monitor_v2();
        drop(handler);
        aware
    })
    .join()
    .expect("provider thread");
    assert!(
        aware,
        "the provider thread did not set PER_MONITOR_AWARE_V2"
    );
}

/// M3: the fixture window and the provider agree on where a control is —
/// UI Automation's (always physical) rectangle for the button matches the
/// button's client rectangle mapped through the fixture's own
/// `ClientToScreen` on a per-monitor-v2 thread. On a scaled monitor this is
/// exactly the disagreement the old test process had.
#[test]
fn ui_automation_and_win32_agree_on_a_controls_physical_rectangle() {
    with_fixture(|provider, fixture| {
        let observed = observe(provider, fixture, false);
        let button = index_of(&observed, " OK");
        let app = provider.apps.resolve(&fixture.app_query()).unwrap();
        let snapshot = super::snapshot::build(
            &provider.client,
            super::snapshot::BuildRequest {
                app: &app,
                include_screenshot: false,
                window_id: None,
                window_index: None,
                restore_window: false,
            },
        )
        .unwrap();
        let (record, _) = snapshot.element(button).unwrap();
        let (x, y) = snapshot.center_of(record).expect("button frame");
        let (expected_x, expected_y) = fixture.button_center_screen();
        assert!(
            (x - expected_x).abs() <= 2.0 && (y - expected_y).abs() <= 2.0,
            "UIA centre ({x}, {y}) vs Win32 centre ({expected_x}, {expected_y}) — dpi {}",
            fixture.dpi()
        );
    });
}

/// L4 / M4: the desktop's own furniture is never a candidate, and the
/// frame-host rule keeps a classic window's identity as its own process.
#[test]
fn shell_surfaces_are_not_candidates_and_a_classic_window_is_its_own_app() {
    assert!(desktop_windows::is_shell_surface("Shell_TrayWnd"));
    assert!(desktop_windows::is_shell_surface("Progman"));
    assert!(desktop_windows::is_shell_surface("WorkerW"));
    assert!(!desktop_windows::is_shell_surface("Notepad"));
    assert!(!desktop_windows::is_shell_surface("ApplicationFrameWindow"));
    with_fixture(|_, fixture| {
        let own = desktop_windows::candidates(fixture.pid)
            .into_iter()
            .find(|candidate| candidate.hwnd == fixture.hwnd)
            .expect("fixture candidate");
        assert_eq!(own.pid, fixture.pid);
        assert_eq!(own.host_pid, fixture.pid);
        assert_eq!(
            desktop_windows::class_name(fixture.handle()),
            "ZeroCodeComputerUseFixture"
        );
        let listed: Vec<u32> = desktop_windows::all_top_level()
            .into_iter()
            .map(|(_, candidate)| candidate.hwnd)
            .filter_map(|hwnd| {
                let class = desktop_windows::class_name(desktop_windows::hwnd_from_id(hwnd as u64));
                desktop_windows::is_shell_surface(&class).then_some(1)
            })
            .collect();
        assert!(listed.is_empty(), "shell windows listed as candidates");
    });
}

/// M2/M7: process descriptions are served from the catalog while the same
/// process stands — a name query stops reading every version resource on
/// every action.
#[test]
fn the_app_catalog_serves_repeat_lookups_from_memory() {
    with_fixture(|provider, fixture| {
        let first = provider.apps.resolve(&fixture.app_query()).unwrap();
        let misses = provider.apps.misses;
        let hits = provider.apps.hits;
        for _ in 0..5 {
            let again = provider.apps.resolve(&fixture.app_query()).unwrap();
            assert_eq!(again.name, first.name);
        }
        assert_eq!(
            provider.apps.misses, misses,
            "a repeat lookup re-read the process"
        );
        assert_eq!(provider.apps.hits, hits + 5);
        // A name query walks every window-owning process once, then hits.
        let by_name = provider.apps.resolve(&first.name).unwrap();
        assert_eq!(by_name.pid, fixture.pid);
        let misses = provider.apps.misses;
        let _ = provider.apps.resolve(&first.name).unwrap();
        assert_eq!(
            provider.apps.misses, misses,
            "the second name query re-read processes"
        );
    });
}

#[test]
fn permission_status_reports_granted_when_ui_automation_starts() {
    let report = super::permission_status();
    assert_eq!(report.platform, "windows");
    assert!(report.helper_unavailable_reason.is_none(), "{report:?}");
    assert!(report.permissions.iter().all(|row| {
        row.status == zerocode_core::computer_use::ComputerPermissionStatus::Granted
    }));
    let setup = super::open_permission(None).unwrap();
    assert!(!setup.opened_settings && !setup.launched_helper && setup.next_step.is_none());
}

fn percentile(samples: &mut [f64], fraction: f64) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let index = ((samples.len() as f64 - 1.0) * fraction).round() as usize;
    samples[index]
}

/// One axis, `samples` times: the last answer and every duration in ms.
fn timed(
    provider: &mut Provider,
    samples: usize,
    mut step: impl FnMut(&mut Provider) -> Value,
) -> (Value, Vec<f64>) {
    let mut durations = Vec::with_capacity(samples);
    let mut last = Value::Null;
    for _ in 0..samples {
        let started = Instant::now();
        last = step(provider);
        durations.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    (last, durations)
}

fn axis(name: &str, mut durations: Vec<f64>, report: &mut serde_json::Map<String, Value>) {
    let p50 = percentile(&mut durations, 0.5);
    let p95 = percentile(&mut durations, 0.95);
    report.insert(
        name.to_string(),
        json!({
            "samples": durations.len(),
            "p50Ms": p50,
            "p95Ms": p95,
            "minMs": durations.first(),
            "maxMs": durations.last(),
        }),
    );
}

/// This process's CPU time and memory, for the slope of a loop.
fn process_usage() -> (f64, u64, u64) {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: plain reads on the current process's pseudo-handle into
    // structs sized for them.
    unsafe {
        let _ = GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        );
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb);
    }
    let hundred_ns =
        |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
    let cpu_seconds = (hundred_ns(kernel) + hundred_ns(user)) as f64 / 10_000_000.0;
    (
        cpu_seconds,
        counters.WorkingSetSize as u64,
        counters.PagefileUsage as u64,
    )
}

/// The measured baseline: `ZEROCODE_COMPUTER_PERF=1 cargo test … perf_baseline -- --nocapture`.
/// Writes JSON to `ZEROCODE_COMPUTER_PERF_OUT` when set. Axes: cold and
/// warm session start, tree fetch with and without a picture, catalog
/// reuse, element click and value write, the large tree, a twenty-second
/// observation loop's CPU/RSS slope, and the DPI/monitor facts they were
/// measured under. A foreground-dependent axis that cannot run says
/// "skipping" like the functional tests do.
#[test]
fn perf_baseline() {
    if std::env::var_os("ZEROCODE_COMPUTER_PERF").is_none() {
        eprintln!("skipping perf_baseline: set ZEROCODE_COMPUTER_PERF=1 to measure");
        return;
    }
    let mut report = serde_json::Map::new();
    // Session start: the provider thread, its COM apartment and UIA client,
    // through the same road the window takes — cold is the first in this
    // process, warm the ones after it.
    {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let started = Instant::now();
        let first =
            super::super::thread::ProviderThread::spawn("perf-cold", ComHandler::start).unwrap();
        let cold = started.elapsed().as_secs_f64() * 1000.0;
        let _ = first.request("handshake", json!({}), Duration::from_secs(10));
        drop(first);
        let mut warm = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let thread =
                super::super::thread::ProviderThread::spawn("perf-warm", ComHandler::start)
                    .unwrap();
            let _ = thread.request("handshake", json!({}), Duration::from_secs(10));
            warm.push(started.elapsed().as_secs_f64() * 1000.0);
            drop(thread);
        }
        report.insert("session.coldStartMs".into(), json!(cold));
        axis("session.warmStartAndHandshake", warm, &mut report);
    }
    with_fixture(|provider, fixture| {
        const SAMPLES: usize = 20;
        let query = fixture.app_query();

        let (_, quiet) = timed(provider, SAMPLES, |provider| {
            observe(provider, fixture, false)
        });
        axis("getAppState.noScreenshot", quiet, &mut report);
        let (shot, pictured) = timed(provider, SAMPLES, |provider| {
            observe(provider, fixture, true)
        });
        axis("getAppState.screenshot", pictured, &mut report);
        report.insert(
            "catalog".into(),
            json!({ "hits": provider.apps.hits, "misses": provider.apps.misses }),
        );
        let name = provider.apps.resolve(&query).unwrap().name;
        let (_, by_name) = timed(provider, SAMPLES, |provider| {
            provider
                .handle("listWindows", json!({ "app": name }))
                .unwrap()
        });
        axis("listWindows.byName", by_name, &mut report);

        let observed = observe(provider, fixture, false);
        let button = index_of(&observed, " OK");
        let edit = index_of(&observed, "edit");
        let (_, clicks) = timed(provider, SAMPLES, |provider| {
            provider
                .handle(
                    "click",
                    json!({ "app": query, "elementIndex": button, "noScreenshot": true }),
                )
                .unwrap()
        });
        axis("click.element", clicks, &mut report);
        let (_, writes) = timed(provider, SAMPLES, |provider| {
            provider
                .handle(
                    "setValue",
                    json!({ "app": query, "elementIndex": edit, "value": "perf", "noScreenshot": true }),
                )
                .unwrap()
        });
        axis("setValue.edit", writes, &mut report);

        // The foreground axes: a synthetic click and synthetic typing.
        if foreground_or_skip(fixture, "perf synthetic click and typeText") {
            let (screen_x, screen_y) = fixture.button_center_screen();
            let windows = provider
                .handle("listWindows", json!({ "app": query }))
                .unwrap();
            let own = windows["windows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|window| window["id"].as_u64() == Some(fixture.hwnd as u64))
                .unwrap()
                .clone();
            let x = screen_x - own["x"].as_f64().unwrap();
            let y = screen_y - own["y"].as_f64().unwrap();
            let (_, synthetic) = timed(provider, SAMPLES, |provider| {
                provider
                    .handle(
                        "click",
                        json!({ "app": query, "x": x, "y": y, "noScreenshot": true }),
                    )
                    .unwrap()
            });
            axis("click.coordinate", synthetic, &mut report);
            let (_, typed) = timed(provider, SAMPLES, |provider| {
                provider
                    .handle(
                        "typeText",
                        json!({ "app": query, "text": "perf", "noScreenshot": true }),
                    )
                    .unwrap()
            });
            axis("typeText.focused", typed, &mut report);
        }

        // Twenty seconds of observation: CPU seconds per wall second, and
        // the working-set growth per minute, so a leak shows as a slope.
        let (cpu_before, rss_before, private_before) = process_usage();
        let loop_started = Instant::now();
        let mut loops = 0u32;
        while loop_started.elapsed() < Duration::from_secs(20) {
            let _ = observe(provider, fixture, false);
            loops += 1;
        }
        let wall = loop_started.elapsed().as_secs_f64();
        let (cpu_after, rss_after, private_after) = process_usage();
        report.insert(
            "loop.observe20s".into(),
            json!({
                "observations": loops,
                "cpuSecondsPerWallSecond": (cpu_after - cpu_before) / wall,
                "workingSetGrowthMbPerMinute": (rss_after as f64 - rss_before as f64) / 1_048_576.0 / (wall / 60.0),
                "privateBytesGrowthMbPerMinute": (private_after as f64 - private_before as f64) / 1_048_576.0 / (wall / 60.0),
            }),
        );

        report.insert(
            "screenshotBytes".into(),
            json!(
                shot["screenshot"]["data"]
                    .as_str()
                    .map_or(0, |data| data.len() * 3 / 4)
            ),
        );
        report.insert(
            "environment".into(),
            json!({
                "elementCount": observed["snapshot"]["elementCount"],
                "windowWidth": observed["snapshot"]["window"]["width"],
                "windowHeight": observed["snapshot"]["window"]["height"],
                "virtualScreen": format!("{:?}", desktop_windows::virtual_screen()),
                "dpi": {
                    "testThread": ThreadDpiAwareness::describe(),
                    "fixtureWindowDpi": fixture.dpi(),
                    "monitors": desktop_windows::monitor_count(),
                },
            }),
        );
    });
    // The large tree, on its own fixture: the cut observation's cost.
    with_fixture_rows(LARGE_TREE_ROWS, |provider, fixture| {
        let (last, large) = timed(provider, 10, |provider| observe(provider, fixture, false));
        axis("getAppState.largeTree.noScreenshot", large, &mut report);
        report.insert(
            "largeTree".into(),
            json!({
                "rows": LARGE_TREE_ROWS,
                "elementCount": last["snapshot"]["elementCount"],
                "truncated": last["snapshot"]["truncation"]["truncated"],
            }),
        );
    });
    let json = serde_json::to_string_pretty(&Value::Object(report)).unwrap();
    eprintln!("{json}");
    if let Some(path) = std::env::var_os("ZEROCODE_COMPUTER_PERF_OUT") {
        std::fs::write(path, json).expect("write the perf report");
    }
}
