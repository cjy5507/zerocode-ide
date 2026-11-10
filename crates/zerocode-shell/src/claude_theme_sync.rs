//! Claude Code's theme, kept in step with the terminal palette this window
//! paints — the window's half of [`zerocode_core::claude_theme`].
//!
//! Every Claude Code this window launches under an account reads one home
//! ([`crate::accounts::runtime_home`]), so there is one place to write: the
//! theme file `themes/zerocode.json`, and the `theme` key of that home's
//! `settings.json`. A person's own `~/.claude` is never touched — a launch
//! that names no home reads nothing of ours.
//!
//! Three rules, each Orca's (stablyai/orca#39447) and each a way of not
//! taking what was chosen:
//!
//! - **Only a theme nobody picked on purpose is replaced.** Claude Code's
//!   default pair and `auto` ([`theme::REPLACEABLE_THEMES`]); a colour-blind
//!   theme or a custom one of the person's own stays. Settings that name no
//!   theme are not "nobody picked": Claude Code then reads the one its older
//!   global config kept ([`kept_by_global_config`]), and that one is judged.
//! - **What was replaced is written down first**, in this window's own record
//!   (`claude-theme-sync.json` under the config root), so letting go puts it
//!   back — and a crash between the two writes still leaves a way back.
//! - **A person who then picks another theme has spoken.** The record marks
//!   it, and the window does not select its own again until it is turned off
//!   and on.
//!
//! The file goes before the selection, so `settings.json` never names a theme
//! Claude Code cannot load, and it is replaced whole ([`write_atomically`]):
//! Claude Code watches the folder, and a session already running recolours
//! from whatever it finds there.
//!
//! [`write_atomically`]: zerocode_core::atomic_file::write_atomically

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zerocode_core::claude_theme::{self as theme, Theme};

/// The window's record of the theme it replaced, under the config root.
const RECORD_FILE: &str = "claude-theme-sync.json";

/// What this window remembers of the theme it replaced in its Claude home.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    /// The `theme` the home's settings named before the window's; `None`
    /// when they named none.
    previous_theme: Option<String>,
    /// The person chose another theme after the window's was selected.
    #[serde(default)]
    user_changed: bool,
}

/// What one pass is asked for.
pub(crate) enum Target<'a> {
    /// Keep the home on `theme`. `loads_custom_themes` answers whether the
    /// installed Claude Code can load one, and is asked only when the
    /// selection is about to be made for the first time — it starts the CLI.
    Match {
        theme: &'a Theme,
        loads_custom_themes: &'a dyn Fn() -> bool,
    },
    /// Put back what was there. `terminal_base` is what `auto` resolves to
    /// for a session still running on the window's file.
    Release { terminal_base: &'a str },
}

/// What one pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Pass {
    /// The theme file was replaced.
    pub(crate) wrote_theme: bool,
    /// `settings.json` was pointed at the window's theme, or back.
    pub(crate) wrote_settings: bool,
}

fn record_path(config_root: &Path) -> PathBuf {
    config_root.join(RECORD_FILE)
}

fn theme_path(home: &Path) -> PathBuf {
    home.join("themes").join(format!("{}.json", theme::SLUG))
}

fn settings_path(home: &Path) -> PathBuf {
    home.join(crate::accounts::CLAUDE_USER_SETTINGS_FILE)
}

/// Whether this window holds a record of a theme it replaced — something to
/// put back, or to keep in step. A window that holds none and is not asked
/// to match has nothing to do, and need not start a pass to find that out.
pub(crate) fn holds_record(config_root: &Path) -> bool {
    record_path(config_root).is_file()
}

fn read_record(config_root: &Path) -> Option<Record> {
    serde_json::from_str(&std::fs::read_to_string(record_path(config_root)).ok()?).ok()
}

fn write_record(config_root: &Path, record: &Record) -> Result<(), String> {
    let text = serde_json::to_string_pretty(record).map_err(|error| error.to_string())?;
    crate::durable_file::replace_bytes(&record_path(config_root), text.as_bytes())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn forget_record(config_root: &Path) -> Result<(), String> {
    match std::fs::remove_file(record_path(config_root)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// The home's settings as a JSON object: empty when the file is not there
/// yet, `None` when it is there and is not an object this window can merge
/// into — merging into a file nobody understood is how one gets destroyed.
fn read_settings(path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(serde_json::Value::Object(held)) => Some(held),
            _ => None,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(serde_json::Map::new()),
        Err(_) => None,
    }
}

/// Write `settings` to `path` when the file still holds `seen` — what this
/// pass read and judged. Claude Code and the hook installer write the same
/// file; a pass that lost that race leaves the file to the next one.
fn write_settings_if_unmoved(
    path: &Path,
    seen: &serde_json::Map<String, serde_json::Value>,
    settings: &serde_json::Map<String, serde_json::Value>,
) -> Result<bool, String> {
    if read_settings(path).as_ref() != Some(seen) {
        return Ok(false);
    }
    let text = serde_json::to_string_pretty(settings).map_err(|error| error.to_string())?;
    crate::durable_file::replace_bytes(path, text.as_bytes())
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// Replace the theme file with `theme` unless it already says so.
fn write_theme(home: &Path, theme: &Theme) -> Result<bool, String> {
    let path = theme_path(home);
    let text = theme.to_file_text();
    if std::fs::read_to_string(&path).is_ok_and(|held| held == text) {
        return Ok(false);
    }
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| error.to_string())?;
    }
    zerocode_core::atomic_file::write_atomically(&path, text.as_bytes())
        .map(|()| true)
        .map_err(|error| error.to_string())
}

/// The theme Claude Code falls back to when no settings name one: what its
/// global config (`.claude.json`) kept from before `theme` moved into the
/// settings. Measured in the 2.1.321 binary — `resolveSetting("theme")` walks
/// the settings sources and then answers from `legacyGlobalConfig`. A pick
/// made on an older CLI is still a pick.
fn kept_by_global_config(home: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(crate::agent_trust_presets::claude_config_in(home)).ok()?;
    let held: serde_json::Value = serde_json::from_str(&text).ok()?;
    held.get("theme").cloned()
}

/// Whether a `theme` value is one the window's may stand in for: none at
/// all, or one of Claude Code's own defaults.
fn replaceable(named: Option<&serde_json::Value>) -> bool {
    match named {
        None => true,
        Some(serde_json::Value::String(name)) => theme::REPLACEABLE_THEMES.contains(&name.as_str()),
        Some(_) => false,
    }
}

/// One pass over this window's Claude home under `config_root`.
///
/// # Errors
///
/// The sentence for a write that failed; what was written before it stands
/// (the record before the selection, so a failed selection is undone here).
pub(crate) fn sync(config_root: &Path, target: Target<'_>) -> Result<Pass, String> {
    let home = crate::accounts::runtime_home(config_root);
    match target {
        Target::Match {
            theme,
            loads_custom_themes,
        } => keep(config_root, &home, theme, loads_custom_themes),
        Target::Release { terminal_base } => release(config_root, &home, terminal_base),
    }
}

fn keep(
    config_root: &Path,
    home: &Path,
    theme: &Theme,
    loads_custom_themes: &dyn Fn() -> bool,
) -> Result<Pass, String> {
    // No home yet: no Claude Code has been launched under this window, and
    // the pass after the first launch's palette report finds one.
    if !home.is_dir() {
        return Ok(Pass::default());
    }
    let mut pass = Pass::default();
    let record = read_record(config_root);
    // A person who chose another theme is not handed a file either: theirs
    // may be a custom one, and this folder is where Claude Code lists them.
    if record.as_ref().is_some_and(|held| held.user_changed) {
        return Ok(pass);
    }
    let settings_file = settings_path(home);
    let Some(seen) = read_settings(&settings_file) else {
        return Ok(pass);
    };
    let named = seen.get("theme");
    let selected = named.and_then(serde_json::Value::as_str) == Some(theme::SETTING);
    match record {
        Some(mut held) => {
            if selected {
                pass.wrote_theme = write_theme(home, theme)?;
            } else {
                held.user_changed = true;
                write_record(config_root, &held)?;
            }
        }
        None => {
            // A selection this window has no record of making is the
            // person's own: the file is theirs to have, the key is not ours.
            if selected {
                pass.wrote_theme = write_theme(home, theme)?;
                return Ok(pass);
            }
            // Settings that name none leave Claude Code on the theme its
            // global config kept; that is the one being replaced.
            let kept = match named {
                Some(_) => None,
                None => kept_by_global_config(home),
            };
            if !replaceable(named.or(kept.as_ref())) || !loads_custom_themes() {
                return Ok(pass);
            }
            pass.wrote_theme = write_theme(home, theme)?;
            write_record(
                config_root,
                &Record {
                    previous_theme: named
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string),
                    user_changed: false,
                },
            )?;
            let mut settings = seen.clone();
            settings.insert(
                "theme".to_string(),
                serde_json::Value::String(theme::SETTING.to_string()),
            );
            match write_settings_if_unmoved(&settings_file, &seen, &settings) {
                Ok(true) => pass.wrote_settings = true,
                // Without this the next pass would read the unchanged theme
                // as the person's own pick.
                Ok(false) => forget_record(config_root)?,
                Err(error) => {
                    forget_record(config_root)?;
                    return Err(error);
                }
            }
        }
    }
    Ok(pass)
}

fn release(config_root: &Path, home: &Path, terminal_base: &str) -> Result<Pass, String> {
    let Some(record) = read_record(config_root) else {
        return Ok(Pass::default());
    };
    let mut pass = Pass::default();
    if !record.user_changed && home.is_dir() {
        let settings_file = settings_path(home);
        // A file that cannot be read keeps the record: the restore is owed.
        let Some(seen) = read_settings(&settings_file) else {
            return Err(format!(
                "{} could not be read, so its theme is not put back yet",
                settings_file.display()
            ));
        };
        if seen.get("theme").and_then(serde_json::Value::as_str) == Some(theme::SETTING) {
            let mut settings = seen.clone();
            match &record.previous_theme {
                Some(previous) => {
                    settings.insert(
                        "theme".to_string(),
                        serde_json::Value::String(previous.clone()),
                    );
                }
                None => {
                    settings.remove("theme");
                }
            }
            if !write_settings_if_unmoved(&settings_file, &seen, &settings)? {
                return Err(format!(
                    "{} changed while its theme was being put back",
                    settings_file.display()
                ));
            }
            pass.wrote_settings = true;
        }
        // A session still running keeps the window's theme selected — it
        // read the setting once, when it started — and its file loaded; it
        // shows what was there before from the next time it reads the folder.
        // What was there, where the settings named nothing, is what the
        // global config kept.
        let shown = record.previous_theme.clone().or_else(|| {
            kept_by_global_config(home).and_then(|kept| kept.as_str().map(str::to_string))
        });
        pass.wrote_theme = write_theme(home, &Theme::released(shown.as_deref(), terminal_base))?;
    }
    forget_record(config_root)?;
    Ok(pass)
}

/// The palette a theme is made from, off the colours the renderer reported.
pub(crate) fn palette_of(colors: &zerocode_pty::TerminalColors) -> theme::Palette {
    let channels = |color: zerocode_pty::Rgb| [color.0, color.1, color.2];
    let mut ansi = [[0_u8; 3]; 16];
    for (slot, color) in ansi.iter_mut().zip(colors.ansi.iter()) {
        *slot = channels(*color);
    }
    theme::Palette {
        foreground: channels(colors.foreground),
        background: channels(colors.background),
        ansi,
    }
}

#[cfg(test)]
mod tests;
