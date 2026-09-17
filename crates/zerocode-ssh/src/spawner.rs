use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};

use tokio::runtime::{Handle, RuntimeFlavor};
use zerocode_core::{HostError, PtySpec};
use zerocode_lane::{PtyHandle, PtySpawner, PtyTransportError};

use crate::{ConnectError, SshConnection};

/// A one-shot bridge from an authenticated SSH connection to a lane PTY.
///
/// [`PtySpawner::spawn`] is synchronous, while SSH setup is asynchronous. This
/// bridge uses an explicit multi-thread Tokio runtime: outside Tokio it blocks
/// on that handle, and inside a multi-thread runtime it first yields the caller
/// with `block_in_place`. A current-thread runtime cannot make progress while
/// its only driver is blocked, so that configuration fails with a stable host
/// transport error instead of hanging or nesting `block_on`.
pub struct SshPtySpawner {
    connection: SshConnection,
    runtime: Handle,
}

impl SshPtySpawner {
    /// Pair one authenticated connection with the live multi-thread runtime
    /// that will own its SSH session task.
    ///
    /// A current-thread handle is retained but refused by [`PtySpawner::spawn`]
    /// with a stable transport error because it cannot safely bridge this
    /// synchronous boundary.
    #[must_use]
    pub fn new(connection: SshConnection, runtime: Handle) -> Self {
        Self {
            connection,
            runtime,
        }
    }
}

impl fmt::Debug for SshPtySpawner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SshPtySpawner")
    }
}

impl PtySpawner for SshPtySpawner {
    fn spawn(self: Box<Self>, spec: &PtySpec) -> Result<PtyHandle, PtyTransportError> {
        let Self {
            connection,
            runtime,
        } = *self;
        if !matches!(runtime.runtime_flavor(), RuntimeFlavor::MultiThread) {
            return Err(transport_error());
        }

        let spec = spec.clone();
        let open = move || runtime.block_on(connection.open_pty(&spec));
        let opened = match Handle::try_current() {
            Ok(current) if matches!(current.runtime_flavor(), RuntimeFlavor::MultiThread) => {
                catch_unwind(AssertUnwindSafe(|| tokio::task::block_in_place(open)))
            }
            Ok(_) => return Err(transport_error()),
            Err(_) => catch_unwind(AssertUnwindSafe(open)),
        }
        .map_err(|_| transport_error())?;

        opened.map_err(|error| host_error(error).into())
    }
}

fn host_error(error: ConnectError) -> HostError {
    match error {
        ConnectError::PasswordRequired
        | ConnectError::PasswordNotAccepted
        | ConnectError::AgentUnavailable { .. }
        | ConnectError::AgentSigning { .. }
        | ConnectError::AuthenticationRejected => HostError::Authentication,
        ConnectError::Timeout => HostError::Timeout,
        ConnectError::RemoteEnvironmentRejected
        | ConnectError::PtyRequestRejected
        | ConnectError::SftpRequestRejected
        | ConnectError::WorkspaceRootNotDirectory => HostError::NoAccess,
        ConnectError::InvalidPinnedHostKey
        | ConnectError::HostKeyMismatch
        | ConnectError::RemotePtyWorkingDirectoryRequired
        | ConnectError::InvalidPtyCommand
        | ConnectError::InvalidWorkspaceRoot
        | ConnectError::ChannelProtocol
        | ConnectError::Transport { .. }
        | ConnectError::Sftp { .. } => HostError::Transport,
        #[cfg(not(any(unix, windows)))]
        ConnectError::AgentUnsupported => HostError::Authentication,
    }
}

fn transport_error() -> PtyTransportError {
    HostError::Transport.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_errors_collapse_to_stable_host_categories() {
        assert_eq!(
            host_error(ConnectError::AuthenticationRejected),
            HostError::Authentication
        );
        assert_eq!(
            host_error(ConnectError::RemoteEnvironmentRejected),
            HostError::NoAccess
        );
        assert_eq!(host_error(ConnectError::Timeout), HostError::Timeout);
        assert_eq!(
            host_error(ConnectError::InvalidPtyCommand),
            HostError::Transport
        );
    }
}
