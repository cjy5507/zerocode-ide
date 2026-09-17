//! Orca-compatible Warp terminal-theme import.
//!
//! File discovery and YAML parsing stay native. The renderer receives only a
//! bounded, typed preview and the settings repository validates those same
//! values again before they become a live terminal palette.

use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use sha2::{Digest, Sha256};

use crate::{TerminalColorKey, durable_file, normalized_optional_hex_color};

pub(crate) const CUSTOM_THEME_SELECTION_PREFIX: &str = "custom:";
pub(crate) const MAX_CUSTOM_TERMINAL_THEMES: usize = 200;
const MAX_THEME_FILE_BYTES: u64 = 1_000_000;
const MAX_DIRECTORY_DEPTH: usize = 3;
const MAX_DIRECTORIES: usize = 80;
const MAX_DIRECTORY_ENTRIES: usize = 500;
const MAX_PARSE_TIME: Duration = Duration::from_secs(1);
const MAX_YAML_ALIASES: usize = 20;

const ANSI_COLORS: [(&str, TerminalColorKey); 8] = [
    ("black", TerminalColorKey::Black),
    ("red", TerminalColorKey::Red),
    ("green", TerminalColorKey::Green),
    ("yellow", TerminalColorKey::Yellow),
    ("blue", TerminalColorKey::Blue),
    ("magenta", TerminalColorKey::Magenta),
    ("cyan", TerminalColorKey::Cyan),
    ("white", TerminalColorKey::White),
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WarpThemeImportSource {
    Auto,
    Files,
    Folder,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CustomTerminalThemeSource {
    Warp,
    Ghostty,
    Manual,
}

impl CustomTerminalThemeSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Warp => "warp",
            Self::Ghostty => "ghostty",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CustomTerminalThemeMode {
    Dark,
    Light,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CustomTerminalTheme {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) source: CustomTerminalThemeSource,
    pub(crate) mode: CustomTerminalThemeMode,
    pub(crate) terminal: BTreeMap<TerminalColorKey, String>,
    pub(crate) imported_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) unsupported_features: Vec<String>,
}

#[cfg(test)]
impl CustomTerminalTheme {
    pub(crate) fn selection_value(&self) -> String {
        format!("{CUSTOM_THEME_SELECTION_PREFIX}{}", self.id)
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WarpThemeSkipReason {
    InvalidYaml,
    NotAnObject,
    MissingTerminalColors,
    TooLarge,
    UnsupportedFileType,
    UnsupportedEntryType,
    ReadFailed,
    TooManyAliases,
    ParseTimedOut,
    LimitReached,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WarpThemeSkip {
    label: String,
    reason: WarpThemeSkipReason,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WarpThemePreview {
    themes: Vec<CustomTerminalTheme>,
    skipped: Vec<WarpThemeSkip>,
}

#[derive(Debug)]
struct ThemeCandidate {
    path: PathBuf,
    label: String,
}

pub(crate) fn preview_files(paths: Vec<PathBuf>) -> WarpThemePreview {
    let mut skipped = Vec::new();
    let candidates = paths
        .into_iter()
        .filter_map(|path| candidate_for_file(path, &mut skipped))
        .collect();
    preview_candidates(candidates, skipped)
}

pub(crate) fn preview_folder(root: &Path) -> WarpThemePreview {
    let mut skipped = Vec::new();
    let candidates = scan_theme_directory(root, &mut skipped);
    preview_candidates(candidates, skipped)
}

pub(crate) fn preview_auto() -> WarpThemePreview {
    let mut skipped = Vec::new();
    let mut candidates = Vec::new();
    for root in auto_theme_roots() {
        candidates.extend(scan_theme_directory(&root, &mut skipped));
        if candidates.len() > MAX_CUSTOM_TERMINAL_THEMES {
            break;
        }
    }
    preview_candidates(candidates, skipped)
}

pub(crate) fn normalized_custom_themes(
    themes: Vec<CustomTerminalTheme>,
) -> Vec<CustomTerminalTheme> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for theme in themes.into_iter().rev() {
        let Some(theme) = normalized_custom_theme(theme) else {
            continue;
        };
        if seen.insert(theme.id.clone()) {
            normalized.push(theme);
            if normalized.len() == MAX_CUSTOM_TERMINAL_THEMES {
                break;
            }
        }
    }
    normalized.reverse();
    normalized
}

pub(crate) fn merge_custom_themes(
    mut existing: Vec<CustomTerminalTheme>,
    incoming: Vec<CustomTerminalTheme>,
) -> Vec<CustomTerminalTheme> {
    existing.extend(incoming);
    normalized_custom_themes(existing)
}

pub(crate) fn remove_custom_theme(
    themes: Vec<CustomTerminalTheme>,
    id: &str,
) -> Vec<CustomTerminalTheme> {
    themes
        .into_iter()
        .filter(|theme| theme.id != id.trim())
        .collect()
}

pub(crate) fn has_custom_theme_selection(selection: &str, themes: &[CustomTerminalTheme]) -> bool {
    selection
        .strip_prefix(CUSTOM_THEME_SELECTION_PREFIX)
        .is_some_and(|id| themes.iter().any(|theme| theme.id == id))
}

fn preview_candidates(
    mut candidates: Vec<ThemeCandidate>,
    mut skipped: Vec<WarpThemeSkip>,
) -> WarpThemePreview {
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path == right.path);
    if candidates.len() > MAX_CUSTOM_TERMINAL_THEMES {
        candidates.truncate(MAX_CUSTOM_TERMINAL_THEMES);
        skipped.push(WarpThemeSkip {
            label: "Warp themes".to_string(),
            reason: WarpThemeSkipReason::LimitReached,
        });
    }

    let mut themes = Vec::new();
    for candidate in candidates {
        match read_and_parse(&candidate) {
            Ok(theme) => themes.push(theme),
            Err(reason) => skipped.push(WarpThemeSkip {
                label: candidate.label,
                reason,
            }),
        }
    }
    themes = normalized_custom_themes(themes);
    WarpThemePreview { themes, skipped }
}

fn candidate_for_file(path: PathBuf, skipped: &mut Vec<WarpThemeSkip>) -> Option<ThemeCandidate> {
    let label = source_label(&path);
    if !is_yaml_path(&path) {
        skipped.push(WarpThemeSkip {
            label,
            reason: WarpThemeSkipReason::UnsupportedFileType,
        });
        return None;
    }
    Some(ThemeCandidate { path, label })
}

fn scan_theme_directory(root: &Path, skipped: &mut Vec<WarpThemeSkip>) -> Vec<ThemeCandidate> {
    let mut candidates = Vec::new();
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return candidates;
    };
    if !durable_file::is_plain_directory(&metadata) {
        skipped.push(WarpThemeSkip {
            label: source_label(root),
            reason: WarpThemeSkipReason::UnsupportedEntryType,
        });
        return candidates;
    }

    let mut pending = VecDeque::from([(root.to_path_buf(), 0_usize)]);
    let mut directories = 0_usize;
    while let Some((directory, depth)) = pending.pop_front() {
        if directories == MAX_DIRECTORIES {
            skipped.push(WarpThemeSkip {
                label: source_label(root),
                reason: WarpThemeSkipReason::LimitReached,
            });
            break;
        }
        directories += 1;
        let Ok(read) = fs::read_dir(&directory) else {
            skipped.push(WarpThemeSkip {
                label: source_label(&directory),
                reason: WarpThemeSkipReason::ReadFailed,
            });
            continue;
        };
        let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
        entries.sort_by_key(fs::DirEntry::file_name);
        if entries.len() > MAX_DIRECTORY_ENTRIES {
            entries.truncate(MAX_DIRECTORY_ENTRIES);
            skipped.push(WarpThemeSkip {
                label: source_label(&directory),
                reason: WarpThemeSkipReason::LimitReached,
            });
        }
        for entry in entries {
            let path = entry.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                skipped.push(WarpThemeSkip {
                    label: source_label(&path),
                    reason: WarpThemeSkipReason::ReadFailed,
                });
                continue;
            };
            if durable_file::is_plain_file(&metadata) && is_yaml_path(&path) {
                candidates.push(ThemeCandidate {
                    label: source_label(&path),
                    path,
                });
            } else if depth < MAX_DIRECTORY_DEPTH && durable_file::is_plain_directory(&metadata) {
                pending.push_back((path, depth + 1));
            } else if metadata.file_type().is_symlink() {
                skipped.push(WarpThemeSkip {
                    label: source_label(&path),
                    reason: WarpThemeSkipReason::UnsupportedEntryType,
                });
            }
            if candidates.len() > MAX_CUSTOM_TERMINAL_THEMES {
                return candidates;
            }
        }
    }
    candidates
}

fn read_and_parse(candidate: &ThemeCandidate) -> Result<CustomTerminalTheme, WarpThemeSkipReason> {
    let mut file = durable_file::open_plain_file(&candidate.path)
        .map_err(|_| WarpThemeSkipReason::ReadFailed)?;
    let metadata = file
        .metadata()
        .map_err(|_| WarpThemeSkipReason::ReadFailed)?;
    if metadata.len() > MAX_THEME_FILE_BYTES {
        return Err(WarpThemeSkipReason::TooLarge);
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(MAX_THEME_FILE_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|_| WarpThemeSkipReason::ReadFailed)?;
    if content.len() as u64 > MAX_THEME_FILE_BYTES {
        return Err(WarpThemeSkipReason::TooLarge);
    }
    let started = Instant::now();
    let parsed = parse_warp_theme_yaml(&content, &candidate.path, &candidate.label);
    if started.elapsed() > MAX_PARSE_TIME {
        Err(WarpThemeSkipReason::ParseTimedOut)
    } else {
        parsed
    }
}

fn parse_warp_theme_yaml(
    content: &[u8],
    source_path: &Path,
    source_label: &str,
) -> Result<CustomTerminalTheme, WarpThemeSkipReason> {
    if yaml_alias_count(content) > MAX_YAML_ALIASES {
        return Err(WarpThemeSkipReason::TooManyAliases);
    }
    let value: Value =
        serde_yaml_ng::from_slice(content).map_err(|_| WarpThemeSkipReason::InvalidYaml)?;
    let input = value.as_mapping().ok_or(WarpThemeSkipReason::NotAnObject)?;
    let fallback_name = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Imported Theme");
    let name = normalize_theme_name(mapping_string(input, "name"), fallback_name);
    let mut terminal = BTreeMap::new();

    let background = mapping_color(input, "background");
    let foreground = mapping_color(input, "foreground");
    let cursor = mapping_color(input, "cursor").or_else(|| mapping_color(input, "accent"));
    if let Some(color) = background.as_ref() {
        terminal.insert(TerminalColorKey::Background, color.clone());
    }
    if let Some(color) = foreground.as_ref() {
        terminal.insert(TerminalColorKey::Foreground, color.clone());
    }
    if let Some(color) = cursor {
        terminal.insert(TerminalColorKey::Cursor, color);
    }

    if let Some(colors) = mapping_value(input, "terminal_colors").and_then(Value::as_mapping) {
        add_palette(
            &mut terminal,
            mapping_value(colors, "normal").and_then(Value::as_mapping),
            false,
        );
        add_palette(
            &mut terminal,
            mapping_value(colors, "bright").and_then(Value::as_mapping),
            true,
        );
    }
    if !has_usable_colors(&terminal) {
        return Err(WarpThemeSkipReason::MissingTerminalColors);
    }

    let discriminator = source_discriminator(source_path);
    let id = normalize_theme_id(&format!("warp:{name}:{discriminator}"), "warp:theme");
    let details = mapping_string(input, "details");
    Ok(CustomTerminalTheme {
        id,
        name,
        source: CustomTerminalThemeSource::Warp,
        mode: infer_mode(background.as_deref(), details),
        terminal,
        imported_at_ms: imported_at_ms(),
        source_label: Some(normalize_source_label(source_label)),
        unsupported_features: detect_unsupported_features(input),
    })
}

/// Count YAML alias indicators without mistaking quoted text or comments for
/// aliases. `serde_yaml_ng` bounds recursive expansion internally; matching
/// Orca's lower public limit keeps imported theme work predictable as well.
fn yaml_alias_count(content: &[u8]) -> usize {
    let Ok(content) = std::str::from_utf8(content) else {
        return 0;
    };
    content
        .lines()
        .map(|line| {
            let bytes = line.as_bytes();
            let mut aliases = 0;
            let mut single_quoted = false;
            let mut double_quoted = false;
            let mut escaped = false;
            for (index, byte) in bytes.iter().copied().enumerate() {
                if double_quoted && escaped {
                    escaped = false;
                    continue;
                }
                if double_quoted && byte == b'\\' {
                    escaped = true;
                    continue;
                }
                if !double_quoted && byte == b'\'' {
                    single_quoted = !single_quoted;
                    continue;
                }
                if !single_quoted && byte == b'"' {
                    double_quoted = !double_quoted;
                    continue;
                }
                if single_quoted || double_quoted {
                    continue;
                }
                if byte == b'#' {
                    break;
                }
                if byte != b'*' {
                    continue;
                }
                let separated = index == 0
                    || bytes[index - 1].is_ascii_whitespace()
                    || matches!(bytes[index - 1], b'[' | b'{' | b',' | b':' | b'-' | b'?');
                let named = bytes.get(index + 1).is_some_and(|next| {
                    !next.is_ascii_whitespace() && !matches!(next, b']' | b'}' | b',')
                });
                aliases += usize::from(separated && named);
            }
            aliases
        })
        .sum()
}

fn add_palette(
    terminal: &mut BTreeMap<TerminalColorKey, String>,
    palette: Option<&Mapping>,
    bright: bool,
) {
    let Some(palette) = palette else {
        return;
    };
    for (name, normal_key) in ANSI_COLORS {
        let Some(color) = mapping_color(palette, name) else {
            continue;
        };
        let key = if bright {
            bright_key(normal_key)
        } else {
            normal_key
        };
        terminal.insert(key, color);
    }
}

const fn bright_key(key: TerminalColorKey) -> TerminalColorKey {
    match key {
        TerminalColorKey::Black => TerminalColorKey::BrightBlack,
        TerminalColorKey::Red => TerminalColorKey::BrightRed,
        TerminalColorKey::Green => TerminalColorKey::BrightGreen,
        TerminalColorKey::Yellow => TerminalColorKey::BrightYellow,
        TerminalColorKey::Blue => TerminalColorKey::BrightBlue,
        TerminalColorKey::Magenta => TerminalColorKey::BrightMagenta,
        TerminalColorKey::Cyan => TerminalColorKey::BrightCyan,
        TerminalColorKey::White => TerminalColorKey::BrightWhite,
        _ => key,
    }
}

fn mapping_value<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping.get(Value::String(key.to_string()))
}

fn mapping_string<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a str> {
    mapping_value(mapping, key).and_then(Value::as_str)
}

fn mapping_color(mapping: &Mapping, key: &str) -> Option<String> {
    color_value(mapping_value(mapping, key)?)
}

fn color_value(value: &Value) -> Option<String> {
    value
        .as_str()
        .and_then(normalized_optional_hex_color)
        .or_else(|| {
            let gradient = value.as_mapping()?;
            ["top", "bottom", "left", "right"]
                .into_iter()
                .find_map(|key| mapping_color(gradient, key))
        })
}

fn has_usable_colors(terminal: &BTreeMap<TerminalColorKey, String>) -> bool {
    terminal.contains_key(&TerminalColorKey::Background)
        && terminal.contains_key(&TerminalColorKey::Foreground)
        && terminal
            .keys()
            .any(|key| ANSI_COLORS.iter().any(|(_, ansi)| ansi == key) || is_bright_ansi(*key))
}

const fn is_bright_ansi(key: TerminalColorKey) -> bool {
    matches!(
        key,
        TerminalColorKey::BrightBlack
            | TerminalColorKey::BrightRed
            | TerminalColorKey::BrightGreen
            | TerminalColorKey::BrightYellow
            | TerminalColorKey::BrightBlue
            | TerminalColorKey::BrightMagenta
            | TerminalColorKey::BrightCyan
            | TerminalColorKey::BrightWhite
    )
}

fn normalized_custom_theme(mut theme: CustomTerminalTheme) -> Option<CustomTerminalTheme> {
    let name = normalize_theme_name(Some(&theme.name), "Imported Theme");
    let id_base = normalize_theme_id(&theme.id, &format!("{}:{name}", theme.source.as_str()));
    let id = if id_base.contains(':') {
        id_base
    } else {
        format!("{}:{id_base}", theme.source.as_str())
    };
    theme.terminal = theme
        .terminal
        .into_iter()
        .filter_map(|(key, color)| normalized_optional_hex_color(&color).map(|color| (key, color)))
        .collect();
    if !has_usable_colors(&theme.terminal) {
        return None;
    }
    theme.id = id;
    theme.name = name;
    theme.source_label = theme
        .source_label
        .as_deref()
        .map(normalize_source_label)
        .filter(|label| !label.is_empty());
    theme.unsupported_features = theme
        .unsupported_features
        .into_iter()
        .map(|feature| normalize_source_label(&feature))
        .filter(|feature| !feature.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Some(theme)
}

fn normalize_theme_name(value: Option<&str>, fallback: &str) -> String {
    let normalized = value
        .unwrap_or(fallback)
        .chars()
        .filter(|character| !character.is_control())
        .map(|character| {
            if matches!(character, '/' | '\\') {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        "Imported Theme".to_string()
    } else {
        normalized
    }
}

fn normalize_theme_id(value: &str, fallback: &str) -> String {
    let mut normalized = String::new();
    let mut pending_dash = false;
    for character in value
        .trim()
        .chars()
        .filter(|character| !character.is_control())
    {
        let character = character.to_ascii_lowercase();
        if character.is_ascii_alphanumeric() || matches!(character, ':' | '_' | '-') {
            if pending_dash && !normalized.is_empty() && !normalized.ends_with('-') {
                normalized.push('-');
            }
            pending_dash = false;
            if character != '\'' && character != '"' {
                normalized.push(character);
            }
        } else if !matches!(character, '\'' | '"') {
            pending_dash = true;
        }
    }
    let normalized = normalized.trim_matches('-');
    if normalized.is_empty() {
        fallback.to_string()
    } else {
        normalized.to_string()
    }
}

fn normalize_source_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_string()
}

fn infer_mode(background: Option<&str>, details: Option<&str>) -> CustomTerminalThemeMode {
    if let Some(background) = background {
        let hex = background.trim_start_matches('#');
        if hex.len() == 6 {
            let red = u8::from_str_radix(&hex[0..2], 16).unwrap_or_default() as f32 / 255.0;
            let green = u8::from_str_radix(&hex[2..4], 16).unwrap_or_default() as f32 / 255.0;
            let blue = u8::from_str_radix(&hex[4..6], 16).unwrap_or_default() as f32 / 255.0;
            return if 0.2126 * red + 0.7152 * green + 0.0722 * blue >= 0.55 {
                CustomTerminalThemeMode::Light
            } else {
                CustomTerminalThemeMode::Dark
            };
        }
    }
    match details {
        Some("lighter") => CustomTerminalThemeMode::Light,
        Some("darker") => CustomTerminalThemeMode::Dark,
        _ => CustomTerminalThemeMode::Unknown,
    }
}

fn detect_unsupported_features(input: &Mapping) -> Vec<String> {
    let mut unsupported = BTreeSet::new();
    if mapping_value(input, "background_image").is_some() {
        unsupported.insert("background image not supported".to_string());
    }
    if mapping_value(input, "background").is_some_and(Value::is_mapping) {
        unsupported.insert("background gradient not supported".to_string());
    }
    if mapping_value(input, "accent").is_some_and(Value::is_mapping) {
        unsupported.insert("accent gradient not supported".to_string());
    }
    if ["background_gradient", "gradient", "gradients"]
        .into_iter()
        .any(|key| mapping_value(input, key).is_some())
    {
        unsupported.insert("gradient not supported".to_string());
    }
    unsupported.into_iter().collect()
}

fn source_discriminator(path: &Path) -> String {
    let mut digest = Sha256::new();
    digest.update(path.to_string_lossy().as_bytes());
    let digest = format!("{:x}", digest.finalize());
    digest[..12].to_string()
}

fn source_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "theme.yaml".to_string(), normalize_source_label)
}

fn is_yaml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "yaml" | "yml"))
}

fn imported_at_ms() -> u64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn auto_theme_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let Some(home) = dirs::home_dir() else {
            return Vec::new();
        };
        matching_child_directories(&home, |name| name.starts_with(".warp"))
            .into_iter()
            .map(|root| root.join("themes"))
            .filter(|root| root.is_dir())
            .collect()
    }
    #[cfg(target_os = "windows")]
    {
        let Some(data) = dirs::data_dir() else {
            return Vec::new();
        };
        matching_child_directories(&data.join("warp"), |_| true)
            .into_iter()
            .map(|channel| channel.join("data/themes"))
            .filter(|root| root.is_dir())
            .collect()
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let Some(data) = dirs::data_dir() else {
            return Vec::new();
        };
        matching_child_directories(&data, |name| {
            name.starts_with("warp-terminal") || name.starts_with("warp-")
        })
        .into_iter()
        .map(|root| root.join("themes"))
        .filter(|root| root.is_dir())
        .collect()
    }
}

fn matching_child_directories(parent: &Path, accepts: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let Ok(read) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut matches: Vec<_> = read
        .filter_map(Result::ok)
        .take(MAX_DIRECTORY_ENTRIES)
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if !accepts(name) {
                return None;
            }
            let metadata = fs::symlink_metadata(entry.path()).ok()?;
            durable_file::is_plain_directory(&metadata).then(|| entry.path())
        })
        .collect();
    matches.sort();
    matches
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const WARP_THEME: &str = r##"
name: Ocean Night
accent:
  top: "#abc"
background: "#102030"
foreground: "f0f1f2"
details: darker
terminal_colors:
  normal:
    black: "#010203"
    red: "#aabbcc"
  bright:
    blue: "#336699"
background_image: /tmp/wallpaper.png
"##;

    #[test]
    fn warp_yaml_becomes_one_stable_typed_preview() {
        let theme = parse_warp_theme_yaml(
            WARP_THEME.as_bytes(),
            Path::new("/themes/ocean.yaml"),
            "ocean.yaml",
        )
        .expect("valid Warp theme");

        assert_eq!(theme.name, "Ocean Night");
        assert_eq!(theme.mode, CustomTerminalThemeMode::Dark);
        assert_eq!(theme.selection_value(), format!("custom:{}", theme.id));
        assert_eq!(
            theme.terminal.get(&TerminalColorKey::Background),
            Some(&"#102030".to_string())
        );
        assert_eq!(
            theme.terminal.get(&TerminalColorKey::Cursor),
            Some(&"#aabbcc".to_string())
        );
        assert_eq!(
            theme.terminal.get(&TerminalColorKey::BrightBlue),
            Some(&"#336699".to_string())
        );
        assert_eq!(
            theme.unsupported_features,
            vec![
                "accent gradient not supported",
                "background image not supported",
            ]
        );
        let again = parse_warp_theme_yaml(
            WARP_THEME.as_bytes(),
            Path::new("/themes/ocean.yaml"),
            "ocean.yaml",
        )
        .expect("same theme");
        assert_eq!(theme.id, again.id, "the source path owns stable identity");
    }

    #[test]
    fn unusable_or_duplicate_yaml_never_enters_settings() {
        let missing_ansi = b"name: Empty\nbackground: '#000'\nforeground: '#fff'\n";
        assert_eq!(
            parse_warp_theme_yaml(missing_ansi, Path::new("empty.yaml"), "empty.yaml"),
            Err(WarpThemeSkipReason::MissingTerminalColors)
        );
        let duplicate = b"background: '#000'\nbackground: '#111'\nforeground: '#fff'\nterminal_colors:\n  normal:\n    red: '#f00'\n";
        assert_eq!(
            parse_warp_theme_yaml(duplicate, Path::new("dupe.yaml"), "dupe.yaml"),
            Err(WarpThemeSkipReason::InvalidYaml)
        );

        let aliases = format!(
            "anchor: &palette '#abc'\n{WARP_THEME}\naliases:\n{}",
            "  - *palette\n".repeat(MAX_YAML_ALIASES + 1)
        );
        assert_eq!(
            parse_warp_theme_yaml(
                aliases.as_bytes(),
                Path::new("aliases.yaml"),
                "aliases.yaml"
            ),
            Err(WarpThemeSkipReason::TooManyAliases)
        );

        let quoted = WARP_THEME.replace("Ocean Night", "Stars * are not aliases");
        assert!(
            parse_warp_theme_yaml(quoted.as_bytes(), Path::new("stars.yaml"), "stars.yaml").is_ok()
        );
    }

    #[test]
    fn directory_scan_is_bounded_and_never_follows_a_symlink() {
        let root = tempfile::tempdir().expect("theme root");
        let outside = tempfile::tempdir().expect("outside");
        fs::write(root.path().join("valid.yaml"), WARP_THEME).expect("theme");
        fs::write(root.path().join("notes.txt"), WARP_THEME).expect("not a theme");
        fs::write(outside.path().join("outside.yaml"), WARP_THEME).expect("outside theme");
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked"))
            .expect("directory symlink");

        let preview = preview_folder(root.path());
        assert_eq!(preview.themes.len(), 1);
        assert_eq!(
            preview.themes[0].source_label.as_deref(),
            Some("valid.yaml")
        );
        #[cfg(unix)]
        assert!(preview.skipped.iter().any(|skip| {
            skip.label == "linked" && skip.reason == WarpThemeSkipReason::UnsupportedEntryType
        }));
    }

    #[test]
    fn stored_custom_themes_are_validated_deduplicated_and_capped() {
        let theme = parse_warp_theme_yaml(
            WARP_THEME.as_bytes(),
            Path::new("/themes/ocean.yaml"),
            "ocean.yaml",
        )
        .expect("theme");
        let mut replacement = theme.clone();
        replacement.name = "Ocean Replacement".to_string();
        let normalized = normalized_custom_themes(vec![theme, replacement]);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].name, "Ocean Replacement");
        assert!(has_custom_theme_selection(
            &normalized[0].selection_value(),
            &normalized
        ));
        assert!(!has_custom_theme_selection("custom:missing", &normalized));
    }

    #[test]
    fn imported_selection_round_trips_and_removal_restores_the_builtin() {
        let theme = parse_warp_theme_yaml(
            WARP_THEME.as_bytes(),
            Path::new("/themes/ocean.yaml"),
            "ocean.yaml",
        )
        .expect("theme");
        let selection = theme.selection_value();
        let mut prefs = crate::TerminalPrefs::default();
        crate::TerminalPrefsPatch::ImportCustomThemes(vec![theme]).apply(&mut prefs);
        crate::TerminalPrefsPatch::ThemeDark(selection.clone()).apply(&mut prefs);
        assert_eq!(prefs.theme_dark, selection);

        let restored: crate::TerminalPrefs = serde_json::from_value(
            serde_json::to_value(&prefs).expect("serialize custom terminal prefs"),
        )
        .expect("restore custom terminal prefs");
        assert_eq!(restored.clamped(), prefs);

        let id = prefs.custom_themes[0].id.clone();
        crate::TerminalPrefsPatch::RemoveCustomTheme(id).apply(&mut prefs);
        assert!(prefs.custom_themes.is_empty());
        assert_eq!(prefs.theme_dark, crate::TERM_THEME_DARK);
    }
}
