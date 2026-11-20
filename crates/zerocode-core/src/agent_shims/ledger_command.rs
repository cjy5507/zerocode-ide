//! The ledger's command as a process on this machine runs it: which installed
//! `zerocode-orc` it finds, and how it starts it. One copy for every ledger
//! client outside the window — zo's Agent tool and zo's build line
//! (I-t118403-W1: the line called a bare `zerocode-orc`, which finds nothing on
//! Windows, so no end the line told there went anywhere).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::{ORCHESTRATION_COMMAND, orchestration_program};

/// The installed ledger command a process finds: the first `zerocode-orc` on
/// `path` that is a command file — on Windows its `.ps1`, the one
/// [`orchestration_command`] runs — else the one beside the window's
/// `endpoint` ([`orchestration_program`]). A bare `zerocode-orc` finds only a
/// `zerocode-orc.exe` on Windows, and the window installs none: a shebang file,
/// a `.ps1` and a `.cmd`. An empty entry of `path` is skipped.
#[must_use]
pub fn find_orchestration_program(
    path: Option<&OsStr>,
    endpoint: Option<&Path>,
    windows: bool,
) -> Option<PathBuf> {
    let name = if windows {
        format!("{ORCHESTRATION_COMMAND}.ps1")
    } else {
        ORCHESTRATION_COMMAND.to_string()
    };
    // An empty entry is no folder: a shell reads it as the current one, and a
    // checkout's own `zerocode-orc` is not the window's.
    path.and_then(|path| {
        std::env::split_paths(path)
            .filter(|folder| !folder.as_os_str().is_empty())
            .map(|folder| folder.join(&name))
            .find(|program| command_file(program, windows))
    })
    .or_else(|| {
        endpoint
            .and_then(|endpoint| orchestration_program(endpoint, windows))
            .filter(|program| command_file(program, windows))
    })
}

/// The command that runs the ledger's `program`. On Windows the `.ps1` goes
/// through Windows PowerShell under the process policy every pane is given:
/// Rust refuses to start a `.cmd` with an argument that holds a line break,
/// and a briefing or a job's words may hold one. Elsewhere the program itself.
#[must_use]
pub fn orchestration_command(program: &Path, windows: bool) -> Command {
    if windows {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        command.arg(program);
        command
    } else {
        Command::new(program)
    }
}

/// A file a ledger client may run: on unix one with an execute bit, unless it
/// is the Windows `.ps1`, which PowerShell reads.
fn command_file(path: &Path, windows: bool) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        windows || metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = windows;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;

    use super::*;

    /// A command file at `path`, runnable on unix.
    fn installed(path: &Path) {
        fs::create_dir_all(path.parent().expect("a folder")).expect("folders");
        fs::write(path, "a fixture command; never run here").expect("written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("runnable");
        }
    }

    /// The first command on PATH wins, the `.ps1` on Windows; with none there,
    /// the one beside the window's endpoint; with neither, none.
    #[test]
    fn the_ledgers_command_is_found_on_path_first_and_beside_the_endpoint_after() {
        let root = tempfile::tempdir().expect("a folder");
        let (first, second) = (root.path().join("first"), root.path().join("second"));
        for name in ["zerocode-orc", "zerocode-orc.ps1", "zerocode-orc.cmd"] {
            installed(&second.join(name));
        }
        installed(&first.join("zerocode-orc.cmd"));
        let path = std::env::join_paths([&first, &second]).expect("a PATH");
        let endpoint = root.path().join("application/agent-hooks/endpoint.env");
        for windows in [false, true] {
            installed(&orchestration_program(&endpoint, windows).expect("a place"));
        }

        let on_path = |windows| {
            find_orchestration_program(Some(path.as_os_str()), Some(endpoint.as_path()), windows)
        };
        assert_eq!(on_path(false), Some(second.join("zerocode-orc")));
        assert_eq!(on_path(true), Some(second.join("zerocode-orc.ps1")));
        let empty = OsString::from(root.path().join("empty"));
        for windows in [false, true] {
            assert_eq!(
                find_orchestration_program(
                    Some(empty.as_os_str()),
                    Some(endpoint.as_path()),
                    windows
                ),
                orchestration_program(&endpoint, windows),
                "windows {windows}"
            );
            assert_eq!(
                find_orchestration_program(None, None, windows),
                None,
                "windows {windows}"
            );
        }
        assert_eq!(
            find_orchestration_program(
                Some(empty.as_os_str()),
                Some(Path::new("endpoint.env")),
                true
            ),
            None,
            "a relative endpoint names no window"
        );
    }

    /// On unix a file without an execute bit is no command to run — the
    /// Windows `.ps1` is read by PowerShell and needs none.
    #[cfg(unix)]
    #[test]
    fn a_posix_file_without_an_execute_bit_is_no_ledger_command() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().expect("a folder");
        for name in ["zerocode-orc", "zerocode-orc.ps1"] {
            installed(&root.path().join(name));
            fs::set_permissions(root.path().join(name), fs::Permissions::from_mode(0o644))
                .expect("not runnable");
        }
        let path = root.path().as_os_str();
        assert_eq!(find_orchestration_program(Some(path), None, false), None);
        assert_eq!(
            find_orchestration_program(Some(path), None, true),
            Some(root.path().join("zerocode-orc.ps1"))
        );
    }

    /// Windows runs the `.ps1` through Windows PowerShell, every argument after
    /// the script's path its own — a line break, quotes and `%PATH%` too;
    /// elsewhere the program runs itself.
    #[test]
    fn windows_runs_the_powershell_script_and_unix_the_command_itself() {
        let mut command = orchestration_command(Path::new("zerocode-orc.ps1"), true);
        command.args([
            "send",
            "--body",
            "first line\nquotes \" and %PATH% $(literal)",
        ]);
        assert_eq!(command.get_program(), "powershell.exe");
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
                "zerocode-orc.ps1",
                "send",
                "--body",
                "first line\nquotes \" and %PATH% $(literal)",
            ]
        );
        let command = orchestration_command(Path::new("/bin/zerocode-orc"), false);
        assert_eq!(command.get_program(), "/bin/zerocode-orc");
        assert_eq!(command.get_args().count(), 0);
    }
}
