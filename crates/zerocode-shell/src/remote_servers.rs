//! Saved authenticated session servers reached through a loopback tunnel.
//!
//! A routable plaintext session endpoint would disclose its bearer. This
//! domain therefore accepts only an exact `zerocode://pair` access link whose
//! endpoint is a numeric loopback socket. SSH/Tailscale supplies the encrypted tunnel;
//! settings JSON stores only a stable UUID, display name, and numeric socket.
//! The bearer stays in the OS-native credential provider.

#[cfg(test)]
use crate::credential_store::SecretStore;
use crate::credential_store::{SecretStanding, VerifiedSecretError, VerifiedSecretStore};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;
#[cfg(test)]
use std::sync::Arc;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

const REMOTE_SERVER_CREDENTIAL_SERVICE: &str = "dev.zerocode.shell.remote-servers";
const SERVER_ACCOUNT_PREFIX: &str = "server:";
const ACCESS_LINK_SCHEME: &str = "zerocode";
const ACCESS_LINK_HOST: &str = "pair";
const MAX_SERVERS: usize = 32;
const MAX_NAME_CHARS: usize = 80;
const MAX_TOKEN_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteServerEntry {
    id: Uuid,
    name: String,
    endpoint: SocketAddr,
}

impl RemoteServerEntry {
    pub(crate) fn id(&self) -> Uuid {
        self.id
    }

    pub(crate) fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    fn normalized(mut self) -> Self {
        self.name = normalized_stored_name(&self.name, self.endpoint);
        self
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteServerInput {
    pub(crate) id: Option<String>,
    pub(crate) name: String,
    pub(crate) access_link: String,
}

pub(crate) struct PreparedRemoteServer {
    pub(crate) entry: RemoteServerEntry,
    pub(crate) token: Zeroizing<String>,
    pub(crate) update: bool,
}

impl RemoteServerInput {
    pub(crate) fn prepare(self) -> Result<PreparedRemoteServer, RemoteServerError> {
        let update = self.id.as_deref().is_some_and(|id| !id.trim().is_empty());
        let id = match self.id.as_deref().map(str::trim) {
            Some("") | None => Uuid::new_v4(),
            Some(id) => Uuid::parse_str(id).map_err(|_| RemoteServerError::InvalidId)?,
        };
        let (endpoint, token) = parse_access_link(&self.access_link)?;
        let name = normalized_input_name(&self.name, endpoint)?;
        Ok(PreparedRemoteServer {
            entry: RemoteServerEntry { id, name, endpoint },
            token,
            update,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RemoteServerCredentialStatus {
    Available,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RemoteServerView {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) endpoint: String,
    pub(crate) credential_status: RemoteServerCredentialStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RemoteServersReport {
    pub(crate) servers: Vec<RemoteServerView>,
}

#[derive(Clone)]
pub(crate) struct RemoteServerService {
    credentials: VerifiedSecretStore,
}

impl RemoteServerService {
    pub(crate) fn new() -> Self {
        Self {
            credentials: VerifiedSecretStore::new(REMOTE_SERVER_CREDENTIAL_SERVICE),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_secret_store(credentials: Arc<dyn SecretStore>) -> Self {
        Self {
            credentials: VerifiedSecretStore::with_store(credentials),
        }
    }

    pub(crate) fn report(&self, entries: &[RemoteServerEntry]) -> RemoteServersReport {
        RemoteServersReport {
            servers: entries
                .iter()
                .map(|entry| RemoteServerView {
                    id: entry.id.to_string(),
                    name: entry.name.clone(),
                    endpoint: entry.endpoint.to_string(),
                    credential_status: match self.credentials.standing(&server_account(entry.id)) {
                        SecretStanding::Available => RemoteServerCredentialStatus::Available,
                        SecretStanding::Missing => RemoteServerCredentialStatus::Missing,
                        SecretStanding::Unreadable => RemoteServerCredentialStatus::Unreadable,
                    },
                })
                .collect(),
        }
    }

    pub(crate) fn token(&self, id: Uuid) -> Result<Zeroizing<String>, RemoteServerError> {
        self.credentials
            .read(&server_account(id))
            .map_err(map_secret_read_error)
    }

    pub(crate) fn store_token(
        &self,
        id: Uuid,
        token: Zeroizing<String>,
    ) -> Result<(), RemoteServerError> {
        self.credentials
            .write(&server_account(id), token)
            .map_err(map_secret_write_error)
    }

    pub(crate) fn delete_token(&self, id: Uuid) -> Result<(), RemoteServerError> {
        self.credentials
            .delete(&server_account(id))
            .map_err(|_| RemoteServerError::CredentialUnavailable)
    }
}

pub(crate) fn normalize_entries(entries: Vec<RemoteServerEntry>) -> Vec<RemoteServerEntry> {
    let mut ids = HashSet::new();
    let mut endpoints = HashSet::new();
    entries
        .into_iter()
        .map(RemoteServerEntry::normalized)
        .filter(|entry| ids.insert(entry.id) && endpoints.insert(entry.endpoint))
        .take(MAX_SERVERS)
        .collect()
}

pub(crate) fn upsert_entry(
    entries: &mut Vec<RemoteServerEntry>,
    entry: RemoteServerEntry,
    update: bool,
) -> Result<(), RemoteServerError> {
    if entries
        .iter()
        .any(|saved| saved.endpoint == entry.endpoint && saved.id != entry.id)
    {
        return Err(RemoteServerError::EndpointAlreadyExists);
    }
    match entries.iter().position(|saved| saved.id == entry.id) {
        Some(index) if update => entries[index] = entry,
        Some(_) => return Err(RemoteServerError::ServerAlreadyExists),
        None if update => return Err(RemoteServerError::ServerNotFound),
        None if entries.len() >= MAX_SERVERS => return Err(RemoteServerError::ServerLimitReached),
        None => entries.push(entry),
    }
    Ok(())
}

pub(crate) fn remove_entry(
    entries: &mut Vec<RemoteServerEntry>,
    id: Uuid,
) -> Result<RemoteServerEntry, RemoteServerError> {
    let index = entries
        .iter()
        .position(|entry| entry.id == id)
        .ok_or(RemoteServerError::ServerNotFound)?;
    Ok(entries.remove(index))
}

pub(crate) fn parse_id(id: &str) -> Result<Uuid, RemoteServerError> {
    Uuid::parse_str(id.trim()).map_err(|_| RemoteServerError::InvalidId)
}

pub(crate) fn find_entry(
    entries: &[RemoteServerEntry],
    id: Uuid,
) -> Result<&RemoteServerEntry, RemoteServerError> {
    entries
        .iter()
        .find(|entry| entry.id == id)
        .ok_or(RemoteServerError::ServerNotFound)
}

fn parse_access_link(
    access_link: &str,
) -> Result<(SocketAddr, Zeroizing<String>), RemoteServerError> {
    let link = Url::parse(access_link.trim()).map_err(|_| RemoteServerError::InvalidAccessLink)?;
    if link.scheme() != ACCESS_LINK_SCHEME
        || link.host_str() != Some(ACCESS_LINK_HOST)
        || !matches!(link.path(), "" | "/")
        || link.username() != ""
        || link.password().is_some()
        || link.port().is_some()
        || link.fragment().is_some()
    {
        return Err(RemoteServerError::InvalidAccessLink);
    }

    let mut endpoint = None;
    let mut token = None;
    for (key, value) in link.query_pairs() {
        match key.as_ref() {
            "endpoint" if endpoint.is_none() => endpoint = Some(value.into_owned()),
            "token" if token.is_none() => token = Some(value.into_owned()),
            _ => return Err(RemoteServerError::InvalidAccessLink),
        }
    }
    let endpoint = endpoint.ok_or(RemoteServerError::InvalidAccessLink)?;
    let endpoint = zerocode_lane::validate_loopback_bind(&endpoint)
        .map_err(|_| RemoteServerError::EndpointMustBeLoopback)?;
    let token = Zeroizing::new(token.ok_or(RemoteServerError::InvalidAccessLink)?);
    if token.is_empty() || token.len() > MAX_TOKEN_BYTES || token.chars().any(char::is_control) {
        return Err(RemoteServerError::InvalidToken);
    }
    Ok((endpoint, token))
}

fn server_account(id: Uuid) -> String {
    format!("{SERVER_ACCOUNT_PREFIX}{id}")
}

fn normalized_input_name(name: &str, endpoint: SocketAddr) -> Result<String, RemoteServerError> {
    let name = name.trim();
    let name = if name.is_empty() {
        endpoint.to_string()
    } else {
        name.to_string()
    };
    if name.chars().count() > MAX_NAME_CHARS || name.chars().any(char::is_control) {
        return Err(RemoteServerError::InvalidName);
    }
    Ok(name)
}

fn normalized_stored_name(name: &str, endpoint: SocketAddr) -> String {
    normalized_input_name(name, endpoint).unwrap_or_else(|_| endpoint.to_string())
}

fn map_secret_read_error(error: VerifiedSecretError) -> RemoteServerError {
    match error {
        VerifiedSecretError::Missing => RemoteServerError::TokenRequired,
        VerifiedSecretError::InvalidEncoding => RemoteServerError::InvalidCredentialEncoding,
        VerifiedSecretError::Empty => RemoteServerError::InvalidToken,
        VerifiedSecretError::Unavailable | VerifiedSecretError::ReadbackMismatch => {
            RemoteServerError::CredentialUnavailable
        }
    }
}

fn map_secret_write_error(error: VerifiedSecretError) -> RemoteServerError {
    match error {
        VerifiedSecretError::Empty => RemoteServerError::InvalidToken,
        VerifiedSecretError::ReadbackMismatch => RemoteServerError::CredentialReadbackMismatch,
        VerifiedSecretError::Missing
        | VerifiedSecretError::Unavailable
        | VerifiedSecretError::InvalidEncoding => RemoteServerError::CredentialUnavailable,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemoteServerError {
    InvalidId,
    InvalidName,
    InvalidAccessLink,
    EndpointMustBeLoopback,
    InvalidToken,
    TokenRequired,
    InvalidCredentialEncoding,
    CredentialUnavailable,
    CredentialReadbackMismatch,
    ServerAlreadyExists,
    EndpointAlreadyExists,
    ServerNotFound,
    ServerLimitReached,
}

impl fmt::Display for RemoteServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "invalid remote server id",
            Self::InvalidName => "invalid remote server name",
            Self::InvalidAccessLink => "invalid ZeroCode server access link",
            Self::EndpointMustBeLoopback => {
                "remote server access links must use a loopback tunnel endpoint"
            }
            Self::InvalidToken => "invalid remote server access token",
            Self::TokenRequired => "remote server access token required",
            Self::InvalidCredentialEncoding => "remote server credential is not valid UTF-8",
            Self::CredentialUnavailable => "remote server credential is unavailable",
            Self::CredentialReadbackMismatch => "remote server credential verification failed",
            Self::ServerAlreadyExists => "remote server already exists",
            Self::EndpointAlreadyExists => "remote server endpoint already exists",
            Self::ServerNotFound => "remote server not found",
            Self::ServerLimitReached => "remote server limit reached",
        })
    }
}

impl std::error::Error for RemoteServerError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::MemorySecretStore;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    fn input(link: &str) -> RemoteServerInput {
        RemoteServerInput {
            id: None,
            name: "Build runtime".to_string(),
            access_link: link.to_string(),
        }
    }

    fn session_server(required_token: &str) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind session fixture");
        let address = listener.local_addr().expect("fixture address");
        let required_token = required_token.to_string();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer_session_probe(stream, &required_token);
            }
        });
        address
    }

    fn answer_session_probe(stream: TcpStream, required_token: &str) {
        let Ok(read_half) = stream.try_clone() else {
            return;
        };
        let mut lines = BufReader::new(read_half).lines();
        let Some(Ok(line)) = lines.next() else {
            return;
        };
        let Ok(request) = serde_json::from_str::<serde_json::Value>(&line) else {
            return;
        };
        let id = request["id"].as_u64().unwrap_or_default();
        let presented = request.get("token").and_then(serde_json::Value::as_str);
        let reply = if presented == Some(required_token) {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": { "sessions": [] },
            })
        } else {
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32002, "message": "unauthorized" },
            })
        };
        let mut stream = stream;
        let _ = writeln!(stream, "{reply}");
        let _ = stream.flush();
    }

    #[test]
    fn an_access_link_keeps_only_numeric_loopback_metadata_in_json() {
        let prepared = input("zerocode://pair?endpoint=127.0.0.1%3A43123&token=fixture-secret")
            .prepare()
            .expect("valid tunneled access link");
        let json = serde_json::to_string(&prepared.entry).expect("serialize metadata");

        assert_eq!(prepared.entry.endpoint.ip().to_string(), "127.0.0.1");
        assert_eq!(prepared.token.as_str(), "fixture-secret");
        assert!(!json.contains("fixture-secret"));
        assert!(!json.contains("access_link"));
    }

    #[test]
    fn a_routable_or_ambiguous_access_link_never_reaches_settings() {
        for link in [
            "zerocode://pair?endpoint=192.0.2.10%3A43123&token=secret",
            "zerocode://pair?endpoint=127.0.0.1%3A43123&token=one&token=two",
            "zerocode://pair?endpoint=127.0.0.1%3A43123&token=secret&extra=no",
            "https://pair?endpoint=127.0.0.1%3A43123&token=secret",
        ] {
            assert!(input(link).prepare().is_err(), "accepted {link}");
        }
    }

    #[test]
    fn one_endpoint_has_one_stable_server_identity() {
        let first = input("zerocode://pair?endpoint=127.0.0.1%3A43123&token=first")
            .prepare()
            .expect("first server")
            .entry;
        let second = input("zerocode://pair?endpoint=127.0.0.1%3A43123&token=second")
            .prepare()
            .expect("second server")
            .entry;
        let mut entries = vec![first];

        assert_eq!(
            upsert_entry(&mut entries, second, false),
            Err(RemoteServerError::EndpointAlreadyExists)
        );
    }

    #[test]
    fn native_secret_readback_is_the_only_available_credential_state() {
        let credentials = Arc::new(MemorySecretStore::default());
        let service = RemoteServerService::with_secret_store(credentials.clone());
        let prepared = input("zerocode://pair?endpoint=127.0.0.1%3A43123&token=fixture-secret")
            .prepare()
            .expect("valid server");
        let id = prepared.entry.id;

        service
            .store_token(id, prepared.token)
            .expect("verified native write");
        assert_eq!(
            service.token(id).expect("read token").as_str(),
            "fixture-secret"
        );
        assert_eq!(
            service.report(&[prepared.entry]).servers[0].credential_status,
            RemoteServerCredentialStatus::Available
        );
        assert_eq!(
            credentials.secret(&server_account(id)),
            Some(b"fixture-secret".to_vec())
        );
    }

    #[test]
    fn metadata_and_native_token_move_as_one_restart_safe_transaction() {
        let directory = tempfile::tempdir().expect("settings root");
        let repository = crate::settings::SettingsRepository::new(directory.path());
        let credentials = Arc::new(MemorySecretStore::default());
        let service = RemoteServerService::with_secret_store(credentials.clone());
        let address = session_server("fixture-secret");
        let link = format!(
            "zerocode://pair?endpoint=127.0.0.1%3A{}&token=fixture-secret",
            address.port()
        );

        let (saved, report) =
            crate::save_remote_server_transaction(&repository, &service, input(&link))
                .expect("pair verified server");
        let id = parse_id(&report.servers[0].id).expect("saved server id");
        let json = serde_json::to_string(&saved.document).expect("serialize settings document");
        assert!(!json.contains("fixture-secret"));
        assert_eq!(
            credentials.secret(&server_account(id)),
            Some(b"fixture-secret".to_vec())
        );

        let (removed, report) =
            crate::remove_remote_server_transaction(&repository, &service, &id.to_string())
                .expect("remove paired server");
        assert!(report.servers.is_empty());
        assert!(removed.document.remote_servers.is_empty());
        assert_eq!(credentials.secret(&server_account(id)), None);
        let restarted = crate::load_settings(&repository).expect("restart-safe settings");
        assert!(restarted.document.remote_servers.is_empty());
    }
}
