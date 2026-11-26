//! A child and every process it started, ended together (t-98207).
//!
//! `Child::kill` ends one process, and a CLI npm installs is two or three
//! deep: on Windows its `.cmd`, which std starts through `cmd.exe`, runs
//! node, and node runs the CLI's own binary; on unix the node script runs
//! that binary. Ending the child alone left the rest running, holding the
//! pipe a reader waits on. So a child that may have to be ended leads a
//! tree of its own ([`lead_own_tree`]) and [`end_tree`] ends the tree: on
//! unix its process group, on Windows `taskkill /T`, which follows
//! parentage — the way ZO's build runner ends a step's `cmd` (`end_tree`).
//!
//! `taskkill` starts with no console window of its own (`CREATE_NO_WINDOW`,
//! 152): the window, which has no console, ends its wires' trees through here,
//! and a bare `taskkill` flashed a black window over the person's screen for
//! each one (AO #5309). Its input and output are nobody's, so a console
//! program loses nothing by it either. The window's own `taskkill`s go through
//! its quiet door (`proc.rs`), with the same [`TASKKILL_TREE`] words.
//!
//! Whether one process has ended, by its pid, is asked here too ([`ended`]),
//! on unix and on Windows alike: `zo line` asks it of the window a pane's seat
//! came from (t-131644).

use std::io::Read as _;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// `taskkill`'s words for one pid and every process it started, forced: the
/// pid follows them. Spelled once for every Windows caller that ends a tree.
pub const TASKKILL_TREE: [&str; 3] = ["/T", "/F", "/PID"];

/// How long [`end_tree`] waits for `taskkill` before it lets go of it. It
/// answers in a moment; one that does not must not hold the caller longer
/// than this. Chosen, not measured: there is no Windows machine here.
#[cfg(windows)]
const TASKKILL_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// How often [`end_tree`] looks whether `taskkill` has ended.
#[cfg(windows)]
const TASKKILL_LOOK_EVERY: std::time::Duration = std::time::Duration::from_millis(10);

/// The most [`end_tree`] holds its caller: on Windows `taskkill`'s wait.
#[cfg(windows)]
const END_TREE_MOST: Duration = TASKKILL_WAIT;

/// The most [`end_tree`] holds its caller: on unix nothing to speak of — a
/// `SIGKILL` to the group, which no process refuses, and the collection of
/// its head.
#[cfg(not(windows))]
const END_TREE_MOST: Duration = Duration::ZERO;

/// `winbase.h`'s `CREATE_NO_WINDOW`: a console child started without a
/// console window — the flag the window's `proc::quiet_command` sets too.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Make the child `command` starts the head of a tree [`end_tree`] can end:
/// on unix the leader of a process group of its own, which every process it
/// starts joins unless it leaves; on Windows nothing, since `taskkill /T`
/// follows parentage. A group of its own is not the terminal's foreground
/// one, so this is for a child that never reads the terminal.
pub fn lead_own_tree(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    {
        let _ = command;
    }
}

/// How often [`run_bounded`] looks whether its child has ended.
const LOOK_EVERY: Duration = Duration::from_millis(10);

/// The most of one output [`run_bounded`] reads — a bound for a writer that
/// never stops, set far above every answer measured: a CLI's help is
/// kilobytes, and the largest model list seen, devin's 1,302 models
/// (2026-11-21), was 0.68 MB.
const OUTPUT_CAP: u64 = 16 * 1024 * 1024;

/// What one bounded run of a command returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ran {
    /// It ended with this code (`None`: by a signal), and what it wrote on
    /// each output that was piped — `None` for one that was not, or that
    /// did not finish reaching this side within what was left of the limit.
    Ended {
        code: Option<i32>,
        stdout: Option<String>,
        stderr: Option<String>,
    },
    /// It did not end within the limit, and was ended with every process it
    /// started ([`end_tree`]) — or a deadline left it no time, and it was
    /// never started ([`run_bounded_until`]).
    TimedOut,
    /// It could not be started.
    NotStarted(String),
}

/// Make `command` a question rather than a session: its input closed, and
/// its colour off whatever the person's shell exported, so a coloured
/// heading never reads as an answer without a list (t-97498 P3-A). Every
/// question [`run_bounded`] asks is built through here.
pub fn as_a_question(command: &mut Command) -> &mut Command {
    command
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .stdin(std::process::Stdio::null())
}

/// Ask `command` one question within `limit` — its outputs piped or not as
/// the caller set them — as the head of a tree of its own
/// ([`lead_own_tree`]): each piped output is read beside the wait (to
/// `OUTPUT_CAP`) so a long answer cannot stall it against a full pipe, a
/// run past the limit is ended whole, and an output that has not reached
/// this side when the run ends is waited for no longer than the limit's
/// rest. The one bounded question this crate asks a CLI with (t-124593):
/// the mirror's help question and a CLI's model list.
pub fn run_bounded(command: &mut Command, limit: Duration) -> Ran {
    lead_own_tree(command);
    let began = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Ran::NotStarted(error.to_string()),
    };
    let stdout = child.stdout.take().map(read_beside);
    let stderr = child.stderr.take().map(read_beside);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let heard = |output: Option<std::sync::mpsc::Receiver<String>>| {
                    output?
                        .recv_timeout(limit.saturating_sub(began.elapsed()))
                        .ok()
                };
                return Ran::Ended {
                    code: status.code(),
                    stdout: heard(stdout),
                    stderr: heard(stderr),
                };
            }
            Ok(None) if began.elapsed() < limit => std::thread::sleep(LOOK_EVERY),
            _ => {
                end_tree(&mut child);
                return Ran::TimedOut;
            }
        }
    }
}

/// [`run_bounded`], over by `until` with the end of a run that runs out in it
/// (t-131644): the question is given what is left of `until` but the most
/// [`end_tree`] may hold the caller, so several questions under one deadline
/// cannot add up past it. A deadline with no time left for a question starts
/// none, and is [`Ran::TimedOut`].
#[must_use]
pub fn run_bounded_until(command: &mut Command, until: Instant) -> Ran {
    let left = until
        .saturating_duration_since(Instant::now())
        .saturating_sub(END_TREE_MOST);
    if left.is_zero() {
        return Ran::TimedOut;
    }
    run_bounded(command, left)
}

/// Read one output to its end, or [`OUTPUT_CAP`], on a thread of its own.
fn read_beside(pipe: impl std::io::Read + Send + 'static) -> std::sync::mpsc::Receiver<String> {
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = pipe.take(OUTPUT_CAP).read_to_end(&mut bytes);
        let _ = said.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    heard
}

/// End `child`, started with [`lead_own_tree`], and every process it
/// started, then collect it. Its pid cannot have been taken by anyone else:
/// the child is collected only at the end.
pub fn end_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(group) = i32::try_from(child.id()) {
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(group),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
    }
    #[cfg(windows)]
    end_tree_by_taskkill(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

/// `taskkill /T /F /PID <pid>`, waited for [`TASKKILL_WAIT`] at most.
#[cfg(windows)]
fn end_tree_by_taskkill(pid: u32) {
    end_trees(&[pid]);
}

/// `taskkill /T /F /PID <pid>` for every pid at once — each tree's ending
/// started before any is waited for — then [`TASKKILL_WAIT`] at most for all
/// of them together (152): several trees wait once, not once a tree.
#[cfg(windows)]
pub fn end_trees(pids: &[u32]) {
    let mut running: Vec<Child> = pids.iter().filter_map(|pid| taskkill_tree(*pid)).collect();
    let until = std::time::Instant::now() + TASKKILL_WAIT;
    while running
        .iter_mut()
        .any(|taskkill| matches!(taskkill.try_wait(), Ok(None)))
        && std::time::Instant::now() < until
    {
        std::thread::sleep(TASKKILL_LOOK_EVERY);
    }
    for mut taskkill in running {
        let _ = taskkill.kill();
        let _ = taskkill.wait();
    }
}

/// `taskkill /T /F /PID <pid>` for every pid, waited for by nobody (152): a
/// window on its way out ends its wires' trees without holding the way out.
#[cfg(windows)]
pub fn start_ending_trees(pids: &[u32]) {
    for pid in pids {
        drop(taskkill_tree(*pid));
    }
}

/// The one `taskkill` this file starts: `/T /F /PID <pid>`, with no console
/// window of its own ([`CREATE_NO_WINDOW`]) and nobody's input or output.
#[cfg(windows)]
fn taskkill_tree(pid: u32) -> Option<Child> {
    use std::os::windows::process::CommandExt as _;
    use std::process::Stdio;
    Command::new("taskkill")
        .args(TASKKILL_TREE)
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .ok()
}

/// Whether process `pid` has ended, as far as this host can tell
/// (t-131644): on unix no process answers signal 0 by that pid; on Windows no
/// process opens by it, or the one that opens has exited — the two calls the
/// window asks it with (`resource_usage::process_exited`). A pid this cannot
/// ask about, or that the host will not answer for — 0, another user's, a
/// refusal it cannot read — has not ended: a caller acts only on an end it
/// saw. A pid a new process has taken since reads as standing, here as
/// everywhere a pid is asked after.
#[must_use]
pub fn ended(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        !crate::settings_file_lock::process_alive(pid)
    }
    #[cfg(windows)]
    {
        ended_on_windows(pid)
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

/// [`ended`] asked of Win32: `OpenProcess` for no more than waiting on the
/// process, then a wait that does not wait.
#[cfg(windows)]
fn ended_on_windows(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    // SAFETY: a query by pid with no pointer in it; the handle it returns is closed below.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        // The one refusal that says no process has the pid: denied says one does.
        return std::io::Error::last_os_error().raw_os_error()
            == i32::try_from(ERROR_INVALID_PARAMETER).ok();
    }
    // SAFETY: the handle opened above, still open; a wait of no time.
    let read = unsafe { WaitForSingleObject(process, 0) };
    // SAFETY: closing the handle opened above, once.
    unsafe {
        CloseHandle(process);
    }
    read == WAIT_OBJECT_0
}

/// [`ended`] on every host this builds for — the Windows cross-compile builds
/// it too, where it asks Win32: a child that was collected has ended, this
/// process has not, and pid 0 says nothing.
#[cfg(test)]
mod an_ended_process {
    #[test]
    fn is_told_from_one_that_stands() {
        #[cfg(unix)]
        let mut command = std::process::Command::new("true");
        #[cfg(windows)]
        let mut command = {
            let mut command = std::process::Command::new("cmd");
            command.args(["/C", "exit"]);
            command
        };
        let pid = {
            let mut child = command
                .stdin(std::process::Stdio::null())
                .spawn()
                .expect("a child");
            child.wait().expect("it ends");
            child.id()
        };
        assert!(super::ended(pid), "{pid} ended and was collected");
        assert!(!super::ended(std::process::id()), "this process stands");
        assert!(!super::ended(0), "pid 0 says nothing");
    }
}

/// Unix only: the Windows tree is ended by `taskkill`, which this machine
/// cannot run, so there is nothing here for a Windows build to compile.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// What a child started leaves running is ended with it: the shape of
    /// npm's codex, a script whose binary runs under it. The kernel is asked
    /// about the grandchild itself, by its pid.
    #[test]
    fn a_tree_ends_with_what_its_head_started() {
        use std::io::BufRead as _;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 30 & echo $!; wait"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped());
        lead_own_tree(&mut command);
        let mut child = command.spawn().expect("a head with a grandchild");
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().expect("its output"))
            .read_line(&mut line)
            .expect("the grandchild's pid");
        let grandchild: u32 = line.trim().parse().expect("a pid");
        assert!(crate::settings_file_lock::process_alive(grandchild));

        end_tree(&mut child);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::settings_file_lock::process_alive(grandchild)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !crate::settings_file_lock::process_alive(grandchild),
            "the grandchild {grandchild} outlived its tree"
        );
    }

    /// A deadline is the whole wait (t-131644): a question that outlives it
    /// is ended by it, and one asked with no time left is never started.
    #[test]
    fn a_question_is_over_by_its_deadline_and_one_with_no_time_left_never_starts() {
        let began = Instant::now();
        let ran = run_bounded_until(
            Command::new("sleep").arg("30"),
            began + Duration::from_millis(300),
        );
        assert_eq!(ran, Ran::TimedOut);
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "a 300 ms deadline held the caller {:?}",
            began.elapsed()
        );

        let folder = tempfile::tempdir().expect("a folder");
        let mark = folder.path().join("started");
        let ran = run_bounded_until(
            Command::new("/bin/sh")
                .args(["-c", "touch \"$0\""])
                .arg(&mark),
            Instant::now(),
        );
        assert_eq!(ran, Ran::TimedOut);
        std::thread::sleep(Duration::from_millis(200));
        assert!(!mark.exists(), "a question with no time left was started");
    }
}
