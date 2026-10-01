//! Absolute paths for tests, spelled the way the host spells them.
//!
//! A test that names `/work` means "some absolute directory"; on Windows
//! `/work` has no drive and is not absolute, so a path the code under test
//! requires to be absolute is refused. [`absolute`] gives the unix spelling
//! unchanged and the Windows spelling a drive.

use std::path::PathBuf;

/// The drive a Windows test path is rooted on.
#[cfg(windows)]
const TEST_DRIVE: &str = "C:";

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

/// `unix` spelled for this host when it is an absolute unix path, and
/// unchanged when it is a relative one.
pub(crate) fn host_spelled(unix: &str) -> String {
    if unix.starts_with('/') {
        absolute(unix).to_string_lossy().into_owned()
    } else {
        unix.to_owned()
    }
}
