use std::fmt;

use thiserror::Error;

/// Failures at the verified SSH connection boundary.
///
/// Display and Debug deliberately omit key material, agent paths, and
/// authentication inputs. Callers can inspect [`std::error::Error::source`]
/// for operational diagnostics without putting those details in normal logs.
#[derive(Error)]
pub enum ConnectError {
    #[error("the pinned SSH host key is invalid")]
    InvalidPinnedHostKey,
    #[error("the SSH server key does not match the pinned key")]
    HostKeyMismatch,
    #[error("this host requires a caller-supplied password")]
    PasswordRequired,
    #[error("a password was supplied for an SSH-agent host")]
    PasswordNotAccepted,
    #[error("the system SSH agent is unavailable")]
    AgentUnavailable {
        #[source]
        source: russh::keys::Error,
    },
    #[error("the SSH agent could not sign an authentication request")]
    AgentSigning {
        #[source]
        source: russh::AgentAuthError,
    },
    #[error("SSH authentication was rejected")]
    AuthenticationRejected,
    #[error("an SSH terminal requires a validated remote working directory")]
    RemotePtyWorkingDirectoryRequired,
    #[error("the SSH terminal command is not representable by the POSIX shell boundary")]
    InvalidPtyCommand,
    #[error("the SSH server rejected a terminal environment value")]
    RemoteEnvironmentRejected,
    #[error("the SSH server rejected the terminal request")]
    PtyRequestRejected,
    #[error("the SSH server rejected the SFTP subsystem")]
    SftpRequestRejected,
    #[error("the SSH server returned an invalid canonical workspace path")]
    InvalidWorkspaceRoot,
    #[error("the remote workspace root is not a directory")]
    WorkspaceRootNotDirectory,
    #[error("the SSH operation timed out")]
    Timeout,
    #[error("the SSH channel returned an unexpected protocol message")]
    ChannelProtocol,
    #[error("the SSH transport failed")]
    Transport {
        #[from]
        #[source]
        source: russh::Error,
    },
    #[error("the SFTP session failed")]
    Sftp {
        #[from]
        #[source]
        source: russh_sftp::client::error::Error,
    },
    #[cfg(not(any(unix, windows)))]
    #[error("the system SSH agent is unsupported on this platform")]
    AgentUnsupported,
}

impl fmt::Debug for ConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPinnedHostKey => "ConnectError::InvalidPinnedHostKey",
            Self::HostKeyMismatch => "ConnectError::HostKeyMismatch",
            Self::PasswordRequired => "ConnectError::PasswordRequired",
            Self::PasswordNotAccepted => "ConnectError::PasswordNotAccepted",
            Self::AgentUnavailable { .. } => "ConnectError::AgentUnavailable",
            Self::AgentSigning { .. } => "ConnectError::AgentSigning",
            Self::AuthenticationRejected => "ConnectError::AuthenticationRejected",
            Self::RemotePtyWorkingDirectoryRequired => {
                "ConnectError::RemotePtyWorkingDirectoryRequired"
            }
            Self::InvalidPtyCommand => "ConnectError::InvalidPtyCommand",
            Self::RemoteEnvironmentRejected => "ConnectError::RemoteEnvironmentRejected",
            Self::PtyRequestRejected => "ConnectError::PtyRequestRejected",
            Self::SftpRequestRejected => "ConnectError::SftpRequestRejected",
            Self::InvalidWorkspaceRoot => "ConnectError::InvalidWorkspaceRoot",
            Self::WorkspaceRootNotDirectory => "ConnectError::WorkspaceRootNotDirectory",
            Self::Timeout => "ConnectError::Timeout",
            Self::ChannelProtocol => "ConnectError::ChannelProtocol",
            Self::Transport { .. } => "ConnectError::Transport",
            Self::Sftp { .. } => "ConnectError::Sftp",
            #[cfg(not(any(unix, windows)))]
            Self::AgentUnsupported => "ConnectError::AgentUnsupported",
        })
    }
}
