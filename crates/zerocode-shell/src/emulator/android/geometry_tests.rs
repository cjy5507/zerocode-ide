//! Exercise the actual metadata command without connecting to a device.

use std::os::unix::fs::PermissionsExt;

use super::*;

struct Fixture {
    directory: tempfile::TempDir,
    sdk: AndroidSdk,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let adb = directory.path().join("adb");
        std::fs::write(
            &adb,
            r#"#!/bin/sh
cd "$(dirname "$0")" || exit 1
printf '%s\n' "$*" >> commands
# meet <mine> <others…>: say this read is here, then wait (3 s at most) for
# the others — only while a test asks for a rendezvous.
meet() {
  [ -f rendezvous ] || return 0
  touch "$1"; shift
  for _ in $(seq 1 300); do
    ready=1
    for other in "$@"; do [ -f "$other" ] || ready=0; done
    [ "$ready" = 1 ] && return 0
    sleep 0.01
  done
  return 1
}
case "$*" in
  '-s fixture shell wm size') printf 'Physical size: 1080x2400\n' ;;
  '-s fixture shell dumpsys input') meet display-read dump-started || exit 1; cat display ;;
  '-s fixture emu avd name') meet avd-read dump-started || exit 1; cat avd-name ;;
  '-s fixture shell input '*) exit 0 ;;
  '-s fixture shell uiautomator dump '*)
    meet dump-started $(cat rendezvous 2>/dev/null) || exit 1
    if [ -f next-display ]; then sleep 0.3; cp next-display display; fi ;;
  '-s fixture exec-out cat '*) cat tree.xml ;;
  '-s fixture shell rm -f '*) exit 0 ;;
  *) exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&adb, std::fs::Permissions::from_mode(0o700)).unwrap();
        let sdk = AndroidSdk {
            root: directory.path().into(),
            adb,
            emulator: Ok(directory.path().join("unused-emulator")),
        };
        Self { directory, sdk }
    }

    fn display(&self, width: u32, height: u32, rotation: u32) {
        // AOSP DisplayViewport::toString. Physical device dimensions remain
        // natural; logicalFrame already includes rotation and size overrides.
        std::fs::write(
            self.directory.path().join("display"),
            format!(
                "Input Reader State:\n  Configuration:\n    Viewports:\n      Viewport INTERNAL: displayId=0, uniqueId=local:fixture, port=0, orientation={rotation}, logicalFrame=[0, 0, {width}, {height}], physicalFrame=[0, 0, 1080, 2400], deviceSize=[1080, 2400], isActive=[1]\n"
            ),
        )
        .unwrap();
    }
}

#[test]
fn avd_identity_is_reread_and_missing_or_ambiguous_names_are_refused() {
    let fixture = Fixture::new();
    let path = fixture.directory.path().join("avd-name");
    for name in ["first-avd", "second-avd"] {
        std::fs::write(&path, format!("{name}\r\nOK\r\n")).unwrap();
        assert_eq!(android_avd_name(&fixture.sdk.adb, "fixture").unwrap(), name);
    }
    for invalid in ["OK\n", "first-avd\nsecond-avd\nOK\n"] {
        std::fs::write(&path, invalid).unwrap();
        assert!(android_avd_name(&fixture.sdk.adb, "fixture").is_err());
    }
}

#[test]
fn raw_tap_and_swipe_remeasure_geometry_for_every_input() {
    let fixture = Fixture::new();
    for (size, rotation) in [((1080, 2400), 0), ((2400, 1080), 1), ((1600, 900), 1)] {
        fixture.display(size.0, size.1, rotation);
        tap_normalized(&fixture.sdk, "fixture", 0.75, 0.25).unwrap();
        swipe_normalized(
            &fixture.sdk,
            "fixture",
            (0.75, 0.75),
            (0.25, 0.25),
            Some(150),
        )
        .unwrap();
    }
    let commands = std::fs::read_to_string(fixture.directory.path().join("commands")).unwrap();
    assert_eq!(
        commands.lines().collect::<Vec<_>>(),
        [
            "-s fixture shell dumpsys input",
            "-s fixture shell input tap 809 600",
            "-s fixture shell dumpsys input",
            "-s fixture shell input swipe 809 1799 270 600 150",
            "-s fixture shell dumpsys input",
            "-s fixture shell input tap 1799 270",
            "-s fixture shell dumpsys input",
            "-s fixture shell input swipe 1799 809 600 270 150",
            "-s fixture shell dumpsys input",
            "-s fixture shell input tap 1199 225",
            "-s fixture shell dumpsys input",
            "-s fixture shell input swipe 1199 674 400 225 150",
        ]
    );
}

#[test]
fn unavailable_geometry_never_sends_normalized_input() {
    let fixture = Fixture::new();
    std::fs::write(fixture.directory.path().join("display"), "unavailable").unwrap();
    assert!(tap_normalized(&fixture.sdk, "fixture", 0.75, 0.25).is_err());
    assert!(swipe_normalized(&fixture.sdk, "fixture", (0.75, 0.75), (0.25, 0.25), None).is_err());
    let commands = std::fs::read_to_string(fixture.directory.path().join("commands")).unwrap();
    assert_eq!(
        commands.lines().collect::<Vec<_>>(),
        ["-s fixture shell dumpsys input"; 2]
    );
}

/// The AVD the fixture's `emu avd name` answers.
fn named(fixture: &Fixture, avd: &str) {
    std::fs::write(
        fixture.directory.path().join("avd-name"),
        format!("{avd}\nOK\n"),
    )
    .unwrap();
}

/// The fixture's tree, dumped in `rotation` — or in none, as a dump that
/// does not say its rotation.
fn dumped_in(fixture: &Fixture, rotation: Option<u32>) {
    let hierarchy = rotation.map_or_else(
        || "<hierarchy>".to_string(),
        |rotation| format!(r#"<hierarchy rotation="{rotation}">"#),
    );
    std::fs::write(
        fixture.directory.path().join("tree.xml"),
        include_str!("../marks/fixtures/android.xml")
            .replace(r#"<hierarchy rotation="0">"#, &hierarchy),
    )
    .unwrap();
}

/// A display turned while the dump runs — after the reads beside it.
fn turned_during_the_dump(fixture: &Fixture, now: (u32, u32, u32), then: (u32, u32, u32)) {
    fixture.display(then.0, then.1, then.2);
    std::fs::rename(
        fixture.directory.path().join("display"),
        fixture.directory.path().join("next-display"),
    )
    .unwrap();
    fixture.display(now.0, now.1, now.2);
}

/// How many of the adb calls the fixture answered said `what`.
fn asked(fixture: &Fixture, what: &str) -> usize {
    std::fs::read_to_string(fixture.directory.path().join("commands"))
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(what))
        .count()
}

#[test]
fn geometry_changed_during_the_tree_cannot_issue_a_snapshot() {
    let fixture = Fixture::new();
    named(&fixture, "fixture-avd");
    dumped_in(&fixture, Some(1));
    fixture.display(2400, 1080, 1);
    let (snapshot, size, identity) = marks_snapshot(&fixture.sdk, "fixture", true).unwrap();
    assert_eq!(identity, "fixture-avd");
    assert_eq!(size, (2400, 1080));
    assert_eq!(snapshot.screen.width, 2400.0);
    assert_eq!(snapshot.screen.height, 1080.0);
    // Even a half turn, with unchanged dimensions, invalidates a snapshot
    // whose XML was read across that transition before a press lands on it.
    turned_during_the_dump(&fixture, (2400, 1080, 1), (2400, 1080, 3));
    let error = marks_snapshot(&fixture.sdk, "fixture", true).unwrap_err();
    assert!(error.contains("display changed"), "{error}");
    let commands = std::fs::read_to_string(fixture.directory.path().join("commands")).unwrap();
    assert!(!commands.contains("shell input "));
}

#[test]
fn logical_display_geometry_follows_rotation_and_overrides_without_guessing() {
    let fixture = Fixture::new();
    // Includes a portrait app overriding a landscape request and a logical
    // size override which does not match the physical device dimensions.
    for (width, height, rotation) in [
        (2400, 1080, 1),
        (1080, 2400, 0),
        (1080, 2400, 2),
        (2400, 1080, 3),
        (1600, 900, 1),
    ] {
        fixture.display(width, height, rotation);
        assert_eq!(
            read_android_screen_size(&fixture.sdk, "fixture").unwrap(),
            (width, height),
            "rotation {rotation} must use the current logical frame"
        );
    }
    let commands = std::fs::read_to_string(fixture.directory.path().join("commands")).unwrap();
    assert_eq!(
        commands.lines().collect::<Vec<_>>(),
        vec!["-s fixture shell dumpsys input"; 5],
        "geometry comes only from metadata, without a PNG or wm-size fallback"
    );
}

/// The next read meets the dump: the dump waits for each of `beside` to
/// have started, and each read waits for the dump — reads taken one after
/// another never meet, and the fixture fails them.
fn meeting(fixture: &Fixture, beside: &[&str]) {
    for marker in ["dump-started", "display-read", "avd-read"] {
        let _ = std::fs::remove_file(fixture.directory.path().join(marker));
    }
    std::fs::write(
        fixture.directory.path().join("rendezvous"),
        beside.join(" "),
    )
    .unwrap();
}

/// A look reads its display and its AVD beside its dump, not before and
/// after it (t-6385): each of those reads waited for the one before it. A
/// press reads its display beside its dump too, and again with its AVD
/// after the dump, before its tap.
#[test]
fn a_look_reads_its_display_and_avd_beside_its_dump_and_a_press_once_more_after() {
    let fixture = Fixture::new();
    named(&fixture, "fixture-avd");
    dumped_in(&fixture, Some(0));
    fixture.display(1080, 2400, 0);
    meeting(&fixture, &["display-read", "avd-read"]);
    let (_, size, identity) = marks_snapshot(&fixture.sdk, "fixture", false).unwrap();
    assert_eq!((size, identity.as_str()), ((1080, 2400), "fixture-avd"));
    meeting(&fixture, &["display-read"]);
    let (_, _, identity) = marks_snapshot(&fixture.sdk, "fixture", true).unwrap();
    assert_eq!(identity, "fixture-avd");
    let commands = std::fs::read_to_string(fixture.directory.path().join("commands")).unwrap();
    assert!(
        commands.rfind("emu avd name") > commands.rfind("uiautomator dump"),
        "a press reads its AVD after its dump: {commands}"
    );
    assert_eq!(asked(&fixture, "uiautomator dump"), 2);
    assert_eq!(
        asked(&fixture, "dumpsys input"),
        3,
        "one beside the look's dump, one beside and one after the press's"
    );
    assert_eq!(asked(&fixture, "emu avd name"), 2, "one each");
}

/// A look numbers its tree only in a display in the rotation the tree was
/// dumped in; a dump that names none stands only when the display did not
/// change around it, read again after it as every read once was.
#[test]
fn a_look_numbers_its_tree_only_in_the_rotation_it_was_dumped_in() {
    let fixture = Fixture::new();
    named(&fixture, "fixture-avd");
    fixture.display(1080, 2400, 0);
    dumped_in(&fixture, Some(1));
    let error = marks_snapshot(&fixture.sdk, "fixture", false).unwrap_err();
    assert!(error.contains("display changed"), "{error}");
    assert_eq!(asked(&fixture, "dumpsys input"), 1);
    dumped_in(&fixture, None);
    let (_, size, _) = marks_snapshot(&fixture.sdk, "fixture", false).unwrap();
    assert_eq!(size, (1080, 2400));
    assert_eq!(asked(&fixture, "dumpsys input"), 3, "beside and after");
    turned_during_the_dump(&fixture, (1080, 2400, 0), (2400, 1080, 1));
    let error = marks_snapshot(&fixture.sdk, "fixture", false).unwrap_err();
    assert!(error.contains("display changed"), "{error}");
}
