//! Native secret persistence behind a small, injectable boundary.
//!
//! Production selects exactly one OS-native provider. Tests use the in-memory
//! implementation below, so a test run can never create a real Keychain or
//! Credential Manager entry. Product domains own their service and account
//! names; this module only maps those names to the selected provider.

use std::fmt;
use std::sync::Arc;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretProtection {
    Native,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStanding {
    Available,
    Missing,
    Unreadable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretStoreErrorKind {
    NotFound,
    AccessDenied,
    Unavailable,
}

/// A deliberately secret-free error. Provider messages can contain store
/// metadata, so none of them cross this boundary or reach renderer IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretStoreError {
    kind: SecretStoreErrorKind,
}

impl SecretStoreError {
    pub const fn new(kind: SecretStoreErrorKind) -> Self {
        Self { kind }
    }

    pub const fn kind(self) -> SecretStoreErrorKind {
        self.kind
    }
}

impl fmt::Display for SecretStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self.kind {
            SecretStoreErrorKind::NotFound => "native credential not found",
            SecretStoreErrorKind::AccessDenied => "native credential access denied",
            SecretStoreErrorKind::Unavailable => "native credential store unavailable",
        })
    }
}

impl std::error::Error for SecretStoreError {}

pub type SecretResult<T> = Result<T, SecretStoreError>;

pub trait SecretStore: fmt::Debug + Send + Sync {
    fn protection(&self) -> SecretProtection;
    fn standing(&self, account: &str) -> SecretStanding;
    fn read(&self, account: &str) -> SecretResult<Vec<u8>>;
    fn write(&self, account: &str, secret: &[u8]) -> SecretResult<()>;
    /// Deletion is idempotent: a missing entry is already deleted.
    fn delete(&self, account: &str) -> SecretResult<()>;
}

/// Verified UTF-8 secrets over the selected native provider.
///
/// Product domains still own account names and user-facing errors. This small
/// boundary owns the security mechanics they must not each reimplement:
/// empty-value rejection, exact readback, rollback, UTF-8 validation, and
/// zeroizing temporary buffers.
#[derive(Clone)]
pub struct VerifiedSecretStore {
    store: Arc<dyn SecretStore>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedSecretError {
    Missing,
    Unavailable,
    InvalidEncoding,
    Empty,
    ReadbackMismatch,
}

impl VerifiedSecretStore {
    pub fn new(_service: &'static str) -> Self {
        #[cfg(not(test))]
        let store = native_secret_store(_service);
        #[cfg(test)]
        let store = Arc::new(MemorySecretStore::default());
        Self { store }
    }

    #[cfg(any(test, feature = "test-memory"))]
    pub fn with_store(store: Arc<dyn SecretStore>) -> Self {
        Self { store }
    }

    pub fn standing(&self, account: &str) -> SecretStanding {
        self.store.standing(account)
    }

    pub fn read(&self, account: &str) -> Result<Zeroizing<String>, VerifiedSecretError> {
        let bytes = Zeroizing::new(
            self.store
                .read(account)
                .map_err(|error| VerifiedSecretError::from(error.kind()))?,
        );
        let secret = std::str::from_utf8(bytes.as_slice())
            .map_err(|_| VerifiedSecretError::InvalidEncoding)?;
        if secret.is_empty() {
            return Err(VerifiedSecretError::Empty);
        }
        Ok(Zeroizing::new(secret.to_owned()))
    }

    pub fn write(
        &self,
        account: &str,
        secret: Zeroizing<String>,
    ) -> Result<(), VerifiedSecretError> {
        if secret.is_empty() {
            return Err(VerifiedSecretError::Empty);
        }
        let previous = match self.store.read(account) {
            Ok(secret) => Some(Zeroizing::new(secret)),
            Err(error) if error.kind() == SecretStoreErrorKind::NotFound => None,
            Err(_) => return Err(VerifiedSecretError::Unavailable),
        };
        self.store
            .write(account, secret.as_bytes())
            .map_err(|_| VerifiedSecretError::Unavailable)?;
        let readback = self.store.read(account).map(Zeroizing::new);
        if matches!(readback.as_ref(), Ok(readback) if readback.as_slice() == secret.as_bytes()) {
            return Ok(());
        }
        restore_secret(
            self.store.as_ref(),
            account,
            previous.as_ref().map(|previous| previous.as_slice()),
        );
        Err(VerifiedSecretError::ReadbackMismatch)
    }

    pub fn delete(&self, account: &str) -> Result<(), VerifiedSecretError> {
        self.store
            .delete(account)
            .map_err(|_| VerifiedSecretError::Unavailable)
    }
}

impl From<SecretStoreErrorKind> for VerifiedSecretError {
    fn from(kind: SecretStoreErrorKind) -> Self {
        match kind {
            SecretStoreErrorKind::NotFound => Self::Missing,
            SecretStoreErrorKind::AccessDenied | SecretStoreErrorKind::Unavailable => {
                Self::Unavailable
            }
        }
    }
}

fn restore_secret(store: &dyn SecretStore, account: &str, previous: Option<&[u8]>) {
    match previous {
        Some(secret) => {
            let _ = store.write(account, secret);
        }
        None => {
            let _ = store.delete(account);
        }
    }
}

/// Every service this app owns is a constant.
///
/// There WAS a second door here for a name computed at run time — the Claude
/// keychain service, which is scoped by config directory. That road does not
/// come through this crate any more: a macOS keychain item remembers which
/// binaries may read it, and an in-process write put an adhoc-signed identity on
/// that list, one that is different every build. Those calls go through
/// `/usr/bin/security` now (`accounts::security_command`), whose identity is the
/// same tomorrow as today.
#[cfg(not(test))]
pub fn native_secret_store(service: &'static str) -> Arc<dyn SecretStore> {
    native_store_result(Arc::from(service)).unwrap_or_else(|_| Arc::new(UnavailableSecretStore))
}

#[cfg(all(not(test), target_os = "macos"))]
fn native_store_result(service: Arc<str>) -> SecretResult<Arc<dyn SecretStore>> {
    let store =
        apple_native_keyring_store::keychain::Store::new().map_err(classify_keyring_error)?;
    Ok(Arc::new(KeyringSecretStore { service, store }))
}

#[cfg(all(not(test), target_os = "windows"))]
fn native_store_result(service: Arc<str>) -> SecretResult<Arc<dyn SecretStore>> {
    let store = windows_native_keyring_store::Store::new().map_err(classify_keyring_error)?;
    Ok(Arc::new(KeyringSecretStore { service, store }))
}

#[cfg(all(not(test), not(any(target_os = "macos", target_os = "windows"))))]
fn native_store_result(_service: Arc<str>) -> SecretResult<Arc<dyn SecretStore>> {
    Err(SecretStoreError::new(SecretStoreErrorKind::Unavailable))
}

#[cfg(all(not(test), any(target_os = "macos", target_os = "windows")))]
struct KeyringSecretStore {
    /// Not `&'static str`: Claude Code 2.1+ scopes its keychain service by the
    /// config directory it was launched with, so one of these names is built at
    /// run time (see `accounts::keychain_services`).
    service: Arc<str>,
    store: Arc<keyring_core::CredentialStore>,
}

#[cfg(all(not(test), any(target_os = "macos", target_os = "windows")))]
impl fmt::Debug for KeyringSecretStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("KeyringSecretStore")
    }
}

#[cfg(all(not(test), any(target_os = "macos", target_os = "windows")))]
impl KeyringSecretStore {
    fn entry(&self, account: &str) -> SecretResult<keyring_core::Entry> {
        #[cfg(target_os = "windows")]
        {
            // An explicit target removes the provider default's delimiter
            // ambiguity, and app credentials stay local to this machine.
            let target = format!("{}/{account}", self.service);
            let modifiers = std::collections::HashMap::from([
                ("target", target.as_str()),
                ("persistence", "Local"),
            ]);
            return self
                .store
                .build(&self.service, account, Some(&modifiers))
                .map_err(classify_keyring_error);
        }

        #[cfg(target_os = "macos")]
        self.store
            .build(&self.service, account, None)
            .map_err(classify_keyring_error)
    }
}

#[cfg(all(not(test), any(target_os = "macos", target_os = "windows")))]
impl SecretStore for KeyringSecretStore {
    fn protection(&self) -> SecretProtection {
        SecretProtection::Native
    }

    fn standing(&self, account: &str) -> SecretStanding {
        match self.entry(account).and_then(|entry| {
            entry
                .get_credential()
                .map(|_| ())
                .map_err(classify_keyring_error)
        }) {
            Ok(()) => SecretStanding::Available,
            Err(error) if error.kind() == SecretStoreErrorKind::NotFound => SecretStanding::Missing,
            Err(_) => SecretStanding::Unreadable,
        }
    }

    fn read(&self, account: &str) -> SecretResult<Vec<u8>> {
        self.entry(account)?
            .get_secret()
            .map_err(classify_keyring_error)
    }

    fn write(&self, account: &str, secret: &[u8]) -> SecretResult<()> {
        self.entry(account)?
            .set_secret(secret)
            .map_err(classify_keyring_error)
    }

    fn delete(&self, account: &str) -> SecretResult<()> {
        match self
            .entry(account)?
            .delete_credential()
            .map_err(classify_keyring_error)
        {
            Err(error) if error.kind() == SecretStoreErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

#[cfg(all(not(test), any(target_os = "macos", target_os = "windows")))]
fn classify_keyring_error(error: keyring_core::Error) -> SecretStoreError {
    let kind = match error {
        keyring_core::Error::NoEntry => SecretStoreErrorKind::NotFound,
        keyring_core::Error::NoStorageAccess(_) => SecretStoreErrorKind::AccessDenied,
        _ => SecretStoreErrorKind::Unavailable,
    };
    SecretStoreError::new(kind)
}

#[derive(Debug)]
pub struct UnavailableSecretStore;

impl SecretStore for UnavailableSecretStore {
    fn protection(&self) -> SecretProtection {
        SecretProtection::Unavailable
    }

    fn standing(&self, _account: &str) -> SecretStanding {
        SecretStanding::Unreadable
    }

    fn read(&self, _account: &str) -> SecretResult<Vec<u8>> {
        Err(SecretStoreError::new(SecretStoreErrorKind::Unavailable))
    }

    fn write(&self, _account: &str, _secret: &[u8]) -> SecretResult<()> {
        Err(SecretStoreError::new(SecretStoreErrorKind::Unavailable))
    }

    fn delete(&self, _account: &str) -> SecretResult<()> {
        Err(SecretStoreError::new(SecretStoreErrorKind::Unavailable))
    }
}

#[cfg(any(test, feature = "test-memory"))]
pub use self::memory::{MemorySecretStore, SecretOperation};

#[cfg(any(test, feature = "test-memory"))]
mod memory {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum SecretOperation {
        Probe,
        Read,
        Write,
        Delete,
    }

    #[derive(Default)]
    pub struct MemorySecretStore {
        secrets: Mutex<HashMap<String, Vec<u8>>>,
        failures: Mutex<HashMap<SecretOperation, SecretStoreErrorKind>>,
        read_replacement: Mutex<Option<Vec<u8>>>,
    }

    impl fmt::Debug for MemorySecretStore {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("MemorySecretStore")
        }
    }

    impl MemorySecretStore {
        pub fn secret(&self, account: &str) -> Option<Vec<u8>> {
            self.secrets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(account)
                .cloned()
        }

        pub fn fail(&self, operation: SecretOperation, kind: SecretStoreErrorKind) {
            self.failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(operation, kind);
        }

        pub fn allow(&self, operation: SecretOperation) {
            self.failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&operation);
        }

        pub fn replace_reads_with(&self, replacement: Option<Vec<u8>>) {
            *self
                .read_replacement
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = replacement;
        }

        fn check(&self, operation: SecretOperation) -> SecretResult<()> {
            match self
                .failures
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&operation)
                .copied()
            {
                Some(kind) => Err(SecretStoreError::new(kind)),
                None => Ok(()),
            }
        }
    }

    impl SecretStore for MemorySecretStore {
        fn protection(&self) -> SecretProtection {
            SecretProtection::Native
        }

        fn standing(&self, account: &str) -> SecretStanding {
            if self.check(SecretOperation::Probe).is_err() {
                return SecretStanding::Unreadable;
            }
            if self
                .secrets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(account)
            {
                SecretStanding::Available
            } else {
                SecretStanding::Missing
            }
        }

        fn read(&self, account: &str) -> SecretResult<Vec<u8>> {
            self.check(SecretOperation::Read)?;
            if let Some(replacement) = self
                .read_replacement
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
            {
                return Ok(replacement);
            }
            self.secret(account)
                .ok_or_else(|| SecretStoreError::new(SecretStoreErrorKind::NotFound))
        }

        fn write(&self, account: &str, secret: &[u8]) -> SecretResult<()> {
            self.check(SecretOperation::Write)?;
            self.secrets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(account.to_string(), secret.to_vec());
            Ok(())
        }

        fn delete(&self, account: &str) -> SecretResult<()> {
            self.check(SecretOperation::Delete)?;
            self.secrets
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(account);
            Ok(())
        }
    }
}
