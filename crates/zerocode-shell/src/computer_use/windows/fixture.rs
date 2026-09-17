//! A disposable native window the functional tests drive: an edit, a push
//! button that counts its clicks, a checkbox, a status line, a password
//! edit whose value no accessibility read may see, a multi-line edit, and a
//! context menu the frame shows on a right-click and closes by itself.
//! Standard Win32 controls, so UI Automation sees them through the same
//! proxies it uses for every classic app, and nothing of the person's is
//! touched.
//!
//! The frame pumps its messages through `IsDialogMessage`, the way a dialog
//! does: the Tab KEY moves focus between the controls and the Enter KEY
//! presses the default button (the OK button, which counts). That is what
//! makes the literal-text tests mean something — typed text that moved the
//! focus or pressed OK would show here.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, ClientToScreen, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BM_GETCHECK, BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyMenu, DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL,
    ES_AUTOVSCROLL, ES_MULTILINE, ES_PASSWORD, ES_WANTRETURN, EndMenu, GUITHREADINFO, GetDlgCtrlID,
    GetDlgItem, GetGUIThreadInfo, GetMessageW, GetWindowTextW, HMENU, IsDialogMessageW, KillTimer,
    LB_ADDSTRING, LB_INITSTORAGE, LBS_NOINTEGRALHEIGHT, MF_STRING, MSG, PostMessageW,
    PostQuitMessage, RegisterClassW, SendMessageW, SetForegroundWindow, SetTimer, SetWindowTextW,
    TPM_LEFTALIGN, TPM_RETURNCMD, TrackPopupMenu, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_TIMER, WNDCLASSW, WS_BORDER, WS_CHILD,
    WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use super::dpi::ThreadDpiAwareness;

pub(super) const EDIT_ID: i32 = 101;
pub(super) const BUTTON_ID: i32 = 102;
pub(super) const CHECKBOX_ID: i32 = 103;
pub(super) const STATUS_ID: i32 = 104;
pub(super) const PASSWORD_ID: i32 = 105;
pub(super) const LIST_ID: i32 = 106;
pub(super) const MULTILINE_ID: i32 = 107;

/// The button's client rectangle, for coordinate clicks.
pub(super) const BUTTON_RECT: (i32, i32, i32, i32) = (20, 70, 100, 32);
/// A client point on the frame itself, away from every control — where a
/// coordinate right-click reaches the frame's own context menu.
pub(super) const BARE_FRAME_POINT: (i32, i32) = (400, 250);

/// The timer that closes the fixture's context menu from inside the menu's
/// own modal loop, so a test never has to.
const MENU_CLOSE_TIMER: usize = 1;

static CLICKS: AtomicU32 = AtomicU32::new(0);
static MENUS: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: standard window-procedure calls on the fixture's own windows.
    unsafe {
        match msg {
            WM_COMMAND => {
                let id = (wparam.0 & 0xffff) as i32;
                let code = ((wparam.0 >> 16) & 0xffff) as u32;
                if id == BUTTON_ID && code == BN_CLICKED {
                    CLICKS.fetch_add(1, Ordering::SeqCst);
                    if let Ok(status) = GetDlgItem(Some(hwnd), STATUS_ID) {
                        let _ = SetWindowTextW(status, w!("clicked"));
                    }
                }
                LRESULT(0)
            }
            // A right-click (or `ShowContextMenu`) on the frame or any of
            // its controls: count it and show a real popup menu at the
            // pointer, the way an app does — it is a top-level `#32768`
            // window owned by this frame, which is what the click fence
            // must recognise as the target's own. The menu closes itself
            // from a timer that fires inside `TrackPopupMenu`'s modal loop.
            WM_CONTEXTMENU => {
                MENUS.fetch_add(1, Ordering::SeqCst);
                let x = (lparam.0 & 0xffff) as i16 as i32;
                let y = ((lparam.0 >> 16) & 0xffff) as i16 as i32;
                if let Ok(menu) = CreatePopupMenu() {
                    let _ = AppendMenuW(menu, MF_STRING, 1, w!("Fixture item"));
                    SetTimer(Some(hwnd), MENU_CLOSE_TIMER, 250, None);
                    let _ =
                        TrackPopupMenu(menu, TPM_LEFTALIGN | TPM_RETURNCMD, x, y, None, hwnd, None);
                    let _ = KillTimer(Some(hwnd), MENU_CLOSE_TIMER);
                    let _ = DestroyMenu(menu);
                }
                LRESULT(0)
            }
            WM_TIMER if wparam.0 == MENU_CLOSE_TIMER => {
                let _ = KillTimer(Some(hwnd), MENU_CLOSE_TIMER);
                let _ = EndMenu();
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(super) struct Fixture {
    pub hwnd: isize,
    pub pid: u32,
    /// The fixture thread, whose focus `GetGUIThreadInfo` reports.
    thread_id: u32,
    thread: Option<JoinHandle<()>>,
}

impl Fixture {
    /// Stand the window up on its own thread and wait until it exists.
    pub fn launch(title: &str) -> Self {
        Self::launch_with_rows(title, 0)
    }

    /// The same window with a list box of `rows` rows beside the controls —
    /// the large-tree fixture (`rows` in the thousands) the truncation and
    /// acquisition budget are proved against.
    pub fn launch_with_rows(title: &str, rows: usize) -> Self {
        let (ready, readiness) = mpsc::channel::<(isize, u32)>();
        let title = wide(title);
        let thread = std::thread::Builder::new()
            .name("computer-use-fixture".into())
            .spawn(move || {
                // The fixture speaks the provider's coordinate contract:
                // physical pixels, whatever the test process's default is.
                let _dpi = ThreadDpiAwareness::per_monitor_v2();
                // SAFETY: the fixture thread owns every window it creates and
                // pumps their messages until the frame closes.
                unsafe {
                    let instance = HINSTANCE(
                        GetModuleHandleW(None)
                            .map(|module| module.0)
                            .unwrap_or(std::ptr::null_mut()),
                    );
                    let class_name = w!("ZeroCodeComputerUseFixture");
                    let class = WNDCLASSW {
                        lpfnWndProc: Some(wndproc),
                        hInstance: instance,
                        lpszClassName: class_name,
                        hbrBackground: HBRUSH(
                            (COLOR_WINDOW.0 + 1) as usize as *mut core::ffi::c_void,
                        ),
                        ..Default::default()
                    };
                    // Already registered by an earlier fixture in this process
                    // is fine: the class is the same.
                    let _ = RegisterClassW(&class);
                    let frame = CreateWindowExW(
                        WINDOW_EX_STYLE(0),
                        class_name,
                        PCWSTR(title.as_ptr()),
                        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                        120,
                        120,
                        520,
                        360,
                        None,
                        None,
                        Some(instance),
                        None,
                    )
                    .expect("fixture frame");
                    let child = |class: PCWSTR,
                                 text: PCWSTR,
                                 style: u32,
                                 x: i32,
                                 y: i32,
                                 width: i32,
                                 height: i32,
                                 id: i32| {
                        CreateWindowExW(
                            WINDOW_EX_STYLE(0),
                            class,
                            text,
                            WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(style),
                            x,
                            y,
                            width,
                            height,
                            Some(frame),
                            Some(HMENU(id as usize as *mut core::ffi::c_void)),
                            Some(instance),
                            None,
                        )
                        .expect("fixture control")
                    };
                    child(
                        w!("EDIT"),
                        w!(""),
                        WS_BORDER.0 | ES_AUTOHSCROLL as u32,
                        20,
                        20,
                        320,
                        28,
                        EDIT_ID,
                    );
                    child(
                        w!("BUTTON"),
                        w!("OK"),
                        BS_DEFPUSHBUTTON as u32,
                        BUTTON_RECT.0,
                        BUTTON_RECT.1,
                        BUTTON_RECT.2,
                        BUTTON_RECT.3,
                        BUTTON_ID,
                    );
                    child(
                        w!("BUTTON"),
                        w!("Enable"),
                        BS_AUTOCHECKBOX as u32,
                        140,
                        70,
                        160,
                        32,
                        CHECKBOX_ID,
                    );
                    child(w!("STATIC"), w!("ready"), 0, 20, 120, 400, 28, STATUS_ID);
                    child(
                        w!("EDIT"),
                        w!(""),
                        WS_BORDER.0
                            | ES_MULTILINE as u32
                            | ES_WANTRETURN as u32
                            | ES_AUTOVSCROLL as u32,
                        20,
                        200,
                        320,
                        90,
                        MULTILINE_ID,
                    );
                    child(
                        w!("EDIT"),
                        w!(""),
                        WS_BORDER.0 | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
                        20,
                        160,
                        320,
                        28,
                        PASSWORD_ID,
                    );
                    if rows > 0 {
                        let list = child(
                            w!("LISTBOX"),
                            w!(""),
                            WS_BORDER.0 | WS_VSCROLL.0 | LBS_NOINTEGRALHEIGHT as u32,
                            360,
                            20,
                            140,
                            280,
                            LIST_ID,
                        );
                        let _ = SendMessageW(
                            list,
                            LB_INITSTORAGE,
                            Some(WPARAM(rows)),
                            Some(LPARAM((rows * 16) as isize)),
                        );
                        for row in 0..rows {
                            let text = wide(&format!("row {row}"));
                            let _ = SendMessageW(
                                list,
                                LB_ADDSTRING,
                                None,
                                Some(LPARAM(text.as_ptr() as isize)),
                            );
                        }
                    }
                    let _ = SetForegroundWindow(frame);
                    let _ = ready.send((frame.0 as isize, GetCurrentThreadId()));
                    let mut message = MSG::default();
                    while GetMessageW(&mut message, None, 0, 0).as_bool() {
                        // A dialog's loop: Tab navigates, Enter presses the
                        // default button, and only what is left reaches
                        // the controls as characters.
                        if IsDialogMessageW(frame, &message).as_bool() {
                            continue;
                        }
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
            })
            .expect("fixture thread");
        let (hwnd, thread_id) = readiness
            .recv_timeout(Duration::from_secs(10))
            .expect("the fixture window did not appear");
        Self {
            hwnd,
            pid: std::process::id(),
            thread_id,
            thread: Some(thread),
        }
    }

    pub fn handle(&self) -> HWND {
        HWND(self.hwnd as usize as *mut core::ffi::c_void)
    }

    /// The DPI the fixture window is shown at (96 = 100%).
    pub fn dpi(&self) -> u32 {
        // SAFETY: a plain query on the fixture's own window.
        unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.handle()) }
    }

    /// `--app pid:N` for this process.
    pub fn app_query(&self) -> String {
        format!("pid:{}", self.pid)
    }

    fn control(&self, id: i32) -> HWND {
        // SAFETY: a lookup on the fixture's own frame.
        unsafe { GetDlgItem(Some(self.handle()), id) }.expect("fixture control")
    }

    fn text_of(&self, id: i32) -> String {
        let mut buffer = [0u16; 1024];
        // SAFETY: the buffer outlives the call; the API bounds itself by it.
        let length = unsafe { GetWindowTextW(self.control(id), &mut buffer) };
        String::from_utf16_lossy(&buffer[..length.max(0) as usize])
    }

    pub fn edit_text(&self) -> String {
        self.text_of(EDIT_ID)
    }

    /// What the password edit holds — readable here because the fixture
    /// owns the control; no accessibility client may read it.
    pub fn password_text(&self) -> String {
        self.text_of(PASSWORD_ID)
    }

    pub fn multiline_text(&self) -> String {
        self.text_of(MULTILINE_ID)
    }

    /// The control id that holds keyboard focus on the fixture thread, or
    /// 0 when none of the fixture's controls does.
    pub fn focused_control_id(&self) -> i32 {
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: a query on the fixture's own thread into a struct sized
        // for it; the focus handle it names is one of the fixture's.
        unsafe {
            if GetGUIThreadInfo(self.thread_id, &mut info).is_err() || info.hwndFocus.0.is_null() {
                return 0;
            }
            GetDlgCtrlID(info.hwndFocus)
        }
    }

    pub fn set_password_text(&self, text: &str) {
        let text = wide(text);
        // SAFETY: a text write on the fixture's own control.
        let _ = unsafe { SetWindowTextW(self.control(PASSWORD_ID), PCWSTR(text.as_ptr())) };
    }

    pub fn status_text(&self) -> String {
        self.text_of(STATUS_ID)
    }

    pub fn checkbox_checked(&self) -> bool {
        // SAFETY: a state query on the fixture's own checkbox.
        let state = unsafe { SendMessageW(self.control(CHECKBOX_ID), BM_GETCHECK, None, None) };
        state.0 == 1
    }

    pub fn clicks() -> u32 {
        CLICKS.load(Ordering::SeqCst)
    }

    /// How many context menus the frame has shown.
    pub fn menus() -> u32 {
        MENUS.load(Ordering::SeqCst)
    }

    /// A point on the bare frame, in screen pixels.
    pub fn bare_frame_point_screen(&self) -> (f64, f64) {
        self.client_to_screen(BARE_FRAME_POINT.0, BARE_FRAME_POINT.1)
    }

    fn client_to_screen(&self, x: i32, y: i32) -> (f64, f64) {
        let mut point = POINT { x, y };
        // SAFETY: converts a point of the fixture's own client area.
        let _ = unsafe { ClientToScreen(self.handle(), &mut point) };
        (f64::from(point.x), f64::from(point.y))
    }

    /// The button's centre in screen pixels.
    pub fn button_center_screen(&self) -> (f64, f64) {
        self.client_to_screen(
            BUTTON_RECT.0 + BUTTON_RECT.2 / 2,
            BUTTON_RECT.1 + BUTTON_RECT.3 / 2,
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // SAFETY: asks the fixture thread to close its own frame, then waits.
        let _ = unsafe { PostMessageW(Some(self.handle()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
