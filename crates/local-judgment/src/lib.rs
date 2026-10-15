mod adapter;
mod archive;
mod catalog;
mod engine;
mod manager;
mod selection;
mod source;
mod storage;

pub use catalog::{Artifact, Catalog, EngineKind, Model};
pub use manager::{Manager, Snapshot, SourceInspection};
pub use selection::{Binding, MANAGED_MODEL_KEY};
pub use source::SourceInput;

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
    CatalogConflict,
    InstanceChanged,
    EngineUnsupported,
    ModelManifestMissing,
    ExecutableArtifact,
    InputUnsupported,
    EngineUnverified,
    ModelFormatUnsupported,
    SourceInvalid,
    SourceUnavailable,
    SourceAuthRequired,
    SourceNotNative,
    SourceHashUnavailable,
    SourceChanged,
    InspectionChanged,
    SourceRequiresFolder,
    DestinationUnavailable,
    WeightsFormat,
    RestoreFailed,
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
            Self::CatalogConflict => "catalog_conflict",
            Self::InstanceChanged => "manager_instance_changed",
            Self::EngineUnsupported => "engine_not_supported",
            Self::ModelManifestMissing => "model_manifest_missing",
            Self::ExecutableArtifact => "executable_model_artifact",
            Self::InputUnsupported => "input_not_supported",
            Self::EngineUnverified => "engine_unverified",
            Self::ModelFormatUnsupported => "model_format_not_supported",
            Self::SourceInvalid => "source_invalid",
            Self::SourceUnavailable => "source_unavailable",
            Self::SourceAuthRequired => "source_auth_required",
            Self::SourceNotNative => "not_native_decision_model",
            Self::SourceHashUnavailable => "source_hash_unavailable",
            Self::SourceChanged => "source_changed",
            Self::InspectionChanged => "source_inspection_changed",
            Self::SourceRequiresFolder => "source_folder_required",
            Self::DestinationUnavailable => "destination_unavailable",
            Self::WeightsFormat => "weights_format_invalid",
            Self::RestoreFailed => "model_restore_failed",
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.token())
    }
}

impl std::error::Error for Error {}

#[cfg(all(test, unix))]
mod admission_tests;
#[cfg(test)]
mod source_tests;
#[cfg(all(test, unix))]
mod switch_tests;
#[cfg(test)]
mod tests;
