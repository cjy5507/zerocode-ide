//! The copies of the shared core's tables outside Rust: the macOS helper's
//! (B3) — the words that mark a secret field, the characters that move the
//! focus, the reasons an action is unverified, the refusal a password field
//! answers, the keyboard's table the window sends, the stop and key words,
//! the marks' press list, pin words and text roles (C1) — and the bench's
//! command lines (E2). A copy that drifts from the core fails
//! here, not on a person's desk.

const HELPER: &str =
    include_str!("../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/main.swift");
const RENDERING: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/SnapshotRendering.swift"
);
const KEYBOARD: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/KeyboardInputSafety.swift"
);
const OUTCOME: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/TextReplaceOutcome.swift"
);
const MARK_PIN: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/MarkPin.swift"
);
const DESKTOP_APPS: &str = include_str!(
    "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/DesktopApps.swift"
);
const IOS_BRIDGE: &str = include_str!("../../native/ios-emulator-helper/AccessibilityBridge.swift");
const IOS_HELPER: &str = include_str!("../../native/ios-emulator-helper/main.swift");
const IOS_HID: &str = include_str!("../../src/emulator/ios_hid.rs");
const IOS_CHILD_TALLY: &str = include_str!(
    "../../native/ios-emulator-helper/Sources/ZeroCodeIosEmulatorHelperCore/AccessibilityChildTally.swift"
);
const IOS_LEAF: &str = include_str!(
    "../../native/ios-emulator-helper/Sources/ZeroCodeIosEmulatorHelperCore/AccessibilityLeaf.swift"
);
const MOBILE_MARKS: &str = include_str!("../../src/emulator/marks.rs");

/// The quoted strings of the Swift array literal on the line that declares it
/// (the value after `=`, whatever the type annotation holds).
fn swift_array(source: &str, declared: &str) -> Vec<String> {
    let line = source
        .lines()
        .find(|line| line.contains(declared))
        .unwrap_or_else(|| panic!("`{declared}` is gone from the helper"));
    let value = &line[line.find('=').expect("a value") + 1..];
    let list = &value[value.find('[').expect("an array") + 1..value.rfind(']').expect("an array")];
    list.split(',')
        .map(|word| word.trim().trim_matches('"').to_string())
        .filter(|word| !word.is_empty())
        .collect()
}

/// Every `"…"` literal that follows `marker` in `source`.
fn literals_after<'a>(source: &'a str, marker: &str) -> Vec<&'a str> {
    source
        .split(marker)
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect()
}

#[test]
fn the_helpers_secret_field_words_are_the_cores() {
    use zerocode_core::computer_use_protocol::render::{
        SECURE_TEXT_PROBE_ROLE_WORDS, SECURE_TEXT_WORDS,
    };
    assert_eq!(
        swift_array(RENDERING, "static let secureTextWords ="),
        SECURE_TEXT_WORDS
    );
    assert_eq!(
        swift_array(RENDERING, "static let secureTextProbeRoleWords ="),
        SECURE_TEXT_PROBE_ROLE_WORDS
    );
}

#[test]
fn the_helpers_focus_moving_characters_are_the_cores() {
    use zerocode_core::computer_use_protocol::validate::FOCUS_MOVING_CHARS;
    let swift: Vec<char> = swift_array(KEYBOARD, "static let focusMovingScalars:")
        .iter()
        .map(|escape| match escape.as_str() {
            "\\t" => '\t',
            "\\r" => '\r',
            "\\n" => '\n',
            other => panic!("a scalar the table does not spell: {other}"),
        })
        .collect();
    assert_eq!(swift, FOCUS_MOVING_CHARS);
}

/// Every reason the helper writes into an unverified action is one the core
/// names — the words a walk judges a landing by (`value_unchanged`) and a
/// model reads.
#[test]
fn every_reason_the_helper_reports_is_the_cores() {
    use zerocode_core::computer_use_protocol::unverified_reason::ALL;
    let outcome = OUTCOME
        .split("public var unverifiedReason")
        .nth(1)
        .expect("the outcome's reasons are gone");
    let outcome = &outcome[..outcome.find("\n    }\n").unwrap_or(outcome.len())];
    let spelled: std::collections::BTreeSet<&str> = literals_after(HELPER, "reason: \"")
        .into_iter()
        .chain(literals_after(outcome, "return \""))
        .collect();
    assert!(
        spelled.len() >= 6,
        "the helper's reasons were read: {spelled:?}"
    );
    for reason in spelled {
        assert!(
            ALL.contains(&reason),
            "the helper reports `{reason}`, which the core's unverified_reason table does not name"
        );
    }
}

/// A password field's refusal is the core's code, in the words the window
/// turns into a sentence (`validate::secure_input_sentence`).
#[test]
fn a_password_field_refuses_in_the_words_the_window_reads() {
    use zerocode_core::computer_use_protocol::{error_code, unverified_reason, validate};
    assert!(
        HELPER.contains(&format!(
            "ProviderError.coded(\"{}\", message)",
            error_code::SECURE_INPUT
        )),
        "the helper refuses a secret field with the core's code"
    );
    let spelled = literals_after(HELPER, "var message = \"")
        .into_iter()
        .find(|message| message.starts_with(unverified_reason::SECRET_FIELD))
        .expect("the helper's secret-field message");
    let message = spelled.replace("\\(app)", "Safari");
    assert!(
        validate::secure_input_sentence(&message).is_some(),
        "`{spelled}` is not what the window reads"
    );
    assert!(
        HELPER.contains("message += \"; typed \\(done) of \\(total)\""),
        "the typed count is spelled the way the window reads it"
    );
    let shell = include_str!("../../src/agent_tools_runtime.rs");
    assert!(
        shell.contains("validate::refusal_sentence(&error.code, &error.message)"),
        "the window turns the helper's words into the model's sentence"
    );
    let line_break = validate::TextEntryRefusal::LineBreak.as_str();
    assert!(
        KEYBOARD.contains(&format!("case lineBreak = \"{line_break}\"")),
        "the helper refuses a line break in the core's word"
    );
    assert!(
        HELPER.contains("throw ProviderError.coded(\"invalid_argument\", \"\\(refusal.rawValue): "),
        "the helper's line-break refusal is `<word>: <app>`, as the window reads it"
    );
}

/// The helper's stop reasons are the core's words, and the person's stop it
/// can make (the chord) is one the core says only a person lifts.
#[test]
fn the_helpers_stop_reasons_are_the_cores() {
    use zerocode_core::computer_use::{
        STOP_REASON_HOTKEY, STOP_REASON_REQUEST, STOP_REASON_SESSION_BUDGET, STOP_REASON_SIGNAL,
        persons_stop,
    };
    let guard = include_str!(
        "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/OperatorGuard.swift"
    );
    for (name, word) in [
        ("hotkey", STOP_REASON_HOTKEY),
        ("signal", STOP_REASON_SIGNAL),
        ("request", STOP_REASON_REQUEST),
        ("sessionBudget", STOP_REASON_SESSION_BUDGET),
    ] {
        assert!(
            guard.contains(&format!("public static let {name} = \"{word}\"")),
            "the helper's StopReason.{name} is not the core's `{word}`"
        );
    }
    assert!(persons_stop(STOP_REASON_HOTKEY) && !persons_stop(STOP_REASON_SIGNAL));
    let settings = include_str!("../../src/cmd/settings.rs");
    assert!(
        settings.contains("guard::stop(zerocode_core::computer_use::STOP_REASON_WINDOW)"),
        "the window's stop button stops in the person's word"
    );
}

/// The hand's pace and session count are the core's alone: the window hands
/// the helper `guard_table()` at launch, the helper reads exactly that shape
/// under the core's name and words and keeps no pace of its own, and without
/// the table it acts on nothing — an unlimited pace is never quietly replaced
/// by a throttle (t-3909).
#[test]
fn the_helper_counts_by_the_windows_guard_table() {
    use zerocode_core::computer_use::{
        COMPUTER_GUARD_TABLE_ENV, COMPUTER_PACE, PACE_MODE_PACED, PACE_MODE_UNLIMITED, guard_table,
        pace_table,
    };
    let guard = include_str!(
        "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOSCore/OperatorGuard.swift"
    );
    let tests = include_str!(
        "../../native/computer-use-macos/Tests/ZeroCodeComputerUseMacOSTests/OperatorGuardTests.swift"
    );
    assert!(
        guard.contains(&format!(
            "static let windowTableEnvironmentKey = \"{COMPUTER_GUARD_TABLE_ENV}\""
        )),
        "the helper reads the table under the core's name"
    );
    for (name, word) in [
        ("unlimited", PACE_MODE_UNLIMITED),
        ("paced", PACE_MODE_PACED),
    ] {
        assert!(
            guard.contains(&format!("public static let {name} = \"{word}\"")),
            "the helper's PaceMode.{name} is not the core's `{word}`"
        );
    }
    // The helper's reader holds exactly the keys the window writes.
    let reader = guard
        .split("private struct WindowTable: Decodable {")
        .nth(1)
        .expect("the helper's table reader is gone");
    let reader = &reader[..reader.find("\n    }\n").expect("its end")];
    let read: std::collections::BTreeSet<&str> = reader
        .lines()
        .filter_map(|line| line.trim().strip_prefix("let "))
        .filter_map(|line| line.split(':').next())
        .collect();
    let table = guard_table();
    let pace = pace_table(COMPUTER_PACE);
    let written: std::collections::BTreeSet<&str> = table
        .keys()
        .chain(pace.as_object().expect("a pace is an object").keys())
        .map(String::as_str)
        .collect();
    assert_eq!(
        read, written,
        "the helper reads exactly what the window sends"
    );
    // The helper's own test reads the table the core writes today.
    let line = tests
        .lines()
        .find(|line| line.contains("static let windowTable = #\""))
        .expect("the helper's test names the window's table");
    let literal =
        &line[line.find("#\"").expect("raw string") + 2..line.rfind("\"#").expect("its end")];
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(literal).expect("the literal is json"),
        serde_json::Value::Object(table),
        "the helper's test reads the table the window sends"
    );
    // No number of the helper's own, and no table-less action.
    assert!(
        !guard.contains("static let table = ActionBudget("),
        "the helper keeps a pace table of its own again"
    );
    let host = HELPER
        .split("enum OperatorGuardHost {")
        .nth(1)
        .expect("the helper's guard host is gone");
    assert!(
        host.contains("private static var ledger = OperatorLedger(budget: nil)")
            && host.contains("environment[ActionBudget.windowTableEnvironmentKey]")
            && host.contains("case .noBudget:\n                throw ProviderError.coded(\"provider_incompatible\""),
        "the helper must count by the window's table and act on nothing without it"
    );
    assert!(
        !HELPER.contains("usleep(useconds_t(max(0, seconds)"),
        "a pace's wait must not trap on a long sleep"
    );
    // `status` answers the same table, before any helper session stands.
    let shell = include_str!("../../src/agent_tools_runtime.rs");
    assert!(
        shell.contains("\"pace\": zerocode_core::computer_use::pace_table(\n                zerocode_core::computer_use::COMPUTER_PACE\n            ),"),
        "`status` must answer the core's pace table, not numbers of its own"
    );
    // The skill tells the model the pace the table holds, in its words.
    let skill = include_str!("../../../../skills/computer-use/SKILL.md")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let words = zerocode_core::computer_use::pace_words(COMPUTER_PACE);
    assert!(
        skill.contains(&words)
            && skill.contains(&format!("`pace.mode: \"{PACE_MODE_UNLIMITED}\"`")),
        "the skill does not say the table's pace: {words}"
    );
    assert!(
        !skill.contains("then 10 a second") && !skill.contains("stopped, paced,"),
        "the skill still teaches the old speed cap"
    );
}

/// The helper names ⌘, the text modifiers and the rest with the core's own
/// modifier words, and presses with the core's pressing characters.
#[test]
fn the_helpers_key_words_are_the_cores() {
    use zerocode_core::computer_use_protocol::keys::{Modifier, PASTE_KEY, Platform};
    use zerocode_core::computer_use_protocol::validate::PRESSING_CHARS;
    let names = |declared: &str| {
        let mut names = swift_array(KEYBOARD, declared);
        names.sort();
        names
    };
    let primary = names("static let primaryModifierNames: Set<String> =");
    let text = names("static let textModifierNames: Set<String> =");
    let other = names("static let otherModifierNames: Set<String> =");
    /// Which kind of modifier the core says a name is, on macOS.
    type Kind = fn(Modifier) -> bool;
    let primary_kind: Kind = |modifier| modifier.is_primary_on(Platform::MacOs);
    let text_kind: Kind = |modifier| matches!(modifier, Modifier::Shift | Modifier::Alt);
    let other_kind: Kind = |modifier| modifier == Modifier::Control;
    for (swift, kind) in [
        (&primary, primary_kind),
        (&text, text_kind),
        (&other, other_kind),
    ] {
        for name in swift {
            let modifier =
                Modifier::parse(name).unwrap_or_else(|| panic!("`{name}` is no core modifier"));
            assert!(kind(modifier), "`{name}` is filed under the wrong kind");
        }
    }
    for name in [
        "cmdorctrl",
        "commandorcontrol",
        "cmd",
        "command",
        "meta",
        "super",
        "win",
        "ctrl",
        "control",
        "alt",
        "option",
        "shift",
    ] {
        assert!(Modifier::parse(name).is_some(), "{name}");
        assert!(
            primary
                .iter()
                .chain(&text)
                .chain(&other)
                .any(|swift| swift == name),
            "the helper does not know the core's modifier `{name}`"
        );
    }
    assert!(KEYBOARD.contains(&format!("static let pasteKey = \"{PASTE_KEY}\"")));
    let pressing: Vec<char> = swift_array(KEYBOARD, "static let pressingScalars:")
        .iter()
        .map(|escape| match escape.as_str() {
            "\\r" => '\r',
            "\\n" => '\n',
            other => panic!("a scalar the table does not spell: {other}"),
        })
        .collect();
    assert_eq!(pressing, PRESSING_CHARS);
}

/// The keyboard's table travels under one name with the fields the helper
/// reads, and the helper holds no numbers of its own.
#[test]
fn the_keyboards_table_is_sent_as_the_helper_reads_it() {
    use zerocode_core::computer_use::{KEYBOARD_GUARD_PARAM, keyboard_guard};
    let reader = HELPER
        .split("private func keyboardGuardConfig(")
        .nth(1)
        .expect("the helper's table reader is gone");
    let reader = &reader[..reader.find("\n}\n").expect("its end")];
    assert!(reader.contains(&format!("params[\"{KEYBOARD_GUARD_PARAM}\"]")));
    let read: std::collections::BTreeSet<&str> =
        literals_after(reader, "table[\"").into_iter().collect();
    let table = keyboard_guard();
    let sent: std::collections::BTreeSet<&str> = table
        .as_object()
        .expect("a table")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(read, sent, "the helper reads exactly what the window sends");
}

/// The continuous eye (V1): the helper reads exactly the table the window
/// sends with `eyeStart`, answers its methods, and speaks the core's words —
/// a drifted key would leave every wait reading an empty stream.
#[test]
fn the_eyes_table_methods_and_words_are_the_cores() {
    use zerocode_core::computer_use::eye_table;
    use zerocode_core::computer_use_protocol::eye::{
        ACT_KEY, AFTER_KEY, AT_KEY, CHANGES_KEY, CHANGES_METHOD, FRAME_METHOD, FROM_KEY,
        NOT_WATCHING, NOW_KEY, RECTS_KEY, SEQ_KEY, START_METHOD, STREAMING_KEY, WHOLE_KEY,
    };
    const EYE: &str = include_str!(
        "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/ScreenEye.swift"
    );
    let reader = HELPER
        .split("private func eyeConfig(")
        .nth(1)
        .expect("the helper's eye table reader is gone");
    let reader = &reader[..reader.find("\n}\n").expect("its end")];
    let read: std::collections::BTreeSet<&str> =
        literals_after(reader, "params[\"").into_iter().collect();
    let table = eye_table();
    let sent: std::collections::BTreeSet<&str> = table.keys().map(String::as_str).collect();
    assert_eq!(read, sent, "the helper reads exactly what the window sends");
    for method in [START_METHOD, CHANGES_METHOD, FRAME_METHOD] {
        assert!(HELPER.contains(&format!("case \"{method}\":")), "{method}");
    }
    for key in [AFTER_KEY, FROM_KEY] {
        assert!(
            HELPER.contains(&format!("optionalInteger(params, \"{key}\")")),
            "{key}"
        );
    }
    for word in [
        SEQ_KEY,
        NOW_KEY,
        ACT_KEY,
        AT_KEY,
        RECTS_KEY,
        WHOLE_KEY,
        STREAMING_KEY,
        CHANGES_KEY,
    ] {
        assert!(
            EYE.contains(&format!("\"{word}\":")) || EYE.contains(&format!("[\"{word}\"]")),
            "{word}"
        );
    }
    assert!(EYE.contains(&format!("\"{NOT_WATCHING}\"")));
}

/// What a desktop reading leaves out, and the overlay a window list names,
/// are the core's words: the window sends the region under the core's key,
/// the helper answers what it left out under the core's keys, and the cover
/// sums read the overlay under the word the helper writes it with.
#[test]
fn what_a_reading_leaves_out_and_an_overlay_are_the_cores_words() {
    use zerocode_core::computer_use_protocol::eye::{EXCLUDED_REGIONS_KEY, OWN_REGION_KEY};
    use zerocode_core::computer_use_protocol::marks::OVERLAY_KEY;
    assert!(
        HELPER.contains(&format!("params[\"{OWN_REGION_KEY}\"]")),
        "the helper reads {OWN_REGION_KEY}"
    );
    assert!(
        HELPER.contains(&format!("[\"{EXCLUDED_REGIONS_KEY}\"]")),
        "the helper answers {EXCLUDED_REGIONS_KEY}"
    );
    assert!(
        HELPER.contains(&format!("row[\"{OVERLAY_KEY}\"]")),
        "the helper writes {OVERLAY_KEY}"
    );
}

/// The bench's commands are the product's: every reference line a scenario
/// replays (loops expanded) and every command the runner makes itself parse
/// through the one CLI grammar — a bad flag or a missing value fails here,
/// not in the middle of a run on the person's desk (E2).
#[test]
fn every_bench_command_is_one_the_cli_accepts() {
    use zerocode_core::computer_use::parse_command;
    let table: serde_json::Value =
        serde_json::from_str(include_str!("../../../../tools/computer-bench/bench.json"))
            .expect("bench.json is JSON");
    let words = |line: &serde_json::Value| -> Vec<String> {
        line.as_array()
            .expect("a command line")
            .iter()
            .map(|word| word.as_str().expect("a word").to_string())
            .collect()
    };
    let mut lines: Vec<Vec<String>> = Vec::new();
    let scenarios = table["scenarios"]["value"].as_array().expect("scenarios");
    for scenario in scenarios {
        // A setup that clears a display acts: its lines are the CLI's too.
        for step in scenario["setup"].as_array().expect("a setup") {
            if step["kind"] == "clear" {
                let app = step["app"].as_str().expect("an app").to_string();
                let key = step["key"].as_str().expect("a key").to_string();
                lines.push(vec!["activate".into(), "--app".into(), app]);
                lines.push(vec!["key".into(), "--key".into(), key]);
            }
        }
        for entry in scenario["reference"].as_array().expect("a reference") {
            if let Some(name) = entry["for"].as_str() {
                let hole = format!("{{{name}}}");
                for value in entry["in"].as_array().expect("values") {
                    let value = value.as_str().expect("a value");
                    for line in entry["do"].as_array().expect("lines") {
                        lines.push(
                            words(line)
                                .iter()
                                .map(|word| word.replace(&hole, value))
                                .collect(),
                        );
                    }
                }
            } else {
                lines.push(words(entry));
            }
        }
    }
    // The runner's own shapes (scenarios.py setup/oracle/teardown, bench.py).
    let setup_ms = table["setup_timeout_ms"]["value"].to_string();
    let quit_ms = table["quit_wait_ms"]["value"].to_string();
    let last = table["flush_last"]["value"].to_string();
    for line in [
        vec![
            "wait-for",
            "--app",
            "TextEdit",
            "--window",
            "note.txt",
            "--timeout-ms",
            &setup_ms,
        ],
        vec![
            "wait-for",
            "--app",
            "TextEdit",
            "--window",
            "note.txt",
            "--absent",
            "--timeout-ms",
            &quit_ms,
        ],
        // Both places a walk may name, because the usage offers both and the
        // gate took only one: `--pane` was refused, so the seat that judges a
        // browser walk was unreachable and its ledger stayed empty
        // (2026-09-19).
        vec!["walk", "--goal", "pay", "--app", "Safari"],
        vec!["walk", "--goal", "pay", "--pane", "browser-13"],
        vec!["read", "--app", "Calculator"],
        vec!["read", "--app", "Calculator", "--ocr"],
        vec!["quit", "--app", "TextEdit"],
        vec!["quit", "--app", "TextEdit", "--force"],
        vec!["window-close", "--id", "7"],
        vec!["list-all-windows"],
        vec!["status"],
        vec!["permissions"],
        vec!["screenshot"],
        vec!["displays"],
        vec!["capabilities"],
        vec!["evidence", "--last", &last],
    ] {
        lines.push(line.into_iter().map(str::to_string).collect());
    }
    assert!(
        lines.len() > 40,
        "the table's lines were read: {}",
        lines.len()
    );
    for mut line in lines {
        line.push("--json".into());
        if let Err(why) = parse_command(&line) {
            panic!("`{}` is refused by the CLI: {why}", line.join(" "));
        }
    }
}

/// An answer a program reads carries the fence's flag, not its marker lines.
///
/// The fence is a label for a model; a `--json` answer has a program for a
/// reader, and `untrusted::JSON_FLAG` exists so such an answer can say where
/// its words came from and still parse. `marks --json` wore the markers, and
/// the browser walk — which looks at a pane by running exactly that and
/// parsing what comes back — read every look as nothing. Every walk ended at
/// its first step and the seat that judges one has no rows to show for it
/// (2026-09-19).
#[test]
fn a_json_answer_a_program_reads_wears_the_flag_and_not_the_markers() {
    let browser = std::fs::read_to_string("src/cmd/browser.rs").expect("the browser commands");
    let marks = browser
        .split("pub(crate) fn marks_json")
        .nth(1)
        .and_then(|rest| rest.split("\n}").next())
        .expect("marks_json");
    assert!(
        marks.contains("untrusted::JSON_FLAG"),
        "the machine-readable marks answer does not say its words came from outside"
    );

    let runtime = std::fs::read_to_string("src/agent_tools_runtime.rs").expect("the agent tools");
    let branch = runtime
        .split("Ok(marks) if command.json =>")
        .nth(1)
        .and_then(|rest| rest.split("Ok(marks) =>").next())
        .expect("the json branch");
    assert!(
        branch.contains("browser_said"),
        "the json marks answer is fenced, so no program can parse it"
    );
    assert!(
        !branch.contains("page_said"),
        "the json marks answer is fenced, so no program can parse it"
    );
}

/// The marks (C1): the presses a mark counts on are the helper's own click
/// path, the pin's words are the core's, and the roles a click focuses
/// rather than confirms are the core's text-entry table.
#[test]
fn the_marks_press_list_pin_words_and_text_roles_are_the_cores() {
    use zerocode_core::computer_use::{
        MACOS_PRESS_ACTIONS, MARK_CLIP_ROLES, MARK_HIT_DEPTH, TEXT_ENTRY_ROLES,
    };
    use zerocode_core::computer_use_protocol::marks::{
        ELEMENT_FRAMES_KEY, EVERY_LAYER_KEY, PIN_CONTEXT_KEY, PIN_FRAME_KEY, PIN_NAME_KEY,
        PIN_SIGNATURE_KEY, PIN_TOLERANCE_KEY,
    };
    let press = HELPER
        .lines()
        .find(|line| line.contains("for action in [\"AXPress\""))
        .expect("performClickAction's press list is gone from the helper");
    let listed: Vec<&str> = press.split('"').skip(1).step_by(2).collect();
    assert_eq!(listed, MACOS_PRESS_ACTIONS);
    for (swift, core) in [
        ("elementFramesKey", ELEMENT_FRAMES_KEY),
        ("everyLayerKey", EVERY_LAYER_KEY),
        ("signatureKey", PIN_SIGNATURE_KEY),
        ("nameKey", PIN_NAME_KEY),
        ("contextKey", PIN_CONTEXT_KEY),
        ("frameKey", PIN_FRAME_KEY),
        ("toleranceKey", PIN_TOLERANCE_KEY),
    ] {
        assert!(
            MARK_PIN.contains(&format!("static let {swift} = \"{core}\"")),
            "MarkPin.{swift} is the core's {core}"
        );
    }
    assert_eq!(
        swift_array(MARK_PIN, "static let textEntryRoles:"),
        TEXT_ENTRY_ROLES
    );
    assert_eq!(
        swift_array(MARK_PIN, "static let clipRoles:"),
        MARK_CLIP_ROLES
    );
    assert!(
        MARK_PIN.contains(&format!("static let hitDepth = {MARK_HIT_DEPTH}")),
        "MarkPin.hitDepth is the core's MARK_HIT_DEPTH"
    );
    assert!(
        HELPER.contains("\"own\": ownerPid == getpid() || isTrustedZeroCodeApplication(ownerPid)"),
        "a window list row says whether it is ZeroCode's own"
    );
}

/// The faces the helper writes (`renderElementFaces`) are the core's
/// `ElementFace`, key for key: a key one side renames is a face the other
/// never reads.
#[test]
fn the_helpers_element_faces_are_the_cores() {
    use zerocode_core::computer_use_protocol::marks::{ElementFace, FaceFrame};
    let body = HELPER
        .split("private func renderElementFaces(")
        .nth(1)
        .and_then(|rest| rest.split("\n    }\n").next())
        .expect("renderElementFaces is gone from the helper");
    let mut written: Vec<String> = body
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|word| word.chars().all(|c| c.is_ascii_alphabetic()))
        .map(str::to_string)
        .collect();
    written.sort();
    written.dedup();
    let face = ElementFace {
        index: 1,
        role: "AXButton".into(),
        name: Some("n".into()),
        placeholder: Some("p".into()),
        traits: vec![],
        actions: vec![],
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
        signature: "s".into(),
        visible: Some(FaceFrame {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        }),
        context: Some("c".into()),
    };
    let mut keys: Vec<String> = serde_json::to_value(&face)
        .unwrap()
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    keys.sort();
    assert_eq!(written, keys, "the helper's face keys are the core's");
}

/// An app's other names are the core's table, read where the core says
/// (`identity::MACOS_BUNDLE_NAME_KEYS`), and every verb that names a running
/// app — `resolveApp` and `launch`'s lookup alike — asks by that one rule;
/// `launch` answers the bundle's own name for the next verb to ask by.
#[test]
fn the_helpers_other_app_names_are_the_cores() {
    use zerocode_core::computer_use_protocol::identity::MACOS_BUNDLE_NAME_KEYS;
    let keys = swift_array(DESKTOP_APPS, "public let bundleNameKeys");
    assert_eq!(
        keys.iter().map(String::as_str).collect::<Vec<_>>(),
        MACOS_BUNDLE_NAME_KEYS
    );
    let matches = HELPER
        .split("private func matches(_ app: AppDescriptor, query: String) -> Bool {")
        .nth(1)
        .expect("the helper's matches");
    assert!(
        matches.trim_start().starts_with(
            "applicationAnswers(to: query, name: app.name, bundleId: app.bundleId, otherNames: app.otherNames)"
        ),
        "the helper's matches spells a rule of its own"
    );
    let launch = HELPER
        .split("static func resolveApplicationURL(")
        .nth(1)
        .expect("launch's lookup");
    assert!(
        launch.contains("running.first(where: { matches($0, query: query) })"),
        "launch looks a running app up by a rule of its own"
    );
    assert!(
        HELPER.contains(
            r#""bundleName": jsonNullable(bundleName(info: Bundle(url: url)?.infoDictionary))"#
        ),
        "launch no longer answers the bundle's own name"
    );
}

/// The iOS exporter's answer for an element's centre is read by the key it
/// writes.
#[test]
fn the_ios_exporters_centre_answer_is_read_by_the_key_it_writes() {
    let key = MOBILE_MARKS
        .split("pub(crate) const HIT_AT_CENTRE_KEY: &str = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the marks key");
    assert_eq!(key, "hit_at_centre");
    assert!(
        IOS_BRIDGE.contains(&format!("dict[\"{key}\"] = answered")),
        "the bridge writes its answer under another key"
    );
}
/// A press by number's last-moment question (t-6385) is asked by the kind
/// the helper answers, and answered in the tree's own shape — the exporter's
/// one element description, not a second spelling of it — so the reader can
/// fold a point's answer with the fold it reads every tree with.
#[test]
fn the_ios_helper_answers_a_point_in_the_trees_own_shape() {
    assert!(
        IOS_HID.contains("Self::Hit { .. } => \"hit\""),
        "the window asks for a point under another kind"
    );
    let handled = IOS_HELPER
        .split("case \"hit\":")
        .nth(1)
        .expect("the helper no longer answers a point");
    assert!(
        handled
            .split("case \"")
            .next()
            .is_some_and(|arm| arm.contains("AccessibilityBridge.shared.elementAt(")),
        "the helper answers a point with something other than elementAt"
    );
    let at = IOS_BRIDGE
        .split("func elementAt(")
        .nth(1)
        .and_then(|body| body.split("\n    }\n").next())
        .expect("elementAt is gone");
    assert!(
        at.contains("describe(rootElement, frame: screen)")
            && at.contains("describe(element, frame: frame)"),
        "a point's answer describes its elements some other way than the walk"
    );
    let walk = IOS_BRIDGE
        .split("private func serialize(")
        .nth(1)
        .expect("the walk is gone");
    assert!(
        walk.contains("var dict = describe(element, frame: frame)"),
        "the walk describes an element some other way than a point's answer"
    );
}

/// The exporter's canvas is the core's page (t-6385): an element over this
/// share of the screen blocks no grid point whatever its centre answered,
/// for the reason a mark refuses one — its centre is nobody's target, and
/// controls float over the rest of it.
#[test]
fn the_ios_exporters_canvas_is_the_cores_page() {
    use zerocode_core::computer_use::MARK_MAX_WINDOW_SHARE;
    assert!(
        IOS_LEAF.contains(&format!(
            "static let canvasShare: CGFloat = {MARK_MAX_WINDOW_SHARE}"
        )),
        "the exporter's canvas share drifted from MARK_MAX_WINDOW_SHARE"
    );
    assert!(
        IOS_BRIDGE.contains("AccessibilityLeaf.blocksGrid("),
        "the walk decides what blocks the grid some other way"
    );
}

/// A settling press reads the walk without the grid (t-6385): the window
/// asks for it by the kind the helper answers, and the helper answers it by
/// the tree's own exporter with the grid left out.
#[test]
fn the_ios_helper_walks_without_the_grid_for_a_settling_press() {
    assert!(
        IOS_HID.contains("Self::Walk => \"walk\""),
        "the window asks for the walk under another kind"
    );
    let handled = IOS_HELPER
        .split("case \"walk\":")
        .nth(1)
        .and_then(|rest| rest.split("case \"").next())
        .expect("the helper no longer answers the walk alone");
    assert!(
        handled.contains("describeUI(udid: CommandLine.arguments[1], grid: false)"),
        "the walk alone is answered by something other than the exporter without its grid"
    );
}

/// A subview two parents reference is not a missing observation.
///
/// `Snapshot::new` refuses a truncated tree outright, so the exporter's word
/// for it decides whether a whole screen can be numbered. The walk seats a
/// revisited element once and every parent after the first saw one fewer
/// child than it declared — counted as a cut, that refused screens nothing
/// was missing from (t-5445). The three reasons a child leaves no dictionary
/// are told apart here and nowhere else, so the source says it.
#[test]
fn the_ios_exporter_counts_a_revisited_child_before_it_calls_a_subtree_truncated() {
    let revisit = IOS_BRIDGE
        .split("guard visited.insert(ObjectIdentifier(element)).inserted else {")
        .nth(1)
        .expect("the walk no longer remembers the elements it seated");
    assert!(
        revisit.trim_start().starts_with("return .revisited"),
        "a second parent's reach answers the same nothing a depth or budget cut does"
    );
    assert!(
        IOS_BRIDGE.contains("AccessibilityChildTally(declared: children.count)"),
        "the walk no longer counts the children the element declared"
    );
    let flags: Vec<&str> = IOS_BRIDGE
        .lines()
        .filter(|line| line.contains(r#"dict["truncated"] = true"#))
        .collect();
    assert_eq!(flags.len(), 1, "{flags:?}");
    assert!(
        flags[0].contains("tally.truncated"),
        "a subtree is called truncated on something other than the tally: {}",
        flags[0]
    );
    assert!(
        IOS_CHILD_TALLY.contains("seats.count + revisited < declared"),
        "the tally counts a child already seated elsewhere as one it is missing"
    );
    // The other half of the rule: a spent budget cannot prove the unseen tail
    // was empty, and that loss still reaches the reader.
    assert!(
        IOS_BRIDGE.contains(r#"if remainingElements == 0 { root["truncated"] = true }"#),
        "a spent element budget no longer says the tree is truncated"
    );
}
