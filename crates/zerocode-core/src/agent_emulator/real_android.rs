//! The pure half of driving a real Android phone over `adb` (t-93824): the id,
//! the words, the reader of the phone's lock, and the one list of what may be
//! sent to a phone at all.
//!
//! The input and screen roads of an emulator (`adb shell input`, `exec-out
//! screencap`, `uiautomator`, the text helper) are `adb`'s own and run on a phone
//! on a cable just the same. What an emulator has and a phone must never get —
//! the `emu` console, a boot, a shutdown, an idle reclaim, a rotation written
//! into its settings, its power key, an app installed or removed — is kept out by
//! [`adb_refusal`], which every command for a real phone passes before it runs.
//!
//! The road follows the real iPhone's (`real_phone`): off until the person's
//! answer on the window's card for that one phone, sealed the same way and kept
//! in the same list; a locked phone is refused before anything is read or
//! pressed; a password field is the person's; a payment or a transfer is the
//! person's whatever the desktop setting says. Running `adb` is the window's
//! (`emulator::android::phone`); nothing here starts a process.

use serde_json::{Value, json};

use super::real_phone::{
    LABEL_KEY, PHONE_LOCKED, PHONE_NOT_ALLOWED, PHONE_SECURE_FIELD, PHONE_UNREACHABLE,
    REAL_PHONE_PREFIX, VALUE_KEY, phone_handle,
};

/// What a real Android phone's id starts with on `--device`. It starts with
/// [`REAL_PHONE_PREFIX`] — it is a real phone, under the person's same answers —
/// and no iPhone's id can start with it, because an iPhone's carries only hex
/// digits after that prefix.
pub const ANDROID_PHONE_PREFIX: &str = "phone-android-";

/// The id `--device` takes for the phone `adb` lists under `serial`: a hash, so
/// the serial (a hardware identifier) does not reach the agent's context. A
/// phone on wireless debugging is the same phone it is on a cable
/// ([`phone_identity`]), so its id — and the person's answer kept under it —
/// does not change with the way it is connected.
#[must_use]
pub fn android_phone_handle(serial: &str) -> String {
    let hashed = phone_handle(phone_identity(serial));
    format!(
        "{ANDROID_PHONE_PREFIX}{}",
        &hashed[REAL_PHONE_PREFIX.len()..]
    )
}

/// The service a phone's wireless debugging offers its connection under, and
/// the one it offers pairing under while 「페어링 코드로 기기 페어링」 is open on
/// it (T6, 2026-11-09). `adb` names a phone it connected through the first
/// `adb-<the phone's serial>-<a random word>._adb-tls-connect._tcp`.
pub const WIRELESS_CONNECT_SERVICE: &str = "_adb-tls-connect._tcp";
pub const WIRELESS_PAIRING_SERVICE: &str = "_adb-tls-pairing._tcp";

/// The phone's own serial in an `adb-<serial>-<random>` name its wireless
/// debugging announces (with or without the service after it), or `None` for a
/// name of any other shape.
#[must_use]
pub fn mdns_serial(name: &str) -> Option<&str> {
    let instance = name
        .strip_suffix(&format!(".{WIRELESS_CONNECT_SERVICE}"))
        .unwrap_or(name);
    let (serial, random) = instance.strip_prefix("adb-")?.rsplit_once('-')?;
    (!serial.is_empty()
        && !random.is_empty()
        && serial
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))
    .then_some(serial)
}

/// Which phone a serial `adb` lists stands for: the phone's own serial for a
/// wireless connection `adb` found by mDNS ([`mdns_serial`]) — the serial a
/// cable shows for the same phone — and any other serial as it stands. Read off
/// the name alone: the phone is asked nothing before its person's answer.
#[must_use]
pub fn phone_identity(serial: &str) -> &str {
    if serial.ends_with(WIRELESS_CONNECT_SERVICE) {
        mdns_serial(serial).unwrap_or(serial)
    } else {
        serial
    }
}

/// One service `adb mdns services` found: the phone's announced name, whether it
/// offers pairing (or its connection), and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdnsService {
    pub name: String,
    pub pairing: bool,
    pub addr: String,
}

/// Whether `addr` is a `host:port` this window would pass to `adb pair` or
/// `adb connect`: an IPv4 or bracketed IPv6 host (what mDNS answers) and a port.
#[must_use]
pub fn wireless_addr_ok(addr: &str) -> bool {
    let Some((host, port)) = addr.rsplit_once(':') else {
        return false;
    };
    let port_ok = !port.is_empty()
        && port.bytes().all(|byte| byte.is_ascii_digit())
        && port
            .parse::<u32>()
            .is_ok_and(|number| (1..=65_535).contains(&number));
    let host = host
        .strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host);
    port_ok
        && !host.is_empty()
        && host
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'.' || byte == b':')
}

/// The pairing and connection services in `adb mdns services`'s answer; every
/// other line — its header, another service — is not one.
#[must_use]
pub fn parse_mdns_services(listing: &str) -> Vec<MdnsService> {
    listing
        .lines()
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let name = columns.next()?;
            let service = columns.next()?.trim_end_matches('.');
            let addr = columns.next()?;
            let pairing = match service {
                WIRELESS_PAIRING_SERVICE => true,
                WIRELESS_CONNECT_SERVICE => false,
                _ => return None,
            };
            (wireless_addr_ok(addr) && mdns_serial(name).is_some()).then(|| MdnsService {
                name: name.to_string(),
                pairing,
                addr: addr.to_string(),
            })
        })
        .collect()
}

/// Whether `code` is the six-digit code a phone shows for pairing.
#[must_use]
pub fn pairing_code_ok(code: &str) -> bool {
    code.len() == 6 && code.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `adb pair`'s answer says it paired.
#[must_use]
pub fn paired(answer: &str) -> bool {
    answer.contains("Successfully paired")
}

/// Whether `--device` names a real Android phone.
#[must_use]
pub fn is_android_phone(device: &str) -> bool {
    device.starts_with(ANDROID_PHONE_PREFIX)
}

/// The road every act on a real Android phone answers under `road`.
pub const ROAD_ADB: &str = "adb";

/// What the person's seal takes where an iPhone's takes its signing team: an
/// Android phone is signed by nobody, so its answer is sealed under this word,
/// and an answer about an iPhone's team never takes it back.
pub const SEAL_TEAM: &str = "android-adb";

/// The refusals of this road beside the iPhone's (`phone_not_allowed`,
/// `phone_locked`, `phone_secure_field`, `phone_unreachable`), as `code`.
pub const PHONE_UNAUTHORIZED: &str = "phone_unauthorized";
pub const PHONE_EMULATOR_ONLY: &str = "phone_emulator_only";
pub const PHONE_SETTINGS_REFUSED: &str = "phone_settings_refused";
pub const PHONE_POWER_REFUSED: &str = "phone_power_refused";
pub const PHONE_INSTALL_REFUSED: &str = "phone_install_refused";
pub const PHONE_COMMAND_REFUSED: &str = "phone_command_refused";

/// The buttons a real Android phone takes, with their key codes: home, back,
/// enter, and delete to take back what was typed. Its power, lock, sleep and
/// volume keys are not among them.
pub const PHONE_KEYS: [(&str, &str); 4] =
    [("home", "3"), ("back", "4"), ("enter", "66"), ("del", "67")];

/// The keep-awake session's poke (T7, 2026-11-09): a key no app is sent and no
/// screen answers, which a phone still counts as its person's activity, so its
/// screen does not time out while a long piece of work on it waits. Only a
/// session the person starts in the window sends it — no verb names it — and
/// only to an open phone: waking, locking or unlocking a phone stays the
/// person's, and its settings are never written to keep it on.
pub const AWAKE_POKE: &str = "KEYCODE_UNKNOWN";

/// How often a keep-awake session pokes: under the shortest screen timeout
/// Android offers (30 s).
pub const AWAKE_POKE_EVERY_SECS: u64 = 25;
// A poke comes before the shortest screen timeout Android offers (30 s).
const _: () = assert!(AWAKE_POKE_EVERY_SECS < 30);

/// The lengths a person keeps a phone awake for, in minutes; the session ends
/// by itself when its time is up.
pub const AWAKE_MINUTES: [u32; 3] = [15, 30, 60];

/// A scroll's finger phases as `input motionevent` names them: a phone takes
/// these three and no other (no CANCEL, no second pointer).
pub const SCROLL_PHASES: [&str; 3] = ["DOWN", "MOVE", "UP"];

/// Where `uiautomator dump` writes the tree when it is asked for it in one
/// round trip: the reply itself, read by `adb exec-out`. The words it prints
/// after the tree are cut off by the reader.
pub const TREE_DUMP_TO_REPLY: &str = "/dev/tty";

/// Where this window's text helper lives on a phone (`/data/local/tmp`, the one
/// folder `adb` may write without an app). The window adds the helper's hash
/// and `.dex`.
pub const TEXT_HELPER_PREFIX: &str = "/data/local/tmp/zerocode-text-";
const TEXT_HELPER_SUFFIX: &str = ".dex";
/// The class the text helper runs (`android_text/dev/zerocode/text/Insert.java`).
pub const TEXT_HELPER_CLASS: &str = "dev.zerocode.text.Insert";
/// The typing line's own exit when the phone has no copy of the helper.
pub const TEXT_HELPER_MISSING_EXIT: i32 = 98;
/// What follows the helper's `--check` on a real phone: any answer but 0 (no
/// field has the focus, or it is a password field) ends the line before a key
/// is typed.
pub const TEXT_FOCUS_CHECK: &str = "c=$?; [ $c = 0 ] || exit $c";
/// Where a tree is dumped on a phone before it is read; the window adds a
/// random suffix per read.
pub const TREE_DUMP_PREFIX: &str = "/sdcard/window_dump.xml.";

/// The key codes and names that wake, sleep, lock or turn off a phone.
const POWER_KEYS: [&str; 7] = ["26", "223", "224", "276", "POWER", "SLEEP", "WAKEUP"];

/// The key code of a button a real phone takes; `None` for any other name.
#[must_use]
pub fn phone_key(name: &str) -> Option<&'static str> {
    PHONE_KEYS
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, code)| *code)
}

/// Whether a button's name is a power, lock or sleep key.
#[must_use]
pub fn is_power_button(name: &str) -> bool {
    matches!(
        name,
        "power" | "lock" | "sleep" | "wakeup" | "wake" | "screen-off"
    )
}

fn is_number(word: &str) -> bool {
    !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_text_helper(path: &str) -> bool {
    path.strip_prefix(TEXT_HELPER_PREFIX)
        .and_then(|rest| rest.strip_suffix(TEXT_HELPER_SUFFIX))
        .is_some_and(|hash| !hash.is_empty() && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn is_tree_dump(path: &str) -> bool {
    path.strip_prefix(TREE_DUMP_PREFIX).is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    })
}

/// The one `adb shell` line a real phone takes: the text helper's typing line,
/// in exactly one of its two shapes and nothing more —
///
/// - keys: `[ -f <helper> ] || exit 98; CLASSPATH=<helper> app_process /
///   <class> --check; c=$?; [ $c = 0 ] || exit $c; input text '<one word>'`
/// - accessibility: `… app_process / <class> <base64>`
///
/// A tail after either (`; reboot`), an unquoted shell character, or a quote
/// that does not close where the word ends is refused. Inside the quoted word
/// `;`, `&`, `|`, `$` and the rest are the typed text's own.
fn is_text_line(line: &str) -> bool {
    let Some((path, rest)) = line
        .strip_prefix("[ -f ")
        .and_then(|rest| rest.split_once(' '))
    else {
        return false;
    };
    if !is_text_helper(path) {
        return false;
    }
    let head = format!(
        "] || exit {TEXT_HELPER_MISSING_EXIT}; CLASSPATH={path} app_process / {TEXT_HELPER_CLASS} "
    );
    let Some(tail) = rest.strip_prefix(head.as_str()) else {
        return false;
    };
    let keys = format!("--check; {TEXT_FOCUS_CHECK}; input text ");
    match tail.strip_prefix(keys.as_str()) {
        Some(word) => is_quoted_word(word),
        None => is_base64(tail),
    }
}

/// One single-quoted word as `shell_word` writes it: printable ASCII between
/// two quotes, a quote inside only as `'\''`.
fn is_quoted_word(word: &str) -> bool {
    word.len() >= 2
        && word.bytes().all(|byte| (b' '..=b'~').contains(&byte))
        && word
            .strip_prefix('\'')
            .and_then(|inner| inner.strip_suffix('\''))
            .is_some_and(|inner| inner.split(r"'\''").all(|piece| !piece.contains('\'')))
}

fn is_base64(text: &str) -> bool {
    !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

/// Why `args` (what follows `-s <serial>`) may not go to a real phone: the code
/// and the sentence, or `None` for a command on this road's one list. The list
/// names what the road sends — the input and screen reads, the lock read, the
/// tree dump and its cleanup, the text helper and its copy — and nothing else
/// goes, so a command added anywhere later is refused until it is named here.
#[must_use]
pub fn adb_refusal(args: &[&str]) -> Option<(&'static str, &'static str)> {
    let allowed = match args {
        ["get-state"]
        | ["shell", "dumpsys", "window", "policy"]
        | ["shell", "dumpsys", "input"]
        | ["exec-out", "screencap", "-p"] => true,
        ["shell", "input", "tap", x, y] => is_number(x) && is_number(y),
        ["shell", "input", "swipe", x1, y1, x2, y2, ms] => {
            [x1, y1, x2, y2, ms].into_iter().all(|word| is_number(word))
        }
        // A scroll's one finger, a phase at a time (`scroll`): down, moves, up —
        // each its own line, never a chain the phone's shell would run whole.
        ["shell", "input", "motionevent", phase, x, y] => {
            SCROLL_PHASES.contains(phase) && is_number(x) && is_number(y)
        }
        ["shell", "input", "keyevent", code] => {
            PHONE_KEYS.iter().any(|(_, known)| known == code) || *code == AWAKE_POKE
        }
        // The tree in one round trip, written to the reply instead of a file.
        ["exec-out", "uiautomator", "dump", TREE_DUMP_TO_REPLY] => true,
        ["shell", "uiautomator", "dump", path] | ["exec-out", "cat", path] => is_tree_dump(path),
        ["shell", "rm", "-f", path] => is_tree_dump(path) || is_text_helper(path),
        ["shell", "sha256sum", path] => is_text_helper(path),
        ["push", _local, path] => is_text_helper(path),
        ["shell", line] => is_text_line(line),
        _ => false,
    };
    (!allowed).then(|| refusal_of(args))
}

fn refusal_of(args: &[&str]) -> (&'static str, &'static str) {
    let first = args.first().copied().unwrap_or_default();
    let shell = |word: &str| first == "shell" && args.get(1) == Some(&word);
    let names_power = args.iter().any(|word| {
        let upper = word.to_ascii_uppercase();
        POWER_KEYS
            .iter()
            .any(|key| upper == *key || upper == format!("KEYCODE_{key}"))
    });
    if first == "emu" {
        (
            PHONE_EMULATOR_ONLY,
            "phone_emulator_only: the emulator console (and a boot, a shutdown or an idle reclaim) is an emulator's; a real phone is never sent one. Nothing was sent.",
        )
    } else if first == "reboot"
        || shell("reboot")
        || shell("svc")
        || (shell("input") && names_power)
    {
        (
            PHONE_POWER_REFUSED,
            "phone_power_refused: a real phone's power, lock and sleep are the person's; this window never presses them. Nothing was sent.",
        )
    } else if first.starts_with("install") || first == "uninstall" || shell("pm") {
        (
            PHONE_INSTALL_REFUSED,
            "phone_install_refused: installing or removing an app on a real phone is the person's. Nothing was sent.",
        )
    } else if shell("settings") || shell("wm") || shell("cmd") || shell("setprop") {
        (
            PHONE_SETTINGS_REFUSED,
            "phone_settings_refused: a real phone's settings (its rotation among them) are the person's; this window never writes them. Nothing was sent.",
        )
    } else {
        (
            PHONE_COMMAND_REFUSED,
            "phone_command_refused: this command is not one a real phone's road sends. Nothing was sent.",
        )
    }
}

/// A phone's tree as the window exports a `uiautomator` dump (`className`,
/// `text`, `contentDesc`, `bounds`, `clickable`, `children`) in the shape the
/// real iPhone's money guard reads (`real_phone::money_refusal`: `type`,
/// `AXLabel`, `AXValue`, `frame`), so one guard judges both phones. A button,
/// a clickable view and a field are controls by the guard's own roles; the root
/// is the screen, `width` by `height` at the dump's own pixels, where a press
/// and a drag are given.
#[must_use]
pub fn money_tree(android: &Value, width: f64, height: f64) -> Value {
    let mut root = money_node(android);
    root["frame"] = json!({ "x": 0.0, "y": 0.0, "width": width, "height": height });
    root
}

fn money_node(node: &Value) -> Value {
    let class = node["className"].as_str().unwrap_or_default();
    let short = class.rsplit('.').next().unwrap_or_default();
    let kind = match short {
        "EditText" | "AutoCompleteTextView" | "MultiAutoCompleteTextView" => "TextField",
        "CheckBox" | "Switch" | "SwitchCompat" | "ToggleButton" | "CheckedTextView" => "CheckBox",
        "RadioButton" => "RadioButton",
        "SeekBar" | "RatingBar" => "Slider",
        _ if node["clickable"] == true || short.ends_with("Button") => "Button",
        _ => "Other",
    };
    let bound = |key: &str| node["bounds"][key].as_f64().unwrap_or_default();
    let (left, top) = (bound("left"), bound("top"));
    let children: Vec<Value> = node["children"]
        .as_array()
        .into_iter()
        .flatten()
        .map(money_node)
        .collect();
    let mut out = json!({
        "type": kind,
        "frame": {
            "x": left,
            "y": top,
            "width": (bound("right") - left).max(0.0),
            "height": (bound("bottom") - top).max(0.0),
        },
        "children": children,
    });
    for (key, from) in [(LABEL_KEY, "text"), (VALUE_KEY, "contentDesc")] {
        if let Some(words) = node[from].as_str() {
            out[key] = json!(words);
        }
    }
    out
}

/// What a phone's own window manager says of its lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockRead {
    /// The keyguard shows, or the screen is off or asleep.
    Locked,
    /// The keyguard says it does not show and the screen is on.
    Open,
    /// Nothing this reader knows was said: refused as if locked.
    Unread,
}

/// `dumpsys window policy` and whether the phone is open to a press. The
/// keyguard delegate prints `showing=` and `dreaming=`, its state monitor
/// `mIsShowing=`, older phones `mShowingLockscreen=` and `mDreamingLockscreen=`;
/// the delegate also prints the screen's state. Any of them saying locked, a
/// dream (a screen saver or an always-on display) or the screen off, wins. A
/// phone counts as open only when a keyguard's own name for it (`mIsShowing`,
/// `mShowingLockscreen`) said `false` and nothing said locked: a bare
/// `showing=false` may be any part of the dump's, so it opens nothing.
#[must_use]
pub fn lock_in_policy_dump(dump: &str) -> LockRead {
    let mut said_open = false;
    for token in dump.split_whitespace() {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };
        match (key, value) {
            (
                "showing"
                | "mIsShowing"
                | "mShowingLockscreen"
                | "mDreamingLockscreen"
                | "isStatusBarKeyguard"
                | "dreaming",
                "true",
            )
            | ("screenState", "SCREEN_STATE_OFF" | "SCREEN_STATE_TURNING_OFF")
            | (
                "interactiveState",
                "INTERACTIVE_STATE_SLEEP" | "INTERACTIVE_STATE_GOING_TO_SLEEP",
            )
            | ("mAwake", "false") => return LockRead::Locked,
            ("mIsShowing" | "mShowingLockscreen", "false") => said_open = true,
            _ => {}
        }
    }
    if said_open {
        LockRead::Open
    } else {
        LockRead::Unread
    }
}

/// Why a locked phone, or one whose lock could not be read, was refused.
#[must_use]
pub fn locked_sentence(read: LockRead) -> String {
    let what = if read == LockRead::Unread {
        "this window could not tell whether the phone is locked, so it is treated as locked"
    } else {
        "the phone is locked or its screen is off"
    };
    format!(
        "{PHONE_LOCKED}: {what}; nothing was read or pressed. Only the person can unlock it: this window never unlocks a phone or presses its power key. Pass this on to the person and stop."
    )
}

/// What the agent hears when the person has not allowed this phone.
#[must_use]
pub fn not_allowed_sentence(handle: &str) -> String {
    format!(
        "{PHONE_NOT_ALLOWED}: the person has not allowed driving this Android phone. Run `zerocode-emulator phone-install --device {handle}`: the window puts a card to the person, and only their answer turns it on. If nobody answers, tell the person and stop."
    )
}

/// A phone on the cable that has not allowed this computer's debugging.
#[must_use]
pub fn unauthorized_sentence() -> String {
    format!(
        "{PHONE_UNAUTHORIZED}: the phone is connected but has not allowed USB debugging from this computer. Only the person can press Allow on the phone's own prompt (and turn on USB debugging in its Developer options). Nothing was sent."
    )
}

/// A phone `adb` lists in a state that takes no command.
#[must_use]
pub fn unreachable_sentence(state: &str) -> String {
    format!(
        "{PHONE_UNREACHABLE}: adb lists the phone as `{state}`, so nothing was sent. Check the cable or the network and ask list again."
    )
}

/// A focused password field.
#[must_use]
pub fn secure_field_sentence() -> String {
    format!(
        "{PHONE_SECURE_FIELD}: the focused field is a password field. Nothing was typed. A password, PIN or biometric is the person's; stop and tell them."
    )
}

/// The card that gives this road its consent for one phone, in the words a
/// person reads; the page's own sentence under [`CONSENT_KEY`] says the same.
#[must_use]
pub fn consent_sentence(phone: &str) -> String {
    format!(
        "An agent asks to control the real Android phone \"{phone}\" over USB or wireless debugging (adb). Allowing it copies a small text helper of ours into the phone's /data/local/tmp so typing works in every language; no app is installed. Agents can then read the screen, press, swipe and type on this phone without the Mac's pointer. They can press while you are using the phone too; lock it to stop them. They never unlock it, press its power key, change its settings, install or remove apps, or enter a password, and a payment or a transfer stays yours. This lasts for this phone until you turn it off in Settings or run phone-remove."
    )
}

/// The keys of the page's sentences for the two cards.
pub const CONSENT_KEY: &str = "emulator.realPhone.androidConsent";
pub const REMOVE_KEY: &str = "emulator.realPhone.androidRemove";

/// The sentence for [`REMOVE_KEY`].
#[must_use]
pub fn remove_sentence(phone: &str) -> String {
    format!(
        "An agent asks to remove this window's text helper from the Android phone \"{phone}\" and to turn off controlling it. Nothing else on the phone changes."
    )
}

#[cfg(test)]
mod tests;
