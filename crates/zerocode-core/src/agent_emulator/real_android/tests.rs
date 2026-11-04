//! The real Android road's pure half: its id, its one list of commands, its
//! lock reader. Every serial and name here is made up.

use super::*;
use crate::agent_emulator::real_phone::is_real_phone;

const SERIAL: &str = "R5CTSYNTH01";

#[test]
fn a_phone_id_is_a_real_phone_hash_and_never_an_iphones() {
    let handle = android_phone_handle(SERIAL);
    assert!(is_android_phone(&handle), "{handle}");
    assert!(
        is_real_phone(&handle),
        "an Android phone is a real phone: {handle}"
    );
    assert!(!handle.contains(SERIAL), "the serial stays out: {handle}");
    assert_eq!(handle, android_phone_handle(SERIAL), "one phone, one id");
    assert_ne!(handle, android_phone_handle("R5CTSYNTH02"));
    let iphone = phone_handle(SERIAL);
    assert!(
        !is_android_phone(&iphone),
        "an iPhone's id is hex only: {iphone}"
    );
}

#[test]
fn the_roads_own_commands_pass_and_the_emulators_never_do() {
    let dex = format!("{TEXT_HELPER_PREFIX}029899f8ee59.dex");
    let dump = format!("{TREE_DUMP_PREFIX}6f1c2b1e-0d7a-4c3e-9f61-1f2e3d4c5b6a");
    let line = format!(
        "[ -f {dex} ] || exit 98; CLASSPATH={dex} app_process / dev.zerocode.text.Insert --check; c=$?; [ $c = 0 ] || exit $c; input text 'hello'"
    );
    for passes in [
        vec!["get-state"],
        vec!["shell", "dumpsys", "window", "policy"],
        vec!["shell", "dumpsys", "input"],
        vec!["exec-out", "screencap", "-p"],
        vec!["shell", "input", "tap", "540", "1200"],
        vec!["shell", "input", "swipe", "1", "2", "3", "4", "300"],
        vec!["shell", "input", "keyevent", "3"],
        vec!["shell", "input", "keyevent", "4"],
        vec!["shell", "input", "keyevent", "66"],
        vec!["shell", "uiautomator", "dump", dump.as_str()],
        vec!["exec-out", "cat", dump.as_str()],
        vec!["shell", "rm", "-f", dump.as_str()],
        vec!["shell", "rm", "-f", dex.as_str()],
        vec!["shell", "sha256sum", dex.as_str()],
        vec!["push", "/tmp/local.dex", dex.as_str()],
        vec!["shell", line.as_str()],
    ] {
        assert_eq!(adb_refusal(&passes), None, "{passes:?}");
    }
    for (refused, code) in [
        (vec!["emu", "kill"], PHONE_EMULATOR_ONLY),
        (vec!["emu", "avd", "name"], PHONE_EMULATOR_ONLY),
        (vec!["emu", "rotate"], PHONE_EMULATOR_ONLY),
        (vec!["reboot"], PHONE_POWER_REFUSED),
        (
            vec!["shell", "input", "keyevent", "26"],
            PHONE_POWER_REFUSED,
        ),
        (
            vec!["shell", "input", "keyevent", "223"],
            PHONE_POWER_REFUSED,
        ),
        (
            vec!["shell", "input", "keyevent", "KEYCODE_POWER"],
            PHONE_POWER_REFUSED,
        ),
        (
            vec!["shell", "input", "keyevent", "KEYCODE_SLEEP"],
            PHONE_POWER_REFUSED,
        ),
        (
            vec!["shell", "svc", "power", "stayon", "true"],
            PHONE_POWER_REFUSED,
        ),
        (vec!["install", "/tmp/app.apk"], PHONE_INSTALL_REFUSED),
        (
            vec!["install-multiple", "/tmp/a.apk"],
            PHONE_INSTALL_REFUSED,
        ),
        (vec!["uninstall", "com.example.app"], PHONE_INSTALL_REFUSED),
        (
            vec!["shell", "pm", "uninstall", "com.example.app"],
            PHONE_INSTALL_REFUSED,
        ),
        (
            vec!["shell", "settings", "put", "system", "user_rotation", "1"],
            PHONE_SETTINGS_REFUSED,
        ),
        (
            vec!["shell", "wm", "size", "720x1280"],
            PHONE_SETTINGS_REFUSED,
        ),
        (
            vec!["shell", "input", "keyevent", "24"],
            PHONE_COMMAND_REFUSED,
        ),
        (
            vec!["shell", "input", "tap", "1;reboot", "2"],
            PHONE_COMMAND_REFUSED,
        ),
        (
            vec!["shell", "rm", "-f", "/sdcard/DCIM"],
            PHONE_COMMAND_REFUSED,
        ),
        (vec!["push", "/tmp/x", "/sdcard/x"], PHONE_COMMAND_REFUSED),
        (
            vec!["shell", "am start -a android.intent.action.CALL"],
            PHONE_COMMAND_REFUSED,
        ),
        (
            vec!["shell", "[ -f /sdcard/x.dex ] || exit 98; reboot"],
            PHONE_COMMAND_REFUSED,
        ),
        (
            vec!["exec-out", "cat", "/data/data/com.example/db"],
            PHONE_COMMAND_REFUSED,
        ),
        (vec![], PHONE_COMMAND_REFUSED),
    ] {
        let said = adb_refusal(&refused);
        assert_eq!(said.map(|(said, _)| said), Some(code), "{refused:?}");
        let sentence = said.map(|(_, sentence)| sentence).unwrap_or_default();
        assert!(
            sentence.starts_with(code) && sentence.contains("Nothing was sent"),
            "{sentence}"
        );
    }
}

#[test]
fn a_phone_takes_home_back_enter_and_delete_and_never_its_power_key() {
    assert_eq!(phone_key("home"), Some("3"));
    assert_eq!(phone_key("back"), Some("4"));
    assert_eq!(phone_key("enter"), Some("66"));
    assert_eq!(phone_key("del"), Some("67"));
    for name in ["power", "lock", "sleep", "volume-up", "recents"] {
        assert_eq!(phone_key(name), None, "{name}");
    }
    assert!(is_power_button("power") && is_power_button("lock"));
    assert!(!is_power_button("home"));
    for (_, code) in PHONE_KEYS {
        assert_eq!(
            adb_refusal(&["shell", "input", "keyevent", code]),
            None,
            "every button the road takes is on its list"
        );
    }
}

/// Shapes of `dumpsys window policy` (the keyguard delegate and its state
/// monitor, Android 10 and later; the older window manager's own lines).
#[test]
fn the_lock_is_read_from_the_keyguard_and_an_unread_lock_is_not_open() {
    let open = "  KeyguardServiceDelegate\n    showing=false\n    occluded=false\n    screenState=SCREEN_STATE_ON\n    interactiveState=INTERACTIVE_STATE_AWAKE\n    KeyguardStateMonitor\n      mIsShowing=false\n";
    assert_eq!(lock_in_policy_dump(open), LockRead::Open);
    let locked = open.replace("mIsShowing=false", "mIsShowing=true");
    assert_eq!(lock_in_policy_dump(&locked), LockRead::Locked);
    let shown = open.replace("showing=false", "showing=true");
    assert_eq!(lock_in_policy_dump(&shown), LockRead::Locked);
    let dark = open.replace("SCREEN_STATE_ON", "SCREEN_STATE_OFF");
    assert_eq!(
        lock_in_policy_dump(&dark),
        LockRead::Locked,
        "a dark screen is closed"
    );
    let asleep = open.replace("INTERACTIVE_STATE_AWAKE", "INTERACTIVE_STATE_SLEEP");
    assert_eq!(lock_in_policy_dump(&asleep), LockRead::Locked);
    assert_eq!(
        lock_in_policy_dump("mShowingLockscreen=true mDreamingLockscreen=false"),
        LockRead::Locked
    );
    assert_eq!(
        lock_in_policy_dump("mShowingLockscreen=false mDreamingLockscreen=false"),
        LockRead::Open
    );
    for unread in [
        "",
        "Can't find service: window",
        "WINDOW MANAGER POLICY STATE\n",
    ] {
        assert_eq!(lock_in_policy_dump(unread), LockRead::Unread, "{unread:?}");
    }
    assert!(locked_sentence(LockRead::Unread).contains("treated as locked"));
    assert!(locked_sentence(LockRead::Locked).starts_with(PHONE_LOCKED));
}

#[test]
fn the_cards_say_what_goes_onto_the_phone_and_what_stays_the_persons() {
    let card = consent_sentence("Synthetic Pixel");
    for words in [
        "Synthetic Pixel",
        "/data/local/tmp",
        "no app is installed",
        "lock it to stop them",
        "power key",
        "password",
        "payment",
        "phone-remove",
    ] {
        assert!(
            card.contains(words),
            "the card never says `{words}`: {card}"
        );
    }
    assert!(remove_sentence("Synthetic Pixel").contains("text helper"));
    assert!(not_allowed_sentence("phone-android-0123456789ab").contains("phone-install"));
    assert!(unauthorized_sentence().starts_with(PHONE_UNAUTHORIZED));
    assert!(unreachable_sentence("offline").contains("offline"));
    assert!(secure_field_sentence().starts_with(PHONE_SECURE_FIELD));
}

/// The typing line, as `type_line` builds it for a real phone, in each of its
/// two shapes.
fn keys_line(dex: &str, word: &str) -> String {
    format!(
        "[ -f {dex} ] || exit 98; CLASSPATH={dex} app_process / dev.zerocode.text.Insert --check; c=$?; [ $c = 0 ] || exit $c; input text {word}"
    )
}

fn insert_line(dex: &str, encoded: &str) -> String {
    format!(
        "[ -f {dex} ] || exit 98; CLASSPATH={dex} app_process / dev.zerocode.text.Insert {encoded}"
    )
}

/// Review t-94083 P2-1: the line was let through on its first words, so the
/// real helper's path with `; reboot` after it passed.
#[test]
fn the_typing_line_passes_in_its_two_shapes_and_nothing_rides_after_them() {
    let dex = format!("{TEXT_HELPER_PREFIX}029899f8ee59.dex");
    let shell = |line: &str| adb_refusal(&["shell", line]).map(|(code, _)| code);
    // What the shell's `shell_word` makes of `a;b&c|$x`>y<z` and of `it's`:
    // the characters are the typed text's own inside the quotes.
    for word in [
        "'hello'",
        "'a;b&c|$x`>y<z'",
        r"'it'\''s'",
        r"''\'''",
        "'%s%s'",
        "''",
    ] {
        assert_eq!(shell(&keys_line(&dex, word)), None, "{word}");
    }
    for encoded in ["7JWI64WV7ZWY7IS47JqU", "YQ==", "a+b/"] {
        assert_eq!(shell(&insert_line(&dex, encoded)), None, "{encoded}");
    }
    let refused = [
        format!("[ -f {dex} ] || exit 98; reboot"),
        format!("[ -f {dex} ] || exit 98; settings put system user_rotation 1"),
        format!("[ -f {dex} ] || exit 98; pm uninstall com.example.app"),
        format!("{}; reboot", keys_line(&dex, "'hello'")),
        format!("{} && reboot", keys_line(&dex, "'hello'")),
        format!("{}\nreboot", keys_line(&dex, "'hello'")),
        keys_line(&dex, "hello"),
        keys_line(&dex, "'hello"),
        keys_line(&dex, "'a' ; reboot ; 'b'"),
        keys_line(&dex, "'a'$(reboot)'b'"),
        keys_line(&dex, "'a'`reboot`'b'"),
        keys_line(&dex, "'a'|reboot"),
        keys_line(&dex, "'a'>/sdcard/x"),
        keys_line(&dex, "'a\nb'"),
        format!("{}; reboot", insert_line(&dex, "YQ==")),
        insert_line(&dex, "YQ==|reboot"),
        insert_line(&dex, "$(reboot)"),
        insert_line(&dex, ""),
        format!(
            "[ -f {dex} ] || exit 98; CLASSPATH=/sdcard/x.dex app_process / dev.zerocode.text.Insert YQ=="
        ),
        format!(
            "[ -f {dex} ] || exit 97; CLASSPATH={dex} app_process / dev.zerocode.text.Insert YQ=="
        ),
        format!("[ -f {dex} ] || exit 98; CLASSPATH={dex} app_process / com.example.Other YQ=="),
        format!(
            "[ -f {dex} ] || exit 98; CLASSPATH={dex} app_process / dev.zerocode.text.Insert --check; input text 'a'"
        ),
    ];
    for line in &refused {
        assert_eq!(shell(line), Some(PHONE_COMMAND_REFUSED), "{line:?}");
    }
}

/// Review t-94083 P3: a bare `showing=false` from another part of the dump
/// opened a phone whose keyguard said nothing, and a dream counted as open.
#[test]
fn only_the_keyguards_own_word_opens_a_phone_and_a_dream_is_closed() {
    assert_eq!(
        lock_in_policy_dump(
            "  SomethingElse\n    showing=false\n    screenState=SCREEN_STATE_ON\n"
        ),
        LockRead::Unread
    );
    let open = "  KeyguardServiceDelegate\n    showing=false\n    dreaming=false\n    screenState=SCREEN_STATE_ON\n    KeyguardStateMonitor\n      mIsShowing=false\n";
    assert_eq!(lock_in_policy_dump(open), LockRead::Open);
    assert_eq!(
        lock_in_policy_dump(&open.replace("dreaming=false", "dreaming=true")),
        LockRead::Locked
    );
}

/// A transfer's confirm as the window exports a phone's dump (1080 by 2400).
fn transfer_dump() -> serde_json::Value {
    let node = |class: &str, text: Option<&str>, desc: Option<&str>, bounds: [i32; 4]| {
        let mut node = serde_json::json!({
            "className": class,
            "clickable": class.ends_with("Button"),
            "bounds": { "left": bounds[0], "top": bounds[1], "right": bounds[2], "bottom": bounds[3] },
            "children": [],
        });
        if let Some(text) = text {
            node["text"] = serde_json::json!(text);
        }
        if let Some(desc) = desc {
            node["contentDesc"] = serde_json::json!(desc);
        }
        node
    };
    serde_json::json!({
        "children": [{
            "className": "android.widget.FrameLayout",
            "bounds": { "left": 0, "top": 0, "right": 1080, "bottom": 2400 },
            "children": [
                node("android.widget.ImageButton", None, Some("뒤로"), [0, 80, 160, 240]),
                node("android.widget.TextView", Some("받는 분 Synthetic"), None, [100, 600, 980, 700]),
                node("android.widget.TextView", Some("50,000원"), None, [100, 800, 980, 900]),
                node("android.widget.Button", Some("보내기"), None, [100, 2000, 980, 2200]),
            ],
        }],
    })
}

/// t-93824 r2: a phone's dump is read by the real iPhone's money guard, so a
/// 「보내기」 that names no money is the person's where an amount is shown.
#[test]
fn a_phones_dump_is_read_by_the_iphones_money_guard() {
    use crate::agent_emulator::real_phone::{MoneyAct, money_refusal};
    let tree = money_tree(&transfer_dump(), 1080.0, 2400.0);
    assert_eq!(tree["frame"]["height"], 2400.0);
    let back = &tree["children"][0]["children"][0];
    assert_eq!(back["type"], "Button");
    assert_eq!(back["AXValue"], "뒤로");
    assert_eq!(back["frame"]["width"], 160.0);
    for act in [
        MoneyAct::Press(540.0, 2100.0),
        MoneyAct::Press(540.0, 650.0),
        MoneyAct::Swipe {
            from: (540.0, 1200.0),
            to: (540.0, 1080.0),
        },
        MoneyAct::Swipe {
            from: (970.0, 2100.0),
            to: (110.0, 2100.0),
        },
        MoneyAct::Enter,
    ] {
        assert_eq!(
            money_refusal(&tree, act).as_deref(),
            Some("50,000원"),
            "{act:?}"
        );
    }
    for act in [
        MoneyAct::Press(80.0, 160.0),
        MoneyAct::Swipe {
            from: (540.0, 1900.0),
            to: (540.0, 500.0),
        },
    ] {
        assert_eq!(money_refusal(&tree, act), None, "{act:?}");
    }
    let mut chat = transfer_dump();
    chat["children"][0]["children"][2]["text"] = serde_json::json!("내일 봐요");
    let chat = money_tree(&chat, 1080.0, 2400.0);
    assert_eq!(money_refusal(&chat, MoneyAct::Press(540.0, 2100.0)), None);
    assert_eq!(money_refusal(&chat, MoneyAct::Enter), None);
}
