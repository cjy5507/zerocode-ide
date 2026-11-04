//! A real Android phone's road on a fake `adb` (t-93824): the person's answer
//! before any process, the commands each verb sends, and the ones a phone never
//! gets. The serial, the model and every word on the screen are made up.

use std::os::unix::fs::PermissionsExt;
use std::time::Instant;

use zerocode_core::agent_emulator::real_android::{
    PHONE_EMULATOR_ONLY, PHONE_INSTALL_REFUSED, PHONE_POWER_REFUSED, PHONE_SETTINGS_REFUSED,
};

use super::*;

const SERIAL: &str = "R5CTSYNTH01";

const OPEN: &str = "  KeyguardServiceDelegate\n    showing=false\n    screenState=SCREEN_STATE_ON\n    KeyguardStateMonitor\n      mIsShowing=false\n";

/// A notes app's screen: no money on it, a password field among its rows.
const TREE: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node class="android.widget.FrameLayout" package="com.example.notes" enabled="true" bounds="[0,0][1080,2400]"><node class="android.widget.EditText" package="com.example.notes" text="synthetic-secret" password="true" clickable="true" enabled="true" bounds="[100,400][980,520]" /><node class="android.widget.TextView" package="com.example.notes" text="메모 Synthetic" enabled="true" bounds="[100,600][980,700]" /><node class="android.widget.Button" package="com.example.notes" text="저장" clickable="true" enabled="true" bounds="[100,2000][980,2200]" /></node></hierarchy>"#;

/// A wallet's screen with a payment button.
const WALLET: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node class="android.widget.FrameLayout" package="com.example.wallet" enabled="true" bounds="[0,0][1080,2400]"><node class="android.widget.EditText" package="com.example.wallet" text="synthetic-secret" password="true" clickable="true" enabled="true" bounds="[100,400][980,520]" /><node class="android.widget.TextView" package="com.example.wallet" text="받는 분 Synthetic" enabled="true" bounds="[100,600][980,700]" /><node class="android.widget.Button" package="com.example.wallet" text="결제하기" clickable="true" enabled="true" bounds="[100,2000][980,2200]" /></node></hierarchy>"#;

struct Bench {
    directory: tempfile::TempDir,
    road: Road,
}

impl Bench {
    fn new(allowed: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let adb = directory.path().join("adb");
        std::fs::write(
            &adb,
            format!(
                r#"#!/bin/sh
cd "$(dirname "$0")" || exit 1
printf '%s\n' "$*" >> commands
case "$*" in
  'devices -l') cat devices ;;
  '-s {SERIAL} shell dumpsys window policy') cat policy ;;
  '-s {SERIAL} shell dumpsys input') cat display ;;
  '-s {SERIAL} shell uiautomator dump '*) exit 0 ;;
  '-s {SERIAL} exec-out cat '*) cat tree.xml ;;
  '-s {SERIAL} shell rm -f '*) exit 0 ;;
  '-s {SERIAL} shell input '*) exit 0 ;;
  '-s {SERIAL} shell sha256sum '*) cat dexsum ;;
  '-s {SERIAL} push '*) exit 0 ;;
  '-s {SERIAL} shell [ -f '*) exit "$(cat text-exit)" ;;
  '-s {SERIAL} exec-out screencap -p') printf 'PNGBYTES' ;;
  '-s emulator-5554 shell dumpsys input') cat display ;;
  '-s emulator-5554 shell input '*) exit 0 ;;
  *) exit 1 ;;
esac
"#
            ),
        )
        .unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o700)).unwrap();
        let bench = Self {
            road: Road {
                sdk: AndroidSdk {
                    root: directory.path().into(),
                    adb,
                    emulator: Ok(directory.path().join("unused-emulator")),
                },
                allowed: Box::new(move |_| allowed),
            },
            directory,
        };
        bench.write(
            "devices",
            &format!("List of devices attached\n{SERIAL}  device usb:1-1 product:synth model:Synthetic_Pixel transport_id:2\n"),
        );
        bench.write("policy", OPEN);
        bench.write(
            "display",
            "Input Reader State:\n  Configuration:\n    Viewports:\n      Viewport INTERNAL: displayId=0, uniqueId=local:synthetic, port=0, orientation=0, logicalFrame=[0, 0, 1080, 2400], physicalFrame=[0, 0, 1080, 2400], deviceSize=[1080, 2400], isActive=[1]\n",
        );
        bench.write("tree.xml", TREE);
        bench.write(
            "dexsum",
            &format!("{}  helper\n", super::super::INSERT_DEX_SHA256),
        );
        bench.write("text-exit", "0");
        bench
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.directory.path().join(name), text).unwrap();
    }

    /// Every command the fake adb was asked, in order.
    fn commands(&self) -> Vec<String> {
        std::fs::read_to_string(self.directory.path().join("commands"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn forget_commands(&self) {
        let _ = std::fs::remove_file(self.directory.path().join("commands"));
    }

    fn handle() -> String {
        android_phone_handle(SERIAL)
    }

    /// The commands that went to the phone itself, without `-s <serial>`.
    fn to_the_phone(&self) -> Vec<String> {
        let prefix = format!("-s {SERIAL} ");
        self.commands()
            .iter()
            .filter_map(|line| line.strip_prefix(&prefix).map(str::to_string))
            .collect()
    }
}

fn code_of<T: std::fmt::Debug>(result: Result<T, ProviderError>) -> String {
    match result {
        Ok(answer) => panic!("refused nothing: {answer:?}"),
        Err(error) => error.code,
    }
}

#[test]
fn without_the_persons_answer_every_verb_is_refused_and_adb_is_never_asked() {
    let bench = Bench::new(false);
    let handle = Bench::handle();
    let road = &bench.road;
    for code in [
        code_of(tree_on(road, &handle)),
        code_of(marks_on(road, &handle).map(|_| ())),
        code_of(tap_on(road, &handle, 0.5, 0.5)),
        code_of(swipe_on(road, &handle, (0.5, 0.8), (0.5, 0.2), None)),
        code_of(text_on(road, &handle, "hello")),
        code_of(button_on(road, &handle, "home")),
        code_of(screenshot_on(road, &handle)),
    ] {
        assert_eq!(code, PHONE_NOT_ALLOWED);
    }
    assert_eq!(bench.commands(), Vec::<String>::new(), "adb was asked");
}

#[test]
fn an_allowed_phone_takes_tap_text_tree_and_buttons_as_adb_input() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    let road = &bench.road;

    let tapped = tap_on(road, &handle, 0.5, 0.25).expect("a tap on an open phone");
    assert_eq!(tapped[ROAD_KEY], ROAD_ADB);
    assert_eq!(tapped[MAC_POINTER_KEY], false);
    assert!(
        tapped
            .get(zerocode_core::agent_emulator::EMULATOR_SETTLE_KEY)
            .is_some()
    );
    let sent = bench.to_the_phone();
    assert!(
        sent.contains(&"shell input tap 540 600".to_string()),
        "{sent:?}"
    );
    assert_eq!(
        sent.first().map(String::as_str),
        Some("shell dumpsys window policy"),
        "the lock is read before anything else reaches the phone"
    );

    bench.forget_commands();
    let tree = tree_on(road, &handle).expect("a tree");
    assert_eq!(tree["app"]["package"], "com.example.notes");
    let sent = bench.to_the_phone();
    assert!(
        sent.iter()
            .any(|line| line.starts_with("shell uiautomator dump /sdcard/window_dump.xml."))
    );
    assert!(
        sent.iter()
            .any(|line| line.starts_with("exec-out cat /sdcard/window_dump.xml."))
    );

    bench.forget_commands();
    text_on(road, &handle, "hello").expect("ascii typed as keys");
    let line = bench.to_the_phone().pop().unwrap_or_default();
    assert!(
        line.contains("--check; c=$?; [ $c = 0 ] || exit $c; input text 'hello'"),
        "{line}"
    );

    bench.forget_commands();
    text_on(road, &handle, "안녕하세요").expect("Hangul through the helper");
    let line = bench.to_the_phone().pop().unwrap_or_default();
    assert!(
        line.ends_with("dev.zerocode.text.Insert 7JWI64WV7ZWY7IS47JqU"),
        "{line}"
    );

    for (name, code) in [("home", "3"), ("back", "4"), ("enter", "66")] {
        bench.forget_commands();
        button_on(road, &handle, name).expect("a button");
        assert_eq!(
            bench.to_the_phone().pop().unwrap_or_default(),
            format!("shell input keyevent {code}")
        );
    }

    bench.forget_commands();
    swipe_on(road, &handle, (0.5, 0.8), (0.5, 0.2), Some(300)).expect("a swipe");
    assert!(
        bench
            .to_the_phone()
            .iter()
            .any(|line| line.starts_with("shell input swipe 540 1919 540 480 300"))
    );

    assert_eq!(
        screenshot_on(road, &handle).expect("a picture"),
        b"PNGBYTES"
    );
}

#[test]
fn what_only_an_emulator_has_never_reaches_a_real_serial() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    for (args, code) in [
        (vec!["emu", "kill"], PHONE_EMULATOR_ONLY),
        (vec!["emu", "avd", "name"], PHONE_EMULATOR_ONLY),
        (
            vec!["shell", "settings", "put", "system", "user_rotation", "1"],
            PHONE_SETTINGS_REFUSED,
        ),
        (
            vec!["shell", "input", "keyevent", "26"],
            PHONE_POWER_REFUSED,
        ),
        (
            vec!["shell", "input", "keyevent", "KEYCODE_SLEEP"],
            PHONE_POWER_REFUSED,
        ),
        (vec!["install", "/tmp/app.apk"], PHONE_INSTALL_REFUSED),
        (
            vec!["uninstall", "com.example.wallet"],
            PHONE_INSTALL_REFUSED,
        ),
    ] {
        let refused = adb_command(&bench.road.sdk.adb, SERIAL, &args).map(|_| ());
        assert_eq!(code_of(refused), code, "{args:?}");
    }
    assert_eq!(
        code_of(button_on(&bench.road, &handle, "power").map(|_| ())),
        PHONE_POWER_REFUSED
    );
    assert_eq!(
        code_of(button_on(&bench.road, &handle, "lock").map(|_| ())),
        PHONE_POWER_REFUSED
    );
    assert_eq!(rotate_refused().code, PHONE_SETTINGS_REFUSED);
    assert_eq!(emulator_only().code, PHONE_EMULATOR_ONLY);
    assert_eq!(
        bench.commands(),
        Vec::<String>::new(),
        "a refusal spent a process"
    );
    // The emulator's own road is untouched: its console still goes to its serial.
    assert!(adb_command(&bench.road.sdk.adb, "emulator-5554", &["emu", "kill"]).is_ok());
}

#[test]
fn a_locked_phone_or_an_unread_lock_is_neither_read_nor_pressed() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    for policy in [
        OPEN.replace("mIsShowing=false", "mIsShowing=true"),
        String::new(),
    ] {
        bench.write("policy", &policy);
        bench.forget_commands();
        assert_eq!(
            code_of(tap_on(&bench.road, &handle, 0.5, 0.5)),
            PHONE_LOCKED
        );
        assert_eq!(code_of(tree_on(&bench.road, &handle)), PHONE_LOCKED);
        assert_eq!(code_of(screenshot_on(&bench.road, &handle)), PHONE_LOCKED);
        assert_eq!(
            bench.to_the_phone(),
            ["shell dumpsys window policy"; 3],
            "only the lock was read"
        );
    }
}

#[test]
fn a_password_field_is_refused_and_its_value_never_carried() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    bench.write("text-exit", "5");
    assert_eq!(
        code_of(text_on(&bench.road, &handle, "hunter2")),
        PHONE_SECURE_FIELD
    );
    bench.write("text-exit", "3");
    assert_eq!(
        code_of(text_on(&bench.road, &handle, "hello")),
        "invalid_argument"
    );
    let tree = tree_on(&bench.road, &handle).expect("a tree");
    let said = tree.to_string();
    assert!(!said.contains("synthetic-secret"), "{said}");
    let marks = marks_on(&bench.road, &handle).expect("a look");
    assert!(!marks.raw.to_string().contains("synthetic-secret"));
    assert_eq!(
        code_of(text_on(&bench.road, &handle, "line\nbreak")),
        "invalid_argument"
    );
    assert_eq!(
        code_of(text_on(&bench.road, &handle, &"x".repeat(4097))),
        "invalid_argument"
    );
}

#[test]
fn a_payment_press_is_refused_and_a_scroll_passes() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    bench.write("tree.xml", WALLET);
    bench.forget_commands();
    assert_eq!(
        code_of(tap_on(&bench.road, &handle, 0.5, 0.875)),
        error_code::CONFIRMATION_REQUIRED
    );
    assert!(
        !bench
            .to_the_phone()
            .iter()
            .any(|line| line.starts_with("shell input tap")),
        "the payment control was pressed"
    );
    swipe_on(&bench.road, &handle, (0.5, 0.8), (0.5, 0.2), None)
        .expect("a scroll on the same screen");
    // A screen that states a payment is the person's beyond its button too.
    assert_eq!(
        code_of(tap_on(&bench.road, &handle, 0.5, 0.27)),
        error_code::CONFIRMATION_REQUIRED
    );
    assert!(
        !bench
            .to_the_phone()
            .iter()
            .any(|line| line.starts_with("shell input tap")),
        "a press on the money screen went"
    );
}

#[test]
fn a_phone_that_has_not_allowed_this_computer_or_is_offline_says_why() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    for (state, code) in [
        ("unauthorized", PHONE_UNAUTHORIZED),
        ("offline", PHONE_UNREACHABLE),
    ] {
        bench.write(
            "devices",
            &format!("List of devices attached\n{SERIAL}  {state} usb:1-1 transport_id:2\n"),
        );
        bench.forget_commands();
        assert_eq!(code_of(tap_on(&bench.road, &handle, 0.5, 0.5)), code);
        assert_eq!(
            bench.to_the_phone(),
            Vec::<String>::new(),
            "{state}: nothing reached the phone"
        );
    }
    bench.write("devices", "List of devices attached\n");
    assert_eq!(
        code_of(tap_on(&bench.road, &handle, 0.5, 0.5)),
        PHONE_UNREACHABLE
    );
    let checked = check_on(&bench.road, &handle);
    assert_eq!(checked["ready"], false);
}

#[test]
fn every_command_the_road_sends_is_on_the_phones_list() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    let road = &bench.road;
    tree_on(road, &handle).expect("tree");
    marks_on(road, &handle).expect("marks");
    tap_on(road, &handle, 0.5, 0.25).expect("tap");
    swipe_on(road, &handle, (0.5, 0.8), (0.5, 0.2), None).expect("swipe");
    text_on(road, &handle, "hello").expect("keys");
    text_on(road, &handle, "안녕").expect("helper");
    button_on(road, &handle, "back").expect("button");
    screenshot_on(road, &handle).expect("screenshot");
    let _ = check_on(road, &handle);
    let sent = bench.to_the_phone();
    assert!(sent.len() > 10, "{sent:?}");
    for line in &sent {
        assert!(
            !line.starts_with("emu"),
            "the emulator console reached a phone: {line}"
        );
        let args: Vec<&str> = if let Some(rest) = line.strip_prefix("shell [ -f ") {
            vec!["shell", line.strip_prefix("shell ").unwrap_or(rest)]
        } else {
            line.split(' ').collect()
        };
        assert_eq!(adb_refusal(&args), None, "off the list: {line}");
    }
}

/// One tap's round trip on the fake adb: the phone's (answer, serial, lock, tree,
/// tap, settle) beside the emulator road's own send (display, tap).
#[test]
fn a_tap_round_trip_is_measured_beside_the_emulators() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    let started = Instant::now();
    tap_on(&bench.road, &handle, 0.5, 0.25).expect("a phone tap");
    let phone = started.elapsed();
    let started = Instant::now();
    super::super::tap_normalized(&bench.road.sdk, "emulator-5554", 0.5, 0.25)
        .expect("an emulator tap");
    let emulator = started.elapsed();
    eprintln!(
        "t-93824 fake adb tap: phone {} ms (settle included), emulator send {} ms",
        phone.as_millis(),
        emulator.as_millis()
    );
}

/// Review t-94083 P2-2: `phone-check` read the lock of a phone the person had
/// not allowed (`shell dumpsys window policy`). Before the answer only the
/// device list is read.
#[test]
fn phone_check_before_the_persons_answer_asks_the_phone_nothing() {
    let bench = Bench::new(false);
    let handle = Bench::handle();
    let checked = check_on(&bench.road, &handle);
    assert_eq!(
        bench.to_the_phone(),
        Vec::<String>::new(),
        "the phone was asked"
    );
    assert_eq!(bench.commands(), vec!["devices -l".to_string()]);
    assert_eq!(checked["ready"], false);
    let items = checked["items"].as_array().cloned().unwrap_or_default();
    let item = |name: &str| {
        items
            .iter()
            .find(|item| item["name"] == name)
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(item("consent")["ok"], false);
    assert_eq!(item("connected")["ok"], true);
    assert_eq!(item("phoneLocked")["ok"], Value::Null, "{checked}");

    let allowed = Bench::new(true);
    let _ = check_on(&allowed.road, &handle);
    assert_eq!(
        allowed.to_the_phone(),
        vec!["shell dumpsys window policy".to_string()],
        "once allowed, the lock is read"
    );
}

/// The tree, display and screen reads (`accessibility::run`) take the phone's
/// list too: a command off it spends no process on a real serial, and an
/// emulator's goes as it always did.
#[test]
fn a_read_off_the_list_spends_no_process_on_a_real_phone() {
    let bench = Bench::new(true);
    let adb = bench.road.sdk.adb.clone();
    for args in [
        vec!["-s", SERIAL, "shell", "reboot"],
        vec!["-s", SERIAL, "emu", "avd", "name"],
        vec!["-s", SERIAL, "shell", "wm", "size"],
        vec!["-s", SERIAL, "exec-out", "cat", "/data/data/com.example/db"],
    ] {
        assert!(
            super::super::accessibility::run(&adb, &args, None).is_err(),
            "{args:?}"
        );
    }
    assert_eq!(bench.commands(), Vec::<String>::new(), "adb was asked");
    super::super::accessibility::run(
        &adb,
        &["-s", "emulator-5554", "shell", "dumpsys", "input"],
        None,
    )
    .expect("an emulator's read");
    assert_eq!(
        bench.commands(),
        vec!["-s emulator-5554 shell dumpsys input".to_string()]
    );
}

/// A transfer's confirm: an amount and a recipient on the screen, a back arrow
/// and a 「보내기」 that names no money itself.
const TRANSFER: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node class="android.widget.FrameLayout" package="com.example.bank" enabled="true" bounds="[0,0][1080,2400]"><node class="android.widget.ImageButton" package="com.example.bank" content-desc="뒤로" clickable="true" enabled="true" bounds="[0,80][160,240]" /><node class="android.widget.TextView" package="com.example.bank" text="받는 분 Synthetic" enabled="true" bounds="[100,600][980,700]" /><node class="android.widget.TextView" package="com.example.bank" text="50,000원" enabled="true" bounds="[100,800][980,900]" /><node class="android.widget.Button" package="com.example.bank" text="보내기" clickable="true" enabled="true" bounds="[100,2000][980,2200]" /></node></hierarchy>"#;

/// A chat: 「보내기」 with no money anywhere on the screen.
const CHAT: &str = r#"<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation="0"><node class="android.widget.FrameLayout" package="com.example.chat" enabled="true" bounds="[0,0][1080,2400]"><node class="android.widget.TextView" package="com.example.chat" text="내일 봐요 Synthetic" enabled="true" bounds="[100,600][980,700]" /><node class="android.widget.EditText" package="com.example.chat" text="좋아요" clickable="true" focused="true" enabled="true" bounds="[100,2000][760,2200]" /><node class="android.widget.Button" package="com.example.chat" text="보내기" clickable="true" enabled="true" bounds="[780,2000][980,2200]" /></node></hierarchy>"#;

fn pressed(bench: &Bench) -> Vec<String> {
    bench
        .to_the_phone()
        .into_iter()
        .filter(|line| line.starts_with("shell input"))
        .collect()
}

/// t-93824 r2 (ZO m-94307): the Android road had only the label guard, so a
/// 「보내기」 with no money word was pressed on a transfer's confirm. The real
/// iPhone's money guard reads the phone's tree now.
#[test]
fn a_transfer_screen_takes_only_a_scroll_and_a_back_and_a_chat_sends() {
    let bench = Bench::new(true);
    let handle = Bench::handle();
    let road = &bench.road;
    bench.write("tree.xml", TRANSFER);
    for (what, refused) in [
        (
            "tap on 보내기",
            tap_on(road, &handle, 0.5, 0.875).map(|_| ()),
        ),
        (
            "a short drag",
            swipe_on(road, &handle, (0.5, 0.5), (0.5, 0.45), None).map(|_| ()),
        ),
        (
            "a sideways slide",
            swipe_on(road, &handle, (0.9, 0.875), (0.1, 0.875), None).map(|_| ()),
        ),
        ("enter", button_on(road, &handle, "enter").map(|_| ())),
    ] {
        bench.forget_commands();
        let error = refused.expect_err(what);
        assert_eq!(error.code, error_code::CONFIRMATION_REQUIRED, "{what}");
        assert!(
            error.message.contains("50,000원"),
            "{what}: {}",
            error.message
        );
        assert_eq!(pressed(&bench), Vec::<String>::new(), "{what} went");
    }
    bench.forget_commands();
    swipe_on(road, &handle, (0.5, 0.8), (0.5, 0.2), None).expect("a vertical scroll");
    assert_eq!(pressed(&bench).len(), 1, "the scroll went");
    bench.forget_commands();
    tap_on(road, &handle, 0.07, 0.067).expect("the back arrow");
    assert!(
        pressed(&bench)
            .iter()
            .any(|line| line.starts_with("shell input tap")),
        "the back arrow was not pressed"
    );
    for name in ["back", "home"] {
        bench.forget_commands();
        button_on(road, &handle, name).expect(name);
        assert_eq!(pressed(&bench).len(), 1, "{name}");
    }

    bench.write("tree.xml", CHAT);
    bench.forget_commands();
    tap_on(road, &handle, 0.83, 0.875).expect("a chat's 보내기");
    assert!(
        pressed(&bench)
            .iter()
            .any(|line| line.starts_with("shell input tap")),
        "the chat's 보내기 was not pressed"
    );
    bench.forget_commands();
    button_on(road, &handle, "enter").expect("enter in a chat");
    assert_eq!(pressed(&bench), vec!["shell input keyevent 66".to_string()]);
}
