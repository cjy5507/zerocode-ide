//! Single-responsibility runner for plugin subprocesses.
//!
//! Plugin tool invocations, lifecycle (`init`/`shutdown`) commands, and slash
//! commands all spawn an untrusted external process, pipe it some input, and
//! read its stdout/stderr. Left unguarded each of those did two dangerous
//! things: it waited for the child forever (a hung or interactive script froze
//! the whole agent), and it slurped stdout/stderr with no size limit (a chatty
//! or malicious script could exhaust memory).
//!
//! [`run_plugin_process`] is the one place that owns those safety concerns —
//! wall-clock [`PLUGIN_PROCESS_TIMEOUT`], bounded head+tail capture at
//! [`MAX_PLUGIN_OUTPUT_BYTES`], and killing the child (so it cannot linger)
//! when the budget is exceeded — so every caller inherits the same behavior
//! instead of re-deriving it. It is deliberately *not* used for `git clone`
//! and other install-time commands: those have different semantics (network
//! fetch, their own progress/timeout expectations) and are left to `install`.

use std::io::{Read, Write};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::error::PluginError;

/// Maximum wall-clock time a single plugin subprocess may run before it is
/// killed. Generous enough for a real build/format/lint helper, short enough
/// that a hung or interactive script cannot freeze the agent indefinitely.
pub(crate) const PLUGIN_PROCESS_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum bytes retained from each of a plugin subprocess's stdout and stderr.
/// A stream longer than this keeps its head and tail (the parts a human or the
/// model actually needs) with an elision marker between, so a runaway writer
/// cannot exhaust memory yet the useful context survives. 1 MiB per stream.
pub(crate) const MAX_PLUGIN_OUTPUT_BYTES: usize = 1024 * 1024;

/// The captured result of a finished plugin subprocess.
#[derive(Debug)]
pub(crate) struct PluginProcessOutput {
    pub(crate) success: bool,
    pub(crate) status: std::process::ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// Spawn `command`, optionally write `stdin_data` to its stdin, then wait up to
/// [`PLUGIN_PROCESS_TIMEOUT`] while capturing stdout/stderr bounded to
/// [`MAX_PLUGIN_OUTPUT_BYTES`] each. On timeout the child is killed and reaped
/// and [`PluginError::TimedOut`] is returned with `context` naming the caller.
///
/// `command` must already have its args, env, and working directory set; only
/// the stdio pipes are configured here so the capture is well-defined.
pub(crate) fn run_plugin_process(
    command: Command,
    stdin_data: Option<&[u8]>,
    context: &str,
) -> Result<PluginProcessOutput, PluginError> {
    run_plugin_process_with_timeout(command, stdin_data, context, PLUGIN_PROCESS_TIMEOUT)
}

/// Like [`run_plugin_process`] but with an explicit timeout, so the kill path
/// can be exercised in tests without a two-minute wait. Production callers use
/// the [`PLUGIN_PROCESS_TIMEOUT`] wrapper.
fn run_plugin_process_with_timeout(
    mut command: Command,
    stdin_data: Option<&[u8]>,
    context: &str,
    timeout: Duration,
) -> Result<PluginProcessOutput, PluginError> {
    use std::process::Stdio;

    command
        .stdin(if stdin_data.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Make the child its own process-group leader so a timeout can signal the
    // whole subtree; a plain `child.kill()` would orphan grandchildren.
    // `process_group` is a safe `CommandExt` method (no `pre_exec`/unsafe).
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }

    let mut child = command.spawn()?;

    // Write stdin on a thread (a child that never drains stdin would deadlock the
    // parent once the pipe fills) but keep the handle so a real write error is
    // not silently hidden; it is joined after the wait.
    let stdin_writer = stdin_data.and_then(|data| {
        child.stdin.take().map(|mut stdin| {
            let owned = data.to_vec();
            thread::spawn(move || stdin.write_all(&owned))
        })
    });

    let stdout_reader = spawn_bounded_reader(child.stdout.take());
    let stderr_reader = spawn_bounded_reader(child.stderr.take());

    let Some(status) = wait_with_timeout(&mut child, timeout)? else {
        // Timed out: signal the whole process group (the child plus any
        // grandchildren), then kill+reap the direct child so no orphan lingers,
        // and drop the read handles (joining the reader threads, which end at
        // pipe EOF once the processes are gone).
        #[cfg(unix)]
        terminate_process_group(child.id());
        let _ = child.kill();
        let _ = child.wait();
        let _ = stdout_reader.join();
        let _ = stderr_reader.join();
        join_stdin_writer(stdin_writer, context)?;
        return Err(PluginError::TimedOut(format!(
            "{context} exceeded the {}s plugin timeout and was killed",
            timeout.as_secs()
        )));
    };

    join_stdin_writer(stdin_writer, context)?;

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();

    Ok(PluginProcessOutput {
        success: status.success(),
        status,
        stdout,
        stderr,
    })
}

/// Join the optional stdin-writer thread, surfacing a genuine write failure. A
/// broken-pipe error (the child exited before reading all input) is benign and
/// swallowed; any other write error or a thread panic becomes `CommandFailed` so
/// truncated-input delivery is not hidden.
fn join_stdin_writer(
    writer: Option<thread::JoinHandle<std::io::Result<()>>>,
    context: &str,
) -> Result<(), PluginError> {
    let Some(handle) = writer else {
        return Ok(());
    };
    match handle.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Ok(Err(error)) => Err(PluginError::CommandFailed(format!(
            "{context}: failed to write stdin to the plugin process: {error}"
        ))),
        Err(_) => Err(PluginError::CommandFailed(format!(
            "{context}: the stdin writer thread panicked"
        ))),
    }
}

/// Join handle wrapper returning the bounded-decoded stream on `join`.
struct ReaderHandle(Option<thread::JoinHandle<String>>);

impl ReaderHandle {
    fn join(mut self) -> Result<String, ()> {
        match self.0.take() {
            Some(handle) => handle.join().map_err(|_| ()),
            None => Ok(String::new()),
        }
    }
}

/// Read `pipe` to end, retaining at most [`MAX_PLUGIN_OUTPUT_BYTES`] via a
/// head+tail window, on its own thread so stdout and stderr drain concurrently
/// (a child can fill either pipe; reading only one risks a deadlock).
fn spawn_bounded_reader<R: Read + Send + 'static>(pipe: Option<R>) -> ReaderHandle {
    let Some(mut pipe) = pipe else {
        return ReaderHandle(None);
    };
    let handle = thread::spawn(move || {
        let mut capture = BoundedCapture::new(MAX_PLUGIN_OUTPUT_BYTES);
        let mut buffer = [0_u8; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => capture.push(&buffer[..read]),
                Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        capture.into_string()
    });
    ReaderHandle(Some(handle))
}

/// The marker inserted between a truncated stream's retained head and tail,
/// recording how many bytes were dropped.
fn elision_marker(dropped: usize) -> String {
    format!("… [{dropped} bytes elided] …")
}

/// Signal the timed-out child's whole process group (`SIGTERM`, then `SIGKILL`)
/// so grandchildren are reaped too, not just the direct child. The child leads
/// its own group, so its pid is the group id; errors are best-effort (the caller
/// still `kill()`s + `wait()`s the direct child).
#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;

    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // Negative pid targets the whole process group. Any kill error (ESRCH gone
    // group, macOS EPERM for a zombies-only group we own) means "nothing left to
    // stop", so a failed SIGTERM just skips the SIGKILL.
    let group = Pid::from_raw(-pid);
    if signal::kill(group, Signal::SIGTERM).is_err() {
        return;
    }
    std::thread::sleep(Duration::from_millis(50));
    let _ = signal::kill(group, Signal::SIGKILL);
}

/// Accumulate bytes but keep only the first and last `limit / 2` bytes once the
/// total exceeds `limit`, so an unbounded stream cannot exhaust memory while the
/// human-relevant head and tail survive. The elision marker records how many
/// bytes were dropped.
struct BoundedCapture {
    limit: usize,
    head: Vec<u8>,
    tail: std::collections::VecDeque<u8>,
    total: usize,
}

impl BoundedCapture {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            head: Vec::new(),
            tail: std::collections::VecDeque::new(),
            total: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.total += bytes.len();
        let half = self.limit / 2;
        for &byte in bytes {
            if self.head.len() < half {
                self.head.push(byte);
            } else {
                self.tail.push_back(byte);
                if self.tail.len() > half {
                    self.tail.pop_front();
                }
            }
        }
    }

    fn into_string(self) -> String {
        if self.total <= self.limit {
            let mut bytes = self.head;
            bytes.extend(self.tail);
            return String::from_utf8_lossy(&bytes).into_owned();
        }
        let dropped = self.total - self.head.len() - self.tail.len();
        let mut out = String::from_utf8_lossy(&self.head).into_owned();
        out.push('\n');
        out.push_str(&elision_marker(dropped));
        out.push('\n');
        let tail: Vec<u8> = self.tail.into_iter().collect();
        out.push_str(&String::from_utf8_lossy(&tail));
        out
    }
}

/// Wait for `child` up to `timeout`, returning `Ok(Some(status))` when it exits,
/// `Ok(None)` on timeout, and `Err` if polling the child genuinely fails. A
/// dedicated wait-thread plus `recv_timeout` keeps this to safe std only (no
/// `unsafe`, no extra deps) and works identically on every platform.
///
/// A `try_wait` error (e.g. the child handle became invalid) is a real failure,
/// not a timeout: the earlier code swallowed it with `if let Ok(Some(_))` and
/// then reported a spurious `TimedOut`. It is propagated as `io::Result` so the
/// runner can distinguish "still running" from "wait failed".
fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<Option<std::process::ExitStatus>> {
    // Fast path: many plugin commands finish promptly. Poll once cheaply before
    // paying for a thread.
    if let Some(status) = child.try_wait()? {
        return Ok(Some(status));
    }

    let (sender, receiver) = mpsc::channel();
    let deadline = std::time::Instant::now() + timeout;
    // Poll on a background thread so the parent can bound the wait; a short
    // sleep between polls keeps this from busy-spinning a core.
    let poll_handle = {
        let sender = sender.clone();
        // The thread cannot borrow `child`, so it signals the parent to poll.
        thread::spawn(move || {
            while std::time::Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
                if sender.send(()).is_err() {
                    return;
                }
            }
        })
    };

    let result = loop {
        if let Some(status) = child.try_wait()? {
            break Ok(Some(status));
        }
        if std::time::Instant::now() >= deadline {
            break Ok(None);
        }
        // Wake on each poll tick; if the ticker thread has ended, fall back to a
        // bounded recv so we still honor the deadline.
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.try_wait(),
        }
    };

    drop(receiver);
    let _ = poll_handle.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        command
    }

    #[cfg(unix)]
    #[test]
    fn successful_command_returns_stdout() {
        let output = run_plugin_process(sh("printf 'hello'"), None, "test tool")
            .expect("command should run");
        assert!(output.success);
        assert_eq!(output.stdout, "hello");
    }

    #[cfg(unix)]
    #[test]
    fn stdin_is_delivered_to_the_child() {
        let output = run_plugin_process(sh("cat"), Some(b"piped-input"), "test tool")
            .expect("command should run");
        assert!(output.success);
        assert_eq!(output.stdout, "piped-input");
    }

    #[cfg(unix)]
    #[test]
    fn hung_command_is_killed_and_reports_timeout() {
        // A command that sleeps well past the (short, test-only) timeout must be
        // killed and surface `TimedOut`, not hang the caller.
        let started = std::time::Instant::now();
        let error = run_plugin_process_with_timeout(
            sh("sleep 30"),
            None,
            "test tool",
            Duration::from_millis(150),
        )
        .expect_err("a hung command must time out");
        assert!(
            matches!(error, PluginError::TimedOut(_)),
            "expected TimedOut, got {error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the runner must not block for the child's full sleep"
        );
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_descendants_holding_the_output_pipe() {
        // Regression: the immediate child (`sh`) runs a pipeline whose members
        // are grandchildren — `cat` inherits and holds the write end of our
        // stdout pipe while `sleep` keeps the pipeline alive past the timeout.
        // Killing only the immediate child leaves `cat` holding the pipe, so the
        // reader join blocks forever; the group kill reaps `sleep`+`cat`, closes
        // the pipe, and lets the runner return `TimedOut` promptly.
        let started = std::time::Instant::now();
        let error = run_plugin_process_with_timeout(
            sh("sleep 30 | cat"),
            None,
            "descendant test",
            Duration::from_millis(200),
        )
        .expect_err("a descendant holding the pipe must still time out");
        assert!(
            matches!(error, PluginError::TimedOut(_)),
            "expected TimedOut, got {error:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the group kill must free the reader join, not wait for the grandchild's full sleep (elapsed {:?})",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn oversized_output_is_truncated_with_marker() {
        // Emit well over the cap; the capture must stay bounded and mark elision.
        let bytes = MAX_PLUGIN_OUTPUT_BYTES + 4096;
        let script = format!("head -c {bytes} /dev/zero | tr '\\0' 'a'");
        let output = run_plugin_process(sh(&script), None, "test tool")
            .expect("command should run");
        assert!(output.success);
        assert!(
            output.stdout.len() <= MAX_PLUGIN_OUTPUT_BYTES + 64,
            "captured {} bytes, expected bounded",
            output.stdout.len()
        );
        assert!(
            output.stdout.contains("bytes elided"),
            "truncated output must carry the elision marker"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failing_command_reports_stderr_and_nonzero() {
        let output = run_plugin_process(sh("echo boom 1>&2; exit 3"), None, "test tool")
            .expect("command should run to completion");
        assert!(!output.success);
        assert_eq!(output.stderr.trim(), "boom");
    }

    #[test]
    fn bounded_capture_keeps_head_and_tail() {
        let mut capture = BoundedCapture::new(8);
        capture.push(b"ABCDEFGHIJKLMNOP");
        let out = capture.into_string();
        assert!(out.starts_with("ABCD"), "head preserved: {out:?}");
        assert!(out.trim_end().ends_with("MNOP"), "tail preserved: {out:?}");
        assert!(out.contains("bytes elided"));
    }

    #[test]
    fn bounded_capture_passes_small_output_through() {
        let mut capture = BoundedCapture::new(1024);
        capture.push(b"small");
        assert_eq!(capture.into_string(), "small");
    }
}
