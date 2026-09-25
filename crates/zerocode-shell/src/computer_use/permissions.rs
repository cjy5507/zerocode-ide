//! The permission reports every platform builds from the same two rows.

use zerocode_core::computer_use::{
    ComputerPermissionId, ComputerPermissionReport, ComputerPermissionState,
    ComputerPermissionStatus,
};

/// Both permissions the settings page shows, each in `status`.
#[must_use]
pub(super) fn every_permission(status: ComputerPermissionStatus) -> Vec<ComputerPermissionState> {
    [
        ComputerPermissionId::Accessibility,
        ComputerPermissionId::Screenshots,
    ]
    .into_iter()
    .map(|id| ComputerPermissionState { id, status })
    .collect()
}

/// Both permissions, not granted — the report a platform gives when it could
/// not ask.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[must_use]
pub(super) fn missing_permissions() -> Vec<ComputerPermissionState> {
    every_permission(ComputerPermissionStatus::NotGranted)
}

/// The report of a platform that has no provider at all.
#[cfg_attr(target_os = "macos", allow(dead_code))]
#[must_use]
pub(super) fn unsupported_permissions() -> ComputerPermissionReport {
    ComputerPermissionReport {
        identity: None,
        platform: std::env::consts::OS.into(),
        helper_app_path: None,
        helper_unavailable_reason: None,
        permissions: every_permission(ComputerPermissionStatus::Unsupported),
        judged_rows: Vec::new(),
        // No TCC database stands here: no rows to read, said by an empty list.
        tcc_rows: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_platforms_still_answer_every_permission() {
        let report = unsupported_permissions();
        assert_eq!(report.permissions.len(), 2);
        assert!(
            report
                .permissions
                .iter()
                .all(|permission| permission.status == ComputerPermissionStatus::Unsupported)
        );
        assert_eq!(report.platform, std::env::consts::OS);
        assert!(
            missing_permissions()
                .iter()
                .all(|permission| permission.status == ComputerPermissionStatus::NotGranted)
        );
    }
}
