//! Finding `zo` — the independence contract, in code.
//!
//! ZeroCode is an IDE first and an agent front-end second. It must open a
//! folder, edit files, show diffs and run a terminal on a machine that has
//! never heard of `zo`. So the harness is **discovered, not depended on**: this
//! module answers "is `zo` on this machine?" and every agent surface is built
//! to fold away when the answer is no.
//!
//! There is deliberately no bundling, no installer, and no version pin. If the
//! user upgrades `zo`, the IDE follows with no rebuild.

use std::path::{Path, PathBuf};

/// Executable name, per platform.
#[cfg(windows)]
pub const ZO_EXECUTABLE: &str = "zo.exe";
/// Executable name, per platform.
#[cfg(not(windows))]
pub const ZO_EXECUTABLE: &str = "zo";

/// Where a pane's `zo` writes the address its events channel really bound.
///
/// The window asks for port zero, then reads the kernel-selected address from
/// this file. This spelling is the public contract shared with `zo`.
pub const ZO_EVENTS_ADDR_FILE_ENV: &str = "ZO_EVENTS_ADDR_FILE";

/// A `zo` the IDE may call. Absence is a normal state, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoBinary {
    pub path: PathBuf,
}

impl ZoBinary {
    /// Look for `zo` on `PATH`, then in conventional user and package-manager
    /// binary directories.
    ///
    /// Returns `None` when it is not installed — the caller folds the lane
    /// surfaces away and the rest of the IDE carries on.
    #[must_use]
    pub fn discover() -> Option<Self> {
        let fallback_dirs = well_known_fallback_dirs(std::env::var_os("HOME").as_deref());
        Self::discover_in_with_fallbacks(std::env::var_os("PATH").as_deref(), &fallback_dirs)
    }

    /// `discover` against an explicit `PATH`, so the contract is testable
    /// without touching the machine's real environment. This explicit form
    /// does not consult the conventional fallback directories.
    #[must_use]
    pub fn discover_in(path_var: Option<&std::ffi::OsStr>) -> Option<Self> {
        Self::discover_in_with_fallbacks(path_var, &[])
    }

    /// `discover` against an explicit `PATH` and ordered fallback directories.
    ///
    /// `PATH` always wins. Supplying the fallbacks separately keeps both
    /// search stages testable without reading the machine's environment.
    #[must_use]
    pub fn discover_in_with_fallbacks(
        path_var: Option<&std::ffi::OsStr>,
        fallback_dirs: &[PathBuf],
    ) -> Option<Self> {
        let path_candidates = path_var
            .into_iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join(ZO_EXECUTABLE));
        let fallback_candidates = fallback_dirs.iter().map(|dir| dir.join(ZO_EXECUTABLE));
        path_candidates
            .chain(fallback_candidates)
            .find(|candidate| is_executable_file(candidate))
            .map(|path| Self { path })
    }

    /// The command that attaches a lane to a session on a running `zo serve`.
    /// This is the whole continuity story: the server owns the session, the PTY
    /// only hosts a client, so closing the window detaches instead of
    /// destroying work.
    ///
    /// `None` means **start a new session**, and the id is then omitted rather
    /// than invented — `zo attach [SESSION_ID]` creates a session only when
    /// none is named. Passing a placeholder like `"new"` attaches to a session
    /// literally called `new`, which the server never registers, and the lane
    /// still renders a screen. That is what makes the mistake quiet: it looks
    /// right and nothing is persisted.
    #[must_use]
    pub fn attach_args(session_id: Option<&str>, bind_addr: &str) -> Vec<String> {
        let mut args = vec!["attach".to_string()];
        if let Some(session_id) = session_id {
            args.push(session_id.to_string());
        }
        args.push("--bind".to_string());
        args.push(bind_addr.to_string());
        args
    }

    /// The command that opens one IDE pane and its private events channel.
    ///
    /// A pane owns its session; the window learns the actual id later through
    /// `session.info`. `resume` therefore follows `--resume`, while a new pane
    /// carries no invented id. A non-UTF-8 cwd is omitted because the PTY's cwd
    /// remains authoritative and a lossy argument would name another path.
    #[must_use]
    pub fn pane_args(bind_addr: &str, cwd: Option<&Path>, resume: Option<&str>) -> Vec<String> {
        let mut args = vec!["--events-bind".to_string(), bind_addr.to_string()];
        if let Some(cwd) = cwd.and_then(Path::to_str) {
            args.push("--cwd".to_string());
            args.push(cwd.to_string());
        }
        if let Some(session_id) = resume {
            args.push("--resume".to_string());
            args.push(session_id.to_string());
        }
        args
    }

    /// The command that starts the session server on loopback.
    #[must_use]
    pub fn serve_args(bind_addr: &str) -> Vec<String> {
        vec![
            "serve".to_string(),
            "--bind".to_string(),
            bind_addr.to_string(),
        ]
    }
}

#[cfg(unix)]
fn well_known_fallback_dirs(home: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    let home = home.map(PathBuf::from);
    let mut dirs = Vec::with_capacity(4);
    if let Some(home) = &home {
        dirs.push(home.join(".local/bin"));
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    if let Some(home) = home {
        dirs.push(home.join("bin"));
    }
    dirs
}

#[cfg(not(unix))]
fn well_known_fallback_dirs(_home: Option<&std::ffi::OsStr>) -> Vec<PathBuf> {
    Vec::new()
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    // Windows has no execute bit; being a file on PATH under the right name is
    // as much as we can check without running it.
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The independence contract's own test: a machine with no `zo` answers
    /// `None`, and `None` is a normal state — the module doc's first
    /// paragraph, held by an assertion instead of prose alone.
    #[test]
    fn absence_is_a_normal_state_not_an_error() {
        assert_eq!(ZoBinary::discover_in(None), None);
        let bare = tempfile::tempdir().expect("tempdir");
        let path_var = std::env::join_paths([bare.path()]).expect("join");
        assert_eq!(ZoBinary::discover_in(Some(&path_var)), None);
    }

    /// And presence is found — first hit on PATH wins, plain files that are
    /// not executable do not count.
    #[cfg(unix)]
    #[test]
    fn a_real_zo_on_path_is_found_and_a_plain_file_is_not() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let zo = dir.path().join(ZO_EXECUTABLE);
        std::fs::write(&zo, b"#!/bin/sh\n").expect("write");
        let path_var = std::env::join_paths([dir.path()]).expect("join");
        // Present but not executable: still absent.
        assert_eq!(ZoBinary::discover_in(Some(&path_var)), None);
        std::fs::set_permissions(&zo, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert_eq!(
            ZoBinary::discover_in(Some(&path_var)).map(|found| found.path),
            Some(zo)
        );
    }

    #[cfg(unix)]
    #[test]
    fn fallback_finds_zo_when_path_has_no_executable() {
        use std::os::unix::fs::PermissionsExt;

        let path_dir = tempfile::tempdir().expect("path tempdir");
        let fallback_dir = tempfile::tempdir().expect("fallback tempdir");
        let fallback_zo = fallback_dir.path().join(ZO_EXECUTABLE);
        std::fs::write(&fallback_zo, b"#!/bin/sh\n").expect("write fallback zo");
        std::fs::set_permissions(&fallback_zo, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fallback zo");
        let path_var = std::env::join_paths([path_dir.path()]).expect("join PATH");

        let found = ZoBinary::discover_in_with_fallbacks(
            Some(&path_var),
            &[fallback_dir.path().to_path_buf()],
        );

        assert_eq!(found.map(|binary| binary.path), Some(fallback_zo));
    }

    #[cfg(unix)]
    #[test]
    fn path_takes_precedence_over_fallback_directories() {
        use std::os::unix::fs::PermissionsExt;

        let path_dir = tempfile::tempdir().expect("path tempdir");
        let fallback_dir = tempfile::tempdir().expect("fallback tempdir");
        let path_zo = path_dir.path().join(ZO_EXECUTABLE);
        let fallback_zo = fallback_dir.path().join(ZO_EXECUTABLE);
        for zo in [&path_zo, &fallback_zo] {
            std::fs::write(zo, b"#!/bin/sh\n").expect("write zo");
            std::fs::set_permissions(zo, std::fs::Permissions::from_mode(0o755)).expect("chmod zo");
        }
        let path_var = std::env::join_paths([path_dir.path()]).expect("join PATH");

        let found = ZoBinary::discover_in_with_fallbacks(
            Some(&path_var),
            &[fallback_dir.path().to_path_buf()],
        );

        assert_eq!(found.map(|binary| binary.path), Some(path_zo));
    }

    #[test]
    fn absence_from_path_and_fallbacks_returns_none() {
        let path_dir = tempfile::tempdir().expect("path tempdir");
        let fallback_dir = tempfile::tempdir().expect("fallback tempdir");
        let path_var = std::env::join_paths([path_dir.path()]).expect("join PATH");

        let found = ZoBinary::discover_in_with_fallbacks(
            Some(&path_var),
            &[fallback_dir.path().to_path_buf()],
        );

        assert_eq!(found, None);
    }

    #[cfg(unix)]
    #[test]
    fn well_known_fallbacks_keep_the_deployment_directory_first() {
        let home = Path::new("/Users/dev");

        let fallback_dirs = well_known_fallback_dirs(Some(home.as_os_str()));

        assert_eq!(
            fallback_dirs,
            [
                PathBuf::from("/Users/dev/.local/bin"),
                PathBuf::from("/opt/homebrew/bin"),
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/Users/dev/bin"),
            ]
        );
    }
}
