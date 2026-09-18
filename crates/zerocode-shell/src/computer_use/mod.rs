//! The window-owned Computer Use runtime.
//!
//! One provider session per window, whichever platform stands behind it. The
//! agent shims send typed commands to the window rather than launching
//! anything themselves, so the element indexes one observation handed out
//! are still meaningful to the action that follows: the snapshot cache lives
//! with the session, and the session lives here.
//!
//! What a session IS differs by platform, and only there:
//! - macOS (`macos`): a signed helper app the window launches through
//!   LaunchServices and talks to over an authenticated UNIX socket — the
//!   helper, not the window, is what TCC judges (module comment there).
//! - Windows (`windows`): a dedicated COM thread inside this process — there
//!   is no permission identity to keep separate, and the foreground-window
//!   rights an in-process provider inherits from the window are exactly what
//!   `--restore-window` needs.
//! - Anywhere else (`unsupported`): an honest refusal.
//!
//! Every session speaks [`session::ProviderSession`]; the code in this file
//! is the part that is the same everywhere: hold one, start it lazily, drop
//! it on a transport failure so the next request gets a clean start, keep it
//! on a provider refusal so the snapshots survive.

pub mod arena;
mod permissions;
pub mod repeat;
mod screenshot_export;
mod session;
// The Windows session's platform-neutral halves: pointer arithmetic, a
// provider on a thread and a PNG within budget. Compiled into every test
// build so their tests run on the developer's machine, and into the Windows
// binary that uses them.
#[cfg(any(target_os = "windows", test))]
mod clipboard_formats;
pub mod compare;
pub mod confirm;
pub mod errand;
pub mod evidence;
pub mod eye;
#[cfg(any(target_os = "windows", test))]
mod geometry;
pub mod guard;
pub mod guarded;
pub mod listen;
pub mod marks;
pub mod observe;
pub mod recipe_run;
pub mod recipes;
pub mod report;
pub(crate) mod screenshot_png;
pub mod sequence;
pub mod state;
#[cfg(any(target_os = "windows", test))]
mod thread;
#[cfg(any(target_os = "windows", test))]
mod typed_text;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use zerocode_core::computer_use::{
    ComputerPermissionId, ComputerPermissionReport, ComputerPermissionReset,
    ComputerPermissionSetup,
};

pub use screenshot_export::{export_screenshot, screenshot_png};
/// The provider's refusal type is the shared contract's; the window keeps
/// its old name for it.
pub use zerocode_core::computer_use_protocol::ProviderError as ComputerUseError;

use session::{ProviderSession as _, SessionFailure};

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
static CLIENT: OnceLock<Mutex<Option<platform::Session>>> = OnceLock::new();

/// Where the bundled resources are, when the app knows.
pub fn initialize(resource_dir: Option<PathBuf>) {
    if let Some(resource_dir) = resource_dir {
        let _ = RESOURCE_DIR.set(resource_dir);
    }
}

/// The resource directory `initialize` was told about.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn resource_dir() -> Option<&'static Path> {
    RESOURCE_DIR.get().map(PathBuf::as_path)
}

fn held_client() -> &'static Mutex<Option<platform::Session>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

/// Call the persistent provider. Transport failures discard the session so a
/// later request gets one clean restart; provider refusals keep its snapshots.
pub fn call(method: &str, params: Value) -> Result<Value, ComputerUseError> {
    let mut held = held_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if held.is_none() {
        let session = platform::Session::start()?;
        guard::remember_helper(session.helper_pid());
        *held = Some(session);
    }
    let session = held.as_mut().expect("session stood");
    match session.request(method, params) {
        Ok(answer) => Ok(answer),
        Err(SessionFailure::Provider(error)) => Err(error),
        Err(SessionFailure::Transport(error)) => {
            *held = None;
            guard::remember_helper(None);
            evidence::end_session();
            Err(error)
        }
    }
}

/// Whether a provider session stands right now — without starting one.
#[must_use]
pub fn session_stands() -> bool {
    held_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

/// End the session, if one stands. The next `call` starts a fresh one.
pub fn shutdown() {
    if let Some(client) = CLIENT.get() {
        *client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
    guard::remember_helper(None);
    evidence::end_session();
}

#[must_use]
pub fn permission_status() -> ComputerPermissionReport {
    platform::permission_status()
}

pub fn open_permission(
    permission_id: Option<ComputerPermissionId>,
) -> Result<ComputerPermissionSetup, ComputerUseError> {
    platform::open_permission(permission_id)
}

pub fn setup_permission(
    id: Option<ComputerPermissionId>,
    reset: bool,
) -> Result<ComputerPermissionSetup, ComputerUseError> {
    if reset {
        let _id = id.ok_or_else(|| {
            ComputerUseError::invalid_argument("permissions --reset requires --id")
        })?;
        #[cfg(target_os = "macos")]
        platform::reset_permission(_id)?;
    }
    open_permission(id)
}

pub fn reset_permissions() -> Result<ComputerPermissionReset, ComputerUseError> {
    platform::reset_permissions()
}
