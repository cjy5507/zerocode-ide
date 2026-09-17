//! Persisted SSH host metadata and native-vault credentials.
//!
//! This module owns the settings-domain boundary only. Network authentication
//! stays in `zerocode-ssh`; JSON stores the endpoint and exact confirmed host
//! key, while a password is addressed by the stable host UUID in the OS vault.

#[cfg(test)]
use crate::credential_store::SecretStore;
use crate::credential_store::{SecretStanding, VerifiedSecretError, VerifiedSecretStore};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;
#[cfg(test)]
use std::sync::Arc;
use zerocode_core::host::{
    ExecutionHostId, PinnedHostKey, SSH_DEFAULT_PORT, SshAuthentication, SshEndpoint, SshHostDraft,
    SshHostRecord,
};
use zerocode_ssh::{SshPassword, host_key_fingerprint};
use zeroize::Zeroizing;

const SSH_CREDENTIAL_SERVICE: &str = "dev.zerocode.shell.ssh";
const HOST_ACCOUNT_PREFIX: &str = "host:";
const MAX_HOSTS: usize = 64;
const MAX_LABEL_CHARS: usize = 80;
#[cfg(test)]
pub(crate) const TEST_HOST_KEY: &str =
    "AAAAC3NzaC1lZDI1NTE5AAAAIDCPkgWjLtezUuBUNbq5efqpD/hwrUfJIuZi3jFtgNEc";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SshHostEntry {
    label: String,
    record: SshHostRecord,
}

impl SshHostEntry {
    pub(crate) fn id(&self) -> ExecutionHostId {
        self.record.id()
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn record(&self) -> &SshHostRecord {
        &self.record
    }

    fn normalized(mut self) -> Self {
        self.label = normalized_stored_label(&self.label, &self.record);
        self
    }
}

/// Keep hand-edited settings deterministic without inventing a second store.
pub(crate) fn normalize_entries(entries: Vec<SshHostEntry>) -> Vec<SshHostEntry> {
    let mut seen = HashSet::new();
    entries
        .into_iter()
        .map(SshHostEntry::normalized)
        .filter(|entry| seen.insert(entry.id()))
        .take(MAX_HOSTS)
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SshHostInput {
    pub(crate) id: Option<String>,
    pub(crate) label: String,
    pub(crate) host: String,
    #[serde(default = "default_port")]
    pub(crate) port: u16,
    pub(crate) user: String,
    pub(crate) authentication: SshAuthentication,
    pub(crate) key_algorithm: String,
    pub(crate) encoded_key: String,
    pub(crate) password: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SshHostProbeInput {
    pub(crate) host: String,
    #[serde(default = "default_port")]
    pub(crate) port: u16,
    pub(crate) user: String,
}

impl SshHostProbeInput {
    pub(crate) fn endpoint(self) -> Result<SshEndpoint, SshHostsError> {
        SshEndpoint::new(self.host.trim(), self.port, self.user.trim())
            .map_err(|_| SshHostsError::InvalidEndpoint)
    }
}

fn default_port() -> u16 {
    SSH_DEFAULT_PORT
}

pub(crate) struct PreparedSshHost {
    pub(crate) entry: SshHostEntry,
    pub(crate) password: Option<Zeroizing<String>>,
    pub(crate) update: bool,
}

impl SshHostInput {
    pub(crate) fn prepare(self) -> Result<PreparedSshHost, SshHostsError> {
        let update = self.id.as_deref().is_some_and(|id| !id.trim().is_empty());
        let id = match self.id.as_deref().map(str::trim) {
            Some("") | None => ExecutionHostId::generate(),
            Some(id) => ExecutionHostId::from_str(id).map_err(|_| SshHostsError::InvalidId)?,
        };
        let host = self.host.trim();
        let user = self.user.trim();
        let endpoint =
            SshEndpoint::new(host, self.port, user).map_err(|_| SshHostsError::InvalidEndpoint)?;
        let pin = PinnedHostKey::new(self.key_algorithm.trim(), self.encoded_key.trim())
            .map_err(|_| SshHostsError::InvalidHostKey)?;
        host_key_fingerprint(&pin).map_err(|_| SshHostsError::InvalidHostKey)?;

        let password = self.password.map(Zeroizing::new);
        match (self.authentication, password.as_ref()) {
            (SshAuthentication::Agent, Some(_)) => {
                return Err(SshHostsError::PasswordNotAccepted);
            }
            (SshAuthentication::Password, Some(password)) if password.is_empty() => {
                return Err(SshHostsError::EmptyPassword);
            }
            _ => {}
        }

        let label = normalized_input_label(&self.label, &endpoint)?;
        let record = SshHostDraft::new(id, endpoint, self.authentication).confirm(pin);
        Ok(PreparedSshHost {
            entry: SshHostEntry { label, record },
            password,
            update,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SshCredentialStatus {
    NotRequired,
    Available,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SshHostView {
    pub(crate) id: String,
    pub(crate) label: String,
    pub(crate) host: String,
    pub(crate) port: u16,
    pub(crate) user: String,
    pub(crate) authentication: SshAuthentication,
    pub(crate) key_algorithm: String,
    pub(crate) encoded_key: String,
    pub(crate) fingerprint: String,
    pub(crate) credential_status: SshCredentialStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SshHostsReport {
    pub(crate) hosts: Vec<SshHostView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SshHostKeyView {
    pub(crate) key_algorithm: String,
    pub(crate) encoded_key: String,
    pub(crate) fingerprint: String,
}

impl SshHostKeyView {
    pub(crate) fn from_pin(pin: &PinnedHostKey) -> Result<Self, SshHostsError> {
        Ok(Self {
            key_algorithm: pin.algorithm().to_string(),
            encoded_key: pin.encoded_key().to_string(),
            fingerprint: host_key_fingerprint(pin).map_err(|_| SshHostsError::InvalidHostKey)?,
        })
    }
}

#[derive(Clone)]
pub(crate) struct SshHostService {
    credentials: VerifiedSecretStore,
}

impl SshHostService {
    pub(crate) fn new() -> Self {
        Self {
            credentials: VerifiedSecretStore::new(SSH_CREDENTIAL_SERVICE),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_secret_store(credentials: Arc<dyn SecretStore>) -> Self {
        Self {
            credentials: VerifiedSecretStore::with_store(credentials),
        }
    }

    pub(crate) fn view(&self, entry: &SshHostEntry) -> Result<SshHostView, SshHostsError> {
        let record = entry.record();
        let endpoint = record.endpoint();
        let pin = record.pinned_host_key();
        Ok(SshHostView {
            id: record.id().to_string(),
            label: entry.label().to_string(),
            host: endpoint.host().to_string(),
            port: endpoint.port(),
            user: endpoint.user().to_string(),
            authentication: record.authentication(),
            key_algorithm: pin.algorithm().to_string(),
            encoded_key: pin.encoded_key().to_string(),
            fingerprint: host_key_fingerprint(pin).map_err(|_| SshHostsError::InvalidHostKey)?,
            credential_status: self.credential_status(record),
        })
    }

    pub(crate) fn report(&self, entries: &[SshHostEntry]) -> Result<SshHostsReport, SshHostsError> {
        entries
            .iter()
            .map(|entry| self.view(entry))
            .collect::<Result<Vec<_>, _>>()
            .map(|hosts| SshHostsReport { hosts })
    }

    pub(crate) fn credential_status(&self, record: &SshHostRecord) -> SshCredentialStatus {
        if record.authentication() == SshAuthentication::Agent {
            return SshCredentialStatus::NotRequired;
        }
        match self.credentials.standing(&host_account(record.id())) {
            SecretStanding::Available => SshCredentialStatus::Available,
            SecretStanding::Missing => SshCredentialStatus::Missing,
            SecretStanding::Unreadable => SshCredentialStatus::Unreadable,
        }
    }

    pub(crate) fn password(
        &self,
        record: &SshHostRecord,
    ) -> Result<Option<SshPassword>, SshHostsError> {
        if record.authentication() == SshAuthentication::Agent {
            return Ok(None);
        }
        let password = self
            .credentials
            .read(&host_account(record.id()))
            .map_err(map_secret_read_error)?;
        Ok(Some(SshPassword::new(password.to_string())))
    }

    pub(crate) fn store_password(
        &self,
        id: ExecutionHostId,
        password: Zeroizing<String>,
    ) -> Result<(), SshHostsError> {
        self.credentials
            .write(&host_account(id), password)
            .map_err(map_secret_write_error)
    }

    pub(crate) fn delete_password(&self, id: ExecutionHostId) -> Result<(), SshHostsError> {
        self.credentials
            .delete(&host_account(id))
            .map_err(|_| SshHostsError::CredentialUnavailable)
    }
}

pub(crate) fn upsert_entry(
    entries: &mut Vec<SshHostEntry>,
    entry: SshHostEntry,
    update: bool,
) -> Result<(), SshHostsError> {
    match entries.iter().position(|saved| saved.id() == entry.id()) {
        Some(index) if update => entries[index] = entry,
        Some(_) => return Err(SshHostsError::HostAlreadyExists),
        None if update => return Err(SshHostsError::HostNotFound),
        None if entries.len() >= MAX_HOSTS => return Err(SshHostsError::HostLimitReached),
        None => entries.push(entry),
    }
    Ok(())
}

pub(crate) fn remove_entry(
    entries: &mut Vec<SshHostEntry>,
    id: ExecutionHostId,
) -> Result<SshHostEntry, SshHostsError> {
    let index = entries
        .iter()
        .position(|entry| entry.id() == id)
        .ok_or(SshHostsError::HostNotFound)?;
    Ok(entries.remove(index))
}

pub(crate) fn parse_id(id: &str) -> Result<ExecutionHostId, SshHostsError> {
    ExecutionHostId::from_str(id.trim()).map_err(|_| SshHostsError::InvalidId)
}

pub(crate) fn find_entry(
    entries: &[SshHostEntry],
    id: ExecutionHostId,
) -> Result<&SshHostEntry, SshHostsError> {
    entries
        .iter()
        .find(|entry| entry.id() == id)
        .ok_or(SshHostsError::HostNotFound)
}

fn map_secret_read_error(error: VerifiedSecretError) -> SshHostsError {
    match error {
        VerifiedSecretError::Missing => SshHostsError::PasswordRequired,
        VerifiedSecretError::InvalidEncoding => SshHostsError::InvalidCredentialEncoding,
        VerifiedSecretError::Empty => SshHostsError::EmptyPassword,
        VerifiedSecretError::Unavailable | VerifiedSecretError::ReadbackMismatch => {
            SshHostsError::CredentialUnavailable
        }
    }
}

fn map_secret_write_error(error: VerifiedSecretError) -> SshHostsError {
    match error {
        VerifiedSecretError::Empty => SshHostsError::EmptyPassword,
        VerifiedSecretError::ReadbackMismatch => SshHostsError::CredentialReadbackMismatch,
        VerifiedSecretError::Missing
        | VerifiedSecretError::Unavailable
        | VerifiedSecretError::InvalidEncoding => SshHostsError::CredentialUnavailable,
    }
}

fn host_account(id: ExecutionHostId) -> String {
    format!("{HOST_ACCOUNT_PREFIX}{id}")
}

fn normalized_input_label(label: &str, endpoint: &SshEndpoint) -> Result<String, SshHostsError> {
    let label = label.trim();
    let label = if label.is_empty() {
        format!("{}@{}", endpoint.user(), endpoint.host())
    } else {
        label.to_string()
    };
    if label.chars().count() > MAX_LABEL_CHARS || label.chars().any(char::is_control) {
        return Err(SshHostsError::InvalidLabel);
    }
    Ok(label)
}

fn normalized_stored_label(label: &str, record: &SshHostRecord) -> String {
    let trimmed = label.trim();
    let fallback = format!("{}@{}", record.endpoint().user(), record.endpoint().host());
    let source = if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        fallback.as_str()
    } else {
        trimmed
    };
    source.chars().take(MAX_LABEL_CHARS).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SshHostsError {
    InvalidId,
    InvalidLabel,
    InvalidEndpoint,
    InvalidHostKey,
    EmptyPassword,
    PasswordNotAccepted,
    PasswordRequired,
    InvalidCredentialEncoding,
    CredentialUnavailable,
    CredentialReadbackMismatch,
    HostAlreadyExists,
    HostNotFound,
    HostLimitReached,
}

impl fmt::Display for SshHostsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidId => "SSH host identity is invalid",
            Self::InvalidLabel => "SSH host label is invalid",
            Self::InvalidEndpoint => "SSH endpoint is invalid",
            Self::InvalidHostKey => "SSH host key is invalid",
            Self::EmptyPassword => "SSH password is empty",
            Self::PasswordNotAccepted => "SSH agent authentication does not accept a password",
            Self::PasswordRequired => "SSH password is missing",
            Self::InvalidCredentialEncoding => "SSH password is not valid UTF-8",
            Self::CredentialUnavailable => "native SSH credential storage is unavailable",
            Self::CredentialReadbackMismatch => "native SSH credential verification failed",
            Self::HostAlreadyExists => "SSH host identity already exists",
            Self::HostNotFound => "SSH host was not found",
            Self::HostLimitReached => "SSH host limit reached",
        })
    }
}

impl std::error::Error for SshHostsError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::{MemorySecretStore, SecretOperation, SecretStoreErrorKind};

    fn input(authentication: SshAuthentication, password: Option<&str>) -> SshHostInput {
        SshHostInput {
            id: None,
            label: "Build machine".to_string(),
            host: "build.example.com".to_string(),
            port: SSH_DEFAULT_PORT,
            user: "joe".to_string(),
            authentication,
            key_algorithm: "ssh-ed25519".to_string(),
            encoded_key: TEST_HOST_KEY.to_string(),
            password: password.map(str::to_string),
        }
    }

    #[test]
    fn metadata_and_renderer_status_never_contain_the_password() {
        let store = Arc::new(MemorySecretStore::default());
        let service = SshHostService::with_secret_store(store.clone());
        let prepared = input(SshAuthentication::Password, Some("swordfish"))
            .prepare()
            .expect("valid host");
        let id = prepared.entry.id();
        service
            .store_password(id, prepared.password.expect("password"))
            .expect("native write");

        let metadata = serde_json::to_string(&prepared.entry).expect("serialize metadata");
        let report = serde_json::to_string(&service.view(&prepared.entry).expect("host view"))
            .expect("serialize view");
        assert!(!metadata.contains("swordfish"));
        assert!(!report.contains("swordfish"));
        assert!(report.contains("available"));
        assert_eq!(store.secret(&host_account(id)), Some(b"swordfish".to_vec()));
        assert!(service.password(prepared.entry.record()).unwrap().is_some());
    }

    #[test]
    fn agent_hosts_reject_passwords_and_need_no_vault_entry() {
        assert!(matches!(
            input(SshAuthentication::Agent, Some("must-not-be-kept")).prepare(),
            Err(SshHostsError::PasswordNotAccepted)
        ));

        let prepared = input(SshAuthentication::Agent, None)
            .prepare()
            .expect("agent host");
        let service = SshHostService::with_secret_store(Arc::new(MemorySecretStore::default()));
        assert_eq!(
            service.credential_status(prepared.entry.record()),
            SshCredentialStatus::NotRequired
        );
        assert!(service.password(prepared.entry.record()).unwrap().is_none());
    }

    #[test]
    fn malformed_wire_keys_are_rejected_before_persistence() {
        let mut invalid = input(SshAuthentication::Agent, None);
        invalid.encoded_key = "plausible-but-not-an-ssh-key".to_string();
        assert!(matches!(
            invalid.prepare(),
            Err(SshHostsError::InvalidHostKey)
        ));
    }

    #[test]
    fn a_denied_native_write_leaves_no_password_in_metadata() {
        let store = Arc::new(MemorySecretStore::default());
        store.fail(SecretOperation::Write, SecretStoreErrorKind::AccessDenied);
        let service = SshHostService::with_secret_store(store.clone());
        let prepared = input(SshAuthentication::Password, Some("never-persisted"))
            .prepare()
            .expect("valid host");
        let id = prepared.entry.id();

        assert!(matches!(
            service.store_password(id, prepared.password.expect("password")),
            Err(SshHostsError::CredentialUnavailable)
        ));
        assert_eq!(store.secret(&host_account(id)), None);
        assert!(
            !serde_json::to_string(&prepared.entry)
                .expect("serialize metadata")
                .contains("never-persisted")
        );
    }

    #[test]
    fn normalization_keeps_one_stable_row_per_host_identity() {
        let first = input(SshAuthentication::Agent, None)
            .prepare()
            .expect("first")
            .entry;
        let mut duplicate = first.clone();
        duplicate.label = " duplicate ".to_string();

        let normalized = normalize_entries(vec![first.clone(), duplicate]);
        assert_eq!(normalized, vec![first]);
    }

    #[test]
    fn updates_require_an_existing_identity_and_new_rows_cannot_collide() {
        let first = input(SshAuthentication::Agent, None)
            .prepare()
            .expect("first")
            .entry;
        let mut rows = vec![first.clone()];
        assert!(matches!(
            upsert_entry(&mut rows, first.clone(), false),
            Err(SshHostsError::HostAlreadyExists)
        ));

        let missing = input(SshAuthentication::Agent, None)
            .prepare()
            .expect("missing")
            .entry;
        assert!(matches!(
            upsert_entry(&mut rows, missing, true),
            Err(SshHostsError::HostNotFound)
        ));
        assert_eq!(remove_entry(&mut rows, first.id()).unwrap(), first);
        assert!(rows.is_empty());
    }
}
