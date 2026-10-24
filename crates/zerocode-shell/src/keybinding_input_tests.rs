use super::*;

#[test]
fn keybinding_markup_and_control_sequences_are_refused() {
    for key in [
        "<b>demo</b>",
        "&lt;b&gt;",
        "arrow left",
        "a\nb",
        "\0",
        "\u{1b}[A",
        "x+y",
    ] {
        let chord = format!("mod+{key}");
        assert!(
            normalize_keybinding_chord(&chord).is_err(),
            "accepted {chord:?}"
        );
    }
}

#[test]
fn keybinding_character_and_named_keys_keep_the_renderer_grammar() {
    for key in [
        "t",
        "`",
        "=",
        "[",
        "<",
        " ",
        "한",
        "arrowleft",
        "enter",
        "f24",
        "mediaplaypause",
    ] {
        for modifiers in ["mod", "mod+alt", "mod+shift", "mod+alt+shift"] {
            let chord = format!("{modifiers}+{key}");
            assert_eq!(normalize_keybinding_chord(&chord), Ok(chord.clone()));
        }
    }
    assert_eq!(
        normalize_keybinding_chord(" MOD+SHIFT+ENTER "),
        Ok("mod+shift+enter".into())
    );
}

#[test]
fn keybinding_read_and_write_use_the_same_validation_without_losing_reset_or_disable() {
    let action = "terminal.newTab";
    let mut overrides = BTreeMap::from([(action.into(), vec!["mod+<b>demo</b>".into()])]);
    assert_eq!(
        effective_keybinding_chords(&overrides, action, &["mod+t"]),
        ["mod+t"]
    );
    let before = overrides.clone();
    assert!(
        apply_keybinding(
            &mut overrides,
            action.into(),
            Some(vec!["mod+<b>demo</b>".into()])
        )
        .is_err()
    );
    assert_eq!(overrides, before);
    apply_keybinding(&mut overrides, action.into(), Some(vec![])).unwrap();
    assert!(effective_keybinding_chords(&overrides, action, &["mod+t"]).is_empty());
    apply_keybinding(&mut overrides, action.into(), None).unwrap();
    assert_eq!(
        effective_keybinding_chords(&overrides, action, &["mod+t"]),
        ["mod+t"]
    );
}
