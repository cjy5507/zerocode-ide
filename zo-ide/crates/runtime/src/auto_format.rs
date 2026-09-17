use std::fs;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const FORMATTER_STDERR_LIMIT: usize = 64 * 1024;
const FORMATTER_STDERR_TRUNCATED_NOTICE: &[u8] = b"\n[formatter stderr truncated]\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Formatter {
    Rustfmt,
    Prettier,
    Ruff,
    Gofmt,
    ClangFormat,
    ShFmt,
}

impl Formatter {
    fn command_args(self, path: &str) -> (&'static str, Vec<&str>) {
        match self {
            Self::Rustfmt => (
                "rustfmt",
                vec!["--edition", "2024", "--config", "skip_children=true", path],
            ),
            Self::Prettier => ("prettier", vec!["--write", path]),
            Self::Ruff => ("ruff", vec!["format", path]),
            Self::Gofmt => ("gofmt", vec!["-w", path]),
            Self::ClangFormat => ("clang-format", vec!["-i", path]),
            Self::ShFmt => ("shfmt", vec!["-w", path]),
        }
    }
}

impl std::fmt::Display for Formatter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rustfmt => write!(f, "rustfmt"),
            Self::Prettier => write!(f, "prettier"),
            Self::Ruff => write!(f, "ruff"),
            Self::Gofmt => write!(f, "gofmt"),
            Self::ClangFormat => write!(f, "clang-format"),
            Self::ShFmt => write!(f, "shfmt"),
        }
    }
}

#[must_use]
pub fn formatter_for_path(path: &Path) -> Option<Formatter> {
    let ext = path.extension()?.to_str()?;
    match ext {
        "rs" => Some(Formatter::Rustfmt),
        "ts" | "tsx" | "js" | "jsx" | "json" | "css" | "scss" | "html" | "md" | "yaml" | "yml" => {
            Some(Formatter::Prettier)
        }
        "py" | "pyi" => Some(Formatter::Ruff),
        "go" => Some(Formatter::Gofmt),
        "c" | "cpp" | "cc" | "h" | "hpp" => Some(Formatter::ClangFormat),
        "sh" | "bash" | "zsh" => Some(Formatter::ShFmt),
        _ => None,
    }
}

#[must_use]
pub fn format_file(path: &str) -> Option<FormatResult> {
    format_file_inner(path, None)
}

/// Format a file with a hard wall-clock budget.
///
/// Unlike wrapping [`format_file`] in a worker thread, this function owns the
/// formatter child process directly. If the budget expires, it kills and waits
/// for the child (and, on Unix, its process group) before returning `None` so a
/// wedged formatter cannot leak a worker thread or subprocess.
#[must_use]
pub fn format_file_with_timeout(path: &str, budget: Duration) -> Option<FormatResult> {
    if budget.is_zero() {
        return None;
    }
    format_file_inner(path, Some(budget))
}

fn format_file_inner(path: &str, budget: Option<Duration>) -> Option<FormatResult> {
    let p = Path::new(path);
    let formatter = formatter_for_path(p)?;
    let (cmd, args) = formatter.command_args(path);

    if !which_exists(cmd) {
        return None;
    }

    let output = run_formatter(cmd, &args, budget)?;
    Some(formatter_output_to_result(formatter, &output))
}

fn formatter_output_to_result(formatter: Formatter, output: &FormatterOutput) -> FormatResult {
    if output.status.success() {
        FormatResult {
            formatter,
            success: true,
            message: None,
        }
    } else {
        FormatResult {
            formatter,
            success: false,
            message: Some(String::from_utf8_lossy(&output.stderr).trim().to_string()),
        }
    }
}

struct FormatterOutput {
    status: ExitStatus,
    stderr: Vec<u8>,
}

fn run_formatter(cmd: &str, args: &[&str], budget: Option<Duration>) -> Option<FormatterOutput> {
    match budget {
        Some(budget) => run_formatter_with_timeout(cmd, args, budget),
        None => Command::new(cmd)
            .args(args)
            .output()
            .ok()
            .map(|output| FormatterOutput {
                status: output.status,
                stderr: output.stderr,
            }),
    }
}

fn run_formatter_with_timeout(
    cmd: &str,
    args: &[&str],
    budget: Duration,
) -> Option<FormatterOutput> {
    if budget.is_zero() {
        return None;
    }

    // Capture stderr via an exclusive temp file instead of a pipe. A formatter
    // that emits enough diagnostics to fill a pipe would otherwise block before
    // `try_wait` can observe completion, falsely looking like a timeout. The
    // exclusive create avoids following a pre-existing symlink in a shared temp
    // directory.
    let (stderr_path, stderr_file) = create_temp_stderr_file()?;

    let mut command = Command::new(cmd);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file));
    // Make the formatter the leader of its own process group so a wedged
    // wrapper script plus grandchildren are all signalled on timeout.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    #[cfg(windows)]
    let child_result = {
        use process_wrap::std::{CommandWrap, JobObject};

        // Formatter wrappers can spawn their own children. A Job Object makes
        // timeout cleanup cover the whole tree instead of only the direct
        // `cmd`/formatter process.
        let mut wrapped = CommandWrap::from(command);
        wrapped.wrap(JobObject);
        wrapped.spawn()
    };
    #[cfg(not(windows))]
    let child_result = command.spawn();

    let Ok(mut child) = child_result else {
        let _ = fs::remove_file(&stderr_path);
        return None;
    };
    #[cfg(unix)]
    let child_pid = child.id();
    let started = Instant::now();

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= budget => {
                #[cfg(unix)]
                terminate_process_group_or_log(child_pid);
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&stderr_path);
                return None;
            }
            Ok(None) => {
                let remaining = budget.saturating_sub(started.elapsed());
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            }
            Err(_) => {
                #[cfg(unix)]
                terminate_process_group_or_log(child_pid);
                let _ = child.kill();
                let _ = child.wait();
                let _ = fs::remove_file(&stderr_path);
                return None;
            }
        }
    };

    #[cfg(windows)]
    {
        // `try_wait` may observe the direct formatter after it exited while a
        // descendant is still active. Terminate the remainder before returning.
        let _ = child.start_kill();
        let _ = child.wait();
    }

    let stderr = read_formatter_stderr(&stderr_path);
    let _ = fs::remove_file(&stderr_path);
    Some(FormatterOutput { status, stderr })
}

fn read_formatter_stderr(path: &Path) -> Vec<u8> {
    let Ok(file) = fs::File::open(path) else {
        return Vec::new();
    };
    let mut stderr = Vec::with_capacity(FORMATTER_STDERR_LIMIT.min(8 * 1024));
    let mut limited = file.take((FORMATTER_STDERR_LIMIT + 1) as u64);
    if limited.read_to_end(&mut stderr).is_err() {
        return Vec::new();
    }
    if stderr.len() > FORMATTER_STDERR_LIMIT {
        stderr.truncate(FORMATTER_STDERR_LIMIT);
        stderr.extend_from_slice(FORMATTER_STDERR_TRUNCATED_NOTICE);
    }
    stderr
}

fn create_temp_stderr_file() -> Option<(PathBuf, fs::File)> {
    for _ in 0..16 {
        let path = temp_stderr_path();
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Some((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return None,
        }
    }
    None
}

fn temp_stderr_path() -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "zo-auto-format-stderr-{}-{unique}.log",
        std::process::id()
    ))
}

#[cfg(unix)]
fn terminate_process_group_or_log(pid: u32) {
    if let Err(error) = terminate_process_group(pid) {
        eprintln!("auto-format timeout: failed to signal formatter process group {pid}: {error}");
    }
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) -> Result<(), nix::errno::Errno> {
    use nix::errno::Errno;
    use nix::sys::signal::{self, Signal};
    use nix::unistd::Pid;

    let Ok(pid) = i32::try_from(pid) else {
        return Err(Errno::EINVAL);
    };
    let process_group = Pid::from_raw(-pid);
    // ESRCH: the group is already gone. EPERM: macOS (XNU) reports EPERM for a
    // group whose remaining members are all zombies — the formatter exited
    // right at the deadline and hasn't been reaped yet. We spawned this group
    // under our own uid, so a real privilege mismatch is impossible; both mean
    // "nothing left to stop" and the caller's `kill()`+`wait()` reaps the rest.
    match signal::kill(process_group, Signal::SIGTERM) {
        Ok(()) => {}
        Err(Errno::ESRCH | Errno::EPERM) => return Ok(()),
        Err(error) => return Err(error),
    }
    std::thread::sleep(Duration::from_millis(50));
    match signal::kill(process_group, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH | Errno::EPERM) => Ok(()),
        Err(error) => Err(error),
    }
}

fn which_exists(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }

    let cmd_path = Path::new(cmd);
    if cmd_path.components().count() > 1 {
        return is_executable_file(cmd_path);
    }

    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| command_exists_in_dir(&dir, cmd))
}

#[cfg(not(windows))]
fn command_exists_in_dir(dir: &Path, cmd: &str) -> bool {
    is_executable_file(&dir.join(cmd))
}

#[cfg(windows)]
fn command_exists_in_dir(dir: &Path, cmd: &str) -> bool {
    let direct = dir.join(cmd);
    if direct.extension().is_some() && is_executable_file(&direct) {
        return true;
    }

    let pathext = std::env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
    pathext
        .to_string_lossy()
        .split(';')
        .filter_map(|ext| {
            let ext = ext.trim();
            if ext.is_empty() {
                return None;
            }
            Some(if ext.starts_with('.') {
                format!("{cmd}{ext}")
            } else {
                format!("{cmd}.{ext}")
            })
        })
        .any(|candidate| is_executable_file(&dir.join(candidate)))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && is_executable_metadata(&metadata)
}

#[cfg(unix)]
fn is_executable_metadata(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_metadata(_metadata: &fs::Metadata) -> bool {
    true
}

#[derive(Debug, Clone)]
pub struct FormatResult {
    pub formatter: Formatter,
    pub success: bool,
    pub message: Option<String>,
}

impl std::fmt::Display for FormatResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.success {
            write!(f, "Formatted with {}", self.formatter)
        } else {
            write!(
                f,
                "{} formatting failed: {}",
                self.formatter,
                self.message.as_deref().unwrap_or("unknown error")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_path(name: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "zo-auto-format-{name}-{}-{unique}",
            std::process::id()
        ))
    }

    #[test]
    fn formatter_selection() {
        assert_eq!(
            formatter_for_path(Path::new("main.rs")),
            Some(Formatter::Rustfmt)
        );
        assert_eq!(
            formatter_for_path(Path::new("app.tsx")),
            Some(Formatter::Prettier)
        );
        assert_eq!(
            formatter_for_path(Path::new("main.py")),
            Some(Formatter::Ruff)
        );
        assert_eq!(
            formatter_for_path(Path::new("main.go")),
            Some(Formatter::Gofmt)
        );
        assert_eq!(
            formatter_for_path(Path::new("lib.c")),
            Some(Formatter::ClangFormat)
        );
        assert_eq!(
            formatter_for_path(Path::new("build.sh")),
            Some(Formatter::ShFmt)
        );
        assert_eq!(formatter_for_path(Path::new("data.bin")), None);
    }

    #[test]
    fn rustfmt_command_formats_only_the_requested_file() {
        let (cmd, args) = Formatter::Rustfmt.command_args("src/providers/mod.rs");
        assert_eq!(cmd, "rustfmt");
        assert_eq!(
            args,
            vec![
                "--edition",
                "2024",
                "--config",
                "skip_children=true",
                "src/providers/mod.rs",
            ]
        );
    }

    #[test]
    fn display_impl_works() {
        let r = FormatResult {
            formatter: Formatter::Rustfmt,
            success: true,
            message: None,
        };
        assert_eq!(r.to_string(), "Formatted with rustfmt");
    }

    #[test]
    fn formatter_stderr_read_is_capped() {
        let path = unique_temp_path("stderr").with_extension("log");
        fs::write(&path, vec![b'x'; FORMATTER_STDERR_LIMIT + 32]).expect("write stderr fixture");

        let stderr = read_formatter_stderr(&path);

        assert_eq!(
            stderr.len(),
            FORMATTER_STDERR_LIMIT + FORMATTER_STDERR_TRUNCATED_NOTICE.len()
        );
        assert!(
            stderr[..FORMATTER_STDERR_LIMIT]
                .iter()
                .all(|byte| *byte == b'x')
        );
        assert!(stderr.ends_with(FORMATTER_STDERR_TRUNCATED_NOTICE));
        let _ = fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn command_lookup_requires_executable_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = unique_temp_path("cmd-dir");
        fs::create_dir_all(&dir).expect("create command dir");
        let command = dir.join("fakefmt");
        fs::write(&command, "#!/bin/sh\n").expect("write command");
        let mut perms = fs::metadata(&command)
            .expect("command metadata")
            .permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&command, perms).expect("chmod non-executable");
        assert!(!command_exists_in_dir(&dir, "fakefmt"));

        let mut perms = fs::metadata(&command)
            .expect("command metadata")
            .permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&command, perms).expect("chmod executable");
        assert!(command_exists_in_dir(&dir, "fakefmt"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn formatter_timeout_kills_process_group() {
        use std::os::unix::fs::PermissionsExt;

        // Under heavy parallel load shell startup can outlast the formatter
        // budget, so the grandchild pid file is never written and that run
        // proves nothing about group kill. Retry with a fresh scenario
        // instead of flaking on the missing pid file.
        for attempt in 0..3 {
            let root = unique_temp_path(&format!("timeout-{attempt}"));
            fs::create_dir_all(&root).expect("create temp dir");
            let script = root.join("fake-formatter.sh");
            let child_pid_file = root.join("child.pid");
            fs::write(&script, "#!/bin/sh\n(sleep 30) &\necho $! > \"$1\"\nwait\n")
                .expect("write formatter script");
            let mut perms = fs::metadata(&script)
                .expect("script metadata")
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).expect("chmod formatter script");

            let started = Instant::now();
            let result = run_formatter_with_timeout(
                script.to_str().expect("utf-8 script path"),
                &[child_pid_file.to_str().expect("utf-8 pid path")],
                Duration::from_millis(1_000),
            );

            assert!(result.is_none(), "timed-out formatter returns no result");
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "timeout should return promptly, not wait out the formatter"
            );

            let recorded_pid = fs::read_to_string(&child_pid_file)
                .ok()
                .and_then(|raw| raw.trim().parse::<i32>().ok());
            let Some(child_pid) = recorded_pid else {
                // The shell was killed before recording the grandchild pid:
                // the scenario never armed. Try again.
                let _ = fs::remove_dir_all(&root);
                continue;
            };

            let mut alive = true;
            for _ in 0..40 {
                let still_alive = process_exists(child_pid);
                if !still_alive {
                    alive = false;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if alive {
                kill_process(child_pid);
            }
            let _ = fs::remove_dir_all(&root);
            assert!(
                !alive,
                "formatter timeout must kill the formatter process group"
            );
            return;
        }
        panic!("formatter script never recorded the grandchild pid in 3 attempts");
    }

    #[cfg(unix)]
    fn process_exists(pid: i32) -> bool {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;

        signal::kill(Pid::from_raw(pid), None::<Signal>).is_ok()
    }

    #[cfg(unix)]
    fn kill_process(pid: i32) {
        use nix::sys::signal::{self, Signal};
        use nix::unistd::Pid;

        let _ = signal::kill(Pid::from_raw(pid), Signal::SIGKILL);
    }
}
