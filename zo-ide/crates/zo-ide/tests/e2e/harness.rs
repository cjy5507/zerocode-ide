//! Process drivers shared by the hermetic end-to-end tests.
//!
//! The PTY path deliberately mirrors the capture scripts: a fresh 120x40
//! terminal, bytes written to the master, and a background reader that keeps
//! the child from blocking on a full terminal buffer.  The child receives a
//! private Zo home/session/state tree and a loopback provider URL.

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::fcntl::{fcntl, FcntlArg, FdFlag};
use nix::poll::{poll, PollFd, PollFlags};
use nix::pty::{openpty, Winsize};

/// 120x40 mirrors the capture scripts. `ZO_E2E_ROWS` overrides it for the
/// repaint measurement, which needs a screen the viewport can actually reach
/// the bottom of — on 40 rows a short scripted turn never gets there, so the
/// scrolling-growth path goes unexercised.
fn pty_rows() -> u16 {
    std::env::var("ZO_E2E_ROWS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|rows| *rows >= 8)
        .unwrap_or(PTY_ROWS)
}

const PTY_ROWS: u16 = 40;
const PTY_COLS: u16 = 120;

/// `ZO_E2E_COLS` overrides the width the same way `ZO_E2E_ROWS` overrides the
/// height — a report from an IDE pane arrived at 176 columns, and a table that
/// renders at 120 says nothing about 176.
fn pty_cols() -> u16 {
    std::env::var("ZO_E2E_COLS")
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|cols| *cols >= 40)
        .unwrap_or(PTY_COLS)
}

/// Whether the pty is zo's controlling terminal or only its standard streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    /// zo's stdio is the pty, in a new session with no controlling terminal.
    Detached,
    /// zo leads its own session and the pty is that session's terminal.
    Controlling,
}

/// `ZO_E2E_CTTY`, when present, names the new session's controlling terminal.
/// It is removed before the program starts.
const TERMINAL_SESSION_WRAPPER: &str = "use POSIX (); \
POSIX::setsid() or die qq(setsid: $!); \
if (defined(my $tty = delete $ENV{ZO_E2E_CTTY})) { \
open(my $fh, q(+<), $tty) or die qq(open $tty: $!); close $fh; } \
exec @ARGV or die qq(exec: $!);";
const CHILD_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the reader waits for bytes before it looks again at whether the
/// test has hung the terminal up. A read blocked on the master would hold the
/// terminal open past the moment a test closed it — the one thing
/// [`PtyRun::hang_up`] exists to do.
const READ_WAIT_MS: u16 = 50;

fn terminal_command(
    binary: &Path,
    terminal: Terminal,
    slave_path: Option<&Path>,
) -> io::Result<Command> {
    // This crate forbids unsafe pre_exec. Both modes need setsid: otherwise
    // Crossterm can read the runner's /dev/tty instead of the test's stdio PTY.
    let mut command = Command::new("perl");
    command
        .arg("-e")
        .arg(TERMINAL_SESSION_WRAPPER)
        .arg("--")
        .arg(binary)
        .env_remove("ZO_E2E_CTTY");
    if terminal == Terminal::Controlling {
        let slave_path =
            slave_path.ok_or_else(|| io::Error::other("the pty slave has no name"))?;
        command.env("ZO_E2E_CTTY", slave_path);
    }
    Ok(command)
}

#[derive(Default)]
struct VisibilityProbe {
    submitted: Option<Instant>,
    first_byte: Option<Instant>,
}

/// The `zo` under test: the one cargo built, unless `ZO_E2E_BIN` names
/// another — a baseline binary kept aside, so the same scenario can be
/// replayed before and after a change for a screen-dump comparison.
fn zo_binary() -> PathBuf {
    std::env::var_os("ZO_E2E_BIN")
        .map_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_zo")), PathBuf::from)
}

/// A child connected to a real PTY master.
pub struct PtyRun {
    writer: Option<File>,
    human_inputs: u64,
    visibility: Arc<Mutex<VisibilityProbe>>,
    output: Arc<Mutex<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    /// Tells the reader to let go of its copy of the master.
    hung_up: Arc<AtomicBool>,
    child: Child,
    /// The slave device, so a test can resize the terminal the way a window
    /// does (`stty -f <slave> rows R cols C` → `TIOCSWINSZ` → `SIGWINCH`).
    /// Read by the pane suite (`e2e_pane_resize_and_wide_table.rs`), not by
    /// this binary — the harness is shared and each binary uses a part of it.
    #[allow(dead_code)]
    slave_path: Option<std::path::PathBuf>,
}

impl PtyRun {
    /// Start the built `zo` binary with all credentials and state redirected
    /// into the supplied temporary directories.
    pub fn spawn(
        cwd: &Path,
        home: &Path,
        sessions: &Path,
        state: &Path,
        base_url: &str,
        args: &[&str],
    ) -> io::Result<Self> {
        Self::spawn_with_env(cwd, home, sessions, state, base_url, args, &[])
    }

    /// [`Self::spawn`] plus extra environment for the child.
    ///
    /// The measurement runs need knobs the contract tests never set — the
    /// compaction window above all — and threading them through a global
    /// `std::env::set_var` would leak across the `--test-threads=1` suite and
    /// silently re-scale every later scenario.
    pub fn spawn_with_env(
        cwd: &Path,
        home: &Path,
        sessions: &Path,
        state: &Path,
        base_url: &str,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> io::Result<Self> {
        Self::spawn_sized(
            pty_rows(),
            pty_cols(),
            cwd,
            home,
            sessions,
            state,
            base_url,
            args,
            extra_env,
        )
    }

    /// [`Self::spawn_with_env`] at an explicit size, for a scenario that
    /// starts small and grows — an IDE pane that is resized under a running
    /// zo (see `e2e_pane_resize_and_wide_table.rs`).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_sized(
        rows: u16,
        cols: u16,
        cwd: &Path,
        home: &Path,
        sessions: &Path,
        state: &Path,
        base_url: &str,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> io::Result<Self> {
        Self::spawn_inner(
            rows,
            cols,
            cwd,
            home,
            sessions,
            state,
            base_url,
            args,
            extra_env,
            Terminal::Detached,
        )
    }

    /// [`Self::spawn`] with the pty as zo's *controlling* terminal, the way a
    /// pane's shell or the window hands one over: zo leads a session of its
    /// own and the pty is that session's terminal. That is what lets a child
    /// of zo reach the terminal through `/dev/tty` — and take it — so the
    /// scenarios about the terminal's foreground group run here; the plain
    /// spawn gives zo a tty on its stdio but no controlling terminal, and a
    /// child there finds no `/dev/tty` to take.
    pub fn spawn_controlling(
        cwd: &Path,
        home: &Path,
        sessions: &Path,
        state: &Path,
        base_url: &str,
        args: &[&str],
    ) -> io::Result<Self> {
        Self::spawn_controlling_sized(
            pty_rows(),
            pty_cols(),
            cwd,
            home,
            sessions,
            state,
            base_url,
            args,
            &[],
        )
    }

    /// [`Self::spawn_sized`] with the pty as zo's controlling terminal.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_controlling_sized(
        rows: u16,
        cols: u16,
        cwd: &Path,
        home: &Path,
        sessions: &Path,
        state: &Path,
        base_url: &str,
        args: &[&str],
        extra_env: &[(&str, &str)],
    ) -> io::Result<Self> {
        Self::spawn_inner(
            rows,
            cols,
            cwd,
            home,
            sessions,
            state,
            base_url,
            args,
            extra_env,
            Terminal::Controlling,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_inner(
        rows: u16,
        cols: u16,
        cwd: &Path,
        home: &Path,
        sessions: &Path,
        state: &Path,
        base_url: &str,
        args: &[&str],
        extra_env: &[(&str, &str)],
        terminal: Terminal,
    ) -> io::Result<Self> {
        let winsize = Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pair = openpty(Some(&winsize), None::<&nix::sys::termios::Termios>)
            .map_err(io::Error::from)?;
        let master = File::from(pair.master);
        let slave = File::from(pair.slave);
        // Both ends close-on-exec, as the window's pty marks them
        // (portable_pty's `cloexec`). `openpty` hands them over inheritable:
        // the child kept a master of its own, and closing every copy the test
        // held left the terminal open — no hang-up, no EIO, none of what a
        // pane closing under a program is.
        for end in [&master, &slave] {
            fcntl(end.as_raw_fd(), FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC)).map_err(io::Error::from)?;
        }
        let slave_path = nix::unistd::ttyname(&slave).ok();

        let mut command = terminal_command(&zo_binary(), terminal, slave_path.as_deref())?;
        configure_command(
            &mut command,
            cwd,
            home,
            sessions,
            state,
            base_url,
            args,
        );
        for (name, value) in extra_env {
            command.env(name, value);
        }
        command
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        let child = command.spawn()?;

        let output = Arc::new(Mutex::new(Vec::new()));
        let reader_output = Arc::clone(&output);
        let visibility = Arc::new(Mutex::new(VisibilityProbe::default()));
        let reader_visibility = Arc::clone(&visibility);
        let reader_file = master.try_clone()?;
        let hung_up = Arc::new(AtomicBool::new(false));
        let reader_hung_up = Arc::clone(&hung_up);
        let reader = thread::spawn(move || {
            let mut reader = reader_file;
            let mut chunk = [0_u8; 16 * 1024];
            while !reader_hung_up.load(Ordering::Relaxed) {
                let ready = poll(
                    &mut [PollFd::new(reader.as_fd(), PollFlags::POLLIN)],
                    READ_WAIT_MS,
                );
                match ready {
                    Ok(0) | Err(Errno::EINTR) => continue,
                    Ok(_) => {},
                    Err(_) => break,
                }
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let mut bytes = reader_output.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        bytes.extend_from_slice(&chunk[..read]);
                        let mut probe = reader_visibility.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        if probe.submitted.is_some() && probe.first_byte.is_none() {
                            probe.first_byte = Some(Instant::now());
                        }
                    },
                }
            }
        });

        Ok(Self {
            writer: Some(master),
            human_inputs: 0,
            visibility,
            output,
            reader: Some(reader),
            hung_up,
            child,
            slave_path,
        })
    }

    /// Resize the PTY the way a window does when its pane is dragged. Done
    /// through `stty` on the slave device rather than an `ioctl` here: this
    /// crate forbids `unsafe`, and `stty` performs the same `TIOCSWINSZ`, after
    /// which the kernel delivers `SIGWINCH` to the child.
    #[allow(dead_code)] // pane suite only; see `slave_path`
    pub fn resize(&mut self, rows: u16, cols: u16) -> io::Result<()> {
        self.resize_silently(rows, cols)?;
        // A detached child's stdio PTY has no foreground process group.
        // Deliver SIGWINCH explicitly, as a window's controlling terminal would.
        let pid = nix::unistd::Pid::from_raw(
            i32::try_from(self.child.id()).map_err(|_| io::Error::other("pid overflow"))?,
        );
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGWINCH).map_err(io::Error::from)
    }

    /// [`Self::resize`] without the signal: the pty changes size and the child
    /// is told nothing — the shape of a host whose `SIGWINCH` never arrives.
    #[allow(dead_code)] // pane suite only; see `slave_path`
    pub fn resize_silently(&mut self, rows: u16, cols: u16) -> io::Result<()> {
        let slave = self
            .slave_path
            .as_ref()
            .ok_or_else(|| io::Error::other("slave device name unknown"))?;
        let status = Command::new("stty")
            .arg("-f")
            .arg(slave)
            .arg("rows")
            .arg(rows.to_string())
            .arg("cols")
            .arg(cols.to_string())
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!("stty exited with {status}")));
        }
        Ok(())
    }

    /// Write raw keyboard bytes to the PTY master.
    pub fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer closed"))?;
        writer.write_all(bytes)?;
        writer.flush()
    }

    /// Human-origin writes, separate from scripted fixture input and shutdown.
    #[allow(dead_code)]
    pub fn send_human(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.human_inputs += 1;
        self.send(bytes)
    }

    #[allow(dead_code)]
    pub fn interventions(&self) -> u64 { self.human_inputs }

    /// Arm a constant-size first-byte probe immediately before scripted input.
    #[allow(dead_code)]
    pub fn measure_submission(&self) {
        *self.visibility.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            VisibilityProbe { submitted: Some(Instant::now()), first_byte: None };
    }

    /// First PTY render byte after submission (includes composer echo).
    #[allow(dead_code)]
    pub fn first_visible_ms(&self) -> Option<f64> {
        let probe = self.visibility.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(probe.first_byte?.duration_since(probe.submitted?).as_secs_f64() * 1000.0)
    }

    /// The child's resident set size in KiB, or `None` when it has exited.
    ///
    /// Sampled through `ps` because the interesting question — does a long
    /// session grow without bound — is about the REAL process under a real
    /// terminal, not about an allocator counter inside a unit test.
    pub fn rss_kib(&self) -> Option<u64> {
        let out = Command::new("ps")
            .args(["-o", "rss=", "-p", &self.child.id().to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    /// The child's own cumulative CPU time in milliseconds, or `None` when it
    /// has exited.
    ///
    /// The wait/compute split a turn profile needs is `cpu / wall`, and `ps`
    /// is the only place that number exists for a process the test does not
    /// own the rusage of.  This counts the zo process itself — a `bash` tool
    /// child's CPU is reaped separately and is deliberately NOT zo's cost.
    pub fn cpu_ms(&self) -> Option<u64> {
        let out = Command::new("ps")
            .args(["-o", "time=", "-p", &self.child.id().to_string()])
            .output()
            .ok()?;
        parse_ps_time(String::from_utf8_lossy(&out.stdout).trim())
    }

    /// How many threads the child currently has (`ps -M` prints one row per
    /// thread after a header).  Sub-agents run on detached OS threads, so this
    /// is what says whether three children are really three.
    pub fn thread_count(&self) -> Option<usize> {
        let out = Command::new("ps")
            .args(["-M", "-p", &self.child.id().to_string()])
            .output()
            .ok()?;
        let rows = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        rows.checked_sub(2)
    }

    /// The child's process id, for an external profiler (`sample`).
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Wait until the child has emitted `needle` or fail with a bounded
    /// diagnostic.  This is output-driven rather than sleep-driven, so slow
    /// CI machines do not change the scenario ordering.
    pub fn wait_for(&mut self, needle: &str, timeout: Duration) -> usize {
        self.wait_for_after(needle, 0, timeout)
    }

    /// Wait for a new occurrence of `needle` after an already observed output
    /// offset.  Repeated assistant answers in a resumed session otherwise
    /// make a plain substring wait return before the next turn is idle.
    ///
    /// Returns the offset just past the match. A scenario that waits for the
    /// NEXT occurrence of the same words starts from that end, never from a
    /// fresh [`Self::output_len`] taken after this returns: the child does
    /// not wait for the 10 ms poll, so under load a whole queued turn — its
    /// answer included — can land in the capture between the match and the
    /// length read, and the "next" occurrence is then already behind the
    /// offset (lane #34, 2026-09-08: `R34_FOLLOW_UP_DONE` twice on screen,
    /// the wait still timing out).
    pub fn wait_for_after(&mut self, needle: &str, offset: usize, timeout: Duration) -> usize {
        let needle = needle.as_bytes();
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(end) = self.match_end_after(needle, offset) {
                return end;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                let snapshot = self.snapshot();
                panic!(
                    "zo exited ({status}) before emitting {needle:?}; output:\n{}",
                    printable(&snapshot)
                );
            }
            if Instant::now() >= deadline {
                let snapshot = self.snapshot();
                panic!(
                    "timed out waiting for {needle:?}; output:\n{}",
                    printable(&snapshot)
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Kill the child the way a window restart kills it: SIGKILL, no chance to
    /// flush, no `/exit`, no shutdown hook.
    ///
    /// `Child::kill` is SIGKILL on Unix, which is exactly the signal this
    /// models — a graceful TERM would let zo settle the turn and the scenario
    /// under test (a transcript frozen mid-turn) would never happen. The PTY
    /// reader is left attached so output written before the kill is still in
    /// the capture.
    pub fn kill9(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Ask the interactive child to exit, wait at most five seconds, close the
    /// PTY, and return the exact bytes read from it.
    pub fn finish(mut self) -> Vec<u8> {
        let running = self.child.try_wait().ok().flatten().is_none();
        if running {
            let _ = self.send(b"/exit\r");
            let deadline = Instant::now() + CHILD_EXIT_TIMEOUT;
            while self.child.try_wait().ok().flatten().is_none()
                && Instant::now() < deadline
            {
                thread::sleep(Duration::from_millis(20));
            }
        }
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.writer.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.snapshot()
    }

    fn snapshot(&self) -> Vec<u8> {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn match_end_after(&self, needle: &[u8], offset: usize) -> Option<usize> {
        let output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match_end_after(&output, needle, offset)
    }

    /// The child's exit code once it has left on its own, or `None` when it
    /// is still running at `timeout`. Nothing is sent and nothing is killed:
    /// a scenario about a process that must end by itself asks this first and
    /// only then reads the bytes with [`Self::finish`].
    pub fn exit_code(&mut self, timeout: Duration) -> Option<i32> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return status.code();
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Close the terminal under the child the way a window closes a pane —
    /// every copy of the master goes, so the child's next write answers EIO
    /// and a controlling session is hung up — then wait at most `timeout` for
    /// the child to leave by itself. Its exit code, or `None` while it still
    /// runs; the bytes read before the hang-up stay in the capture.
    pub fn hang_up(&mut self, timeout: Duration) -> Option<i32> {
        self.writer.take();
        self.hung_up.store(true, Ordering::Relaxed);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.exit_code(timeout)
    }

    /// Everything captured so far.
    pub fn snapshot_output(&self) -> Vec<u8> {
        self.snapshot()
    }

    /// Poll the capture until `ready` holds for the bytes after `offset`, or
    /// panic at `timeout` — for a state the TUI reaches on its own clock (the
    /// size poll noticing a silent resize, a repaint settling), where a fixed
    /// sleep is a bet on the machine's load and loses at load 13.
    pub fn wait_until(
        &mut self,
        offset: usize,
        timeout: Duration,
        mut ready: impl FnMut(&[u8]) -> bool,
    ) {
        let deadline = Instant::now() + timeout;
        loop {
            let snapshot = self.snapshot();
            if ready(snapshot.get(offset..).unwrap_or(&[])) {
                return;
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                panic!(
                    "zo exited ({status}) before the terminal settled; output:\n{}",
                    printable(&snapshot)
                );
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for the terminal to settle; output:\n{}",
                printable(&snapshot)
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Wait until `needle` stands in a COMMITTED history row — one the painter
    /// wrote with the `\r\n` + erase grammar `history_rows_raw` scans — not
    /// merely somewhere in the live head. A golden that compares history rows
    /// used to wait for the text to appear anywhere and then `finish()`, which
    /// raced the commit animation: under load the last bullet was still in the
    /// head, the history lacked it, and the golden went red (the hook-context
    /// and tool-turn goldens, 2026-09-04).
    pub fn wait_for_history_row(&mut self, needle: &str, timeout: Duration) {
        let needle = needle.as_bytes();
        self.wait_until(0, timeout, |bytes| {
            scan_history_rows(bytes)
                .iter()
                .any(|row| find_bytes(row, needle).is_some())
        });
    }

    /// Number of bytes captured so far, useful for waiting on the next
    /// occurrence of a repeated frame.
    pub fn output_len(&self) -> usize {
        self.output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl Drop for PtyRun {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        self.writer.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Run the same child setup with ordinary pipes.  This is the `exec` side of
/// the TTY/pipe split and intentionally does not allocate a PTY.
pub fn run_pipe(
    cwd: &Path,
    home: &Path,
    sessions: &Path,
    state: &Path,
    base_url: &str,
    args: &[&str],
    input: &[u8],
) -> io::Result<Output> {
    run_pipe_with_env(cwd, home, sessions, state, base_url, args, input, &[])
}

/// [`run_pipe`] plus extra environment for the child — the door a scenario
/// opens by env rather than by flag.
#[allow(clippy::too_many_arguments)]
pub fn run_pipe_with_env(
    cwd: &Path,
    home: &Path,
    sessions: &Path,
    state: &Path,
    base_url: &str,
    args: &[&str],
    input: &[u8],
    extra_env: &[(&str, &str)],
) -> io::Result<Output> {
    let mut command = Command::new(zo_binary());
    configure_command(
        &mut command,
        cwd,
        home,
        sessions,
        state,
        base_url,
        args,
    );
    for (key, value) in extra_env {
        command.env(key, value);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input)?;
    }
    child.wait_with_output()
}

fn configure_command(
    command: &mut Command,
    cwd: &Path,
    home: &Path,
    sessions: &Path,
    state: &Path,
    base_url: &str,
    args: &[&str],
) {
    for (name, _) in std::env::vars_os() {
        let key = name.to_string_lossy();
        if key.starts_with("ZEROCODE_") || key.starts_with("ZO_EVENTS_")
            || key.starts_with("ZO_SERVE_") || key.starts_with("ZO_MODEL_")
            || key.starts_with("ZO_LAUNCH_") {
            command.env_remove(name);
        }
    }
    command
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("ZO_CONFIG_HOME", home)
        .env("TMPDIR", state)
        .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
        .env("CODEX_HOME", home.join("codex"))
        .env("ZO_CODEX_HOME", home.join("codex"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("ZO_SESSION_ROOT", sessions)
        .env("ZO_STATE_DIR", state)
        .env("ANTHROPIC_BASE_URL", base_url)
        .env(
            "ZO_PROBE_PAINT",
            std::env::var("ZO_PROBE_PAINT").unwrap_or_default(),
        )
        .env("ANTHROPIC_API_KEY", "test-dummy-key")
        .env("ZO_DISABLE_KEYCHAIN", "1")
        // 세션이 열릴 때 사용량 프로브가 세 provider 를 긁는다. 이 하네스의
        // HOME·설정 홈이 임시 폴더라 자격증명이 없어 이미 나가지 못하지만,
        // 오프라인 보장을 우연이 아니라 **선언**으로 두려고 킬 스위치를 켠다.
        .env("ZO_DISABLE_EXTERNAL_CREDENTIALS", "1")
        .env("TERM", "xterm-256color")
        .env("RUST_BACKTRACE", "1")
        .env_remove("NO_COLOR")
        .env_remove("ZO_HOME")
        .env_remove("ANTHROPIC_AUTH_TOKEN")
        .env_remove("OPENAI_API_KEY")
        .env_remove("XAI_API_KEY")
        .env_remove("GOOGLE_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("ZO_CUSTOM_PROVIDERS")
        // The window plants the developer's second-brain vault into every
        // pane; a hermetic zo must not read it, or its prompt gains a section
        // and its recall a corpus the goldens never saw.
        .env_remove(runtime::second_brain::VAULT_ENV)
        .env_remove("ZO_AUTO_VERIFY")
        .env_remove("ZO_AUTO_VERIFY_CMD")
        .env_remove("ZO_TUI_FOLD_MARKERS")
        .env_remove("ZEROCODE_HOOK_PORT")
        .env_remove("ZEROCODE_HOOK_TOKEN")
        .env_remove("ZEROCODE_PANE_KEY")
        .env_remove("ZEROCODE_HOOK_PANE_KEY")
        // The window's own agent-team coordinates, and the multiplexer they
        // point at. A pane of this window carries all four, and a zo that
        // finds them runs its sub-agents in PANES — so the suite, run from a
        // developer's pane, cut real leaves in their live window and left the
        // teammate processes behind (measured 2026-09-04, mid-gate). A
        // hermetic run is one whose helpers stay inside it.
        .env_remove("TMUX")
        .env_remove("TMUX_PANE")
        .env_remove("ZEROCODE_AGENT_TEAM_ID")
        .env_remove("ZEROCODE_AGENT_TEAM_TOKEN")
        .env_remove("ZEROCODE_AGENT_TEAM_TOKEN_FILE")
        .env_remove("ZEROCODE_AGENT_TEAM_PANE")
        .env_remove("ZEROCODE_AGENT_TEAM_LEADER")
        // And the override that would turn them back on without them.
        .env_remove("ZO_SUBAGENT_MODE");
}

/// Extract the payloads of rows committed through `Painter::history_line`.
/// Viewport frames and cursor movement are intentionally ignored, matching
/// `docs/captures/hist-lines.py` while keeping the comparison in Rust.
#[must_use]
/// 스크롤백에 커밋된 행을 **바이트 그대로** 돌려준다(SGR 포함).
///
/// 골든이 여기 기대는 이유: 색과 간격은 codex 패리티 계약의 일부다. 평문만
/// 비교하면 굵기·색·순서가 무너져도 통과한다.
pub fn history_rows_raw(capture: &[u8]) -> Vec<Vec<u8>> {
    scan_history_rows(capture)
}

/// A terminal for the tests that care where rows END UP rather than which
/// bytes moved them: the painter's grammar — CUP, EL, ED, DECSTBM, line feed,
/// reverse index — replayed onto a grid of characters with a scrollback. A row
/// that leaves a region whose top margin is the first screen row goes to
/// scrollback; a row leaving any other region is dropped. That is xterm's rule
/// and this window's grid's. Cells are characters, not columns: a wide glyph
/// takes one slot, so rows are compared by text, never by column.
pub struct Screen {
    rows: Vec<Vec<char>>,
    scrollback: Vec<String>,
    row: usize,
    col: usize,
    top_margin: usize,
    bottom_margin: usize,
}

impl Screen {
    #[must_use]
    pub fn new(rows: usize) -> Self {
        let rows = rows.max(1);
        Self {
            rows: vec![Vec::new(); rows],
            scrollback: Vec::new(),
            row: 0,
            col: 0,
            top_margin: 0,
            bottom_margin: rows - 1,
        }
    }

    /// The pane grew or shrank: rows are added blank at the bottom or taken
    /// from it, the margins reset, the cursor stays on a row that exists.
    #[allow(dead_code)] // pane suite only
    pub fn resize(&mut self, rows: usize) {
        let rows = rows.max(1);
        self.rows.resize(rows, Vec::new());
        self.top_margin = 0;
        self.bottom_margin = rows - 1;
        self.row = self.row.min(rows - 1);
    }

    pub fn feed(&mut self, capture: &[u8]) {
        let text = String::from_utf8_lossy(capture);
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '\u{1b}' => self.escape(&mut chars),
                '\r' => self.col = 0,
                '\n' => self.line_feed(),
                ch if ch.is_control() => {}
                ch => {
                    let row = &mut self.rows[self.row];
                    if self.col < row.len() {
                        row[self.col] = ch;
                    } else {
                        row.resize(self.col, ' ');
                        row.push(ch);
                    }
                    self.col += 1;
                }
            }
        }
    }

    /// The screen's rows, trailing blanks trimmed.
    #[must_use]
    pub fn visible(&self) -> Vec<String> {
        self.rows.iter().map(|row| Self::text(row)).collect()
    }

    /// Every row a person can scroll to, oldest first: the scrollback, then
    /// the screen.
    #[must_use]
    #[allow(dead_code)] // the hermetic suite's floor scenarios only
    pub fn transcript(&self) -> Vec<String> {
        self.scrollback
            .iter()
            .cloned()
            .chain(self.visible())
            .collect()
    }

    fn text(row: &[char]) -> String {
        row.iter().collect::<String>().trim_end().to_owned()
    }

    fn escape(&mut self, chars: &mut std::str::Chars<'_>) {
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                let mut final_byte = None;
                for ch in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&ch) {
                        final_byte = Some(ch);
                        break;
                    }
                    params.push(ch);
                }
                self.csi(&params, final_byte);
            }
            Some(']') => {
                let mut previous = '\0';
                for ch in chars.by_ref() {
                    if ch == '\u{7}' || (previous == '\u{1b}' && ch == '\\') {
                        break;
                    }
                    previous = ch;
                }
            }
            Some('M') => self.reverse_index(),
            // Charset and mode selections take one more byte; nothing moves.
            Some('(' | ')' | '#' | '%' | ' ') => {
                chars.next();
            }
            _ => {}
        }
    }

    fn csi(&mut self, params: &str, final_byte: Option<char>) {
        // Private modes and DECSCUSR move nothing.
        if params.starts_with('?') || params.contains(' ') {
            return;
        }
        let values: Vec<usize> = params
            .split(';')
            .map(|value| value.parse().unwrap_or(0))
            .collect();
        let arg = |index: usize, default: usize| {
            values
                .get(index)
                .copied()
                .filter(|value| *value > 0)
                .unwrap_or(default)
        };
        let last = self.rows.len() - 1;
        match final_byte {
            Some('H' | 'f') => {
                self.row = (arg(0, 1) - 1).min(last);
                self.col = arg(1, 1) - 1;
            }
            Some('d') => self.row = (arg(0, 1) - 1).min(last),
            Some('G') => self.col = arg(0, 1) - 1,
            Some('A') => self.row = self.row.saturating_sub(arg(0, 1)),
            Some('B') => self.row = (self.row + arg(0, 1)).min(last),
            Some('C') => self.col += arg(0, 1),
            Some('D') => self.col = self.col.saturating_sub(arg(0, 1)),
            Some('K') => match values.first().copied().unwrap_or(0) {
                0 => self.rows[self.row].truncate(self.col),
                1 => {
                    let row = &mut self.rows[self.row];
                    let end = (self.col + 1).min(row.len());
                    row[..end].fill(' ');
                }
                2 => self.rows[self.row].clear(),
                _ => {}
            },
            Some('J') => match values.first().copied().unwrap_or(0) {
                0 => {
                    self.rows[self.row].truncate(self.col);
                    for row in &mut self.rows[self.row + 1..] {
                        row.clear();
                    }
                }
                2 => {
                    for row in &mut self.rows {
                        row.clear();
                    }
                }
                3 => self.scrollback.clear(),
                _ => {}
            },
            Some('r') => {
                if params.is_empty() {
                    self.top_margin = 0;
                    self.bottom_margin = last;
                } else {
                    let top = (arg(0, 1) - 1).min(last);
                    let bottom = (arg(1, last + 1) - 1).min(last);
                    if top < bottom {
                        self.top_margin = top;
                        self.bottom_margin = bottom;
                    }
                }
                self.row = 0;
                self.col = 0;
            }
            _ => {}
        }
    }

    fn line_feed(&mut self) {
        if self.row == self.bottom_margin {
            let gone = self.rows.remove(self.top_margin);
            if self.top_margin == 0 {
                self.scrollback.push(Self::text(&gone));
            }
            self.rows.insert(self.bottom_margin, Vec::new());
        } else if self.row < self.rows.len() - 1 {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.row == self.top_margin {
            self.rows.remove(self.bottom_margin);
            self.rows.insert(self.top_margin, Vec::new());
        } else if self.row > 0 {
            self.row -= 1;
        }
    }
}

/// Replays the terminal controls emitted by the TUI and reports which visible
/// rows contain text at the end of the capture — the layout regressions
/// measured as blank vertical space, on the same [`Screen`] the row-content
/// assertions use.
#[must_use]
pub fn visible_row_occupancy(capture: &[u8], rows: usize) -> Vec<bool> {
    let mut screen = Screen::new(rows);
    screen.feed(capture);
    screen
        .visible()
        .iter()
        .map(|row| !row.trim().is_empty())
        .collect()
}

fn scan_history_rows(capture: &[u8]) -> Vec<Vec<u8>> {
    const ERASE_LINE: &[u8] = b"\x1b[K";
    const LINE_TAIL: &[u8] = b"\x1b[39m\x1b[49m\x1b[0m";
    let mut rows = Vec::new();
    let mut cursor = 0;
    while let Some(relative) = find_bytes(&capture[cursor..], b"\r\n") {
        let start = cursor + relative + 2;
        let Some(erase_relative) = find_bytes(&capture[start..], ERASE_LINE) else {
            cursor = start;
            continue;
        };
        let body_start = start + erase_relative + ERASE_LINE.len();
        let Some(tail_relative) = find_bytes(&capture[body_start..], LINE_TAIL) else {
            cursor = body_start;
            continue;
        };
        let body_end = body_start + tail_relative;
        rows.push(capture[body_start..body_end].to_vec());
        cursor = body_end + LINE_TAIL.len();
    }
    rows
}

/// The offset just past the first `needle` at or after `offset` in
/// `output` — the resume point for a wait on the next occurrence.
/// Where `needle` ends after `offset`: as bytes, or else as the words a
/// person reads — the same text with CSI sequences (SGR) between its letters,
/// as a bold key beside dim words writes it (`? for shortcuts` on the
/// footer's second row, t-10232). The byte match wins, so a needle ever
/// found as bytes is found where it was.
fn match_end_after(output: &[u8], needle: &[u8], offset: usize) -> Option<usize> {
    let suffix = output.get(offset..)?;
    if let Some(at) = find_bytes(suffix, needle) {
        return Some(offset + at + needle.len());
    }
    if needle.contains(&0x1b) {
        return None;
    }
    let mut visible = Vec::with_capacity(suffix.len());
    let mut ends = Vec::with_capacity(suffix.len());
    let mut index = 0;
    while index < suffix.len() {
        if suffix[index] == 0x1b && suffix.get(index + 1) == Some(&b'[') {
            index += 2;
            while index < suffix.len() && !(0x40..=0x7e).contains(&suffix[index]) {
                index += 1;
            }
            index += 1;
            continue;
        }
        visible.push(suffix[index]);
        ends.push(index + 1);
        index += 1;
    }
    find_bytes(&visible, needle).map(|at| offset + ends[at + needle.len() - 1])
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}


/// `ps -o time=` prints `[[DD-]HH:]MM:SS.cc`.  Parsed here rather than shelled
/// out to a formatter because a silently mis-parsed CPU number would make the
/// wait/compute ratio — the whole point of the probe — quietly wrong.
fn parse_ps_time(raw: &str) -> Option<u64> {
    if raw.is_empty() {
        return None;
    }
    let (days, clock) = match raw.split_once('-') {
        Some((days, rest)) => (days.parse::<u64>().ok()?, rest),
        None => (0, raw),
    };
    let mut total_ms = days * 24 * 60 * 60 * 1000;
    let mut parts = clock.split(':').collect::<Vec<_>>();
    let seconds = parts.pop()?;
    let (whole, fraction) = seconds.split_once('.').unwrap_or((seconds, "0"));
    total_ms += whole.parse::<u64>().ok()? * 1000;
    total_ms += format!("{fraction:0<3}")[..3].parse::<u64>().ok()?;
    let mut scale = 60_000;
    while let Some(part) = parts.pop() {
        total_ms += part.parse::<u64>().ok()? * scale;
        scale *= 60;
    }
    Some(total_ms)
}

/// Head AND tail of a capture, because a timeout is a question about what is
/// on the screen NOW: keeping only the first 12k characters shows the boot
/// banner of a session whose last frame is what actually explains the failure.
fn printable(bytes: &[u8]) -> String {
    let escaped = String::from_utf8_lossy(bytes).replace('\u{1b}', "<ESC>");
    let chars: Vec<char> = escaped.chars().collect();
    if chars.len() <= 12_000 {
        return chars.into_iter().collect();
    }
    let head: String = chars[..6_000].iter().collect();
    let tail: String = chars[chars.len() - 6_000..].iter().collect();
    format!("{head}\n… [{} characters elided] …\n{tail}", chars.len() - 12_000)
}

#[cfg(test)]
mod tests {
    use super::{match_end_after, parse_ps_time, terminal_command, Terminal, CHILD_EXIT_TIMEOUT};
    use std::fs::File;
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    fn terminal_probe(rows: u16, cols: u16, terminal: Terminal, stage: &str) {
        let size = nix::pty::Winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let pair = nix::pty::openpty(Some(&size), None::<&nix::sys::termios::Termios>).unwrap();
        let mut master = File::from(pair.master);
        let slave = File::from(pair.slave);
        let path = nix::unistd::ttyname(&slave).unwrap();
        let binary = std::env::current_exe().unwrap();
        let name = format!(
            "{}::pty_sessions_do_not_inherit_the_runners_terminal",
            module_path!().split_once("::").unwrap().1,
        );
        let mut command = terminal_command(&binary, terminal, Some(&path)).unwrap();
        command
            .args(["--exact", &name, "--nocapture"])
            .env("ZO_E2E_PTY_PROBE", stage)
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        let mut child = command.spawn().unwrap();
        drop(command);
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = master.read_to_end(&mut bytes);
            bytes
        });
        let deadline = Instant::now() + CHILD_EXIT_TIMEOUT;
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("terminal probe timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let bytes = reader.join().unwrap();
        let output = String::from_utf8_lossy(&bytes);
        assert!(status.success(), "{stage}: {output}");
        assert!(output.contains("1 passed"), "probe did not run: {output}");
    }

    #[test]
    fn pty_sessions_do_not_inherit_the_runners_terminal() {
        match std::env::var("ZO_E2E_PTY_PROBE").ok().as_deref() {
            None => terminal_probe(24, 62, Terminal::Controlling, "runner"),
            Some("runner") => {
                assert_eq!(crossterm::terminal::size().unwrap(), (62, 24));
                for (rows, cols) in [(40, 120), (24, 80), (60, 200)] {
                    for terminal in [Terminal::Detached, Terminal::Controlling] {
                        let stage = format!("{rows},{cols},{}", terminal == Terminal::Controlling);
                        terminal_probe(rows, cols, terminal, &stage);
                        assert_eq!(crossterm::terminal::size().unwrap(), (62, 24));
                    }
                }
            }
            Some(stage) => {
                let parts: Vec<_> = stage.split(',').collect();
                let rows: u16 = parts[0].parse().unwrap();
                let cols: u16 = parts[1].parse().unwrap();
                assert_eq!(crossterm::terminal::size().unwrap(), (cols, rows));
                assert_eq!(File::open("/dev/tty").is_ok(), parts[2] == "true");
            }
        }
    }

    /// Two answers in one read: the resume point after the first one still
    /// finds the second, while the capture's length after both — the
    /// offset a wait used to take — finds nothing and would time out.
    #[test]
    fn the_next_occurrence_is_found_from_the_first_matchs_end_not_the_length_after_both() {
        let output = b"\x1b[2m\x1b[22mDONE\x1b[39m ... \x1b[22mDONE\x1b[39m";
        let first_end = match_end_after(output, b"DONE", 0).expect("the first answer");
        assert_eq!(&output[first_end - 4..first_end], b"DONE");
        let second_end =
            match_end_after(output, b"DONE", first_end).expect("the queued turn's answer");
        assert!(second_end > first_end);
        assert_eq!(match_end_after(output, b"DONE", output.len()), None);
        assert_eq!(match_end_after(output, b"DONE", output.len() + 1), None);
    }

    /// A bold key and its dim words are one needle to a person and to a
    /// wait; the end is the raw offset just past the last letter.
    #[test]
    fn a_needle_split_by_sgr_is_found_as_the_words_on_screen() {
        let output = b"  \x1b[1m?\x1b[22m\x1b[2m for shortcuts\x1b[0m tail";
        let end = match_end_after(output, b"? for shortcuts", 0).expect("the styled hint");
        assert_eq!(&output[end - 9..end], b"shortcuts");
        assert_eq!(match_end_after(output, b"? for shortcuts", end), None);
        assert_eq!(match_end_after(output, b"? for agents", 0), None);
    }

    #[test]
    fn ps_time_parses_every_shape_macos_prints() {
        assert_eq!(parse_ps_time("0:00.35"), Some(350));
        assert_eq!(parse_ps_time("0:01.00"), Some(1_000));
        assert_eq!(parse_ps_time("2:03.50"), Some(123_500));
        assert_eq!(parse_ps_time("1:00:00.00"), Some(3_600_000));
        assert_eq!(parse_ps_time(""), None);
    }
}
