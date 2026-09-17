use std::fmt;
use std::sync::Arc;

use russh::client;
use russh::keys::PublicKeyBase64;
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh_sftp::client::SftpSession;
use zerocode_core::{
    PinnedHostKey, PtySpec, RemotePath, SshAuthentication, SshEndpoint, SshHostRecord,
};
use zerocode_lane::PtyHandle;

use crate::channel_reply::await_request_reply;
use crate::deadline;
use crate::host_key::{HostKeyProbeHandler, PinnedHostKeyHandler};
use crate::pty::{OUTPUT_MESSAGE_CAPACITY, PTY_OUTPUT_BYTE_BUDGET, SSH_CHANNEL_PACKET_BYTES};
use crate::{ConnectError, SshPassword};

type DynamicAgent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

/// Creates an authenticated connection only after the server key matches the
/// key already confirmed in the host record.
pub struct SshConnector {
    config: Arc<client::Config>,
}

impl Default for SshConnector {
    fn default() -> Self {
        let config = client::Config {
            keepalive_interval: Some(deadline::KEEPALIVE),
            window_size: PTY_OUTPUT_BYTE_BUDGET as u32,
            maximum_packet_size: SSH_CHANNEL_PACKET_BYTES,
            channel_buffer_size: OUTPUT_MESSAGE_CAPACITY,
            ..client::Config::default()
        };
        Self {
            config: Arc::new(config),
        }
    }
}

impl fmt::Debug for SshConnector {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SshConnector")
    }
}

impl SshConnector {
    /// Observe the server key without sending a username or authentication.
    ///
    /// The returned wire key is still untrusted. A settings surface must show
    /// its fingerprint and persist it only after explicit confirmation; every
    /// later connection then uses [`Self::connect`] and exact pin comparison.
    pub async fn probe_host_key(
        &self,
        endpoint: &SshEndpoint,
    ) -> Result<PinnedHostKey, ConnectError> {
        let (handler, observed) = HostKeyProbeHandler::new();
        let outcome = tokio::time::timeout(
            deadline::CONNECT,
            client::connect(
                self.config.clone(),
                (endpoint.host(), endpoint.port()),
                handler,
            ),
        )
        .await;
        let connection_error = match outcome {
            Err(_) => Some(ConnectError::Timeout),
            Ok(Err(error)) => Some(error),
            Ok(Ok(handle)) => {
                let _ = handle
                    .disconnect(russh::Disconnect::ByApplication, "", "")
                    .await;
                None
            }
        };
        let key = observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match key {
            Some(key) => PinnedHostKey::new(key.algorithm().as_str(), key.public_key_base64())
                .map_err(|_| ConnectError::InvalidPinnedHostKey),
            None => Err(connection_error.unwrap_or(ConnectError::ChannelProtocol)),
        }
    }

    /// Connects and authenticates using the method declared by `host`.
    ///
    /// Password hosts require `Some(password)`; agent hosts require `None`.
    /// Host-key verification completes during key exchange, before either
    /// authentication path is entered. An unconfirmed/unknown host cannot
    /// reach this method because only [`SshHostRecord`] is accepted. Connect
    /// and the complete selected authentication path each have fixed deadlines.
    pub async fn connect(
        &self,
        host: &SshHostRecord,
        password: Option<SshPassword>,
    ) -> Result<SshConnection, ConnectError> {
        let password = match (host.authentication(), password) {
            (SshAuthentication::Password, Some(password)) => Some(password),
            (SshAuthentication::Password, None) => return Err(ConnectError::PasswordRequired),
            (SshAuthentication::Agent, Some(_)) => {
                return Err(ConnectError::PasswordNotAccepted);
            }
            (SshAuthentication::Agent, None) => None,
        };

        let handler = PinnedHostKeyHandler::from_pin(host.pinned_host_key())?;
        let endpoint = host.endpoint();
        let mut handle = tokio::time::timeout(
            deadline::CONNECT,
            client::connect(
                self.config.clone(),
                (endpoint.host(), endpoint.port()),
                handler,
            ),
        )
        .await
        .map_err(|_| ConnectError::Timeout)??;

        let authentication = tokio::time::timeout(deadline::AUTHENTICATE, async {
            match password {
                Some(password) => {
                    let result = handle
                        .authenticate_password(endpoint.user(), password.expose())
                        .await?;
                    if !result.success() {
                        return Err(ConnectError::AuthenticationRejected);
                    }
                    Ok(())
                }
                None => authenticate_with_agent(&mut handle, endpoint.user()).await,
            }
        })
        .await
        .unwrap_or(Err(ConnectError::Timeout));
        if let Err(error) = authentication {
            let _ = shutdown_handle(&mut handle).await;
            return Err(error);
        }

        Ok(SshConnection { handle })
    }
}

/// One host-key-verified and authenticated SSH connection.
pub struct SshConnection {
    pub(crate) handle: client::Handle<PinnedHostKeyHandler>,
}

impl fmt::Debug for SshConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshConnection")
            .field("closed", &self.handle.is_closed())
            .finish()
    }
}

impl SshConnection {
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    pub async fn open_sftp(&self) -> Result<SftpSession, ConnectError> {
        let mut channel =
            tokio::time::timeout(deadline::CHANNEL_OPEN, self.handle.channel_open_session())
                .await
                .map_err(|_| ConnectError::Timeout)??;
        tokio::time::timeout(deadline::CHANNEL_REQUEST, async {
            channel.request_subsystem(true, "sftp").await?;
            await_request_reply(&mut channel, ConnectError::SftpRequestRejected).await
        })
        .await
        .map_err(|_| ConnectError::Timeout)??;
        tokio::time::timeout(
            deadline::CHANNEL_REQUEST,
            SftpSession::new(channel.into_stream()),
        )
        .await
        .map_err(|_| ConnectError::Timeout)?
        .map_err(ConnectError::from)
    }

    /// Resolve and verify a workspace root through the authenticated SFTP
    /// channel. The canonical path is what callers persist and later hand to
    /// the PTY boundary, so a symlink spelling cannot silently become the
    /// workspace identity.
    pub async fn verify_workspace_root(
        &mut self,
        root: &RemotePath,
    ) -> Result<RemotePath, ConnectError> {
        let sftp = self.open_sftp().await?;
        let verified = async {
            let canonical = sftp.canonicalize(root.as_str()).await?;
            let canonical =
                RemotePath::parse(canonical).map_err(|_| ConnectError::InvalidWorkspaceRoot)?;
            let metadata = sftp.symlink_metadata(canonical.as_str()).await?;
            if !metadata.is_dir() {
                return Err(ConnectError::WorkspaceRootNotDirectory);
            }
            Ok(canonical)
        }
        .await;
        let closed = sftp.close().await.map_err(ConnectError::from);
        match verified {
            Err(error) => Err(error),
            Ok(root) => {
                closed?;
                Ok(root)
            }
        }
    }

    /// Consumes this authenticated connection and opens one remote PTY.
    ///
    /// The returned handle owns the SSH session through its asynchronous
    /// worker. There is deliberately no inherited/local working-directory
    /// fallback: `spec.cwd` must contain a validated remote path, and the
    /// account's SSH exec shell must accept the encoded POSIX command.
    ///
    /// PTY and exec setup await explicit server replies before this returns.
    /// Nonempty environment values use acknowledged SSH environment requests
    /// and never enter the exec command; a server policy such as OpenSSH
    /// `AcceptEnv` may reject them, in which case setup fails before exec.
    /// Later input, resize, and termination requests are queued to the worker,
    /// so the lane registry never performs network I/O. SSH window-change and
    /// signal messages have no reply bit: a successful resize means queued,
    /// while kill reports the conventional status 137 after the worker writes
    /// `KILL` and closes this channel; neither is a remote-process ACK.
    pub async fn open_pty(self, spec: &PtySpec) -> Result<PtyHandle, ConnectError> {
        crate::pty::open(self.handle, spec).await
    }

    /// Open a terminal channel while the connection also owns file channels.
    /// Closing this terminal leaves those channels alive.
    pub async fn open_shared_pty(
        self: Arc<Self>,
        spec: &PtySpec,
    ) -> Result<PtyHandle, ConnectError> {
        crate::pty::open_shared(self, spec).await
    }

    /// Sends SSH disconnect and waits for the russh session task to terminate.
    ///
    /// Consuming the connection bounds the lifetime of russh's internal
    /// password-authentication copy. Russh deallocates that copy but does not
    /// explicitly zeroize it. The graceful shutdown has a fixed deadline;
    /// timeout drops the remaining handle and returns a redacted timeout.
    pub async fn disconnect(mut self) -> Result<(), ConnectError> {
        shutdown_handle(&mut self.handle).await
    }
}

pub(crate) async fn shutdown_handle(
    handle: &mut client::Handle<PinnedHostKeyHandler>,
) -> Result<(), ConnectError> {
    shutdown_handle_within(handle, deadline::DISCONNECT).await
}

async fn shutdown_handle_within(
    handle: &mut client::Handle<PinnedHostKeyHandler>,
    timeout: std::time::Duration,
) -> Result<(), ConnectError> {
    tokio::time::timeout(timeout, async {
        handle
            .disconnect(russh::Disconnect::ByApplication, "", "")
            .await?;
        match (&mut *handle).await {
            Ok(())
            | Err(ConnectError::Transport {
                source: russh::Error::Disconnect,
            }) => Ok(()),
            Err(error) => Err(error),
        }
    })
    .await
    .map_err(|_| ConnectError::Timeout)?
}

async fn authenticate_with_agent(
    handle: &mut client::Handle<PinnedHostKeyHandler>,
    user: &str,
) -> Result<(), ConnectError> {
    let mut agent = connect_system_agent().await?;
    let identities = agent
        .request_identities()
        .await
        .map_err(|source| ConnectError::AgentUnavailable { source })?;

    for identity in identities {
        let hash_algorithm = if identity.public_key().algorithm().is_rsa() {
            handle.best_supported_rsa_hash().await?.flatten()
        } else {
            None
        };

        let result = match identity {
            AgentIdentity::PublicKey { key, .. } => {
                handle
                    .authenticate_publickey_with(user, key, hash_algorithm, &mut agent)
                    .await
            }
            AgentIdentity::Certificate { certificate, .. } => {
                handle
                    .authenticate_certificate_with(user, certificate, hash_algorithm, &mut agent)
                    .await
            }
        }
        .map_err(|source| ConnectError::AgentSigning { source })?;

        if result.success() {
            return Ok(());
        }
    }

    Err(ConnectError::AuthenticationRejected)
}

#[cfg(unix)]
async fn connect_system_agent() -> Result<DynamicAgent, ConnectError> {
    AgentClient::connect_env()
        .await
        .map(AgentClient::dynamic)
        .map_err(|source| ConnectError::AgentUnavailable { source })
}

#[cfg(windows)]
async fn connect_system_agent() -> Result<DynamicAgent, ConnectError> {
    if let Some(path) = std::env::var_os("SSH_AUTH_SOCK") {
        return AgentClient::connect_named_pipe(path)
            .await
            .map(AgentClient::dynamic)
            .map_err(|source| ConnectError::AgentUnavailable { source });
    }

    match AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        Ok(agent) => Ok(agent.dynamic()),
        Err(_) => AgentClient::connect_pageant()
            .await
            .map(AgentClient::dynamic)
            .map_err(|source| ConnectError::AgentUnavailable { source }),
    }
}

#[cfg(not(any(unix, windows)))]
async fn connect_system_agent() -> Result<DynamicAgent, ConnectError> {
    Err(ConnectError::AgentUnsupported)
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::PublicKeyBase64;
    use russh::keys::ssh_key::{Algorithm, PrivateKey};
    use russh::server;
    use tokio::net::TcpListener;
    use zerocode_core::PinnedHostKey;

    struct PasswordServer;

    impl server::Handler for PasswordServer {
        type Error = russh::Error;

        async fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> Result<server::Auth, Self::Error> {
            Ok(server::Auth::Accept)
        }
    }

    #[tokio::test]
    async fn a_stalled_session_join_hits_the_disconnect_deadline() {
        let private_key =
            PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("fixture key");
        let public_key = private_key.public_key().clone();
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fixture listener");
        let address = listener.local_addr().expect("fixture address");
        let server_config = Arc::new(server::Config {
            auth_rejection_time: std::time::Duration::from_millis(1),
            auth_rejection_time_initial: Some(std::time::Duration::from_millis(1)),
            keys: vec![private_key],
            ..server::Config::default()
        });
        let server_task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept fixture client");
            let session = server::run_stream(server_config, socket, PasswordServer)
                .await
                .expect("run fixture server");
            let _ = session.await;
        });
        let pin = PinnedHostKey::new(
            public_key.algorithm().as_str(),
            public_key.public_key_base64(),
        )
        .expect("fixture pin");
        let mut handler = PinnedHostKeyHandler::from_pin(&pin).expect("pinned handler");
        handler.stall_disconnect();
        let mut handle = client::connect(Arc::new(client::Config::default()), address, handler)
            .await
            .expect("connect fixture");
        let authenticated = handle
            .authenticate_password("fixture-user", "fixture-password")
            .await
            .expect("authenticate fixture");
        assert!(authenticated.success());

        let result =
            shutdown_handle_within(&mut handle, std::time::Duration::from_millis(25)).await;

        assert!(matches!(result, Err(ConnectError::Timeout)));
        server_task.abort();
    }
}
