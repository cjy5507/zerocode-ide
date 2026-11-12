//! Narrowing the "always allow" rules an older zo saved wider than the command
//! the person approved (t-105662).
//!
//! Up to v1.1.143 "always allow" stored the approved command as the rule's
//! subject with only `\ ( )` escaped, so a command carrying `*`, `?` or `[`
//! became a glob: approving `cargo test build/*` also allowed
//! `cargo test build/x && cargo publish`. v1.1.144 writes the subject
//! literally ([`crate::permissions`]), but the rules already saved stay in each
//! project's `.zo/settings.local.json`. Once per project, before the session
//! reads its permission rules ([`super::ConfigLoader::load_for_session`]), the
//! allow rules there that read as a glob are rewritten as the literal command
//! they were approved for. The approved command is still allowed; anything
//! else asks again. A glob that never matched its own text (`bash(rm [ab])`)
//! never allowed the command approved, so it is taken out rather than written
//! literally: narrowing allows no command that was not allowed before
//! (t-105746).
//!
//! What is left alone: deny and ask rules (a wide refusal is the safe side), a
//! `prefix:*` rule, a rule whose subject is `*`, a bare tool name, a glob
//! already in the literal form, every other settings file, and — after the
//! project is marked done — any glob the person writes by hand.
//!
//! The marker, the lock, the notice for the next session a person watches,
//! and the file as it was all live in the project's zo state folder, outside
//! the repository, so nothing new can be committed by mistake. The settings
//! file is replaced by a rename, never cut and rewritten in place, so a reader
//! — and a failure at any point — sees the file as it was or the narrowed one,
//! whole. Two zo starting in one project take turns on the lock, and the one
//! that waited reads what the other wrote. Any failure changes nothing and
//! leaves no marker, so the next session tries again; it never stops a
//! session from starting.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use core_types::paths::{restrict_permissions_owner_only, write_private_file, ParentDirPolicy};

use super::parsers::{parse_json_object_contents, parse_optional_permission_rules};
use super::{
    trusted_uncommitted_zo_file_snapshot, zo_project_state_dir, ConfigError,
    RuntimePermissionRuleConfig,
};
use crate::json::JsonValue;
use crate::permissions::{narrowed_allow_rule, narrowed_ordered_allow_rule, AllowRuleNarrowing};
use crate::secure_fs::{remove_abandoned_temp_files, write_atomic_owner_only_named_temp, Durability};

/// The one file narrowed: where "always allow" writes its rules.
const SETTINGS_FILE: &str = "settings.local.json";
/// The folder under a project's zo state that holds the marker, the lock, the
/// notice and the kept originals.
const STATE_FOLDER: &str = "allow-always-narrow";
/// Present once the project has been narrowed (or had nothing to narrow).
const DONE_MARKER: &str = "done";
/// Held while one zo looks at and narrows the file, so a second zo starting in
/// the same project waits, then finds the marker and only reads. Never
/// removed: a waiter may hold the old file, and a new one would let a third
/// zo in beside it.
const LOCK_FILE: &str = "lock";
/// How long a session waits for another zo's narrowing before it reads the
/// file as it is, unnarrowed and unmarked (the next session tries again).
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(20);
/// A narrowing killed between its write and its rename leaves its temporary
/// file in `.zo/`; under the lock, one older than this is nobody's.
const ABANDONED_TEMP_AGE: Duration = Duration::from_secs(60);
/// A cutoff past every temporary file's write time, so the sweep after a
/// narrowing takes them whatever their age.
const ANY_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// What the next session a person watches says, until it has said it.
const NOTICE_FILE: &str = "notice.json";
/// How many rules of each kind the notice names; the rest it counts.
const NOTICE_RULES_SHOWN: usize = 5;
/// How many characters of one rule the notice shows.
const NOTICE_RULE_CHARS: usize = 80;

/// One rule the narrowing rewrote: as it was, and as it now reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarrowedRule {
    pub before: String,
    pub after: String,
}

/// What [`narrow_broad_allow_always_rules`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowAlwaysNarrowing {
    /// The project was marked done before; nothing was read.
    AlreadyDone,
    /// No rule needed narrowing; the project is now marked done.
    NothingToNarrow,
    /// These rules were narrowed, and these (`dropped`, as they were) taken
    /// out because their glob never matched the command approved; the file
    /// as it was is kept at `backup`.
    Narrowed {
        rules: Vec<NarrowedRule>,
        dropped: Vec<String>,
        backup: PathBuf,
    },
    /// Another zo held the lock past the wait: nothing was read or changed,
    /// and no marker was left, so the next session tries again.
    Busy,
    /// Nothing was changed and no marker was left, so the next session tries
    /// again: the file is not the person's trusted one, it is empty, it
    /// changed while it was read, it does not parse, the lock could not be
    /// opened, or a write failed.
    Skipped,
}

/// Narrow the broad allow rules in `<cwd>/.zo/settings.local.json`, once per
/// project. See the module documentation for what is narrowed and what is not.
#[must_use]
pub fn narrow_broad_allow_always_rules(cwd: &Path) -> AllowAlwaysNarrowing {
    narrow_in(cwd, &state_folder(cwd), LOCK_WAIT)
}

/// The notice for a session a person watches, taken so it is said once: how
/// many rules were narrowed or taken out, which, and where the original is
/// kept. `None` when there is nothing to say, or when the notice could not be
/// removed (it is then not said at all rather than said on every start).
#[must_use]
pub fn take_allow_always_narrow_notice(cwd: &Path) -> Option<String> {
    take_notice_in(&state_folder(cwd))
}

fn state_folder(cwd: &Path) -> PathBuf {
    zo_project_state_dir(cwd).join(STATE_FOLDER)
}

fn settings_relative() -> PathBuf {
    Path::new(".zo").join(SETTINGS_FILE)
}

fn narrow_in(cwd: &Path, folder: &Path, lock_wait: Duration) -> AllowAlwaysNarrowing {
    let path = cwd.join(settings_relative());
    narrow_with(
        cwd,
        folder,
        lock_wait,
        &mut || trusted_uncommitted_zo_file_snapshot(cwd, &path, SETTINGS_FILE),
        &mut |bytes| {
            write_atomic_owner_only_named_temp(cwd, &settings_relative(), bytes, Durability::Flushed)
        },
    )
}

/// The narrowing, with the trusted read and the settings write passed in so a
/// file changing between reads and a write failing part way can be shown to
/// change nothing.
fn narrow_with(
    cwd: &Path,
    folder: &Path,
    lock_wait: Duration,
    snapshot: &mut dyn FnMut() -> Option<String>,
    write_settings: &mut dyn FnMut(&[u8]) -> std::io::Result<()>,
) -> AllowAlwaysNarrowing {
    // Before any lock or git work: a marked project costs one `stat` per
    // session.
    if std::fs::symlink_metadata(folder.join(DONE_MARKER)).is_ok() {
        return AllowAlwaysNarrowing::AlreadyDone;
    }
    let path = cwd.join(settings_relative());
    // "Always allow" never creates this file, so a project without one holds
    // no rule an older zo saved; a glob written into it later is the person's.
    if matches!(std::fs::symlink_metadata(&path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
    {
        return mark_done(folder, AllowAlwaysNarrowing::NothingToNarrow);
    }
    let _lock = match take_lock(folder, lock_wait) {
        Ok(Some(lock)) => lock,
        Ok(None) => return AllowAlwaysNarrowing::Busy,
        Err(_) => return AllowAlwaysNarrowing::Skipped,
    };
    // The zo that waited finds what the one before it did, and only reads.
    if std::fs::symlink_metadata(folder.join(DONE_MARKER)).is_ok() {
        return AllowAlwaysNarrowing::AlreadyDone;
    }
    if let Some(cutoff) = SystemTime::now().checked_sub(ABANDONED_TEMP_AGE) {
        remove_abandoned_temp_files(cwd, &settings_relative(), cutoff);
    }
    let Some(original) = snapshot() else {
        return AllowAlwaysNarrowing::Skipped;
    };
    // An empty file reads as no settings at all; it is more likely a write
    // caught half way (an editor's, an older zo's) than the person's file, so
    // it is passed by unmarked and the next session looks again.
    if original.trim().is_empty() {
        return AllowAlwaysNarrowing::Skipped;
    }
    let (rendered, rules, dropped) = match narrow_allow_always_document(&path, &original) {
        Ok(Some(narrowed)) => narrowed,
        Ok(None) => return mark_done(folder, AllowAlwaysNarrowing::NothingToNarrow),
        Err(_) => return AllowAlwaysNarrowing::Skipped,
    };
    let Some(backup) = keep_original(folder, &original) else {
        return AllowAlwaysNarrowing::Skipped;
    };
    // Never write over an edit made while the candidate was built.
    if snapshot().as_deref() != Some(original.as_str()) {
        let _ = std::fs::remove_file(&backup);
        return AllowAlwaysNarrowing::Skipped;
    }
    // The write lands by rename, so the file is the original or the narrowed
    // text, whole — even when the write reports a failure (the flush after
    // the rename can fail too). Read back through the trusted gate the next
    // session uses to tell which.
    let written = write_settings(rendered.as_bytes());
    let now = snapshot();
    if now.as_deref() != Some(rendered.as_str()) {
        if let Err(error) = &written {
            eprintln!(
                "[zo] warning: could not narrow the \"always allow\" rules in {}: {error}; the file \
                 is unchanged and the next session tries again",
                path.display()
            );
            // The file is still the original, so the copy is not needed.
            if now.as_deref() == Some(original.as_str()) {
                let _ = std::fs::remove_file(&backup);
            }
        }
        // Otherwise an edit landed right after the write; that edit is the
        // person's, so it is not written over — the copy stays, and the next
        // session tries again.
        return AllowAlwaysNarrowing::Skipped;
    }
    // The file is narrowed and no zo using this lock is writing it, so every
    // temporary file of this name in `.zo/` — however new — is one a killed
    // narrowing left, holding the whole local settings. Once the project is
    // marked no session comes back to take it, so it goes now. (A zo with
    // another state folder writing just then loses its rename and tries again
    // next session.)
    remove_abandoned_temp_files(cwd, &settings_relative(), SystemTime::now() + ANY_AGE);
    let strings = |items: Vec<String>| JsonValue::Array(items.into_iter().map(JsonValue::String).collect());
    let notice = JsonValue::Object(
        [
            ("settings".to_string(), JsonValue::String(path.display().to_string())),
            ("backup".to_string(), JsonValue::String(backup.display().to_string())),
            ("rules".to_string(), strings(rules.iter().map(|rule| rule.before.clone()).collect())),
            ("dropped".to_string(), strings(dropped.clone())),
        ]
        .into_iter()
        .collect(),
    );
    // The notice before the marker: a marker without a notice would narrow
    // silently, while a notice without a marker only re-checks a file that has
    // nothing broad left in it. A notice that cannot be written is logged and
    // the project still marked — the file is narrowed either way.
    if let Err(error) = write_private_file(
        &folder.join(NOTICE_FILE),
        notice.render().as_bytes(),
        &ParentDirPolicy::CreateAndRestrict,
    ) {
        eprintln!(
            "[zo] warning: the \"always allow\" rules in {} were narrowed, but the notice for the \
             next session could not be written: {error}; the file as it was is kept at {}",
            path.display(),
            backup.display()
        );
    }
    mark_done(folder, AllowAlwaysNarrowing::Narrowed { rules, dropped, backup })
}

/// Take the project's narrowing lock, waiting up to `wait` for another zo
/// holding it. `Ok(None)` when it was still held at the deadline. The lock is
/// the operating system's file lock, so a zo that dies lets it go.
fn take_lock(folder: &Path, wait: Duration) -> std::io::Result<Option<std::fs::File>> {
    std::fs::create_dir_all(folder)?;
    restrict_permissions_owner_only(folder)?;
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(folder.join(LOCK_FILE))?;
    let deadline = Instant::now() + wait;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Some(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(LOCK_POLL);
            }
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// Leave the marker and answer `outcome`, or [`AllowAlwaysNarrowing::Skipped`]
/// when the marker cannot be written — an unmarked project is tried again.
/// A narrowing already written stays written either way.
fn mark_done(folder: &Path, outcome: AllowAlwaysNarrowing) -> AllowAlwaysNarrowing {
    let marked =
        write_private_file(&folder.join(DONE_MARKER), b"", &ParentDirPolicy::CreateAndRestrict)
            .is_ok();
    if marked || matches!(outcome, AllowAlwaysNarrowing::Narrowed { .. }) {
        outcome
    } else {
        AllowAlwaysNarrowing::Skipped
    }
}

/// Keep `original` byte for byte in `folder`, readable by the person alone,
/// under a name no earlier attempt used; `None` when it cannot be kept whole.
fn keep_original(folder: &Path, original: &str) -> Option<PathBuf> {
    let millis = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let backup = folder.join(format!("{SETTINGS_FILE}.before-narrowing-{millis}"));
    if std::fs::symlink_metadata(&backup).is_ok() {
        return None;
    }
    write_private_file(&backup, original.as_bytes(), &ParentDirPolicy::CreateAndRestrict).ok()?;
    if std::fs::read(&backup).ok()? == original.as_bytes() {
        Some(backup)
    } else {
        let _ = std::fs::remove_file(&backup);
        None
    }
}

/// The document to write, the rules rewritten, and the rules taken out
/// (as they were), when any rule needs either.
type NarrowedDocument = (String, Vec<NarrowedRule>, Vec<String>);

/// Narrow the broad allow rules in `contents`, returning the document to write,
/// the rules rewritten and the rules taken out, or `None` when no rule needs
/// either.
///
/// Pure on purpose, like `merge_allow_always_rules` beside it: the form
/// follows the file (`permissions.allow`, or `"…=allow"` specs in the ordered
/// `permissions.rules`), each rule is rewritten where it stands so the order
/// holds, and the document is rendered as `persist_allow_always_rules` renders
/// it — one line — so a file zo has saved before keeps its shape. The
/// candidate is read back through the parser a session uses so a document the
/// renderer would change in any other way is refused.
fn narrow_allow_always_document(
    path: &Path,
    contents: &str,
) -> Result<Option<NarrowedDocument>, ConfigError> {
    let mut root = parse_json_object_contents(path, contents)?;
    let existing = parse_optional_permission_rules(&JsonValue::Object(root.clone()))?;
    let [allow_key, _, _, rules_key] = RuntimePermissionRuleConfig::LISTS;
    let ordered = !existing.rules().is_empty();

    let Some(JsonValue::Object(permissions)) = root.get_mut("permissions") else {
        return Ok(None);
    };
    let Some(JsonValue::Array(list)) = permissions.get_mut(if ordered { rules_key } else { allow_key })
    else {
        return Ok(None);
    };
    let mut narrowed = Vec::new();
    let mut dropped = Vec::new();
    let mut kept = Vec::with_capacity(list.len());
    for value in std::mem::take(list) {
        let rule = match value {
            JsonValue::String(rule) => rule,
            other => {
                kept.push(other);
                continue;
            }
        };
        let decision = if ordered {
            narrowed_ordered_allow_rule(&rule)
        } else {
            narrowed_allow_rule(&rule)
        }
        .map_err(|error| ConfigError::Parse(format!("{}: {error}", path.display())))?;
        match decision {
            AllowRuleNarrowing::Keep => kept.push(JsonValue::String(rule)),
            AllowRuleNarrowing::Narrow(after) => {
                kept.push(JsonValue::String(after.clone()));
                narrowed.push(NarrowedRule { before: rule, after });
            }
            AllowRuleNarrowing::Drop => dropped.push(rule),
        }
    }
    *list = kept;
    if narrowed.is_empty() && dropped.is_empty() {
        return Ok(None);
    }

    let rendered = JsonValue::Object(root.clone()).render();
    let reread = parse_json_object_contents(path, &rendered)?;
    if reread != root {
        return Err(ConfigError::Parse(format!(
            "{}: the narrowed settings did not survive a JSON round-trip",
            path.display()
        )));
    }
    parse_optional_permission_rules(&JsonValue::Object(reread))?;
    Ok(Some((rendered, narrowed, dropped)))
}

fn take_notice_in(folder: &Path) -> Option<String> {
    let path = folder.join(NOTICE_FILE);
    let contents = std::fs::read_to_string(&path).ok()?;
    std::fs::remove_file(&path).ok()?;
    let JsonValue::Object(notice) = JsonValue::parse(&contents).ok()? else {
        return None;
    };
    let text = |key: &str| match notice.get(key) {
        Some(JsonValue::String(value)) => Some(value.clone()),
        _ => None,
    };
    let strings = |key: &str| match notice.get(key) {
        Some(JsonValue::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                JsonValue::String(item) => Some(item.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    let (rules, dropped) = (strings("rules"), strings("dropped"));
    if rules.is_empty() && dropped.is_empty() {
        return None;
    }
    Some(narrow_notice_text(&text("settings")?, &rules, &dropped, &text("backup")?))
}

/// The notice's words: for the rules narrowed and the rules taken out, the
/// count and up to [`NOTICE_RULES_SHOWN`] of them as they were, the rest
/// counted; where the original is kept; and what to do if a rule was wide on
/// purpose. Every rule and path is shown through [`shown`], so a saved rule
/// cannot write to the terminal.
fn narrow_notice_text(settings: &str, rules: &[String], dropped: &[String], backup: &str) -> String {
    let settings = shown(settings, usize::MAX);
    let noun = |count: usize| if count == 1 { "rule" } else { "rules" };
    let mut said = String::new();
    if !rules.is_empty() {
        let count = rules.len();
        let _ = write!(
            said,
            "zo narrowed {count} \"always allow\" {} in {settings} that an earlier zo saved wider \
             than the command approved: {}. The approved command is still allowed; anything else \
             asks again. ",
            noun(count),
            shown_list(rules)
        );
    }
    if !dropped.is_empty() {
        let count = dropped.len();
        let (form, approved, it, command, asks, refused, them) = if count == 1 {
            ("form", "the command", "it", "that command", "asks", "is refused", "it")
        } else {
            ("forms", "the commands", "they", "those commands", "ask", "are refused", "them")
        };
        let _ = write!(
            said,
            "zo removed {count} \"always allow\" {} in {settings} whose saved {form} never matched \
             {approved} approved, so {it} never allowed {command}: {}. Next time {command} {asks} \
             again — or {refused} where an earlier deny rule matches — and \"always allow\" now \
             saves {them} as approved. ",
            noun(count),
            shown_list(dropped)
        );
    }
    let _ = write!(
        said,
        "The file as it was is kept at {}. If you wrote one of these wide on purpose, write it again.",
        shown(backup, usize::MAX)
    );
    said
}

/// Up to [`NOTICE_RULES_SHOWN`] rules in backticks, the rest counted.
fn shown_list(rules: &[String]) -> String {
    let mut list = rules
        .iter()
        .take(NOTICE_RULES_SHOWN)
        .map(|rule| format!("`{}`", shown(rule, NOTICE_RULE_CHARS)))
        .collect::<Vec<_>>()
        .join(", ");
    let rest = rules.len().saturating_sub(NOTICE_RULES_SHOWN);
    if rest > 0 {
        let _ = write!(list, " and {rest} more");
    }
    list
}

/// `text` as the notice may show it: a control character (an escape sequence
/// would reach the terminal as it is) or a bidirectional override written as
/// `\u{..}`, and anything past `limit` characters cut with `…`.
fn shown(text: &str, limit: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index == limit {
            out.push('…');
            break;
        }
        if ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
            let _ = write!(out, "\\u{{{:x}}}", u32::from(ch));
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        narrow_allow_always_document, narrow_in, narrow_notice_text, narrow_with, settings_relative,
        take_notice_in, AllowAlwaysNarrowing, NarrowedRule, DONE_MARKER, LOCK_FILE, LOCK_WAIT,
        NOTICE_FILE, SETTINGS_FILE, STATE_FOLDER,
    };
    use crate::config::tests::{init_git_repo_with, overrides_stable};
    use crate::config::{trusted_uncommitted_zo_file_snapshot, ConfigLoader, RuntimeConfig};
    use crate::json::JsonValue;
    use crate::permissions::{PermissionMode, PermissionOutcome, PermissionPolicy};
    use crate::test_root::TestRoot;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    const BROAD: &str = "bash(cargo test build/*)";
    const NARROWED: &str = "bash(cargo test build/[*])";
    const APPROVED: &str = "cargo test build/*";
    const WIDER: &str = "cargo test build/x && cargo publish";
    /// Saved by v1.1.143 for the approved `rm [ab]`; its class meets `rm a`.
    const NEVER_MATCHED: &str = "bash(rm [ab])";

    /// A project whose `.zo/settings.local.json` holds `settings`, uncommitted
    /// in a git repository (the person's trusted file, unless `commit`), with
    /// a zo state folder of its own.
    struct Project {
        _root: TestRoot,
        cwd: PathBuf,
        home: PathBuf,
        folder: PathBuf,
    }

    impl Project {
        fn new(settings: &str, commit: bool) -> Self {
            let root = TestRoot::new("allow-always-narrow");
            let cwd = root.join("project");
            let home = root.join("home").join(".zo");
            fs::create_dir_all(cwd.join(".zo")).expect("project .zo");
            fs::create_dir_all(&home).expect("home .zo");
            fs::write(cwd.join(".zo").join(SETTINGS_FILE), settings).expect("write settings");
            init_git_repo_with(&cwd, ".zo/settings.local.json", commit);
            let folder = root.join("state").join(STATE_FOLDER);
            Self { _root: root, cwd, home, folder }
        }

        fn settings_path(&self) -> PathBuf {
            self.cwd.join(".zo").join(SETTINGS_FILE)
        }

        fn settings(&self) -> String {
            fs::read_to_string(self.settings_path()).expect("read settings")
        }

        fn narrow(&self) -> AllowAlwaysNarrowing {
            narrow_in(&self.cwd, &self.folder, LOCK_WAIT)
        }

        fn load(&self) -> RuntimeConfig {
            ConfigLoader::new(&self.cwd, &self.home).load().expect("config loads")
        }

        fn marked(&self) -> bool {
            self.folder.join(DONE_MARKER).exists()
        }

        /// Every file in the state folder but the lock, sorted; empty when the
        /// folder is absent.
        fn state_files(&self) -> Vec<String> {
            let Ok(entries) = fs::read_dir(&self.folder) else {
                return Vec::new();
            };
            let mut names = entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name != LOCK_FILE)
                .collect::<Vec<_>>();
            names.sort();
            names
        }

        /// The files in `.zo/` beside the settings file.
        fn zo_folder_files(&self) -> Vec<String> {
            let mut names = fs::read_dir(self.cwd.join(".zo"))
                .expect(".zo")
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            names.sort();
            names
        }

        /// The project's lock, held by another zo until it is dropped.
        fn hold_lock(&self) -> fs::File {
            fs::create_dir_all(&self.folder).expect("state folder");
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(self.folder.join(LOCK_FILE))
                .expect("lock file");
            lock.lock().expect("held");
            lock
        }
    }

    /// How a session built from `config` answers `command` with nobody to ask:
    /// `Allow` from a rule, `Deny` where it would have asked.
    fn decide(config: &RuntimeConfig, command: &str) -> PermissionOutcome {
        PermissionPolicy::new(PermissionMode::WorkspaceWrite)
            .with_tool_requirement("bash", PermissionMode::DangerFullAccess)
            .with_permission_rules(config.permission_rules())
            .authorize("bash", &serde_json::json!({ "command": command }).to_string(), None)
    }

    fn asks(outcome: &PermissionOutcome) -> bool {
        matches!(outcome, PermissionOutcome::Deny { .. })
    }

    fn object(contents: &str) -> std::collections::BTreeMap<String, JsonValue> {
        match JsonValue::parse(contents).expect("json") {
            JsonValue::Object(object) => object,
            other => panic!("not an object: {other:?}"),
        }
    }

    fn list(contents: &str, key: &str) -> Vec<String> {
        let root = object(contents);
        let Some(JsonValue::Object(permissions)) = root.get("permissions") else {
            panic!("no permissions in {contents}");
        };
        let Some(JsonValue::Array(items)) = permissions.get(key) else {
            panic!("no permissions.{key} in {contents}");
        };
        items
            .iter()
            .map(|item| match item {
                JsonValue::String(rule) => rule.clone(),
                other => panic!("not a rule: {other:?}"),
            })
            .collect()
    }

    /// The broad rule stops allowing the wider command and keeps allowing the
    /// approved one, and the file read back by a later session says the same.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_broad_allow_rule_stops_allowing_the_wider_command_and_reads_back_the_same() {
        let _stable = overrides_stable();
        let project = Project::new(
            &format!(r#"{{"permissions":{{"allow":["bash(git status:*)","{BROAD}"]}}}}"#),
            false,
        );
        let before = project.load();
        assert_eq!(decide(&before, WIDER), PermissionOutcome::Allow, "the old rule is broad");

        let outcome = project.narrow();
        let AllowAlwaysNarrowing::Narrowed { rules, dropped, .. } = &outcome else {
            panic!("expected a narrowing, got {outcome:?}");
        };
        assert_eq!(
            rules,
            &[NarrowedRule { before: BROAD.to_string(), after: NARROWED.to_string() }]
        );
        assert!(dropped.is_empty(), "{dropped:?}");
        assert_eq!(list(&project.settings(), "allow"), ["bash(git status:*)", NARROWED]);

        let after = project.load();
        assert!(asks(&decide(&after, WIDER)), "the wider command asks again");
        assert_eq!(decide(&after, APPROVED), PermissionOutcome::Allow);
        assert_eq!(decide(&after, "git status --short"), PermissionOutcome::Allow);
    }

    /// Only the broad allow rule moves: the person's own shapes, deny and ask
    /// rules, other keys and the order of every list stay as they were, and
    /// the file is written on one line, as `persist_allow_always_rules`
    /// writes it.
    #[test]
    fn only_the_broad_allow_rule_moves_in_an_allow_document() {
        let path = Path::new("/project/.zo/settings.local.json");
        let before = format!(
            r#"{{"model":"opus","permissions":{{"allow":["bash(git status:*)","{BROAD}","bash(*)","bash","bash(ls [*].rs)"],"deny":["bash(rm *)"],"ask":["bash(git push *)"],"defaultMode":"default"}},"env":{{"A":"1"}}}}"#
        );
        let (rendered, rules, dropped) = narrow_allow_always_document(path, &before)
            .expect("narrows")
            .expect("one broad rule");
        assert_eq!(rules.len(), 1);
        assert!(dropped.is_empty());
        assert!(!rendered.contains('\n'), "one line, as zo saves it: {rendered}");
        assert_eq!(rendered, JsonValue::Object(object(&rendered)).render());
        assert_eq!(
            list(&rendered, "allow"),
            ["bash(git status:*)", NARROWED, "bash(*)", "bash", "bash(ls [*].rs)"]
        );
        assert_eq!(list(&rendered, "deny"), ["bash(rm *)"]);
        assert_eq!(list(&rendered, "ask"), ["bash(git push *)"]);
        let (mut was, mut now) = (object(&before), object(&rendered));
        for root in [&mut was, &mut now] {
            let Some(JsonValue::Object(permissions)) = root.get_mut("permissions") else {
                panic!("permissions");
            };
            permissions.remove("allow");
        }
        assert_eq!(was, now, "every other key is as it was");
    }

    /// A class rule that never met its own text is taken out where it stood;
    /// the rest keep their order.
    #[test]
    fn a_rule_that_never_matched_its_text_is_taken_out_of_an_allow_document() {
        let path = Path::new("/project/.zo/settings.local.json");
        let before = format!(
            r#"{{"permissions":{{"allow":["bash(git status:*)","{NEVER_MATCHED}","{BROAD}","edit_file(app/[id]/page.tsx)"]}}}}"#
        );
        let (rendered, rules, dropped) = narrow_allow_always_document(path, &before)
            .expect("narrows")
            .expect("rules to change");
        assert_eq!(list(&rendered, "allow"), ["bash(git status:*)", NARROWED]);
        assert_eq!(rules.len(), 1);
        assert_eq!(dropped, [NEVER_MATCHED, "edit_file(app/[id]/page.tsx)"]);

        let only_dropped = format!(r#"{{"permissions":{{"allow":["{NEVER_MATCHED}"]}}}}"#);
        let (rendered, rules, dropped) = narrow_allow_always_document(path, &only_dropped)
            .expect("narrows")
            .expect("a rule taken out is a change");
        assert!(list(&rendered, "allow").is_empty() && rules.is_empty());
        assert_eq!(dropped, [NEVER_MATCHED]);
    }

    /// The ordered form narrows or takes out only `…=allow` specs, where they
    /// stand; a deny spec with the same class stays.
    #[test]
    fn only_the_broad_allow_spec_moves_in_an_ordered_document() {
        let path = Path::new("/project/.zo/settings.local.json");
        let before = format!(
            r#"{{"permissions":{{"rules":["bash(rm *)=deny","{BROAD}=allow","{NEVER_MATCHED}=allow","{NEVER_MATCHED}=deny","bash(git status:*)=allow","bash(*)=ask","bash(ls [*].rs)=allow"]}},"model":"opus"}}"#
        );
        let (rendered, _, dropped) = narrow_allow_always_document(path, &before)
            .expect("narrows")
            .expect("one broad rule");
        assert_eq!(
            list(&rendered, "rules"),
            [
                "bash(rm *)=deny".to_string(),
                format!("{NARROWED}=allow"),
                format!("{NEVER_MATCHED}=deny"),
                "bash(git status:*)=allow".to_string(),
                "bash(*)=ask".to_string(),
                "bash(ls [*].rs)=allow".to_string(),
            ]
        );
        assert_eq!(dropped, [format!("{NEVER_MATCHED}=allow")]);
        assert_eq!(object(&rendered).get("model"), object(&before).get("model"));
    }

    /// In the ordered form, where the last match wins, the class rule's text
    /// is refused by the earlier deny before narrowing and after it: taking
    /// the rule out allows nothing that was refused.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn an_ordered_class_rule_is_taken_out_and_its_text_stays_refused() {
        let _stable = overrides_stable();
        let project = Project::new(
            &format!(r#"{{"permissions":{{"rules":["bash(rm *)=deny","{NEVER_MATCHED}=allow"]}}}}"#),
            false,
        );
        assert!(asks(&decide(&project.load(), "rm [ab]")), "refused before");
        let outcome = project.narrow();
        let AllowAlwaysNarrowing::Narrowed { rules, dropped, .. } = &outcome else {
            panic!("expected a change, got {outcome:?}");
        };
        assert!(rules.is_empty(), "{rules:?}");
        assert_eq!(dropped, &[format!("{NEVER_MATCHED}=allow")]);
        assert_eq!(list(&project.settings(), "rules"), ["bash(rm *)=deny"]);
        assert!(asks(&decide(&project.load(), "rm [ab]")), "refused after");
        assert!(project.marked() && project.folder.join(NOTICE_FILE).exists());
    }

    /// A file holding only narrow rules is not written, and the project is
    /// marked: the literal form is never wrapped a second time.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn an_already_narrow_rule_stays_byte_for_byte() {
        let before = r#"{ "permissions": { "allow": ["bash(ls [*].rs)", "bash(git status:*)"] } }"#;
        let project = Project::new(before, false);
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::NothingToNarrow);
        assert_eq!(project.settings(), before);
        assert!(project.marked());
        assert_eq!(project.state_files(), [DONE_MARKER]);
    }

    /// Once marked, a project is not read again: a second run changes no byte,
    /// and a glob the person writes by hand afterwards stays a glob.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_second_run_changes_nothing_and_a_glob_written_later_stays_a_glob() {
        let _stable = overrides_stable();
        let project = Project::new(&format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#), false);
        assert!(matches!(project.narrow(), AllowAlwaysNarrowing::Narrowed { .. }));
        let narrowed = project.settings();
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::AlreadyDone);
        assert_eq!(project.settings(), narrowed);

        let by_hand = format!(r#"{{"permissions":{{"allow":["{NARROWED}","bash(npm run *)"]}}}}"#);
        fs::write(project.settings_path(), &by_hand).expect("person edits");
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::AlreadyDone);
        assert_eq!(project.settings(), by_hand);
        assert_eq!(decide(&project.load(), "npm run build"), PermissionOutcome::Allow);
    }

    /// A committed file is not the person's trusted one: nothing is written,
    /// kept or marked, so a later session tries again.
    #[test]
    fn an_untrusted_file_is_left_as_it_was_and_unmarked() {
        let before = format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#);
        let project = Project::new(&before, true);
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::Skipped);
        assert_eq!(project.settings(), before);
        assert!(project.state_files().is_empty(), "{:?}", project.state_files());
    }

    /// An empty file — a write caught half way reads as no settings — is
    /// passed by unmarked, so the next session looks again.
    #[test]
    fn an_empty_file_leaves_no_marker() {
        for empty in ["", " \n\t"] {
            let project = Project::new(empty, false);
            let outcome =
                narrow_with(&project.cwd, &project.folder, LOCK_WAIT, &mut || Some(empty.to_string()), &mut |_| {
                    panic!("no write")
                });
            assert_eq!(outcome, AllowAlwaysNarrowing::Skipped, "{empty:?}");
            assert!(project.state_files().is_empty(), "{:?}", project.state_files());
            assert_eq!(project.settings(), empty);
        }
    }

    /// A file that changes between the read and the write is not written over.
    #[test]
    fn a_file_that_changes_while_it_is_read_is_left_as_it_was_and_unmarked() {
        let project = Project::new("{}", false);
        let before = format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#);
        let mut reads = [before.clone(), format!("{before} ")].into_iter();
        let mut writes = 0;
        let outcome =
            narrow_with(&project.cwd, &project.folder, LOCK_WAIT, &mut || reads.next(), &mut |_| {
                writes += 1;
                Ok(())
            });
        assert_eq!(outcome, AllowAlwaysNarrowing::Skipped);
        assert_eq!(writes, 0, "nothing is written over a newer edit");
        assert!(project.state_files().is_empty(), "{:?}", project.state_files());
    }

    /// The real write failing part way — its temporary file written, then
    /// gone before the rename — leaves the settings file on disk byte for byte
    /// as it was, no temporary file in `.zo/`, no kept copy and no marker.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_write_that_fails_part_way_leaves_the_file_on_disk_as_it_was() {
        let before = format!(r#"{{ "permissions": {{ "allow": ["{BROAD}"], "deny": ["bash(rm *)"] }} }}"#);
        let project = Project::new(&before, false);
        let (cwd, zo) = (project.cwd.clone(), project.cwd.join(".zo"));
        let path = project.settings_path();
        let mut failed = false;
        let outcome = narrow_with(
            &project.cwd,
            &project.folder,
            LOCK_WAIT,
            &mut || trusted_uncommitted_zo_file_snapshot(&cwd, &path, SETTINGS_FILE),
            &mut |bytes| {
                crate::secure_fs::write_atomic_owner_only_observed(
                    &cwd,
                    &settings_relative(),
                    bytes,
                    crate::secure_fs::Durability::Flushed,
                    crate::secure_fs::TempName::Target,
                    &mut |step| {
                        if step == "temp_written" {
                            // The disk takes the bytes, then loses them.
                            for entry in fs::read_dir(&zo).expect(".zo").filter_map(Result::ok) {
                                if entry.file_name().to_string_lossy().starts_with(".zo-state-") {
                                    fs::remove_file(entry.path()).expect("lose the temporary file");
                                    failed = true;
                                }
                            }
                        }
                    },
                )
            },
        );
        assert!(failed, "the write reached its temporary file");
        assert_eq!(outcome, AllowAlwaysNarrowing::Skipped);
        assert_eq!(fs::read(project.settings_path()).expect("settings"), before.as_bytes());
        assert_eq!(project.zo_folder_files(), [SETTINGS_FILE], "nothing is left in .zo/");
        assert!(project.state_files().is_empty(), "no copy, no marker: {:?}", project.state_files());
    }

    /// A temporary file a killed narrowing left in `.zo/` — however new — is
    /// gone once a later narrowing succeeds, so nothing but the settings file
    /// is left there after the project is marked.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_temporary_file_a_killed_narrowing_left_goes_with_the_next_narrowing() {
        let project = Project::new(&format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#), false);
        let left = project.cwd.join(".zo").join(format!(".zo-state-{SETTINGS_FILE}-99999-0.tmp"));
        fs::write(&left, format!(r#"{{"permissions":{{"allow":["{NARROWED}"]}}}}"#)).expect("left behind");
        assert!(matches!(project.narrow(), AllowAlwaysNarrowing::Narrowed { .. }));
        assert!(project.marked());
        assert_eq!(project.zo_folder_files(), [SETTINGS_FILE], "nothing is left in .zo/");
    }

    /// A second zo starting while the first holds the lock waits, then finds
    /// the marker the first left and reads the file without touching it.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_second_zo_waits_for_the_lock_and_then_only_reads() {
        let before = format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#);
        let project = Project::new(&before, false);
        let lock = project.hold_lock();
        std::thread::scope(|scope| {
            let waiter = scope.spawn(|| narrow_in(&project.cwd, &project.folder, Duration::from_secs(30)));
            std::thread::sleep(Duration::from_millis(300));
            assert!(!waiter.is_finished(), "the second zo waits for the lock");
            assert_eq!(project.settings(), before);
            // The first zo finishes: its marker, then the lock goes.
            fs::write(project.folder.join(DONE_MARKER), b"").expect("marker");
            drop(lock);
            assert_eq!(waiter.join().expect("waiter"), AllowAlwaysNarrowing::AlreadyDone);
        });
        assert_eq!(project.settings(), before, "the waiter read and wrote nothing");
    }

    /// A lock still held at the deadline: the session reads the file as it is,
    /// unnarrowed and unmarked, and the next session tries again.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn a_lock_held_past_the_wait_reads_without_narrowing() {
        let _stable = overrides_stable();
        let before = format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#);
        let project = Project::new(&before, false);
        let lock = project.hold_lock();
        assert_eq!(
            narrow_in(&project.cwd, &project.folder, Duration::from_millis(100)),
            AllowAlwaysNarrowing::Busy
        );
        assert_eq!(project.settings(), before);
        assert!(project.state_files().is_empty(), "{:?}", project.state_files());
        assert_eq!(decide(&project.load(), WIDER), PermissionOutcome::Allow, "read as it is");
        drop(lock);
        assert!(matches!(project.narrow(), AllowAlwaysNarrowing::Narrowed { .. }), "tried again");
    }

    /// The file as it was is kept byte for byte, readable by the person alone,
    /// and outside the repository; the narrowed file is the person's alone too.
    #[test]
    #[cfg(unix)]
    fn the_original_is_kept_byte_for_byte_and_private() {
        use std::os::unix::fs::PermissionsExt as _;
        let before = format!("{{\n  \"permissions\" : {{ \"allow\" : [ \"{BROAD}\" ] }}\n}}\n");
        let project = Project::new(&before, false);
        let AllowAlwaysNarrowing::Narrowed { backup, .. } = project.narrow() else {
            panic!("expected a narrowing");
        };
        assert!(backup.starts_with(&project.folder) && !backup.starts_with(&project.cwd));
        assert_eq!(fs::read(&backup).expect("kept"), before.as_bytes());
        let mode = |path: &Path| fs::metadata(path).expect("mode").permissions().mode() & 0o777;
        assert_eq!(mode(&backup), 0o600);
        assert_eq!(mode(&project.folder), 0o700);
        assert_eq!(mode(&project.settings_path()), 0o600);
        assert_eq!(project.zo_folder_files(), [SETTINGS_FILE], "no temporary file is left");
    }

    /// A project with no settings file holds no saved rule; it is marked so a
    /// glob the person writes into a new file later stays theirs.
    #[test]
    fn a_project_without_the_file_is_marked_and_nothing_is_created() {
        let project = Project::new("{}", false);
        fs::remove_file(project.settings_path()).expect("remove settings");
        let outcome =
            narrow_with(&project.cwd, &project.folder, LOCK_WAIT, &mut || panic!("no read"), &mut |_| {
                panic!("no write")
            });
        assert_eq!(outcome, AllowAlwaysNarrowing::NothingToNarrow);
        assert!(project.marked() && !project.settings_path().exists());
    }

    /// The narrowing leaves a notice that waits — through any number of runs
    /// nobody watches — for the first session that says it, and is said once.
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn the_notice_waits_for_the_first_watched_session_and_is_said_once() {
        let project = Project::new(
            &format!(r#"{{"permissions":{{"allow":["{BROAD}","{NEVER_MATCHED}"]}}}}"#),
            false,
        );
        let AllowAlwaysNarrowing::Narrowed { backup, .. } = project.narrow() else {
            panic!("expected a narrowing");
        };
        // An unattended run later: the project is done, the notice still waits.
        assert_eq!(project.narrow(), AllowAlwaysNarrowing::AlreadyDone);
        assert!(project.folder.join(NOTICE_FILE).exists());

        let said = take_notice_in(&project.folder).expect("the notice");
        assert!(said.contains("narrowed 1 \"always allow\" rule "), "{said}");
        assert!(said.contains(&format!("`{BROAD}`")), "{said}");
        assert!(said.contains("removed 1 \"always allow\" rule "), "{said}");
        assert!(said.contains(&format!("`{NEVER_MATCHED}`")), "{said}");
        assert!(said.contains(&backup.display().to_string()), "{said}");
        assert!(said.contains(&project.settings_path().display().to_string()), "{said}");
        assert!(said.contains("write it again"), "{said}");
        assert_eq!(take_notice_in(&project.folder), None, "said once");
    }

    /// The notice tells the rules narrowed from the rules taken out, and says
    /// "still allowed" only of the narrowed ones.
    #[test]
    fn the_notice_tells_rules_narrowed_from_rules_taken_out() {
        let narrowed = narrow_notice_text("/p/s.json", &[BROAD.to_string()], &[], "/s/kept");
        assert!(narrowed.contains("The approved command is still allowed"), "{narrowed}");
        assert!(!narrowed.contains("removed"), "{narrowed}");

        let removed = narrow_notice_text("/p/s.json", &[], &[NEVER_MATCHED.to_string()], "/s/kept");
        assert!(removed.starts_with("zo removed 1 \"always allow\" rule in /p/s.json"), "{removed}");
        assert!(removed.contains("never matched the command approved"), "{removed}");
        assert!(!removed.contains("still allowed"), "{removed}");
        assert!(!removed.contains("narrowed"), "{removed}");
        assert!(removed.contains("kept at /s/kept"), "{removed}");
        assert!(removed.contains("or is refused where an earlier deny rule matches"), "{removed}");

        let two = [NEVER_MATCHED.to_string(), "bash(ls [ab])".to_string()];
        let removed = narrow_notice_text("/p/s.json", &[], &two, "/s/kept");
        assert!(removed.contains("removed 2 \"always allow\" rules"), "{removed}");
        assert!(removed.contains("so they never allowed those commands"), "{removed}");
        assert!(removed.contains("those commands ask again"), "{removed}");
        assert!(removed.contains("now saves them as approved"), "{removed}");
        assert!(!removed.contains("  "), "no doubled space: {removed}");
    }

    /// The notice names five rules of each kind and counts the rest.
    #[test]
    fn the_notice_names_five_rules_and_counts_the_rest() {
        let rules = (1..=7).map(|n| format!("bash(make t{n} *)")).collect::<Vec<_>>();
        let said = narrow_notice_text("/p/.zo/settings.local.json", &rules, &rules, "/s/kept");
        assert!(said.starts_with("zo narrowed 7 \"always allow\" rules in /p/.zo/settings.local.json"), "{said}");
        assert_eq!(said.matches("`bash(make t5 *)` and 2 more.").count(), 2, "{said}");
        assert!(!said.contains("t6"), "{said}");
    }

    /// A saved rule cannot write to the terminal through the notice: a control
    /// character is shown as `\u{..}`, and a long rule is cut at 80
    /// characters — counted as characters, so Korean text is cut whole.
    #[test]
    fn the_notice_shows_a_rule_without_its_control_characters_and_cut_short() {
        let title = "bash(printf x\u{1b}]0;hi\u{7} *)".to_string();
        let long = format!("bash(echo {} *)", "가".repeat(100));
        let said = narrow_notice_text("/p/s.json", &[title, long], &[], "/s/kept\u{1b}[2J");
        assert!(!said.chars().any(char::is_control), "{said:?}");
        assert!(said.contains(r"`bash(printf x\u{1b}]0;hi\u{7} *)`"), "{said}");
        assert!(said.contains(r"/s/kept\u{1b}[2J"), "{said}");
        let cut = format!("`bash(echo {}…`", "가".repeat(80 - "bash(echo ".chars().count()));
        assert!(said.contains(&cut), "{said}");
    }

    /// The session's own load narrows first, so the policy that very session
    /// builds is narrow (and the state folder is the project's zo state).
    #[test]
    #[cfg_attr(windows, ignore = "a trusted Local .zo/settings.local.json is Unix-only: its provenance gate needs Unix filesystem identities, so on Windows the file fails closed and nothing is narrowed")]
    fn the_session_that_narrows_builds_a_narrow_policy() {
        let _stable = overrides_stable();
        let _env = crate::test_env_lock();
        let project = Project::new(&format!(r#"{{"permissions":{{"allow":["{BROAD}"]}}}}"#), false);
        let state_root = project.cwd.parent().expect("root").join("zo-state");
        let prior = std::env::var_os(core_types::paths::ZO_STATE_DIR_ENV);
        std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, &state_root);

        let config = ConfigLoader::new(&project.cwd, &project.home).load_for_session();
        let folder = crate::config::zo_project_state_dir(&project.cwd).join(STATE_FOLDER);

        match prior {
            Some(value) => std::env::set_var(core_types::paths::ZO_STATE_DIR_ENV, value),
            None => std::env::remove_var(core_types::paths::ZO_STATE_DIR_ENV),
        }
        let config = config.expect("config loads");
        assert!(asks(&decide(&config, WIDER)), "the session that narrowed asks again");
        assert_eq!(decide(&config, APPROVED), PermissionOutcome::Allow);
        assert!(folder.starts_with(&state_root) && folder.join(DONE_MARKER).exists());
        assert!(folder.join(NOTICE_FILE).exists());
    }
}
