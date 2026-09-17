//! One bounded subprocess boundary for authority effects.
//!
//! Unix process groups stop ordinary descendants, but they are not an OS
//! sandbox: a deliberately daemonized child can leave the group. All pipes are
//! therefore nonblocking and owned by this deadline loop. Even an escaped
//! writer cannot retain a reader thread, file descriptor, or caller latency
//! beyond the configured limit. Strong daemon containment belongs in a future
//! platform sandbox adapter (cgroup/sandbox), not in this primitive.
//!
//! Windows has no process groups and its anonymous pipes cannot be made
//! nonblocking, so the same contract is kept differently: the child is
//! created suspended, put in a job object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and only then resumed (so nothing it
//! spawns can start outside the job), one reader
//! thread per pipe feeds this deadline loop through a channel, and the
//! deadline terminates the job — every process in it, descendants included —
//! which closes the pipes' write ends and so ends the readers. Until
//! 2026-09-11 this platform answered `UnsupportedPlatform`, and with it the
//! local git ref publisher, test-evidence verification and the codex queue
//! were unavailable on Windows (docs/design/windows-parity-audit-20260910.md).

#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd as _;
#[cfg(unix)]
use std::process::{Child, Stdio};
use std::process::{Command, ExitStatus};
#[cfg(unix)]
use std::thread;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

#[cfg(any(unix, windows))]
const POLL: Duration = Duration::from_millis(10);
#[cfg(unix)]
const DRAIN_BUDGET: usize = 64 * 1024;
#[cfg(windows)]
const READ_CHUNK: usize = 8192;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProcessLimits {
    pub(crate) timeout: Duration,
    pub(crate) stdout_bytes: usize,
    pub(crate) stderr_bytes: usize,
}

#[derive(Debug)]
pub(crate) struct ProcessOutput {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) stdout_truncated: bool,
    pub(crate) stderr_truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProcessError {
    Timeout,
    Unavailable,
    #[cfg(not(any(unix, windows)))]
    UnsupportedPlatform,
}

#[cfg(any(unix, windows))]
#[derive(Debug)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

#[cfg(any(unix, windows))]
impl Captured {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            truncated: false,
        }
    }

    fn push(&mut self, chunk: &[u8], limit: usize) {
        let room = limit.saturating_sub(self.bytes.len());
        let kept = room.min(chunk.len());
        self.bytes.extend_from_slice(&chunk[..kept]);
        self.truncated |= kept < chunk.len();
    }
}

#[cfg(unix)]
struct Input<'a> {
    bytes: &'a [u8],
    written: usize,
    failed: bool,
}

#[cfg(unix)]
impl<'a> Input<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            written: 0,
            failed: false,
        }
    }

    fn complete(&self) -> bool {
        self.written == self.bytes.len() && !self.failed
    }
}

pub(crate) fn run(
    command: &mut Command,
    input: Option<&[u8]>,
    limits: ProcessLimits,
) -> Result<ProcessOutput, ProcessError> {
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (command, input, limits);
        return Err(ProcessError::UnsupportedPlatform);
    }
    #[cfg(windows)]
    {
        return windows::run(command, input, limits);
    }
    #[cfg(unix)]
    {
        let started = Instant::now();
        let deadline = started.checked_add(limits.timeout).unwrap_or(started);
        command.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        prepare_process_group(command);
        let mut child = command.spawn().map_err(|_| ProcessError::Unavailable)?;
        let Some(mut stdout) = child.stdout.take() else {
            stop(&mut child);
            return Err(ProcessError::Unavailable);
        };
        let Some(mut stderr) = child.stderr.take() else {
            stop(&mut child);
            return Err(ProcessError::Unavailable);
        };
        if set_nonblocking(stdout.as_raw_fd()).is_err()
            || set_nonblocking(stderr.as_raw_fd()).is_err()
        {
            stop(&mut child);
            return Err(ProcessError::Unavailable);
        }
        let mut stdin = if input.is_some() {
            let Some(stdin) = child.stdin.take() else {
                stop(&mut child);
                return Err(ProcessError::Unavailable);
            };
            if set_nonblocking(stdin.as_raw_fd()).is_err() {
                stop(&mut child);
                return Err(ProcessError::Unavailable);
            }
            Some(stdin)
        } else {
            None
        };
        let mut input = input.map(Input::new);
        let mut stdout_capture = Captured::new(limits.stdout_bytes);
        let mut stderr_capture = Captured::new(limits.stderr_bytes);
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut status = None;

        loop {
            if let (Some(writer), Some(input)) = (stdin.as_mut(), input.as_mut()) {
                if write_available(writer, input).is_err() {
                    stop(&mut child);
                    return Err(ProcessError::Unavailable);
                }
                if input.written == input.bytes.len() || input.failed {
                    stdin = None;
                }
            }
            if stdout_open {
                match drain_available(&mut stdout, &mut stdout_capture, limits.stdout_bytes) {
                    Ok(open) => stdout_open = open,
                    Err(error) => {
                        stop(&mut child);
                        return Err(error);
                    }
                }
            }
            if stderr_open {
                match drain_available(&mut stderr, &mut stderr_capture, limits.stderr_bytes) {
                    Ok(open) => stderr_open = open,
                    Err(error) => {
                        stop(&mut child);
                        return Err(error);
                    }
                }
            }
            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(finished)) => {
                        status = Some(finished);
                        stop_descendants(child.id());
                        stdin = None;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stop(&mut child);
                        return Err(ProcessError::Unavailable);
                    }
                }
            }
            if status.is_some() && !stdout_open && !stderr_open {
                if input.as_ref().is_some_and(|input| !input.complete()) {
                    return Err(ProcessError::Unavailable);
                }
                return Ok(ProcessOutput {
                    status: status.take().expect("status was checked"),
                    stdout: stdout_capture.bytes,
                    stderr: stderr_capture.bytes,
                    stdout_truncated: stdout_capture.truncated,
                    stderr_truncated: stderr_capture.truncated,
                });
            }
            let now = Instant::now();
            if now >= deadline {
                if status.is_none() {
                    stop(&mut child);
                } else {
                    stop_descendants(child.id());
                }
                // Closing our pipe ends makes an escaped writer observe EOF or
                // SIGPIPE; no background reader survives this return.
                drop(stdin);
                drop(stdout);
                drop(stderr);
                return Err(ProcessError::Timeout);
            }
            thread::sleep(POLL.min(deadline.saturating_duration_since(now)));
        }
    }
}

#[cfg(unix)]
fn set_nonblocking(file: libc::c_int) -> io::Result<()> {
    // SAFETY: `file` comes from one of this child's live pipe handles.
    let flags = unsafe { libc::fcntl(file, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptor remains owned by its Rust pipe handle.
    if unsafe { libc::fcntl(file, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn drain_available(
    reader: &mut impl io::Read,
    captured: &mut Captured,
    limit: usize,
) -> Result<bool, ProcessError> {
    let mut chunk = [0_u8; 8192];
    let mut drained = 0;
    while drained < DRAIN_BUDGET {
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(false),
            Ok(read) => {
                captured.push(&chunk[..read], limit);
                drained += read;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return Err(ProcessError::Unavailable),
        }
    }
    Ok(true)
}

#[cfg(unix)]
fn write_available(writer: &mut impl io::Write, input: &mut Input<'_>) -> io::Result<()> {
    while input.written < input.bytes.len() {
        match writer.write(&input.bytes[input.written..]) {
            Ok(0) => {
                input.failed = true;
                return Ok(());
            }
            Ok(written) => input.written += written,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {
                input.failed = true;
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(unix)]
fn stop(child: &mut Child) {
    stop_descendants(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn prepare_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(unix)]
fn stop_descendants(process_group: u32) {
    let Ok(process_group) = i32::try_from(process_group) else {
        return;
    };
    // SAFETY: the child is placed in a fresh process group whose id is its pid.
    // A negative pid addresses that group and never this process.
    unsafe {
        libc::kill(-process_group, libc::SIGKILL);
    }
}

#[cfg(windows)]
mod windows {
    //! The Windows road: a job object for containment, reader threads for the
    //! pipes, the same deadline loop and the same limits.

    use super::{
        Captured, POLL, ProcessError, ProcessLimits, ProcessOutput, READ_CHUNK, threads_of,
    };
    use std::io::Write as _;
    use std::os::windows::io::AsRawHandle as _;
    use std::os::windows::process::CommandExt as _;
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
    };

    /// Every thread on the machine as `(thread id, owner pid)` — one snapshot.
    fn thread_table() -> Vec<(u32, u32)> {
        let mut table = Vec::new();
        // SAFETY: a plain snapshot request; the handle is closed below.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return table;
        }
        let mut entry = THREADENTRY32::default();
        entry.dwSize = u32::try_from(std::mem::size_of::<THREADENTRY32>()).unwrap_or(0);
        // SAFETY: `entry` is a valid, sized THREADENTRY32 for the whole walk.
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            table.push((entry.th32ThreadID, entry.th32OwnerProcessID));
            // SAFETY: same live snapshot and entry as above.
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        // SAFETY: closing the snapshot handle we opened.
        unsafe {
            CloseHandle(snapshot);
        }
        table
    }

    /// Let a child created with `CREATE_SUSPENDED` run: resume its threads
    /// (a fresh suspended process has exactly its main one). Std keeps the
    /// main thread's handle to itself on stable, so the thread is found by
    /// its owner pid.
    fn resume(child: &Child) -> Result<(), ProcessError> {
        let threads = threads_of(child.id(), &thread_table());
        if threads.is_empty() {
            return Err(ProcessError::Unavailable);
        }
        for thread in threads {
            // SAFETY: opening one thread of our own child with the one right
            // that resumes it; closed below.
            let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread) };
            if handle.is_null() {
                return Err(ProcessError::Unavailable);
            }
            // SAFETY: a live thread handle with THREAD_SUSPEND_RESUME.
            let previous = unsafe { ResumeThread(handle) };
            // SAFETY: closing the handle we opened.
            unsafe {
                CloseHandle(handle);
            }
            if previous == u32::MAX {
                return Err(ProcessError::Unavailable);
            }
        }
        Ok(())
    }

    /// A job object that kills whatever is still inside it when the handle
    /// closes — so an escaped descendant dies with this call's return, not
    /// with the caller's patience.
    struct Job(HANDLE);

    impl Job {
        fn new() -> Result<Self, ProcessError> {
            // SAFETY: plain Win32 calls with null attributes and no name; the
            // returned handle is owned by this struct and closed in Drop.
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(ProcessError::Unavailable);
            }
            let job = Self(handle);
            // SAFETY: a zeroed repr(C) struct is a valid "no limits" value;
            // only the kill-on-close flag is set.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .map_err(|_| ProcessError::Unavailable)?;
            // SAFETY: `limits` outlives the call and `size` is its exact length.
            let set = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    std::ptr::from_ref(&limits).cast(),
                    size,
                )
            };
            if set == 0 {
                return Err(ProcessError::Unavailable);
            }
            Ok(job)
        }

        fn assign(&self, child: &Child) -> Result<(), ProcessError> {
            // SAFETY: the child's process handle is live for as long as `child`
            // is; the job handle is ours.
            let ok = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle().cast()) };
            if ok == 0 {
                return Err(ProcessError::Unavailable);
            }
            Ok(())
        }

        fn terminate(&self) {
            // SAFETY: terminating our own job; the exit code is arbitrary.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle was returned by CreateJobObjectW and is
            // closed exactly once, here. Kill-on-close ends any survivor.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Pipe {
        Stdout,
        Stderr,
    }

    enum Event {
        Chunk(Pipe, Vec<u8>),
        Closed(Pipe),
    }

    fn read_into(
        pipe: Pipe,
        mut reader: impl std::io::Read + Send + 'static,
        events: mpsc::Sender<Event>,
    ) {
        thread::spawn(move || {
            let mut chunk = vec![0_u8; READ_CHUNK];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => {
                        if events
                            .send(Event::Chunk(pipe, chunk[..read].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            let _ = events.send(Event::Closed(pipe));
        });
    }

    fn stop(job: &Job, child: &mut Child) {
        job.terminate();
        let _ = child.kill();
        let _ = child.wait();
    }

    pub(super) fn run(
        command: &mut Command,
        input: Option<&[u8]>,
        limits: ProcessLimits,
    ) -> Result<ProcessOutput, ProcessError> {
        let started = Instant::now();
        let deadline = started.checked_add(limits.timeout).unwrap_or(started);
        command.stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        // Created suspended, so nothing it would spawn can exist before it
        // is in the job: a child joined after it started could hand the
        // pipes to a grandchild (Git for Windows' `cmd\git.exe` launcher
        // starts the real git at once) that KILL_ON_JOB_CLOSE never sees.
        // This primitive owns the creation flags; callers set none.
        command.creation_flags(CREATE_SUSPENDED);
        let job = Job::new()?;
        let mut child = command.spawn().map_err(|_| ProcessError::Unavailable)?;
        if job.assign(&child).is_err() || resume(&child).is_err() {
            stop(&job, &mut child);
            return Err(ProcessError::Unavailable);
        }
        let Some(stdout) = child.stdout.take() else {
            stop(&job, &mut child);
            return Err(ProcessError::Unavailable);
        };
        let Some(stderr) = child.stderr.take() else {
            stop(&job, &mut child);
            return Err(ProcessError::Unavailable);
        };
        let (events, inbox) = mpsc::channel();
        read_into(Pipe::Stdout, stdout, events.clone());
        read_into(Pipe::Stderr, stderr, events);
        // The writer runs on its own thread because a full pipe blocks; it
        // reports whether every byte went through.
        let input_written = input.map(|bytes| {
            let bytes = bytes.to_vec();
            let stdin = child.stdin.take();
            thread::spawn(move || {
                let Some(mut stdin) = stdin else {
                    return false;
                };
                stdin.write_all(&bytes).is_ok() && stdin.flush().is_ok()
            })
        });
        let mut stdout_capture = Captured::new(limits.stdout_bytes);
        let mut stderr_capture = Captured::new(limits.stderr_bytes);
        let mut stdout_open = true;
        let mut stderr_open = true;
        let mut status = None;

        loop {
            let now = Instant::now();
            if now >= deadline {
                stop(&job, &mut child);
                return Err(ProcessError::Timeout);
            }
            match inbox.recv_timeout(POLL.min(deadline.saturating_duration_since(now))) {
                Ok(Event::Chunk(Pipe::Stdout, chunk)) => {
                    stdout_capture.push(&chunk, limits.stdout_bytes);
                }
                Ok(Event::Chunk(Pipe::Stderr, chunk)) => {
                    stderr_capture.push(&chunk, limits.stderr_bytes);
                }
                Ok(Event::Closed(Pipe::Stdout)) => stdout_open = false,
                Ok(Event::Closed(Pipe::Stderr)) => stderr_open = false,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    stdout_open = false;
                    stderr_open = false;
                }
            }
            if status.is_none() {
                match child.try_wait() {
                    Ok(Some(finished)) => {
                        status = Some(finished);
                        // The child is done; anything it left behind goes too.
                        job.terminate();
                    }
                    Ok(None) => {}
                    Err(_) => {
                        stop(&job, &mut child);
                        return Err(ProcessError::Unavailable);
                    }
                }
            }
            if status.is_some() && !stdout_open && !stderr_open {
                if let Some(writer) = input_written {
                    if !writer.join().unwrap_or(false) {
                        return Err(ProcessError::Unavailable);
                    }
                }
                return Ok(ProcessOutput {
                    status: status.take().expect("status was checked"),
                    stdout: stdout_capture.bytes,
                    stderr: stderr_capture.bytes,
                    stdout_truncated: stdout_capture.truncated,
                    stderr_truncated: stderr_capture.truncated,
                });
            }
        }
    }
}

#[cfg(all(test, any(unix, windows)))]
mod tests {
    //! The contract both roads keep, spelled with each platform's own shell.

    use super::{ProcessError, ProcessLimits, run};
    use std::process::Command;
    use std::time::{Duration, Instant};

    fn shell(script_unix: &str, script_windows: &str) -> Command {
        if cfg!(windows) {
            let mut command = Command::new("cmd");
            command.args(["/C", script_windows]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", script_unix]);
            command
        }
    }

    fn limits(timeout_ms: u64, bytes: usize) -> ProcessLimits {
        ProcessLimits {
            timeout: Duration::from_millis(timeout_ms),
            stdout_bytes: bytes,
            stderr_bytes: bytes,
        }
    }

    #[test]
    fn output_and_status_come_back_whole() {
        let output = run(
            &mut shell(
                "printf hello; printf oops >&2; exit 3",
                "<nul set /p =hello & <nul set /p =oops 1>&2 & exit 3",
            ),
            None,
            limits(5_000, 1024),
        )
        .expect("the shell runs");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
        assert_eq!(String::from_utf8_lossy(&output.stderr).trim(), "oops");
        assert_eq!(output.status.code(), Some(3));
        assert!(!output.stdout_truncated && !output.stderr_truncated);
    }

    #[test]
    fn stdin_reaches_the_child() {
        let output = run(
            &mut shell("cat", "findstr ."),
            Some(b"typed\n"),
            limits(5_000, 1024),
        )
        .expect("the shell runs");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "typed");
    }

    #[test]
    fn output_past_the_limit_is_cut_and_said() {
        let output = run(
            &mut shell(
                "i=0; while [ $i -lt 400 ]; do printf 0123456789; i=$((i+1)); done",
                "for /L %i in (1,1,400) do @<nul set /p =0123456789",
            ),
            None,
            limits(5_000, 100),
        )
        .expect("the shell runs");
        assert_eq!(output.stdout.len(), 100);
        assert!(output.stdout_truncated);
    }

    #[test]
    fn a_child_that_outlives_the_deadline_is_a_timeout_within_the_budget() {
        let started = Instant::now();
        let error = run(
            &mut shell("sleep 30", "ping -n 31 127.0.0.1 >nul"),
            None,
            limits(300, 1024),
        )
        .expect_err("the deadline wins");
        assert_eq!(error, ProcessError::Timeout);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the deadline loop returned after {:?}",
            started.elapsed()
        );
    }
}

/// The threads of `process` in a `(thread id, owner pid)` snapshot, which
/// lists every thread on the machine. Pure, so the pick is pinned on every
/// platform that builds this crate.
#[cfg(any(windows, test))]
fn threads_of(process: u32, table: &[(u32, u32)]) -> Vec<u32> {
    table
        .iter()
        .filter(|(_, owner)| *owner == process)
        .map(|(thread, _)| *thread)
        .collect()
}

#[cfg(test)]
mod containment_tests {
    //! The Windows containment order, pinned where it can be seen: this Mac
    //! compiles the Windows road only through a cross-check and cannot run
    //! it, so the order is read off the source and the one decision in it
    //! is a pure function.

    use super::threads_of;

    /// A job that a child joins after it started can be escaped by whatever
    /// the child spawned in between — Git for Windows' `cmd\git.exe`
    /// launcher starts the real `git.exe` at once, and that one then held
    /// the pipes and the index lock past the deadline. The child is created
    /// suspended, joins the job, and only then runs.
    #[test]
    fn the_windows_child_joins_its_job_before_it_runs() {
        let source = include_str!("bounded_process.rs");
        let windows = &source[source.find("mod windows {").expect("the windows road")..];
        let run = &windows[windows.find("pub(super) fn run(").expect("its run")..];
        let at = |needle: &str| {
            run.find(needle)
                .unwrap_or_else(|| panic!("the windows run lost {needle:?}"))
        };
        let suspended = at("creation_flags(CREATE_SUSPENDED)");
        let spawned = at("command.spawn()");
        let joined = at("job.assign(&child)");
        let resumed = at("resume(&child)");
        assert!(
            suspended < spawned && spawned < joined && joined < resumed,
            "spawned suspended, joined to the job, then resumed — in that order"
        );
    }

    /// Resuming a suspended child means resuming ITS threads: a thread
    /// snapshot lists every thread on the machine, and only the child's
    /// own are picked.
    #[test]
    fn only_the_childs_own_threads_are_resumed() {
        let table = [(11, 500), (12, 900), (13, 500), (14, 7)];
        assert_eq!(threads_of(500, &table), [11, 13]);
        assert!(threads_of(42, &table).is_empty());
    }
}
