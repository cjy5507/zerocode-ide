//! Durable Linear connection metadata with its API key in native credentials.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::credential_store::{SecretStanding, VerifiedSecretError, VerifiedSecretStore};

pub const FILE_NAME: &str = "linear-connection.json";
const CREDENTIAL_SERVICE: &str = "dev.zerocode.shell.linear";
const CREDENTIAL_ACCOUNT: &str = "personal-api-key";
const FILE_VERSION: u8 = 1;

static STORE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinearConnection {
    pub user_id: String,
    pub user_name: String,
    pub user_email: String,
    pub organization_id: String,
    pub organization_name: String,
    pub teams: Vec<zerocode_core::linear::Team>,
    pub active_team_id: Option<String>,
}

impl From<zerocode_core::linear::Connection> for LinearConnection {
    fn from(connection: zerocode_core::linear::Connection) -> Self {
        let active_team_id = connection.teams.first().map(|team| team.id.clone());
        Self {
            user_id: connection.user_id,
            user_name: connection.user_name,
            user_email: connection.user_email,
            organization_id: connection.organization_id,
            organization_name: connection.organization_name,
            teams: connection.teams,
            active_team_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LinearCredentialStanding {
    Available,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinearStoreStatus {
    pub connected: bool,
    pub credential: LinearCredentialStanding,
    pub connection: Option<LinearConnection>,
}

#[derive(Debug)]
pub enum LinearStoreError {
    Io(io::Error),
    InvalidMetadata(String),
    Credential(VerifiedSecretError),
    EmptyCredential,
}

impl fmt::Display for LinearStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Linear 설정을 저장하지 못했습니다: {error}"),
            Self::InvalidMetadata(message) => formatter.write_str(message),
            Self::Credential(VerifiedSecretError::Missing) => {
                formatter.write_str("저장된 Linear API 키가 없습니다.")
            }
            Self::Credential(_) => {
                formatter.write_str("Linear API 키 보안 저장소를 사용할 수 없습니다.")
            }
            Self::EmptyCredential => formatter.write_str("Linear API 키를 입력하세요."),
        }
    }
}

impl std::error::Error for LinearStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for LinearStoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub type StoreResult<T> = Result<T, LinearStoreError>;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredConnection {
    version: u8,
    connection: LinearConnection,
}

#[derive(Clone)]
pub struct LinearStore {
    root: PathBuf,
    credentials: VerifiedSecretStore,
}

impl LinearStore {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            credentials: VerifiedSecretStore::new(CREDENTIAL_SERVICE),
        }
    }

    pub fn load(&self) -> StoreResult<Option<LinearConnection>> {
        let _lock = store_lock()?;
        self.load_locked()
    }

    pub fn status(&self) -> StoreResult<LinearStoreStatus> {
        let _lock = store_lock()?;
        let connection = self.load_locked()?;
        let credential = credential_standing(self.credentials.standing(CREDENTIAL_ACCOUNT));
        Ok(LinearStoreStatus {
            connected: connection.is_some() && credential == LinearCredentialStanding::Available,
            credential,
            connection,
        })
    }

    pub fn read_api_key(&self) -> StoreResult<Zeroizing<String>> {
        self.credentials
            .read(CREDENTIAL_ACCOUNT)
            .map_err(LinearStoreError::Credential)
    }

    pub fn connect_commit(
        &self,
        connection: LinearConnection,
        api_key: &str,
    ) -> StoreResult<LinearStoreStatus> {
        validate_connection(&connection)?;
        if api_key.is_empty() {
            return Err(LinearStoreError::EmptyCredential);
        }
        let _lock = store_lock()?;
        let previous = self.credentials.read(CREDENTIAL_ACCOUNT).ok();
        self.credentials
            .write(CREDENTIAL_ACCOUNT, Zeroizing::new(api_key.to_string()))
            .map_err(LinearStoreError::Credential)?;
        if let Err(error) = self.write_locked(&connection) {
            match previous {
                Some(secret) => {
                    let _ = self.credentials.write(CREDENTIAL_ACCOUNT, secret);
                }
                None => {
                    let _ = self.credentials.delete(CREDENTIAL_ACCOUNT);
                }
            }
            return Err(error);
        }
        let credential = credential_standing(self.credentials.standing(CREDENTIAL_ACCOUNT));
        Ok(LinearStoreStatus {
            connected: credential == LinearCredentialStanding::Available,
            credential,
            connection: Some(connection),
        })
    }

    pub fn disconnect(&self) -> StoreResult<LinearStoreStatus> {
        let _lock = store_lock()?;
        self.credentials
            .delete(CREDENTIAL_ACCOUNT)
            .map_err(LinearStoreError::Credential)?;
        let path = self.path();
        match std::fs::remove_file(&path) {
            Ok(()) => {
                let _ = crate::durable_file::sync_visible_parent(&path)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(LinearStoreError::Io(error)),
        }
        Ok(LinearStoreStatus {
            connected: false,
            credential: LinearCredentialStanding::Missing,
            connection: None,
        })
    }

    fn load_locked(&self) -> StoreResult<Option<LinearConnection>> {
        let bytes = match crate::durable_file::read_plain_file(&self.path()) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(LinearStoreError::Io(error)),
        };
        let stored: StoredConnection = serde_json::from_slice(&bytes).map_err(|error| {
            LinearStoreError::InvalidMetadata(format!("Linear 설정을 읽지 못했습니다: {error}"))
        })?;
        if stored.version != FILE_VERSION {
            return Err(LinearStoreError::InvalidMetadata(format!(
                "지원하지 않는 Linear 설정 버전입니다: {}",
                stored.version
            )));
        }
        validate_connection(&stored.connection)?;
        Ok(Some(stored.connection))
    }

    fn write_locked(&self, connection: &LinearConnection) -> StoreResult<()> {
        let bytes = serde_json::to_vec_pretty(&StoredConnection {
            version: FILE_VERSION,
            connection: connection.clone(),
        })
        .map_err(|error| {
            LinearStoreError::InvalidMetadata(format!("Linear 설정을 만들지 못했습니다: {error}"))
        })?;
        let _ = crate::durable_file::replace_bytes(&self.path(), &bytes)?;
        Ok(())
    }

    fn path(&self) -> PathBuf {
        self.root.join(FILE_NAME)
    }
}

fn store_lock() -> StoreResult<MutexGuard<'static, ()>> {
    STORE_LOCK.lock().map_err(|_| {
        LinearStoreError::InvalidMetadata("Linear 설정 잠금이 손상되었습니다.".to_string())
    })
}

fn credential_standing(standing: SecretStanding) -> LinearCredentialStanding {
    match standing {
        SecretStanding::Available => LinearCredentialStanding::Available,
        SecretStanding::Missing => LinearCredentialStanding::Missing,
        SecretStanding::Unreadable => LinearCredentialStanding::Unreadable,
    }
}

fn validate_connection(connection: &LinearConnection) -> StoreResult<()> {
    for (name, value) in [
        ("사용자 ID", connection.user_id.as_str()),
        ("사용자 이름", connection.user_name.as_str()),
        ("조직 ID", connection.organization_id.as_str()),
        ("조직 이름", connection.organization_name.as_str()),
    ] {
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(LinearStoreError::InvalidMetadata(format!(
                "Linear {name}이 올바르지 않습니다."
            )));
        }
    }
    if connection.teams.len() > 1_000 {
        return Err(LinearStoreError::InvalidMetadata(
            "Linear 팀 수가 허용 범위를 넘었습니다.".to_string(),
        ));
    }
    if connection
        .active_team_id
        .as_deref()
        .is_some_and(|active| !connection.teams.iter().any(|team| team.id == active))
    {
        return Err(LinearStoreError::InvalidMetadata(
            "활성 Linear 팀을 찾지 못했습니다.".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> LinearConnection {
        LinearConnection {
            user_id: "user-1".into(),
            user_name: "Hana".into(),
            user_email: "hana@example.com".into(),
            organization_id: "org-1".into(),
            organization_name: "Acme".into(),
            teams: vec![zerocode_core::linear::Team {
                id: "team-1".into(),
                key: "ENG".into(),
                name: "Engineering".into(),
            }],
            active_team_id: Some("team-1".into()),
        }
    }

    #[test]
    fn connect_keeps_secret_out_of_metadata() {
        let root = tempfile::tempdir().expect("state root");
        let store = LinearStore::new(root.path());
        store
            .connect_commit(connection(), "lin_api_secret")
            .expect("connected");
        let metadata = std::fs::read_to_string(root.path().join(FILE_NAME)).expect("metadata");
        assert!(!metadata.contains("lin_api_secret"));
    }

    #[test]
    fn disconnect_removes_metadata_and_credential() {
        let root = tempfile::tempdir().expect("state root");
        let store = LinearStore::new(root.path());
        store
            .connect_commit(connection(), "lin_api_secret")
            .expect("connected");
        store.disconnect().expect("disconnected");
        assert!(!store.status().expect("status").connected);
    }
}
