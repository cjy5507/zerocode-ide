//! The person's record of the project-local control files zo wrote.
//!
//! A folder's `.zo/settings.local.json` and `.zo/trusted-mcp-servers.json` are
//! read as the person's own documents (their hooks, env, "always allow" rules
//! and MCP servers apply unasked) only when their bytes are what zo last wrote
//! at that exact path. zo keeps that record in the person's config home, where
//! no folder can put one: `local-settings-record.json`, the canonical path of
//! each file and the SHA-256 of the bytes written there.
//!
//! What git says about the file is no evidence: a downloaded bundle can carry a
//! valid `.git` with the file untracked, a template's `.gitignore` keeps it
//! untracked after `git init`, a folder unpacked inside the person's repository
//! is untracked there, and a model's file tools can write the file in place
//! (152's remaining roads). Every one of them leaves bytes zo did not write.
//!
//! The record names the path, not only the bytes: the same bytes in another
//! folder are not the person's there — a hook's `./scripts/run.sh` runs that
//! folder's script.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::Digest as _;

/// The record's file name in the person's config home.
pub const RECORD_FILE: &str = "local-settings-record.json";

/// The project-local control files the record speaks for.
pub const RECORDED_FILES: [&str; 2] = [SETTINGS_FILE, TRUSTED_FILE];

const SETTINGS_FILE: &str = "settings.local.json";
const TRUSTED_FILE: &str = "trusted-mcp-servers.json";

/// The settings a `settings.local.json` may hold and still be taken as the
/// person's without a record: the ones a repository's reading of the file
/// keeps whole, none of which reaches outside the session — the model and how
/// hard it thinks, how answers read, the memory switches. Every other key
/// (hooks, env, MCP servers, plugins, providers, `smart`, `cache`, an
/// `enableAllProject…` opt-in, a key zo learns later) keeps a file from being
/// taken without the person's word. A key missed here costs a grant kept for
/// the session and a notice, never a widening (153).
const PLAIN_SETTINGS: [&str; 8] = [
    "model",
    "reasoningEffort",
    "alwaysThinkingEnabled",
    "outputStyle",
    "language",
    "autoMemoryEnabled",
    "autoDreamEnabled",
    "recallHintEnabled",
];

/// The permission modes a repository's own document may pin (the loader
/// strips only `dontAsk` and `danger-full-access` from one): read-only and
/// workspace-write. A local file saying one of these grants nothing a
/// repository could not.
const MODES_A_REPOSITORY_MAY_PIN: [&str; 6] = [
    "default",
    "plan",
    "read-only",
    "acceptEdits",
    "auto",
    "workspace-write",
];

/// Most entries kept; past this the oldest go first.
const MAX_ENTRIES: usize = 1000;

const VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    sha256: String,
    at: u64,
}

/// Whether `bytes`, read from the file whose canonical path is `real`, are what
/// zo last wrote there by the person's record in the config home `home`. False
/// where the record cannot be trusted (another account owns or can write it) or this
/// system cannot tell (no Unix ownership).
#[must_use]
pub fn recorded(home: &Path, real: &Path, bytes: &[u8]) -> bool {
    #[cfg(test)]
    if treated_as_recorded(real) {
        return true;
    }
    if !cfg!(unix) {
        return false;
    }
    let Some(entries) = read_entries(home) else {
        return false;
    };
    entries
        .get(&key(real))
        .is_some_and(|entry| entry.sha256 == sha256_hex(bytes))
}

/// Remember that zo wrote `bytes` to the file whose canonical path is `real`.
/// Entries whose file is gone are dropped, and past a thousand the oldest.
/// The record is read and written under its lock: sessions ending together (a
/// window restart ends every pane) each keep their line.
///
/// # Errors
///
/// When the record cannot be written. A record that cannot be trusted is not
/// written over: it is reported, and the file reads as the repository's.
pub fn remember(home: &Path, real: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let _lock = crate::file_ops::SettingsFileLock::acquire(&record_path(home))?;
    let mut entries = read_entries(home).ok_or_else(|| untrusted_record(home))?;
    entries.retain(|path, _| Path::new(path).is_file());
    entries.insert(
        key(real),
        Entry {
            sha256: sha256_hex(bytes),
            at: now_seconds(),
        },
    );
    while entries.len() > MAX_ENTRIES {
        let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, entry)| entry.at)
            .map(|(path, _)| path.clone())
        else {
            break;
        };
        entries.remove(&oldest);
    }
    write_entries(home, &entries)
}

/// Take the file whose canonical path is `real` out of the record.
///
/// # Errors
///
/// As [`remember`].
pub fn forget(home: &Path, real: &Path) -> std::io::Result<bool> {
    let _lock = crate::file_ops::SettingsFileLock::acquire(&record_path(home))?;
    let mut entries = read_entries(home).ok_or_else(|| untrusted_record(home))?;
    let removed = entries.remove(&key(real)).is_some();
    if removed {
        write_entries(home, &entries)?;
    }
    Ok(removed)
}

/// Whether `path` is one of the local control files the record speaks for:
/// `<folder>/.zo/<one of RECORDED_FILES>`, the person's home folder's own
/// (`~/.zo/…`, inside the config home) as much as any other folder's.
#[must_use]
pub fn is_recorded_kind(path: &Path) -> bool {
    local_file_name(path).is_some()
}

/// The name of the local file `path` is (`<folder>/.zo/<name>`), by its name
/// alone.
fn local_file_name(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?;
    let name = RECORDED_FILES.into_iter().find(|recorded| *recorded == name)?;
    let in_dot_zo = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|parent| parent == ".zo");
    in_dot_zo.then_some(name)
}

/// The folder whose `.zo` holds the local file `path`.
fn folder_of(path: &Path) -> &Path {
    path.parent()
        .and_then(Path::parent)
        .filter(|folder| !folder.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Whether the `.zo` holding `path` is the folder's own: a real folder at
/// exactly `<folder>/.zo`, not a link to another folder — the loader reads a
/// local file only there. A `.zo` not made yet is: zo makes a real one.
fn dot_zo_in_place(path: &Path) -> bool {
    let Some(dot_zo) = path.parent() else {
        return false;
    };
    match std::fs::symlink_metadata(dot_zo) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Ok(metadata) if metadata.file_type().is_dir() => dot_zo
            .canonicalize()
            .ok()
            .zip(folder_of(path).canonicalize().ok())
            .is_some_and(|(real, base)| real == base.join(".zo")),
        _ => false,
    }
}

/// Whether zo may write the local file `path` (any other path, it may): the
/// folder's own `.zo` holds it, and nothing stands there, or the bytes there
/// are what zo last wrote, or they carry no authority
/// ([`carries_no_authority`]). Writing over a file the folder came with would
/// make what it grants the person's. The person's home folder's files, inside
/// the config home, are no exception: a file unpacked there is not one zo
/// wrote either.
///
/// Through a `.zo` that links to another folder (a neighbouring project's, or
/// the config home) zo would make and record a file the loader never reads
/// here, and it would apply there.
#[must_use]
pub fn may_write(home: &Path, path: &Path) -> bool {
    let Some(name) = local_file_name(path) else {
        return true;
    };
    if !dot_zo_in_place(path) {
        return false;
    }
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Ok(metadata) if metadata.file_type().is_file() => {
            let (Ok(real), Ok(bytes)) = (path.canonicalize(), std::fs::read(path)) else {
                return false;
            };
            recorded(home, &real, &bytes) || carries_no_authority(name, &bytes)
        }
        _ => false,
    }
}

/// Whether a local file's bytes grant nothing a repository's reading of the
/// file would not: a `settings.local.json` that is a JSON object holding only
/// [`PLAIN_SETTINGS`], a `permissions` of `deny`, `ask` and a mode a
/// repository may pin, and such a `permissionMode`. Read as the repository's,
/// such a file loses nothing, so zo may take it as the person's when it writes
/// into it — a file an older zo wrote, or one holding the model alone. A
/// `trusted-mcp-servers.json` never qualifies: every name in it is a server
/// let run.
#[must_use]
pub fn carries_no_authority(name: &str, bytes: &[u8]) -> bool {
    if name != SETTINGS_FILE {
        return false;
    }
    // Judged as the loader reads it: through its own parser, so a key given
    // twice counts as the one that applies (the last), whatever another
    // parser would make of it.
    let Some(loaded) = std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| crate::json::JsonValue::parse(text).ok())
    else {
        return false;
    };
    match serde_json::from_str::<serde_json::Value>(&loaded.render()) {
        Ok(serde_json::Value::Object(document)) => authority(&document).is_empty(),
        _ => false,
    }
}

/// Whether `after` grants something `before` did not: an entry of its
/// [`authority`] that `before`'s does not hold the same. A change of plain
/// settings, or one that narrows (a mode set to `plan`, a grant taken out),
/// widens nothing.
#[must_use]
pub fn widens(
    before: &serde_json::Map<String, serde_json::Value>,
    after: &serde_json::Map<String, serde_json::Value>,
) -> bool {
    let had = authority(before);
    authority(after)
        .iter()
        .any(|(place, said)| had.get(place) != Some(said))
}

/// What a local settings document grants beyond [`PLAIN_SETTINGS`] and the
/// modes and restrictions a repository may set, keyed by where each stands
/// (`permissions.allow`, `hooks`, …). Empty for a document that carries no
/// authority.
fn authority(
    document: &serde_json::Map<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    let pinnable = |said: &serde_json::Value| {
        said.as_str()
            .is_some_and(|mode| MODES_A_REPOSITORY_MAY_PIN.contains(&mode))
    };
    let mut granted = BTreeMap::new();
    for (key, said) in document {
        match (key.as_str(), said) {
            (plain, _) if PLAIN_SETTINGS.contains(&plain) => {}
            ("permissionMode", mode) if pinnable(mode) => {}
            ("permissions", serde_json::Value::Object(rules)) => {
                for (rule, said) in rules {
                    let restricts = matches!(rule.as_str(), "deny" | "ask")
                        || (rule == "defaultMode" && pinnable(said));
                    if !restricts {
                        granted.insert(format!("permissions.{rule}"), said.clone());
                    }
                }
            }
            _ => {
                granted.insert(key.clone(), said.clone());
            }
        }
    }
    granted
}

/// Write `bytes` as the local file `path` (`<folder>/.zo/<name>`) through the
/// folder's own `.zo`, made where none stands: the `.zo` and the file are
/// opened without following a link, and the bytes go to a temporary file
/// renamed over the name. A `.zo` swapped for a link after [`may_write`]
/// looked is refused, not written through.
///
/// # Errors
///
/// When `path` is not a local file's, its `.zo` or the file is a link, or the
/// write fails.
pub fn write_local_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let Some(name) = local_file_name(path) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is not a folder's .zo local file", path.display()),
        ));
    };
    let folder = folder_of(path);
    std::fs::create_dir(folder.join(".zo")).or_else(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Ok(())
        } else {
            Err(error)
        }
    })?;
    crate::secure_fs::write_atomic_owner_only_named_temp(
        folder,
        &Path::new(".zo").join(name),
        bytes,
        crate::secure_fs::Durability::Flushed,
    )
}

/// After zo wrote `bytes` to the local file `path`, remember them — the
/// person's home folder's `~/.zo/…` as any other: the loader reads it as the
/// person's only by the record. The path is named from the folder, not by
/// following `.zo` again. A failure is reported once on stderr and answered
/// `false`: the write stands, and the file reads as the repository's until zo
/// writes it again. Any other path needs no record (`true`).
pub fn wrote(home: &Path, path: &Path, bytes: &[u8]) -> bool {
    let Some(name) = local_file_name(path) else {
        return true;
    };
    let result = folder_of(path)
        .canonicalize()
        .and_then(|folder| remember(home, &folder.join(".zo").join(name), bytes));
    match result {
        Ok(()) => true,
        Err(error) => {
            eprintln!(
                "[zo] warning: wrote {} but could not record it as yours: {error}",
                path.display()
            );
            false
        }
    }
}

fn record_path(home: &Path) -> PathBuf {
    home.join(RECORD_FILE)
}

fn key(real: &Path) -> String {
    real.to_string_lossy().into_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn untrusted_record(home: &Path) -> std::io::Error {
    let why = if cfg!(unix) {
        format!(
            "{} is not yours alone (another account owns it or can write it), so zo neither \
             reads nor writes it",
            record_path(home).display()
        )
    } else {
        "zo keeps no record of local files on this system: it trusts one by Unix ownership"
            .to_string()
    };
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, why)
}

/// Whether the record and the home holding it are the person's alone: owned by
/// the home's owner, a regular file, and writable by no group or other account.
/// The home's owner stands for the person — Rust's standard library has no
/// effective user id.
#[cfg(unix)]
fn record_is_the_persons(home: &Path, record: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(home_metadata) = std::fs::metadata(home) else {
        return false;
    };
    record.file_type().is_file()
        && record.uid() == home_metadata.uid()
        && record.mode() & 0o022 == 0
        && home_metadata.mode() & 0o022 == 0
}

#[cfg(not(unix))]
fn record_is_the_persons(_home: &Path, _record: &std::fs::Metadata) -> bool {
    false
}

/// Whether the config home is writable by no group or other account. A home
/// that does not exist yet is made when the record is written, with the
/// process's umask; one others can then write is refused at the next read.
#[cfg(unix)]
fn home_is_the_persons(home: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    match std::fs::metadata(home) {
        Ok(metadata) => metadata.mode() & 0o022 == 0,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(not(unix))]
fn home_is_the_persons(_home: &Path) -> bool {
    false
}

/// The record's entries, or `None` when it cannot be trusted or read. An
/// absent record is an empty one.
fn read_entries(home: &Path) -> Option<BTreeMap<String, Entry>> {
    let path = record_path(home);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        // No record yet is an empty one — in a home that is the person's
        // alone; a record written into a home others can write could be
        // replaced by them.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return home_is_the_persons(home).then(BTreeMap::new);
        }
        Err(_) => return None,
    };
    if !record_is_the_persons(home, &metadata) {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    if value.get("version").and_then(serde_json::Value::as_u64) != Some(VERSION) {
        return None;
    }
    let files = value.get("files")?.as_object()?;
    let mut entries = BTreeMap::new();
    for (path, entry) in files {
        let sha256 = entry.get("sha256").and_then(serde_json::Value::as_str)?;
        if sha256.len() != 64 || !sha256.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let at = entry
            .get("at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        entries.insert(
            path.clone(),
            Entry {
                sha256: sha256.to_ascii_lowercase(),
                at,
            },
        );
    }
    Some(entries)
}

fn write_entries(home: &Path, entries: &BTreeMap<String, Entry>) -> std::io::Result<()> {
    let files: serde_json::Map<String, serde_json::Value> = entries
        .iter()
        .map(|(path, entry)| {
            (
                path.clone(),
                serde_json::json!({ "sha256": entry.sha256, "at": entry.at }),
            )
        })
        .collect();
    let document = serde_json::json!({ "version": VERSION, "files": files });
    let bytes = serde_json::to_vec_pretty(&document).map_err(std::io::Error::other)?;
    std::fs::create_dir_all(home)?;
    crate::secure_fs::write_atomic_owner_only(home, Path::new(RECORD_FILE), &bytes)
}

#[cfg(test)]
thread_local! {
    static TREATED_AS_RECORDED: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Tests about something else take a folder as the person's: on this thread,
/// every file under `folder` reads as one zo wrote.
#[cfg(test)]
pub(crate) fn treat_as_recorded_on_this_thread(folder: &Path) {
    let folder = folder.canonicalize().unwrap_or_else(|_| folder.to_path_buf());
    TREATED_AS_RECORDED.with(|folders| folders.borrow_mut().push(folder));
}

#[cfg(test)]
fn treated_as_recorded(real: &Path) -> bool {
    TREATED_AS_RECORDED.with(|folders| folders.borrow().iter().any(|folder| real.starts_with(folder)))
}

// The record is trusted by Unix ownership: on Windows nothing is recorded,
// and these tests have nothing to show.
#[cfg(all(test, unix))]
mod tests;
