//! Persistent model and reasoning-effort preferences for the plain `zo` CLI.
//!
//! The preference file deliberately lives beside the other user settings in
//! the canonical Zo home (`ZO_CONFIG_HOME`, then `ZO_HOME`, then `~/.zo`).
//! Keeping the two selected values in that document means the runtime's
//! existing config loader sees the same defaults as the plain front-end, while
//! unknown settings remain untouched when a preference is changed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::effort::Effort;

/// The user settings document that carries the two persistent defaults.
pub const PREFERENCES_FILE_NAME: &str = "settings.json";
const MODEL_KEY: &str = "model";
const EFFORT_KEY: &str = "reasoningEffort";
const WORKSPACE_TRUST_KEY: &str = "workspaceTrust";
const TOOL_DIGEST_KEY: &str = "toolDigest";
/// Whether the interactive front shows thinking cells (t-5872). Absent means
/// the front's own default (`tui::thinking::SHOW_BY_DEFAULT`); `/thinking`
/// toggles the running session without writing here.
pub const SHOW_THINKING_KEY: &str = "showThinking";
/// How many builds `zo build` runs at once on this machine, whatever their
/// projects (`build_runner`).
pub const BUILDS_AT_ONCE_KEY: &str = "buildsAtOnce";
/// The screen's look: `"marks"` (the default) or `"codex"` (`tui::style`).
pub const TUI_STYLE_KEY: &str = "tuiStyle";
/// Code blocks drawn from column 0, so a mouse copy carries no indent.
pub const CODE_BLOCKS_FLUSH_KEY: &str = "codeBlocksFlush";
/// The after-answer hint rides in the turn's divider.
pub const TURN_HINTS_IN_DIVIDER_KEY: &str = "turnHintsInDivider";
/// The working line holds still.
pub const REDUCE_MOTION_KEY: &str = "reduceMotion";
/// What an empty conversation shows: `"page"` (the default — the field in the
/// middle with the conversations to continue under it) or `"line"` (the head
/// row alone, as before 1.1.139). See `tui::start`.
pub const START_SCREEN_KEY: &str = "startScreen";
/// How a finished turn stands: `"turns"` (the default — its work folded into
/// one cell above the answer) or `"full"` (a row a step). See `tui::turns`.
pub const CONVERSATION_VIEW_KEY: &str = "conversationView";
/// The widest prose and tool rows are drawn, in columns; `0` is the pane's
/// whole width. Tables, code and command output keep the pane's width.
pub const MAX_PROSE_WIDTH_KEY: &str = "maxProseWidth";
/// The interactive TUI's colour theme slug (`tui::theme`).
pub const THEME_KEY: &str = "theme";
/// Whether the terminal's background is light or dark, for a terminal that
/// does not answer zo's colour question: `"auto"` (the default), `"light"` or
/// `"dark"` (`tui::ground`).
pub const TERMINAL_BACKGROUND_KEY: &str = "terminalBackground";

/// The one recognized `workspaceTrust` value: the interactive trust gate is
/// retired and every folder opens in full access, no prompt, outranking even
/// a folder's previously recorded decision.
pub const ALWAYS_FULL_WORKSPACE_TRUST: &str = "always-full";

/// A best-effort snapshot of the persisted startup choices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preferences {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    /// Raw `workspaceTrust` value; only [`ALWAYS_FULL_WORKSPACE_TRUST`] has
    /// meaning today. Kept as the raw string so an unrecognized value keeps
    /// the gate instead of failing the whole settings load.
    pub workspace_trust: Option<String>,
    /// Raw `toolDigest` value (`off` turns the over-cap fold off; anything
    /// else keeps the default on). Parsed by `tools::ToolDigestMode`.
    pub tool_digest: Option<String>,
    /// `showThinking`, when the document says one way or the other.
    pub show_thinking: Option<bool>,
    /// `buildsAtOnce`, when the document gives a count above none.
    pub builds_at_once: Option<usize>,
    /// Raw `tuiStyle`; `tui::style::TuiStyle::parse` reads it, and a word it
    /// does not know keeps the default look.
    pub tui_style: Option<String>,
    /// `codeBlocksFlush`, when the document says.
    pub code_blocks_flush: Option<bool>,
    /// `turnHintsInDivider`, when the document says.
    pub turn_hints_in_divider: Option<bool>,
    /// `reduceMotion`, when the document says.
    pub reduce_motion: Option<bool>,
    /// Raw `startScreen`; `tui::style::Look::chosen` reads it.
    pub start_screen: Option<String>,
    /// Raw `conversationView`; `tui::style::ConversationView::parse` reads it.
    pub conversation_view: Option<String>,
    /// `maxProseWidth`, when the document gives a count.
    pub max_prose_width: Option<usize>,
    /// Raw `theme` slug; `tui::theme` validates and resolves it.
    pub theme: Option<String>,
    /// Raw `terminalBackground`; `tui::ground::choose` reads it, and a word
    /// it does not know is `auto` with a startup warning.
    pub terminal_background: Option<String>,
}

impl Preferences {
    /// True when the user asked for every folder to open in full access with
    /// no trust prompt (`"workspaceTrust": "always-full"`).
    #[must_use]
    pub fn grants_full_trust_everywhere(&self) -> bool {
        self.workspace_trust.as_deref() == Some(ALWAYS_FULL_WORKSPACE_TRUST)
    }
}

/// Resolve the canonical preference path without creating anything.
#[must_use]
pub fn preferences_path() -> PathBuf {
    runtime::default_config_home().join(PREFERENCES_FILE_NAME)
}

/// Load the preferences from the canonical user settings file.
///
/// A missing, malformed, or non-object document is intentionally treated as
/// empty. Startup must retain its safe defaults when a settings file is
/// damaged, and a later explicit choice can repair just the known fields.
#[must_use]
pub fn load() -> Preferences {
    load_from_path(&preferences_path())
}

/// Load preferences from an explicit path. This is public so callers and
/// hermetic tests can use the same parser without changing process-global
/// environment variables.
#[must_use]
pub fn load_from_path(path: &Path) -> Preferences {
    let Ok(contents) = fs::read_to_string(path) else {
        return Preferences::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        return Preferences::default();
    };
    let Some(object) = value.as_object() else {
        return Preferences::default();
    };

    Preferences {
        model: object
            .get(MODEL_KEY)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(crate::cli_args::resolve_model_alias),
        effort: object
            .get(EFFORT_KEY)
            .and_then(Value::as_str)
            .and_then(Effort::from_token),
        workspace_trust: object
            .get(WORKSPACE_TRUST_KEY)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        tool_digest: object
            .get(TOOL_DIGEST_KEY)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        show_thinking: object.get(SHOW_THINKING_KEY).and_then(Value::as_bool),
        builds_at_once: object
            .get(BUILDS_AT_ONCE_KEY)
            .and_then(Value::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .filter(|count| *count > 0),
        tui_style: object.get(TUI_STYLE_KEY).and_then(Value::as_str).map(str::to_string),
        code_blocks_flush: object.get(CODE_BLOCKS_FLUSH_KEY).and_then(Value::as_bool),
        turn_hints_in_divider: object.get(TURN_HINTS_IN_DIVIDER_KEY).and_then(Value::as_bool),
        reduce_motion: object.get(REDUCE_MOTION_KEY).and_then(Value::as_bool),
        start_screen: object.get(START_SCREEN_KEY).and_then(Value::as_str).map(str::to_string),
        conversation_view: object.get(CONVERSATION_VIEW_KEY).and_then(Value::as_str).map(str::to_string),
        max_prose_width: object
            .get(MAX_PROSE_WIDTH_KEY)
            .and_then(Value::as_u64)
            .and_then(|columns| usize::try_from(columns).ok()),
        theme: object.get(THEME_KEY).and_then(Value::as_str).map(str::to_string),
        terminal_background: object
            .get(TERMINAL_BACKGROUND_KEY)
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

/// Persist both startup choices in the canonical user settings document.
///
/// The existing JSON object is merged rather than replaced, and publication
/// goes through Zo's atomic writer so a process interruption cannot leave a
/// half-written settings file. If the old document is corrupt, a fresh object
/// containing the explicit choices is written.
pub fn save(model: &str, effort: Effort) -> io::Result<()> {
    save_to_path(&preferences_path(), model, effort)
}

/// Write the chosen theme into the settings document at `path`: `"theme"` is
/// set to `slug`, or removed for `None` (the `terminal` theme is the absence
/// of a choice). Only that member changes — every other key, value, their
/// order and the file's own indentation stay byte for byte — and the file is
/// replaced whole (a sibling temporary file, then a rename). A missing file
/// is created for a slug and left missing for `None`.
///
/// The path is an argument so that a test never reaches the person's
/// settings; the screens pass [`preferences_path`].
///
/// # Errors
/// The file could not be read, locked or written, or it holds something
/// other than one JSON object — it is left as it was, never rewritten.
pub fn save_theme_to(path: &Path, slug: Option<&str>) -> io::Result<()> {
    // `terminal` on a file that is not there: nothing to take out, and no
    // folder or lock file made for it.
    if slug.is_none() && !path.exists() {
        return Ok(());
    }
    let _guard = runtime::file_ops::SettingsFileLock::acquire(path)?;
    let current = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    let Some(next) = with_top_level_string(current.as_deref(), THEME_KEY, slug)? else {
        return Ok(());
    };
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    crate::write_atomic(path, next.as_bytes())
}

/// `text` — a settings document, or none yet — with its top-level member
/// `key` set to the string `value`, or removed for `None`, and every other
/// byte as it was. `Ok(None)` when there is nothing to write.
///
/// The other writers of this file read it into a map and print the map
/// again, which re-orders and re-indents what a person wrote by hand. This
/// one finds the member in the text and changes those bytes alone; the
/// result is read back, and nothing is returned to write unless it is the
/// document asked for.
fn with_top_level_string(
    text: Option<&str>,
    key: &str,
    value: Option<&str>,
) -> io::Result<Option<String>> {
    let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
        return Ok(value.map(|value| format!("{{\n  {}: {}\n}}\n", json_string(key), json_string(value))));
    };
    let not_one_object =
        || io::Error::new(io::ErrorKind::InvalidData, "the settings file is not one JSON object");
    let Ok(Value::Object(mut wanted)) = serde_json::from_str::<Value>(text) else {
        return Err(not_one_object());
    };
    let edited = if let Some(value) = value {
        wanted.insert(key.to_string(), Value::String(value.to_string()));
        set_member(text, key, &json_string(value))
    } else {
        wanted.remove(key);
        remove_member(text, key)
    }
    .ok_or_else(not_one_object)?;
    if serde_json::from_str::<Value>(&edited).ok() != Some(Value::Object(wanted)) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the settings file could not be changed in place",
        ));
    }
    Ok((edited != text).then_some(edited))
}

/// `text` as a JSON string, quotes and escapes included.
fn json_string(text: &str) -> String {
    Value::String(text.to_string()).to_string()
}

/// Where one member of a JSON object stands in the object's text: its key
/// (quotes included), its value, and the comma after it when one follows.
struct MemberSpan {
    key: std::ops::Range<usize>,
    value: std::ops::Range<usize>,
    comma: Option<usize>,
}

/// The offsets of the braces of the object `text` is, and its members in
/// between. `text` is JSON already read as an object; `None` only for a
/// layout this reading does not follow, and then nothing is edited.
fn object_layout(text: &str) -> Option<(usize, Vec<MemberSpan>, usize)> {
    let bytes = text.as_bytes();
    let past_space = |mut at: usize| {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    };
    let open = past_space(0);
    if bytes.get(open) != Some(&b'{') {
        return None;
    }
    let mut members = Vec::new();
    let mut at = past_space(open + 1);
    loop {
        match bytes.get(at)? {
            b'}' => return Some((open, members, at)),
            b'"' => {}
            _ => return None,
        }
        let key_end = string_end(bytes, at)?;
        let colon = past_space(key_end);
        if bytes.get(colon) != Some(&b':') {
            return None;
        }
        let value_start = past_space(colon + 1);
        let value_stop = value_end(bytes, value_start)?;
        let after = past_space(value_stop);
        let comma = (bytes.get(after) == Some(&b',')).then_some(after);
        members.push(MemberSpan { key: at..key_end, value: value_start..value_stop, comma });
        at = comma.map_or(after, |comma| past_space(comma + 1));
    }
}

/// The offset just past the string whose opening quote is at `start`.
fn string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut at = start + 1;
    loop {
        match bytes.get(at)? {
            b'\\' => at += 2,
            b'"' => return Some(at + 1),
            _ => at += 1,
        }
    }
}

/// The offset just past the value that starts at `start`: a string, an
/// object or an array to its closing mark, anything else to the comma, the
/// brace or the space that ends it.
fn value_end(bytes: &[u8], start: usize) -> Option<usize> {
    if bytes.get(start) == Some(&b'"') {
        return string_end(bytes, start);
    }
    let mut depth = 0_usize;
    let mut at = start;
    loop {
        let byte = *bytes.get(at)?;
        if byte == b'"' {
            at = string_end(bytes, at)?;
            continue;
        }
        // Inside no bracket of its own, the value ends before the comma, the
        // closing mark of what holds it, or the space after it.
        if depth == 0 && (matches!(byte, b',' | b'}' | b']') || byte.is_ascii_whitespace()) {
            return Some(at);
        }
        match byte {
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at + 1);
                }
            }
            _ => {}
        }
        at += 1;
    }
}

/// Whether `member`'s key, read as the JSON string it is, is `key`.
fn member_is_named(text: &str, member: &MemberSpan, key: &str) -> bool {
    text.get(member.key.clone())
        .and_then(|quoted| serde_json::from_str::<String>(quoted).ok())
        .is_some_and(|name| name == key)
}

/// `text` with its member `key` holding `rendered` (a JSON value's text):
/// the value replaced where the member stands — the last one, the one a
/// reader takes, when the key is written twice — or a new member after the
/// last, written as the last one is: after the same whitespace and with the
/// same spacing at its colon.
fn set_member(text: &str, key: &str, rendered: &str) -> Option<String> {
    let (open, members, close) = object_layout(text)?;
    if let Some(member) = members.iter().rev().find(|member| member_is_named(text, member, key)) {
        return Some([text.get(..member.value.start)?, rendered, text.get(member.value.end..)?].concat());
    }
    let key = json_string(key);
    let Some(last) = members.last() else {
        return Some([text.get(..=open)?, "\n  ", key.as_str(), ": ", rendered, "\n", text.get(close..)?].concat());
    };
    let lead_from = members
        .iter()
        .rev()
        .nth(1)
        .and_then(|before| before.comma)
        .map_or(open + 1, |comma| comma + 1);
    let lead = text.get(lead_from..last.key.start)?;
    let colon = text.get(last.key.end..last.value.start)?;
    Some([text.get(..last.value.end)?, ",", lead, key.as_str(), colon, rendered, text.get(last.value.end..)?].concat())
}

/// `text` without its member `key` — every one, when the key is written
/// twice — and without the comma that went with it.
fn remove_member(text: &str, key: &str) -> Option<String> {
    let mut text = text.to_string();
    loop {
        let (open, members, close) = object_layout(&text)?;
        let Some(index) = members.iter().position(|member| member_is_named(&text, member, key)) else {
            return Some(text);
        };
        let member = members.get(index)?;
        let before = index.checked_sub(1).and_then(|before| members.get(before));
        let cut = match (before, members.get(index + 1)) {
            // Not the last: the member, its comma and the space up to the next.
            (_, Some(next)) => member.key.start..next.key.start,
            // The last of several: the comma before it goes with it.
            (Some(before), None) => before.value.end..member.value.end,
            // The only one.
            (None, None) => open + 1..close,
        };
        if !(text.is_char_boundary(cut.start) && text.is_char_boundary(cut.end)) {
            return None;
        }
        text.replace_range(cut, "");
    }
}

/// Persist choices to a specific settings path. The path's parent is created
/// on demand; this is the only write side effect of this module.
///
/// A family head is stored by its alias (`opus`, not `claude-opus-5`): the
/// alias follows the family when the provider ships the next release, while
/// a release id would freeze the choice on the day it was made.
pub fn save_to_path(path: &Path, model: &str, effort: Effort) -> io::Result<()> {
    let _guard = runtime::file_ops::SettingsFileLock::acquire(path)?;
    let mut object = read_object(path);
    let model = pinned_model_id(&persistable_model_id(model));
    if object.get(MODEL_KEY).and_then(Value::as_str) != Some(&model) {
        if let Some(smart) = object.get_mut("smart").and_then(Value::as_object_mut) {
            smart.remove(judgment_provider::connections::CODING_CONNECTION_KEY);
            smart.remove(judgment_provider::connections::PREVIOUS_CODING_KEY);
        }
    }
    object.insert(MODEL_KEY.to_string(), Value::String(model));
    object.insert(
        EFFORT_KEY.to_string(),
        Value::String(effort.canonical().to_string()),
    );
    let payload = serde_json::to_vec_pretty(&Value::Object(object)).map_err(io::Error::other)?;
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    crate::write_atomic(path, &payload)
}

fn read_object(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// `/fast` is a session-only serving tier. Store the bare family so a future
/// launch does not silently inherit a transient speed choice.
#[must_use]
pub fn persistable_model_id(model: &str) -> String {
    api::openai_fast_variant_pair(model)
        .map_or_else(|| model.to_string(), |(bare, _fast)| bare)
}

/// The value settings should carry for `model`: its family alias when it is
/// a family head, otherwise the id itself. A `provider/model` reference and a
/// model no alias names are stored as written.
#[must_use]
pub fn pinned_model_id(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.contains('/') {
        return trimmed.to_string();
    }
    api::family_alias_for(trimmed).map_or_else(|| trimmed.to_string(), str::to_string)
}

/// Settings key recording every automatic rewrite of `model`, newest last.
pub const MODEL_MIGRATIONS_KEY: &str = "modelMigrations";

/// One automatic rewrite of the persisted model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMigration {
    pub from: String,
    pub to: String,
}

/// Pin the persisted model to its family alias, once.
///
/// A settings file written before aliases were pinned carries a release id
/// (`claude-opus-5`). Left alone it would keep that release forever while the
/// family moved on. Rewritten to `opus` it follows — and the rewrite is
/// recorded under `modelMigrations` so the person can see what happened and
/// when. `None` when nothing needed rewriting.
pub fn pin_persisted_model_alias() -> io::Result<Option<ModelMigration>> {
    pin_persisted_model_alias_at(&preferences_path())
}

pub fn pin_persisted_model_alias_at(path: &Path) -> io::Result<Option<ModelMigration>> {
    let _guard = runtime::file_ops::SettingsFileLock::acquire(path)?;
    let mut object = read_object(path);
    let Some(current) = object
        .get(MODEL_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let pinned = pinned_model_id(&current);
    if pinned.eq_ignore_ascii_case(&current) {
        return Ok(None);
    }
    object.insert(MODEL_KEY.to_string(), Value::String(pinned.clone()));
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let record = serde_json::json!({
        "from": current,
        "to": pinned,
        "at": at,
        "reason": "family-alias-pin",
    });
    match object.get_mut(MODEL_MIGRATIONS_KEY) {
        Some(Value::Array(records)) => records.push(record),
        _ => {
            object.insert(MODEL_MIGRATIONS_KEY.to_string(), Value::Array(vec![record]));
        }
    }
    let payload = serde_json::to_vec_pretty(&Value::Object(object)).map_err(io::Error::other)?;
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    crate::write_atomic(path, &payload)?;
    Ok(Some(ModelMigration {
        from: current,
        to: pinned,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        load_from_path, persistable_model_id, save_to_path, Preferences, EFFORT_KEY,
    };
    use crate::effort::Effort;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn path(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "zo-preferences-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test directory");
        path.join("settings.json")
    }

    #[test]
    fn missing_and_corrupt_documents_use_safe_empty_preferences() {
        let missing = path("missing");
        assert_eq!(load_from_path(&missing), Preferences::default());
        fs::write(&missing, b"not json").expect("write corrupt settings");
        assert_eq!(load_from_path(&missing), Preferences::default());
        let _ = fs::remove_dir_all(missing.parent().expect("test parent"));
    }

    #[test]
    fn preferences_writers_preserve_a_document_locked_by_another_writer() {
        let settings = path("shared-lock");
        let original = br#"{"model":"fixture/model","smart":{"retained":true}}"#;
        fs::write(&settings, original).expect("seed");
        let lock = settings.with_extension("json.lock");
        let owner = format!("{}\n", std::process::id());
        fs::write(&lock, &owner).expect("lock");
        let save = save_to_path(&settings, "replacement/model", Effort::Smart);
        assert_eq!(save.expect_err("must respect the other writer").kind(), std::io::ErrorKind::WouldBlock);
        let pin = super::pin_persisted_model_alias_at(&settings);
        assert_eq!(pin.expect_err("migration must also lock before reading").kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(fs::read(&settings).expect("read"), original);
        assert_eq!(fs::read_to_string(&lock).expect("owner"), owner);
        fs::remove_dir_all(settings.parent().expect("parent")).expect("cleanup");
    }

    #[test]
    fn preferences_round_trip_and_preserve_unrelated_settings() {
        let _lock = crate::test_env_lock();
        let settings = path("round-trip");
        fs::write(&settings, r#"{"mcpServers":{},"custom":true}"#)
            .expect("seed settings");
        save_to_path(&settings, api::ANTHROPIC_OPUS_MODEL_ALIAS, Effort::Smart)
            .expect("save preferences");
        let loaded = load_from_path(&settings);
        let expected = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        assert_eq!(loaded.model.as_deref(), Some(expected.as_str()));
        assert_eq!(loaded.effort, Some(Effort::Smart));
        let document: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).expect("read settings"))
                .expect("valid settings");
        assert_eq!(document["custom"], true);
        assert_eq!(document[EFFORT_KEY], "smart");
        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    #[test]
    fn builds_at_once_is_a_count_above_none_or_nothing() {
        let _lock = crate::test_env_lock();
        let settings = path("builds-at-once");
        for (document, read) in [
            (r#"{"buildsAtOnce":3}"#, Some(3)),
            (r#"{"buildsAtOnce":0}"#, None),
            (r#"{"buildsAtOnce":"two"}"#, None),
            (r"{}", None),
        ] {
            fs::write(&settings, document).expect("seed settings");
            assert_eq!(load_from_path(&settings).builds_at_once, read, "{document}");
        }
        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    #[test]
    fn theme_is_a_string_or_nothing() {
        let settings = path("theme-read");
        for (document, expected) in [
            (r#"{"theme":"dracula"}"#, Some("dracula")),
            (r#"{"theme":""}"#, Some("")),
            (r#"{"theme":123}"#, None),
            (r#"{"theme":true}"#, None),
            (r"{}", None),
        ] {
            fs::write(&settings, document).expect("seed settings");
            assert_eq!(load_from_path(&settings).theme.as_deref(), expected, "{document}");
        }
        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    /// `terminalBackground` is read as `theme` is: a string, whatever it
    /// says (`tui::ground::choose` judges the word), or nothing.
    #[test]
    fn terminal_background_setting_is_a_string_or_nothing() {
        let settings = path("terminal-background-read");
        for (document, expected) in [
            (r#"{"terminalBackground":"light"}"#, Some("light")),
            (r#"{"terminalBackground":" Dark "}"#, Some(" Dark ")),
            (r#"{"terminalBackground":"blue"}"#, Some("blue")),
            (r#"{"terminalBackground":""}"#, Some("")),
            (r#"{"terminalBackground":1}"#, None),
            (r#"{"terminalBackground":false}"#, None),
            (r#"{"terminalBackground":{"light":true}}"#, None),
            (r#"{"theme":"dracula"}"#, None),
            (r"{}", None),
        ] {
            fs::write(&settings, document).expect("seed settings");
            assert_eq!(
                load_from_path(&settings).terminal_background.as_deref(),
                expected,
                "{document}"
            );
        }
        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    #[test]
    fn saving_terminal_removes_the_theme_key() {
        let settings = path("theme-save-remove");
        let seeded = "{\n    \"z\": 1,\n    \"theme\": \"dracula\",\n    \"a\": true\n}\n";
        fs::write(&settings, seeded).expect("seed settings");

        super::save_theme_to(&settings, None).expect("remove theme");
        let after_remove = fs::read_to_string(&settings).expect("read after remove");
        assert_eq!(after_remove, "{\n    \"z\": 1,\n    \"a\": true\n}\n");

        super::save_theme_to(&settings, Some("nord")).expect("save nord");
        let after_add = fs::read_to_string(&settings).expect("read after add");
        assert_eq!(
            after_add,
            "{\n    \"z\": 1,\n    \"a\": true,\n    \"theme\": \"nord\"\n}\n"
        );

        super::save_theme_to(&settings, Some("dracula")).expect("update to dracula");
        let after_update = fs::read_to_string(&settings).expect("read after update");
        assert_eq!(
            after_update,
            "{\n    \"z\": 1,\n    \"a\": true,\n    \"theme\": \"dracula\"\n}\n"
        );

        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    /// Saving a theme is an edit of one member of the person's file, not a
    /// rewrite of it: each case is the file before, the theme saved (`None`
    /// is `terminal`) and the file after — `None` where nothing is written.
    #[test]
    fn a_saved_theme_changes_one_member_and_no_other_byte() {
        let cases: &[(Option<&str>, Option<&str>, Option<&str>)] = &[
            // No file yet, an empty one, an empty object.
            (None, Some("dracula"), Some("{\n  \"theme\": \"dracula\"\n}\n")),
            (None, None, None),
            (Some(""), Some("dracula"), Some("{\n  \"theme\": \"dracula\"\n}\n")),
            (Some("{}"), Some("dracula"), Some("{\n  \"theme\": \"dracula\"\n}")),
            (Some("{ }\n"), Some("dracula"), Some("{\n  \"theme\": \"dracula\"\n}\n")),
            (Some("{}\n"), None, None),
            // One line, as a tool writes it: the new member takes the last one's spacing.
            (
                Some(r#"{"smart":{"autoClassifier":"off"}}"#),
                Some("dracula"),
                Some(r#"{"smart":{"autoClassifier":"off"},"theme":"dracula"}"#),
            ),
            (Some(r#"{"a": 1, "b": [2, 3]}"#), Some("nord"), Some(r#"{"a": 1, "b": [2, 3], "theme": "nord"}"#)),
            // First, last and only member: the comma that goes with it.
            (Some(r#"{"theme":"nord","a":1}"#), None, Some(r#"{"a":1}"#)),
            (Some(r#"{"a":1,"theme":"nord"}"#), None, Some(r#"{"a":1}"#)),
            (Some(r#"{"a":1,"theme":"nord","b":2}"#), None, Some(r#"{"a":1,"b":2}"#)),
            (Some(r#"{"theme":"nord"}"#), None, Some("{}")),
            (Some("{\n  \"theme\": \"nord\"\n}\n"), None, Some("{}\n")),
            (Some("{\n  \"theme\": \"nord\",\n  \"a\": 1\n}\n"), None, Some("{\n  \"a\": 1\n}\n")),
            (Some("{\n  \"a\": 1,\n  \"theme\": \"nord\"\n}\n"), None, Some("{\n  \"a\": 1\n}\n")),
            // A changed value, in place; the same value writes nothing.
            (Some(r#"{"theme":"nord","a":1}"#), Some("dracula"), Some(r#"{"theme":"dracula","a":1}"#)),
            (Some(r#"{"theme": 123, "a": 1}"#), Some("nord"), Some(r#"{"theme": "nord", "a": 1}"#)),
            (Some(r#"{"theme":"nord","a":1}"#), Some("nord"), None),
            (Some(r#"{"a":1}"#), None, None),
            // A `theme` inside another member, and one spelled in a string, are not the setting.
            (
                Some(r#"{"ui":{"theme":"x"},"note":"\"theme\": \"y\" }","theme":"nord"}"#),
                Some("dracula"),
                Some(r#"{"ui":{"theme":"x"},"note":"\"theme\": \"y\" }","theme":"dracula"}"#),
            ),
            (Some(r#"{"ui":{"theme":"x"}}"#), None, None),
            (Some(r#"{"ui":{"theme":"x"}}"#), Some("nord"), Some(r#"{"ui":{"theme":"x"},"theme":"nord"}"#)),
            // Tabs, no newline at the end, a brace inside a string.
            (
                Some("{\n\t\"a\": [1, {\"b\": \"}\"}],\n\t\"theme\": \"nord\"\n}"),
                Some("dracula"),
                Some("{\n\t\"a\": [1, {\"b\": \"}\"}],\n\t\"theme\": \"dracula\"\n}"),
            ),
            (
                Some("{\n\t\"a\": [1, {\"b\": \"}\"}]\n}"),
                Some("dracula"),
                Some("{\n\t\"a\": [1, {\"b\": \"}\"}],\n\t\"theme\": \"dracula\"\n}"),
            ),
            // Windows line ends stay Windows line ends.
            (
                Some("{\r\n  \"a\": 1\r\n}\r\n"),
                Some("dracula"),
                Some("{\r\n  \"a\": 1,\r\n  \"theme\": \"dracula\"\r\n}\r\n"),
            ),
            // The key written with an escape is the same key; written twice, the last one is read.
            (Some(r#"{"theme":"nord","a":1}"#), None, Some(r#"{"a":1}"#)),
            (Some(r#"{"theme":"a","x":1,"theme":"b"}"#), None, Some(r#"{"x":1}"#)),
            (
                Some(r#"{"theme":"a","x":1,"theme":"b"}"#),
                Some("c"),
                Some(r#"{"theme":"a","x":1,"theme":"c"}"#),
            ),
        ];
        for (before, slug, after) in cases {
            let written = super::with_top_level_string(*before, super::THEME_KEY, *slug)
                .unwrap_or_else(|error| panic!("{before:?} with {slug:?}: {error}"));
            assert_eq!(written.as_deref(), *after, "{before:?} with {slug:?}");
        }
        // Something other than one JSON object is not edited at all.
        for damaged in ["not json", "[1]", r#"{"a":1,}"#, r#"{"a":1} trailing"#, "\"theme\""] {
            for slug in [Some("dracula"), None] {
                let refused = super::with_top_level_string(Some(damaged), super::THEME_KEY, slug);
                assert_eq!(
                    refused.map_err(|error| error.kind()),
                    Err(std::io::ErrorKind::InvalidData),
                    "{damaged:?} with {slug:?}"
                );
            }
        }
    }

    /// The file side of [`super::save_theme_to`]: a missing file (its folder
    /// too) is created for a theme and left missing for `terminal`, and a
    /// damaged file is refused and left as it was.
    #[test]
    fn a_saved_theme_creates_a_missing_file_and_leaves_a_damaged_one_alone() {
        let home = path("theme-save-files");
        let folder = home.parent().expect("test parent").to_path_buf();
        let missing = folder.join("not-yet").join("settings.json");
        super::save_theme_to(&missing, None).expect("terminal on a missing file");
        assert!(!missing.exists(), "saving terminal created a settings file");
        super::save_theme_to(&missing, Some("dracula")).expect("save into a missing folder");
        assert_eq!(fs::read_to_string(&missing).expect("read the new file"), "{\n  \"theme\": \"dracula\"\n}\n");
        assert_eq!(load_from_path(&missing).theme.as_deref(), Some("dracula"));

        let damaged = folder.join("damaged.json");
        fs::write(&damaged, "{ \"model\": \"x\", ").expect("seed a damaged file");
        for slug in [Some("dracula"), None] {
            let refused = super::save_theme_to(&damaged, slug).expect_err("a damaged file is refused");
            assert_eq!(refused.kind(), std::io::ErrorKind::InvalidData, "{slug:?}");
            assert_eq!(fs::read_to_string(&damaged).expect("read"), "{ \"model\": \"x\", ", "{slug:?}");
        }
        assert!(
            fs::read_dir(&folder).expect("list").flatten().all(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                !name.contains(".lock") && !name.contains(".tmp")
            }),
            "a lock or a temporary file was left behind"
        );
        let _ = fs::remove_dir_all(&folder);
    }

    /// A family head is persisted by its alias, and a release id already on
    /// disk is pinned to the alias once, with a migration record.
    #[test]
    fn family_heads_are_pinned_to_their_alias() {
        let _lock = crate::test_env_lock();
        let opus = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        assert_eq!(super::pinned_model_id(&opus), "opus");
        assert_eq!(super::pinned_model_id("claude-opus-4-8"), "claude-opus-4-8", "not a family head");
        assert_eq!(super::pinned_model_id("openai/gpt-5.6-terra"), "openai/gpt-5.6-terra");

        let settings = path("pin");
        save_to_path(&settings, &opus, Effort::Smart).expect("save");
        let document: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(document["model"], "opus");
        assert_eq!(load_from_path(&settings).model.as_deref(), Some(opus.as_str()));

        fs::write(&settings, format!(r#"{{"model":"{opus}","custom":true}}"#)).unwrap();
        let migration = super::pin_persisted_model_alias_at(&settings).expect("pin").expect("rewritten");
        assert_eq!(migration.from, opus);
        assert_eq!(migration.to, "opus");
        let document: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(document["model"], "opus");
        assert_eq!(document["custom"], true);
        assert_eq!(document[super::MODEL_MIGRATIONS_KEY][0]["to"], "opus");
        assert_eq!(
            super::pin_persisted_model_alias_at(&settings).expect("pin again"),
            None,
            "the second pass has nothing to do"
        );
        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    #[test]
    fn persisted_fast_models_are_normalized_to_the_bare_family() {
        let _lock = crate::test_env_lock();
        let _catalog =
            crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        let current = api::builtin_provider_catalog()
            .iter()
            .find(|entry| entry.alias == api::OPENAI_LATEST_MODEL_ALIAS)
            .expect("shipped current OpenAI model")
            .canonical_model_id;
        let (bare, fast) =
            api::openai_fast_variant_pair(current).expect("current fast service-tier pair");
        assert_eq!(
            persistable_model_id(&fast),
            bare
        );
        assert_eq!(persistable_model_id(current), current);
        let anthropic = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        assert_eq!(persistable_model_id(&anthropic), anthropic);
    }

    /// Marks a run of this test binary as a child of
    /// [`one_catalog_view_from_process_start_in_either_order`], naming the
    /// order it asks in.
    const CATALOG_VIEW_ORDER_ENV: &str = "ZO_TEST_CATALOG_VIEW_ORDER";
    /// The child asks the catalog first, then reads the catalog file.
    const START_FIRST: &str = "start-first";
    /// The child reads the catalog file first, then asks.
    const CATALOG_FIRST: &str = "catalog-first";
    /// The marker before a child's answer on its stdout.
    const CATALOG_VIEW_LINE: &str = "catalog-view ";

    /// What the catalog answers: where the opus alias resolves, the alias that
    /// id is pinned to, every family alias with its head, and each provider's
    /// latest model.
    fn catalog_view() -> serde_json::Value {
        let opus = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        let heads: std::collections::BTreeMap<&str, &str> = api::provider_catalog()
            .iter()
            .filter(|entry| api::family_alias_for(entry.canonical_model_id) == Some(entry.alias))
            .map(|entry| (entry.alias, entry.canonical_model_id))
            .collect();
        let latest: Vec<Option<&str>> = [
            api::ProviderKind::Anthropic,
            api::ProviderKind::OpenAi,
            api::ProviderKind::Google,
            api::ProviderKind::Xai,
        ]
        .into_iter()
        .map(api::latest_model_for_provider)
        .collect();
        serde_json::json!({
            "opus": opus,
            "pinned": super::pinned_model_id(&opus),
            "heads": heads,
            "latest": latest,
        })
    }

    /// Write `home`'s catalog file the way an overlay moves a family: the opus
    /// alias on another shipped release whose id carries the family's name.
    /// Read from the shipped catalog's own bytes, so nothing here asks the live
    /// catalog before the file exists. Returns that release.
    fn declare_another_opus_release(home: &Path) -> String {
        let shipped: serde_json::Value =
            serde_json::from_str(api::builtin_model_catalog_json()).expect("shipped catalog");
        let row = shipped["aliases"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["alias"] == api::ANTHROPIC_OPUS_MODEL_ALIAS))
            .expect("the shipped opus row");
        let head = row["canonical"].as_str().expect("the shipped opus head");
        let release = shipped["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| model["provider"] == row["provider"])
            .flat_map(|model| model["ids"].as_array().into_iter().flatten())
            .filter_map(serde_json::Value::as_str)
            .find(|id| *id != head && id.contains(api::ANTHROPIC_OPUS_MODEL_ALIAS))
            .expect("another shipped opus release")
            .to_string();
        let overlay = serde_json::json!({
            "aliases": [{
                "alias": api::ANTHROPIC_OPUS_MODEL_ALIAS,
                "canonical": release,
                "provider": row["provider"],
            }],
        });
        fs::create_dir_all(home).expect("config home");
        fs::write(home.join(runtime::model_catalog::CATALOG_FILE), overlay.to_string())
            .expect("write the catalog file");
        release
    }

    /// A child's side: in its own test home, declare the moved release, then
    /// print what the catalog answers — at process start when the order asks
    /// first, and after the catalog file is read.
    fn answer_as_a_catalog_view_child(order: &str) {
        let _lock = crate::test_env_lock();
        let home = runtime::default_config_home();
        let release = declare_another_opus_release(&home);
        let mut views = Vec::new();
        if order == START_FIRST {
            views.push(("start", catalog_view()));
        }
        crate::runtime_support::publish_model_catalog();
        views.push(("catalog", catalog_view()));
        for (phase, view) in views {
            println!(
                "{CATALOG_VIEW_LINE}{}",
                serde_json::json!({ "phase": phase, "declared": release, "view": view })
            );
        }
        let _ = fs::remove_file(home.join(runtime::model_catalog::CATALOG_FILE));
    }

    /// The views a fresh run of this test binary, on this test alone, reports
    /// when it asks in `order`. Its home is temporary and it inherits no
    /// catalog from this process's environment.
    fn catalog_views_of_a_child(order: &str) -> Vec<serde_json::Value> {
        let home = crate::support::temp_dir("catalog-view-child");
        let module = module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_crate, path)| path);
        let filter = format!("{module}::one_catalog_view_from_process_start_in_either_order");
        let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
            .arg(&filter)
            .args(["--exact", "--nocapture", "--test-threads=1"])
            .env(CATALOG_VIEW_ORDER_ENV, order)
            .env("HOME", &home)
            .env("ZO_CONFIG_HOME", &home)
            .env("ZO_HOME", &home)
            .env("ZO_DISABLE_KEYCHAIN", "1")
            .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
            .env_remove(api::MODEL_CONTEXT_WINDOWS_ENV)
            .env_remove(api::MODEL_EFFORT_CEILINGS_ENV)
            .env_remove(api::CUSTOM_PROVIDERS_ENV)
            .output()
            .expect("run a child");
        let _ = fs::remove_dir_all(&home);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "the {order} child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // libtest names the test (`test … ... `) on the line the child's
        // first answer lands on, so the marker is looked for, not anchored.
        stdout
            .lines()
            .filter_map(|line| line.split_once(CATALOG_VIEW_LINE).map(|(_, view)| view))
            .map(|line| serde_json::from_str(line).expect("a view line"))
            .collect()
    }

    /// One catalog per process (t-44087): what the catalog answers at process
    /// start — where the opus alias resolves, the alias it is pinned to, the
    /// family heads — is what it answers once the catalog file is read, and
    /// reading the file first changes nothing. Each order runs in a fresh run
    /// of this binary in a temporary home: the catalog is process-global, so
    /// only a new process can be asked at its start.
    #[test]
    fn one_catalog_view_from_process_start_in_either_order() {
        if let Ok(order) = std::env::var(CATALOG_VIEW_ORDER_ENV) {
            answer_as_a_catalog_view_child(&order);
            return;
        }
        let start_first = catalog_views_of_a_child(START_FIRST);
        let catalog_first = catalog_views_of_a_child(CATALOG_FIRST);
        let [at_start, after_reading] = start_first.as_slice() else {
            panic!("two views from the {START_FIRST} child: {start_first:?}");
        };
        let [read_first] = catalog_first.as_slice() else {
            panic!("one view from the {CATALOG_FIRST} child: {catalog_first:?}");
        };
        assert_eq!(
            after_reading["view"]["opus"], after_reading["declared"],
            "the catalog file was read: {after_reading}"
        );
        assert_eq!(
            at_start["view"], after_reading["view"],
            "process start answered from another catalog than the one the file declares"
        );
        assert_eq!(
            read_first["view"], after_reading["view"],
            "reading the catalog file first changes nothing"
        );
    }
}
