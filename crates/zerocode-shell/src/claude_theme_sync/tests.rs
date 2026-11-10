use super::*;
use std::cell::Cell;

fn sample(claude: &str) -> Theme {
    Theme {
        name: theme::NAME.to_string(),
        base: "dark".to_string(),
        overrides: [("claude".to_string(), claude.to_string())].into(),
    }
}

/// A config root with this window's Claude home in it, holding `settings`.
fn home_with(settings: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let root = tempfile::tempdir().expect("config root");
    let home = crate::accounts::runtime_home(root.path());
    std::fs::create_dir_all(&home).expect("home");
    if let Some(text) = settings {
        std::fs::write(settings_path(&home), text).expect("settings");
    }
    (root, home)
}

fn settings_of(home: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(settings_path(home)).expect("settings"))
        .expect("JSON")
}

fn theme_file(home: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(theme_path(home)).ok()?).ok()
}

fn matching<'a>(theme: &'a Theme, loads: &'a dyn Fn() -> bool) -> Target<'a> {
    Target::Match {
        theme,
        loads_custom_themes: loads,
    }
}

/// A default theme is replaced: the file first, the record of what was
/// there, then the selection — every other key of the settings left as it
/// was. A pass that changes nothing writes nothing, and a new palette
/// rewrites the file alone.
#[test]
fn a_default_theme_is_replaced_and_what_it_was_is_written_down() {
    let (root, home) = home_with(Some(
        r#"{"theme":"dark","statusLine":{"type":"command","command":"x"},"hooks":{}}"#,
    ));
    let asked = Cell::new(0);
    let loads = || {
        asked.set(asked.get() + 1);
        true
    };
    let first = sample("#7aa2f7");
    let pass = sync(root.path(), matching(&first, &loads)).expect("a pass");
    assert_eq!(
        pass,
        Pass {
            wrote_theme: true,
            wrote_settings: true
        }
    );
    assert_eq!(asked.get(), 1, "the CLI is asked once, at the takeover");
    let settings = settings_of(&home);
    assert_eq!(settings["theme"], theme::SETTING);
    assert_eq!(settings["statusLine"]["command"], "x");
    assert!(settings["hooks"].is_object());
    assert_eq!(
        theme_file(&home).expect("a theme file")["overrides"]["claude"],
        "#7aa2f7"
    );
    assert_eq!(
        read_record(root.path()),
        Some(Record {
            previous_theme: Some("dark".to_string()),
            user_changed: false
        })
    );
    // Nothing in the folder but the theme: the temporary sibling is gone, and
    // was never a `.json` Claude Code would have listed.
    let listed: Vec<String> = std::fs::read_dir(home.join("themes"))
        .expect("themes")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(listed, ["zerocode.json"]);

    let again = sync(root.path(), matching(&first, &loads)).expect("a pass");
    assert_eq!(again, Pass::default(), "the same palette writes nothing");
    let moved = sync(root.path(), matching(&sample("#268bd2"), &loads)).expect("a pass");
    assert_eq!(
        moved,
        Pass {
            wrote_theme: true,
            wrote_settings: false
        }
    );
    assert_eq!(
        theme_file(&home).expect("a theme file")["overrides"]["claude"],
        "#268bd2"
    );
    assert_eq!(asked.get(), 1, "the CLI is not asked again");
}

/// Letting go puts back exactly what was there — a named theme, `auto` as
/// the terminal's own base for a session still running, or no key at all —
/// rewrites the file as that look, and forgets the record.
#[test]
fn letting_go_puts_back_what_was_there() {
    for (before, terminal, key_after, base_after) in [
        (
            Some(r#"{"theme":"light","x":1}"#),
            "dark",
            Some("light"),
            "light",
        ),
        (
            Some(r#"{"theme":"auto","x":1}"#),
            "light",
            Some("auto"),
            "light",
        ),
        (Some(r#"{"x":1}"#), "light", None, "dark"),
        (None, "light", None, "dark"),
    ] {
        let (root, home) = home_with(before);
        let theme = sample("#7aa2f7");
        sync(root.path(), matching(&theme, &|| true)).expect("a pass");
        assert_eq!(settings_of(&home)["theme"], theme::SETTING, "{before:?}");

        let released = sync(
            root.path(),
            Target::Release {
                terminal_base: terminal,
            },
        )
        .expect("a release");
        assert!(
            released.wrote_settings && released.wrote_theme,
            "{before:?}"
        );
        let settings = settings_of(&home);
        assert_eq!(
            settings.get("theme").and_then(serde_json::Value::as_str),
            key_after,
            "{before:?}"
        );
        if before.is_some() {
            assert_eq!(settings["x"], 1, "{before:?}");
        }
        let file = theme_file(&home).expect("the released file");
        assert_eq!(file["base"], base_after, "{before:?}");
        assert_eq!(file["overrides"], serde_json::json!({}), "{before:?}");
        assert_eq!(read_record(root.path()), None, "{before:?}");
        // And letting go again is nothing at all.
        let idle = sync(
            root.path(),
            Target::Release {
                terminal_base: terminal,
            },
        )
        .expect("a release");
        assert_eq!(idle, Pass::default());
    }
}

/// A theme somebody picked on purpose — colour-blind, ANSI-only, a custom
/// one of their own — is never replaced, a CLI that cannot load a custom
/// theme is never handed one, and neither leaves a file or a record.
#[test]
fn a_theme_picked_on_purpose_and_a_cli_that_cannot_load_one_are_left_alone() {
    for chosen in [
        r#"{"theme":"dark-daltonized"}"#,
        r#"{"theme":"light-ansi"}"#,
        r#"{"theme":"custom:mine"}"#,
        r#"{"theme":7}"#,
    ] {
        let (root, home) = home_with(Some(chosen));
        let asked = Cell::new(false);
        let loads = || {
            asked.set(true);
            true
        };
        let pass = sync(root.path(), matching(&sample("#7aa2f7"), &loads)).expect("a pass");
        assert_eq!(pass, Pass::default(), "{chosen}");
        assert!(
            !asked.get(),
            "the CLI was started for a theme nobody would replace: {chosen}"
        );
        assert_eq!(
            std::fs::read_to_string(settings_path(&home)).expect("settings"),
            chosen
        );
        assert!(
            theme_file(&home).is_none() && read_record(root.path()).is_none(),
            "{chosen}"
        );
    }

    let (root, home) = home_with(Some(r#"{"theme":"dark"}"#));
    let pass = sync(root.path(), matching(&sample("#7aa2f7"), &|| false)).expect("a pass");
    assert_eq!(pass, Pass::default());
    assert_eq!(settings_of(&home)["theme"], "dark");
    assert!(theme_file(&home).is_none() && read_record(root.path()).is_none());

    // No home yet, and a settings file nobody can merge into: nothing.
    let bare = tempfile::tempdir().expect("config root");
    assert_eq!(
        sync(bare.path(), matching(&sample("#7aa2f7"), &|| true)).expect("a pass"),
        Pass::default()
    );
    assert!(!crate::accounts::runtime_home(bare.path()).exists());
    for rubble in ["not json", "[1,2]", "\"dark\""] {
        let (root, home) = home_with(Some(rubble));
        let pass = sync(root.path(), matching(&sample("#7aa2f7"), &|| true)).expect("a pass");
        assert_eq!(pass, Pass::default(), "{rubble}");
        assert_eq!(
            std::fs::read_to_string(settings_path(&home)).expect("settings"),
            rubble
        );
        assert!(read_record(root.path()).is_none(), "{rubble}");
    }
}

/// Settings that name no theme are not a theme nobody picked: Claude Code
/// then shows the one its global config kept from an older CLI. A colour-blind
/// theme kept there is left alone; a default kept there is replaced, and
/// letting go shows it again — the key gone from the settings, as it was.
#[test]
fn a_theme_the_older_global_config_kept_is_the_one_judged() {
    let global = |home: &Path| crate::agent_trust_presets::claude_config_in(home);

    let (root, home) = home_with(Some(r#"{"x":1}"#));
    std::fs::write(
        global(&home),
        r#"{"theme":"dark-daltonized","numStartups":7}"#,
    )
    .expect("global config");
    let asked = Cell::new(false);
    let loads = || {
        asked.set(true);
        true
    };
    let pass = sync(root.path(), matching(&sample("#7aa2f7"), &loads)).expect("a pass");
    assert_eq!(pass, Pass::default());
    assert!(!asked.get(), "the CLI was started for a kept pick");
    assert_eq!(settings_of(&home), serde_json::json!({"x": 1}));
    assert!(theme_file(&home).is_none() && read_record(root.path()).is_none());

    // The settings' own word comes first, as it does for Claude Code: a
    // default named there is replaced whatever the older file still says.
    std::fs::write(settings_path(&home), r#"{"theme":"dark","x":1}"#).expect("settings");
    let pass = sync(root.path(), matching(&sample("#7aa2f7"), &|| true)).expect("a pass");
    assert!(pass.wrote_settings);
    assert_eq!(settings_of(&home)["theme"], theme::SETTING);

    let (root, home) = home_with(None);
    std::fs::write(global(&home), r#"{"theme":"light"}"#).expect("global config");
    let pass = sync(root.path(), matching(&sample("#7aa2f7"), &|| true)).expect("a pass");
    assert!(pass.wrote_settings && pass.wrote_theme);
    assert_eq!(
        read_record(root.path()),
        Some(Record {
            previous_theme: None,
            user_changed: false
        }),
        "the settings named nothing, and nothing is what goes back"
    );
    sync(
        root.path(),
        Target::Release {
            terminal_base: "dark",
        },
    )
    .expect("a release");
    assert_eq!(settings_of(&home).get("theme"), None);
    assert_eq!(
        theme_file(&home).expect("the released file")["base"],
        "light"
    );
    // The global config was read and never written.
    assert_eq!(
        std::fs::read_to_string(global(&home)).expect("global config"),
        r#"{"theme":"light"}"#
    );
}

/// A person who picks another theme after the window's was selected has
/// spoken: the window marks it, stops writing — their theme may be a custom
/// one in the same folder — and letting go leaves their choice where it is.
#[test]
fn a_person_who_then_picks_another_theme_keeps_it() {
    let (root, home) = home_with(Some(r#"{"theme":"dark"}"#));
    let theme = sample("#7aa2f7");
    sync(root.path(), matching(&theme, &|| true)).expect("a pass");
    std::fs::write(settings_path(&home), r#"{"theme":"light-daltonized"}"#).expect("their pick");

    let noticed = sync(root.path(), matching(&sample("#268bd2"), &|| true)).expect("a pass");
    assert_eq!(noticed, Pass::default());
    assert!(read_record(root.path()).expect("a record").user_changed);
    assert_eq!(
        theme_file(&home).expect("the file")["overrides"]["claude"],
        "#7aa2f7"
    );
    let later = sync(root.path(), matching(&sample("#859900"), &|| true)).expect("a pass");
    assert_eq!(later, Pass::default());
    assert_eq!(settings_of(&home)["theme"], "light-daltonized");

    let released = sync(
        root.path(),
        Target::Release {
            terminal_base: "dark",
        },
    )
    .expect("a release");
    assert_eq!(released, Pass::default());
    assert_eq!(settings_of(&home)["theme"], "light-daltonized");
    assert_eq!(read_record(root.path()), None);

    // A selection the window has no record of making is the person's own:
    // the file is kept fresh for it, and the key is never taken or put back.
    let (root, home) = home_with(Some(r#"{"theme":"custom:zerocode"}"#));
    let pass = sync(root.path(), matching(&theme, &|| true)).expect("a pass");
    assert_eq!(
        pass,
        Pass {
            wrote_theme: true,
            wrote_settings: false
        }
    );
    assert!(read_record(root.path()).is_none());
    sync(
        root.path(),
        Target::Release {
            terminal_base: "dark",
        },
    )
    .expect("a release");
    assert_eq!(settings_of(&home)["theme"], theme::SETTING);
}

/// A settings file that cannot be read when the theme is to be put back keeps
/// the record: the restore is owed, and is made by the next pass that can.
#[test]
fn a_restore_that_cannot_be_made_is_still_owed() {
    let (root, home) = home_with(Some(r#"{"theme":"dark"}"#));
    sync(root.path(), matching(&sample("#7aa2f7"), &|| true)).expect("a pass");
    std::fs::write(settings_path(&home), "half a fi").expect("rubble");
    let refused = sync(
        root.path(),
        Target::Release {
            terminal_base: "dark",
        },
    );
    assert!(refused.is_err());
    assert!(
        read_record(root.path()).is_some(),
        "the way back was thrown away"
    );
    std::fs::write(
        settings_path(&home),
        format!(r#"{{"theme":"{}"}}"#, theme::SETTING),
    )
    .expect("whole again");
    sync(
        root.path(),
        Target::Release {
            terminal_base: "dark",
        },
    )
    .expect("a release");
    assert_eq!(settings_of(&home)["theme"], "dark");
    assert_eq!(read_record(root.path()), None);
}

/// The palette a theme is made from is the renderer's own report.
#[test]
fn the_palette_is_the_renderers_report() {
    let mut named = [zerocode_pty::Rgb::default(); 16];
    named[4] = zerocode_pty::Rgb(0x7a, 0xa2, 0xf7);
    let colors = zerocode_pty::TerminalColors::with_standard_tail(
        zerocode_pty::Rgb(0xc0, 0xca, 0xf5),
        zerocode_pty::Rgb(0x1a, 0x1b, 0x26),
        zerocode_pty::Rgb(1, 2, 3),
        named,
    );
    let palette = palette_of(&colors);
    assert_eq!(palette.foreground, [0xc0, 0xca, 0xf5]);
    assert_eq!(palette.background, [0x1a, 0x1b, 0x26]);
    assert_eq!(palette.ansi[4], [0x7a, 0xa2, 0xf7]);
    assert_eq!(theme::theme_from_terminal(&palette).base, "dark");
}
