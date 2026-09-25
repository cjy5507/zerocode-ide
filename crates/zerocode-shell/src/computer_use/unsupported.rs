//! The platform with no provider: every door answers honestly that it is shut.

use serde_json::Value;
use zerocode_core::computer_use::{
    ComputerPermissionId, ComputerPermissionReport, ComputerPermissionReset,
    ComputerPermissionRowAction, ComputerPermissionSetup,
};
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;
use super::permissions::unsupported_permissions;
use super::session::{ProviderSession, SessionFailure};

pub(super) struct Session;

fn refusal() -> ComputerUseError {
    ComputerUseError::new(
        error_code::UNSUPPORTED_CAPABILITY,
        format!(
            "Computer Use is not available on {}; it requires macOS 14 or newer or Windows 10 or newer",
            std::env::consts::OS
        ),
    )
}

impl ProviderSession for Session {
    fn start() -> Result<Self, ComputerUseError> {
        Err(refusal())
    }

    fn request(&mut self, _method: &str, _params: Value) -> Result<Value, SessionFailure> {
        Err(SessionFailure::Provider(refusal()))
    }
}

#[must_use]
pub(super) fn permission_status() -> ComputerPermissionReport {
    unsupported_permissions()
}

pub(super) fn open_permission(
    permission_id: Option<ComputerPermissionId>,
) -> Result<ComputerPermissionSetup, ComputerUseError> {
    Ok(ComputerPermissionSetup {
        identity: None,
        platform: std::env::consts::OS.into(),
        helper_app_path: None,
        judged_rows: Vec::new(),
        tcc_rows: Vec::new(),
        permission_id,
        requested_os: false,
        opened_settings: false,
        launched_helper: false,
        permissions: Some(unsupported_permissions().permissions),
        next_step: None,
    })
}

pub(super) fn reset_permissions() -> Result<ComputerPermissionReset, ComputerUseError> {
    Ok(ComputerPermissionReset {
        report: unsupported_permissions(),
        bundle_ids: Vec::new(),
    })
}

/// No TCC database stands here, so no row either: unsupported, said aloud.
pub(super) fn tcc_row_action(
    _id: ComputerPermissionId,
    _bundle_id: &str,
    _action: ComputerPermissionRowAction,
) -> Result<ComputerPermissionReport, ComputerUseError> {
    Err(refusal())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shut_door_says_so_with_the_code_the_skill_recovers_from() {
        assert!(Session::start().is_err());
        let error = Session::start().err().unwrap();
        assert_eq!(error.code, "unsupported_capability");
        assert!(error.message.contains(std::env::consts::OS));
        let setup = open_permission(Some(ComputerPermissionId::Accessibility)).unwrap();
        assert!(!setup.opened_settings && !setup.launched_helper);
        assert_eq!(
            setup.permission_id,
            Some(ComputerPermissionId::Accessibility)
        );
        assert!(reset_permissions().unwrap().bundle_ids.is_empty());
        assert_eq!(
            tcc_row_action(
                ComputerPermissionId::Accessibility,
                "dev.zerocode.app",
                ComputerPermissionRowAction::Reset,
            )
            .unwrap_err()
            .code,
            "unsupported_capability"
        );
    }
}
