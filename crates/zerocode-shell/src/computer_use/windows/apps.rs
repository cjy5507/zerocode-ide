//! Which processes are "apps" and how an agent names one.
//!
//! macOS asks `NSWorkspace` for regular-activation-policy apps; Windows has
//! no such register, so an app is a process that owns at least one usable
//! top-level window ([`super::desktop_windows::candidates`]). Its name is the
//! executable's `FileDescription` ("Google Chrome") when the version resource
//! carries one, its file stem otherwise; its `bundleId` is the
//! AppUserModelId of a packaged app and `null` for a plain executable.
//!
//! Describing a process reads its executable's version resource from disk.
//! A name query (`--app Notepad`) has to describe every window-owning
//! process to find its match, and every action observes twice — so the
//! descriptions live in a [`Catalog`] the provider keeps, keyed by pid AND
//! process start time (a pid is reused the moment its process ends), and
//! pruned to the processes that still own a window on each listing.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{
    APPMODEL_ERROR_NO_APPLICATION, CloseHandle, ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE,
};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{PCWSTR, PWSTR};
use zerocode_core::computer_use_protocol::identity::{
    AppQuery, blocked, is_blocked_windows_app, is_known_browser, matches,
};
use zerocode_core::computer_use_protocol::{AppIdentity, ListedApp, ProviderError, error_code};

use super::desktop_windows;

#[derive(Debug, Clone)]
pub(super) struct AppDescriptor {
    pub name: String,
    pub aumid: Option<String>,
    pub pid: u32,
    pub executable: Option<PathBuf>,
}

impl AppDescriptor {
    #[must_use]
    pub fn identity(&self) -> AppIdentity {
        AppIdentity {
            name: self.name.clone(),
            bundle_id: self.aumid.clone(),
            pid: self.pid,
        }
    }

    #[must_use]
    pub fn listed(&self) -> ListedApp {
        ListedApp::running(self.identity())
    }

    #[must_use]
    pub fn executable_name(&self) -> Option<String> {
        self.executable
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
    }

    #[must_use]
    pub fn executable_stem(&self) -> Option<String> {
        self.executable
            .as_ref()
            .and_then(|path| path.file_stem())
            .map(|stem| stem.to_string_lossy().into_owned())
    }

    #[must_use]
    pub fn is_browser(&self) -> bool {
        is_known_browser(
            &self.name,
            self.aumid.as_deref(),
            self.executable_name().as_deref(),
        )
    }

    fn is_blocked(&self) -> bool {
        is_blocked_windows_app(self.executable_name().as_deref(), self.aumid.as_deref())
    }

    fn blocked_identity(&self) -> String {
        self.aumid
            .clone()
            .or_else(|| self.executable_name())
            .unwrap_or_else(|| self.name.clone())
    }
}

/// A process handle with query rights, closed when dropped.
struct ProcessHandle(HANDLE);

impl ProcessHandle {
    fn open(pid: u32) -> Option<Self> {
        // SAFETY: a plain OpenProcess; the handle is closed in Drop.
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
            .ok()
            .map(Self)
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: closes the handle this value owns, once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn image_path(process: &ProcessHandle) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32 * 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: the buffer outlives the call and `length` names its capacity;
    // the API writes at most that many code units and returns the count.
    unsafe {
        QueryFullProcessImageNameW(
            process.0,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

/// When the process started, as the 64-bit FILETIME — the half of a
/// process identity a pid alone lacks.
fn start_time(process: &ProcessHandle) -> Option<u64> {
    let mut creation = FILETIME::default();
    let mut exit = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: four FILETIMEs the API fills, all outliving the call.
    unsafe { GetProcessTimes(process.0, &mut creation, &mut exit, &mut kernel, &mut user) }.ok()?;
    Some((u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime))
}

fn app_user_model_id(process: &ProcessHandle) -> Option<String> {
    let mut length = 0u32;
    // SAFETY: a length query with no buffer; the API only writes `length`.
    let sizing = unsafe { GetApplicationUserModelId(process.0, &mut length, None) };
    if sizing == APPMODEL_ERROR_NO_APPLICATION || sizing != ERROR_INSUFFICIENT_BUFFER {
        return None;
    }
    let mut buffer = vec![0u16; length as usize];
    // SAFETY: `buffer` holds exactly the `length` code units the first call
    // asked for and outlives this call.
    let filled = unsafe {
        GetApplicationUserModelId(process.0, &mut length, Some(PWSTR(buffer.as_mut_ptr())))
    };
    if filled.0 != 0 {
        return None;
    }
    let end = buffer
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `FileDescription` from the executable's version resource, in its first
/// listed translation — "Google Chrome" for `chrome.exe`.
fn file_description(path: &Path) -> Option<String> {
    let wide_path = wide(&path.to_string_lossy());
    // SAFETY: the wide path is NUL-terminated and outlives both calls.
    let size = unsafe { GetFileVersionInfoSizeW(PCWSTR(wide_path.as_ptr()), None) };
    if size == 0 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    // SAFETY: `block` has the `size` bytes the API asked for.
    unsafe {
        GetFileVersionInfoW(
            PCWSTR(wide_path.as_ptr()),
            None,
            size,
            block.as_mut_ptr().cast(),
        )
    }
    .ok()?;
    let translation_key = wide("\\VarFileInfo\\Translation");
    let mut translation: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut translation_len = 0u32;
    // SAFETY: `block` outlives the query; the out pointers are written by the
    // API and read only after it says they were.
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(translation_key.as_ptr()),
            &mut translation,
            &mut translation_len,
        )
    };
    if !found.as_bool() || translation.is_null() || translation_len < 4 {
        return None;
    }
    // SAFETY: the API reported at least four bytes of (language, codepage)
    // pairs at `translation`, inside `block`.
    let (language, codepage) = unsafe {
        let pairs = translation.cast::<u16>();
        (*pairs, *pairs.add(1))
    };
    let key = wide(&format!(
        "\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription"
    ));
    let mut text: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut text_len = 0u32;
    // SAFETY: as above; `text` points into `block` when `found`.
    let found = unsafe {
        VerQueryValueW(
            block.as_ptr().cast(),
            PCWSTR(key.as_ptr()),
            &mut text,
            &mut text_len,
        )
    };
    if !found.as_bool() || text.is_null() || text_len == 0 {
        return None;
    }
    // SAFETY: `text_len` code units at `text`, inside `block`.
    let units = unsafe { std::slice::from_raw_parts(text.cast::<u16>(), text_len as usize) };
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    let description = String::from_utf16_lossy(&units[..end]).trim().to_string();
    (!description.is_empty()).then_some(description)
}

/// Everything this module knows about a process, or `None` when it cannot be
/// opened at all. One disk read of the version resource per call — which
/// is why callers go through the [`Catalog`].
fn describe_process(pid: u32) -> Option<(u64, AppDescriptor)> {
    let process = ProcessHandle::open(pid)?;
    let started = start_time(&process).unwrap_or(0);
    let executable = image_path(&process);
    let aumid = app_user_model_id(&process);
    let name = executable
        .as_deref()
        .and_then(file_description)
        .or_else(|| {
            executable
                .as_deref()
                .and_then(Path::file_stem)
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| format!("pid:{pid}"));
    Some((
        started,
        AppDescriptor {
            name,
            aumid,
            pid,
            executable,
        },
    ))
}

/// The process start time a pid currently has, cheap enough to ask on
/// every lookup: it is what tells a cached description from a stranger who
/// inherited the pid.
fn current_start_time(pid: u32) -> Option<u64> {
    let process = ProcessHandle::open(pid)?;
    start_time(&process)
}

/// Whether `pid` runs elevated. `None` when the token cannot be read — which
/// for a foreign process usually means it IS elevated above us.
pub(super) fn is_process_elevated(pid: u32) -> Option<bool> {
    let process = ProcessHandle::open(pid)?;
    token_is_elevated(process.0)
}

pub(super) fn current_process_elevated() -> bool {
    // SAFETY: the pseudo-handle needs no closing.
    token_is_elevated(unsafe { GetCurrentProcess() }).unwrap_or(false)
}

fn token_is_elevated(process: HANDLE) -> Option<bool> {
    let mut token = HANDLE::default();
    // SAFETY: `token` receives a handle we close below.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0u32;
    // SAFETY: `elevation` is the struct the class names, its size is passed.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&raw mut elevation).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
    };
    // SAFETY: closes the token handle opened above.
    let _ = unsafe { CloseHandle(token) };
    read.ok()?;
    Some(elevation.TokenIsElevated != 0)
}

/// One described process, remembered with the start time that makes its
/// pid an identity.
#[derive(Debug, Clone)]
struct CachedApp {
    started: u64,
    app: AppDescriptor,
}

/// The provider's memory of the processes it has described. Deliberate
/// invalidation, not repeated walks: an entry is reused while the pid still
/// has the start time it was described with, dropped when the pid is gone
/// or reused, and swept on every listing to the processes that own a
/// window.
#[derive(Default)]
pub(super) struct Catalog {
    apps: HashMap<u32, CachedApp>,
    /// How many descriptions were served from memory versus read from the
    /// process — the number the performance report prints.
    pub hits: u64,
    pub misses: u64,
}

impl Catalog {
    /// Describe `pid`, from memory when the same process is still there.
    pub fn describe(&mut self, pid: u32) -> Option<AppDescriptor> {
        if let Some(cached) = self.apps.get(&pid)
            && current_start_time(pid) == Some(cached.started)
        {
            self.hits += 1;
            return Some(cached.app.clone());
        }
        self.misses += 1;
        let (started, app) = describe_process(pid)?;
        self.apps.insert(
            pid,
            CachedApp {
                started,
                app: app.clone(),
            },
        );
        Some(app)
    }

    /// Every process with a usable window, the foreground one first, the
    /// rest by name. Entries for processes without a window are swept.
    pub fn list(&mut self) -> Vec<AppDescriptor> {
        let foreground_pid = desktop_windows::foreground().map(|(pid, _)| pid);
        let mut pids: Vec<u32> = desktop_windows::all_top_level()
            .into_iter()
            .map(|(pid, _)| pid)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        self.apps.retain(|pid, _| pids.binary_search(pid).is_ok());
        let mut apps: Vec<AppDescriptor> = pids
            .into_iter()
            .filter_map(|pid| self.describe(pid))
            .collect();
        apps.sort_by(|left, right| {
            let left_active = Some(left.pid) == foreground_pid;
            let right_active = Some(right.pid) == foreground_pid;
            right_active
                .cmp(&left_active)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
        });
        apps
    }

    /// `resolveApp`: a pid, a name, an AppUserModelId or an executable stem;
    /// blocked apps refused; an elevated app above a non-elevated window
    /// refused as `permission_denied` (UIPI would drop every input anyway).
    pub fn resolve(&mut self, query: &str) -> Result<AppDescriptor, ProviderError> {
        let parsed = AppQuery::parse(query)?;
        let app = match &parsed {
            AppQuery::Pid(pid) => self
                .describe(*pid)
                .filter(|_| !desktop_windows::candidates(*pid).is_empty())
                .ok_or_else(|| parsed.not_found())?,
            AppQuery::Named(name) => self
                .list()
                .into_iter()
                .find(|app| {
                    matches(
                        name,
                        &app.name,
                        app.aumid.as_deref(),
                        app.executable_stem().as_deref(),
                    )
                })
                .ok_or_else(|| parsed.not_found())?,
        };
        if app.is_blocked() {
            return Err(blocked(&app.blocked_identity()));
        }
        // A token that cannot be read is not proof of elevation: the accessibility
        // read that follows answers `E_ACCESSDENIED` for a truly elevated app, and
        // `snapshot::build` words that as `permission_denied` too.
        if !current_process_elevated() && is_process_elevated(app.pid).unwrap_or(false) {
            return Err(ProviderError::new(
                error_code::PERMISSION_DENIED,
                format!(
                    "app '{}' runs elevated and cannot be observed or driven from a non-elevated ZeroCode; run it without administrator rights or run ZeroCode elevated",
                    app.name
                ),
            ));
        }
        Ok(app)
    }
}
