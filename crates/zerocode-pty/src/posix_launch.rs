//! Starting a pty child on macOS without a fork.
//!
//! `portable_pty`'s `spawn_command` adds a `pre_exec` closure (signal reset,
//! `setsid`, `TIOCSCTTY`, closing stray descriptors), and a `pre_exec`
//! closure makes std fork and run code in the child before its exec. In a
//! window that runs CEF that is not safe: on 2026-10-25 03:56 a child forked
//! for the usage scan stood forever in `xpc_atfork_child` waiting on a
//! malloc-zone mutex another thread held at the fork, never reached its exec,
//! and kept a copy of every descriptor the window had open — the settings
//! lock among them, which held the next three windows at boot (t-73907).
//!
//! `posix_spawn` asks the kernel for the whole start in one call, so no code
//! of ours or of any library runs between the two. Everything the closure
//! did has a spawn attribute or file action that does the same:
//!
//! - `POSIX_SPAWN_SETSID` makes the child a session and group leader;
//! - opening the pty's slave by path as descriptor 0 then makes the slave its
//!   controlling terminal, as an open by a session leader with none does on
//!   BSD (what `TIOCSCTTY` did), and 1 and 2 are copies of it;
//! - `POSIX_SPAWN_CLOEXEC_DEFAULT` closes every other descriptor;
//! - `POSIX_SPAWN_SETSIGDEF` and `POSIX_SPAWN_SETSIGMASK` reset the signals the
//!   closure reset (plus `SIGPIPE`, which std resets for every child) and
//!   empty the mask.
//!
//! How the program, its directory and `SHELL` are chosen follows
//! `portable_pty::CommandBuilder::as_command` exactly, so a lane's child
//! sees the same start it saw before.

use std::ffi::{CStr, CString, OsStr, OsString, c_char, c_int};
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Component, Path, PathBuf};

use portable_pty::{Child, ChildKiller, ExitStatus};

/// `<spawn.h>` names the `libc` crate does not carry.
const POSIX_SPAWN_SETSID: c_int = 0x0400;

unsafe extern "C" {
    fn posix_spawn_file_actions_addchdir_np(
        actions: *mut libc::posix_spawn_file_actions_t,
        path: *const c_char,
    ) -> c_int;
}

/// The signals a child starts with at their default action: the ones
/// `portable_pty`'s closure reset, and `SIGPIPE`, which this process ignores
/// (Rust's runtime does) and std resets for every child it starts.
const DEFAULT_SIGNALS: [c_int; 7] = [
    libc::SIGCHLD,
    libc::SIGHUP,
    libc::SIGINT,
    libc::SIGQUIT,
    libc::SIGTERM,
    libc::SIGALRM,
    libc::SIGPIPE,
];

/// One change to the inherited environment: a value to set, or `None` to
/// take the name out.
pub(crate) type EnvEdit = (OsString, Option<OsString>);

/// Start `program` with `args` on the pty whose slave is `tty`.
pub(crate) fn spawn(
    tty: &Path,
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
    edits: &[EnvEdit],
) -> io::Result<SpawnedChild> {
    let mut env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (key, value) in edits {
        env.retain(|(named, _)| named != key);
        if let Some(value) = value {
            env.push((key.clone(), value.clone()));
        }
    }
    let lookup = |key: &str| {
        env.iter()
            .find(|(named, _)| named == key)
            .map(|(_, value)| value.clone())
    };
    let home = lookup("HOME")
        .filter(|home| home.to_str().is_some())
        .map_or_else(password_home, PathBuf::from);
    let dir = cwd
        .filter(|dir| dir.is_dir())
        .map_or(home, Path::to_path_buf);
    let shell = lookup("SHELL")
        .filter(|shell| shell.to_str().is_some() && executable(Path::new(shell)))
        .map_or_else(password_shell, PathBuf::from);
    let resolved = search_path(program, &dir, lookup("PATH").as_deref())?;
    env.retain(|(named, _)| named != "SHELL");
    env.push(("SHELL".into(), shell.into_os_string()));

    let mut argv = vec![c_string(program.as_bytes())?];
    for arg in args {
        argv.push(c_string(arg.as_bytes())?);
    }
    let envp = env
        .iter()
        .map(|(key, value)| {
            let mut pair = key.as_bytes().to_vec();
            pair.push(b'=');
            pair.extend_from_slice(value.as_bytes());
            c_string(&pair)
        })
        .collect::<io::Result<Vec<_>>>()?;
    let path = c_string(resolved.as_os_str().as_bytes())?;
    let tty = c_string(tty.as_os_str().as_bytes())?;
    let dir = c_string(dir.as_os_str().as_bytes())?;

    let mut argv_ptrs: Vec<*mut c_char> = argv.iter().map(|arg| arg.as_ptr().cast_mut()).collect();
    argv_ptrs.push(std::ptr::null_mut());
    let mut envp_ptrs: Vec<*mut c_char> =
        envp.iter().map(|pair| pair.as_ptr().cast_mut()).collect();
    envp_ptrs.push(std::ptr::null_mut());

    let mut actions = FileActions::new()?;
    actions.chdir(&dir)?;
    actions.open(0, &tty, libc::O_RDWR)?;
    actions.dup2(0, 1)?;
    actions.dup2(0, 2)?;
    let mut attributes = Attributes::new()?;
    attributes.set_signals()?;
    attributes.set_flags(
        POSIX_SPAWN_SETSID
            | libc::POSIX_SPAWN_CLOEXEC_DEFAULT
            | libc::POSIX_SPAWN_SETSIGDEF
            | libc::POSIX_SPAWN_SETSIGMASK,
    )?;

    let mut pid: libc::pid_t = 0;
    let started = unsafe {
        libc::posix_spawn(
            &raw mut pid,
            path.as_ptr(),
            &raw const actions.0,
            &raw const attributes.0,
            argv_ptrs.as_ptr(),
            envp_ptrs.as_ptr(),
        )
    };
    if started != 0 {
        return Err(io::Error::from_raw_os_error(started));
    }
    Ok(SpawnedChild { pid, status: None })
}

fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "a launch argument holds a NUL byte",
        )
    })
}

fn checked(code: c_int) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code))
    }
}

struct FileActions(libc::posix_spawn_file_actions_t);

impl FileActions {
    fn new() -> io::Result<Self> {
        let mut actions = std::mem::MaybeUninit::uninit();
        checked(unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) })?;
        Ok(Self(unsafe { actions.assume_init() }))
    }

    fn chdir(&mut self, dir: &CStr) -> io::Result<()> {
        checked(unsafe { posix_spawn_file_actions_addchdir_np(&raw mut self.0, dir.as_ptr()) })
    }

    fn open(&mut self, fd: c_int, path: &CStr, flags: c_int) -> io::Result<()> {
        checked(unsafe {
            libc::posix_spawn_file_actions_addopen(&raw mut self.0, fd, path.as_ptr(), flags, 0)
        })
    }

    fn dup2(&mut self, from: c_int, to: c_int) -> io::Result<()> {
        checked(unsafe { libc::posix_spawn_file_actions_adddup2(&raw mut self.0, from, to) })
    }
}

impl Drop for FileActions {
    fn drop(&mut self) {
        unsafe { libc::posix_spawn_file_actions_destroy(&raw mut self.0) };
    }
}

struct Attributes(libc::posix_spawnattr_t);

impl Attributes {
    fn new() -> io::Result<Self> {
        let mut attributes = std::mem::MaybeUninit::uninit();
        checked(unsafe { libc::posix_spawnattr_init(attributes.as_mut_ptr()) })?;
        Ok(Self(unsafe { attributes.assume_init() }))
    }

    fn set_signals(&mut self) -> io::Result<()> {
        let attributes = &raw mut self.0;
        let mut defaults: libc::sigset_t = 0;
        let mut mask: libc::sigset_t = 0;
        unsafe {
            libc::sigemptyset(&raw mut defaults);
            libc::sigemptyset(&raw mut mask);
            for signal in DEFAULT_SIGNALS {
                libc::sigaddset(&raw mut defaults, signal);
            }
        }
        checked(unsafe { libc::posix_spawnattr_setsigdefault(attributes, &raw const defaults) })?;
        checked(unsafe { libc::posix_spawnattr_setsigmask(attributes, &raw const mask) })
    }

    fn set_flags(&mut self, flags: c_int) -> io::Result<()> {
        let flags = libc::c_short::try_from(flags)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "spawn flags"))?;
        checked(unsafe { libc::posix_spawnattr_setflags(&raw mut self.0, flags) })
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { libc::posix_spawnattr_destroy(&raw mut self.0) };
    }
}

fn executable(path: &Path) -> bool {
    CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0)
}

fn exists(path: &Path) -> bool {
    CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::F_OK) } == 0)
}

/// The password database's word on this user, as `portable_pty` reads it.
fn password_entry<T>(read: impl FnOnce(&libc::passwd) -> Option<T>) -> Option<T> {
    let entry = unsafe { libc::getpwuid(libc::getuid()) };
    if entry.is_null() {
        return None;
    }
    read(unsafe { &*entry })
}

fn password_home() -> PathBuf {
    password_entry(|entry| {
        (!entry.pw_dir.is_null()).then(|| {
            PathBuf::from(OsString::from_vec(
                unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes().to_vec(),
            ))
        })
    })
    .unwrap_or_else(|| PathBuf::from("/"))
}

fn password_shell() -> PathBuf {
    password_entry(|entry| {
        (!entry.pw_shell.is_null()).then(|| {
            PathBuf::from(OsString::from_vec(
                unsafe { CStr::from_ptr(entry.pw_shell) }
                    .to_bytes()
                    .to_vec(),
            ))
        })
    })
    .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

/// `portable_pty`'s own lookup: an absolute program as it is, one that starts
/// with `.` or `..` under `dir` only, and any other name on the child's own
/// `PATH`, whose relative entries are read from `dir`.
fn search_path(program: &OsStr, dir: &Path, path: Option<&OsStr>) -> io::Result<PathBuf> {
    let named = Path::new(program);
    if named.is_absolute() {
        return Ok(named.to_path_buf());
    }
    let refused = |reason: String| io::Error::new(io::ErrorKind::NotFound, reason);
    if matches!(
        named.components().next(),
        Some(Component::CurDir | Component::ParentDir)
    ) {
        let candidate = dir.join(named);
        return if candidate.is_dir() {
            Err(refused(format!("{} is a directory", candidate.display())))
        } else if executable(&candidate) {
            Ok(candidate)
        } else if exists(&candidate) {
            Err(refused(format!(
                "{} is not executable",
                candidate.display()
            )))
        } else {
            Err(refused(format!("{} does not exist", candidate.display())))
        };
    }
    let Some(path) = path else {
        return Err(refused(format!(
            "{} is not on a PATH: the launch has none",
            named.display()
        )));
    };
    for entry in std::env::split_paths(path) {
        let candidate = dir.join(entry).join(named);
        if !candidate.is_dir() && executable(&candidate) {
            return Ok(candidate);
        }
    }
    Err(refused(format!("{} is not on PATH", named.display())))
}

/// A child [`spawn`] started. Waited on with `waitpid`, the way
/// `std::process::Child` is, and killed the way `portable_pty` kills one:
/// `SIGHUP`, a fifth of a second for it to go, then `SIGKILL`.
#[derive(Debug)]
pub(crate) struct SpawnedChild {
    pid: libc::pid_t,
    status: Option<ExitStatus>,
}

impl SpawnedChild {
    fn reap(&mut self, options: c_int) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = &self.status {
            return Ok(Some(status.clone()));
        }
        let mut raw = 0;
        loop {
            match unsafe { libc::waitpid(self.pid, &raw mut raw, options) } {
                0 => return Ok(None),
                -1 => {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                _ => {
                    let status = ExitStatus::from(std::process::ExitStatus::from_raw(raw));
                    self.status = Some(status.clone());
                    return Ok(Some(status));
                }
            }
        }
    }
}

impl Child for SpawnedChild {
    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.reap(libc::WNOHANG)
    }

    fn wait(&mut self) -> io::Result<ExitStatus> {
        self.reap(0)?
            .ok_or_else(|| io::Error::other("waitpid answered without a status"))
    }

    fn process_id(&self) -> Option<u32> {
        u32::try_from(self.pid).ok()
    }
}

impl ChildKiller for SpawnedChild {
    fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        if unsafe { libc::kill(self.pid, libc::SIGHUP) } != 0 {
            return Err(io::Error::last_os_error());
        }
        for attempt in 0..5 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if let Ok(Some(_)) = self.try_wait() {
                return Ok(());
            }
        }
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Hangup(self.pid))
    }
}

/// What [`SpawnedChild::clone_killer`] hands out: a `SIGHUP` by pid, as
/// `portable_pty`'s own split-off killer sends.
#[derive(Debug)]
struct Hangup(libc::pid_t);

impl ChildKiller for Hangup {
    fn kill(&mut self) -> io::Result<()> {
        if unsafe { libc::kill(self.0, libc::SIGHUP) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(Hangup(self.0))
    }
}
