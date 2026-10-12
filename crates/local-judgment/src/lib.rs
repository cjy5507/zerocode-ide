mod archive;
mod catalog;
mod engine;
mod manager;
mod selection;
mod storage;

pub use manager::{Manager, Snapshot};
pub use selection::{Binding, MANAGED_MODEL_KEY};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Manifest,
    UnsafePath,
    Storage,
    Space,
    Busy,
    Cancelled,
    UnknownModel,
    RevisionChanged,
    Transport,
    Integrity,
    TooLarge,
    Unsupported,
    RuntimeMissing,
    ModelMissing,
    RuntimeFailed,
    ServerStopped,
    ServerChanged,
    NotChecked,
    Protocol,
    Timeout,
    Configured,
    SettingsChanged,
}

impl Error {
    pub const fn token(self) -> &'static str {
        match self {
            Self::Manifest => "manifest_invalid",
            Self::UnsafePath => "unsafe_path",
            Self::Storage => "storage_failed",
            Self::Space => "insufficient_space",
            Self::Busy => "operation_busy",
            Self::Cancelled => "cancelled",
            Self::UnknownModel => "unknown_model",
            Self::RevisionChanged => "model_revision_changed",
            Self::Transport => "download_failed",
            Self::Integrity => "integrity_failed",
            Self::TooLarge => "size_limit",
            Self::Unsupported => "platform_unsupported",
            Self::RuntimeMissing => "runtime_missing",
            Self::ModelMissing => "model_missing",
            Self::RuntimeFailed => "runtime_failed",
            Self::ServerStopped => "server_stopped",
            Self::ServerChanged => "server_changed",
            Self::NotChecked => "server_not_checked",
            Self::Protocol => "native_protocol_failed",
            Self::Timeout => "runtime_timeout",
            Self::Configured => "model_configured",
            Self::SettingsChanged => "settings_changed",
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.token())
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests;
