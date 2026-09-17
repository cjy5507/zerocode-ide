//! Discovery files for pane-owned Zo event channels.

use std::fmt;
use std::io::{self, Read as _};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

const CHANNEL_FILE_PREFIX: &str = "zo-events-";
const CHANNEL_FILE_SUFFIX: &str = ".addr";
const CHANNEL_FILE_MAX_BYTES: u64 = 16 * 1024;

/// One private event channel published by an interactive Zo process.
#[derive(Clone, Eq, PartialEq)]
pub struct PaneChannelIdentity {
    /// Numeric loopback address selected by the process.
    pub addr: String,
    /// Bearer required by that listener. `None` exists only for the legacy
    /// one-line publisher used by explicitly supervised panes.
    pub token: Option<String>,
    /// Session the publisher already opened, when it says so (the optional
    /// third line). Lets the window dedupe a lane-road pane it is already
    /// subscribed to without a `session.info` round trip.
    pub session: Option<String>,
}

impl fmt::Debug for PaneChannelIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PaneChannelIdentity")
            .field("addr", &self.addr)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// Path where an interactive Zo process publishes its private event channel.
#[must_use]
pub fn pane_channel_file(runtime_dir: &Path, pid: u32) -> PathBuf {
    runtime_dir.join(format!("{CHANNEL_FILE_PREFIX}{pid}{CHANNEL_FILE_SUFFIX}"))
}

/// Read a complete pane-channel file.
///
/// The first line is the numeric loopback address and the optional second line
/// is its bearer. A missing file or a write without its final newline is not an
/// error: the process has not published a complete identity yet.
///
/// # Errors
///
/// Returns an error when the file cannot be read, exceeds the fixed discovery
/// budget, carries extra lines, or names a non-loopback listener.
pub fn read_pane_channel(path: &Path) -> io::Result<Option<PaneChannelIdentity>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid_channel_file("address path is not a regular file"));
    }
    let mut contents = String::new();
    file.take(CHANNEL_FILE_MAX_BYTES + 1)
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > CHANNEL_FILE_MAX_BYTES {
        return Err(invalid_channel_file("address file exceeds its size limit"));
    }
    if !contents.ends_with('\n') {
        return Ok(None);
    }
    let mut lines = contents.lines();
    let addr = lines.next().unwrap_or_default().trim();
    let token = lines
        .next()
        .map(str::trim)
        .filter(|token| !token.is_empty());
    // zo's publisher writes addr\ntoken\nsession-id\n; the session line is
    // optional so the supervised two-line shape keeps parsing.
    let session = lines
        .next()
        .map(str::trim)
        .filter(|session| !session.is_empty());
    if addr.is_empty() || lines.next().is_some() {
        return Err(invalid_channel_file("address file has the wrong shape"));
    }
    let socket = addr
        .parse::<SocketAddr>()
        .map_err(|_| invalid_channel_file("address file does not name a numeric socket"))?;
    if !socket.ip().is_loopback() {
        return Err(invalid_channel_file("address file does not name loopback"));
    }
    Ok(Some(PaneChannelIdentity {
        addr: socket.to_string(),
        token: token.map(str::to_string),
        session: session.map(str::to_string),
    }))
}

/// Discover the authenticated channel published for one interactive Zo pid.
///
/// # Errors
///
/// Returns the same file errors as [`read_pane_channel`], and rejects the
/// legacy address-only shape because a predictable pid file cannot safely
/// grant an unauthenticated control channel.
pub fn discover_pane_channel(
    runtime_dir: &Path,
    pid: u32,
) -> io::Result<Option<PaneChannelIdentity>> {
    let path = pane_channel_file(runtime_dir, pid);
    let Some(identity) = read_pane_channel(&path)? else {
        return Ok(None);
    };
    if identity.token.is_none() {
        return Err(invalid_channel_file("address file carries no token"));
    }
    Ok(Some(identity))
}

fn invalid_channel_file(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_incomplete_write_is_not_discovered() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pane_channel_file(dir.path(), 7);
        std::fs::write(&path, "127.0.0.1:49152\ntoken").expect("partial file");

        assert_eq!(read_pane_channel(&path).expect("read"), None);
    }

    #[test]
    fn pid_discovery_refuses_the_legacy_address_only_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pane_channel_file(dir.path(), 9);
        std::fs::write(&path, "127.0.0.1:49152\n").expect("legacy address file");

        let error = discover_pane_channel(dir.path(), 9).expect_err("missing token");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn the_three_line_publisher_shape_is_accepted_with_its_session() {
        // zo publishes addr\ntoken\nsession-id\n (zo-ide events.rs) — the
        // third line is the session identity the window dedupes lane-road
        // ownership by. A parser that refuses it orphans every adopted pane.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pane_channel_file(dir.path(), 11);
        std::fs::write(&path, "127.0.0.1:49152\ntoken\nsession-17882-0\n")
            .expect("three-line file");

        let identity = discover_pane_channel(dir.path(), 11)
            .expect("read")
            .expect("published");

        assert_eq!(identity.addr, "127.0.0.1:49152");
        assert_eq!(identity.token.as_deref(), Some("token"));
        assert_eq!(identity.session.as_deref(), Some("session-17882-0"));
    }

    #[test]
    fn a_fourth_line_is_still_the_wrong_shape() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pane_channel_file(dir.path(), 12);
        std::fs::write(&path, "127.0.0.1:49152\ntoken\nsession\nextra\n").expect("four-line file");

        let error = read_pane_channel(&path).expect_err("extra line");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_routable_address_is_refused_before_its_token_can_be_used() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = pane_channel_file(dir.path(), 8);
        std::fs::write(&path, "203.0.113.8:49152\ntoken\n").expect("address file");

        let error = read_pane_channel(&path).expect_err("routable address");

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
