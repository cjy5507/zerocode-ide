//! One request to create the live side of a terminal.
//!
//! The composition root chooses this factory after resolving and authenticating
//! the execution backend. Neither [`PtySpec`] nor the returned [`PtyHandle`]
//! carries credentials. A factory is consumed because an authenticated SSH
//! connection is likewise consumed to open one PTY; local spawning follows the
//! same ownership shape without pretending that remote connections are reusable.

use zerocode_core::host::{PtyCwd, PtySpec};
use zerocode_pty::{PtyError, PtyLane};

use crate::{PtyHandle, PtyTransportError};

/// A selected backend that can open exactly one terminal.
///
/// Selection and authentication stay outside this trait. After spawn, all
/// lifecycle operations belong to [`crate::PtyTransport`].
pub trait PtySpawner: Send {
    /// Consume this selected backend and open [`PtySpec`].
    fn spawn(self: Box<Self>, spec: &PtySpec) -> Result<PtyHandle, PtyTransportError>;
}

/// 이 프로세스가 도는 기계의 터미널.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalPty;

impl PtySpawner for LocalPty {
    fn spawn(self: Box<Self>, spec: &PtySpec) -> Result<PtyHandle, PtyTransportError> {
        let cwd = match spec.cwd.as_ref() {
            None => None,
            Some(PtyCwd::Local(path)) => Some(path.as_path()),
            Some(PtyCwd::Remote(_)) => {
                return Err(PtyError::Spawn {
                    program: spec.program.clone(),
                    reason: "a remote working directory cannot be opened by the local PTY"
                        .to_string(),
                }
                .into());
            }
        };
        PtyLane::spawn(
            &spec.program,
            &spec.args,
            cwd,
            &spec.env,
            spec.rows,
            spec.cols,
        )
        .map(PtyHandle::from)
        .map_err(PtyTransportError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PtyTransport;
    use zerocode_core::host::RemotePath;

    /// 경계가 띄운 자식은 예전 자유 호출이 띄운 자식과 같은 자식이다.
    #[cfg(unix)]
    #[test]
    fn the_boundary_spawns_the_child_the_direct_call_would_have() {
        let cwd = tempfile::tempdir().expect("working directory");
        let canonical = cwd
            .path()
            .canonicalize()
            .expect("canonical working directory");
        let expected = format!("through the boundary:{}", canonical.display());
        let spec = PtySpec::new(
            "/bin/sh",
            &[
                "-c".to_string(),
                "printf 'through the boundary:%s' \"$PWD\"".to_string(),
            ],
            Some(cwd.path()),
            &[],
            10,
            240,
        );
        let mut lane = Box::new(LocalPty).spawn(&spec).expect("spawn");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            lane.pump();
            if lane.terminal().grid().visible_text().contains(&expected) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!(
            "the child the boundary started printed nothing: {:?}",
            lane.terminal().grid().visible_text()
        );
    }

    /// 없는 프로그램은 조용히 성공하지 않는다.
    #[test]
    fn a_program_that_is_not_there_fails_by_name() {
        let spec = PtySpec::new("zerocode-no-such-program", &[], None, &[], 10, 40);
        let failed = Box::new(LocalPty).spawn(&spec);
        assert!(failed.is_err(), "a missing program started a terminal");
    }

    /// A remote path must never degrade into "inherit this process's cwd" at
    /// the local boundary. That would run the right command in the wrong tree.
    #[cfg(unix)]
    #[test]
    fn a_local_pty_refuses_a_remote_working_directory() {
        let spec = PtySpec::remote(
            "/bin/sh",
            &[],
            RemotePath::parse("/srv/work").expect("validated remote cwd"),
            &[],
            10,
            40,
        );

        assert!(
            matches!(
                Box::new(LocalPty).spawn(&spec),
                Err(PtyTransportError::Local(PtyError::Spawn { program, .. }))
                    if program == "/bin/sh"
            ),
            "the local PTY silently inherited a cwd for a remote request"
        );
    }
}
