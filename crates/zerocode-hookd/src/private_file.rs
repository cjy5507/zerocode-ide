//! A file only its owner may read — the 0600 of a credential or token file,
//! spelled for each platform.
//!
//! Unix: `chmod 0600`. Windows: the profile's inherited ACL usually keeps a
//! file to its owner already, but "usually" is not a promise — a token
//! written under a shared or relocated home inherits whatever that folder
//! grants. So the file's inheritance is cut and one grant is left, to the
//! well-known OWNER RIGHTS principal (`S-1-3-4`, whoever owns the file),
//! through `icacls`, the system tool for exactly this. Before 2026-09-11 the
//! Windows road was a no-op (docs/design/windows-parity-audit-20260910.md
//! §3, hookd row).

use std::io;
use std::path::Path;

/// Owner read/write only.
pub(crate) const PRIVATE_FILE_MODE: u32 = 0o600;

/// Whether a unix mode asks for a private file (no group or other bits).
#[cfg(any(not(unix), test))]
#[must_use]
pub(crate) fn mode_is_private(mode: u32) -> bool {
    mode & 0o077 == 0
}

/// The permission bits `path` has now, so a rewrite can put them back.
/// `None` off unix, where a mode is not this file's vocabulary, and when the
/// file is not there.
pub(crate) fn mode_of(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .ok()
            .map(|meta| meta.permissions().mode())
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Give `path` the unix `mode` a caller recorded or asked for.
///
/// Unix sets exactly that mode: a file put back is put back as it was, so a
/// 0400 or 0700 file is not "approximately private" 0600 afterwards.
/// Elsewhere a mode is unix's vocabulary and only its owner-only meaning has
/// a road ([`make_private`]); any other mode says nothing there.
pub(crate) fn apply_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        if mode_is_private(mode) {
            make_private(path)
        } else {
            Ok(())
        }
    }
}

/// Replace `path` whole with `body`, keeping the permissions it has: a
/// settings file the person keeps at 0600 (its `env` may hold a key) stays
/// 0600. A symlinked file is written through and stays a link
/// ([`crate::codex_install::write_path`]); a missing file gets the default.
pub(crate) fn replace_keeping_mode(path: &Path, body: &str) -> io::Result<()> {
    let target = crate::codex_install::write_path(path);
    write_atomically(&target, body.as_bytes(), mode_of(&target))
}

/// Through a sibling temp and a rename, so no reader ever sees half a file.
/// The temp is born with `mode` on unix and gets it exactly (or, elsewhere,
/// its owner-only ACL) before the rename; it is removed whatever step fails.
pub(crate) fn write_atomically(path: &Path, body: &[u8], mode: Option<u32>) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    zerocode_core::atomic_file::write_atomically_sealed(path, body, mode, |temp| {
        mode.map_or(Ok(()), |mode| apply_mode(temp, mode))
    })
}

/// Make `path` readable and writable by its owner alone.
pub(crate) fn make_private(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(PRIVATE_FILE_MODE))
    }
    #[cfg(windows)]
    {
        windows::make_private(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err(io::Error::other("no private-file road on this platform"))
    }
}

#[cfg(windows)]
mod windows {
    use std::io;
    use std::path::Path;
    use std::process::Command;

    /// The well-known OWNER RIGHTS SID: rights for whoever owns the object.
    const OWNER_RIGHTS_SID: &str = "*S-1-3-4";

    pub(super) fn make_private(path: &Path) -> io::Result<()> {
        let output = Command::new("icacls")
            .arg(path)
            .args([
                "/inheritance:r",
                "/grant:r",
                &format!("{OWNER_RIGHTS_SID}:F"),
            ])
            .output()?;
        if output.status.success() {
            return Ok(());
        }
        Err(io::Error::other(format!(
            "icacls refused to make {} private: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    /// The principals `icacls` lists for a path — the test's witness.
    #[cfg(test)]
    pub(super) fn principals(path: &Path) -> io::Result<String> {
        let output = Command::new("icacls").arg(path).output()?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_private_file_is_the_owners_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("token");
        std::fs::write(&path, "secret").expect("write");
        make_private(&path).expect("make private");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        #[cfg(windows)]
        {
            let listed = windows::principals(&path).expect("icacls");
            assert!(
                !listed.contains("BUILTIN\\Users") && !listed.contains("Authenticated Users"),
                "a private file still grants a group: {listed}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(&path).expect("owner reads"),
            "secret"
        );
    }

    #[test]
    fn only_owner_only_modes_are_private() {
        assert!(mode_is_private(0o600));
        assert!(mode_is_private(0o700));
        assert!(!mode_is_private(0o644));
        assert!(!mode_is_private(0o660));
    }
}
