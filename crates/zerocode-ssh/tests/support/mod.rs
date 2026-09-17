use std::sync::Arc;
use std::time::Duration;

use russh::keys::PublicKeyBase64;
use russh::keys::ssh_key::{Algorithm, PrivateKey, PublicKey};
use russh::server;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use zerocode_core::{
    ExecutionHostId, PinnedHostKey, SshAuthentication, SshEndpoint, SshHostDraft, SshHostRecord,
};

pub const USER: &str = "fixture-user";
pub const ACCEPTED_PASSWORD: &str = "accepted-fixture-password";

pub struct Fixture {
    port: u16,
    pub public_key: PublicKey,
    server_task: JoinHandle<()>,
}

impl Fixture {
    pub async fn start<H>(handler: H) -> Self
    where
        H: server::Handler<Error = russh::Error> + Send + 'static,
    {
        let private_key =
            PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("fixture key");
        let public_key = private_key.public_key().clone();
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind fixture listener");
        let port = listener.local_addr().expect("fixture address").port();
        let config = Arc::new(server::Config {
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::from_millis(1)),
            keys: vec![private_key],
            ..Default::default()
        });
        let server_task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.expect("accept fixture client");
            if let Ok(session) = server::run_stream(config, socket, handler).await {
                let _ = session.await;
            }
        });

        Self {
            port,
            public_key,
            server_task,
        }
    }

    pub fn password_host(&self, pinned_key: &PublicKey) -> SshHostRecord {
        password_host(self.port, pinned_key)
    }
}

pub fn password_host(port: u16, pinned_key: &PublicKey) -> SshHostRecord {
    let endpoint = SshEndpoint::new("127.0.0.1", port, USER).expect("valid fixture endpoint");
    let pin = PinnedHostKey::new(
        pinned_key.algorithm().as_str(),
        pinned_key.public_key_base64(),
    )
    .expect("valid fixture pin");
    SshHostDraft::new(
        ExecutionHostId::generate(),
        endpoint,
        SshAuthentication::Password,
    )
    .confirm(pin)
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server_task.abort();
    }
}
