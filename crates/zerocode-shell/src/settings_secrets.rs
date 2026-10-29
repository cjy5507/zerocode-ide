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
//!
//! And from where (t-76698 r3): the person's home folder (or the config home
//! they named) is taken as it is, and every folder below it down to the file
//! is opened by handle without following a link, so a `.zo` that is a link to
//! a project folder is no settings file of the person's. The file is looked at
//! once more just before zo is asked to rewrite it, and each move goes on from
//! the file zo's move left only when that file is the one read with the one
//! cell, and nothing else, turned into its reference.
//!
//! And of one name (t-76698 r4): a settings file with a second name — a hard
//! link, which a project folder can hold and no folder handle sees — is not
//! read, pinned or gone on from. The file the last move left is looked at as
//! any other, and one not as expected is said. zo's output is read by the
//! watch without blocking and to a bound, and let go at the wait.
//!
//! And only where a key is kept (t-76698 r5): on a machine with no keychain
//! (every one but a Mac) no door is asked and nothing is shown — before zo is
//! started, not after its answer. A pipe there would be read by a blocking
//! thread no deadline can cancel, so no such reader is written.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
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

/// How long a door's watch waits for the output, or between two looks at zo.
const TICK: Duration = Duration::from_millis(20);

/// The most of a door's output read, in bytes: a listing names a few cells,
/// and an output past this is no answer.
#[cfg(unix)]
const PRINTED_BYTES: usize = 1024 * 1024;

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
    /// Whether this machine keeps a key at all. Where it does not, the doors
    /// are not asked (t-76698 r5).
    supported: bool,
}

impl ZoDoor {
    /// The installed zo, with a zo pane's account variables.
    pub(crate) fn of_this_window(config_root: &Path) -> Option<Self> {
        let home = dirs::home_dir()?;
        Some(
            Self::at(
                crate::zo_companion::zo_path_under(&home),
                crate::accounts::reading_env_for(config_root, "zo"),
            )
            .supported(crate::accounts::KEYCHAIN_AVAILABLE),
        )
    }

    pub(crate) fn at(bin: PathBuf, env: Vec<(String, String)>) -> Self {
        Self {
            bin,
            env,
            wait: ZO_WAIT,
            supported: true,
        }
    }

    /// The same door on a machine that does, or does not, keep a key.
    pub(crate) fn supported(self, supported: bool) -> Self {
        Self { supported, ..self }
    }

    /// One door's exit code and the JSON it printed, whatever the code; `None`
    /// when zo is missing, is killed for outliving the wait, or prints no JSON
    /// — and with zo not started where no key is kept.
    fn ask(&self, args: &[&str]) -> Option<(i32, Value)> {
        if !self.supported {
            return None;
        }
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
        let Some(output) = child.stdout.take().and_then(|out| Output::of(out).ok()) else {
            give_up(&mut child);
            return None;
        };
        watch(&mut Started { child, output }, self.wait)
    }
}

/// zo's output as it comes, read to its end or to [`PRINTED_BYTES`] and no
/// further. It is read by the watch itself, without blocking, and let go when
/// the door is given up: a child that left zo's group and holds the output
/// open keeps no reader and no descriptor of the window's past the wait
/// (t-76698 r4).
#[cfg(unix)]
struct Output {
    pipe: Option<File>,
    bytes: Vec<u8>,
}

#[cfg(unix)]
impl Output {
    fn of(pipe: impl Into<std::os::fd::OwnedFd>) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let pipe = File::from(pipe.into());
        let fd = pipe.as_raw_fd();
        // SAFETY: `fd` is the pipe just taken, open for both calls, and only
        // its status flags are read and set.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        // SAFETY: as above.
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self {
            pipe: Some(pipe),
            bytes: Vec::new(),
        })
    }

    /// As [`Answering::printed`]: waits up to `tick` for the output, then
    /// reads what has come. An output past its bound, or one that cannot be
    /// read, is let go and is no answer.
    fn printed(&mut self, tick: Duration) -> Option<Option<String>> {
        use std::os::fd::AsRawFd;
        let Some(pipe) = self.pipe.as_mut() else {
            return Some(None);
        };
        let mut ready = libc::pollfd {
            fd: pipe.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let wait = libc::c_int::try_from(tick.as_millis()).unwrap_or(libc::c_int::MAX);
        // SAFETY: one `pollfd`, alive for the call. What it answers is not
        // needed: the read below finds what has come, or nothing.
        unsafe { libc::poll(&mut ready, 1, wait) };
        let mut chunk = [0_u8; 8192];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => {
                    self.pipe = None;
                    return Some(String::from_utf8(std::mem::take(&mut self.bytes)).ok());
                }
                Ok(read) => {
                    self.bytes.extend_from_slice(&chunk[..read]);
                    if self.bytes.len() > PRINTED_BYTES {
                        self.close();
                        return Some(None);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return None,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => {
                    self.close();
                    return Some(None);
                }
            }
        }
    }

    /// The window's end of the output, closed: a writer still holding the
    /// other end finds it broken.
    fn close(&mut self) {
        self.pipe = None;
        self.bytes = Vec::new();
    }
}

/// Elsewhere no key is kept and zo is not asked (see [`ZoDoor::supported`]);
/// a pipe is not read without blocking there, so none is read: a door that is
/// asked all the same is given up.
#[cfg(not(unix))]
struct Output;

#[cfg(not(unix))]
impl Output {
    fn of(_pipe: std::process::ChildStdout) -> std::io::Result<Self> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn printed(&mut self, _tick: Duration) -> Option<Option<String>> {
        Some(None)
    }

    fn close(&mut self) {}
}

/// A door zo is answering, as [`watch`] sees it: zo's output, zo the process
/// and the clock — made up in the tests, so the watch's order is tried on
/// process states and times no real zo is asked to keep.
trait Answering {
    fn now(&self) -> Instant;
    /// zo's output once zo's end of it has closed (`Some(None)` when it could
    /// not be read), waiting up to `tick` for it; `None` while it is open.
    fn printed(&mut self, tick: Duration) -> Option<Option<String>>;
    /// zo reaped, once it has ended; `None` while it runs.
    fn reaped(&mut self) -> std::io::Result<Option<ExitStatus>>;
    /// zo and its whole process group killed, then zo reaped.
    fn give_up(&mut self);
    fn pause(&mut self, tick: Duration);
}

/// One door watched to `wait`: the output and the process have the same
/// wait. zo is reaped only once its output has closed. Until it is reaped its
/// number names no other process, so a zo that ended while a child holds the
/// output open — in its group or one it left — is still zo's own number at
/// the wait, and the group's kill reaches no one else's group. Once reaped,
/// zo is sent nothing; a zo the window can no longer wait on is sent nothing
/// either.
fn watch(door: &mut dyn Answering, wait: Duration) -> Option<(i32, Value)> {
    let deadline = door.now() + wait;
    let printed = loop {
        if door.now() >= deadline {
            door.give_up();
            return None;
        }
        if let Some(printed) = door.printed(TICK) {
            break printed;
        }
    };
    let status = loop {
        if door.now() >= deadline {
            door.give_up();
            return None;
        }
        match door.reaped() {
            Ok(Some(status)) => break status,
            Ok(None) => door.pause(TICK),
            Err(_) => return None,
        }
    };
    Some((status.code()?, serde_json::from_str(printed?.trim()).ok()?))
}

/// A real zo, started.
struct Started {
    child: Child,
    output: Output,
}

impl Answering for Started {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn printed(&mut self, tick: Duration) -> Option<Option<String>> {
        self.output.printed(tick)
    }

    fn reaped(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn give_up(&mut self) {
        self.output.close();
        give_up(&mut self.child);
    }

    fn pause(&mut self, tick: Duration) {
        std::thread::sleep(tick);
    }
}

/// A door that outlived its wait: zo and everything it started, killed. Only
/// for a zo not yet reaped, whose number is still its own.
fn give_up(child: &mut Child) {
    #[cfg(unix)]
    let _ = crate::codex_queue::signal_process_group(child.id(), libc::SIGKILL);
    let _ = child.kill();
    let _ = child.wait();
}

/// One plain-text cell zo's listing named: the file as zo spelled it (and is
/// handed back), the keys down to the cell, the name it is to refer to, and
/// — for a settings file whose own name is a link zo read through
/// (t-77216, zo's t-78117) — the file the link stands on.
struct Literal {
    file: String,
    path: Vec<String>,
    name: String,
    via_link: Option<String>,
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
        let via_link = match row.get("via_link") {
            None | Some(Value::Null) => None,
            Some(target) => Some(target.as_str().filter(|at| Path::new(at).is_absolute())?),
        };
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
            via_link: via_link.map(str::to_string),
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

/// The most of zo's own sentence about a file it did not read that the page
/// is handed, in characters.
const SKIPPED_WHY_CHARS: usize = 300;

/// A person's settings file zo did not read (t-77216, zo's t-78117): the
/// file as zo's listing saw it, zo's word for why — the page picks its own
/// sentence for a word it knows — and zo's own sentence, said for a word it
/// does not. zo never read the file, so neither carries a value of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Skipped {
    pub file: String,
    pub reason: String,
    pub why: String,
}

impl Skipped {
    /// One row: an absolute file and a word of lower-case letters and `_`;
    /// any other row is left out.
    fn of(row: &Value) -> Option<Self> {
        let file = row.get("file")?.as_str()?;
        let reason = row.get("reason")?.as_str()?;
        let why = row.get("why").and_then(Value::as_str).unwrap_or_default();
        (Path::new(file).is_absolute()
            && !reason.is_empty()
            && reason
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'_'))
        .then(|| Self {
            file: file.to_string(),
            reason: reason.to_string(),
            why: why.chars().take(SKIPPED_WHY_CHARS).collect(),
        })
    }
}

/// Whether zo's `file` is the person's settings file the window pinned: its
/// path, or another spelling of it below the same root that stands on the
/// very same file of one name — never a link to it. A cell elsewhere is not
/// the window's to read. The window reads only through its own path; zo's
/// spelling is what zo's move is handed back.
fn is_settings_file(file: &str, settings: &SettingsFile, pinned: Pinned) -> bool {
    Path::new(file) == settings.path
        || (SettingsFile::under(settings.root.clone(), PathBuf::from(file)).is_some()
            && std::fs::symlink_metadata(file).is_ok_and(|meta| {
                meta.file_type().is_file() && one_name(&meta) && identity(&meta) == pinned.file
            }))
}

/// The person's settings file and the folder it is trusted from — the home
/// folder, or the config home the person named — whose own path is taken as
/// it is. Every folder below it down to the file is opened by handle without
/// following a link: a `.zo` that links to a project folder is not the
/// person's.
#[derive(Debug, Clone)]
pub(crate) struct SettingsFile {
    root: PathBuf,
    path: PathBuf,
}

impl SettingsFile {
    /// `path` below `root` by plain names alone; `None` for one that is not
    /// below it, or names `.` or `..` on the way.
    pub(crate) fn under(root: PathBuf, path: PathBuf) -> Option<Self> {
        let below = path.strip_prefix(&root).ok()?;
        (below.components().next().is_some()
            && below
                .components()
                .all(|name| matches!(name, Component::Normal(_))))
        .then_some(Self { root, path })
    }

    /// The file, opened from the root a folder at a time without following a
    /// link, with the identity of the folder holding it; an error for a link
    /// anywhere below the root, a folder that is not one, or a file that is
    /// not a plain file of one name. A file with a second name — a hard link,
    /// which no folder handle sees — may be a project's file too (t-76698
    /// r4).
    #[cfg(unix)]
    fn open(&self) -> std::io::Result<((u64, u64), File)> {
        let (folder, name) = self.holding_folder()?;
        // Not blocking: a pipe put at the path is not waited on.
        let file = open_at(&folder, name, libc::O_NONBLOCK)?;
        let meta = file.metadata()?;
        if !meta.file_type().is_file() || !one_name(&meta) {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        Ok((identity(&folder.metadata()?), file))
    }

    /// The folder holding the file, opened from the root a folder at a time
    /// without following a link, and the file's own name in it.
    #[cfg(unix)]
    fn holding_folder(&self) -> std::io::Result<(File, &std::ffi::OsStr)> {
        use std::os::unix::fs::OpenOptionsExt;
        let names = self
            .path
            .strip_prefix(&self.root)
            .map_err(std::io::Error::other)?
            .iter()
            .collect::<Vec<_>>();
        let Some((name, folders)) = names.split_last() else {
            return Err(std::io::ErrorKind::NotFound.into());
        };
        let mut folder = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY)
            .open(&self.root)?;
        for below in folders {
            folder = open_at(&folder, below, libc::O_DIRECTORY)?;
        }
        Ok((folder, *name))
    }

    /// Whether the file's own name is a link, every folder above it down
    /// from the root a folder of its own (t-77216): the settings file stow
    /// or home-manager lays, which zo reads through its link and the window
    /// never does. A folder on the way that is a link is no settings file
    /// of the person's at all.
    #[cfg(unix)]
    fn ends_in_a_link(&self) -> bool {
        let Ok((folder, name)) = self.holding_folder() else {
            return false;
        };
        matches!(
            open_at(&folder, name, libc::O_NONBLOCK),
            Err(error) if error.raw_os_error() == Some(libc::ELOOP)
        )
    }

    /// Elsewhere no notice is drawn at all (no keychain, [`ZoDoor`]).
    #[cfg(not(unix))]
    fn ends_in_a_link(&self) -> bool {
        false
    }

    /// Elsewhere each name on the way is looked at for a link, then the file
    /// is opened; there is no inode to pin (see [`identity`]).
    #[cfg(not(unix))]
    fn open(&self) -> std::io::Result<((u64, u64), File)> {
        let mut at = self.root.clone();
        for name in self
            .path
            .strip_prefix(&self.root)
            .map_err(std::io::Error::other)?
        {
            at.push(name);
            if std::fs::symlink_metadata(&at)?.file_type().is_symlink() {
                return Err(std::io::ErrorKind::InvalidInput.into());
            }
        }
        let file = File::open(&self.path)?;
        if !file.metadata()?.file_type().is_file() {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        Ok(((0, 0), file))
    }
}

/// `name` in `folder`, opened read-only without following a link.
#[cfg(unix)]
fn open_at(folder: &File, name: &std::ffi::OsStr, flags: libc::c_int) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(name.as_bytes())?;
    // SAFETY: `folder` is an open folder for the call's length and `name` a
    // NUL-terminated string that outlives it; no other pointer is passed.
    let fd = unsafe {
        libc::openat(
            folder.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | flags,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `fd` was just opened by this call and is owned by nothing else.
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// The settings file a move trusts, as it stood when the move began: the file
/// (a plain one, not a link) and the folder holding it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pinned {
    file: (u64, u64),
    folder: (u64, u64),
}

impl Pinned {
    /// The settings file now, open; `None` when it, or a folder on the way
    /// down from the root, is a link, or it is not a file, or gone.
    fn open(settings: &SettingsFile) -> Option<(Self, File)> {
        let (folder, file) = settings.open().ok()?;
        let pinned = Self {
            file: identity(&file.metadata().ok()?),
            folder,
        };
        Some((pinned, file))
    }

    fn at(settings: &SettingsFile) -> Option<Self> {
        Self::open(settings).map(|(pinned, _)| pinned)
    }
}

/// A file's device and inode.
#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

/// Whether the file has one name: a second, a hard link, can stand anywhere
/// on the same device — in a project folder too.
#[cfg(unix)]
fn one_name(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.nlink() == 1
}

/// Elsewhere a move stops before a value is kept (see [`identity`]).
#[cfg(not(unix))]
fn one_name(_meta: &std::fs::Metadata) -> bool {
    true
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
fn read_pinned(settings: &SettingsFile, pinned: Pinned) -> Result<Vec<u8>, &'static str> {
    let (now, mut file) = Pinned::open(settings).ok_or("file_swapped")?;
    if now != pinned {
        return Err("file_swapped");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|_| "cell_changed")?;
    Ok(bytes)
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
/// each once — never a value, a piece of one or its length. When the
/// settings file's own name is a link zo read through (t-77216), the names
/// it read there are `linked`, beside the file the link stands on: the
/// window moves none of them, and the person edits that file. The settings
/// files zo did not read are `skipped`, each with zo's word for why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretNotice {
    pub names: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub linked: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linked_to: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub skipped: Vec<Skipped>,
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

/// zo's listing whole: every plain-text cell it names, and the person's
/// settings files it did not read (t-77216) — an older zo says nothing of
/// those, and a skipped row of another shape is left out. `None` when zo
/// does not answer the door, says the move is not supported here (no
/// keychain to refer to, as on Windows; a zo that does not say is taken to
/// support it), or answers a cell of another shape.
fn listing(zo: &ZoDoor) -> Option<(Vec<Literal>, Vec<Skipped>)> {
    let (code, said) = zo.ask(&["settings", "secrets", "--json"])?;
    if code != 0 || said.get("supported") == Some(&Value::Bool(false)) {
        return None;
    }
    let rows = said
        .get("literals")?
        .as_array()?
        .iter()
        .map(Literal::of)
        .collect::<Option<Vec<_>>>()?;
    let skipped = said
        .get("skipped")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(Skipped::of).collect())
        .unwrap_or_default();
    Some((rows, skipped))
}

/// The plain-text cells of the person's settings file zo lists that the
/// window may move: never one zo read through a link (t-77216). `None` as
/// for [`listing`].
fn literals(zo: &ZoDoor, settings: &SettingsFile, pinned: Pinned) -> Option<Vec<Literal>> {
    let (rows, _) = listing(zo)?;
    Some(
        rows.into_iter()
            .filter(|literal| {
                literal.via_link.is_none() && is_settings_file(&literal.file, settings, pinned)
            })
            .collect(),
    )
}

/// The names the settings line shows, or `None` when there is nothing to show
/// — none written as plain text, or a zo that does not know the door.
///
/// A settings file whose own name is a link (stow, home-manager) is zo's to
/// read through when its target passes zo's checks (t-77216, zo's t-78117):
/// the names zo read there are said as `linked`, at the window's own path
/// only, and a file zo did not read is said with zo's word for why. Before,
/// such a file dropped out of the line with nothing said. Any other file
/// the window cannot pin — a folder on the way that is a link, a second
/// name — still shows nothing, and zo is not asked.
pub(crate) fn notice(zo: &ZoDoor, settings: &SettingsFile) -> Option<SecretNotice> {
    if !zo.supported {
        return None;
    }
    let pinned = Pinned::at(settings);
    let linked_here = pinned.is_none() && settings.ends_in_a_link();
    if pinned.is_none() && !linked_here {
        return None;
    }
    let (rows, skipped) = listing(zo)?;
    let mut notice = SecretNotice::default();
    for literal in rows {
        let (names, ours) = match literal.via_link {
            Some(target) => {
                let ours = linked_here && Path::new(&literal.file) == settings.path;
                if ours && notice.linked_to.is_none() {
                    notice.linked_to = Some(target);
                }
                (&mut notice.linked, ours)
            }
            None => (
                &mut notice.names,
                pinned.is_some_and(|pinned| is_settings_file(&literal.file, settings, pinned)),
            ),
        };
        if ours && !names.contains(&literal.name) {
            names.push(literal.name);
        }
    }
    notice.skipped = skipped
        .into_iter()
        .filter(|row| {
            SettingsFile::under(settings.root.clone(), PathBuf::from(&row.file)).is_some()
        })
        .collect();
    (!notice.names.is_empty() || !notice.linked.is_empty() || !notice.skipped.is_empty())
        .then_some(notice)
}

/// Every plain-text cell of the person's settings file, moved in the order zo
/// listed them, stopping at the first that cannot be. Each cell after the
/// first is read from the file the move before it left, and the file the last
/// move left is looked at too: one not as expected stops the outcome as
/// `unconfirmed`, under the cell zo said it moved.
pub(crate) fn move_all(zo: &ZoDoor, settings: &SettingsFile, keys: &dyn RouterKeys) -> MoveOutcome {
    let before_any = |reason: &'static str| MoveOutcome {
        stopped: Some(Stopped {
            name: String::new(),
            reason,
            why: None,
            keychain_kept: false,
        }),
        ..MoveOutcome::default()
    };
    if !zo.supported {
        return before_any("zo_silent");
    }
    let Some(_moving) = Moving::take(&settings.path) else {
        return before_any("already_moving");
    };
    let Some(pinned) = Pinned::at(settings) else {
        return before_any("file_swapped");
    };
    let Some(literals) = literals(zo, settings, pinned) else {
        return before_any("zo_silent");
    };
    let mut outcome = MoveOutcome::default();
    let mut pinned = Ok(pinned);
    let mut rest = literals.into_iter();
    while let Some(literal) = rest.next() {
        let step = match pinned {
            Ok(pinned) => move_one(zo, settings, pinned, &literal, keys),
            Err(reason) => Err(Stopped {
                name: literal.name.clone(),
                reason,
                why: None,
                keychain_kept: false,
            }),
        };
        match step {
            Ok((backup, next)) => {
                // The last cell's file is looked at as any other: one zo left
                // other than the file read with that cell turned into its
                // reference is not a finished move (t-76698 r4).
                if next.is_err() && rest.as_slice().is_empty() {
                    outcome.stopped = Some(Stopped {
                        name: literal.name.clone(),
                        reason: "unconfirmed",
                        why: None,
                        keychain_kept: true,
                    });
                }
                outcome.moved.push(Moved {
                    name: literal.name,
                    backup,
                });
                pinned = next;
            }
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
/// call alone and is dropped with it. Answers zo's backup, and the file the
/// move left pinned for the next cell — or why the next cell is not read.
fn move_one(
    zo: &ZoDoor,
    settings: &SettingsFile,
    pinned: Pinned,
    literal: &Literal,
    keys: &dyn RouterKeys,
) -> Result<(String, Result<Pinned, &'static str>), Stopped> {
    let stop = |reason: &'static str, why: Option<String>, keychain_kept: bool| Stopped {
        name: literal.name.clone(),
        reason,
        why,
        keychain_kept,
    };
    // The value, kept past the lock only to keep it out of zo's reasons, and
    // the bytes it was read from, which zo is to rewrite.
    let (value, bytes) = {
        // Held from the read of the cell to the keychain write, and let go
        // before zo is asked: zo takes the same lock to rewrite the cell.
        let _lock = SettingsFileLock::acquire(&settings.path)
            .map_err(|_| stop("settings_busy", None, false))?;
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
        (value, bytes)
    };
    let name = literal.name.as_str();
    match zo.ask(&["settings", "secrets", "check", "--name", name, "--json"]) {
        Some((0, said)) if said.get("resolves") == Some(&Value::Bool(true)) => {}
        Some((_, said)) if said.get("resolves") == Some(&Value::Bool(false)) => {
            return Err(stop("not_resolved", why_of(&said, &value), true));
        }
        _ => return Err(stop("zo_silent", None, true)),
    }
    // The file zo is to rewrite is the file read, holding the bytes read:
    // looked at once more, under the lock, just before zo is asked. What is
    // left between this look and zo's own is zo's to refuse (its own read
    // again, the person's settings file, the value's very bytes).
    {
        let _lock = SettingsFileLock::acquire(&settings.path)
            .map_err(|_| stop("settings_busy", None, true))?;
        match read_pinned(settings, pinned) {
            Ok(now) if now == bytes => {}
            Ok(_) => return Err(stop("cell_changed", None, true)),
            Err(reason) => return Err(stop(reason, None, true)),
        }
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
            .map(|backup| (backup.to_string(), successor(settings, &bytes, literal)))
            .ok_or_else(|| stop("zo_silent", None, true)),
        Some((_, said)) if said.get("moved") == Some(&Value::Bool(false)) => {
            Err(stop("move_refused", why_of(&said, &value), true))
        }
        _ => Err(stop("zo_silent", None, true)),
    }
}

/// The file zo's move left, pinned for the next cell when it is the file the
/// move read with that one cell — and nothing else — now `${NAME}`. Any other
/// file is not gone on from: the window trusts no file it did not see made.
fn successor(
    settings: &SettingsFile,
    before: &[u8],
    literal: &Literal,
) -> Result<Pinned, &'static str> {
    let mut expected = serde_json::from_slice::<Value>(before).map_err(|_| "cell_changed")?;
    let mut at = &mut expected;
    for key in &literal.path {
        at = at.get_mut(key).ok_or("cell_changed")?;
    }
    *at = Value::String(format!("${{{}}}", literal.name));
    let _lock = SettingsFileLock::acquire(&settings.path).map_err(|_| "settings_busy")?;
    let (pinned, mut file) = Pinned::open(settings).ok_or("file_swapped")?;
    let mut now = Vec::new();
    file.read_to_end(&mut now).map_err(|_| "file_swapped")?;
    match serde_json::from_slice::<Value>(&now) {
        Ok(left) if left == expected => Ok(pinned),
        _ => Err("file_swapped"),
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
