//! What the `list` answer promises about real devices: one row per phone the
//! tools show, never drivable, in plain words; a tool that did not answer is a
//! named gap and not an empty list; and the skill that teaches the road speaks
//! in the answer's own words. Every name and id here is made up.

use serde_json::{Value, json};

use super::*;
use crate::agent_emulator::{EMULATOR_VERBS, MOBILE_DESCRIPTION_CHARS_MAX, MOBILE_SKILL_NAME};
use crate::skill_install::bundled_skill;

/// What `devicectl list devices --json-output` writes, cut to the fields the
/// reader uses: two phones the Mac reaches, one it knows and cannot reach, and
/// a watch that is not the person's phone.
fn devicectl_fixture() -> String {
    let device = |name: &str, id: &str, platform: &str, kind: &str, tunnel: &str| {
        json!({
            "identifier": id,
            "connectionProperties": { "tunnelState": tunnel, "pairingState": "paired" },
            "deviceProperties": { "name": name },
            "hardwareProperties": {
                "platform": platform,
                "deviceType": kind,
                "marketingName": format!("Synthetic {kind}"),
                "udid": format!("udid-{id}"),
            },
        })
    };
    json!({
        "info": { "jsonVersion": 3, "outcome": "success" },
        "result": { "devices": [
            device("Synthetic Phone A", "00000000-0000-0000-0000-00000000000a", "iOS", "iPhone", "connected"),
            device("Synthetic Tablet B", "00000000-0000-0000-0000-00000000000b", "iPadOS", "iPad", "disconnected"),
            device("Synthetic Phone C", "00000000-0000-0000-0000-00000000000c", "iOS", "iPhone", "unavailable"),
            device("Synthetic Watch D", "00000000-0000-0000-0000-00000000000d", "watchOS", "AppleWatch", "connected"),
        ] },
    })
    .to_string()
}

/// What `adb devices -l` prints: the daemon's notices, one emulator the window
/// drives, a phone on a cable, a phone that has not allowed this computer, one
/// that is offline, and one on a network address.
const ADB_FIXTURE: &str = "\
* daemon not running; starting now at tcp:5037
* daemon started successfully
List of devices attached
emulator-5554          device product:sdk_synthetic model:sdk_synthetic device:emu64a transport_id:1
SYN0000000001          device usb:1-1 product:synthetic model:Synthetic_Pixel device:synthetic transport_id:2
SYN0000000002          unauthorized usb:1-2 transport_id:3
SYN0000000003          offline transport_id:4
192.0.2.7:5555         device product:synthetic2 model:Synthetic_Tab transport_id:5
";

fn row(hardware: Hardware, name: &str, state: LinkState) -> PhysicalDevice {
    PhysicalDevice::new(hardware, name, None, state)
}

/// An answer that must have come: a refusal fails at this assertion and not at
/// a panic further down.
fn answered(result: Result<Value, String>) -> Value {
    assert!(result.is_ok(), "no answer: {result:?}");
    result.unwrap_or_default()
}

fn reading(
    platform: EmulatorPlatform,
    simulated: Result<Value, String>,
    physical: Result<Vec<PhysicalDevice>, String>,
) -> PlatformReading {
    PlatformReading {
        platform,
        simulated,
        physical,
    }
}

#[test]
fn a_devicectl_answer_becomes_one_undrivable_row_per_phone() {
    let parsed = parse_devicectl(&devicectl_fixture());
    assert!(parsed.is_ok(), "a readable answer: {parsed:?}");
    let rows = parsed.unwrap_or_default();
    let seen: Vec<(&str, LinkState)> = rows
        .iter()
        .map(|row| (row.name.as_str(), row.state))
        .collect();
    assert_eq!(
        seen,
        [
            ("Synthetic Phone A", LinkState::Connected),
            // devicectl's own table calls a `disconnected` tunnel "available
            // (paired)": the Mac reaches the phone.
            ("Synthetic Tablet B", LinkState::Connected),
            ("Synthetic Phone C", LinkState::Unavailable),
        ],
        "the watch is not a phone, and each phone says how the Mac stands with it"
    );
    for row in &rows {
        assert_eq!(row.platform, EmulatorPlatform::Ios);
        assert!(
            !row.drivable,
            "{}: a real device is never drivable here",
            row.name
        );
    }
    let reasons: Vec<&str> = rows.iter().map(|row| row.reason).collect();
    assert_eq!(
        reasons,
        [IPHONE_REASON, IPAD_REASON, IPHONE_REASON],
        "an iPad is told apart from an iPhone, and each says its own road"
    );
    assert_eq!(rows[1].model.as_deref(), Some("Synthetic iPad"));
}

#[test]
fn a_devicectl_answer_it_cannot_read_is_an_error_and_not_an_empty_list() {
    assert!(parse_devicectl("not json at all").is_err());
    assert!(parse_devicectl(r#"{"result": {}}"#).is_err());
    assert_eq!(
        parse_devicectl(r#"{"result": {"devices": []}}"#),
        Ok(Vec::new())
    );
}

#[test]
fn an_adb_listing_keeps_the_phones_and_drops_the_emulators_and_the_noise() {
    let rows = physical_android_rows(&parse_adb_rows(ADB_FIXTURE));
    let seen: Vec<(&str, LinkState)> = rows
        .iter()
        .map(|row| (row.name.as_str(), row.state))
        .collect();
    assert_eq!(
        seen,
        [
            ("Synthetic Pixel", LinkState::Connected),
            (UNNAMED_ANDROID, LinkState::Unauthorized),
            (UNNAMED_ANDROID, LinkState::Offline),
            ("Synthetic Tab", LinkState::Connected),
        ]
    );
    assert!(rows.iter().all(|row| {
        row.platform == EmulatorPlatform::Android && !row.drivable && row.reason == ANDROID_REASON
    }));
}

/// The finding for a real Android phone: today the window does not drive it.
/// One listing splits into exactly two groups — the emulators it drives (an
/// `emulator-` serial that is up) and the devices it only names — and no row is
/// in both.
#[test]
fn a_row_of_one_adb_listing_is_a_driven_emulator_or_a_named_phone_and_never_both() {
    let rows = parse_adb_rows(ADB_FIXTURE);
    assert_eq!(
        rows.len(),
        5,
        "the header and the daemon's notices are not devices"
    );
    let driven: Vec<&str> = rows
        .iter()
        .filter(|row| row.is_emulator() && row.is_up())
        .map(|row| row.serial.as_str())
        .collect();
    let named = physical_android_rows(&rows);
    assert_eq!(driven, ["emulator-5554"]);
    assert_eq!(named.len(), 4);
    assert_eq!(
        rows.len(),
        driven.len() + named.len(),
        "every row is one or the other: none is both, none is lost"
    );
}

#[test]
fn a_device_name_is_one_short_line_without_control_characters() {
    let long = format!("Synthetic\n\u{1b}[31mPhone\u{7}  {}", "x".repeat(200));
    let name = plain_name(&long);
    assert!(name.starts_with("Synthetic [31mPhone x"), "{name:?}");
    assert!(name.chars().all(|ch| !ch.is_control()), "{name:?}");
    assert_eq!(name.chars().count(), DEVICE_NAME_CHARS_MAX);
}

#[test]
fn the_rows_an_answer_carries_put_the_reachable_first_and_count_the_rest() {
    let mut rows = Vec::new();
    for at in 0..6 {
        rows.push(row(
            Hardware::IPhone,
            &format!("Gone {at}"),
            LinkState::Unavailable,
        ));
    }
    for at in 0..6 {
        rows.push(row(
            Hardware::AndroidDevice,
            &format!("Here {at}"),
            LinkState::Connected,
        ));
    }
    let (kept, omitted) = bounded(rows);
    assert_eq!(kept.len(), PHYSICAL_ROWS_MAX);
    assert_eq!(omitted, 12 - PHYSICAL_ROWS_MAX);
    assert!(
        kept[..6]
            .iter()
            .all(|row| row.state == LinkState::Connected),
        "the phones the Mac reaches are not pushed out by old pairings"
    );
}

#[test]
fn a_row_says_drivable_false_and_its_reason_in_the_words_the_skill_names() {
    let phone = row(
        Hardware::IPhone,
        "Synthetic Phone A",
        LinkState::Unavailable,
    );
    let said = serde_json::to_value(&phone).expect("a row serializes");
    let mut keys: Vec<&str> = said
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["drivable", "name", "platform", "reason", "state"]);
    assert_eq!(said["drivable"], false);
    assert_eq!(said["platform"], "ios");
    assert_eq!(said["state"], "unavailable");
    for (state, word) in [
        (LinkState::Connected, "connected"),
        (LinkState::Unauthorized, "unauthorized"),
        (LinkState::Offline, "offline"),
        (LinkState::Unavailable, "unavailable"),
        (LinkState::Unknown, "unknown"),
    ] {
        assert_eq!(serde_json::to_value(state).expect("a state"), word);
    }
}

/// The most characters a real device's reason may take. A row rides every
/// `list` answer, up to [`PHYSICAL_ROWS_MAX`] of them: the road fits in about
/// this much, and a longer one is a paragraph repeated for every phone ever
/// paired.
const REASON_CHARS_MAX: usize = 300;

/// "The iPhone is connected" means a real iPhone on a cable, and the Mac's iPhone
/// Mirroring window is the road it takes (the coordinator's note of 2026-10-04:
/// another session drove that window more than ninety times and called the app
/// by three names, two of which it did not find). The row says the road in the
/// words an agent acts on, in the row itself, so `list` is the one step that
/// tells which road a device takes.
#[test]
fn an_iphone_row_names_the_mac_window_that_drives_it() {
    let rows = parse_devicectl(&devicectl_fixture()).unwrap_or_default();
    let phone = rows.iter().find(|row| row.name == "Synthetic Phone A");
    assert!(phone.is_some(), "the fixture's iPhone is not in the answer");
    let reason = phone.map_or("", |row| row.reason);
    for words in [
        "zerocode-computer",
        "iPhone Mirroring",
        IPHONE_MIRRORING_APP,
        "ask the person",
        "do not open the app",
    ] {
        assert!(
            reason.contains(words),
            "an iPhone's reason never says `{words}`: {reason}"
        );
    }
    assert!(
        reason.chars().count() <= REASON_CHARS_MAX,
        "the reason is {} characters, over {REASON_CHARS_MAX}: {reason}",
        reason.chars().count()
    );
}

/// iPhone Mirroring shows iPhones only: an iPad is not sent to a window that
/// cannot show it.
#[test]
fn an_ipad_row_is_not_sent_to_iphone_mirroring() {
    let rows = parse_devicectl(&devicectl_fixture()).unwrap_or_default();
    let tablet = rows.iter().find(|row| row.name == "Synthetic Tablet B");
    assert!(tablet.is_some(), "the fixture's iPad is not in the answer");
    let reason = tablet.map_or("", |row| row.reason);
    assert_ne!(reason, IPHONE_REASON, "an iPad was given an iPhone's road");
    assert!(!reason.contains(IPHONE_MIRRORING_APP), "{reason}");
    assert!(
        reason.chars().count() <= REASON_CHARS_MAX,
        "the reason is {} characters, over {REASON_CHARS_MAX}: {reason}",
        reason.chars().count()
    );
}

#[test]
fn list_names_a_phone_it_cannot_drive_beside_the_simulators_it_can() {
    let simulators = json!([{ "udid": "SIM-1", "name": "Synthetic Simulator", "booted": true }]);
    let answer = answered(list_answer(
        reading(
            EmulatorPlatform::Ios,
            Ok(simulators.clone()),
            Ok(vec![row(
                Hardware::IPhone,
                "Synthetic Phone A",
                LinkState::Connected,
            )]),
        ),
        reading(EmulatorPlatform::Android, Ok(json!([])), Ok(Vec::new())),
    ));
    assert_eq!(answer["surface"], BUILT_IN_SURFACE);
    assert_eq!(answer["ios"], simulators);
    assert_eq!(answer["android"], json!([]));
    assert_eq!(answer[PHYSICAL_KEY].as_array().map(Vec::len), Some(1));
    assert_eq!(answer[PHYSICAL_KEY][0]["drivable"], false);
    assert_eq!(answer[PHYSICAL_KEY][0]["reason"], IPHONE_REASON);
    assert!(answer.get(UNCHECKED_KEY).is_none() && answer.get(OMITTED_KEY).is_none());
}

#[test]
fn a_tool_that_did_not_answer_is_a_named_gap_and_the_rest_still_answers() {
    // A Mac with Xcode and no Android SDK: the SDK's absence used to be the
    // whole answer, and hid the simulators and the phone.
    let answer = answered(list_answer(
        reading(
            EmulatorPlatform::Ios,
            Ok(json!([{ "udid": "SIM-1" }])),
            Ok(vec![row(
                Hardware::IPhone,
                "Synthetic Phone A",
                LinkState::Connected,
            )]),
        ),
        reading(
            EmulatorPlatform::Android,
            Err("no Android SDK".into()),
            Err("no Android SDK".into()),
        ),
    ));
    assert_eq!(answer["ios"].as_array().map(Vec::len), Some(1));
    assert_eq!(answer["android"], json!([]));
    assert_eq!(answer[PHYSICAL_KEY].as_array().map(Vec::len), Some(1));
    let named = answer[UNCHECKED_KEY]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let gaps: Vec<(&str, &str)> = named
        .iter()
        .map(|gap| {
            (
                gap["platform"].as_str().unwrap_or(""),
                gap["why"].as_str().unwrap_or(""),
            )
        })
        .collect();
    assert_eq!(
        gaps,
        [("android", "no Android SDK")],
        "one missing SDK stops both readers with one sentence, and it is said once"
    );

    let looking = answered(list_answer(
        reading(
            EmulatorPlatform::Ios,
            Ok(json!([])),
            Err("devicectl is still looking".into()),
        ),
        reading(EmulatorPlatform::Android, Ok(json!([])), Ok(Vec::new())),
    ));
    assert_eq!(looking[UNCHECKED_KEY][0]["platform"], "ios");
    assert_eq!(
        looking[UNCHECKED_KEY][0]["why"],
        "devicectl is still looking"
    );
    assert_eq!(
        looking[PHYSICAL_KEY],
        json!([]),
        "a gap is not a device, and not a claim that none exists"
    );
}

#[test]
fn with_neither_platform_readable_there_is_no_answer_and_it_is_the_first_error() {
    let result = list_answer(
        reading(EmulatorPlatform::Ios, Err("first".into()), Ok(Vec::new())),
        reading(
            EmulatorPlatform::Android,
            Err("second".into()),
            Ok(Vec::new()),
        ),
    );
    assert_eq!(result, Err("first".to_string()));
}

#[test]
fn an_answer_with_more_phones_than_the_cap_says_how_many_it_left_out() {
    let many: Vec<PhysicalDevice> = (0..PHYSICAL_ROWS_MAX + 3)
        .map(|at| {
            row(
                Hardware::IPhone,
                &format!("Synthetic Phone {at:02}"),
                LinkState::Connected,
            )
        })
        .collect();
    let answer = answered(list_answer(
        reading(EmulatorPlatform::Ios, Ok(json!([])), Ok(many)),
        reading(EmulatorPlatform::Android, Ok(json!([])), Ok(Vec::new())),
    ));
    assert_eq!(
        answer[PHYSICAL_KEY].as_array().map(Vec::len),
        Some(PHYSICAL_ROWS_MAX)
    );
    assert_eq!(answer[OMITTED_KEY], 3);
}

/// The skill is what an agent reads about the road, so it names the answer's
/// own words and every verb the door answers — one checklist, not two.
#[test]
fn the_mobile_skill_speaks_in_the_answers_words_and_teaches_every_verb() {
    let skill = bundled_skill(MOBILE_SKILL_NAME);
    assert!(
        skill.is_some(),
        "this build carries no {MOBILE_SKILL_NAME} skill"
    );
    let content = skill.map_or("", |skill| skill.content);
    for word in [
        "ios",
        "android",
        PHYSICAL_KEY,
        UNCHECKED_KEY,
        "drivable: false",
        "booted",
        "connected",
        "unavailable",
    ] {
        assert!(
            content.contains(&format!("`{word}`")),
            "the skill never names `{word}`"
        );
    }
    assert!(
        content.contains("zerocode-emulator list --json"),
        "the first step is not written out"
    );
    for verb in &EMULATOR_VERBS {
        assert!(
            content.contains(&format!("zerocode-emulator {}", verb.word)),
            "the skill does not teach `zerocode-emulator {}`",
            verb.word
        );
    }
}

/// A real iPhone is driven through the Mac's iPhone Mirroring window, and the
/// skill says how: the app by its id, a look for the window first, one request
/// to the person when it is not open, and no opening of the app or unlocking on
/// their behalf. Its sample row is the row `list` answers.
#[test]
fn the_mobile_skill_sends_a_real_iphone_through_the_mirroring_window() {
    let content = bundled_skill(MOBILE_SKILL_NAME).map_or("", |skill| skill.content);
    for words in [
        IPHONE_MIRRORING_APP,
        "iPhone Mirroring",
        "iPhone 미러링",
        "ask the person",
        "do not open the app",
        "the `computer-use` skill",
    ] {
        assert!(
            content.contains(words),
            "the skill never says `{words}` about a real iPhone"
        );
    }
    let look = format!("zerocode-computer list-windows --app {IPHONE_MIRRORING_APP}");
    assert!(
        content.contains(&look),
        "the skill does not write out the look for the window: {look}"
    );
    assert!(
        content.contains(IPHONE_REASON),
        "the skill's sample row is not the row `list` answers for an iPhone"
    );
    let description = content
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap_or("");
    assert!(
        !description.contains("never the Mac window list, iPhone Mirroring"),
        "the description turns an agent away from the road a real iPhone takes: {description}"
    );
}

/// The `computer-use` skill points to the mobile skill and keeps no copy of the
/// commands: a table in two places is two tables that drift.
#[test]
fn the_computer_use_skill_points_to_the_mobile_skill_and_keeps_no_command_of_its_own() {
    let skill = bundled_skill("computer-use");
    assert!(skill.is_some(), "this build carries no computer-use skill");
    let content = skill.map_or("", |skill| skill.content);
    assert!(
        content.contains(&format!("`{MOBILE_SKILL_NAME}` skill")),
        "computer-use never points to the {MOBILE_SKILL_NAME} skill"
    );
    let copies: Vec<&str> = content
        .lines()
        .filter(|line| line.trim_start().starts_with("zerocode-emulator "))
        .collect();
    assert!(
        copies.is_empty(),
        "computer-use still carries mobile commands: {copies:?}"
    );
}

/// What a person says, in the two languages the person writes in. The skill's
/// description is the one line every CLI reads to decide whether to open it, so
/// it carries these words.
#[test]
fn the_mobile_skills_description_carries_the_words_a_person_uses() {
    let skill = bundled_skill(MOBILE_SKILL_NAME);
    assert!(
        skill.is_some(),
        "this build carries no {MOBILE_SKILL_NAME} skill"
    );
    let content = skill.map_or("", |skill| skill.content);
    let description = content
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap_or("");
    let lowered = description.to_lowercase();
    for word in [
        "iphone",
        "ipad",
        "android",
        "phone",
        "simulator",
        "emulator",
        "installed",
        "connected",
        "tap",
        "type",
        "screenshot",
        "swipe",
    ] {
        assert!(
            lowered.contains(word),
            "the description never says `{word}`: {description}"
        );
    }
    for word in [
        "아이폰",
        "아이패드",
        "안드로이드",
        "시뮬레이터",
        "에뮬레이터",
        "설치",
        "연결",
        "탭",
        "입력",
        "스크린샷",
    ] {
        assert!(
            description.contains(word),
            "the description never says `{word}`: {description}"
        );
    }
    assert!(
        description.contains("zerocode-emulator list"),
        "the description does not name the first step"
    );
}

/// A description rides every session of every CLI that lists its skills, and
/// the Agent Skills format ends it at 1,024 characters; a single plain line
/// also reads the same in every CLI's frontmatter reader.
#[test]
fn the_mobile_skills_description_is_one_plain_line_under_its_budget() {
    let content = bundled_skill(MOBILE_SKILL_NAME).map_or("", |skill| skill.content);
    let description = content
        .lines()
        .find_map(|line| line.strip_prefix("description: "))
        .unwrap_or("");
    assert!(
        !description.is_empty(),
        "the skill has no one-line description"
    );
    assert!(
        description.chars().count() <= MOBILE_DESCRIPTION_CHARS_MAX,
        "the description is {} characters, over its {MOBILE_DESCRIPTION_CHARS_MAX}",
        description.chars().count()
    );
    // A plain YAML scalar cannot hold a colon followed by a space, or a space
    // followed by `#`.
    assert!(
        !description.contains(": ") && !description.contains(" #"),
        "{description}"
    );
    assert!(
        content.starts_with(&format!("---\nname: {MOBILE_SKILL_NAME}\n")),
        "the skill's name is not its directory's"
    );
}

/// The words the skill and the `--help` page share about `list`: a person who
/// reads only the help page still learns that a phone may be named and not
/// drivable.
#[test]
fn the_help_page_says_what_list_names_beside_the_devices_it_drives() {
    let usage = crate::computer_use::emulator_usage();
    assert!(
        usage.contains(PHYSICAL_KEY) && usage.contains("drivable"),
        "`zerocode-emulator --help` does not say that list names real devices it cannot drive:\n{usage}"
    );
}
