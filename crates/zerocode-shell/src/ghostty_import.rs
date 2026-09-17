//! Orca-compatible Ghostty settings import.
//!
//! Discovery, bounded file reads, parsing, and theme resolution stay native.
//! The renderer receives a typed preview and can only submit that same typed
//! patch through the settings repository's atomic transaction.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    MacOptionAsAlt, SettingsDocument, TERM_LINE_HEIGHT_BASE_LEADING, TerminalCursorStyle,
    durable_file, normalized_optional_hex_color,
};

const MAX_CONFIG_BYTES: u64 = 1_000_000;
const MAX_THEME_BYTES: u64 = 262_144;
const MAX_UNSUPPORTED_KEYS: usize = 100;
const UNSUPPORTED_OVERFLOW: &str = "additional keys omitted";

const PALETTE_KEYS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "brightBlack",
    "brightRed",
    "brightGreen",
    "brightYellow",
    "brightBlue",
    "brightMagenta",
    "brightCyan",
    "brightWhite",
];

const THEME_COLOR_KEYS: [&str; 9] = [
    "palette",
    "background",
    "foreground",
    "cursor-color",
    "cursor-text",
    "selection-background",
    "selection-foreground",
    "bold-color",
    "split-divider-color",
];

type ParsedConfig = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct GhosttyImportPatch {
    pub(crate) mac_option_as_alt: Option<MacOptionAsAlt>,
    pub(crate) terminal_opacity: Option<f64>,
    pub(crate) window_blur: Option<bool>,
    pub(crate) color_overrides: BTreeMap<String, String>,
    pub(crate) divider_color_dark: Option<String>,
    pub(crate) divider_color_light: Option<String>,
    pub(crate) inactive_pane_opacity: Option<f32>,
    pub(crate) padding_x: Option<u32>,
    pub(crate) padding_y: Option<u32>,
    /// Orca's visible multiplier. ZeroCode converts it to its CSS baseline
    /// storage unit only at the settings boundary.
    pub(crate) line_height: Option<f32>,
    pub(crate) hide_mouse_while_typing: Option<bool>,
    pub(crate) cursor_opacity: Option<f32>,
    pub(crate) font_family: Option<String>,
    pub(crate) font_size: Option<u32>,
    pub(crate) font_weight: Option<u32>,
    pub(crate) cursor_style: Option<TerminalCursorStyle>,
    pub(crate) cursor_blink: Option<bool>,
    pub(crate) focus_follows_mouse: Option<bool>,
    pub(crate) primary_selection_middle_click_paste: Option<bool>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GhosttyImportChange {
    pub(crate) key: &'static str,
    pub(crate) value: String,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GhosttyImportPreview {
    pub(crate) found: bool,
    pub(crate) config_paths: Vec<String>,
    pub(crate) patch: GhosttyImportPatch,
    pub(crate) changes: Vec<GhosttyImportChange>,
    pub(crate) unsupported_keys: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

impl GhosttyImportPreview {
    fn failure(error: impl Into<String>) -> Self {
        Self {
            error: Some(error.into()),
            ..Self::default()
        }
    }

    pub(crate) fn against(mut self, current: &SettingsDocument) -> Self {
        if !self.found || self.error.is_some() {
            return self;
        }
        self.patch = self.patch.normalized_against(current);
        self.changes = self.patch.changes();
        self
    }
}

impl GhosttyImportPatch {
    pub(crate) fn apply(self, settings: &mut SettingsDocument) {
        let terminal = &mut settings.terminal_prefs;
        if let Some(value) = self.mac_option_as_alt {
            terminal.mac_option_as_alt = value;
        }
        if let Some(value) = self.terminal_opacity {
            settings.window_material.terminal_opacity = value;
        }
        if let Some(value) = self.window_blur {
            settings.window_material.blur = value;
        }
        terminal.color_overrides.extend(self.color_overrides);
        if let Some(value) = self.divider_color_dark {
            terminal.divider_color_dark = value;
        }
        if let Some(value) = self.divider_color_light {
            terminal.divider_color_light = value;
        }
        if let Some(value) = self.inactive_pane_opacity {
            terminal.inactive_pane_opacity = value;
        }
        if let Some(value) = self.padding_x {
            terminal.padding_x = value;
        }
        if let Some(value) = self.padding_y {
            terminal.padding_y = value;
        }
        if let Some(value) = self.line_height {
            terminal.leading = value * TERM_LINE_HEIGHT_BASE_LEADING;
        }
        if let Some(value) = self.hide_mouse_while_typing {
            terminal.hide_mouse_while_typing = value;
        }
        if let Some(value) = self.cursor_opacity {
            terminal.cursor_opacity = value;
        }
        if let Some(value) = self.font_family {
            terminal.font_family = value;
        }
        if let Some(value) = self.font_size {
            terminal.font_size = value;
        }
        if let Some(value) = self.font_weight {
            terminal.weight = value;
        }
        if let Some(value) = self.cursor_style {
            terminal.cursor_style = value;
        }
        if let Some(value) = self.cursor_blink {
            terminal.cursor_blink = value;
        }
        if let Some(value) = self.focus_follows_mouse {
            terminal.focus_follows_mouse = value;
        }
        if let Some(value) = self.primary_selection_middle_click_paste {
            settings.editing_prefs.primary_selection_middle_click_paste = Some(value);
        }
    }

    fn normalized_against(&self, current: &SettingsDocument) -> Self {
        let mut candidate = current.clone();
        self.clone().apply(&mut candidate);
        candidate.terminal_prefs = candidate.terminal_prefs.clamped();
        candidate.window_material = candidate.window_material.clamped();
        candidate.editing_prefs = candidate.editing_prefs.clamped();

        let before = &current.terminal_prefs;
        let after = &candidate.terminal_prefs;
        let mut kept = Self::default();
        keep_changed(
            &mut kept.mac_option_as_alt,
            self.mac_option_as_alt,
            before.mac_option_as_alt,
            after.mac_option_as_alt,
        );
        keep_changed(
            &mut kept.terminal_opacity,
            self.terminal_opacity
                .map(|_| candidate.window_material.terminal_opacity),
            current.window_material.terminal_opacity,
            candidate.window_material.terminal_opacity,
        );
        keep_changed(
            &mut kept.window_blur,
            self.window_blur.map(|_| candidate.window_material.blur),
            current.window_material.blur,
            candidate.window_material.blur,
        );
        for key in self.color_overrides.keys() {
            let before_color = before.color_overrides.get(key);
            let after_color = after.color_overrides.get(key);
            if before_color != after_color
                && let Some(color) = after_color
            {
                kept.color_overrides.insert(key.clone(), color.clone());
            }
        }
        keep_changed(
            &mut kept.divider_color_dark,
            self.divider_color_dark
                .as_ref()
                .map(|_| after.divider_color_dark.clone()),
            before.divider_color_dark.clone(),
            after.divider_color_dark.clone(),
        );
        keep_changed(
            &mut kept.divider_color_light,
            self.divider_color_light
                .as_ref()
                .map(|_| after.divider_color_light.clone()),
            before.divider_color_light.clone(),
            after.divider_color_light.clone(),
        );
        keep_changed(
            &mut kept.inactive_pane_opacity,
            self.inactive_pane_opacity
                .map(|_| after.inactive_pane_opacity),
            before.inactive_pane_opacity,
            after.inactive_pane_opacity,
        );
        keep_changed(
            &mut kept.padding_x,
            self.padding_x.map(|_| after.padding_x),
            before.padding_x,
            after.padding_x,
        );
        keep_changed(
            &mut kept.padding_y,
            self.padding_y.map(|_| after.padding_y),
            before.padding_y,
            after.padding_y,
        );
        keep_changed(
            &mut kept.line_height,
            self.line_height
                .map(|_| after.leading / TERM_LINE_HEIGHT_BASE_LEADING),
            before.leading,
            after.leading,
        );
        keep_changed(
            &mut kept.hide_mouse_while_typing,
            self.hide_mouse_while_typing
                .map(|_| after.hide_mouse_while_typing),
            before.hide_mouse_while_typing,
            after.hide_mouse_while_typing,
        );
        keep_changed(
            &mut kept.cursor_opacity,
            self.cursor_opacity.map(|_| after.cursor_opacity),
            before.cursor_opacity,
            after.cursor_opacity,
        );
        keep_changed(
            &mut kept.font_family,
            self.font_family.as_ref().map(|_| after.font_family.clone()),
            before.font_family.clone(),
            after.font_family.clone(),
        );
        keep_changed(
            &mut kept.font_size,
            self.font_size.map(|_| after.font_size),
            before.font_size,
            after.font_size,
        );
        keep_changed(
            &mut kept.font_weight,
            self.font_weight.map(|_| after.weight),
            before.weight,
            after.weight,
        );
        keep_changed(
            &mut kept.cursor_style,
            self.cursor_style.map(|_| after.cursor_style),
            before.cursor_style,
            after.cursor_style,
        );
        keep_changed(
            &mut kept.cursor_blink,
            self.cursor_blink.map(|_| after.cursor_blink),
            before.cursor_blink,
            after.cursor_blink,
        );
        keep_changed(
            &mut kept.focus_follows_mouse,
            self.focus_follows_mouse.map(|_| after.focus_follows_mouse),
            before.focus_follows_mouse,
            after.focus_follows_mouse,
        );
        let before_middle = current.editing_prefs.primary_selection_middle_click_paste;
        let after_middle = candidate.editing_prefs.primary_selection_middle_click_paste;
        if self.primary_selection_middle_click_paste.is_some() && before_middle != after_middle {
            kept.primary_selection_middle_click_paste = after_middle;
        }
        kept
    }

    fn changes(&self) -> Vec<GhosttyImportChange> {
        let mut changes = Vec::new();
        push_change(
            &mut changes,
            "terminalMacOptionAsAlt",
            self.mac_option_as_alt,
        );
        push_change(
            &mut changes,
            "terminalBackgroundOpacity",
            self.terminal_opacity,
        );
        push_change(&mut changes, "windowBackgroundBlur", self.window_blur);
        if !self.color_overrides.is_empty() {
            let value = self
                .color_overrides
                .iter()
                .map(|(key, value)| format!("{key}: {value}"))
                .collect::<Vec<_>>()
                .join(", ");
            changes.push(GhosttyImportChange {
                key: "terminalColorOverrides",
                value,
            });
        }
        push_change(
            &mut changes,
            "terminalDividerColorDark",
            self.divider_color_dark.as_deref(),
        );
        push_change(
            &mut changes,
            "terminalDividerColorLight",
            self.divider_color_light.as_deref(),
        );
        push_change(
            &mut changes,
            "terminalInactivePaneOpacity",
            self.inactive_pane_opacity,
        );
        push_change(&mut changes, "terminalPaddingX", self.padding_x);
        push_change(&mut changes, "terminalPaddingY", self.padding_y);
        push_change(&mut changes, "terminalLineHeight", self.line_height);
        push_change(
            &mut changes,
            "terminalMouseHideWhileTyping",
            self.hide_mouse_while_typing,
        );
        push_change(&mut changes, "terminalCursorOpacity", self.cursor_opacity);
        push_change(
            &mut changes,
            "terminalFontFamily",
            self.font_family.as_deref(),
        );
        push_change(&mut changes, "terminalFontSize", self.font_size);
        push_change(&mut changes, "terminalFontWeight", self.font_weight);
        push_change(&mut changes, "terminalCursorStyle", self.cursor_style);
        push_change(&mut changes, "terminalCursorBlink", self.cursor_blink);
        push_change(
            &mut changes,
            "terminalFocusFollowsMouse",
            self.focus_follows_mouse,
        );
        push_change(
            &mut changes,
            "primarySelectionMiddleClickPaste",
            self.primary_selection_middle_click_paste,
        );
        changes
    }
}

fn keep_changed<T: PartialEq>(slot: &mut Option<T>, proposed: Option<T>, before: T, after: T) {
    if proposed.is_some() && before != after {
        *slot = proposed;
    }
}

fn push_change<T: Serialize>(
    changes: &mut Vec<GhosttyImportChange>,
    key: &'static str,
    value: Option<T>,
) {
    if let Some(value) = value {
        let value = match serde_json::to_value(value).expect("Ghostty change values serialize") {
            serde_json::Value::String(value) => value,
            value => value.to_string(),
        };
        changes.push(GhosttyImportChange { key, value });
    }
}

pub(crate) fn preview_auto() -> GhosttyImportPreview {
    let config_paths = config_paths();
    let theme_dirs = theme_search_dirs();
    preview_paths(&config_paths, &theme_dirs, cfg!(target_os = "macos"))
}

fn preview_paths(
    candidates: &[PathBuf],
    theme_dirs: &[PathBuf],
    is_macos: bool,
) -> GhosttyImportPreview {
    let mut found = Vec::new();
    for candidate in candidates {
        match fs::symlink_metadata(candidate) {
            Ok(metadata) if durable_file::is_plain_file(&metadata) => found.push(candidate.clone()),
            Ok(_) => {
                return GhosttyImportPreview::failure(format!(
                    "Ghostty config is not a plain file: {}",
                    candidate.display()
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return GhosttyImportPreview::failure(format!(
                    "Could not inspect Ghostty config {}: {error}",
                    candidate.display()
                ));
            }
        }
    }
    if found.is_empty() {
        return GhosttyImportPreview::default();
    }

    let mut parsed = ParsedConfig::new();
    for path in &found {
        let content = match read_bounded_utf8(path, MAX_CONFIG_BYTES) {
            Ok(content) => content,
            Err(error) => {
                return GhosttyImportPreview::failure(format!(
                    "Could not read Ghostty config {}: {error}",
                    path.display()
                ));
            }
        };
        merge_parsed_config(&mut parsed, parse_config(&content));
    }

    let mut unsupported = UnsupportedKeys::default();
    apply_theme_reference(&mut parsed, theme_dirs, &mut unsupported);
    let patch = map_config(&parsed, is_macos, &mut unsupported);
    GhosttyImportPreview {
        found: true,
        config_paths: found
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        patch,
        changes: Vec::new(),
        unsupported_keys: unsupported.finish(),
        error: None,
    }
}

fn config_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    #[cfg(target_os = "windows")]
    let directories = vec![
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("ghostty"),
    ];
    #[cfg(target_os = "macos")]
    let directories = vec![
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("ghostty"),
        home.join("Library")
            .join("Application Support")
            .join("com.mitchellh.ghostty"),
    ];
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    let directories = vec![
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("ghostty"),
    ];
    directories
        .into_iter()
        .flat_map(|directory| [directory.join("config.ghostty"), directory.join("config")])
        .collect()
}

fn theme_search_dirs() -> Vec<PathBuf> {
    if cfg!(not(any(target_os = "macos", target_os = "linux"))) {
        return Vec::new();
    }
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut directories = vec![
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("ghostty")
            .join("themes"),
    ];
    if let Some(resources) = std::env::var_os("GHOSTTY_RESOURCES_DIR") {
        directories.push(PathBuf::from(resources).join("themes"));
    } else if cfg!(target_os = "macos") {
        directories.push(PathBuf::from(
            "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
        ));
    } else {
        directories.push(PathBuf::from("/usr/share/ghostty/themes"));
        directories.push(PathBuf::from("/usr/local/share/ghostty/themes"));
    }
    directories
}

fn read_bounded_utf8(path: &Path, limit: u64) -> io::Result<String> {
    let mut file = durable_file::open_plain_file(path)?;
    if file.metadata()?.len() > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file exceeds the {limit}-byte import limit"),
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref().take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("file exceeds the {limit}-byte import limit"),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file is not UTF-8"))
}

fn parse_config(content: &str) -> ParsedConfig {
    let mut parsed = ParsedConfig::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        if key.is_empty() {
            continue;
        }
        let value = unquote(strip_inline_comment(raw_value.trim()));
        parsed.entry(key.to_string()).or_default().push(value);
    }
    parsed
}

fn strip_inline_comment(value: &str) -> &str {
    let mut single = false;
    let mut double = false;
    let mut previous = None;
    for (index, character) in value.char_indices() {
        match character {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single && !double && previous.is_some_and(char::is_whitespace) => {
                return value[..index].trim();
            }
            _ => {}
        }
        previous = Some(character);
    }
    value.trim()
}

fn unquote(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        value[1..value.len() - 1].to_string()
    } else {
        value.to_string()
    }
}

fn merge_parsed_config(target: &mut ParsedConfig, incoming: ParsedConfig) {
    for (key, values) in incoming {
        if key == "palette" {
            target.entry(key).or_default().extend(values);
        } else {
            target.insert(key, values);
        }
    }
}

fn apply_theme_reference(
    parsed: &mut ParsedConfig,
    theme_dirs: &[PathBuf],
    unsupported: &mut UnsupportedKeys,
) {
    let Some(theme_values) = parsed.remove("theme") else {
        return;
    };
    let theme_name = last_value(&theme_values).trim();
    if theme_name
        .split(',')
        .map(str::trim)
        .any(|part| part.starts_with("light:") || part.starts_with("dark:"))
    {
        unsupported.insert("theme (light:/dark: pairs not supported)");
        return;
    }
    let Some(theme) = resolve_theme(theme_name, theme_dirs) else {
        unsupported.insert("theme (theme file not found)");
        return;
    };
    for (key, values) in theme {
        if key == "palette" {
            let mut merged = values;
            merged.extend(parsed.remove(&key).unwrap_or_default());
            parsed.insert(key, merged);
        } else {
            parsed.entry(key).or_insert(values);
        }
    }
}

fn resolve_theme(name: &str, theme_dirs: &[PathBuf]) -> Option<ParsedConfig> {
    let candidates = if Path::new(name).is_absolute() {
        vec![PathBuf::from(name)]
    } else {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
        {
            return None;
        }
        theme_dirs
            .iter()
            .map(|directory| directory.join(name))
            .collect()
    };
    for candidate in candidates {
        match read_bounded_utf8(&candidate, MAX_THEME_BYTES) {
            Ok(content) => {
                let mut parsed = parse_config(&content);
                parsed.retain(|key, _| THEME_COLOR_KEYS.contains(&key.as_str()));
                return Some(parsed);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    None
}

fn map_config(
    parsed: &ParsedConfig,
    is_macos: bool,
    unsupported: &mut UnsupportedKeys,
) -> GhosttyImportPatch {
    let mut patch = GhosttyImportPatch::default();
    for (key, raw_values) in parsed {
        let value = last_value(raw_values).trim();
        if value.is_empty() || matches!(key.as_str(), "selection-word-chars" | "bold-color") {
            unsupported.insert(key);
            continue;
        }
        let accepted = match key.as_str() {
            "macos-option-as-alt" if is_macos => parse_option_as_alt(value)
                .map(|parsed| patch.mac_option_as_alt = Some(parsed))
                .is_some(),
            "background-opacity" => parse_unit_f64(value)
                .map(|parsed| patch.terminal_opacity = Some(parsed))
                .is_some(),
            "background" => insert_color(&mut patch.color_overrides, "background", value),
            "foreground" => insert_color(&mut patch.color_overrides, "foreground", value),
            "cursor-color" => insert_color(&mut patch.color_overrides, "cursor", value),
            "cursor-text" => insert_color(&mut patch.color_overrides, "cursorAccent", value),
            "selection-background" => {
                insert_color(&mut patch.color_overrides, "selectionBackground", value)
            }
            "selection-foreground" => {
                insert_color(&mut patch.color_overrides, "selectionForeground", value)
            }
            "palette" => parse_palette(raw_values, &mut patch.color_overrides),
            "background-blur-radius" => parse_nonnegative_u32(value)
                .map(|radius| {
                    patch.window_blur = Some(radius > 0);
                    if radius > 0 {
                        unsupported.insert("background-blur-radius (radius value not preserved)");
                    }
                })
                .is_some(),
            "split-divider-color" => normalized_optional_hex_color(value)
                .map(|color| {
                    patch.divider_color_dark = Some(color.clone());
                    patch.divider_color_light = Some(color);
                })
                .is_some(),
            "unfocused-split-opacity" => parse_unit_f32(value)
                .map(|parsed| patch.inactive_pane_opacity = Some(parsed))
                .is_some(),
            "window-padding-x" => parse_padding(value)
                .map(|parsed| patch.padding_x = Some(parsed))
                .is_some(),
            "window-padding-y" => parse_padding(value)
                .map(|parsed| patch.padding_y = Some(parsed))
                .is_some(),
            "adjust-cell-height" => parse_cell_height(value)
                .map(|parsed| patch.line_height = Some(parsed))
                .is_some(),
            "mouse-hide-while-typing" => parse_bool(value)
                .map(|parsed| patch.hide_mouse_while_typing = Some(parsed))
                .is_some(),
            "cursor-opacity" => parse_unit_f32(value)
                .map(|parsed| patch.cursor_opacity = Some(parsed))
                .is_some(),
            "font-family" => {
                patch.font_family = Some(value.to_string());
                true
            }
            "font-size" => parse_positive_u32(value)
                .map(|parsed| patch.font_size = Some(parsed))
                .is_some(),
            "font-weight" => parse_weight(value)
                .map(|parsed| patch.font_weight = Some(parsed))
                .is_some(),
            "cursor-style" => parse_cursor_style(value)
                .map(|parsed| patch.cursor_style = Some(parsed))
                .is_some(),
            "cursor-style-blink" => parse_bool(value)
                .map(|parsed| patch.cursor_blink = Some(parsed))
                .is_some(),
            "focus-follows-mouse" => parse_bool(value)
                .map(|parsed| patch.focus_follows_mouse = Some(parsed))
                .is_some(),
            "middle-click-action" => match value {
                "primary-paste" => {
                    patch.primary_selection_middle_click_paste = Some(true);
                    true
                }
                "ignore" => {
                    patch.primary_selection_middle_click_paste = Some(false);
                    true
                }
                _ => false,
            },
            _ => false,
        };
        if !accepted {
            unsupported.insert(key);
        }
    }
    patch
}

fn last_value(values: &[String]) -> &str {
    values.last().map_or("", String::as_str)
}

fn insert_color(colors: &mut BTreeMap<String, String>, key: &str, raw: &str) -> bool {
    let Some(color) = normalized_optional_hex_color(raw) else {
        return false;
    };
    colors.insert(key.to_string(), color);
    true
}

fn parse_palette(values: &[String], colors: &mut BTreeMap<String, String>) -> bool {
    let mut accepted = false;
    for value in values {
        let Some((raw_index, raw_color)) = value.split_once('=') else {
            continue;
        };
        let Ok(index) = raw_index.trim().parse::<usize>() else {
            continue;
        };
        let Some(key) = PALETTE_KEYS.get(index) else {
            continue;
        };
        accepted |= insert_color(colors, key, raw_color.trim());
    }
    accepted
}

fn parse_option_as_alt(value: &str) -> Option<MacOptionAsAlt> {
    match value {
        "true" | "on" => Some(MacOptionAsAlt::Both),
        "false" | "off" => Some(MacOptionAsAlt::Off),
        "left" => Some(MacOptionAsAlt::Left),
        "right" => Some(MacOptionAsAlt::Right),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_unit_f64(value: &str) -> Option<f64> {
    let parsed = value.parse::<f64>().ok()?;
    (parsed.is_finite() && (0.0..=1.0).contains(&parsed)).then_some(parsed)
}

fn parse_unit_f32(value: &str) -> Option<f32> {
    let parsed = value.parse::<f32>().ok()?;
    (parsed.is_finite() && (0.0..=1.0).contains(&parsed)).then_some(parsed)
}

fn parse_nonnegative_u32(value: &str) -> Option<u32> {
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.parse().ok())
        .flatten()
}

fn parse_positive_u32(value: &str) -> Option<u32> {
    parse_nonnegative_u32(value).filter(|value| *value > 0)
}

fn parse_weight(value: &str) -> Option<u32> {
    parse_nonnegative_u32(value).filter(|value| (100..=900).contains(value))
}

fn parse_padding(value: &str) -> Option<u32> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.is_empty() || parts.len() > 2 {
        return None;
    }
    let values = parts
        .into_iter()
        .map(parse_nonnegative_u32)
        .collect::<Option<Vec<_>>>()?;
    if values.iter().any(|value| *value > 512) {
        return None;
    }
    let sum = values.iter().copied().sum::<u32>();
    (sum % values.len() as u32 == 0).then_some(sum / values.len() as u32)
}

fn parse_cell_height(value: &str) -> Option<f32> {
    let percent = value.strip_prefix('+').unwrap_or(value).strip_suffix('%')?;
    let valid = percent.bytes().all(|byte| byte.is_ascii_digit())
        || percent.split_once('.').is_some_and(|(whole, fraction)| {
            !whole.is_empty()
                && !fraction.is_empty()
                && whole.bytes().all(|byte| byte.is_ascii_digit())
                && fraction.bytes().all(|byte| byte.is_ascii_digit())
        });
    if !valid {
        return None;
    }
    let percent = percent.parse::<f32>().ok()?;
    let line_height = ((100.0 + percent).round()) / 100.0;
    (percent.is_finite() && (1.0..=3.0).contains(&line_height)).then_some(line_height)
}

fn parse_cursor_style(value: &str) -> Option<TerminalCursorStyle> {
    match value {
        "bar" => Some(TerminalCursorStyle::Bar),
        "block" => Some(TerminalCursorStyle::Block),
        "underline" => Some(TerminalCursorStyle::Underline),
        _ => None,
    }
}

#[derive(Default)]
struct UnsupportedKeys {
    keys: BTreeSet<String>,
    overflowed: bool,
}

impl UnsupportedKeys {
    fn insert(&mut self, key: impl Into<String>) {
        let key = key.into();
        if self.keys.contains(&key) {
            return;
        }
        if self.keys.len() < MAX_UNSUPPORTED_KEYS {
            self.keys.insert(key);
        } else {
            self.overflowed = true;
        }
    }

    fn finish(self) -> Vec<String> {
        let mut keys = self.keys.into_iter().collect::<Vec<_>>();
        if self.overflowed {
            keys.push(UNSUPPORTED_OVERFLOW.to_string());
        }
        keys
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("test parent");
        }
        fs::write(path, content).expect("test file");
    }

    #[test]
    fn parser_keeps_quotes_comments_last_scalars_and_every_palette_entry() {
        let parsed = parse_config(
            "# comment\nfont-family = 'JetBrains # Mono' # kept\nfont-size=12\nfont-size=15\npalette=0=111111\npalette=8=#aaa\n",
        );
        assert_eq!(last_value(&parsed["font-family"]), "JetBrains # Mono");
        assert_eq!(last_value(&parsed["font-size"]), "15");
        assert_eq!(parsed["palette"], ["0=111111", "8=#aaa"]);
    }

    #[test]
    fn compatible_settings_map_without_stringly_typed_application() {
        let parsed = parse_config(
            "macos-option-as-alt=left\nbackground-opacity=.75\nbackground=abc\ncursor-text=010203\npalette=1=ff0000\nbackground-blur-radius=12\nsplit-divider-color=123456\nunfocused-split-opacity=.4\nwindow-padding-x=2,6\nwindow-padding-y=7\nadjust-cell-height=+10%\nmouse-hide-while-typing=true\ncursor-opacity=.5\nfont-family=Commit Mono\nfont-size=16\nfont-weight=600\ncursor-style=bar\ncursor-style-blink=false\nfocus-follows-mouse=true\nmiddle-click-action=ignore\n",
        );
        let mut unsupported = UnsupportedKeys::default();
        let patch = map_config(&parsed, true, &mut unsupported);
        assert_eq!(patch.mac_option_as_alt, Some(MacOptionAsAlt::Left));
        assert_eq!(patch.terminal_opacity, Some(0.75));
        assert_eq!(patch.window_blur, Some(true));
        assert_eq!(patch.color_overrides["background"], "#aabbcc");
        assert_eq!(patch.color_overrides["cursorAccent"], "#010203");
        assert_eq!(patch.color_overrides["red"], "#ff0000");
        assert_eq!((patch.padding_x, patch.padding_y), (Some(4), Some(7)));
        assert_eq!(patch.line_height, Some(1.1));
        assert_eq!(patch.primary_selection_middle_click_paste, Some(false));
        assert_eq!(
            unsupported.finish(),
            ["background-blur-radius (radius value not preserved)"]
        );
    }

    #[test]
    fn invalid_unknown_and_unrepresentable_values_are_reported_not_rounded() {
        let parsed = parse_config(
            "font-size=13.5\nfont-weight=450.5\nwindow-padding-x=1,2\nadjust-cell-height=.5%\nselection-word-chars=abc\nbold-color=ffffff\nunknown=value\ncursor-opacity=2\n",
        );
        let mut unsupported = UnsupportedKeys::default();
        let patch = map_config(&parsed, true, &mut unsupported);
        assert_eq!(patch, GhosttyImportPatch::default());
        assert_eq!(
            unsupported.finish(),
            [
                "adjust-cell-height",
                "bold-color",
                "cursor-opacity",
                "font-size",
                "font-weight",
                "selection-word-chars",
                "unknown",
                "window-padding-x",
            ]
        );
    }

    #[test]
    fn theme_colors_load_before_explicit_config_colors_and_palettes() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let themes = temp.path().join("themes");
        write(
            &themes.join("Night"),
            "background=111111\nforeground=eeeeee\npalette=1=aa0000\npalette=2=00aa00\n",
        );
        let mut parsed = parse_config("theme=Night\nbackground=222222\npalette=1=ff0000\n");
        let mut unsupported = UnsupportedKeys::default();
        apply_theme_reference(&mut parsed, &[themes], &mut unsupported);
        let patch = map_config(&parsed, true, &mut unsupported);
        assert_eq!(patch.color_overrides["background"], "#222222");
        assert_eq!(patch.color_overrides["foreground"], "#eeeeee");
        assert_eq!(patch.color_overrides["red"], "#ff0000");
        assert_eq!(patch.color_overrides["green"], "#00aa00");
        assert!(unsupported.finish().is_empty());
    }

    #[test]
    fn conditional_or_missing_themes_are_explicitly_unsupported() {
        for (source, expected) in [
            (
                "theme=light:Day,dark:Night\n",
                "theme (light:/dark: pairs not supported)",
            ),
            ("theme=Missing\n", "theme (theme file not found)"),
        ] {
            let mut parsed = parse_config(source);
            let mut unsupported = UnsupportedKeys::default();
            apply_theme_reference(&mut parsed, &[], &mut unsupported);
            assert_eq!(unsupported.finish(), [expected]);
        }
    }

    #[test]
    fn preview_is_bounded_and_refuses_a_symlink_config_leaf() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let oversized = temp.path().join("oversized");
        fs::write(&oversized, vec![b'x'; MAX_CONFIG_BYTES as usize + 1]).expect("oversized");
        let preview = preview_paths(&[oversized], &[], true);
        assert!(
            preview
                .error
                .as_deref()
                .is_some_and(|error| error.contains("limit"))
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = temp.path().join("outside");
            let link = temp.path().join("config");
            write(&outside, "font-size=18\n");
            symlink(&outside, &link).expect("symlink");
            let preview = preview_paths(&[link], &[], true);
            assert!(!preview.found);
            assert!(
                preview
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("plain file"))
            );
        }
    }

    #[test]
    fn preview_only_returns_changes_after_canonical_clamping() {
        let mut document = SettingsDocument::default();
        document.terminal_prefs.font_size = 24;
        let preview = GhosttyImportPreview {
            found: true,
            patch: GhosttyImportPatch {
                font_size: Some(999),
                cursor_blink: Some(false),
                ..GhosttyImportPatch::default()
            },
            ..GhosttyImportPreview::default()
        }
        .against(&document);
        assert_eq!(preview.patch.font_size, None);
        assert_eq!(preview.patch.cursor_blink, Some(false));
        assert_eq!(preview.changes.len(), 1);
        assert_eq!(preview.changes[0].key, "terminalCursorBlink");
    }
}
