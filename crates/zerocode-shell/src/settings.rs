//! Compatibility exports for the state-owned settings repository.

#[allow(unused_imports)]
pub(crate) use zerocode_shell_state::settings::{
    PATH_MIGRATION_RECOVERY_MARKERS, PATH_MIGRATION_SCHEMA, is_repository_recovery_file,
};
#[allow(unused_imports)]
pub(crate) use zerocode_shell_state::settings::{
    SettingsError, SettingsHealth, SettingsMutation, SettingsRead, SettingsRepository,
    SettingsWrite,
};
