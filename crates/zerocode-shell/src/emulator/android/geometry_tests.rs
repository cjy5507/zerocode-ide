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
case "$*" in
  '-s fixture shell wm size') printf 'Physical size: 1080x2400\n' ;;
  '-s fixture shell dumpsys input') cat display ;;
  '-s fixture emu avd name') cat avd-name ;;
  '-s fixture shell input '*) exit 0 ;;
  '-s fixture shell uiautomator dump '*)
    if [ -f next-display ]; then cp next-display display; fi ;;
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

#[test]
fn geometry_changed_during_the_tree_cannot_issue_a_snapshot() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.directory.path().join("tree.xml"),
        include_str!("../marks/fixtures/android.xml"),
    )
    .unwrap();
    fixture.display(2400, 1080, 1);
    let (snapshot, size) = marks_snapshot(&fixture.sdk, "fixture").unwrap();
    assert_eq!(size, (2400, 1080));
    assert_eq!(snapshot.screen.width, 2400.0);
    assert_eq!(snapshot.screen.height, 1080.0);
    // Even a half turn, with unchanged dimensions, invalidates a snapshot
    // whose XML was read across that transition.
    fixture.display(2400, 1080, 3);
    std::fs::rename(
        fixture.directory.path().join("display"),
        fixture.directory.path().join("next-display"),
    )
    .unwrap();
    fixture.display(2400, 1080, 1);
    let error = marks_snapshot(&fixture.sdk, "fixture").unwrap_err();
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
