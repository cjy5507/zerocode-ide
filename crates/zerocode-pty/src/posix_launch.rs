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
//! - the pty's slave is opened by path as descriptor 0, and 1 and 2 are
//!   copies of it;
//! - `POSIX_SPAWN_CLOEXEC_DEFAULT` closes every other descriptor;
//! - `POSIX_SPAWN_SETSIGDEF` and `POSIX_SPAWN_SETSIGMASK` reset the signals the
//!   closure reset (plus `SIGPIPE`, which std resets for every child) and
//!   empty the mask.
//!
//! What none of them does is `TIOCSCTTY`: the open a spawn's file action
//! makes leaves the child with no controlling terminal (measured 2026-10-27
//! on macOS 26.3: `/bin/sleep` started this way stands with no terminal and
//! outlives its master closing). A program that opens its terminal again by
//! name takes it then — Node does, so `claude` and `agy` had one — and one
//! that does not — `zsh`, which opens with `O_NOCTTY`, and `zo` — never has
//! one: the window going sends it no hangup, a resize sends nothing to what
//! it runs, and ^C reaches no group (t-77414: five zo processes went on
//! spinning after a restart, each holding its conversation). So when a
//! helper is named ([`launch_through`]), the spawn starts the helper on the
//! terminal instead, and the helper ([`serve_ctty_exec`]) makes descriptor
//! 0 its controlling terminal and becomes the program by `exec`: the same
//! process, the same pid and beginning, and still no fork.
//!
//! Two things a helper has to be for that (t-77495, Codex t-77467). It has
//! to know the road: one that does not — a mirror built before it, which
//! starts its first argument and leaves the marker for every launch under
//! the program — is not named, since it cannot answer the question
//! [`launch_through`] asks first. And the deployed mirror is signed with the
//! hardened runtime, so dyld takes every `DYLD_*` out of its environment
//! before its `main` runs: those travel past it under another name
//! ([`CTTY_DYLD_WRAP`]) and get their own back before the program starts.
//!
//! The question is asked on a thread of its own, within
//! [`CTTY_PROBE_LIMIT`] (t-77530, Codex t-77525): asked on the window's main
//! thread with no limit, a mirror that did not answer held the boot short
//! of its first paint for as long as it stood. A pane started before the
//! answer waits for it, never past the limit, and then starts straight.
//!
//! How the program, its directory and `SHELL` are chosen follows
//! `portable_pty::CommandBuilder::as_command` exactly, so a lane's child
//! sees the same start it saw before.

use std::ffi::{CStr, CString, OsStr, OsString, c_char, c_int};
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

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

/// What tells a helper it is starting a pane's program (t-77414), set to the
/// name the program is started under — its `argv[0]` — and taken out before
/// the program starts. It counts only beside the helper's own `argv[0]`
/// being `CTTY_EXEC_ARG0`, which no process passes on: a marker some older
/// helper left behind is inherited by everything the program starts, and on
/// its own it would turn the next call of a helper — the mirror's tee and
/// its orchestration guard — into a start of whatever that call names
/// (t-77495).
pub const CTTY_EXEC_ENV: &str = "ZEROCODE_CTTY_EXEC";

/// The `argv[0]` a lane starts its helper under, the other half of
/// [`CTTY_EXEC_ENV`].
const CTTY_EXEC_ARG0: &str = "zerocode-ctty-exec";

/// The question [`launch_through`] asks a helper before naming it: set, a
/// helper that knows the road writes `CTTY_PROBE_ANSWER` and ends with 0.
/// A mirror built before the road reads no environment and is handed no
/// argument, so it ends at once, saying its program is missing.
pub const CTTY_PROBE_ENV: &str = "ZEROCODE_CTTY_PROBE";

const CTTY_PROBE_ANSWER: &str = "zerocode-ctty-exec 1";

/// The prefix a `DYLD_*` name travels under past the helper — so
/// `DYLD_LIBRARY_PATH` as `ZEROCODE_CTTY_DYLD_LIBRARY_PATH` — given back its
/// own name before the program starts. dyld takes every `DYLD_*` out of a
/// hardened binary's environment, the deployed mirror's among them, and a
/// pane given one in the settings' environment field lost it (t-77495).
///
/// A name the pane was given that already has that shape is wrapped once
/// more, so no wrapped name lands on one of the pane's own, and the helper
/// takes one prefix off each: everything comes back as it was given
/// (`wraps`). A start with no helper wraps nothing and takes nothing out
/// (t-77530, Codex t-77525: a person's `ZEROCODE_CTTY_DYLD_*` was taken
/// out on every road).
pub const CTTY_DYLD_WRAP: &str = "ZEROCODE_CTTY_";

/// Where the helper writes its one line when the terminal could not become
/// the program's — the window's log. Taken out before the program starts.
pub const CTTY_LOG_ENV: &str = "ZEROCODE_CTTY_LOG";

/// The helper every lane's child starts through, once named.
struct Helper {
    program: PathBuf,
    log: Option<PathBuf>,
}

/// How much of a helper's answer is read: more than the answer is a wrong
/// one already.
const CTTY_PROBE_READ: u64 = 64;

/// How long the window's question to a helper may take before the helper
/// is passed over: a mirror answers in 3 ms, 144 ms the first time after an
/// update copied it (Codex t-77525, measured 10-27).
pub const CTTY_PROBE_LIMIT: Duration = Duration::from_secs(2);

/// The question [`launch_through`] put to a helper, and its answer once in.
struct Naming {
    answer: OnceLock<Option<Helper>>,
    lock: Mutex<()>,
    answered: Condvar,
    limit: Duration,
    deadline: Instant,
}

static NAMING: OnceLock<Arc<Naming>> = OnceLock::new();

/// Start every lane's child through `helper` from now on: an executable
/// whose `main` calls [`serve_ctty_exec`] before anything else. `log` is
/// the file its one line goes to when `TIOCSCTTY` fails.
///
/// Returns at once: the helper is first asked [`CTTY_PROBE_ENV`] on a
/// thread of its own, within [`CTTY_PROBE_LIMIT`], and named only when it
/// answers in time — one that does not know the road would keep the marker
/// in every lane's program. When it is not named, `unnamed` hears why, on
/// that thread, after the question's process is ended and reaped. A lane
/// started before the answer waits for it (`spawn`). Asked once per
/// process; `false` when it was asked already. A process that names none
/// starts its children straight, with no controlling terminal.
pub fn launch_through(
    helper: PathBuf,
    log: Option<PathBuf>,
    unnamed: impl FnOnce(String) + Send + 'static,
) -> bool {
    launch_through_within(helper, log, CTTY_PROBE_LIMIT, unnamed)
}

/// [`launch_through`] with a limit of the caller's, for a test that cannot
/// wait out [`CTTY_PROBE_LIMIT`].
#[doc(hidden)]
pub fn launch_through_within(
    helper: PathBuf,
    log: Option<PathBuf>,
    limit: Duration,
    unnamed: impl FnOnce(String) + Send + 'static,
) -> bool {
    let mut asked = false;
    NAMING.get_or_init(|| {
        asked = true;
        Naming::ask(helper, log, limit, unnamed)
    });
    asked
}

impl Naming {
    /// Put the question to `helper` on a thread of its own, and answer the
    /// naming it will settle.
    fn ask(
        helper: PathBuf,
        log: Option<PathBuf>,
        limit: Duration,
        unnamed: impl FnOnce(String) + Send + 'static,
    ) -> Arc<Self> {
        let naming = Arc::new(Self {
            answer: OnceLock::new(),
            lock: Mutex::new(()),
            answered: Condvar::new(),
            limit,
            deadline: Instant::now() + limit,
        });
        // Shared, so a thread that will not start still leaves the words.
        let question = Arc::new(Mutex::new(Some((helper, log, unnamed))));
        let asking = Arc::clone(&naming);
        let taken = Arc::clone(&question);
        let started = std::thread::Builder::new()
            .name("zerocode-ctty-probe".to_string())
            .spawn(move || {
                let question = taken.lock().unwrap_or_else(PoisonError::into_inner).take();
                if let Some((helper, log, unnamed)) = question {
                    let answer = probe(&helper, asking.deadline, asking.limit).map(|()| Helper {
                        program: helper,
                        log,
                    });
                    asking.settle(answer, unnamed);
                }
            });
        if let Err(error) = started {
            let question = question
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some((helper, _, unnamed)) = question {
                naming.settle(
                    Err(format!(
                        "{} was not asked: no thread to ask on: {error}",
                        helper.display()
                    )),
                    unnamed,
                );
            }
        }
        naming
    }

    /// Write the answer down and wake every lane waiting on it. An answer
    /// in after the limit is no answer: a lane that gave up waiting has
    /// started straight already, and every later one starts as it did.
    ///
    /// The line a failed question leaves is said after the lock is let go:
    /// it is a write to the window's log, and a write that does not return
    /// would hold every waiting lane past the limit (t-77569).
    fn settle(&self, answer: Result<Helper, String>, unnamed: impl FnOnce(String)) {
        let said = {
            let _held = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            let (helper, said) = match answer {
                Ok(helper) if Instant::now() < self.deadline => (Some(helper), None),
                Ok(helper) => {
                    let late = format!(
                        "{} answered after {:?}",
                        helper.program.display(),
                        self.limit
                    );
                    (None, Some(late))
                }
                Err(why) => (None, Some(why)),
            };
            let _ = self.answer.set(helper);
            self.answered.notify_all();
            said
        };
        if let Some(said) = said {
            unnamed(said);
        }
    }

    /// The helper a lane starts through: the answer, waited for until the
    /// question's limit at the latest; none once the limit is past with no
    /// answer in. The same lock as [`Naming::settle`], so no lane starts
    /// straight while a later one starts through a helper named after it.
    fn helper(&self) -> Option<&Helper> {
        if let Some(answer) = self.answer.get() {
            return answer.as_ref();
        }
        let mut held = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(answer) = self.answer.get() {
                return answer.as_ref();
            }
            let left = self.deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            held = self
                .answered
                .wait_timeout(held, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

/// Whether `helper` answers [`CTTY_PROBE_ENV`] as [`serve_ctty_exec`] does,
/// by `deadline`; why not when it does not. It starts in a group of its
/// own, and a question still open at the deadline ends the group before
/// its process is reaped — every road reaps it.
fn probe(helper: &Path, deadline: Instant, limit: Duration) -> Result<(), String> {
    use std::io::Read as _;
    use std::os::unix::process::CommandExt as _;
    let shown = helper.display();
    let mut child = std::process::Command::new(helper)
        .env(CTTY_PROBE_ENV, "1")
        .env_remove(CTTY_EXEC_ENV)
        .env_remove(CTTY_LOG_ENV)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| format!("{shown} does not start: {error}"))?;
    // Read on a thread of its own, so the wait for it has a limit. All of
    // it comes when the helper ends; a process it left holding the pipe
    // outside its group would keep only that thread.
    let (told, said) = std::sync::mpsc::channel();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::thread::Builder::new()
            .name("zerocode-ctty-probe-read".to_string())
            .spawn(move || {
                let mut bytes = Vec::new();
                let _ = (&mut out).take(CTTY_PROBE_READ).read_to_end(&mut bytes);
                let _ = told.send(bytes);
            });
    }
    let said = said.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    let ended = said
        .as_ref()
        .ok()
        .and_then(|_| ended_by(&mut child, deadline));
    if ended.is_none() {
        if let Ok(group) = libc::pid_t::try_from(child.id()) {
            // SAFETY: the question leads its own group and is not reaped
            // yet, so that group is still the question's.
            unsafe { libc::killpg(group, libc::SIGKILL) };
        }
        let _ = child.wait();
    }
    match (said, ended) {
        (Ok(_), Some(status)) if !status.success() => Err(format!(
            "{shown} ended with {status}, not as a pane's helper"
        )),
        (Ok(said), Some(_)) if said != CTTY_PROBE_ANSWER.as_bytes() => Err(format!(
            "{shown} answered {:?}, not as a pane's helper",
            String::from_utf8_lossy(&said)
        )),
        (Ok(_), Some(_)) => Ok(()),
        _ => Err(format!("{shown} did not answer within {limit:?}")),
    }
}

/// How `child` ended, when it ends by `deadline`; looked at every
/// millisecond, which the answer it gave has made a matter of one look.
fn ended_by(
    child: &mut std::process::Child,
    deadline: Instant,
) -> Option<std::process::ExitStatus> {
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            _ => return None,
        }
    }
}

/// Whether the window wraps `name` on a start through a helper: a `DYLD_*`
/// name, or one of those wrapped any number of times over — a name the
/// pane was given that a wrapped one would land on ([`CTTY_DYLD_WRAP`]).
fn wraps(name: &[u8]) -> bool {
    let mut name = name;
    while let Some(inner) = name.strip_prefix(CTTY_DYLD_WRAP.as_bytes()) {
        name = inner;
    }
    name.starts_with(b"DYLD_")
}

/// The name a [`CTTY_DYLD_WRAP`]-ped name had, when `name` is one.
fn unwrapped(name: &OsStr) -> Option<&OsStr> {
    name.as_bytes()
        .strip_prefix(CTTY_DYLD_WRAP.as_bytes())
        .filter(|original| wraps(original))
        .map(OsStr::from_bytes)
}

/// Every name in `env` the window [`wraps`] under its
/// [`CTTY_DYLD_WRAP`]-ped one.
fn wrap_dyld(env: &mut [(OsString, OsString)]) {
    for (name, _) in env {
        if wraps(name.as_bytes()) {
            let mut wrapped = OsString::from(CTTY_DYLD_WRAP);
            wrapped.push(&*name);
            *name = wrapped;
        }
    }
}

/// The helper's side of [`launch_through`]: when this process was started
/// as a lane's helper, it makes descriptor 0 — the pane's terminal, which
/// the spawn opened — its controlling terminal, and becomes the program in
/// this same process, so the pid and beginning the window wrote down stay
/// the program's. Otherwise it returns at once.
///
/// A terminal that will not be taken does not stop the program: it starts
/// as it would have without the helper, and the log gets one line. A program
/// that cannot be started ends this process with 127 and says why on the
/// pane, as a shell does.
///
/// Asked [`CTTY_PROBE_ENV`], it answers and ends. A marker with no
/// `CTTY_EXEC_ARG0` beside it was left by another helper: it is taken out
/// of this process's environment, so nothing this process starts holds it,
/// and this returns to the binary's own road.
///
/// Called first in `main`, before the process starts a thread: it changes
/// the process's environment.
pub fn serve_ctty_exec() {
    if std::env::var_os(CTTY_PROBE_ENV).is_some() {
        use std::io::Write as _;
        let mut out = io::stdout();
        let _ = out.write_all(CTTY_PROBE_ANSWER.as_bytes());
        let _ = out.flush();
        std::process::exit(0);
    }
    let Some(name) = std::env::var_os(CTTY_EXEC_ENV) else {
        return;
    };
    let mut args = std::env::args_os();
    if args.next().as_deref() != Some(OsStr::new(CTTY_EXEC_ARG0)) {
        forget_leftovers();
        return;
    }
    let Some(program) = args.next() else {
        eprintln!("zerocode: {CTTY_EXEC_ENV} is set, and no program was named");
        std::process::exit(127);
    };
    // SAFETY: `TIOCSCTTY` takes no argument beyond the descriptor, and its
    // answer is all that is read.
    let taken = unsafe { libc::ioctl(0, libc::c_ulong::from(libc::TIOCSCTTY), 0) };
    if taken != 0 {
        let error = io::Error::last_os_error();
        if let Some(log) = std::env::var_os(CTTY_LOG_ENV) {
            note_untaken(Path::new(&log), Path::new(&program), &error);
        }
    }
    use std::os::unix::process::CommandExt as _;
    let mut command = std::process::Command::new(&program);
    command
        .arg0(name)
        .args(args)
        .env_remove(CTTY_EXEC_ENV)
        .env_remove(CTTY_LOG_ENV);
    for (name, value) in given_back(std::env::vars_os()) {
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    let error = command.exec();
    eprintln!("zerocode: {}: {error}", Path::new(&program).display());
    std::process::exit(127);
}

/// What the helper gives the program back, in order: every name the window
/// wrapped taken out, then each value under the name it had — which may be
/// one of the names just taken out, when the pane was given a name of the
/// wrapped shape.
fn given_back(vars: impl Iterator<Item = (OsString, OsString)>) -> Vec<EnvEdit> {
    let mut edits = Vec::new();
    let mut back = Vec::new();
    for (name, value) in vars {
        if let Some(original) = unwrapped(&name) {
            back.push((original.to_os_string(), Some(value)));
            edits.push((name, None));
        }
    }
    edits.extend(back);
    edits
}

/// Take the markers an older helper left in this environment out of it.
/// A name of the wrapped shape stays: the window wraps only on a start
/// through a helper, which gives every one back, so here it is one the
/// person gave.
fn forget_leftovers() {
    let names: Vec<OsString> = std::env::vars_os()
        .map(|(name, _)| name)
        .filter(|name| name == CTTY_EXEC_ENV || name == CTTY_LOG_ENV)
        .collect();
    for name in names {
        // SAFETY: `serve_ctty_exec` is called first in `main`, before the
        // process has a second thread to read the environment.
        unsafe { std::env::remove_var(name) };
    }
}

/// One line in the window's log, in its own shape — milliseconds since the
/// epoch, then the words — and in one write, which `O_APPEND` keeps whole.
fn note_untaken(log: &Path, program: &Path, error: &io::Error) {
    use std::io::Write as _;
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let line = format!(
        "{millis} pane launch: {} starts without a controlling terminal: {error}\n",
        program.display()
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// Start `program` with `args` on the pty whose slave is `tty` — through
/// the helper when one is named ([`launch_through`]), so the slave becomes
/// its controlling terminal. A helper that will not start (gone from its
/// path since it was named) is passed over for the program itself: a pane
/// with no controlling terminal is the pane every start had before, a pane
/// with no program is none.
///
/// Started while the helper's question is open, a pane waits for the
/// answer, never past the question's limit, and then starts straight
/// ([`Naming::helper`]). Panes start on the window's command threads,
/// never its main one; the panes a boot brings back start after its first
/// paint, which the question no longer holds, so with a helper that
/// answers in its usual 3 ms they come through it.
pub(crate) fn spawn(
    tty: &Path,
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
    edits: &[EnvEdit],
) -> io::Result<SpawnedChild> {
    if let Some(helper) = NAMING.get().and_then(|naming| naming.helper())
        && let Ok(child) = spawn_with(tty, program, args, cwd, edits, Some(helper))
    {
        return Ok(child);
    }
    spawn_with(tty, program, args, cwd, edits, None)
}

fn spawn_with(
    tty: &Path,
    program: &OsStr,
    args: &[OsString],
    cwd: Option<&Path>,
    edits: &[EnvEdit],
    helper: Option<&Helper>,
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
    env.retain(|(named, _)| named != CTTY_EXEC_ENV && named != CTTY_LOG_ENV);

    // Through a helper, the helper is what starts, under its own `argv[0]`,
    // with the program's path as its first argument, and the program's own
    // name and its `DYLD_*` values in its environment.
    let (path, mut argv) = match helper {
        Some(helper) => {
            wrap_dyld(&mut env);
            env.push((CTTY_EXEC_ENV.into(), program.to_os_string()));
            if let Some(log) = &helper.log {
                env.push((CTTY_LOG_ENV.into(), log.clone().into_os_string()));
            }
            (
                c_string(helper.program.as_os_str().as_bytes())?,
                vec![
                    c_string(CTTY_EXEC_ARG0.as_bytes())?,
                    c_string(resolved.as_os_str().as_bytes())?,
                ],
            )
        }
        None => (
            c_string(resolved.as_os_str().as_bytes())?,
            vec![c_string(program.as_bytes())?],
        ),
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The line a failed question leaves is said once the answer is written
    /// and the lock is let go. Said under the lock, a window log that does
    /// not take the write held every lane waiting on the answer past the
    /// question's limit — 4.4 s against a 2 s limit (t-77569).
    #[test]
    fn a_failed_question_is_said_with_the_answer_written_and_the_lock_let_go() {
        for answer in [
            Err("the helper did not answer".to_string()),
            Ok(Helper {
                program: PathBuf::from("/helper"),
                log: None,
            }),
        ] {
            let naming = Arc::new(Naming {
                answer: OnceLock::new(),
                lock: Mutex::new(()),
                answered: Condvar::new(),
                limit: Duration::ZERO,
                deadline: Instant::now(),
            });
            let seen = Arc::new(Mutex::new(None));
            let (watched, record) = (Arc::clone(&naming), Arc::clone(&seen));
            naming.settle(answer, move |_| {
                let free = watched.lock.try_lock().is_ok();
                let written = watched.answer.get().is_some();
                *record.lock().unwrap_or_else(PoisonError::into_inner) = Some((free, written));
            });
            assert_eq!(
                *seen.lock().unwrap_or_else(PoisonError::into_inner),
                Some((true, true)),
                "(lock free, answer written) as the line was said"
            );
        }
    }

    /// What the helper is handed holds no `DYLD_*` name — dyld would take
    /// it from a hardened helper — and every value comes back under its own
    /// name, with no wrapped name left (t-77495).
    #[test]
    fn a_dyld_value_crosses_the_helper_under_another_name() {
        let mut env: Vec<(OsString, OsString)> = vec![
            ("DYLD_LIBRARY_PATH".into(), "/opt/lib".into()),
            ("PATH".into(), "/bin".into()),
            ("DYLD_INSERT_LIBRARIES".into(), "/opt/a.dylib".into()),
        ];
        wrap_dyld(&mut env);
        assert!(
            env.iter()
                .all(|(name, _)| !name.as_bytes().starts_with(b"DYLD_")),
            "{env:?}"
        );
        let given_back: Vec<(&OsStr, &OsString)> = env
            .iter()
            .filter_map(|(name, value)| unwrapped(name).map(|original| (original, value)))
            .collect();
        assert_eq!(
            given_back,
            [
                (OsStr::new("DYLD_LIBRARY_PATH"), &OsString::from("/opt/lib")),
                (
                    OsStr::new("DYLD_INSERT_LIBRARIES"),
                    &OsString::from("/opt/a.dylib")
                ),
            ]
        );
        assert_eq!(unwrapped(OsStr::new(CTTY_EXEC_ENV)), None);
        assert_eq!(unwrapped(OsStr::new("ZEROCODE_CTTY_PATH")), None);
    }

    /// A person's own name of the wrapped shape — given in the settings'
    /// environment field — crosses the helper as given, beside a `DYLD_*`
    /// value the window wraps into the very same name: the helper gets
    /// back only what the window wrapped (Codex t-77525).
    #[test]
    fn a_persons_name_of_the_wrapped_shape_crosses_the_helper_as_given() {
        let mut given: Vec<(OsString, OsString)> = [
            ("DYLD_LIBRARY_PATH", "/opt/lib"),
            ("ZEROCODE_CTTY_DYLD_LIBRARY_PATH", "user-own"),
            ("ZEROCODE_CTTY_DYLD_REVIEW_CUSTOM", "user-setting"),
            ("ZEROCODE_CTTY_ZEROCODE_CTTY_DYLD_X", "deep"),
            ("ZEROCODE_CTTY_NOT_DYLD", "keep-this"),
        ]
        .into_iter()
        .map(|(name, value)| (name.into(), value.into()))
        .collect();
        let mut env = given.clone();
        wrap_dyld(&mut env);
        // What a hardened helper holds.
        env.retain(|(name, _)| !name.as_bytes().starts_with(b"DYLD_"));
        let mut names: Vec<&OsString> = env.iter().map(|(name, _)| name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), env.len(), "two values under one name: {env:?}");
        let mut back = env.clone();
        for (name, value) in given_back(env.into_iter()) {
            back.retain(|(named, _)| *named != name);
            if let Some(value) = value {
                back.push((name, value));
            }
        }
        back.sort();
        given.sort();
        assert_eq!(back, given);
    }
}
