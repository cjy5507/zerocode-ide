//! The Windows session: the provider on a COM thread inside this process.
//!
//! No helper executable. macOS needs one because TCC judges the responsible
//! process and a permission has to belong to a signed identity; Windows has
//! no grant to give — UI Automation reads and synthesizes input under the
//! caller's own rights — and the one right that matters, bringing a window
//! to the foreground, is held by the foreground process, which the window
//! is while a person uses it. A separate helper would have to be handed that
//! right on every call. So the provider lives here, on one dedicated
//! multithreaded-apartment thread ([`super::thread`]), and the session talks
//! to it over a channel with the same sixty-second deadline the macOS
//! socket has.
//!
//! What "permission" means here is written down in the settings copy and in
//! the skill: both rows are granted whenever the UI Automation client can be
//! created, and an app running elevated above this window answers
//! `permission_denied` at call time (UIPI), which is the only refusal the
//! platform actually makes.

mod actions;
mod apps;
mod capture;
mod clipboard;
mod desktop_windows;
mod dpi;
mod input;
mod provider;
mod snapshot;
mod uia;

#[cfg(test)]
mod fixture;
#[cfg(test)]
mod tests;

use std::time::Duration;

use serde_json::Value;
use zerocode_core::computer_use::{
    ComputerPermissionId, ComputerPermissionReport, ComputerPermissionReset,
    ComputerPermissionRowAction, ComputerPermissionSetup, ComputerPermissionStatus,
};
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;
use super::permissions::every_permission;
use super::session::{ProviderSession, SessionFailure};
use super::thread::{Handler, ProviderThread};

pub(super) const PROVIDER_NAME: &str = "zerocode-computer-use-windows";
pub(super) const PROVIDER_VERSION: &str = "1.0.0";
/// The same deadline the macOS socket keeps (`REQUEST_TIMEOUT` there).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub(super) struct Session {
    thread: ProviderThread,
}

impl ProviderSession for Session {
    fn start() -> Result<Self, ComputerUseError> {
        let thread = ProviderThread::spawn("zerocode-computer-use", ComHandler::start)?;
        Ok(Self { thread })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, SessionFailure> {
        self.thread.request(method, params, REQUEST_TIMEOUT)
    }
}

/// A multithreaded COM apartment on the current thread, left when dropped.
struct ComApartment;

impl ComApartment {
    fn enter() -> Result<Self, ComputerUseError> {
        use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
        // SAFETY: a plain initialisation call on this thread; no memory
        // crosses. An `S_FALSE` (already initialised) is a success.
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        result.ok().map_err(|error| {
            ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                format!("COM could not be initialised for UI Automation: {error}"),
            )
        })?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        use windows::Win32::System::Com::CoUninitialize;
        // SAFETY: balances the `CoInitializeEx` this value stands for, on
        // the same thread, after every COM object created under it has been
        // released (the provider is declared before the apartment and drops
        // first).
        unsafe { CoUninitialize() };
    }
}

/// The provider, the apartment it lives in, and the DPI contract of the
/// thread it runs on — in that order, so the provider's COM objects are
/// released before the apartment closes, and the thread speaks physical
/// pixels for as long as it speaks at all.
struct ComHandler {
    provider: provider::Provider,
    _apartment: ComApartment,
    _dpi: dpi::ThreadDpiAwareness,
}

impl ComHandler {
    fn start() -> Result<Self, ComputerUseError> {
        let dpi = dpi::ThreadDpiAwareness::per_monitor_v2();
        let apartment = ComApartment::enter()?;
        let provider = provider::Provider::new()?;
        Ok(Self {
            provider,
            _apartment: apartment,
            _dpi: dpi,
        })
    }
}

impl Handler for ComHandler {
    fn handle(&mut self, method: &str, params: Value) -> Result<Value, ComputerUseError> {
        self.provider.handle(method, params)
    }
}

/// Whether a UI Automation client can be created at all — on a scratch
/// thread with its own apartment, so the caller's thread keeps whatever COM
/// mode it had.
fn probe_provider() -> Result<(), ComputerUseError> {
    std::thread::Builder::new()
        .name("zerocode-computer-use-probe".into())
        .spawn(|| {
            let _apartment = ComApartment::enter()?;
            uia::UiaClient::new().map(|_| ())
        })
        .map_err(|error| {
            ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                format!("could not start the permission probe: {error}"),
            )
        })?
        .join()
        .unwrap_or_else(|_| {
            Err(ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                "the permission probe crashed",
            ))
        })
}

#[must_use]
pub(super) fn permission_status() -> ComputerPermissionReport {
    match probe_provider() {
        Ok(()) => ComputerPermissionReport {
            identity: None,
            platform: "windows".into(),
            helper_app_path: None,
            helper_unavailable_reason: None,
            permissions: every_permission(ComputerPermissionStatus::Granted),
            judged_rows: Vec::new(),
            // Windows keeps no TCC database: there is no row to read.
            tcc_rows: Vec::new(),
        },
        Err(error) => ComputerPermissionReport {
            identity: None,
            platform: "windows".into(),
            helper_app_path: None,
            helper_unavailable_reason: Some(error.message),
            permissions: every_permission(ComputerPermissionStatus::NotGranted),
            judged_rows: Vec::new(),
            tcc_rows: Vec::new(),
        },
    }
}

/// There is no settings page to open: the answer is the report, with the
/// one next step Windows has when the provider cannot start.
pub(super) fn open_permission(
    permission_id: Option<ComputerPermissionId>,
) -> Result<ComputerPermissionSetup, ComputerUseError> {
    let report = permission_status();
    let next_step = report
        .helper_unavailable_reason
        .as_ref()
        .map(|reason| format!("UI Automation could not start: {reason}"));
    Ok(ComputerPermissionSetup {
        identity: None,
        platform: "windows".into(),
        helper_app_path: None,
        judged_rows: Vec::new(),
        tcc_rows: Vec::new(),
        permission_id,
        requested_os: false,
        opened_settings: false,
        launched_helper: false,
        permissions: Some(report.permissions),
        next_step,
    })
}

/// Windows keeps no TCC database, so no row stands here to press a button
/// on: the door says unsupported rather than pretending to reset.
pub(super) fn tcc_row_action(
    _id: ComputerPermissionId,
    _bundle_id: &str,
    _action: ComputerPermissionRowAction,
) -> Result<ComputerPermissionReport, ComputerUseError> {
    Err(ComputerUseError::new(
        error_code::UNSUPPORTED_CAPABILITY,
        "Windows keeps no TCC rows: there is nothing to reset or open",
    ))
}

/// Nothing to reset but the session itself; the next call starts a fresh
/// provider thread.
pub(super) fn reset_permissions() -> Result<ComputerPermissionReset, ComputerUseError> {
    super::shutdown();
    Ok(ComputerPermissionReset {
        report: permission_status(),
        bundle_ids: Vec::new(),
    })
}
