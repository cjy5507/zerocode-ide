//! The key vocabulary `press-key`, `hotkey` and `--modifiers` share.
//!
//! `KeyMap` from the macOS helper, minus the key codes: the NAMES are the
//! contract (`return`, `tab`, `cmdorctrl+a`, …) and each platform keeps its
//! own code table. Two things the Swift folded together are kept apart here
//! because they only coincide on macOS: `CmdOrCtrl` (the platform's primary
//! modifier — ⌘ there, Ctrl on Windows) and `Cmd`/`Meta`/`Super`/`Win` (the
//! system key itself).

use super::ProviderError;

/// A modifier as the agent names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Modifier {
    /// `cmdorctrl`, `commandorcontrol`: ⌘ on macOS, Ctrl on Windows.
    Primary,
    /// `cmd`, `command`, `meta`, `super`, `win`: ⌘ on macOS, the Windows key on Windows.
    Command,
    /// `ctrl`, `control`.
    Control,
    /// `alt`, `option`.
    Alt,
    /// `shift`.
    Shift,
}

/// The platform a key chord is interpreted on. Only the modifier meanings
/// differ: what the primary editing modifier IS, and what `Cmd` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
}

impl Modifier {
    #[must_use]
    pub fn parse(part: &str) -> Option<Self> {
        match part {
            "cmdorctrl" | "commandorcontrol" => Some(Self::Primary),
            "cmd" | "command" | "meta" | "super" | "win" => Some(Self::Command),
            "ctrl" | "control" => Some(Self::Control),
            "alt" | "option" => Some(Self::Alt),
            "shift" => Some(Self::Shift),
            _ => None,
        }
    }

    /// Whether this modifier is the platform's primary editing modifier —
    /// the one `+a` selects all with. `Cmd` is that on macOS and the Windows
    /// key on Windows, where `Ctrl` is; `CmdOrCtrl` is it everywhere. One
    /// answer per platform, whatever road the action then takes.
    #[must_use]
    pub const fn is_primary_on(self, platform: Platform) -> bool {
        matches!(
            (self, platform),
            (Self::Primary, _)
                | (Self::Command, Platform::MacOs)
                | (Self::Control, Platform::Windows)
        )
    }
}

/// Every key name the helper's code table knows, in its spelling. A platform
/// maps each to its own code; a name outside this list is `invalid_argument`
/// on every platform, so an agent's key vocabulary is one vocabulary.
pub const KEY_NAMES: &[&str] = &[
    "a",
    "b",
    "c",
    "d",
    "e",
    "f",
    "g",
    "h",
    "i",
    "j",
    "k",
    "l",
    "m",
    "n",
    "o",
    "p",
    "q",
    "r",
    "s",
    "t",
    "u",
    "v",
    "w",
    "x",
    "y",
    "z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "=",
    "-",
    "]",
    "[",
    "'",
    ";",
    "\\",
    ",",
    "/",
    ".",
    "`",
    "return",
    "enter",
    "tab",
    "space",
    "backspace",
    "delete",
    "escape",
    "esc",
    "left",
    "right",
    "down",
    "up",
    "insert",
    "home",
    "pageup",
    "page_up",
    "forwarddelete",
    "end",
    "pagedown",
    "page_down",
];

/// The letter a person selects everything with, and the one they paste with —
/// each held with the platform's primary modifier.
pub const SELECT_ALL_KEY: &str = "a";
pub const PASTE_KEY: &str = "v";

/// What one key chord writes into the field that has the focus (B3): nothing,
/// a character — a printable key held with nothing but shift (or, on macOS,
/// option) — or the clipboard's text — the platform's paste chord. Neither of
/// the last two may go into a secret field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChordWrites {
    Nothing,
    Character,
    Clipboard,
}

impl ChordWrites {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Nothing => "nothing",
            Self::Character => "character",
            Self::Clipboard => "clipboard",
        }
    }
}

/// The key a chord pastes with besides the primary modifier's `v`: Windows'
/// `shift+insert`, which every edit control and browser honours.
pub const WINDOWS_PASTE_INSERT: &str = "insert";

/// Whether a key name types a character of its own: a single printable glyph
/// or `space`.
#[must_use]
pub fn is_printable_key(name: &str) -> bool {
    name == "space" || name.chars().count() == 1
}

/// The parsed `--key`: modifiers in the order written, one key name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChord {
    pub modifiers: Vec<Modifier>,
    pub key: String,
}

impl KeyChord {
    /// `isSelectAllHotkey`, decided from the parsed modifiers: `a` with the
    /// platform's primary modifier held (other modifiers may ride along, as
    /// the helper allows). A provider's accessibility fast path and its
    /// synthetic path must both ask THIS, so a chord means one thing.
    #[must_use]
    pub fn selects_all_on(&self, platform: Platform) -> bool {
        self.primary_with(SELECT_ALL_KEY, platform)
    }

    /// Whether the chord pastes the clipboard on `platform` — text typed into
    /// whatever has the focus, a password field included.
    #[must_use]
    pub fn pastes_on(&self, platform: Platform) -> bool {
        self.writes_on(platform) == ChordWrites::Clipboard
    }

    /// What the chord writes on `platform`: the paste chord the clipboard, a
    /// printable key with only text modifiers a character, else nothing.
    #[must_use]
    pub fn writes_on(&self, platform: Platform) -> ChordWrites {
        let only = |allowed: &[Modifier]| self.modifiers.iter().all(|held| allowed.contains(held));
        if self.primary_with(PASTE_KEY, platform)
            || (platform == Platform::Windows
                && self.key == WINDOWS_PASTE_INSERT
                && self.modifiers == [Modifier::Shift])
        {
            return ChordWrites::Clipboard;
        }
        let text_modifiers: &[Modifier] = match platform {
            Platform::MacOs => &[Modifier::Shift, Modifier::Alt],
            Platform::Windows => &[Modifier::Shift],
        };
        if is_printable_key(&self.key) && only(text_modifiers) {
            ChordWrites::Character
        } else {
            ChordWrites::Nothing
        }
    }

    /// `key` with the platform's primary modifier held (other modifiers may
    /// ride along).
    #[must_use]
    pub fn primary_with(&self, key: &str, platform: Platform) -> bool {
        self.key == key
            && self
                .modifiers
                .iter()
                .any(|modifier| modifier.is_primary_on(platform))
    }
}

/// `KeyMap.parse`: split on `+`, lowercase, modifiers to the side, one key.
pub fn parse_key_spec(spec: &str) -> Result<KeyChord, ProviderError> {
    let mut modifiers = Vec::new();
    let mut key_name: Option<String> = None;
    for part in spec.split('+').map(str::to_lowercase) {
        if let Some(modifier) = Modifier::parse(&part) {
            modifiers.push(modifier);
        } else {
            key_name = Some(part);
        }
    }
    match key_name {
        Some(name) if KEY_NAMES.contains(&name.as_str()) => Ok(KeyChord {
            modifiers,
            key: name,
        }),
        _ => Err(ProviderError::invalid_argument(format!(
            "unsupported key '{spec}'"
        ))),
    }
}

/// `KeyMap.parseModifiers`: the `--modifiers` of a click — modifiers only,
/// none empty.
pub fn parse_click_modifiers(spec: Option<&str>) -> Result<Vec<Modifier>, ProviderError> {
    let Some(spec) = spec else {
        return Ok(Vec::new());
    };
    let parts: Vec<String> = spec
        .split('+')
        .map(|part| part.trim().to_lowercase())
        .collect();
    if parts.is_empty() || parts.iter().any(String::is_empty) {
        return Err(ProviderError::invalid_argument(
            "click modifiers require modifier keys only",
        ));
    }
    parts
        .iter()
        .map(|part| {
            Modifier::parse(part).ok_or_else(|| {
                ProviderError::invalid_argument(format!("unsupported click modifier '{part}'"))
            })
        })
        .collect()
}

/// `isSelectAllHotkey` as the macOS helper spells it — tolerant of spaces
/// and `-` between parts, which `parse_key_spec` is not — decided by the
/// same modifier meanings as [`KeyChord::selects_all_on`].
#[must_use]
pub fn is_select_all_hotkey(key: &str) -> bool {
    is_primary_hotkey(key, SELECT_ALL_KEY)
}

/// What a `--key` writes, parsed exactly as the key is then posted
/// (`KeyMap.parse` / `parse_key_spec`), so the check and the keys cannot
/// read one chord two ways. A chord that does not parse writes nothing — it
/// is refused before anything is posted.
#[must_use]
pub fn chord_writes(key: &str, platform: Platform) -> ChordWrites {
    parse_key_spec(key).map_or(ChordWrites::Nothing, |chord| chord.writes_on(platform))
}

/// `isPrimaryHotkey`: `letter` with the macOS primary modifier held.
#[must_use]
pub fn is_primary_hotkey(key: &str, letter: &str) -> bool {
    let normalized = key.to_lowercase().replace(' ', "").replace('-', "+");
    let parts: Vec<&str> = normalized
        .split('+')
        .filter(|part| !part.is_empty())
        .collect();
    let Some((last, modifiers)) = parts.split_last() else {
        return false;
    };
    *last == letter
        && modifiers
            .iter()
            .filter_map(|part| Modifier::parse(part))
            .any(|modifier| modifier.is_primary_on(Platform::MacOs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spec_splits_into_modifiers_and_one_known_key() {
        let chord = parse_key_spec("CmdOrCtrl+Shift+Return").unwrap();
        assert_eq!(chord.modifiers, vec![Modifier::Primary, Modifier::Shift]);
        assert_eq!(chord.key, "return");
        let win = parse_key_spec("win+d").unwrap();
        assert_eq!(win.modifiers, vec![Modifier::Command]);
        assert_eq!(parse_key_spec("Page_Down").unwrap().key, "page_down");
        assert_eq!(parse_key_spec("esc").unwrap().modifiers, vec![]);
        let refused = parse_key_spec("ctrl+f13").unwrap_err();
        assert_eq!(refused.code, "invalid_argument");
        assert_eq!(refused.message, "unsupported key 'ctrl+f13'");
        assert!(
            parse_key_spec("ctrl+shift").is_err(),
            "modifiers alone name no key"
        );
    }

    #[test]
    fn click_modifiers_are_modifiers_only() {
        assert_eq!(parse_click_modifiers(None).unwrap(), vec![]);
        assert_eq!(
            parse_click_modifiers(Some("Shift + Alt")).unwrap(),
            vec![Modifier::Shift, Modifier::Alt]
        );
        assert_eq!(
            parse_click_modifiers(Some("shift+")).unwrap_err().message,
            "click modifiers require modifier keys only"
        );
        assert_eq!(
            parse_click_modifiers(Some("shift+a")).unwrap_err().message,
            "unsupported click modifier 'a'"
        );
    }

    /// The one case table of what a chord writes, run by the Rust core and
    /// the Swift helper's mirror alike.
    #[test]
    fn what_a_chord_writes_is_read_off_the_shared_case_table() {
        let table = include_str!("cases/key_writes.tsv");
        let mut rows = 0;
        for line in table
            .lines()
            .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        {
            let cells: Vec<&str> = line.split('\t').collect();
            let platform = match cells[1] {
                "macos" => Platform::MacOs,
                "windows" => Platform::Windows,
                other => panic!("not a platform: {other}"),
            };
            assert_eq!(
                chord_writes(cells[0], platform).as_str(),
                cells[2],
                "{line}"
            );
            rows += 1;
        }
        assert!(rows >= 20, "the table was read: {rows}");
    }

    #[test]
    fn select_all_is_recognised_in_every_spelling_the_helper_accepts() {
        for spelling in [
            "cmd+a",
            "Command+A",
            "meta-a",
            "CmdOrCtrl + a",
            "commandorcontrol+a",
        ] {
            assert!(is_select_all_hotkey(spelling), "{spelling}");
        }
        assert!(
            !is_select_all_hotkey("ctrl+a"),
            "the helper's list is ⌘ only"
        );
        assert!(!is_select_all_hotkey("cmd+b"));
        assert!(!is_select_all_hotkey("a"));
        // The paste chord is the same rule with its own letter: it types the
        // clipboard's text, so a password field refuses it like typing.
        assert!(
            parse_key_spec("ctrl+v")
                .unwrap()
                .pastes_on(Platform::Windows)
                && parse_key_spec("CmdOrCtrl+V")
                    .unwrap()
                    .pastes_on(Platform::Windows)
        );
        assert!(
            !parse_key_spec("cmd+v")
                .unwrap()
                .pastes_on(Platform::Windows)
        );
    }

    /// One chord, one meaning per platform: on Windows `cmd+a` is Win+A (a
    /// system panel) whatever the focused element offers, and only the
    /// primary-modifier spellings select all — the spellings the skill
    /// documents. The accessibility fast path and the synthetic path ask the
    /// same parsed question, so the answer cannot depend on a Text pattern.
    #[test]
    fn select_all_is_decided_from_parsed_modifiers_per_platform() {
        for spelling in [
            "ctrl+a",
            "Control+A",
            "cmdorctrl+a",
            "CommandOrControl+a",
            "ctrl+shift+a",
        ] {
            let chord = parse_key_spec(spelling).unwrap();
            assert!(chord.selects_all_on(Platform::Windows), "{spelling}");
        }
        for spelling in [
            "cmd+a",
            "command+a",
            "meta+a",
            "super+a",
            "win+a",
            "alt+a",
            "ctrl+b",
        ] {
            let chord = parse_key_spec(spelling).unwrap();
            assert!(!chord.selects_all_on(Platform::Windows), "{spelling}");
        }
        for spelling in ["cmd+a", "command+a", "meta+a", "cmdorctrl+a"] {
            assert!(
                parse_key_spec(spelling)
                    .unwrap()
                    .selects_all_on(Platform::MacOs),
                "{spelling}"
            );
        }
        assert!(
            !parse_key_spec("ctrl+a")
                .unwrap()
                .selects_all_on(Platform::MacOs)
        );
        // Every spelling the Windows skill calls select-all parses to a
        // modifier that is primary there, and `Cmd` never does.
        for spelling in ["ctrl", "control", "cmdorctrl", "commandorcontrol"] {
            assert!(
                Modifier::parse(spelling)
                    .unwrap()
                    .is_primary_on(Platform::Windows),
                "{spelling}"
            );
        }
        for spelling in ["cmd", "command", "meta", "super", "win"] {
            assert!(
                !Modifier::parse(spelling)
                    .unwrap()
                    .is_primary_on(Platform::Windows),
                "{spelling}"
            );
            assert!(
                Modifier::parse(spelling)
                    .unwrap()
                    .is_primary_on(Platform::MacOs),
                "{spelling}"
            );
        }
    }

    #[test]
    fn the_vocabulary_is_the_helpers_table() {
        assert_eq!(KEY_NAMES.len(), 67);
        assert!(KEY_NAMES.contains(&"forwarddelete"));
        assert!(KEY_NAMES.contains(&"\\"));
        assert!(!KEY_NAMES.contains(&"f1"));
    }
}
