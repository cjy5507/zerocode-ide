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
pub(crate) mod browser_route;
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
pub(crate) mod cover;
pub mod enter;
pub mod errand;
pub mod evidence;
pub mod eye;
#[cfg(any(target_os = "windows", test))]
mod geometry;
pub mod guard;
pub mod guarded;
pub(crate) mod input_policy;
pub mod listen;
pub mod marks;
pub mod observe;
mod own_window;
pub mod recipe_run;
pub mod recipes;
pub mod reflex;
pub mod report;
pub(crate) mod screenshot_png;
pub(crate) mod self_surface;
pub mod sequence;
pub(crate) mod share;
pub mod state;
pub(crate) mod surface_execution;
#[cfg(any(target_os = "windows", test))]
mod thread;
#[cfg(any(target_os = "windows", test))]
mod typed_text;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;
/// The signed helper stood in for by a thread of the test process (t-66753):
/// the window's own road to it, with nothing launched.
#[cfg(all(test, target_os = "macos"))]
pub(crate) use macos::stand_in;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use unsupported as platform;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use zerocode_core::computer_use::{
    ComputerPermissionId, ComputerPermissionReport, ComputerPermissionReset,
    ComputerPermissionRowAction, ComputerPermissionSetup,
};

pub use screenshot_export::{export_screenshot, screenshot_png};
/// The provider's refusal type is the shared contract's; the window keeps
/// its old name for it.
pub use zerocode_core::computer_use_protocol::ProviderError as ComputerUseError;

use session::{ProviderSession as _, SessionFailure};

static RESOURCE_DIR: OnceLock<PathBuf> = OnceLock::new();
static CLIENT: OnceLock<Mutex<Option<platform::Session>>> = OnceLock::new();
static SESSION_EPOCH: AtomicU64 = AtomicU64::new(0);

fn session_epoch() -> u64 {
    SESSION_EPOCH.load(Ordering::Acquire)
}

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

/// Only a request in flight defers panel startup. Poison still grants the
/// lock, as it does for the other session callers; it must not keep retrying.
#[cfg(any(target_os = "macos", test))]
fn panel_lock_unless_busy<T>(result: std::sync::TryLockResult<T>) -> Option<T> {
    match result {
        Ok(held) => Some(held),
        Err(std::sync::TryLockError::Poisoned(held)) => Some(held.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

#[test]
fn panel_lock_recovers_poison_and_only_defers_a_busy_request() {
    use std::sync::{PoisonError, TryLockError};

    assert_eq!(panel_lock_unless_busy(Ok(1)), Some(1));
    assert_eq!(
        panel_lock_unless_busy::<u8>(Err(TryLockError::WouldBlock)),
        None
    );
    assert_eq!(
        panel_lock_unless_busy(Err(TryLockError::Poisoned(PoisonError::new(2)))),
        Some(2)
    );
}

/// A shared page needs its native person-only panel even when the page route
/// handles all actions. Starting the helper posts no input and takes no focus.
#[cfg(all(target_os = "macos", not(test)))]
fn ensure_share_panel() -> bool {
    let Some(mut held) = panel_lock_unless_busy(held_client().try_lock()) else {
        // A provider may already be starting; this is not a failed launch.
        return false;
    };
    if held
        .as_ref()
        .is_some_and(platform::Session::panel_watch_ended)
    {
        discard_session(&mut held);
    }
    if held.is_none()
        && let Ok(session) = platform::Session::start()
    {
        guard::remember_helper(session.helper_pid());
        SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
        *held = Some(session);
    }
    // The panel worker remembers an attempted launch, including a failure.
    true
}

/// Call the persistent provider. Transport failures discard the session so a
/// later request gets one clean restart; provider refusals keep its snapshots.
pub fn call(method: &str, params: Value) -> Result<Value, ComputerUseError> {
    input_policy::ensure_current()?;
    // A request admitted to a window the person since took back goes no
    // further (t-73906).
    share::ensure_current()?;
    // ZeroCode's own app is the page's to answer, before any helper road
    // (t-73906): no OS event is made in the window the person works in.
    if let Some(answer) = self_surface::route(method, &params) {
        return answer;
    }
    browser_route::provider(method, &params)?;
    let params = input_policy::current().prepare_provider(method, params)?;
    if let Some(params) = params.as_object() {
        zerocode_core::computer_use_protocol::execution::validate(method, params)?;
    }
    #[cfg(test)]
    if let Some(answer) = browser_route::tests::helper(method, &params) {
        return answer;
    }
    #[cfg(test)]
    if let Some(answer) = test_helper::answer(method, &params) {
        return answer;
    }
    let mut held = held_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    input_policy::ensure_current()?;
    if held.is_none() {
        let session = platform::Session::start()?;
        guard::remember_helper(session.helper_pid());
        SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
        *held = Some(session);
    }
    let session = held.as_mut().expect("session stood");
    if let Some(params) = params.as_object() {
        zerocode_core::computer_use_protocol::execution::require_support(
            session.supports_background(),
            params,
        )?;
    }
    surface_execution::record_provider(session_epoch());
    // ZeroCode's own window under a point the request presses at — or a
    // point whose window cannot be read — is refused here first: the helper
    // reads the pointer's own picture, where the window server lists one, as
    // the window there (t-12979). A road that broke while listing is the
    // request's own break: the session goes as it would below.
    #[cfg(target_os = "macos")]
    {
        let mut broke = None;
        let refused = own_window::own_window_first(method, &params, &mut || {
            marks::desktop_windows(&mut |asked, with| {
                session
                    .request(asked, with)
                    .map_err(|failure| match failure {
                        SessionFailure::Provider(error) => error,
                        SessionFailure::Transport(error) => {
                            broke = Some(error.clone());
                            error
                        }
                    })
            })
            .ok()
        });
        if let Some(error) = broke {
            return Err(discard(&mut held, error));
        }
        if let Some(refused) = refused {
            return Err(refused);
        }
    }
    input_policy::ensure_current()?;
    match session.request(method, params) {
        Ok(answer) => Ok(answer),
        Err(SessionFailure::Provider(error)) => Err(error),
        Err(SessionFailure::Transport(error)) => Err(discard(&mut held, error)),
    }
}

/// A broken road's end: the session is discarded so the next call starts a
/// clean one, and the break is the answer.
fn discard(held: &mut Option<platform::Session>, error: ComputerUseError) -> ComputerUseError {
    discard_session(held);
    error
}

fn discard_session(held: &mut Option<platform::Session>) {
    *held = None;
    SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
    guard::remember_helper(None);
    evidence::end_session();
}

/// Whether a provider session stands right now — without starting one.
#[must_use]
pub fn session_stands() -> bool {
    held_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .is_some()
}

/// Whether a provider session stands, never waiting behind a request in
/// flight (t-62469 B2): a request that holds the session is a session
/// standing. An agent's stop asks it before its signal, and the signal is
/// what ends that request.
#[must_use]
pub fn session_stands_now() -> bool {
    match held_client().try_lock() {
        Ok(held) => held.is_some(),
        Err(std::sync::TryLockError::Poisoned(held)) => held.into_inner().is_some(),
        Err(std::sync::TryLockError::WouldBlock) => true,
    }
}

/// Whether a request holds the session's lock right now — not whether a
/// session stands: one left standing by an earlier test, as a Windows
/// process keeps its in-process provider, is idle and holds nothing.
#[cfg(test)]
pub(crate) fn session_lock_held_now() -> bool {
    matches!(
        held_client().try_lock(),
        Err(std::sync::TryLockError::WouldBlock)
    )
}

/// Hold the session's lock for as long as `hold` runs, as [`call`] holds it
/// across a request in flight — a test's request that reaches no helper.
#[cfg(test)]
pub(crate) fn while_a_request_holds_the_session<T>(hold: impl FnOnce() -> T) -> T {
    let _held = held_client()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    hold()
}

/// The helper's `status` now, or nothing: no session stands, a request holds
/// the session, or the helper did not answer. Whoever asks never waits behind
/// a request in flight, as the stop's signal never does (t-62872); a road that
/// broke drops the session, as [`call`] does.
pub fn status_unless_busy() -> Option<Value> {
    input_policy::ensure_current().ok()?;
    #[cfg(test)]
    if let Some(answer) = test_helper::answer("status", &Value::Null) {
        return answer.ok();
    }
    let mut held = match held_client().try_lock() {
        Ok(held) => held,
        Err(std::sync::TryLockError::Poisoned(held)) => held.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    let session = held.as_mut()?;
    match session.request("status", Value::Object(serde_json::Map::new())) {
        Ok(answer) => Some(answer),
        Err(SessionFailure::Provider(_)) => None,
        Err(SessionFailure::Transport(error)) => {
            discard(&mut held, error);
            None
        }
    }
}

/// End the session, if one stands. The next `call` starts a fresh one.
pub fn shutdown() {
    if let Some(client) = CLIENT.get() {
        let mut held = client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if held.take().is_some() {
            SESSION_EPOCH.fetch_add(1, Ordering::AcqRel);
        }
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

/// A TCC row's button, pressed by the person on the settings page — never by
/// anything else: `action` on `bundle_id`'s row of `id`'s service, answered
/// with the report read after it.
pub fn tcc_row_action(
    id: ComputerPermissionId,
    bundle_id: &str,
    action: ComputerPermissionRowAction,
) -> Result<ComputerPermissionReport, ComputerUseError> {
    platform::tcc_row_action(id, bundle_id, action)
}

/// A test's helper, on the thread that stood it (t-66760): it answers
/// [`call`] and [`status_unless_busy`] in place of a session until the
/// handle drops, so a test can look, wait and be stopped without a helper.
#[cfg(test)]
pub(crate) mod test_helper {
    use std::cell::RefCell;

    use serde_json::Value;

    use super::ComputerUseError;

    type Answers = Box<dyn FnMut(&str, &Value) -> Result<Value, ComputerUseError>>;

    thread_local! {
        static STANDING: RefCell<Option<Answers>> = const { RefCell::new(None) };
    }

    /// The helper stands while this lives.
    pub(crate) struct Standing(());

    impl Drop for Standing {
        fn drop(&mut self) {
            STANDING.with(|standing| standing.borrow_mut().take());
        }
    }

    pub(crate) fn stand(
        answers: impl FnMut(&str, &Value) -> Result<Value, ComputerUseError> + 'static,
    ) -> Standing {
        STANDING.with(|standing| *standing.borrow_mut() = Some(Box::new(answers)));
        Standing(())
    }

    pub(super) fn answer(method: &str, params: &Value) -> Option<Result<Value, ComputerUseError>> {
        STANDING.with(|standing| {
            standing
                .borrow_mut()
                .as_mut()
                .map(|answers| answers(method, params))
        })
    }
}
