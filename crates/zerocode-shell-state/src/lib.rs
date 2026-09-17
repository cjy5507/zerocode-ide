//! State owned by the shell window.
//!
//! The composition root owns the lifetime of this one concrete value. Domain
//! crates receive the narrow capabilities they need instead of reaching into
//! the shell's runtime bookkeeping.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

pub mod credential_store;
pub mod durable_file;
pub mod jira_attachments;
pub mod jira_store;
pub mod linear_store;
pub mod settings;
pub mod work_item_store;

/// The settings documents this crate reads or writes, by file name.
///
/// Owned here because the store that reads them lives here; the shell's own
/// catalogue of settings files re-exports these rather than spelling them
/// again, so a rename cannot leave the two disagreeing.
pub mod settings_file {
    pub const WORK_ITEM_LINKS: &str = "work-item-links.json";
}

/// The one Tauri-managed state value shared by the shell and command crates.
///
/// The runtime extension is intentionally type-erased: shell-only lifecycle
/// state remains private to the composition root while the first extraction
/// exposes only the integration/settings capabilities required by command
/// crates. No raw field is public.
pub struct AppState {
    local_data_root: PathBuf,
    settings: Arc<settings::SettingsRepository>,
    jira: jira_store::JiraStore,
    linear: linear_store::LinearStore,
    shell_extension: OnceLock<Box<dyn Any + Send + Sync>>,
}

impl AppState {
    /// Build the shared state from paths and stores resolved by the shell.
    pub fn new(
        local_data_root: impl Into<PathBuf>,
        settings: Arc<settings::SettingsRepository>,
        jira: jira_store::JiraStore,
        linear: linear_store::LinearStore,
    ) -> Self {
        Self {
            local_data_root: local_data_root.into(),
            settings,
            jira,
            linear,
            shell_extension: OnceLock::new(),
        }
    }

    /// The local-data directory used by Jira attachments and shell services.
    #[must_use]
    pub fn local_data_root(&self) -> &Path {
        &self.local_data_root
    }

    /// The durable typed settings repository.
    #[must_use]
    pub fn settings(&self) -> &Arc<settings::SettingsRepository> {
        &self.settings
    }

    /// The Jira site and credential store.
    #[must_use]
    pub fn jira(&self) -> &jira_store::JiraStore {
        &self.jira
    }

    /// The Linear connection metadata and API-key store.
    #[must_use]
    pub fn linear(&self) -> &linear_store::LinearStore {
        &self.linear
    }

    /// Attach shell-only runtime bookkeeping before the state is managed by
    /// Tauri. A second attachment is a programming error, not a replacement
    /// operation: replacing it would let stale command handles observe a new
    /// runtime.
    pub fn install_shell_extension<T: Any + Send + Sync>(&self, extension: T) {
        assert!(
            self.shell_extension.set(Box::new(extension)).is_ok(),
            "shell state extension installed twice"
        );
    }

    /// Access shell-only runtime bookkeeping through the composition root's
    /// concrete type. Command crates do not need this escape hatch.
    #[must_use]
    pub fn shell_extension<T: Any>(&self) -> Option<&T> {
        self.shell_extension.get()?.downcast_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_state_keeps_integration_capabilities_on_one_value() {
        let root = tempfile::tempdir().expect("state root");
        let settings = Arc::new(settings::SettingsRepository::new(
            root.path().join("config"),
        ));
        let jira = jira_store::JiraStore::new(root.path().join("config"));
        let linear = linear_store::LinearStore::new(root.path().join("config"));
        let state = AppState::new(root.path().join("local-data"), settings, jira, linear);

        assert_eq!(state.local_data_root(), root.path().join("local-data"));
        assert_eq!(state.settings().root(), root.path().join("config"));
        assert!(
            state
                .jira()
                .load()
                .expect("empty Jira metadata")
                .sites
                .is_empty()
        );
        assert!(
            state
                .linear()
                .load()
                .expect("empty Linear metadata")
                .is_none()
        );
    }

    #[test]
    fn shell_extension_is_installed_once_and_keeps_its_type_private_to_the_caller() {
        let root = tempfile::tempdir().expect("state root");
        let state = AppState::new(
            root.path(),
            Arc::new(settings::SettingsRepository::new(
                root.path().join("config"),
            )),
            jira_store::JiraStore::new(root.path().join("config")),
            linear_store::LinearStore::new(root.path().join("config")),
        );
        state.install_shell_extension(String::from("runtime"));

        assert_eq!(
            state.shell_extension::<String>().map(String::as_str),
            Some("runtime")
        );
        assert!(state.shell_extension::<u64>().is_none());
    }
}
