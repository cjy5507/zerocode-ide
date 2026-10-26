//! Secrets written as plain text in zo's settings file, moved into the
//! keychain by a person's press (t-76698; zo's half is t-76674).
//!
//! The window only stopped new plain-text secrets from being saved (t-75740,
//! t-75848): a key a person typed into `mcpServers.<server>.env.<NAME>` before
//! then stayed there. zo answers three doors, none of which ever prints a
//! value:
//!
//! - `zo settings secrets --json` names every such cell (file, path, name);
//! - `zo settings secrets check --name <NAME> --json` says whether `${NAME}`
//!   resolves (zo's environment, then the window's keychain item);
//! - `zo settings secrets move --file … --path … --name … --json` writes the
//!   cell as `${NAME}`, after it has found the reference resolves to the very
//!   bytes the cell holds, and keeps a backup.
//!
//! The window's half is between the first door and the other two: it reads
//! the one cell the listing named into memory, puts the value in the keychain
//! item zo reads (`dev.zerocode.key.<NAME>`, the item the TypeSafe key is kept
//! in), and drops it. The value goes to no log, no error, no answer to the
//! page and no argument of zo's; a reason zo gives that carries it is not
//! said either.
//!
//! A zo without the doors (an older one) answers the listing with an error,
//! and the settings say nothing.
//!
//! What a move trusts (t-76698 r2): the settings file is pinned when the move
//! starts — a plain file at the window's own path, never a link, and the
//! file and folder it is then. The cell is read through that pinned file
//! under the settings lock, the lock is kept to the keychain write, and the
//! file is looked at again just before the write: a link, a swapped file or
//! a changed cell writes nothing. The lock is let go before zo is asked, as zo
//! takes it. One move runs per settings file in the window, and the keychain
//! look and write are one step under the guard every key save takes
//! (`typesafe_settings::save_key_unless_another_at`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use zerocode_core::settings_file_lock::SettingsFileLock;
use zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX;

use crate::api_routers::RouterKeys;
use crate::typesafe_settings::KeptNot;

/// How long one door may take. zo reads one file and asks one keychain item;
/// a door that outlives this is killed and counts as no answer.
const ZO_WAIT: Duration = Duration::from_secs(10);

/// The longest reason the page is given from zo, in characters.
const WHY_CHARS: usize = 300;

/// The zo the window asks, and the environment it asks with: a zo pane's
/// account variables (read, never written), without the window's own
/// `ZEROCODE*` names — so it reads the settings file and the keychain a pane's
/// zo reads.
pub(crate) struct ZoDoor {
    bin: PathBuf,
    env: Vec<(String, String)>,
    wait: Duration,
}

impl ZoDoor {
    /// The installed zo, with a zo pane's account variables.
    pub(crate) fn of_this_window(config_root: &Path) -> Option<Self> {
        let home = dirs::home_dir()?;
        Some(Self::at(
            crate::zo_companion::zo_path_under(&home),
            crate::accounts::reading_env_for(config_root, "zo"),
        ))
    }

    pub(crate) fn at(bin: PathBuf, env: Vec<(String, String)>) -> Self {
        Self {
            bin,
            env,
            wait: ZO_WAIT,
        }
    }

    /// One door's exit code and the JSON it printed, whatever the code; `None`
    /// when zo is missing, is killed for outliving the wait, or prints no JSON.
    /// The wait bounds both the output and the process: a zo that closes its
    /// output and keeps running, or leaves a child holding the output open, is
    /// killed with its whole process group at the wait.
    fn ask(&self, args: &[&str]) -> Option<(i32, Value)> {
        let mut command = crate::proc::quiet_command_with_env(&self.bin, &self.env);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (key, _) in std::env::vars_os() {
            let key = key.to_string_lossy();
            if key.starts_with("ZEROCODE") {
                command.env_remove(&*key);
            }
        }
        #[cfg(unix)]
        crate::codex_queue::prepare_process_group(&mut command);
        let mut child = command.spawn().ok()?;
        let Some(mut stdout) = child.stdout.take() else {
            give_up(&mut child);
            return None;
        };
        let deadline = Instant::now() + self.wait;
        // Not joined: a reader still held open at the wait is let go, and ends
        // when the group's kill closes the pipe.
        let (said, heard) = mpsc::channel();
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = said.send(stdout.read_to_string(&mut text).map(|_| text).ok());
        });
        let mut printed = None;
        let mut status = None;
        while printed.is_none() || status.is_none() {
            if Instant::now() >= deadline {
                give_up(&mut child);
                return None;
            }
            if status.is_none() {
                match child.try_wait() {
                    Ok(ended) => status = ended,
                    Err(_) => {
                        give_up(&mut child);
                        return None;
                    }
                }
            }
            if printed.is_none() {
                match heard.recv_timeout(Duration::from_millis(20)) {
                    Ok(text) => printed = Some(text),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => printed = Some(None),
                }
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        Some((
            status?.code()?,
            serde_json::from_str(printed??.trim()).ok()?,
        ))
    }
}

/// A door that outlived its wait: zo and everything it started, killed.
fn give_up(child: &mut Child) {
    #[cfg(unix)]
    let _ = crate::codex_queue::signal_process_group(child.id(), libc::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();
}

/// One plain-text cell zo's listing named: the file as zo spelled it (and is
/// handed back), the keys down to the cell, and the name it is to refer to.
struct Literal {
    file: String,
    path: Vec<String>,
    name: String,
}

impl Literal {
    /// One row in the one shape the door has: an absolute file, the path
    /// `mcpServers/<server>/env/<NAME>` and the name that same `NAME`. A row
    /// of any other shape is not read as a cell to move.
    fn of(row: &Value) -> Option<Self> {
        let file = row.get("file")?.as_str()?;
        let path = row
            .get("path")?
            .as_array()?
            .iter()
            .map(|key| key.as_str().map(str::to_string))
            .collect::<Option<Vec<_>>>()?;
        let name = row.get("name")?.as_str()?;
        let [top, server, env, last] = path.as_slice() else {
            return None;
        };
        (Path::new(file).is_absolute()
            && top == "mcpServers"
            && !server.is_empty()
            && env == "env"
            && last == name
            && is_name(name))
        .then(|| Self {
            file: file.to_string(),
            path,
            name: name.to_string(),
        })
    }
}

/// A variable's name as `${NAME}` spells one: a letter or `_`, then letters,
/// digits and `_`. Anything else would name a keychain item zo never reads.
fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|rest| rest.is_ascii_alphanumeric() || rest == '_')
}

/// Whether zo's `file` is the person's settings file the window pinned: its
/// path, or a spelling of the path that stands on the very same file — never
/// a link to it. A cell elsewhere is not the window's to read.
fn is_settings_file(file: &str, settings: &Path, pinned: Pinned) -> bool {
    Path::new(file) == settings
        || std::fs::symlink_metadata(file)
            .is_ok_and(|meta| meta.file_type().is_file() && identity(&meta) == pinned.file)
}

/// The settings file a move trusts, as it stood when the move began: the file
/// (a plain one, not a link) and the folder holding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pinned {
    file: (u64, u64),
    folder: (u64, u64),
}

impl Pinned {
    /// The settings file at its path now; `None` when it is a link, not a
    /// file, or gone.
    fn at(settings: &Path) -> Option<Self> {
        let file = std::fs::symlink_metadata(settings).ok()?;
        if !file.file_type().is_file() {
            return None;
        }
        let folder = std::fs::metadata(settings.parent()?).ok()?;
        Some(Self {
            file: identity(&file),
            folder: identity(&folder),
        })
    }
}

/// A file's device and inode.
#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

/// Elsewhere there is no inode to pin, and no keychain keeps a key either
/// (`Keychain::keeps_keys`), so a move stops before a value is kept; the link
/// check stands alone.
#[cfg(not(unix))]
fn identity(_meta: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// The settings file's bytes, read through the file that was pinned: a link
/// or another file standing at the path is refused before a byte is read.
fn read_pinned(settings: &Path, pinned: Pinned) -> Result<Vec<u8>, &'static str> {
    if Pinned::at(settings) != Some(pinned) {
        return Err("file_swapped");
    }
    let mut file = open_without_following(settings).map_err(|_| "file_swapped")?;
    if file.metadata().map(|meta| identity(&meta)).ok() != Some(pinned.file) {
        return Err("file_swapped");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| "cell_changed")?;
    Ok(bytes)
}

#[cfg(unix)]
fn open_without_following(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_without_following(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// The settings files a move is running over in this window, whichever page
/// or window pressed it: a second press waits for nothing and is refused.
static MOVING: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

/// One settings file's move, held until it ends.
struct Moving(PathBuf);

impl Moving {
    fn take(settings: &Path) -> Option<Self> {
        let mut moving = MOVING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if moving.iter().any(|held| held == settings) {
            return None;
        }
        moving.push(settings.to_path_buf());
        Some(Self(settings.to_path_buf()))
    }
}

impl Drop for Moving {
    fn drop(&mut self) {
        MOVING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|held| *held != self.0);
    }
}

/// What the settings show: the names of the secrets written as plain text,
/// each once — never a value, a piece of one or its length.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretNotice {
    pub names: Vec<String>,
}

/// One cell moved: the name it now refers to and zo's backup of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Moved {
    pub name: String,
    pub backup: String,
}

/// Where a move stopped and why. `reason` is the window's word for the page's
/// sentence; `why` is zo's own sentence when zo gave one; `keychain_kept` says
/// the keychain item holding the value was left in place for the next try.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stopped {
    pub name: String,
    pub reason: &'static str,
    pub why: Option<String>,
    pub keychain_kept: bool,
}

/// A press of 「키체인으로 옮기기」: the cells moved in order, where it
/// stopped, and the names not tried because it stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MoveOutcome {
    pub moved: Vec<Moved>,
    pub stopped: Option<Stopped>,
    pub left: Vec<String>,
}

/// The keychain item zo reads `${name}` from.
#[must_use]
pub fn keychain_service(name: &str) -> String {
    format!("{SERVICE_KEYCHAIN_SERVICE_PREFIX}{name}")
}

/// The plain-text cells of the person's settings file zo lists; `None` when
/// zo does not answer the door, or answers a row of another shape.
fn literals(zo: &ZoDoor, settings: &Path, pinned: Pinned) -> Option<Vec<Literal>> {
    let (code, said) = zo.ask(&["settings", "secrets", "--json"])?;
    if code != 0 {
        return None;
    }
    let rows = said
        .get("literals")?
        .as_array()?
        .iter()
        .map(Literal::of)
        .collect::<Option<Vec<_>>>()?;
    Some(
        rows.into_iter()
            .filter(|literal| is_settings_file(&literal.file, settings, pinned))
            .collect(),
    )
}

/// The names the settings line shows, or `None` when there is nothing to show
/// — none written as plain text, or a zo that does not know the door.
pub(crate) fn notice(zo: &ZoDoor, settings: &Path) -> Option<SecretNotice> {
    let mut names: Vec<String> = Vec::new();
    for literal in literals(zo, settings, Pinned::at(settings)?)? {
        if !names.contains(&literal.name) {
            names.push(literal.name);
        }
    }
    (!names.is_empty()).then_some(SecretNotice { names })
}

/// Every plain-text cell of the person's settings file, moved in the order zo
/// listed them, stopping at the first that cannot be.
pub(crate) fn move_all(zo: &ZoDoor, settings: &Path, keys: &dyn RouterKeys) -> MoveOutcome {
    let before_any = |reason: &'static str| MoveOutcome {
        stopped: Some(Stopped {
            name: String::new(),
            reason,
            why: None,
            keychain_kept: false,
        }),
        ..MoveOutcome::default()
    };
    let Some(_moving) = Moving::take(settings) else {
        return before_any("already_moving");
    };
    let Some(pinned) = Pinned::at(settings) else {
        return before_any("file_swapped");
    };
    let Some(literals) = literals(zo, settings, pinned) else {
        return before_any("zo_silent");
    };
    let mut outcome = MoveOutcome::default();
    let mut rest = literals.into_iter();
    while let Some(literal) = rest.next() {
        match move_one(zo, settings, pinned, &literal, keys) {
            Ok(backup) => outcome.moved.push(Moved {
                name: literal.name,
                backup,
            }),
            Err(stopped) => {
                outcome.stopped = Some(stopped);
                outcome.left = rest.map(|literal| literal.name).collect();
                break;
            }
        }
    }
    outcome
}

/// One cell: its value read into memory, kept in the keychain item zo reads,
/// the reference checked, the cell rewritten by zo. The value lives in this
/// call alone and is dropped with it.
fn move_one(
    zo: &ZoDoor,
    settings: &Path,
    pinned: Pinned,
    literal: &Literal,
    keys: &dyn RouterKeys,
) -> Result<String, Stopped> {
    let stop = |reason: &'static str, why: Option<String>, keychain_kept: bool| Stopped {
        name: literal.name.clone(),
        reason,
        why,
        keychain_kept,
    };
    // The value, kept past the lock only to keep it out of zo's reasons.
    let value = {
        // Held from the read of the cell to the keychain write, and let go
        // before zo is asked: zo takes the same lock to rewrite the cell.
        let _lock =
            SettingsFileLock::acquire(settings).map_err(|_| stop("settings_busy", None, false))?;
        let bytes = read_pinned(settings, pinned).map_err(|reason| stop(reason, None, false))?;
        let value = cell(&bytes, &literal.path).ok_or_else(|| stop("cell_changed", None, false))?;
        if value.contains("${") {
            return Err(stop("already_reference", None, false));
        }
        // A trimmed copy would be another value, and zo refuses a reference
        // that does not resolve to the cell's very bytes.
        if value.is_empty() || value.trim() != value {
            return Err(stop("padded", None, false));
        }
        // The last look, just before the write: the same file, holding the
        // same bytes.
        let unchanged = || match read_pinned(settings, pinned) {
            Ok(now) if now == bytes => Ok(()),
            Ok(_) => Err("cell_changed"),
            Err(reason) => Err(reason),
        };
        crate::typesafe_settings::save_key_unless_another_at(
            &keychain_service(&literal.name),
            &value,
            "비밀 값이 비어 있습니다",
            keys,
            unchanged,
        )
        .map_err(|kept_not| match kept_not {
            KeptNot::Another => stop("keychain_differs", None, false),
            KeptNot::Changed(reason) => stop(reason, None, false),
            KeptNot::Refused(refusal) => {
                stop("keychain", said_without(&refusal.message, &value), false)
            }
        })?;
        value
    };
    let name = literal.name.as_str();
    match zo.ask(&["settings", "secrets", "check", "--name", name, "--json"]) {
        Some((0, said)) if said.get("resolves") == Some(&Value::Bool(true)) => {}
        Some((_, said)) if said.get("resolves") == Some(&Value::Bool(false)) => {
            return Err(stop("not_resolved", why_of(&said, &value), true));
        }
        _ => return Err(stop("zo_silent", None, true)),
    }
    let path =
        serde_json::to_string(&literal.path).map_err(|_| stop("cell_changed", None, true))?;
    let args = [
        "settings",
        "secrets",
        "move",
        "--file",
        literal.file.as_str(),
        "--path",
        path.as_str(),
        "--name",
        name,
        "--json",
    ];
    match zo.ask(&args) {
        Some((0, said)) if said.get("moved") == Some(&Value::Bool(true)) => said
            .get("backup")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| stop("zo_silent", None, true)),
        Some((_, said)) if said.get("moved") == Some(&Value::Bool(false)) => {
            Err(stop("move_refused", why_of(&said, &value), true))
        }
        _ => Err(stop("zo_silent", None, true)),
    }
}

/// The string at `path` in the settings file's bytes; `None` when the file is
/// not JSON, or a key on the way or the string is gone.
fn cell(bytes: &[u8], path: &[String]) -> Option<String> {
    let root = serde_json::from_slice::<Value>(bytes).ok()?;
    let mut at = &root;
    for key in path {
        at = at.get(key)?;
    }
    at.as_str().map(str::to_string)
}

/// zo's reason, for the page.
fn why_of(said: &Value, value: &str) -> Option<String> {
    said_without(said.get("why")?.as_str()?, value)
}

/// A sentence for the page, cut to [`WHY_CHARS`] — and not said at all when
/// it carries the value.
fn said_without(sentence: &str, value: &str) -> Option<String> {
    let sentence = sentence.trim();
    if sentence.is_empty() || sentence.contains(value) {
        return None;
    }
    Some(sentence.chars().take(WHY_CHARS).collect())
}

#[cfg(all(test, unix))]
#[path = "settings_secrets_tests.rs"]
mod tests;
