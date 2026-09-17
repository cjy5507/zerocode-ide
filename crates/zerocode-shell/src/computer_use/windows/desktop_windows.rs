//! Top-level windows: which ones count, which one an agent means, and who
//! is in front. The Win32 half of what `CGWindowListCopyWindowInfo` and the
//! AX focused-window probes do on macOS.
//!
//! Every rectangle here is physical screen pixels: the provider thread sets
//! itself per-monitor-DPI-aware (`super::dpi`), so `GetWindowRect`, UI
//! Automation's bounding rectangles, `PrintWindow` and `SendInput` all speak
//! the same coordinates — the only system that stays one system across
//! monitors of different scale.
//!
//! A window has two processes when it is a packaged (UWP) app: the frame the
//! desktop shows belongs to `ApplicationFrameHost.exe`, and the app itself
//! draws into a `Windows.UI.Core.CoreWindow` child of it. The frame's owner
//! is the HOST pid (what foreground and pointer probes see); the app the
//! agent means, names and is blocked by is the CoreWindow's pid. Both are
//! kept on the candidate, and each question asks the one it is about.

use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, HDC, HMONITOR, MONITOR_DEFAULTTONULL, MonitorFromRect,
};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GA_ROOT, GW_OWNER, GWL_EXSTYLE, GetAncestor, GetClassNameW,
    GetForegroundWindow, GetSystemMetrics, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, SM_CMONITORS,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SW_RESTORE,
    SetForegroundWindow, ShowWindow, WINDOW_EX_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WindowFromPoint,
};
use windows::core::BOOL;
use zerocode_core::computer_use_protocol::click_plan::Recipient;
use zerocode_core::computer_use_protocol::render::Rect;

/// One top-level window, as listed and as targeted.
#[derive(Debug, Clone)]
pub(super) struct WindowCandidate {
    pub hwnd: isize,
    /// The app this window belongs to — the process an agent names and a
    /// block list judges. For a packaged app's frame this is the
    /// CoreWindow's process, not the frame host's.
    pub pid: u32,
    /// The process that owns the handle — what `GetWindowThreadProcessId`
    /// answers for the frame, and so what every foreground and
    /// pointer probe answers. Equal to `pid` for a classic window.
    pub host_pid: u32,
    pub title: String,
    pub bounds: Rect,
    pub is_minimized: bool,
    pub is_offscreen: bool,
    pub topmost: bool,
    pub screen_index: Option<usize>,
}

impl WindowCandidate {
    #[must_use]
    pub fn id(&self) -> u64 {
        self.hwnd as u64
    }

    #[must_use]
    pub fn handle(&self) -> HWND {
        hwnd_from_id(self.id())
    }

    /// The helper's `WindowCandidate.score`: bigger, titled, on-screen wins.
    fn score(&self) -> i64 {
        let mut value = self.bounds.area() as i64;
        value += 1_000_000_000;
        if !self.title.is_empty() {
            value += 10_000_000;
        }
        if !self.is_offscreen {
            value += 1_000_000;
        }
        if !self.is_minimized {
            value += 100_000;
        }
        value
    }
}

pub(super) fn hwnd_from_id(id: u64) -> HWND {
    HWND(id as usize as *mut core::ffi::c_void)
}

fn rect_of(rect: RECT) -> Rect {
    Rect::new(
        f64::from(rect.left),
        f64::from(rect.top),
        f64::from(rect.right - rect.left),
        f64::from(rect.bottom - rect.top),
    )
}

pub(super) fn window_rect(hwnd: HWND) -> Option<Rect> {
    let mut rect = RECT::default();
    // SAFETY: `rect` outlives the call and is the struct the API fills.
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect_of(rect))
}

pub(super) fn title_of(hwnd: HWND) -> String {
    let mut buffer = [0u16; 512];
    // SAFETY: the buffer outlives the call; the API bounds itself by its length.
    let length = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

pub(super) fn owner_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` receives the owner; the return (thread id) is unused.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    pid
}

fn owner_thread(hwnd: HWND) -> u32 {
    // SAFETY: no out-parameter requested.
    unsafe { GetWindowThreadProcessId(hwnd, None) }
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: `cloaked` is the u32 the attribute writes, sized accordingly.
    let read = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            std::mem::size_of::<u32>() as u32,
        )
    };
    read.is_ok() && cloaked != 0
}

fn extended_style(hwnd: HWND) -> WINDOW_EX_STYLE {
    // SAFETY: a plain style read.
    WINDOW_EX_STYLE(unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32)
}

pub(super) fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: the buffer outlives the call; the API bounds itself by it.
    let length = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

/// The desktop's own furniture — the taskbar, the wallpaper window, the
/// worker windows behind it. The handshake declares `surfaces.dock: false`
/// and `menubar: false`, and a window an agent cannot be pointed at must
/// not be a candidate either (macOS lists layer 0 only, for the same
/// reason).
pub(super) fn is_shell_surface(class: &str) -> bool {
    matches!(
        class,
        "Shell_TrayWnd"
            | "Shell_SecondaryTrayWnd"
            | "Progman"
            | "WorkerW"
            | "DesktopWindowXamlSource"
    )
}

const FRAME_HOST_CLASS: &str = "ApplicationFrameWindow";
const CORE_WINDOW_CLASS: &str = "Windows.UI.Core.CoreWindow";

unsafe extern "system" fn find_core_window(hwnd: HWND, data: LPARAM) -> BOOL {
    if class_name(hwnd) == CORE_WINDOW_CLASS {
        // SAFETY: `data` is the Option<HWND> this enumeration was started
        // with, alive for the whole call.
        unsafe { *(data.0 as *mut Option<HWND>) = Some(hwnd) };
        return BOOL(0);
    }
    BOOL(1)
}

/// The app process behind a window: the CoreWindow child's process for a
/// packaged app's frame, the window's own otherwise. A frame with no
/// CoreWindow attached (an app still starting, or suspended) keeps the host
/// pid and lists as the frame host — honestly, since that is all there is.
fn app_pid_of(hwnd: HWND, host_pid: u32) -> u32 {
    if class_name(hwnd) != FRAME_HOST_CLASS {
        return host_pid;
    }
    let mut core: Option<HWND> = None;
    // SAFETY: the callback writes only into the Option whose address it is
    // given.
    let _ = unsafe {
        EnumChildWindows(
            Some(hwnd),
            Some(find_core_window),
            LPARAM((&raw mut core) as isize),
        )
    };
    core.map_or(host_pid, owner_pid)
}

/// How many monitors the desktop spans.
pub(super) fn monitor_count() -> i32 {
    // SAFETY: a plain metric read.
    unsafe { GetSystemMetrics(SM_CMONITORS) }
}

/// The virtual desktop: every monitor's union, origin possibly negative.
pub(super) fn virtual_screen() -> Rect {
    // SAFETY: plain metric reads.
    unsafe {
        Rect::new(
            f64::from(GetSystemMetrics(SM_XVIRTUALSCREEN)),
            f64::from(GetSystemMetrics(SM_YVIRTUALSCREEN)),
            f64::from(GetSystemMetrics(SM_CXVIRTUALSCREEN)),
            f64::from(GetSystemMetrics(SM_CYVIRTUALSCREEN)),
        )
    }
}

unsafe extern "system" fn collect_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _rect: *mut RECT,
    data: LPARAM,
) -> BOOL {
    // SAFETY: `data` is the Vec this enumeration was started with, alive for
    // the whole call.
    let monitors = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
    monitors.push(monitor);
    BOOL(1)
}

/// The index of the monitor a rectangle mostly sits on, in the system's
/// enumeration order — the macOS `screenIndex`.
pub(super) fn monitor_index(bounds: &Rect) -> Option<usize> {
    let rect = RECT {
        left: bounds.x as i32,
        top: bounds.y as i32,
        right: bounds.max_x() as i32,
        bottom: bounds.max_y() as i32,
    };
    // SAFETY: `rect` outlives the call.
    let monitor = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONULL) };
    if monitor.is_invalid() {
        return None;
    }
    let mut monitors: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into the Vec whose address it is given.
    let enumerated = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect_monitor),
            LPARAM((&raw mut monitors) as isize),
        )
    };
    if !enumerated.as_bool() {
        return None;
    }
    monitors.iter().position(|held| *held == monitor)
}

unsafe extern "system" fn collect_window(hwnd: HWND, data: LPARAM) -> BOOL {
    // SAFETY: `data` is the Vec this enumeration was started with.
    let windows = unsafe { &mut *(data.0 as *mut Vec<HWND>) };
    windows.push(hwnd);
    BOOL(1)
}

fn top_level_handles() -> Vec<HWND> {
    let mut handles: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into the Vec whose address it is given.
    let _ = unsafe { EnumWindows(Some(collect_window), LPARAM((&raw mut handles) as isize)) };
    handles
}

/// A window an agent could mean: visible, not cloaked (a hidden UWP frame,
/// another virtual desktop), not a tool window, not the desktop's own
/// furniture, at least 48px each way.
fn candidate_of(hwnd: HWND) -> Option<WindowCandidate> {
    // SAFETY: plain visibility reads on a handle the enumeration handed over.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() || is_cloaked(hwnd) {
        return None;
    }
    let style = extended_style(hwnd);
    if style.contains(WS_EX_TOOLWINDOW) || style.contains(WS_EX_NOACTIVATE) {
        return None;
    }
    if is_shell_surface(&class_name(hwnd)) {
        return None;
    }
    let bounds = window_rect(hwnd)?;
    if bounds.width < 48.0 || bounds.height < 48.0 {
        return None;
    }
    // SAFETY: a plain state read.
    let is_minimized = unsafe { IsIconic(hwnd) }.as_bool();
    let desktop = virtual_screen();
    let host_pid = owner_pid(hwnd);
    Some(WindowCandidate {
        hwnd: hwnd.0 as isize,
        pid: app_pid_of(hwnd, host_pid),
        host_pid,
        title: title_of(hwnd),
        is_offscreen: !bounds.intersects(&desktop),
        screen_index: monitor_index(&bounds),
        bounds,
        is_minimized,
        topmost: style.contains(WS_EX_TOPMOST),
    })
}

/// Every candidate on the desktop, with the app it belongs to.
pub(super) fn all_top_level() -> Vec<(u32, WindowCandidate)> {
    top_level_handles()
        .into_iter()
        .filter_map(candidate_of)
        .map(|candidate| (candidate.pid, candidate))
        .collect()
}

/// `WindowCapture.candidates(pid:)`: this app's windows, best first — the
/// frames a packaged app draws into included, since their host is not the
/// app.
pub(super) fn candidates(pid: u32) -> Vec<WindowCandidate> {
    let mut found: Vec<WindowCandidate> = top_level_handles()
        .into_iter()
        .filter_map(candidate_of)
        .filter(|candidate| candidate.pid == pid)
        .collect();
    found.sort_by_key(|candidate| std::cmp::Reverse(candidate.score()));
    found
}

/// `WindowCapture.resolve`: an explicit id or index, else the best candidate
/// with a title-hint tiebreak.
pub(super) fn resolve(
    candidates: &[WindowCandidate],
    title_hint: Option<&str>,
    window_id: Option<u64>,
    window_index: Option<usize>,
) -> Option<WindowCandidate> {
    if let Some(window_id) = window_id {
        return candidates
            .iter()
            .find(|candidate| candidate.id() == window_id)
            .cloned();
    }
    if let Some(window_index) = window_index {
        return candidates.get(window_index).cloned();
    }
    let mut ranked: Vec<&WindowCandidate> = candidates.iter().collect();
    ranked.sort_by(|left, right| {
        let left_hint = title_hint.is_some_and(|hint| left.title == hint);
        let right_hint = title_hint.is_some_and(|hint| right.title == hint);
        right_hint
            .cmp(&left_hint)
            .then_with(|| right.score().cmp(&left.score()))
    });
    ranked.first().map(|candidate| (*candidate).clone())
}

/// The foreground window's owner pid and root handle, if any.
pub(super) fn foreground() -> Option<(u32, isize)> {
    // SAFETY: plain handle reads.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    Some((owner_pid(root), root.0 as isize))
}

/// The root window under a screen point, with its owner pid.
pub(super) fn root_at(x: f64, y: f64) -> Option<(u32, isize)> {
    let point = POINT {
        x: x.round() as i32,
        y: y.round() as i32,
    };
    // SAFETY: plain handle reads.
    let hwnd = unsafe { WindowFromPoint(point) };
    if hwnd.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    Some((owner_pid(root), root.0 as isize))
}

/// Whether a root window is the target's own transient: a context menu, a
/// tooltip, a dropdown — a window the target process owns THROUGH the
/// target window. `GetWindow(GW_OWNER)` walks the ownership chain one owner
/// at a time; a chain that reaches the target is the target acting, and one
/// that does not — another document window of the same app — is not. The
/// pid must match too: ownership across processes is not a thing, but the
/// walk is bounded and the pid is the cheaper first question.
pub(super) fn is_owned_by_target(root: HWND, target: Recipient) -> bool {
    if owner_pid(root) != target.owner_pid {
        return false;
    }
    let mut window = root;
    for _ in 0..16 {
        if window.0 as u64 == target.window_id {
            return true;
        }
        // SAFETY: a plain handle read; a window with no owner ends the walk.
        match unsafe { GetWindow(window, GW_OWNER) } {
            Ok(owner) if !owner.0.is_null() => window = owner,
            _ => return false,
        }
    }
    false
}

/// `isTargetWindowFocused`: the foreground root is the target, or belongs to
/// the same process and mostly covers the same rectangle.
pub(super) fn is_focused(target: &WindowCandidate) -> bool {
    let Some((pid, root)) = foreground() else {
        return false;
    };
    if root == target.hwnd {
        return true;
    }
    if pid != target.host_pid {
        return false;
    }
    window_rect(hwnd_from_id(root as u64)).is_some_and(|frame| frame.mostly_covers(&target.bounds))
}

/// `recoverWindow`: un-minimize, bring forward, give the desktop a moment.
/// `SetForegroundWindow` is refused to a process that is not already in the
/// foreground family; attaching to the foreground thread's input queue is
/// the documented way to be allowed, and the attachment is undone at once.
pub(super) fn bring_to_front(target: &WindowCandidate) -> bool {
    let hwnd = target.handle();
    // SAFETY: plain window-state calls on a handle we were given.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            std::thread::sleep(std::time::Duration::from_millis(400));
            return true;
        }
        let foreground = GetForegroundWindow();
        let ours = GetCurrentThreadId();
        let theirs = if foreground.0.is_null() {
            0
        } else {
            owner_thread(foreground)
        };
        let attached =
            theirs != 0 && theirs != ours && AttachThreadInput(ours, theirs, true).as_bool();
        let raised = SetForegroundWindow(hwnd).as_bool();
        if attached {
            let _ = AttachThreadInput(ours, theirs, false);
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
        raised
    }
}
