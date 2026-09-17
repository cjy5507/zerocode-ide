//! Which session server a project talks to.
//!
//! Shared by every entry point — the terminal CLI and the window must derive
//! the **same** address from the same project, or a lane opened in one is
//! invisible to the other and "the window and the terminal share a session
//! pool" quietly stops being true.

use std::path::{Path, PathBuf};

/// The loopback address this project's session server uses when nothing says
/// otherwise.
///
/// **Derived from the project, not fixed.** A single default put every checkout
/// on one port, so two projects either shared a session pool — mixing work that
/// has nothing to do with each other — or, with different tokens, refused to
/// start at all. Neither is a usable answer for someone with two repositories
/// open.
///
/// The port is a stable function of the project root, so reattaching finds the
/// same server tomorrow, and running from a subdirectory finds it too. It sits
/// in the dynamic range IANA reserves for exactly this. A collision with
/// unrelated software is still possible and still safe: the probe reports a
/// foreign listener and refuses rather than attaching.
#[must_use]
pub fn default_bind_for(root: &Path) -> String {
    format!("127.0.0.1:{}", project_port(root))
}

/// Dynamic-range port for a project root, stable across runs and machines.
#[must_use]
pub fn project_port(root: &Path) -> u16 {
    const FIRST_DYNAMIC_PORT: u32 = 49_152;
    const DYNAMIC_PORTS: u32 = 65_536 - FIRST_DYNAMIC_PORT;

    // FNV-1a: stable across runs, unlike the hasher behind `HashMap`. A port
    // that moved every restart would strand the session pool it named.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in root.as_os_str().as_encoded_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let offset = u32::try_from(hash % u64::from(DYNAMIC_PORTS)).unwrap_or(0);
    u16::try_from(FIRST_DYNAMIC_PORT + offset).unwrap_or(8787)
}

/// The directory that identifies this project: the enclosing repository if
/// there is one, else the starting directory itself.
///
/// Walking to the repository root is what lets a lane opened from
/// `crates/zerocode-app` reach the same server as one opened from the top.
#[must_use]
pub fn project_root(start: &Path) -> PathBuf {
    for directory in start.ancestors() {
        if directory.join(".git").exists() {
            return directory.to_path_buf();
        }
    }
    start.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of deriving the port. On one fixed default these two
    /// landed on the same server, so a lane in one repository could reach — and
    /// mix with — the sessions of the other.
    #[test]
    fn two_projects_never_default_to_the_same_session_server() {
        assert_ne!(
            default_bind_for(Path::new("/Users/dev/2026/zerocode")),
            default_bind_for(Path::new("/Users/dev/2026/forge-code"))
        );
    }

    /// And the port has to be the *same* one tomorrow, or a session pool is
    /// stranded behind an address nothing looks at any more.
    #[test]
    fn a_project_keeps_its_port_across_runs() {
        let root = Path::new("/Users/dev/2026/zerocode");
        assert_eq!(default_bind_for(root), default_bind_for(root));
    }

    /// Ports below the dynamic range belong to registered services; landing on
    /// one would be squatting on somebody else's port by default.
    #[test]
    fn the_derived_port_stays_in_the_dynamic_range() {
        for root in [
            "/",
            "/Users/dev/2026/zerocode",
            "/Users/dev/2026/forge-code",
            "/tmp/a",
            "/한글/경로",
        ] {
            let port = project_port(Path::new(root));
            assert!(port >= 49_152, "{root} gave {port}");
        }
    }

    /// A lane opened deep in the tree must reach the same server as one opened
    /// at the top, or opening a lane would mean different things in different
    /// directories of one project.
    #[test]
    fn a_subdirectory_resolves_to_the_repository_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("project");
        let nested = root.join("crates").join("zerocode-app");
        std::fs::create_dir_all(root.join(".git")).expect("git dir");
        std::fs::create_dir_all(&nested).expect("nested");

        assert_eq!(project_root(&nested), root);
        assert_eq!(
            default_bind_for(&project_root(&nested)),
            default_bind_for(&root)
        );
    }

    /// Outside a repository there is nothing to walk to, and a directory is
    /// still a project — it just identifies itself.
    #[test]
    fn a_directory_outside_a_repository_identifies_itself() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).expect("create");
        assert_eq!(project_root(&plain), plain);
    }
}
