//! What a test needs from the host it runs on: absolute paths spelled the
//! way the host spells them, a program that is a script, and the shell that
//! runs it.
//!
//! A test that names `/work` means "some absolute directory", and one that
//! writes `#!/bin/sh` means "some program that does this". Neither is true of
//! Windows as written: `/work` has no drive, and CreateProcess cannot start a
//! file whose first line is a shebang. These helpers keep the Unix spelling
//! unchanged and give Windows the spelling it can use, once, so no test has
//! to choose.

use std::path::{Path, PathBuf};

/// The drive a Windows test path is rooted on.
#[cfg(windows)]
const TEST_DRIVE: &str = "C:";

/// The shell a Windows runner has — Git for Windows puts `sh` on PATH, and
/// there is no `/bin`.
#[cfg(windows)]
const WINDOWS_SH: &str = "sh";

/// The `pwd` that prints the directory as the host itself spells it: the
/// runner's `sh` otherwise answers with an MSYS path (`/c/…`) the host cannot open.
#[cfg(windows)]
pub(crate) const SH_PHYSICAL_PWD: &str = "pwd -W";
#[cfg(not(windows))]
pub(crate) const SH_PHYSICAL_PWD: &str = "pwd -P";

/// An existing directory every host has, for a test whose subject must find
/// its checkout on disk.
#[cfg(unix)]
const SCRATCH_DIRECTORY: &str = "/tmp";

/// `unix` (an absolute unix path) as this host's absolute path.
pub(crate) fn absolute(unix: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!("{TEST_DRIVE}{unix}"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(unix)
    }
}

/// [`absolute`] as text.
pub(crate) fn absolute_text(unix: &str) -> String {
    absolute(unix).to_string_lossy().into_owned()
}

/// A directory that exists on this host, as text — `/tmp` where there is one.
pub(crate) fn existing_directory() -> &'static str {
    #[cfg(unix)]
    {
        SCRATCH_DIRECTORY
    }
    #[cfg(windows)]
    {
        static HELD: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        HELD.get_or_init(|| {
            std::env::temp_dir()
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_owned()
        })
    }
}

/// The program that runs a shell line: `/bin/sh`, or the runner's `sh`.
pub(crate) fn shell_program() -> &'static str {
    #[cfg(windows)]
    {
        WINDOWS_SH
    }
    #[cfg(not(windows))]
    {
        "/bin/sh"
    }
}

/// Where [`fake_program`] puts the program `name` in `dir` — known before it
/// is written, for a script that has to name a file beside itself.
pub(crate) fn program_path(dir: &Path, name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        dir.join(format!("{name}.cmd"))
    }
    #[cfg(not(windows))]
    {
        dir.join(name)
    }
}

/// Write a program that runs the shell script `body` and return the path to
/// start it by.
///
/// On Unix that is the script itself, executable, under `name`. On Windows
/// the script is written beside a `.cmd` that hands its arguments to the
/// runner's `sh`, and the `.cmd` is what comes back — `.sh` has no meaning
/// to CreateProcess, and a `.cmd` is the one script it runs.
pub(crate) fn fake_program(dir: &Path, name: &str, body: &str) -> PathBuf {
    fake_program_traced(dir, name, body, None)
}

/// [`fake_program`] whose launcher also writes down what it was handed, for
/// a test that has to say afterwards what reached the script and in what
/// spelling. `trace` is the file to append to and the one environment
/// variable to read back. A Unix program has no launcher, so there is
/// nothing to write: the script's own first lines are the only record.
pub(crate) fn fake_program_traced(
    dir: &Path,
    name: &str,
    body: &str,
    trace: Option<(&Path, &str)>,
) -> PathBuf {
    std::fs::create_dir_all(dir).expect("a folder for the program");
    let text = format!("#!/bin/sh\n{body}\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = trace;
        let script = program_path(dir, name);
        std::fs::write(&script, text).expect("write the program");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make the program executable");
        script
    }
    #[cfg(windows)]
    {
        let body_file = dir.join(format!("{name}.sh"));
        std::fs::write(&body_file, text).expect("write the program's script");
        let launcher = program_path(dir, name);
        std::fs::write(&launcher, windows_launcher(WINDOWS_SH, name, trace))
            .expect("write the program's launcher");
        launcher
    }
}

/// The text of the `.cmd` that starts a fake on Windows: it hands its
/// arguments to `shell` and returns the script's exit code.
///
/// With a `trace` it first appends two lines to the file: the arguments as
/// the batch file received them (`%*`, before any shell touched them) and the
/// one variable as the batch file sees it. The file is named before the `>>`
/// so an argument that ends in a digit is not read as a handle number.
fn windows_launcher(shell: &str, name: &str, trace: Option<(&Path, &str)>) -> String {
    let record = trace.map_or_else(String::new, |(file, var)| {
        let file = file.display();
        format!(
            ">>\"{file}\" echo wrapper-argv: %*\r\n>>\"{file}\" echo wrapper-env {var}=%{var}%\r\n"
        )
    });
    format!("@echo off\r\n{record}{shell} \"%~dp0{name}.sh\" %*\r\nexit /b %errorlevel%\r\n")
}

/// The one argument [`fake_cli_program`]'s own run of a fake hands it: the
/// fake marks that it ran and does nothing else.
const FIRST_RUN: &str = "zerocode-first-run";

/// [`fake_program`] for a fake CLI a test times against a wall. A new
/// program's first run can be slow on a loaded machine — here it spends
/// `first_start` seconds more, to stand for that — so the first run is made
/// here, before any wall starts, and the program comes back only once that
/// run has finished: every run a test times is a later one.
pub(crate) fn fake_cli_program(dir: &Path, name: &str, body: &str, first_start: u32) -> PathBuf {
    fake_cli_program_traced(dir, name, body, first_start, None)
}

/// [`fake_cli_program`] with the launcher's record of [`fake_program_traced`].
pub(crate) fn fake_cli_program_traced(
    dir: &Path,
    name: &str,
    body: &str,
    first_start: u32,
    trace: Option<(&Path, &str)>,
) -> PathBuf {
    let started = dir.join(format!("{name}.started"));
    let finished = dir.join(format!("{name}.first-run"));
    let slow = if first_start > 0 {
        format!(
            "[ -e \"{started}\" ] || {{ sleep {first_start}; : > \"{started}\"; }}\n",
            started = started.display()
        )
    } else {
        String::new()
    };
    let program = fake_program_traced(
        dir,
        name,
        &format!(
            "{slow}[ \"$1\" != {FIRST_RUN} ] || {{ : > \"{finished}\"; exit 0; }}\n{body}",
            finished = finished.display()
        ),
        trace,
    );
    let first = crate::proc::quiet_command(&program)
        .arg(FIRST_RUN)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    assert!(
        first.as_ref().is_ok_and(std::process::ExitStatus::success) && finished.exists(),
        "the fake `{name}` did not finish its first run: {first:?}"
    );
    std::fs::remove_file(&finished).expect("the first run's mark");
    program
}

/// A line the project-script runner executes that leaves an empty file at
/// `marker`: `touch` where the runner is bash, `type nul` where it is
/// `cmd.exe`.
pub(crate) fn script_that_creates(marker: &Path) -> String {
    #[cfg(windows)]
    {
        format!("type nul > \"{}\"", marker.display())
    }
    #[cfg(not(windows))]
    {
        format!("touch {}", marker.display())
    }
}

/// `path` resolved to the one spelling the host gives a place: git answers
/// the long name (`C:/Users/dev/…`) where the test holds the 8.3 short one
/// (with a `~1` in it), and both name the folder `canonicalize` reads back the
/// same way.
pub(crate) fn canonical(path: impl AsRef<Path>) -> PathBuf {
    std::fs::canonicalize(path.as_ref()).expect("a place that exists")
}

/// A program that prints its arguments and a newline: `/bin/echo`, or a
/// launcher over the runner's `sh` where there is no such file.
pub(crate) fn echo_program(dir: &Path) -> String {
    #[cfg(windows)]
    {
        fake_program(dir, "echo", "echo \"$*\"")
            .to_string_lossy()
            .into_owned()
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        "/bin/echo".to_owned()
    }
}

/// The one way a test takes a loopback port nobody else can come to (t-74864):
/// a silent port for a connection that must be refused, or a claimed one it
/// will bind, free and bind again. The source is shared with the other crates'
/// test binaries.
#[path = "../../../tools/test-support/quiet_ports.rs"]
pub(crate) mod quiet_ports;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_launcher_without_a_trace_only_hands_over() {
        assert_eq!(
            windows_launcher("sh", "tool", None),
            "@echo off\r\nsh \"%~dp0tool.sh\" %*\r\nexit /b %errorlevel%\r\n"
        );
    }

    #[test]
    fn a_traced_launcher_writes_down_its_arguments_and_variable_before_the_shell_runs() {
        let launcher = windows_launcher("sh", "tool", Some((Path::new("trace.log"), "HOME")));
        let lines: Vec<&str> = launcher.lines().collect();
        assert_eq!(
            lines,
            [
                "@echo off",
                ">>\"trace.log\" echo wrapper-argv: %*",
                ">>\"trace.log\" echo wrapper-env HOME=%HOME%",
                "sh \"%~dp0tool.sh\" %*",
                "exit /b %errorlevel%",
            ]
        );
    }
}
