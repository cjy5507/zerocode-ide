//! Compatibility exports for shell-only credential consumers.
//!
//! Credential ownership lives in `zerocode-shell-state`; this module keeps the
//! existing shell import path stable while the remaining domains migrate.

#[allow(unused_imports)]
pub(crate) use zerocode_shell_state::credential_store::{
    SecretStanding, SecretStore, SecretStoreErrorKind, VerifiedSecretError, VerifiedSecretStore,
};

#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use zerocode_shell_state::credential_store::{MemorySecretStore, SecretOperation};
