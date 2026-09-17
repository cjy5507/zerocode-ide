//! Durable Jira site and credential metadata.
//!
//! The store keeps public site metadata in the application state directory and
//! credentials in an injected secret store. A status response can therefore
//! be serialized for the renderer without ever loading a secret. Production
//! uses the OS-native vault; tests use isolated in-memory credentials.

use crate::credential_store::{
    SecretProtection, SecretStanding, SecretStore, SecretStoreError, SecretStoreErrorKind,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use zeroize::{Zeroize, Zeroizing};

pub const SITE_FILE_NAME: &str = "jira-sites.json";
pub const TOKEN_DIRECTORY_NAME: &str = "jira-tokens";
#[cfg(all(not(test), not(feature = "test-memory")))]
const JIRA_CREDENTIAL_SERVICE: &str = "dev.zerocode.shell.jira";
const SITE_ACCOUNT_PREFIX: &str = "site:";
const PENDING_ACCOUNT_PREFIX: &str = "pending-site:";
const LOCK_FILE_NAME: &str = ".jira-store.lock";
const PENDING_DELETE_PREFIX: &str = ".pending-delete-";
const PENDING_DELETE_SUFFIX: &str = ".json";
const PENDING_CONNECT_PREFIX: &str = ".pending-connect-";
const PENDING_CONNECT_TEMP_PREFIX: &str = "..pending-connect-";
const PENDING_CONNECT_SUFFIX: &str = ".json";
pub const PATH_MIGRATION_SCHEMA: &str = "jira-transform-v1";
pub const PATH_MIGRATION_TRANSACTION_PARTS: &[&str] = &[
    PENDING_DELETE_PREFIX,
    PENDING_DELETE_SUFFIX,
    PENDING_CONNECT_PREFIX,
    PENDING_CONNECT_TEMP_PREFIX,
    PENDING_CONNECT_SUFFIX,
];
const PENDING_CONNECT_VERSION: u8 = 1;
const PENDING_DELETE_VERSION: u8 = 1;
const LEGACY_FILE_VERSION: u8 = 1;
const CURRENT_FILE_VERSION: u8 = 2;
const HTTPS_ERROR: &str = "Jira 사이트 주소는 https:// 로 시작해야 합니다";
const EMPTY_TOKEN_ERROR: &str = "토큰을 입력하세요";

static TEMP_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);
// File locks alone do not consistently serialize independent descriptors in
// one process on every desktop platform.  Pair the stable on-disk lock with a
// process lock so cloned stores and independently constructed handles obey the
// same transaction boundary.
static PROCESS_LOCK: Mutex<()> = Mutex::new(());

fn site_account(site_id: &str) -> String {
    format!("{SITE_ACCOUNT_PREFIX}{site_id}")
}

fn pending_account(site_id: &str) -> String {
    format!("{PENDING_ACCOUNT_PREFIX}{site_id}")
}

fn default_auth_type() -> String {
    "cloud".to_string()
}

/// Public site metadata.  There is intentionally no credential field here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraSite {
    pub id: String,
    pub site_url: String,
    pub email: String,
    pub display_name: String,
    pub account_id: String,
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
}

/// Which connected sites a Jira read should address.
///
/// `active_site_id` is stored separately.  Selecting `All` therefore does not
/// forget which single site was active and selecting `Site` again is stable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "site_id", rename_all = "snake_case")]
pub enum JiraSelection {
    Site(String),
    #[default]
    All,
}

/// The canonical on-disk metadata envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JiraSiteFile {
    pub version: u8,
    pub sites: Vec<JiraSite>,
    pub active_site_id: Option<String>,
    pub selected: JiraSelection,
}

impl Default for JiraSiteFile {
    fn default() -> Self {
        Self {
            version: CURRENT_FILE_VERSION,
            sites: Vec::new(),
            active_site_id: None,
            selected: JiraSelection::All,
        }
    }
}

/// Credential availability determined without reading the credential value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraCredentialStanding {
    Available,
    Missing,
    Unreadable,
}

/// The protection actually provided by this store.
///
/// `Plaintext` means a legacy migration source still exists. It is never used
/// as the live credential backend. `Unavailable` means the platform-native
/// store could not be initialized or accessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraCredentialProtection {
    Native,
    Plaintext,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JiraSiteStatus {
    #[serde(flatten)]
    pub site: JiraSite,
    pub active: bool,
    pub selected: bool,
    pub credential: JiraCredentialStanding,
}

/// Renderer-safe storage status.  Its type graph contains no token value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JiraStoreStatus {
    pub connected: bool,
    pub active_site_id: Option<String>,
    pub selected: JiraSelection,
    pub credential_protection: JiraCredentialProtection,
    pub sites: Vec<JiraSiteStatus>,
}

/// Canonical result of resolving `Site` or `All` for a read operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedJiraSelection {
    pub active_site_id: Option<String>,
    pub selected: JiraSelection,
    pub sites: Vec<JiraSite>,
}

#[derive(Debug)]
pub enum JiraStoreError {
    Io {
        action: &'static str,
        source: io::Error,
    },
    InvalidMetadata(String),
    UnsupportedVersion(u8),
    SiteNotFound(String),
    CredentialUnavailable(String),
    CredentialAccessDenied(String),
    CredentialStoreUnavailable(String),
    EmptyCredential,
    Transaction {
        action: &'static str,
        original: Box<JiraStoreError>,
        rollback: Box<JiraStoreError>,
    },
}

impl fmt::Display for JiraStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { action, source } => write!(formatter, "{action}: {source}"),
            Self::InvalidMetadata(reason) => formatter.write_str(reason),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "지원하지 않는 Jira 설정 버전입니다: {version}")
            }
            Self::SiteNotFound(id) => write!(formatter, "Jira 사이트를 찾지 못했습니다: {id}"),
            Self::CredentialUnavailable(id) => {
                write!(formatter, "저장된 Jira 토큰을 읽지 못했습니다: {id}")
            }
            Self::CredentialAccessDenied(id) => {
                write!(formatter, "Jira 보안 저장소 접근이 거부되었습니다: {id}")
            }
            Self::CredentialStoreUnavailable(id) => {
                write!(formatter, "Jira 보안 저장소를 사용할 수 없습니다: {id}")
            }
            Self::EmptyCredential => formatter.write_str(EMPTY_TOKEN_ERROR),
            Self::Transaction {
                action,
                original,
                rollback,
            } => write!(
                formatter,
                "{action} 처리와 복구가 모두 실패했습니다: {original}; 복구: {rollback}"
            ),
        }
    }
}

impl std::error::Error for JiraStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Transaction { original, .. } => Some(original.as_ref()),
            _ => None,
        }
    }
}

pub type StoreResult<T> = Result<T, JiraStoreError>;

#[derive(Debug, Deserialize)]
struct StoredJiraSiteFile {
    version: u8,
    #[serde(default)]
    sites: Vec<JiraSite>,
    #[serde(default)]
    active_site_id: Option<String>,
    #[serde(default)]
    selected: Option<JiraSelection>,
}

/// Non-secret half of an interrupted connect transaction. The credential is
/// staged under a separate native-vault account; this journal carries only the
/// metadata state that decides whether recovery commits or abandons it.
#[derive(Debug, Serialize, Deserialize)]
struct PendingConnectJournal {
    version: u8,
    site: JiraSite,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingDeleteJournal {
    version: u8,
    site_id: String,
}

#[derive(Clone, Copy)]
enum ConnectCommitBehavior {
    Complete,
    #[cfg(test)]
    InterruptBeforeMetadata,
    #[cfg(test)]
    InterruptAfterMetadata,
}

/// Jira persistence rooted at an injected application state directory.
#[derive(Debug, Clone)]
pub struct JiraStore {
    root: PathBuf,
    credentials: Arc<dyn SecretStore>,
}

/// Holds both halves of the store transaction lock until it is dropped.
///
/// The lock file is deliberately never replaced or removed: every process
/// opens the same inode/path, so an atomic metadata replacement cannot split
/// contenders across different lock objects.
struct JiraStoreLock<'a> {
    _file: File,
    _process: MutexGuard<'a, ()>,
}

impl JiraStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        #[cfg(all(not(test), not(feature = "test-memory")))]
        let credentials = crate::credential_store::native_secret_store(JIRA_CREDENTIAL_SERVICE);
        #[cfg(any(test, feature = "test-memory"))]
        let credentials = Arc::new(crate::credential_store::MemorySecretStore::default());
        Self::with_secret_store(root, credentials)
    }

    /// Finish pending transactions and native-vault promotion before the
    /// enclosing application relocates Jira metadata to another directory.
    ///
    /// Credentials are keyed independently of the filesystem root. A
    /// successful return therefore means the relocation may copy metadata
    /// only; no plaintext token or transaction journal needs to travel.
    pub fn prepare_path_migration(&self) -> StoreResult<()> {
        let file = self.load()?;
        if self.has_legacy_credentials(&file) || self.has_any_legacy_token_artifact()? {
            return Err(JiraStoreError::CredentialStoreUnavailable(
                "legacy-path-migration".to_string(),
            ));
        }
        if self.credentials.protection() != SecretProtection::Native {
            return Err(JiraStoreError::CredentialStoreUnavailable(
                "legacy-path-migration".to_string(),
            ));
        }
        Ok(())
    }

    /// A site index is not an exhaustive credential inventory: interrupted
    /// old versions can leave orphan, pending, temporary, or malformed files.
    /// After recovery, any remaining entry blocks the path authority switch so
    /// plaintext can never be stranded under HOME and silently forgotten.
    fn has_any_legacy_token_artifact(&self) -> StoreResult<bool> {
        let mut entries = match fs::read_dir(self.token_directory()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => {
                return Err(io_error("Jira 토큰 폴더를 검사하지 못했습니다", source));
            }
        };
        match entries.next() {
            Some(entry) => {
                entry.map_err(|source| io_error("Jira 토큰 폴더를 검사하지 못했습니다", source))?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    fn with_secret_store(root: impl Into<PathBuf>, credentials: Arc<dyn SecretStore>) -> Self {
        Self {
            root: root.into(),
            credentials,
        }
    }

    /// Load, validate and normalize metadata.  Version 1 is migrated on read.
    pub fn load(&self) -> StoreResult<JiraSiteFile> {
        let _lock = self.lock()?;
        self.load_locked()
    }

    fn load_locked(&self) -> StoreResult<JiraSiteFile> {
        let stored = self.read_metadata_locked()?;
        let (canonical, should_persist) = reconcile_metadata(stored)?;

        self.recover_pending_transactions_locked(&canonical)?;
        self.migrate_legacy_credentials(&canonical);
        if should_persist {
            self.write_site_file(&canonical)?;
        }
        Ok(canonical)
    }

    fn read_metadata_locked(&self) -> StoreResult<Option<StoredJiraSiteFile>> {
        match crate::durable_file::read_plain_file(&self.site_file()) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice::<StoredJiraSiteFile>(&bytes).map_err(|error| {
                    invalid_metadata(format!("Jira 설정을 읽지 못했습니다: {error}"))
                })?,
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(io_error("Jira 설정을 읽지 못했습니다", source)),
        }
    }

    fn recover_pending_transactions_locked(&self, canonical: &JiraSiteFile) -> StoreResult<()> {
        self.recover_pending_connects(canonical)?;
        self.recover_legacy_pending_deletions(canonical)?;
        self.recover_pending_delete_journals(canonical)
    }

    /// Return renderer-safe status without reading a token value.
    pub fn status(&self) -> StoreResult<JiraStoreStatus> {
        let _lock = self.lock()?;
        self.status_locked()
    }

    fn status_locked(&self) -> StoreResult<JiraStoreStatus> {
        let file = self.load_locked()?;
        let mut any_selected_available = false;
        let sites = file
            .sites
            .iter()
            .cloned()
            .map(|site| {
                let selected = match &file.selected {
                    JiraSelection::Site(id) => id == &site.id,
                    JiraSelection::All => true,
                };
                let credential = self.credential_standing(&site.id);
                any_selected_available |=
                    selected && credential == JiraCredentialStanding::Available;
                JiraSiteStatus {
                    active: file.active_site_id.as_deref() == Some(site.id.as_str()),
                    selected,
                    credential,
                    site,
                }
            })
            .collect();

        let credential_protection = self.credential_protection(&file);
        Ok(JiraStoreStatus {
            connected: any_selected_available,
            active_site_id: file.active_site_id,
            selected: file.selected,
            credential_protection,
            sites,
        })
    }

    /// Commit a site only after the caller has completed its live probe.
    ///
    /// A token is opaque credential data: leading and trailing whitespace is
    /// stored byte-for-byte because Server Basic passwords may legitimately
    /// use it. The new value is staged under a separate native-vault account,
    /// then a non-secret journal and metadata are committed before promotion.
    /// Recovery either completes that promotion or abandons the staged entry;
    /// the previous canonical credential is never overwritten before metadata.
    pub fn connect_commit(&self, site: JiraSite, token: &str) -> StoreResult<JiraStoreStatus> {
        self.connect_commit_with_behavior(site, token, ConnectCommitBehavior::Complete)
    }

    fn connect_commit_with_behavior(
        &self,
        mut site: JiraSite,
        token: &str,
        behavior: ConnectCommitBehavior,
    ) -> StoreResult<JiraStoreStatus> {
        normalize_site(&mut site)?;
        if token.is_empty() {
            return Err(JiraStoreError::EmptyCredential);
        }

        let _lock = self.lock()?;
        let mut file = self.load_locked()?;
        // A reconnect must not let a stale migration source survive beside a
        // newer native credential. Require this site's migration to finish
        // before staging the replacement; unrelated sites remain isolated.
        self.migrate_legacy_credential(&site.id)?;
        self.stage_pending_connect(&site, token)?;

        #[cfg(test)]
        if matches!(behavior, ConnectCommitBehavior::InterruptBeforeMetadata) {
            return Err(invalid_metadata(
                "test interrupted Jira connect before metadata commit",
            ));
        }

        let site_id = site.id.clone();
        match file.sites.iter_mut().find(|stored| stored.id == site_id) {
            Some(stored) => *stored = site,
            None => file.sites.push(site),
        }
        file.active_site_id = Some(site_id.clone());
        file.selected = JiraSelection::Site(site_id.clone());
        file = normalize_file(file.sites, file.active_site_id, Some(file.selected))?;

        self.write_site_file(&file)?;
        // A failed directory sync is recoverable because both durable staging
        // halves still exist. The next load observes whichever metadata state
        // survived and deterministically promotes or abandons the token.
        sync_directory(&self.root)?;

        #[cfg(test)]
        if matches!(behavior, ConnectCommitBehavior::InterruptAfterMetadata) {
            return Err(invalid_metadata(
                "test interrupted Jira connect after metadata commit",
            ));
        }

        #[cfg(not(test))]
        let _ = behavior;

        self.promote_pending_connect(&site_id)?;
        self.status_locked()
    }

    /// Select one site or all sites, returning the canonical status.
    pub fn select(&self, selected: JiraSelection) -> StoreResult<JiraStoreStatus> {
        let _lock = self.lock()?;
        let mut file = self.load_locked()?;
        match &selected {
            JiraSelection::Site(id) => {
                validate_site_id(id)?;
                if !file.sites.iter().any(|site| site.id == *id) {
                    return Err(JiraStoreError::SiteNotFound(id.clone()));
                }
                file.active_site_id = Some(id.clone());
            }
            JiraSelection::All => {
                // Deliberately retain the active site.
            }
        }
        file.selected = selected;
        self.write_site_file(&file)?;
        self.status_locked()
    }

    /// Resolve the selection without touching any credential.
    pub fn resolve_selection(&self) -> StoreResult<ResolvedJiraSelection> {
        let _lock = self.lock()?;
        let file = self.load_locked()?;
        let sites = match &file.selected {
            JiraSelection::Site(id) => file
                .sites
                .iter()
                .find(|site| site.id == *id)
                .cloned()
                .into_iter()
                .collect(),
            JiraSelection::All => file.sites.clone(),
        };
        Ok(ResolvedJiraSelection {
            active_site_id: file.active_site_id,
            selected: file.selected,
            sites,
        })
    }

    /// Remove one site's metadata and only that site's native credential.
    ///
    /// A non-secret journal is durable before metadata changes. Recovery drops
    /// it when metadata is still live, or completes credential deletion after
    /// metadata has committed. No secret is copied to the filesystem.
    pub fn disconnect(&self, id: &str) -> StoreResult<JiraStoreStatus> {
        validate_site_id(id)?;
        let _lock = self.lock()?;
        let mut file = self.load_locked()?;
        if !file.sites.iter().any(|site| site.id == id) {
            return Err(JiraStoreError::SiteNotFound(id.to_string()));
        }

        let previous = file.clone();
        self.write_pending_delete_journal(id)?;
        file.sites.retain(|site| site.id != id);
        if file.active_site_id.as_deref() == Some(id) {
            file.active_site_id = file.sites.first().map(|site| site.id.clone());
        }
        if matches!(&file.selected, JiraSelection::Site(selected) if selected == id) {
            file.selected = file
                .active_site_id
                .clone()
                .map(JiraSelection::Site)
                .unwrap_or(JiraSelection::All);
        }
        file = normalize_file(file.sites, file.active_site_id, Some(file.selected))?;

        if let Err(original) = self.write_site_file(&file) {
            let rollback = remove_file_if_present(&self.pending_delete_journal_file(id))
                .and_then(|_| self.write_site_file(&previous));
            return match rollback {
                Ok(()) => Err(original),
                Err(rollback) => Err(JiraStoreError::Transaction {
                    action: "Jira 연결 해제",
                    original: Box::new(original),
                    rollback: Box::new(rollback),
                }),
            };
        }
        sync_directory(&self.root)?;
        self.finish_pending_delete(id)?;
        self.status_locked()
    }

    pub fn read_token(&self, id: &str) -> StoreResult<String> {
        validate_site_id(id)?;
        let _lock = self.lock()?;
        self.read_token_locked(id)
    }

    fn read_token_locked(&self, id: &str) -> StoreResult<String> {
        self.migrate_legacy_credential(id)?;
        let bytes = self
            .credentials
            .read(&site_account(id))
            .map_err(|error| credential_error(id, error))?;
        if bytes.is_empty() {
            return Err(JiraStoreError::CredentialUnavailable(id.to_string()));
        }
        match String::from_utf8(bytes) {
            Ok(value) => Ok(value),
            Err(error) => {
                let mut bytes = error.into_bytes();
                bytes.zeroize();
                Err(JiraStoreError::CredentialUnavailable(id.to_string()))
            }
        }
    }

    #[cfg(any(test, feature = "test-memory"))]
    pub fn write_token(&self, id: &str, token: &str) -> StoreResult<()> {
        validate_site_id(id)?;
        if token.is_empty() {
            return Err(JiraStoreError::EmptyCredential);
        }
        let _lock = self.lock()?;
        self.write_token_locked(id, token)
    }

    #[cfg(any(test, feature = "test-memory"))]
    fn write_token_locked(&self, id: &str, token: &str) -> StoreResult<()> {
        validate_site_id(id)?;
        self.credentials
            .write(&site_account(id), token.as_bytes())
            .map_err(|error| credential_error(id, error))
    }

    fn stage_pending_connect(&self, site: &JiraSite, token: &str) -> StoreResult<()> {
        validate_site_id(&site.id)?;
        let pending_account = pending_account(&site.id);
        self.credentials
            .delete(&pending_account)
            .map_err(|error| credential_error(&site.id, error))?;
        let journal = PendingConnectJournal {
            version: PENDING_CONNECT_VERSION,
            site: site.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&journal).map_err(|error| {
            invalid_metadata(format!(
                "Jira 연결 복구 상태를 직렬화하지 못했습니다: {error}"
            ))
        })?;
        atomic_write(&self.pending_connect_journal_file(&site.id), &bytes, 0o600)?;
        sync_directory(&self.root)?;
        self.credentials
            .write(&pending_account, token.as_bytes())
            .map_err(|error| credential_error(&site.id, error))
    }

    fn promote_pending_connect(&self, id: &str) -> StoreResult<()> {
        let pending_account = pending_account(id);
        let bytes = Zeroizing::new(
            self.credentials
                .read(&pending_account)
                .map_err(|error| credential_error(id, error))?,
        );
        if bytes.is_empty() {
            return Err(JiraStoreError::CredentialUnavailable(id.to_string()));
        }
        self.write_verified_secret(id, &bytes).and_then(|_| {
            // Keep the pending native entry and journal until every stale
            // plaintext source is gone. If unlinking fails, recovery can
            // safely retry the same new bytes instead of importing the old
            // canonical file over them.
            self.remove_legacy_credential(id)?;
            remove_file_if_present(&self.pending_connect_token_file(id))?;
            self.credentials
                .delete(&pending_account)
                .map_err(|error| credential_error(id, error))?;
            remove_file_if_present(&self.pending_connect_journal_file(id))?;
            sync_directory(&self.root)
        })
    }

    fn credential_standing(&self, id: &str) -> JiraCredentialStanding {
        if self.token_file(id).exists() {
            return JiraCredentialStanding::Unreadable;
        }
        match self.credentials.standing(&site_account(id)) {
            SecretStanding::Available => JiraCredentialStanding::Available,
            SecretStanding::Missing => JiraCredentialStanding::Missing,
            SecretStanding::Unreadable => JiraCredentialStanding::Unreadable,
        }
    }

    fn recover_pending_connects(&self, file: &JiraSiteFile) -> StoreResult<()> {
        let mut journals = Vec::new();
        let mut stale_journal_temporaries = Vec::new();
        for entry in fs::read_dir(&self.root)
            .map_err(|source| io_error("Jira 연결 복구 상태를 읽지 못했습니다", source))?
        {
            let entry = entry
                .map_err(|source| io_error("Jira 연결 복구 상태를 읽지 못했습니다", source))?;
            let name = entry.file_name();
            if let Some(id) = pending_connect_journal_id(&name.to_string_lossy()) {
                journals.push((id.to_string(), entry.path()));
            } else if name
                .to_string_lossy()
                .starts_with(PENDING_CONNECT_TEMP_PREFIX)
            {
                stale_journal_temporaries.push(entry.path());
            }
        }
        if !stale_journal_temporaries.is_empty() {
            for path in stale_journal_temporaries {
                remove_file_if_present(&path)?;
            }
            sync_directory(&self.root)?;
        }
        journals.sort_by(|left, right| left.0.cmp(&right.0));

        for (id, journal_path) in journals {
            if validate_site_id(&id).is_err() {
                return Err(invalid_metadata(
                    "Jira 연결 복구 상태의 ID가 유효하지 않습니다",
                ));
            }
            let journal = match crate::durable_file::read_plain_file(&journal_path) {
                Ok(bytes) => {
                    serde_json::from_slice::<PendingConnectJournal>(&bytes).map_err(|error| {
                        invalid_metadata(format!("Jira 연결 복구 상태가 손상되었습니다: {error}"))
                    })?
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(io_error("Jira 연결 복구 상태를 읽지 못했습니다", source));
                }
            };
            let mut expected = journal.site;
            if journal.version != PENDING_CONNECT_VERSION || expected.id != id {
                return Err(invalid_metadata("Jira 연결 복구 상태가 유효하지 않습니다"));
            }
            normalize_site(&mut expected)?;
            let metadata_matches = file.sites.iter().any(|site| site == &expected);
            if metadata_matches {
                match self.credentials.standing(&pending_account(&id)) {
                    SecretStanding::Available => {
                        self.promote_pending_connect(&id)?;
                        continue;
                    }
                    SecretStanding::Unreadable => {
                        return Err(JiraStoreError::CredentialStoreUnavailable(id));
                    }
                    SecretStanding::Missing => {
                        self.promote_legacy_pending_connect(&id)?;
                    }
                }
            } else {
                self.credentials
                    .delete(&pending_account(&id))
                    .map_err(|error| credential_error(&id, error))?;
                if remove_file_if_present(&self.pending_connect_token_file(&id))? {
                    sync_directory(self.token_directory())?;
                }
            }
            remove_file_if_present(&journal_path)?;
            sync_directory(&self.root)?;
        }

        // A process can end after staging the credential but before writing
        // its journal. No metadata can refer to such a file, so it is always
        // safe to discard.
        let directory = self.token_directory();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error("Jira 연결 대기 상태를 읽지 못했습니다", source)),
        };
        let mut removed = false;
        for entry in entries {
            let entry = entry
                .map_err(|source| io_error("Jira 연결 대기 상태를 읽지 못했습니다", source))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(PENDING_CONNECT_PREFIX)
                || name.starts_with(PENDING_CONNECT_TEMP_PREFIX)
            {
                removed |= remove_file_if_present(&entry.path())?;
            }
        }
        if removed {
            sync_directory(directory)?;
        }
        Ok(())
    }

    fn promote_legacy_pending_connect(&self, id: &str) -> StoreResult<()> {
        let path = self.pending_connect_token_file(id);
        let bytes = Zeroizing::new(match crate::durable_file::read_plain_file(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error("기존 Jira 토큰을 읽지 못했습니다", source)),
        });
        self.write_verified_secret(id, &bytes).and_then(|_| {
            self.remove_legacy_credential(id)?;
            remove_file_if_present(&path)?;
            sync_directory(self.token_directory())
        })
    }

    fn recover_legacy_pending_deletions(&self, file: &JiraSiteFile) -> StoreResult<()> {
        let directory = self.token_directory();
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(source) => return Err(io_error("Jira 토큰 복구 상태를 읽지 못했습니다", source)),
        };
        let live: HashSet<&str> = file.sites.iter().map(|site| site.id.as_str()).collect();
        for entry in entries {
            let entry = entry
                .map_err(|source| io_error("Jira 토큰 복구 상태를 읽지 못했습니다", source))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            let Some(id) = name.strip_prefix(PENDING_DELETE_PREFIX) else {
                continue;
            };
            if validate_site_id(id).is_err() {
                continue;
            }
            let pending = entry.path();
            let token = self.token_file(id);
            crate::durable_file::require_plain_file_if_present(&pending).map_err(|source| {
                io_error("Jira 토큰 삭제 복구 상태를 검사하지 못했습니다", source)
            })?;
            let token_exists = crate::durable_file::require_plain_file_if_present(&token)
                .map_err(|source| io_error("기존 Jira 토큰을 검사하지 못했습니다", source))?;
            if live.contains(id) && !token_exists {
                fs::rename(&pending, &token)
                    .map_err(|source| io_error("Jira 토큰 삭제를 복구하지 못했습니다", source))?;
            } else {
                fs::remove_file(&pending).map_err(|source| {
                    io_error("완료된 Jira 토큰 삭제를 정리하지 못했습니다", source)
                })?;
            }
        }
        sync_directory(directory)
    }

    fn recover_pending_delete_journals(&self, file: &JiraSiteFile) -> StoreResult<()> {
        let mut journals = Vec::new();
        for entry in fs::read_dir(&self.root)
            .map_err(|source| io_error("Jira 삭제 복구 상태를 읽지 못했습니다", source))?
        {
            let entry = entry
                .map_err(|source| io_error("Jira 삭제 복구 상태를 읽지 못했습니다", source))?;
            let name = entry.file_name();
            if let Some(id) = pending_delete_journal_id(&name.to_string_lossy()) {
                journals.push((id.to_string(), entry.path()));
            }
        }
        journals.sort_by(|left, right| left.0.cmp(&right.0));
        for (id, path) in journals {
            validate_site_id(&id)?;
            let journal = crate::durable_file::read_plain_file(&path)
                .map_err(|source| io_error("Jira 삭제 복구 상태를 읽지 못했습니다", source))
                .and_then(|bytes| {
                    serde_json::from_slice::<PendingDeleteJournal>(&bytes).map_err(|error| {
                        invalid_metadata(format!("Jira 삭제 복구 상태가 손상되었습니다: {error}"))
                    })
                })?;
            if journal.version != PENDING_DELETE_VERSION || journal.site_id != id {
                return Err(invalid_metadata("Jira 삭제 복구 상태가 유효하지 않습니다"));
            }
            if file.sites.iter().any(|site| site.id == id) {
                remove_file_if_present(&path)?;
                sync_directory(&self.root)?;
                continue;
            }
            self.finish_pending_delete(&id)?;
        }
        Ok(())
    }

    fn write_pending_delete_journal(&self, id: &str) -> StoreResult<()> {
        let journal = PendingDeleteJournal {
            version: PENDING_DELETE_VERSION,
            site_id: id.to_string(),
        };
        let bytes = serde_json::to_vec_pretty(&journal).map_err(|error| {
            invalid_metadata(format!(
                "Jira 삭제 복구 상태를 직렬화하지 못했습니다: {error}"
            ))
        })?;
        atomic_write(&self.pending_delete_journal_file(id), &bytes, 0o600)?;
        sync_directory(&self.root)
    }

    fn finish_pending_delete(&self, id: &str) -> StoreResult<()> {
        self.credentials
            .delete(&site_account(id))
            .map_err(|error| credential_error(id, error))?;
        self.remove_legacy_credential(id)?;
        remove_file_if_present(&self.pending_delete_file(id))?;
        remove_file_if_present(&self.pending_delete_journal_file(id))?;
        sync_directory(&self.root)
    }

    fn migrate_legacy_credentials(&self, file: &JiraSiteFile) {
        for site in &file.sites {
            // A failed migration intentionally leaves the source untouched.
            // Status reports it as plaintext/unreadable and the next access
            // retries; plaintext is never returned as a live credential.
            let _ = self.migrate_legacy_credential(&site.id);
        }
    }

    fn migrate_legacy_credential(&self, id: &str) -> StoreResult<bool> {
        let path = self.token_file(id);
        let bytes = Zeroizing::new(match crate::durable_file::read_plain_file(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(source) => return Err(io_error("기존 Jira 토큰을 읽지 못했습니다", source)),
        });
        if bytes.is_empty() {
            return Err(JiraStoreError::CredentialUnavailable(id.to_string()));
        }
        self.write_verified_secret(id, &bytes).and_then(|_| {
            remove_file_if_present(&path)?;
            sync_directory(self.token_directory())?;
            Ok(true)
        })
    }

    fn write_verified_secret(&self, id: &str, bytes: &[u8]) -> StoreResult<()> {
        let account = site_account(id);
        self.credentials
            .write(&account, bytes)
            .map_err(|error| credential_error(id, error))?;
        let stored = Zeroizing::new(
            self.credentials
                .read(&account)
                .map_err(|error| credential_error(id, error))?,
        );
        let matches = stored.as_slice() == bytes;
        if !matches {
            return Err(JiraStoreError::CredentialStoreUnavailable(id.to_string()));
        }
        Ok(())
    }

    fn remove_legacy_credential(&self, id: &str) -> StoreResult<()> {
        if remove_file_if_present(&self.token_file(id))? {
            sync_directory(self.token_directory())?;
        }
        Ok(())
    }

    fn credential_protection(&self, file: &JiraSiteFile) -> JiraCredentialProtection {
        if self.has_legacy_credentials(file) {
            return JiraCredentialProtection::Plaintext;
        }
        match self.credentials.protection() {
            SecretProtection::Native => JiraCredentialProtection::Native,
            SecretProtection::Unavailable => JiraCredentialProtection::Unavailable,
        }
    }

    fn has_legacy_credentials(&self, file: &JiraSiteFile) -> bool {
        file.sites
            .iter()
            .any(|site| fs::symlink_metadata(self.token_file(&site.id)).is_ok())
    }

    fn write_site_file(&self, file: &JiraSiteFile) -> StoreResult<()> {
        let bytes = serde_json::to_vec_pretty(file).map_err(|error| {
            invalid_metadata(format!("Jira 설정을 직렬화하지 못했습니다: {error}"))
        })?;
        atomic_write(&self.site_file(), &bytes, 0o600)
    }

    fn site_file(&self) -> PathBuf {
        self.root.join(SITE_FILE_NAME)
    }

    fn lock(&self) -> StoreResult<JiraStoreLock<'_>> {
        let process = PROCESS_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::durable_file::ensure_private_directory(&self.root)
            .map_err(|source| io_error("Jira 설정 폴더를 만들지 못했습니다", source))?;
        let token_directory = self.token_directory();
        crate::durable_file::require_plain_directory_if_present(&token_directory)
            .map_err(|source| io_error("Jira 토큰 폴더를 검사하지 못했습니다", source))?;

        let path = self.root.join(LOCK_FILE_NAME);
        let file = crate::durable_file::private_lock_file(&path)
            .map_err(|source| io_error("Jira 설정 잠금 파일을 열지 못했습니다", source))?;
        lock_file_exclusively(&file)?;
        Ok(JiraStoreLock {
            _file: file,
            _process: process,
        })
    }

    fn token_directory(&self) -> PathBuf {
        self.root.join(TOKEN_DIRECTORY_NAME)
    }

    fn token_file(&self, id: &str) -> PathBuf {
        self.token_directory().join(id)
    }

    fn pending_delete_file(&self, id: &str) -> PathBuf {
        self.token_directory()
            .join(format!("{PENDING_DELETE_PREFIX}{id}"))
    }

    fn pending_delete_journal_file(&self, id: &str) -> PathBuf {
        self.root.join(format!(
            "{PENDING_DELETE_PREFIX}{id}{PENDING_DELETE_SUFFIX}"
        ))
    }

    fn pending_connect_token_file(&self, id: &str) -> PathBuf {
        self.token_directory()
            .join(format!("{PENDING_CONNECT_PREFIX}{id}"))
    }

    fn pending_connect_journal_file(&self, id: &str) -> PathBuf {
        self.root.join(format!(
            "{PENDING_CONNECT_PREFIX}{id}{PENDING_CONNECT_SUFFIX}"
        ))
    }
}

fn reconcile_metadata(stored: Option<StoredJiraSiteFile>) -> StoreResult<(JiraSiteFile, bool)> {
    match stored {
        None => Ok((JiraSiteFile::default(), false)),
        Some(stored) if stored.version == LEGACY_FILE_VERSION => {
            let file = normalize_file(stored.sites, None, None)?;
            Ok((file, true))
        }
        Some(stored) if stored.version == CURRENT_FILE_VERSION => {
            let selection_was_missing = stored.selected.is_none();
            let original = JiraSiteFile {
                version: stored.version,
                sites: stored.sites.clone(),
                active_site_id: stored.active_site_id.clone(),
                selected: stored.selected.clone().unwrap_or_default(),
            };
            let canonical = normalize_file(stored.sites, stored.active_site_id, stored.selected)?;
            let should_persist = canonical != original || selection_was_missing;
            Ok((canonical, should_persist))
        }
        Some(stored) => Err(JiraStoreError::UnsupportedVersion(stored.version)),
    }
}

fn normalize_file(
    mut sites: Vec<JiraSite>,
    active_site_id: Option<String>,
    selected: Option<JiraSelection>,
) -> StoreResult<JiraSiteFile> {
    let mut ids = HashSet::new();
    for site in &mut sites {
        normalize_site(site)?;
        if !ids.insert(site.id.clone()) {
            return Err(invalid_metadata(format!(
                "중복된 Jira 사이트 ID가 있습니다: {}",
                site.id
            )));
        }
    }

    let first = sites.first().map(|site| site.id.clone());
    let valid_active = active_site_id.filter(|id| sites.iter().any(|site| site.id == *id));
    let (active_site_id, selected) = match selected {
        Some(JiraSelection::Site(id)) if sites.iter().any(|site| site.id == id) => {
            (Some(id.clone()), JiraSelection::Site(id))
        }
        Some(JiraSelection::All) => (valid_active.or(first), JiraSelection::All),
        _ => {
            let active = valid_active.or(first);
            let selected = active
                .clone()
                .map(JiraSelection::Site)
                .unwrap_or(JiraSelection::All);
            (active, selected)
        }
    };

    Ok(JiraSiteFile {
        version: CURRENT_FILE_VERSION,
        sites,
        active_site_id,
        selected,
    })
}

fn normalize_site(site: &mut JiraSite) -> StoreResult<()> {
    validate_site_id(&site.id)?;
    site.site_url = https_origin(&site.site_url)?;
    site.email = site.email.trim().to_string();
    site.display_name = site.display_name.trim().to_string();
    site.account_id = site.account_id.trim().to_string();
    site.auth_type = match site.auth_type.trim().to_ascii_lowercase().as_str() {
        "cloud" => "cloud".to_string(),
        "server" => "server".to_string(),
        other => {
            return Err(invalid_metadata(format!(
                "지원하지 않는 Jira 인증 유형입니다: {other}"
            )));
        }
    };
    Ok(())
}

/// Validate and canonicalize a Jira origin before any credential-bearing
/// network request as well as before metadata is committed.
pub fn https_origin(raw: &str) -> StoreResult<String> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| invalid_metadata(HTTPS_ERROR))?;
    if url.scheme() != "https"
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_metadata(HTTPS_ERROR));
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}

fn validate_site_id(id: &str) -> StoreResult<()> {
    let valid = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(invalid_metadata("유효하지 않은 Jira 사이트 ID입니다"))
    }
}

fn pending_connect_journal_id(name: &str) -> Option<&str> {
    name.strip_prefix(PENDING_CONNECT_PREFIX)?
        .strip_suffix(PENDING_CONNECT_SUFFIX)
        .filter(|id| !id.is_empty())
}

fn pending_delete_journal_id(name: &str) -> Option<&str> {
    name.strip_prefix(PENDING_DELETE_PREFIX)?
        .strip_suffix(PENDING_DELETE_SUFFIX)
        .filter(|id| !id.is_empty())
}

fn credential_error(id: &str, error: SecretStoreError) -> JiraStoreError {
    match error.kind() {
        SecretStoreErrorKind::NotFound => JiraStoreError::CredentialUnavailable(id.to_string()),
        SecretStoreErrorKind::AccessDenied => {
            JiraStoreError::CredentialAccessDenied(id.to_string())
        }
        SecretStoreErrorKind::Unavailable => {
            JiraStoreError::CredentialStoreUnavailable(id.to_string())
        }
    }
}

fn remove_file_if_present(path: &Path) -> StoreResult<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(io_error("Jira 복구 파일을 정리하지 못했습니다", source)),
    }
}

fn invalid_metadata(reason: impl Into<String>) -> JiraStoreError {
    JiraStoreError::InvalidMetadata(reason.into())
}

fn io_error(action: &'static str, source: io::Error) -> JiraStoreError {
    JiraStoreError::Io { action, source }
}

#[cfg(test)]
fn ensure_private_directory(path: &Path) -> StoreResult<()> {
    fs::create_dir_all(path)
        .map_err(|source| io_error("Jira 토큰 폴더를 만들지 못했습니다", source))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|source| io_error("Jira 토큰 폴더 권한을 설정하지 못했습니다", source))?;
    }
    Ok(())
}

#[cfg(unix)]
fn lock_file_exclusively(file: &File) -> StoreResult<()> {
    use std::os::fd::AsRawFd;

    loop {
        // SAFETY: `file` owns a valid descriptor for the duration of this
        // call and the returned JiraStoreLock keeps it open while locked.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(());
        }
        let source = io::Error::last_os_error();
        if source.kind() != io::ErrorKind::Interrupted {
            return Err(io_error("Jira 설정 잠금을 얻지 못했습니다", source));
        }
    }
}

#[cfg(windows)]
fn lock_file_exclusively(file: &File) -> StoreResult<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::LockFile;

    // `LockFile` is non-blocking.  ERROR_LOCK_VIOLATION means another process
    // still owns the byte; yielding avoids monopolizing this thread while
    // retaining the same acquire-until-available contract as Unix `flock`.
    const ERROR_LOCK_VIOLATION: i32 = 33;
    loop {
        // SAFETY: the raw handle remains owned by `file`, and locking one byte
        // beyond EOF is supported by LockFile for a stable sentinel file.
        let locked = unsafe { LockFile(file.as_raw_handle() as _, 0, 0, 1, 0) };
        if locked != 0 {
            return Ok(());
        }
        let source = io::Error::last_os_error();
        if source.raw_os_error() != Some(ERROR_LOCK_VIOLATION) {
            return Err(io_error("Jira 설정 잠금을 얻지 못했습니다", source));
        }
        std::thread::yield_now();
    }
}

#[cfg(not(any(unix, windows)))]
fn lock_file_exclusively(_file: &File) -> StoreResult<()> {
    Err(invalid_metadata(
        "이 플랫폼에서는 Jira 설정 잠금을 지원하지 않습니다",
    ))
}

fn atomic_write(path: &Path, bytes: &[u8], unix_mode: u32) -> StoreResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid_metadata("Jira 설정 경로에 상위 폴더가 없습니다"))?;
    crate::durable_file::ensure_private_directory(parent)
        .map_err(|source| io_error("Jira 설정 폴더를 만들지 못했습니다", source))?;

    let temporary = unique_sibling(path, "tmp")?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(unix_mode);
    }
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|source| io_error("임시 Jira 설정 파일을 만들지 못했습니다", source))?;
        file.write_all(bytes)
            .map_err(|source| io_error("임시 Jira 설정 파일을 쓰지 못했습니다", source))?;
        file.sync_all()
            .map_err(|source| io_error("임시 Jira 설정 파일을 동기화하지 못했습니다", source))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(unix_mode))
                .map_err(|source| io_error("Jira 설정 파일 권한을 설정하지 못했습니다", source))?;
        }
        let outcome = replace_file(&temporary, path)?;
        if let crate::durable_file::Durability::VisibleButSyncFailed(error) = outcome.durability {
            // The replacement is committed at this point. Reporting this as
            // a failed commit could make a paired-file transaction roll back
            // while this value is already visible.
            eprintln!(
                "zerocode-shell: Jira replacement at {} is visible but directory sync failed: {error}",
                path.display()
            );
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn replace_file(
    temporary: &Path,
    destination: &Path,
) -> StoreResult<crate::durable_file::CommitOutcome> {
    crate::durable_file::replace_staged(temporary, destination)
        .map_err(|source| io_error("Jira 설정을 원자적으로 교체하지 못했습니다", source))
}

fn unique_sibling(path: &Path, label: &str) -> StoreResult<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid_metadata("Jira 설정 경로에 상위 폴더가 없습니다"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid_metadata("Jira 설정 파일 이름이 유효하지 않습니다"))?;
    for _ in 0..32 {
        let sequence = TEMP_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.{label}.{}.{}",
            std::process::id(),
            sequence
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(invalid_metadata(
        "임시 Jira 설정 파일 이름을 만들지 못했습니다",
    ))
}

#[cfg(unix)]
fn sync_directory(path: impl AsRef<Path>) -> StoreResult<()> {
    File::open(path.as_ref())
        .and_then(|directory| directory.sync_all())
        .map_err(|source| io_error("Jira 설정 폴더를 동기화하지 못했습니다", source))
}

#[cfg(not(unix))]
fn sync_directory(_path: impl AsRef<Path>) -> StoreResult<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_store::{MemorySecretStore, SecretOperation, UnavailableSecretStore};
    use std::sync::{Arc, Barrier};

    fn site(id: &str) -> JiraSite {
        JiraSite {
            id: id.to_string(),
            site_url: format!("https://{id}.example.com/"),
            email: format!("{id}@example.com"),
            display_name: id.to_string(),
            account_id: format!("account-{id}"),
            auth_type: "cloud".to_string(),
        }
    }

    fn connected_pair() -> (tempfile::TempDir, JiraStore) {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        store
            .connect_commit(site("A"), "token-a")
            .expect("connect A");
        store
            .connect_commit(site("B"), "token-b")
            .expect("connect B");
        (directory, store)
    }

    #[test]
    fn credentials_round_trip_without_whitespace_normalization() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        let password = " pass ";

        store
            .connect_commit(site("A"), password)
            .expect("connect with a whitespace-sensitive password");

        assert_eq!(store.read_token("A").expect("stored password"), password);
        assert!(!store.token_file("A").exists());
        assert!(!store.pending_connect_token_file("A").exists());
    }

    #[test]
    fn legacy_plaintext_is_removed_only_after_exact_native_migration() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        let password = b" pass ";
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("legacy metadata");
        ensure_private_directory(&store.token_directory()).expect("legacy token directory");
        fs::write(store.token_file("A"), password).expect("legacy plaintext token");

        let status = store.status().expect("migrated status");

        assert_eq!(
            credentials.secret("site:A").as_deref(),
            Some(password.as_slice())
        );
        assert!(!store.token_file("A").exists());
        assert_eq!(
            status.credential_protection,
            JiraCredentialProtection::Native
        );
    }

    #[test]
    fn orphan_plaintext_without_a_site_blocks_platform_path_activation() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials);
        ensure_private_directory(&store.token_directory()).expect("legacy token directory");
        let orphan = store.token_directory().join("orphan-site");
        fs::write(&orphan, b"unregistered plaintext").expect("orphan token");

        let error = store
            .prepare_path_migration()
            .expect_err("orphan plaintext must fail closed");

        assert!(matches!(
            error,
            JiraStoreError::CredentialStoreUnavailable(ref id)
                if id == "legacy-path-migration"
        ));
        assert_eq!(fs::read(orphan).unwrap(), b"unregistered plaintext");
    }

    #[cfg(unix)]
    #[test]
    fn jira_root_lock_and_metadata_are_owner_only() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = tempfile::tempdir().expect("temporary Jira parent");
        let root = directory.path().join("jira-state");
        let store = JiraStore::new(&root);
        store.load().expect("initialize metadata");
        store
            .connect_commit(site("A"), "native-secret")
            .expect("persist metadata");

        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for file in [root.join(LOCK_FILE_NAME), root.join(SITE_FILE_NAME)] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn jira_lock_symlink_never_touches_the_external_file() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = tempfile::tempdir().expect("temporary Jira parent");
        let root = directory.path().join("jira-state");
        fs::create_dir(&root).expect("Jira root");
        let external = directory.path().join("external-lock");
        fs::write(&external, b"external lock").expect("external lock");
        fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).expect("external mode");
        symlink(&external, root.join(LOCK_FILE_NAME)).expect("lock symlink");

        let error = JiraStore::new(&root)
            .load()
            .expect_err("the lock leaf must fail closed");

        match error {
            JiraStoreError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(
            fs::read(&external).expect("external remains"),
            b"external lock"
        );
        assert_eq!(
            fs::metadata(&external)
                .expect("external metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn registered_legacy_token_symlink_never_reaches_the_vault() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("legacy metadata");
        ensure_private_directory(&store.token_directory()).expect("legacy token directory");
        let external = directory.path().join("outside-token");
        fs::write(&external, b"external secret").expect("external token");
        fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).expect("external mode");
        let token = store.token_file("A");
        symlink(&external, &token).expect("registered token symlink");

        let read_error = store
            .read_token("A")
            .expect_err("a registered token symlink must fail closed");
        match read_error {
            JiraStoreError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert!(matches!(
            store.prepare_path_migration(),
            Err(JiraStoreError::CredentialStoreUnavailable(ref id))
                if id == "legacy-path-migration"
        ));
        assert!(credentials.secret("site:A").is_none());
        assert!(
            fs::symlink_metadata(&token)
                .expect("token link remains")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&external).expect("external remains"),
            b"external secret"
        );
        assert_eq!(
            fs::metadata(&external)
                .expect("external metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_token_directory_symlink_never_reaches_the_vault() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("legacy metadata");
        let outside = tempfile::tempdir().expect("outside token directory");
        let external = outside.path().join("A");
        fs::write(&external, b"external secret").expect("external token");
        fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).expect("external mode");
        symlink(outside.path(), store.token_directory()).expect("token directory symlink");

        let error = store
            .read_token("A")
            .expect_err("a token-directory symlink must fail closed");

        match error {
            JiraStoreError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidInput);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert!(credentials.secret("site:A").is_none());
        assert!(
            fs::symlink_metadata(store.token_directory())
                .expect("token directory link remains")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&external).expect("external remains"),
            b"external secret"
        );
        assert_eq!(
            fs::metadata(&external)
                .expect("external metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }

    #[cfg(unix)]
    #[test]
    fn pending_delete_recovery_preserves_plaintext_when_canonical_is_a_symlink() {
        use std::os::unix::fs::{PermissionsExt as _, symlink};

        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("live metadata");
        ensure_private_directory(&store.token_directory()).expect("legacy token directory");
        let pending = store.pending_delete_file("A");
        fs::write(&pending, b"pending secret").expect("pending plaintext");
        let external = directory.path().join("outside-token");
        fs::write(&external, b"external secret").expect("external token");
        fs::set_permissions(&external, fs::Permissions::from_mode(0o640)).expect("external mode");
        let canonical = store.token_file("A");
        symlink(&external, &canonical).expect("canonical token symlink");

        let error = store
            .load()
            .expect_err("recovery must validate canonical before mutation");

        match error {
            JiraStoreError::Io { source, .. } => {
                assert_eq!(source.kind(), io::ErrorKind::InvalidData);
            }
            other => panic!("unexpected error: {other}"),
        }
        assert_eq!(
            fs::read(&pending).expect("pending remains"),
            b"pending secret"
        );
        assert!(
            fs::symlink_metadata(&canonical)
                .expect("canonical link remains")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(&external).expect("external remains"),
            b"external secret"
        );
        assert_eq!(
            fs::metadata(&external)
                .expect("external metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert!(credentials.secret("site:A").is_none());
    }

    #[test]
    fn denied_migration_preserves_plaintext_and_reports_actual_protection() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        credentials.fail(SecretOperation::Write, SecretStoreErrorKind::AccessDenied);
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        let secret = b"do-not-leak-this-value";
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("legacy metadata");
        ensure_private_directory(&store.token_directory()).expect("legacy directory");
        fs::write(store.token_file("A"), secret).expect("legacy token");

        let status = store.status().expect("truthful degraded status");

        assert_eq!(
            status.credential_protection,
            JiraCredentialProtection::Plaintext
        );
        assert_eq!(
            status.sites[0].credential,
            JiraCredentialStanding::Unreadable
        );
        assert_eq!(fs::read(store.token_file("A")).unwrap(), secret);
        let error = store.read_token("A").expect_err("denied migration read");
        assert!(matches!(error, JiraStoreError::CredentialAccessDenied(ref id) if id == "A"));
        assert!(!error.to_string().contains("do-not-leak-this-value"));

        credentials.allow(SecretOperation::Write);
        assert_eq!(
            store.read_token("A").expect("retried migration"),
            "do-not-leak-this-value"
        );
        assert!(!store.token_file("A").exists());
    }

    #[test]
    fn failed_native_readback_never_deletes_the_migration_source() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        credentials.fail(SecretOperation::Read, SecretStoreErrorKind::AccessDenied);
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("legacy metadata");
        ensure_private_directory(&store.token_directory()).expect("legacy directory");
        fs::write(store.token_file("A"), b"exact bytes").expect("legacy token");

        let status = store.status().expect("degraded status");
        assert_eq!(
            status.credential_protection,
            JiraCredentialProtection::Plaintext
        );
        assert_eq!(fs::read(store.token_file("A")).unwrap(), b"exact bytes");

        credentials.allow(SecretOperation::Read);
        assert_eq!(
            store.read_token("A").expect("migration retry"),
            "exact bytes"
        );
        assert!(!store.token_file("A").exists());
    }

    #[test]
    fn mismatched_native_readback_never_deletes_the_migration_source() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        credentials.replace_reads_with(Some(b"different bytes".to_vec()));
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("legacy metadata");
        ensure_private_directory(&store.token_directory()).expect("legacy directory");
        fs::write(store.token_file("A"), b"canonical bytes").expect("legacy token");

        let status = store.status().expect("degraded status");

        assert_eq!(
            status.credential_protection,
            JiraCredentialProtection::Plaintext
        );
        assert_eq!(
            status.sites[0].credential,
            JiraCredentialStanding::Unreadable
        );
        assert_eq!(fs::read(store.token_file("A")).unwrap(), b"canonical bytes");
        credentials.replace_reads_with(None);
        assert_eq!(
            store.read_token("A").expect("migration retry"),
            "canonical bytes"
        );
        assert!(!store.token_file("A").exists());
    }

    #[test]
    fn reconnect_cannot_replace_a_secret_while_legacy_migration_is_blocked() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .connect_commit(site("A"), "old-native")
            .expect("initial connect");
        ensure_private_directory(&store.token_directory()).expect("legacy directory");
        fs::write(store.token_file("A"), b"old-plaintext").expect("legacy source");
        credentials.fail(SecretOperation::Write, SecretStoreErrorKind::AccessDenied);
        let mut updated = site("A");
        updated.display_name = "updated A".to_string();

        let error = store
            .connect_commit(updated.clone(), "new-native")
            .expect_err("reconnect bypassed failed migration");

        assert!(matches!(error, JiraStoreError::CredentialAccessDenied(ref id) if id == "A"));
        assert_eq!(
            credentials.secret("site:A").as_deref(),
            Some(b"old-native".as_slice())
        );
        assert_eq!(fs::read(store.token_file("A")).unwrap(), b"old-plaintext");
        assert!(!store.pending_connect_journal_file("A").exists());

        credentials.allow(SecretOperation::Write);
        store
            .connect_commit(updated, "new-native")
            .expect("retry reconnect");
        assert_eq!(store.read_token("A").expect("replacement"), "new-native");
        assert!(!store.token_file("A").exists());
    }

    #[test]
    fn native_not_found_and_unavailable_are_distinct() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        store
            .write_site_file(&normalize_file(vec![site("A")], None, None).unwrap())
            .expect("metadata");
        let status = store.status().expect("missing status");
        assert_eq!(status.sites[0].credential, JiraCredentialStanding::Missing);
        assert!(matches!(
            store.read_token("A"),
            Err(JiraStoreError::CredentialUnavailable(id)) if id == "A"
        ));

        let unavailable =
            JiraStore::with_secret_store(directory.path(), Arc::new(UnavailableSecretStore));
        let status = unavailable.status().expect("unavailable status");
        assert_eq!(
            status.credential_protection,
            JiraCredentialProtection::Unavailable
        );
        assert_eq!(
            status.sites[0].credential,
            JiraCredentialStanding::Unreadable
        );
        assert!(matches!(
            unavailable.read_token("A"),
            Err(JiraStoreError::CredentialStoreUnavailable(id)) if id == "A"
        ));
    }

    #[test]
    fn disconnect_delete_denial_is_recoverable_without_losing_the_secret() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .connect_commit(site("A"), "token-a")
            .expect("connect A");
        credentials.fail(SecretOperation::Delete, SecretStoreErrorKind::AccessDenied);

        let error = store.disconnect("A").expect_err("native delete denial");

        assert!(matches!(error, JiraStoreError::CredentialAccessDenied(id) if id == "A"));
        assert_eq!(
            credentials.secret("site:A").as_deref(),
            Some(b"token-a".as_slice())
        );
        assert!(store.pending_delete_journal_file("A").exists());
        credentials.allow(SecretOperation::Delete);
        let recovered = store.load().expect("retry deletion on restart");
        assert!(recovered.sites.is_empty());
        assert!(credentials.secret("site:A").is_none());
        assert!(!store.pending_delete_journal_file("A").exists());
    }

    #[test]
    fn native_probe_denial_is_unreadable_without_reading_the_secret() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let credentials = Arc::new(MemorySecretStore::default());
        let store = JiraStore::with_secret_store(directory.path(), credentials.clone());
        store
            .connect_commit(site("A"), "opaque-secret")
            .expect("connect A");
        credentials.fail(SecretOperation::Probe, SecretStoreErrorKind::AccessDenied);

        let status = store.status().expect("probe-denied status");

        assert_eq!(
            status.sites[0].credential,
            JiraCredentialStanding::Unreadable
        );
        let json = serde_json::to_string(&status).expect("safe status JSON");
        assert!(!json.contains("opaque-secret"));
    }

    #[test]
    fn concurrent_disjoint_connects_preserve_both_sites_and_tokens() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        let start = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for (id, token) in [("A", "token-a"), ("B", "token-b")] {
            let store = store.clone();
            let start = Arc::clone(&start);
            workers.push(std::thread::spawn(move || {
                start.wait();
                store
                    .connect_commit(site(id), token)
                    .expect("concurrent connect");
            }));
        }

        start.wait();
        for worker in workers {
            worker.join().expect("connect worker");
        }

        let file = store.load().expect("canonical sites");
        let ids: HashSet<_> = file.sites.iter().map(|site| site.id.as_str()).collect();
        assert_eq!(ids, HashSet::from(["A", "B"]));
        assert_eq!(store.read_token("A").expect("A token"), "token-a");
        assert_eq!(store.read_token("B").expect("B token"), "token-b");
    }

    #[test]
    fn interrupted_connect_before_metadata_keeps_the_previous_token_and_site() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        store
            .connect_commit(site("A"), "old-token")
            .expect("initial connect");
        let mut updated = site("A");
        updated.display_name = "updated A".to_string();

        store
            .connect_commit_with_behavior(
                updated,
                "new-token",
                ConnectCommitBehavior::InterruptBeforeMetadata,
            )
            .expect_err("simulated crash before metadata");

        assert_eq!(
            store.credentials.read(&site_account("A")).unwrap(),
            b"old-token"
        );
        assert_eq!(
            store.credentials.read(&pending_account("A")).unwrap(),
            b"new-token"
        );
        assert!(!store.pending_connect_token_file("A").exists());
        assert!(store.pending_connect_journal_file("A").exists());

        let restarted = store.clone();
        let recovered = restarted.load().expect("abandon uncommitted connect");
        assert_eq!(recovered.sites[0].display_name, "A");
        assert_eq!(restarted.read_token("A").expect("old token"), "old-token");
        assert_eq!(
            restarted.credentials.standing(&pending_account("A")),
            SecretStanding::Missing
        );
        assert!(!restarted.pending_connect_journal_file("A").exists());
    }

    #[test]
    fn interrupted_connect_after_metadata_promotes_the_exact_staged_token() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        store
            .connect_commit(site("A"), "old-token")
            .expect("initial connect");
        let mut updated = site("A");
        updated.display_name = "updated A".to_string();
        let replacement = " new-token ";

        store
            .connect_commit_with_behavior(
                updated,
                replacement,
                ConnectCommitBehavior::InterruptAfterMetadata,
            )
            .expect_err("simulated crash after metadata");

        assert_eq!(
            store.credentials.read(&site_account("A")).unwrap(),
            b"old-token"
        );
        assert_eq!(
            store.credentials.read(&pending_account("A")).unwrap(),
            replacement.as_bytes()
        );
        assert!(!store.pending_connect_token_file("A").exists());
        assert!(store.pending_connect_journal_file("A").exists());
        let journal = fs::read_to_string(store.pending_connect_journal_file("A"))
            .expect("non-secret connect journal");
        assert!(!journal.contains(replacement), "journal leaked the token");

        let restarted = store.clone();
        let recovered = restarted.load().expect("finish committed connect");
        assert_eq!(recovered.sites[0].display_name, "updated A");
        assert_eq!(
            restarted.read_token("A").expect("promoted token"),
            replacement
        );
        assert_eq!(
            restarted.credentials.standing(&pending_account("A")),
            SecretStanding::Missing
        );
        assert!(!restarted.pending_connect_journal_file("A").exists());
    }

    #[test]
    fn concurrent_select_and_disconnect_leave_canonical_metadata_and_tokens() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let initial = JiraStore::new(directory.path());
        for (id, token) in [("A", "token-a"), ("B", "token-b"), ("C", "token-c")] {
            initial
                .connect_commit(site(id), token)
                .expect("initial connect");
        }
        initial
            .select(JiraSelection::Site("A".to_string()))
            .expect("select A");

        let start = Arc::new(Barrier::new(3));
        let selecting = {
            let store = initial.clone();
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                store
                    .select(JiraSelection::Site("B".to_string()))
                    .expect("select B");
            })
        };
        let disconnecting = {
            let store = initial.clone();
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                store.disconnect("A").expect("disconnect A");
            })
        };

        start.wait();
        selecting.join().expect("select worker");
        disconnecting.join().expect("disconnect worker");

        let store = initial;
        let file = store.load().expect("canonical metadata");
        let ids: HashSet<_> = file.sites.iter().map(|site| site.id.as_str()).collect();
        assert_eq!(ids, HashSet::from(["B", "C"]));
        assert_eq!(file.active_site_id.as_deref(), Some("B"));
        assert_eq!(file.selected, JiraSelection::Site("B".to_string()));
        assert!(matches!(
            store.read_token("A"),
            Err(JiraStoreError::CredentialUnavailable(id)) if id == "A"
        ));
        assert_eq!(store.read_token("B").expect("B token"), "token-b");
        assert_eq!(store.read_token("C").expect("C token"), "token-c");
        assert!(!store.pending_delete_file("A").exists());
        assert!(!store.pending_delete_journal_file("A").exists());
    }

    #[test]
    fn metadata_read_does_not_reconcile_or_recover() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        let legacy = serde_json::json!({
            "version": LEGACY_FILE_VERSION,
            "sites": [site("A")]
        });
        let original = serde_json::to_vec(&legacy).expect("legacy JSON");
        fs::write(store.site_file(), &original).expect("legacy metadata");
        store
            .write_pending_delete_journal("A")
            .expect("pending deletion");

        let _lock = store.lock().expect("Jira lock");
        let stored = store
            .read_metadata_locked()
            .expect("read metadata")
            .expect("stored metadata");

        assert_eq!(stored.version, LEGACY_FILE_VERSION);
        assert!(stored.active_site_id.is_none());
        assert!(stored.selected.is_none());
        assert_eq!(fs::read(store.site_file()).unwrap(), original);
        assert!(store.pending_delete_journal_file("A").exists());
    }

    #[test]
    fn version_one_migrates_to_an_active_selected_site() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let legacy = serde_json::json!({
            "version": LEGACY_FILE_VERSION,
            "sites": [site("A"), site("B")]
        });
        fs::write(
            directory.path().join(SITE_FILE_NAME),
            serde_json::to_vec(&legacy).expect("legacy JSON"),
        )
        .expect("legacy file");

        let store = JiraStore::new(directory.path());
        let loaded = store.load().expect("migrated file");
        assert_eq!(loaded.version, CURRENT_FILE_VERSION);
        assert_eq!(loaded.active_site_id.as_deref(), Some("A"));
        assert_eq!(loaded.selected, JiraSelection::Site("A".to_string()));

        let persisted: serde_json::Value = serde_json::from_slice(
            &fs::read(directory.path().join(SITE_FILE_NAME)).expect("persisted migration"),
        )
        .expect("canonical JSON");
        assert_eq!(persisted["version"], CURRENT_FILE_VERSION);
        assert_eq!(persisted["active_site_id"], "A");
        assert_eq!(persisted["selected"]["kind"], "site");
    }

    #[test]
    fn selecting_b_resolves_only_b() {
        let (_directory, store) = connected_pair();
        let status = store
            .select(JiraSelection::Site("B".to_string()))
            .expect("select B");
        assert_eq!(status.active_site_id.as_deref(), Some("B"));
        assert_eq!(status.selected, JiraSelection::Site("B".to_string()));
        let resolved = store.resolve_selection().expect("resolved B");
        assert_eq!(resolved.sites, vec![site("B").normalized_for_test()]);
    }

    #[test]
    fn selecting_all_keeps_the_active_site() {
        let (_directory, store) = connected_pair();
        store
            .select(JiraSelection::Site("B".to_string()))
            .expect("select B");
        let status = store.select(JiraSelection::All).expect("select all");
        assert_eq!(status.active_site_id.as_deref(), Some("B"));
        assert_eq!(status.selected, JiraSelection::All);
        let resolved = store.resolve_selection().expect("resolved all");
        assert_eq!(resolved.active_site_id.as_deref(), Some("B"));
        assert_eq!(resolved.sites.len(), 2);
    }

    #[test]
    fn stale_v2_selection_falls_back_to_the_first_live_site() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let stale = serde_json::json!({
            "version": CURRENT_FILE_VERSION,
            "sites": [site("A"), site("B")],
            "active_site_id": "removed-active",
            "selected": {"kind": "site", "site_id": "removed-selected"}
        });
        fs::write(
            directory.path().join(SITE_FILE_NAME),
            serde_json::to_vec(&stale).expect("stale JSON"),
        )
        .expect("stale file");

        let loaded = JiraStore::new(directory.path())
            .load()
            .expect("normalized fallback");
        assert_eq!(loaded.active_site_id.as_deref(), Some("A"));
        assert_eq!(loaded.selected, JiraSelection::Site("A".to_string()));
    }

    #[test]
    fn a_missing_a_token_does_not_block_selected_b() {
        let (_directory, store) = connected_pair();
        store
            .select(JiraSelection::Site("B".to_string()))
            .expect("select B");
        store
            .credentials
            .delete(&site_account("A"))
            .expect("remove only A token");

        let status = store.status().expect("isolated status");
        assert!(status.connected, "selected B remains usable");
        assert_eq!(
            status
                .sites
                .iter()
                .find(|site| site.site.id == "A")
                .map(|site| site.credential),
            Some(JiraCredentialStanding::Missing)
        );
        assert_eq!(store.read_token("B").expect("B token"), "token-b");
        assert_eq!(
            store.resolve_selection().expect("B selection").sites[0].id,
            "B"
        );
    }

    #[test]
    fn disconnect_removes_only_a_and_falls_back_to_b() {
        let (_directory, store) = connected_pair();
        store
            .select(JiraSelection::Site("A".to_string()))
            .expect("select A");
        let status = store.disconnect("A").expect("disconnect A");

        assert_eq!(status.active_site_id.as_deref(), Some("B"));
        assert_eq!(status.selected, JiraSelection::Site("B".to_string()));
        assert_eq!(status.sites.len(), 1);
        assert_eq!(status.sites[0].site.id, "B");
        assert_eq!(
            store.credentials.standing(&site_account("A")),
            SecretStanding::Missing
        );
        assert_eq!(store.read_token("B").expect("B remains"), "token-b");
    }

    #[test]
    fn interrupted_delete_before_metadata_commit_restores_the_token() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        store
            .connect_commit(site("A"), "token-a")
            .expect("connect A");
        store
            .write_pending_delete_journal("A")
            .expect("simulate a crash before metadata commit");

        store.load().expect("recover pending delete");
        assert_eq!(store.read_token("A").expect("restored A"), "token-a");
        assert!(!store.pending_delete_journal_file("A").exists());
    }

    #[test]
    fn interrupted_delete_after_metadata_commit_cleans_the_marker() {
        let (_directory, store) = connected_pair();
        let mut file = store.load().expect("metadata before simulation");
        store
            .write_pending_delete_journal("A")
            .expect("stage A deletion");
        file.sites.retain(|site| site.id != "A");
        store.write_site_file(&file).expect("commit metadata");

        store.load().expect("finish pending delete");
        assert_eq!(
            store.credentials.standing(&site_account("A")),
            SecretStanding::Missing
        );
        assert!(!store.pending_delete_journal_file("A").exists());
        assert_eq!(store.read_token("B").expect("B remains"), "token-b");
    }

    #[test]
    fn serialized_status_contains_protection_but_no_secret() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        let secret = "never-cross-the-ipc-boundary";
        let status = store
            .connect_commit(site("A"), secret)
            .expect("connected status");
        let json = serde_json::to_string(&status).expect("status JSON");
        assert!(!json.contains(secret), "status leaked its token: {json}");
        assert!(json.contains("native"), "protection was implicit: {json}");
    }

    #[test]
    fn insecure_metadata_is_rejected_before_a_token_is_read() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let legacy = serde_json::json!({
            "version": LEGACY_FILE_VERSION,
            "sites": [{
                "id": "A",
                "site_url": "http://jira.example.com",
                "email": "a@example.com",
                "display_name": "A",
                "account_id": "account-A",
                "auth_type": "cloud"
            }]
        });
        fs::write(
            directory.path().join(SITE_FILE_NAME),
            serde_json::to_vec(&legacy).expect("legacy JSON"),
        )
        .expect("legacy file");
        let store = JiraStore::new(directory.path());
        assert_eq!(
            store
                .load()
                .expect_err("HTTP metadata accepted")
                .to_string(),
            HTTPS_ERROR
        );
    }

    #[test]
    fn native_connect_never_writes_plaintext() {
        let directory = tempfile::tempdir().expect("temporary Jira state");
        let store = JiraStore::new(directory.path());
        let secret = "must-not-touch-disk";

        store
            .connect_commit(site("A"), secret)
            .expect("native connect");

        assert!(!store.token_directory().exists());
        let journal = fs::read_to_string(store.site_file()).expect("metadata JSON");
        assert!(!journal.contains(secret));
    }

    trait NormalizeForTest {
        fn normalized_for_test(self) -> Self;
    }

    impl NormalizeForTest for JiraSite {
        fn normalized_for_test(mut self) -> Self {
            normalize_site(&mut self).expect("test site is valid");
            self
        }
    }
}
