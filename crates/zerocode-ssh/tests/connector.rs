mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use russh::keys::PublicKeyBase64;
use russh::keys::ssh_key::{Algorithm, HashAlg, PrivateKey};
use russh::server;
use tokio::net::TcpListener;
use tokio::time::timeout;
use zerocode_core::{ExecutionHostId, PinnedHostKey, SshAuthentication, SshEndpoint, SshHostDraft};
use zerocode_ssh::{ConnectError, SshConnector, SshPassword, host_key_fingerprint};

use support::{ACCEPTED_PASSWORD, Fixture, USER, password_host};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

struct PasswordServer {
    authentication_attempts: Arc<AtomicUsize>,
    accept_password: bool,
}

struct StalledPasswordServer;

impl server::Handler for StalledPasswordServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        _user: &str,
        _password: &str,
    ) -> Result<server::Auth, Self::Error> {
        std::future::pending().await
    }
}

impl server::Handler for PasswordServer {
    type Error = russh::Error;

    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        self.authentication_attempts.fetch_add(1, Ordering::SeqCst);
        if self.accept_password && user == USER && password == ACCEPTED_PASSWORD {
            Ok(server::Auth::Accept)
        } else {
            Ok(server::Auth::reject())
        }
    }
}

async fn fixture(accept_password: bool) -> (Fixture, Arc<AtomicUsize>) {
    let authentication_attempts = Arc::new(AtomicUsize::new(0));
    let server = PasswordServer {
        authentication_attempts: Arc::clone(&authentication_attempts),
        accept_password,
    };
    (Fixture::start(server).await, authentication_attempts)
}

#[tokio::test]
async fn a_persisted_pin_has_the_same_fingerprint_as_the_server_key() {
    let (fixture, _) = fixture(true).await;
    let host = fixture.password_host(&fixture.public_key);

    assert_eq!(
        host_key_fingerprint(host.pinned_host_key()).expect("fixture pin parses"),
        fixture.public_key.fingerprint(HashAlg::Sha256).to_string()
    );
}

#[tokio::test]
async fn probing_a_key_sends_no_authentication() {
    let (fixture, authentication_attempts) = fixture(true).await;
    let host = fixture.password_host(&fixture.public_key);

    let pin = SshConnector::default()
        .probe_host_key(host.endpoint())
        .await
        .expect("observe fixture key");
    assert_eq!(pin.algorithm(), fixture.public_key.algorithm().as_str());
    assert_eq!(pin.encoded_key(), fixture.public_key.public_key_base64());
    assert_eq!(authentication_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn exact_pinned_key_connects_and_authenticates() {
    let (fixture, authentication_attempts) = fixture(true).await;
    let host = fixture.password_host(&fixture.public_key);

    let connection = timeout(
        CONNECT_TIMEOUT,
        SshConnector::default()
            .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned()))),
    )
    .await
    .expect("connection timed out")
    .expect("exact pin and password must connect");

    assert!(!connection.is_closed());
    assert_eq!(authentication_attempts.load(Ordering::SeqCst), 1);
    connection.disconnect().await.expect("disconnect fixture");
}

#[tokio::test]
async fn mismatched_pinned_key_fails_before_authentication() {
    let (fixture, authentication_attempts) = fixture(true).await;
    let other_key =
        PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("second fixture key");
    let host = fixture.password_host(other_key.public_key());

    let error = timeout(
        CONNECT_TIMEOUT,
        SshConnector::default()
            .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned()))),
    )
    .await
    .expect("connection timed out")
    .expect_err("a different server key must be rejected");

    assert!(matches!(error, ConnectError::HostKeyMismatch));
    assert_eq!(authentication_attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn authentication_refusal_is_reported_after_key_acceptance() {
    let (fixture, authentication_attempts) = fixture(false).await;
    let host = fixture.password_host(&fixture.public_key);

    let error = timeout(
        CONNECT_TIMEOUT,
        SshConnector::default()
            .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned()))),
    )
    .await
    .expect("connection timed out")
    .expect_err("fixture refuses every password");

    assert!(matches!(error, ConnectError::AuthenticationRejected));
    assert_eq!(authentication_attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_tcp_peer_that_never_speaks_ssh_hits_the_connect_deadline() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind stalled fixture");
    let port = listener.local_addr().expect("stalled address").port();
    let stalled_peer = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.expect("accept stalled client");
        std::future::pending::<()>().await;
    });
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("fixture key");
    let host = password_host(port, key.public_key());

    let result = SshConnector::default()
        .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned())))
        .await;

    assert!(matches!(result, Err(ConnectError::Timeout)));
    stalled_peer.abort();
}

#[tokio::test(start_paused = true)]
async fn an_authenticator_that_never_answers_hits_the_auth_deadline() {
    let fixture = Fixture::start(StalledPasswordServer).await;
    let host = fixture.password_host(&fixture.public_key);

    let result = SshConnector::default()
        .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned())))
        .await;

    assert!(matches!(result, Err(ConnectError::Timeout)));
}

#[tokio::test]
async fn malformed_pin_is_rejected_before_opening_a_socket() {
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).expect("fixture key");
    let endpoint = SshEndpoint::new("127.0.0.1", 9, USER).expect("valid unused endpoint");
    let inconsistent_pin = PinnedHostKey::new("ssh-rsa", key.public_key().public_key_base64())
        .expect("core validates the pin's storage shape");
    let host = SshHostDraft::new(
        ExecutionHostId::generate(),
        endpoint,
        SshAuthentication::Password,
    )
    .confirm(inconsistent_pin);

    let error = SshConnector::default()
        .connect(&host, Some(SshPassword::new(ACCEPTED_PASSWORD.to_owned())))
        .await
        .expect_err("algorithm and encoded key must describe the same key");

    assert!(matches!(error, ConnectError::InvalidPinnedHostKey));
}

#[test]
fn password_and_errors_redact_debug_output() {
    let secret = "never-print-this-secret";
    let password_debug = format!("{:?}", SshPassword::new(secret.to_owned()));
    let error_debug = format!("{:?}", ConnectError::AuthenticationRejected);

    assert!(!password_debug.contains(secret));
    assert!(!error_debug.contains(secret));
    assert!(password_debug.contains("REDACTED"));
}
