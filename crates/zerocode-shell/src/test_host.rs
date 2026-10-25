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
    std::fs::create_dir_all(dir).expect("a folder for the program");
    let text = format!("#!/bin/sh\n{body}\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
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
        std::fs::write(
            &launcher,
            format!("@echo off\r\n{WINDOWS_SH} \"%~dp0{name}.sh\" %*\r\nexit /b %errorlevel%\r\n"),
        )
        .expect("write the program's launcher");
        launcher
    }
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
    let program = fake_program(
        dir,
        name,
        &format!(
            "{slow}[ \"$1\" != {FIRST_RUN} ] || {{ : > \"{finished}\"; exit 0; }}\n{body}",
            finished = finished.display()
        ),
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

/// First port of the loopback range tests take their own ports from.
const QUIET_FIRST: u16 = 20_000;
/// How many ports that range holds.
const QUIET_SPAN: u16 = 10_000;
/// Where this process starts in the range: the process id keeps it apart from
/// the next test process's start, and half a span on keeps it apart from the
/// callback-seat test in `google_login`, which starts at the process id itself
/// and so can never meet a port released here.
static QUIET_START: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
/// How many ports this process has already handed out.
static QUIET_STEP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Takes a loopback port that no test's `bind(0)` and no outgoing connection
/// can have, by running `bind` on candidates until one is taken (t-74682).
///
/// `bind(0)` and every outgoing connection in this test binary draw from the
/// ephemeral range (macOS and Windows 49152.., Linux 32768..). A port released
/// there can be taken by another test's listener before it is used again, and
/// that listener may be gone by the time anyone asks who held it. This range
/// lies below all of them. Each call also moves on from the last, so two callers
/// in one process never hold or release the same port in turn.
pub(crate) fn quiet_port<T>(mut bind: impl FnMut(u16) -> std::io::Result<T>) -> (T, u16) {
    let start = *QUIET_START
        .get_or_init(|| (std::process::id() + u32::from(QUIET_SPAN / 2)) % u32::from(QUIET_SPAN));
    for _ in 0..256 {
        let step = QUIET_STEP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let offset = (start + step) % u32::from(QUIET_SPAN);
        let port = QUIET_FIRST + u16::try_from(offset).expect("the offset is below the span");
        if let Ok(held) = bind(port) {
            return (held, port);
        }
    }
    panic!(
        "no free loopback port in {QUIET_FIRST}..{}",
        QUIET_FIRST + QUIET_SPAN
    );
}

/// A loopback address nobody listens on, and no test can come to listen on:
/// a connection to it is refused. Released at once, since the point is the
/// port's silence — which [`quiet_port`] keeps from every other test here.
pub(crate) fn silent_addr() -> std::net::SocketAddr {
    let (seat, port) =
        quiet_port(|port| std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)));
    drop(seat);
    std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_address_is_below_the_ephemeral_ranges_and_never_handed_out_twice() {
        let first = silent_addr();
        let second = silent_addr();
        assert_ne!(first.port(), second.port());
        for addr in [first, second] {
            assert!(addr.ip().is_loopback());
            assert!((QUIET_FIRST..QUIET_FIRST + QUIET_SPAN).contains(&addr.port()));
            assert!(
                std::net::TcpStream::connect(addr).is_err(),
                "nobody may answer on {addr}"
            );
        }
    }
}
