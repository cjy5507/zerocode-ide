//! Embedded Chromium runtime for the macOS browser pane.
//!
//! CEF objects stay on the AppKit/CEF UI thread. `BrowserPane` is the Send-safe
//! label handle stored by the rest of the shell; every operation resolves that
//! label in the thread-local native registry.

mod macos_app;
pub(crate) mod shield;

use base64::Engine as _;
use cef::*;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::CString,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    AppState, ShellStateExt,
    browser_cookie_import::{self, Dropped, SetOutcome, StagedCookie},
    browser_cookies::SameSite,
    browser_runtime::{BrowserDownload, BrowserNav, BrowserTitle, browsable_target},
};

const FRAMEWORK: &str = "Chromium Embedded Framework.framework/Chromium Embedded Framework";
const HELPER_APP: &str = "ZeroCode Helper.app/Contents/MacOS/ZeroCode Helper";
const UI_WAIT: Duration = Duration::from_secs(10);
const DEVTOOLS_DEADLINE: Duration = Duration::from_secs(30);
/// What the hang watchdog names the main thread while it runs Chromium's work
/// (`crumbs::MainScope`). Without a name that time reads `native_event_loop`,
/// which is also what WebKit's and AppKit's own work reads, so a main thread
/// held by the message pump, the engine's start, a pane operation or a cookie
/// flush could not be told from any other native stretch (the 7.4 s hang of
/// 2026-10-01 17:28 was filed as `main_scope=native_event_loop`).
const SCOPE_PUMP: &str = "cef_message_loop";
const SCOPE_START: &str = "cef_initialize";
const SCOPE_OPERATION: &str = "cef_pane_operation";
const SCOPE_COOKIES: &str = "cef_cookie_flush";
/// Chromium's cookie key normally lives in the login keychain under one
/// generic "Chromium Safe Storage" item shared with every Chromium-based app
/// on the machine. macOS lets a caller read it silently only when the
/// caller's Team ID is on the item's partition list; a locally signed
/// ZeroCode has no Team ID, so each BUILD is a fresh `cdhash:` partition and
/// every launch asked again — four `always allow` approvals in one day, two
/// of them inside a single process (securityd kcacl log, 2026-09-09). With
/// the mock keychain Chromium keeps the key beside the profile: no prompt,
/// and no sharing another app's item. Cookies at rest are then guarded by the
/// profile directory's own permissions, as Linux's basic store already is. A
/// Developer ID build carries a Team ID and may drop this switch.
const CEF_SWITCH_USE_MOCK_KEYCHAIN: &str = "use-mock-keychain";
/// The Chromium features the built-in browser turns off, as the
/// `--disable-features` switch carries them. `HttpsUpgrades` (and the balanced
/// HTTPS-First mode that rides it) silently rewrites a typed `http://` address
/// to `https://` and only falls back when the secure connection is REFUSED —
/// an intranet admin whose :443 is dropped by a firewall (measured 2026-09-15
/// against one such company admin: plain http answered 200 in 33 ms while the
/// pane `ERR_TIMED_OUT` on the https it invented) never falls back and
/// never loads. This browser exists for exactly those signed-in company pages,
/// so the address a person types is the address the pane loads.
///
/// `WebBluetooth` is off as well (t-45873). One road that opens the Bluetooth
/// adapter is a page asking for a Bluetooth device, and the adapter reads the
/// devices paired with this Mac, the call macOS ends the window for without a
/// usage description. With the feature off, pages do not get
/// `navigator.bluetooth`. CEF 152 has no `disable-bluetooth` switch (checked
/// against the framework's strings), so this list is where the feature goes.
/// The trigger of the 2026-10-11 crash was not traced; the usage description
/// covers any other road that reaches the adapter.
///
/// `MacAppCodeSignClone` is deliberately NOT in this list (t-20243). At every
/// start Chromium copies the whole app bundle into the per-user `X` folder
/// (`<bundle id>.code_sign_clone`) and hard-links the running executable into
/// it, and deletes the copy only after an orderly shutdown — the restart road
/// (`exit(0)`), a crash and a force quit leave it: 0.4 GiB each. But that hard
/// link is a second name for the executable, and without one macOS no longer
/// recognises a window whose installed bundle an update has renamed away and
/// deleted (`codesign --verify +pid`: "host has no guest with the requested
/// attributes") — which is every window that outlives two installs. The
/// copies nothing runs are swept instead, once the window has painted
/// (`code_sign_clone::sweep_after_first_paint`).
const CEF_SWITCH_DISABLE_FEATURES: &str = "disable-features";
const CEF_DISABLED_FEATURES: &str = "HttpsUpgrades,HttpsFirstBalancedMode,WebBluetooth";
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static EXIT_PENDING: AtomicBool = AtomicBool::new(false);
static LOADED: AtomicBool = AtomicBool::new(false);
static NEXT_DEVTOOLS_ID: AtomicI32 = AtomicI32::new(1);
static OPEN_BROWSERS: AtomicUsize = AtomicUsize::new(0);
static PUMP: OnceLock<Arc<PumpScheduler>> = OnceLock::new();
/// Where the engine's profile root is, kept from boot until the engine is
/// started. Starting Chromium costs the main thread 0.3 s on an ordinary
/// machine and about 1.5 s on a low-spec one (t-20078), and nothing on the
/// first screen needs it, so the window starts it after its first paint — or
/// at the first browser pane, whichever comes first.
static DEFERRED_ROOT: OnceLock<PathBuf> = OnceLock::new();

pub(crate) fn install_application() -> Result<(), String> {
    let framework = framework_path()?;
    let path = CString::new(framework.as_os_str().to_string_lossy().as_bytes())
        .map_err(|_| "CEF framework path contains a NUL byte".to_string())?;
    let loaded = unsafe { load_library(Some(&*path.as_ptr())) };
    if loaded != 1 {
        return Err(format!(
            "CEF framework를 로드하지 못했습니다: {}",
            framework.display()
        ));
    }
    LOADED.store(true, Ordering::Release);
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);
    Ok(())
}

fn packaged_framework(executable: &Path) -> Option<PathBuf> {
    executable
        .parent()?
        .join("../Frameworks")
        .join(FRAMEWORK)
        .canonicalize()
        .ok()
}

fn framework_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    packaged_framework(&executable)
        .or_else(|| cef::sys::get_cef_dir().map(|root| root.join(FRAMEWORK)))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            "Chromium Embedded Framework를 찾을 수 없습니다. 앱 번들을 다시 설치하거나 CEF_PATH를 확인하세요."
                .to_string()
        })
}

fn helper_path() -> Result<PathBuf, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    executable
        .parent()
        .map(|dir| dir.join("../Frameworks").join(HELPER_APP))
        .filter(|path| path.is_file())
        .ok_or_else(|| {
            "Chromium helper 앱이 Contents/Frameworks에 없습니다. 패키징 검사를 다시 실행하세요."
                .to_string()
        })
}

struct PumpState {
    deadline: Option<Instant>,
    stopped: bool,
}

struct PumpScheduler {
    app: AppHandle,
    state: Mutex<PumpState>,
    wake: Condvar,
    queued_on_main: AtomicBool,
    in_work: AtomicBool,
}

impl PumpScheduler {
    fn start(app: AppHandle) -> Arc<Self> {
        let scheduler = Arc::new(Self {
            app,
            state: Mutex::new(PumpState {
                deadline: None,
                stopped: false,
            }),
            wake: Condvar::new(),
            queued_on_main: AtomicBool::new(false),
            in_work: AtomicBool::new(false),
        });
        let worker = scheduler.clone();
        std::thread::Builder::new()
            .name("cef-message-pump".into())
            .spawn(move || worker.worker())
            .expect("start CEF message scheduler");
        scheduler
    }

    fn schedule(self: &Arc<Self>, delay_ms: i64) {
        let delay = Duration::from_millis(delay_ms.clamp(0, 1_000) as u64);
        let deadline = Instant::now() + delay;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.stopped {
            return;
        }
        if state.deadline.is_none_or(|current| deadline < current) {
            state.deadline = Some(deadline);
            self.wake.notify_one();
        }
    }

    fn worker(self: Arc<Self>) {
        loop {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            while !state.stopped && state.deadline.is_none() {
                let (next, _) = self
                    .wake
                    .wait_timeout(state, Duration::from_millis(15))
                    .unwrap_or_else(|error| error.into_inner());
                state = next;
                if state.deadline.is_none() && INITIALIZED.load(Ordering::Acquire) {
                    break;
                }
            }
            if state.stopped {
                return;
            }
            let now = Instant::now();
            if let Some(deadline) = state.deadline
                && deadline > now
            {
                let (next, _) = self
                    .wake
                    .wait_timeout(state, deadline - now)
                    .unwrap_or_else(|error| error.into_inner());
                drop(next);
                continue;
            }
            state.deadline = None;
            drop(state);
            if self.queued_on_main.swap(true, Ordering::AcqRel) {
                self.schedule(15);
                continue;
            }
            let scheduler = self.clone();
            if self
                .app
                .run_on_main_thread(move || {
                    scheduler.queued_on_main.store(false, Ordering::Release);
                    if scheduler.in_work.swap(true, Ordering::AcqRel) {
                        scheduler.schedule(0);
                        return;
                    }
                    if INITIALIZED.load(Ordering::Acquire) {
                        let _scope = crate::crumbs::MainScope::enter(SCOPE_PUMP);
                        objc2::rc::autoreleasepool(|_| cef::do_message_loop_work());
                    }
                    scheduler.in_work.store(false, Ordering::Release);
                })
                .is_err()
            {
                self.queued_on_main.store(false, Ordering::Release);
            }
        }
    }

    fn stop(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.stopped = true;
        state.deadline = None;
        self.wake.notify_all();
    }
}

wrap_browser_process_handler! {
    struct ChromiumBrowserProcessHandler {
        scheduler: Arc<PumpScheduler>,
    }
    impl BrowserProcessHandler {
        fn on_schedule_message_pump_work(&self, delay_ms: i64) {
            self.scheduler.schedule(delay_ms);
        }
    }
}

wrap_app! {
    struct ChromiumCefApp {
        scheduler: Arc<PumpScheduler>,
    }
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> {
            Some(ChromiumBrowserProcessHandler::new(self.scheduler.clone()))
        }
        // Every process type gets the switch; only the browser process reads
        // the cookie key, and Chromium forwards its own switches to helpers.
        fn on_before_command_line_processing(
            &self,
            _process_type: Option<&CefString>,
            command_line: Option<&mut CommandLine>,
        ) {
            if let Some(command_line) = command_line {
                command_line.append_switch(Some(&CefString::from(CEF_SWITCH_USE_MOCK_KEYCHAIN)));
                command_line.append_switch_with_value(
                    Some(&CefString::from(CEF_SWITCH_DISABLE_FEATURES)),
                    Some(&CefString::from(CEF_DISABLED_FEATURES)),
                );
            }
        }
    }
}

/// Remember where the engine will live, without starting it.
pub(crate) fn defer_initialize(root_cache: &Path) {
    let _ = DEFERRED_ROOT.set(root_cache.to_path_buf());
}

/// Start the engine if it has not been started and a root was deferred. Main
/// thread only (CEF's own rule); idempotent, so the paint's call and a pane's
/// call cannot start it twice.
pub(crate) fn ensure_initialized(app: &AppHandle) -> Result<(), String> {
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    let root = DEFERRED_ROOT
        .get()
        .ok_or_else(|| "Chromium has no profile root to start in".to_string())?;
    let started = initialize(app, root);
    crate::boot_timeline::mark(crate::boot_timeline::Phase::ChromiumReady);
    started
}

/// After the first paint: queue the engine's start on the main thread, behind
/// whatever the paint left there.
pub(crate) fn initialize_after_paint(app: &AppHandle) {
    let starting = app.clone();
    let _ = app.run_on_main_thread(move || {
        let _scope = crate::crumbs::MainScope::enter(SCOPE_START);
        if let Err(error) = ensure_initialized(&starting) {
            eprintln!("zerocode-shell: Chromium을 시작할 수 없습니다: {error}");
        }
    });
}

fn initialize(app: &AppHandle, root_cache: &Path) -> Result<(), String> {
    if INITIALIZED.load(Ordering::Acquire) {
        return Ok(());
    }
    let framework = framework_path()?;
    if !LOADED.load(Ordering::Acquire) {
        return Err("CEF framework was not loaded before Tauri initialization".to_string());
    }
    macos_app::install()?;

    std::fs::create_dir_all(root_cache)
        .map_err(|error| format!("Chromium profile root를 만들지 못했습니다: {error}"))?;
    let root_cache = root_cache
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let helper = helper_path()?;
    let args = cef::args::Args::new();
    let scheduler = PumpScheduler::start(app.clone());
    let mut cef_app = ChromiumCefApp::new(scheduler.clone());
    let settings = Settings {
        no_sandbox: 0,
        browser_subprocess_path: CefString::from(helper.to_string_lossy().as_ref()),
        framework_dir_path: CefString::from(
            framework
                .parent()
                .unwrap_or(&framework)
                .to_string_lossy()
                .as_ref(),
        ),
        external_message_pump: 1,
        multi_threaded_message_loop: 0,
        windowless_rendering_enabled: 0,
        root_cache_path: CefString::from(root_cache.to_string_lossy().as_ref()),
        cache_path: CefString::from(root_cache.to_string_lossy().as_ref()),
        persist_session_cookies: 1,
        remote_debugging_port: 0,
        ..Default::default()
    };
    if cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut cef_app),
        std::ptr::null_mut(),
    ) != 1
    {
        let _ = unload_library();
        scheduler.stop();
        return Err("CEF browser process initialization failed".to_string());
    }
    let _ = PUMP.set(scheduler);
    INITIALIZED.store(true, Ordering::Release);
    Ok(())
}

pub(crate) fn defer_exit(app: &AppHandle, code: i32) -> bool {
    if !INITIALIZED.load(Ordering::Acquire)
        || OPEN_BROWSERS.load(Ordering::Acquire) == 0
        || EXIT_PENDING.swap(true, Ordering::AcqRel)
    {
        return false;
    }
    let panes: Vec<NativePane> =
        PANES.with(|panes| panes.borrow_mut().drain().map(|(_, pane)| pane).collect());
    for mut pane in panes {
        pane.fail_pending("Chromium is shutting down");
        if let Some(host) = pane.browser.host() {
            host.close_browser(1);
        }
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(5);
        while OPEN_BROWSERS.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        app.exit(code);
    });
    true
}

pub(crate) fn shutdown() {
    if !INITIALIZED.swap(false, Ordering::AcqRel) {
        return;
    }
    let panes: Vec<NativePane> =
        PANES.with(|panes| panes.borrow_mut().drain().map(|(_, pane)| pane).collect());
    for mut pane in panes {
        pane.fail_pending("Chromium is shutting down");
        if let Some(host) = pane.browser.host() {
            host.close_browser(1);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while OPEN_BROWSERS.load(Ordering::Acquire) != 0 && Instant::now() < deadline {
        objc2::rc::autoreleasepool(|_| cef::do_message_loop_work());
        std::thread::sleep(Duration::from_millis(10));
    }
    if let Some(pump) = PUMP.get() {
        pump.stop();
    }
    let remaining = OPEN_BROWSERS.load(Ordering::Acquire);
    if remaining != 0 {
        eprintln!(
            "zerocode-shell: Chromium 종료 제한시간 안에 닫히지 않은 브라우저 {remaining}개; 안전을 위해 CEF를 언로드하지 않습니다"
        );
        return;
    }
    cef::shutdown();
    let _ = unload_library();
    LOADED.store(false, Ordering::Release);
}

#[derive(Clone)]
pub(crate) struct BrowserPane {
    app: AppHandle,
    label: String,
}

impl BrowserPane {
    pub(crate) fn new(app: &AppHandle, label: String) -> Self {
        Self {
            app: app.clone(),
            label,
        }
    }

    pub(crate) fn exists(&self) -> bool {
        on_ui(&self.app, &self.label, |_| Ok(())).is_ok()
    }

    pub(crate) fn eval(&self, script: impl Into<String>) -> Result<(), String> {
        let script = script.into();
        on_ui(&self.app, &self.label, move |pane| {
            let frame = pane
                .browser
                .main_frame()
                .ok_or_else(|| "Chromium main frame is unavailable".to_string())?;
            frame.execute_java_script(
                Some(&CefString::from(script.as_str())),
                Some(&CefString::from("zerocode://browser-eval")),
                1,
            );
            Ok(())
        })
    }

    pub(crate) fn eval_with_callback<F>(
        &self,
        script: impl Into<String>,
        callback: F,
    ) -> Result<(), String>
    where
        F: FnOnce(String) + Send + 'static,
    {
        let script = script.into();
        let callback: EvalCallback = Box::new(callback);
        on_ui(&self.app, &self.label, move |pane| {
            pane.devtools(
                "Runtime.evaluate",
                json!({
                    "expression": script,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "userGesture": false
                }),
                Pending::Eval(callback),
            )
        })
    }

    pub(crate) fn navigate_known(
        &self,
        url: tauri::Url,
        number: u64,
        check: crate::browser_workspace::EvidenceCheck,
    ) -> Result<(), crate::cmd::browser::BrowserRefusal> {
        if !browsable_target(&url) {
            crate::cmd::browser::navigation_withdrawn(
                &self.app.state::<AppState>(),
                &self.label,
                number,
            );
            return Err(crate::cmd::browser::BrowserRefusal::Unsent(
                "허용되지 않은 브라우저 주소입니다".to_string(),
            ));
        }
        on_ui(&self.app, &self.label, move |pane| {
            Ok(pane.send_navigation(url.as_str(), number, &check))
        })
        .map_err(crate::cmd::browser::BrowserRefusal::Sent)?
    }

    pub(crate) fn settle_navigation(&self) -> Result<crate::NavProof, String> {
        on_ui(&self.app, &self.label, |pane| {
            let loading = pane.browser.is_loading() == 1;
            Ok(crate::cmd::browser::navigation_settled(
                &pane.app.state::<AppState>(),
                &pane.label,
                loading,
            ))
        })
    }

    pub(crate) fn navigate_after_cookies(
        &self,
        profile_id: String,
        config_root: PathBuf,
        cookies: Vec<StagedCookie>,
        url: tauri::Url,
        number: u64,
    ) -> Result<(), String> {
        if cookies.is_empty() {
            return self
                .navigate_known(url, number, Default::default())
                .map_err(String::from);
        }
        // 저장소가 몇 번을 물어도 같은 답을 줄 쿠키는 시도하지 않는다 —
        // 버린 수는 마지막 주입 결과에 함께 적힌다.
        let (cookies, dropped) =
            browser_cookie_import::keep_acceptable(cookies, browser_cookie_import::now_unix());
        if cookies.is_empty() {
            // 앉힐 것이 하나도 남지 않았다 — 붙들고 있어도 다음 판이 받을
            // 답은 같다.
            let settled = browser_cookie_import::settle_staging(
                &config_root,
                &profile_id,
                &[],
                &[],
                dropped,
                true,
                browser_cookie_import::now_unix(),
            );
            crate::note_window_event(
                self.app.state::<AppState>().local_data_root(),
                &settled.log_line(&profile_id),
            );
            return self
                .navigate_known(url, number, Default::default())
                .map_err(String::from);
        }
        let app = self.app.clone();
        let label = self.label.clone();
        on_ui(&self.app, &self.label, move |pane| {
            let manager = pane
                .request_context
                .cookie_manager(None)
                .ok_or_else(|| "Chromium cookie manager is unavailable".to_string())?;
            let batch = Arc::new(CookieBatch {
                remaining: AtomicUsize::new(cookies.len()),
                outcomes: Mutex::new(vec![SetOutcome::Unsent; cookies.len()]),
                flushed: AtomicBool::new(true),
                cookies,
                dropped,
                app,
                label,
                url,
                number,
                config_root,
                profile_id,
            });
            for (index, staged) in batch.cookies.iter().enumerate() {
                // 주소를 지을 수 없는 쿠키는 저장소에 물을 것도 없이 받지 않은 것이다.
                let Ok(address) = cookie_url(staged) else {
                    batch.finished(index, SetOutcome::Refused);
                    continue;
                };
                let cookie = cef_cookie(staged);
                let mut callback = CookieSetCallback::new(batch.clone(), index);
                if manager.set_cookie(
                    Some(&CefString::from(address.as_str())),
                    Some(&cookie),
                    Some(&mut callback),
                ) != 1
                {
                    batch.finished(index, SetOutcome::Unsent);
                }
            }
            Ok(())
        })
    }

    pub(crate) fn reload(&self) -> Result<(), String> {
        on_ui(&self.app, &self.label, |pane| {
            pane.browser.reload();
            Ok(())
        })
    }

    pub(crate) fn stop(&self) -> Result<(), String> {
        on_ui(&self.app, &self.label, |pane| {
            pane.browser.stop_load();
            Ok(())
        })
    }

    pub(crate) fn go_back(&self) -> Result<(), String> {
        on_ui(&self.app, &self.label, |pane| {
            pane.browser.go_back();
            Ok(())
        })
    }

    pub(crate) fn go_forward(&self) -> Result<(), String> {
        on_ui(&self.app, &self.label, |pane| {
            pane.browser.go_forward();
            Ok(())
        })
    }

    pub(crate) fn set_zoom(&self, factor: f64) -> Result<(), String> {
        let level = zoom_level(factor)?;
        on_ui(&self.app, &self.label, move |pane| {
            pane.browser
                .host()
                .ok_or_else(|| "Chromium browser host is unavailable".to_string())?
                .set_zoom_level(level);
            Ok(())
        })
    }

    pub(crate) fn toggle_devtools(&self) -> Result<(), String> {
        on_ui(&self.app, &self.label, |pane| {
            let host = pane
                .browser
                .host()
                .ok_or_else(|| "Chromium browser host is unavailable".to_string())?;
            if host.has_dev_tools() == 1 {
                host.close_dev_tools();
            } else {
                let mut client = BrowserDevToolsClient::new();
                host.show_dev_tools(None, Some(&mut client), None, None);
            }
            Ok(())
        })
    }

    /// Place the pane where the window says, shown only as its tab's state
    /// allows at this very operation: for the person's hand, or — when they
    /// watch it (`watch`, the shield's dress) — for their eyes only, under the
    /// shield (t-66513). Only a pane in their hands keeps the keyboard.
    pub(crate) fn place(
        &self,
        bounds: [f64; 4],
        shown: bool,
        watch: Option<shield::Dress>,
        revision: Option<u64>,
        main_webview: tauri::Webview,
    ) -> Result<(), String> {
        let app = self.app.clone();
        let label = self.label.clone();
        on_ui(&self.app, &self.label, move |pane| {
            let view = pane.native_view()?;
            let owned = unsafe { crate::browser_runtime::browser_view_owns_first_responder(view) };
            pane.set_hidden(true)?;
            let mut displayed = None;
            let watching = watch.is_some();
            let result = app
                .state::<AppState>()
                .shell_runtime()
                .browser_workspaces
                .with_placement(&label, revision, watching, |placement| {
                    let mut stage = Staged {
                        pane,
                        dress: watch.as_ref(),
                    };
                    if crate::browser_workspace::stage_pane(&mut stage, placement, bounds, shown)? {
                        displayed = Some(placement);
                    } else if shown {
                        return Err(
                            "workspace_paused: 직접 조작은 제어권 전환 후 가능합니다".to_string()
                        );
                    }
                    Ok(())
                });
            if (displayed != Some(crate::browser_workspace::Placement::Control) || result.is_err())
                && owned
            {
                let _ = main_webview.set_focus();
            }
            result
        })
    }

    pub(crate) fn hide(&self, main_webview: tauri::Webview) -> Result<(), String> {
        on_ui(&self.app, &self.label, move |pane| {
            let view = pane.native_view()?;
            let owned = unsafe { crate::browser_runtime::browser_view_owns_first_responder(view) };
            pane.set_hidden(true)?;
            pane.shield.lower();
            if owned {
                let _ = main_webview.set_focus();
            }
            Ok(())
        })
    }

    /// The agent pressed in this pane (t-66513): the shield moves its mark to
    /// the pressed element's centre — a hop to the main thread nobody waits
    /// for, so a press never waits on a drawing.
    pub(crate) fn mark_press(&self, rect: [f64; 4], dpr: f64) {
        let label = self.label.clone();
        let _ = self.app.run_on_main_thread(move || {
            let _scope = crate::crumbs::MainScope::enter(SCOPE_OPERATION);
            PANES.with(|panes| {
                if let Ok(mut panes) = panes.try_borrow_mut()
                    && let Some(pane) = panes.get_mut(&label)
                {
                    pane.shield.pressed(rect, dpr);
                }
            });
        });
    }

    pub(crate) fn close(&self) -> Result<(), String> {
        let label = self.label.clone();
        run_ui(&self.app, move || {
            let mut pane = PANES.with(|panes| panes.borrow_mut().remove(&label));
            let Some(mut pane) = pane.take() else {
                return Err("브라우저 판이 이미 닫혔습니다".to_string());
            };
            pane.fail_pending("Chromium pane was closed");
            pane.browser
                .host()
                .ok_or_else(|| "Chromium browser host is unavailable".to_string())?
                .close_browser(1);
            Ok(())
        })
    }

    pub(crate) fn snapshot_png<F>(
        &self,
        check: crate::browser_workspace::EvidenceCheck,
        callback: F,
    ) -> Result<(), String>
    where
        F: FnOnce(Result<Vec<u8>, String>) + Send + 'static,
    {
        let callback: SnapshotCallback = Box::new(callback);
        on_ui(&self.app, &self.label, move |pane| {
            check.start(|| {
                pane.devtools(
                    "Page.captureScreenshot",
                    json!({"format": "png", "fromSurface": true, "captureBeyondViewport": false}),
                    Pending::Snapshot(callback),
                )
            })
        })
    }
}

fn zoom_level(factor: f64) -> Result<f64, String> {
    if !factor.is_finite() || factor <= 0.0 {
        return Err("브라우저 확대 비율이 올바르지 않습니다".to_string());
    }
    Ok(factor.log(1.2))
}

struct CookieBatch {
    remaining: AtomicUsize,
    /// 쿠키마다 저장소가 준 답 — `cookies`와 같은 차례.
    outcomes: Mutex<Vec<SetOutcome>>,
    /// 저장소가 디스크에 쓰겠다고 답했는가.
    flushed: AtomicBool,
    cookies: Vec<StagedCookie>,
    dropped: Dropped,
    app: AppHandle,
    label: String,
    url: tauri::Url,
    number: u64,
    config_root: PathBuf,
    profile_id: String,
}

impl CookieBatch {
    fn finished(self: &Arc<Self>, index: usize, outcome: SetOutcome) {
        if let Ok(mut outcomes) = self.outcomes.lock()
            && let Some(slot) = outcomes.get_mut(index)
        {
            *slot = outcome;
        }
        if self.remaining.fetch_sub(1, Ordering::AcqRel) != 1 {
            return;
        }
        let batch = self.clone();
        let app = self.app.clone();
        let _ = app.run_on_main_thread(move || {
            let _scope = crate::crumbs::MainScope::enter(SCOPE_COOKIES);
            let manager = PANES.with(|panes| {
                panes
                    .borrow()
                    .get(&batch.label)
                    .and_then(|pane| pane.request_context.cookie_manager(None))
            });
            let Some(manager) = manager else {
                return;
            };
            let mut callback = CookieFlushCallback::new(batch.clone());
            if manager.flush_store(Some(&mut callback)) != 1 {
                batch.flushed.store(false, Ordering::Release);
                batch.finish_navigation();
            }
        });
    }

    fn finish_navigation(self: &Arc<Self>) {
        let batch = self.clone();
        let app = self.app.clone();
        let _ = app.run_on_main_thread(move || {
            // 받은 것과 받지 않은 것은 스테이징에서 빠지고, 저장소에 닿지
            // 못한 것만 다음 판을 기다린다(`settle_staging`).
            let outcomes = batch
                .outcomes
                .lock()
                .map(|outcomes| outcomes.clone())
                .unwrap_or_default();
            let settled = browser_cookie_import::settle_staging(
                &batch.config_root,
                &batch.profile_id,
                &batch.cookies,
                &outcomes,
                batch.dropped,
                batch.flushed.load(Ordering::Acquire),
                browser_cookie_import::now_unix(),
            );
            let state = batch.app.state::<AppState>();
            crate::note_window_event(
                state.local_data_root(),
                &settled.log_line(&batch.profile_id),
            );
            let apply = &settled.apply;
            let counts = json!({
                "label": batch.label,
                "profile": batch.profile_id,
                "applied": apply.applied,
                "refused": apply.refused,
                "retained": apply.retained,
                "dropped": apply.dropped,
            });
            let _ = batch
                .app
                .emit_to("main", "browser:cookie-import-applied", counts.clone());
            if apply.refused + apply.retained > 0 {
                let mut said = counts;
                said["message"] = json!(format!(
                    "가져온 쿠키 {}개를 적용했습니다. {}개는 Chromium이 받지 않았고 {}개는 다음에 다시 시도합니다. 받지 않은 쿠키의 이름은 창 로그에 적었습니다.",
                    apply.applied, apply.refused, apply.retained
                ));
                let _ = batch
                    .app
                    .emit_to("main", "browser:cookie-import-error", said);
            }
            let result: Result<(), String> = PANES.with(|panes| {
                let mut panes = panes.borrow_mut();
                let pane = panes
                    .get_mut(&batch.label)
                    .ok_or_else(|| "cookie import finished after pane close".to_string())?;
                pane.send_navigation(batch.url.as_str(), batch.number, &Default::default())
                    .map_err(String::from)
            });
            if let Err(error) = result {
                eprintln!("zerocode-shell: Chromium cookie-first navigation failed: {error}");
            }
        });
    }
}

wrap_completion_callback! {
    struct CookieFlushCallback {
        batch: Arc<CookieBatch>,
    }
    impl CompletionCallback {
        fn on_complete(&self) {
            self.batch.finish_navigation();
        }
    }
}

wrap_set_cookie_callback! {
    struct CookieSetCallback {
        batch: Arc<CookieBatch>,
        index: usize,
    }
    impl SetCookieCallback {
        fn on_complete(&self, success: i32) {
            let outcome = if success == 1 {
                SetOutcome::Seated
            } else {
                SetOutcome::Refused
            };
            self.batch.finished(self.index, outcome);
        }
    }
}

fn cookie_url(cookie: &StagedCookie) -> Result<String, String> {
    if cookie.url.starts_with("http://") || cookie.url.starts_with("https://") {
        return Ok(cookie.url.clone());
    }
    let domain = cookie.domain.trim().trim_start_matches('.');
    if domain.is_empty() {
        return Err("가져온 쿠키에 URL과 도메인이 없습니다".to_string());
    }
    let scheme = if cookie.secure { "https" } else { "http" };
    Ok(format!("{scheme}://{domain}/"))
}

fn cef_cookie(cookie: &StagedCookie) -> Cookie {
    let expires = cookie.expires_unix.map(|seconds| Basetime {
        val: seconds
            .saturating_mul(1_000_000)
            .saturating_add(11_644_473_600_000_000),
    });
    Cookie {
        name: CefString::from(cookie.name.as_str()),
        value: CefString::from(cookie.value.as_str()),
        // 호스트 전용 쿠키는 Domain 없이 — 서 있으면 CEF가 하위 도메인
        // 쿠키로 넓히고, `__Host-` 쿠키는 거절한다.
        domain: CefString::from(browser_cookie_import::domain_attribute(cookie)),
        path: CefString::from(if cookie.path.is_empty() {
            "/"
        } else {
            &cookie.path
        }),
        secure: cookie.secure as i32,
        httponly: cookie.http_only as i32,
        has_expires: expires.is_some() as i32,
        expires: expires.unwrap_or_default(),
        same_site: match cookie.same_site.unwrap_or(SameSite::Unspecified) {
            SameSite::Unspecified => CookieSameSite::UNSPECIFIED,
            SameSite::NoRestriction => CookieSameSite::NO_RESTRICTION,
            SameSite::Lax => CookieSameSite::LAX_MODE,
            SameSite::Strict => CookieSameSite::STRICT_MODE,
        },
        priority: CookiePriority::MEDIUM,
        ..Default::default()
    }
}

type EvalCallback = Box<dyn FnOnce(String) + Send>;
type NavigationCallback = Box<dyn FnOnce(crate::NavAnswer)>;
type Navigations = Rc<RefCell<NavigationReply>>;
type SnapshotCallback = Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>;

#[derive(Default)]
struct NavigationReply {
    pending: Option<(i32, NavigationCallback)>,
}

impl NavigationReply {
    fn insert(&mut self, message_id: i32, callback: NavigationCallback) {
        self.pending = Some((message_id, callback));
    }

    fn remove(&mut self, message_id: &i32) -> Option<NavigationCallback> {
        if self
            .pending
            .as_ref()
            .is_some_and(|(pending_id, _)| pending_id == message_id)
        {
            self.pending.take().map(|(_, callback)| callback)
        } else {
            None
        }
    }
}

enum Pending {
    Eval(EvalCallback),
    Snapshot(SnapshotCallback),
    Ready(std::sync::mpsc::Sender<Result<(), String>>),
}

impl Pending {
    fn fail(self, reason: &str) {
        match self {
            Self::Eval(callback) => callback("null".to_string()),
            Self::Snapshot(callback) => callback(Err(reason.to_string())),
            Self::Ready(callback) => {
                let _ = callback.send(Err(reason.to_string()));
            }
        }
    }
}

struct PendingCall {
    deadline: Instant,
    timeout: tauri::async_runtime::JoinHandle<()>,
    action: Pending,
}

struct NativePane {
    app: AppHandle,
    label: String,
    browser: Browser,
    request_context: RequestContext,
    _observer: Registration,
    pending: Rc<RefCell<HashMap<i32, PendingCall>>>,
    navigations: Navigations,
    /// The view the window stands over the pane while the person watches it
    /// (t-66513); it goes with the pane.
    shield: shield::Shield,
}

/// A Chromium pane's native moves for one placement (t-66513), in the order
/// `stage_pane` makes them.
struct Staged<'a> {
    pane: &'a mut NativePane,
    dress: Option<&'a shield::Dress>,
}

impl crate::browser_workspace::PaneStage for Staged<'_> {
    fn bounds(&mut self, bounds: [f64; 4]) -> Result<(), String> {
        self.pane
            .set_bounds(bounds[0], bounds[1], bounds[2], bounds[3])
    }

    fn shield(&mut self, raised: bool) -> Result<(), String> {
        let dress = match (raised, self.dress) {
            (false, _) => {
                self.pane.shield.lower();
                return Ok(());
            }
            (true, Some(dress)) => dress,
            (true, None) => return Err(crate::cmd::browser::WATCH_UNSUPPORTED.to_string()),
        };
        let view = self.pane.native_view()?;
        // SAFETY: the pane's native view is a live NSView on this, the main,
        // thread for the length of this operation.
        let view = unsafe { &*view.cast::<objc2_app_kit::NSView>() };
        self.pane.shield.raise(view, dress)
    }

    fn show(&mut self) -> Result<(), String> {
        self.pane.set_hidden(false)
    }
}

impl NativePane {
    fn send_navigation(
        &mut self,
        url: &str,
        number: u64,
        check: &crate::browser_workspace::EvidenceCheck,
    ) -> Result<(), crate::cmd::browser::BrowserRefusal> {
        let app = self.app.clone();
        let label = self.label.clone();
        let state = app.state::<AppState>();
        let id = NEXT_DEVTOOLS_ID.fetch_add(1, Ordering::Relaxed);
        let prepared = serde_json::to_vec(
            &json!({"id": id, "method": "Page.navigate", "params": {"url": url}}),
        )
        .map_err(|error| error.to_string())
        .and_then(|message| {
            self.browser
                .host()
                .map(|host| (host, message))
                .ok_or_else(|| "Chromium browser host is unavailable".to_string())
        });
        let sent = prepared.and_then(|(host, message)| {
            check.start(|| {
                if !crate::cmd::browser::navigation_sent(&state, &label, number) {
                    return Err(crate::cmd::browser::NAVIGATION_SUPERSEDED.to_string());
                }
                let answer_app = app.clone();
                let answer_label = label.clone();
                self.navigations.borrow_mut().insert(
                    id,
                    Box::new(move |answer| {
                        crate::cmd::browser::navigation_answered(
                            &answer_app.state::<AppState>(),
                            &answer_label,
                            number,
                            answer,
                        );
                    }),
                );
                if host.send_dev_tools_message(Some(&message)) != 1 {
                    self.navigations.borrow_mut().remove(&id);
                    crate::cmd::browser::navigation_answered(
                        &state,
                        &label,
                        number,
                        crate::NavAnswer::Resolved,
                    );
                    return Err("Chromium DevTools method was rejected: Page.navigate".to_string());
                }
                Ok(())
            })
        });
        sent.map_err(|why| {
            crate::cmd::browser::navigation_withdrawn(&state, &label, number);
            crate::cmd::browser::BrowserRefusal::Unsent(why)
        })
    }

    fn devtools(&mut self, method: &str, params: Value, pending: Pending) -> Result<(), String> {
        let id = NEXT_DEVTOOLS_ID.fetch_add(1, Ordering::Relaxed);
        let message = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
            .map_err(|error| error.to_string())?;
        self.expire_pending();
        let app = self.app.clone();
        let label = self.label.clone();
        let timeout = tauri::async_runtime::spawn(async move {
            tokio::time::sleep(DEVTOOLS_DEADLINE).await;
            let _ = app.run_on_main_thread(move || {
                let call = PANES.with(|panes| {
                    panes
                        .borrow()
                        .get(&label)
                        .and_then(|pane| pane.pending.borrow_mut().remove(&id))
                });
                if let Some(call) = call {
                    call.action.fail("Chromium renderer callback timed out");
                }
            });
        });
        self.pending.borrow_mut().insert(
            id,
            PendingCall {
                deadline: Instant::now() + DEVTOOLS_DEADLINE,
                timeout,
                action: pending,
            },
        );
        let sent = self
            .browser
            .host()
            .ok_or_else(|| "Chromium browser host is unavailable".to_string())?
            .send_dev_tools_message(Some(&message));
        if sent != 1 {
            if let Some(call) = self.pending.borrow_mut().remove(&id) {
                call.timeout.abort();
                call.action
                    .fail(&format!("Chromium DevTools method was rejected: {method}"));
            }
            return Err(format!("Chromium DevTools method was rejected: {method}"));
        }
        Ok(())
    }

    fn expire_pending(&mut self) {
        let now = Instant::now();
        let expired: Vec<PendingCall> = {
            let mut pending = self.pending.borrow_mut();
            let ids: Vec<i32> = pending
                .iter()
                .filter_map(|(id, call)| (call.deadline <= now).then_some(*id))
                .collect();
            ids.into_iter()
                .filter_map(|id| pending.remove(&id))
                .collect()
        };
        for call in expired {
            call.timeout.abort();
            call.action.fail("Chromium renderer callback timed out");
        }
    }

    fn fail_pending(&mut self, reason: &str) {
        let calls: Vec<PendingCall> = self
            .pending
            .borrow_mut()
            .drain()
            .map(|(_, call)| call)
            .collect();
        for call in calls {
            call.timeout.abort();
            call.action.fail(reason);
        }
    }

    fn set_bounds(&self, x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
        let view = self.native_view()?;
        unsafe {
            let parent: *mut objc2::runtime::AnyObject = objc2::msg_send![view, superview];
            if parent.is_null() {
                return Err("Chromium parent view is unavailable".to_string());
            }
            let flipped: bool = objc2::msg_send![parent, isFlipped];
            let bounds: objc2_foundation::NSRect = objc2::msg_send![parent, bounds];
            let y = if flipped {
                y
            } else {
                bounds.size.height - y - height.max(1.0)
            };
            let frame = objc2_foundation::NSRect::new(
                objc2_foundation::NSPoint::new(x, y),
                objc2_foundation::NSSize::new(width.max(1.0), height.max(1.0)),
            );
            let _: () = objc2::msg_send![view, setFrame: frame];
        }
        Ok(())
    }

    fn set_hidden(&self, hidden: bool) -> Result<(), String> {
        let view = self.native_view()?;
        unsafe {
            let _: () = objc2::msg_send![view, setHidden: hidden];
        }
        Ok(())
    }

    fn native_view(&self) -> Result<*mut objc2::runtime::AnyObject, String> {
        let handle = self
            .browser
            .host()
            .ok_or_else(|| "Chromium browser host is unavailable".to_string())?
            .window_handle();
        if handle.is_null() {
            Err("Chromium NSView is unavailable".to_string())
        } else {
            Ok(handle.cast())
        }
    }
}

thread_local! {
    static PANES: RefCell<HashMap<String, NativePane>> = RefCell::new(HashMap::new());
}

pub(crate) struct CreatePane {
    pub(crate) app: AppHandle,
    pub(crate) label: String,
    pub(crate) profile_cache: Option<PathBuf>,
    pub(crate) user_agent: Option<String>,
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
    pub(crate) hidden: bool,
    pub(crate) initialization_scripts: Vec<String>,
}

/// A newly attached DevTools session starts with the Page domain disabled.
/// Without enabling it, script registration succeeds but document-start
/// scripts never run. The acknowledgement count and dispatch share this plan.
fn initialization_commands(
    scripts: &[String],
    user_agent: Option<&str>,
) -> Vec<(&'static str, Value)> {
    let mut commands = vec![("Page.enable", json!({}))];
    commands.extend(scripts.iter().map(|source| {
        (
            "Page.addScriptToEvaluateOnNewDocument",
            json!({ "source": source }),
        )
    }));
    if let Some(agent) = user_agent {
        commands.push((
            "Network.setUserAgentOverride",
            json!({ "userAgent": agent }),
        ));
    }
    commands
}

pub(crate) fn create_pane(options: CreatePane) -> Result<BrowserPane, String> {
    // Renderer initialization acknowledgements need the main loop to remain free.
    if crate::crumbs::is_main_thread() {
        return Err("Chromium pane creation must be requested off the UI thread".to_string());
    }
    let app = options.app.clone();
    let count = initialization_commands(
        &options.initialization_scripts,
        options.user_agent.as_deref(),
    )
    .len();
    let (ready, acknowledgements) = std::sync::mpsc::channel();
    let (created, creation) = std::sync::mpsc::channel();
    run_ui(&app, move || begin_create_pane(options, ready, created))?;
    let pane = creation
        .recv_timeout(UI_WAIT)
        .map_err(|_| "Chromium profile initialization timed out".to_string())??;
    let deadline = Instant::now() + UI_WAIT;
    for _ in 0..count {
        let result = acknowledgements
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| "Chromium document initialization timed out".to_string())
            .and_then(|answer| answer);
        if let Err(error) = result {
            let _ = pane.close();
            return Err(error);
        }
    }
    Ok(pane)
}

struct PendingPaneCreation {
    options: CreatePane,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
    created: std::sync::mpsc::Sender<Result<BrowserPane, String>>,
}

fn begin_create_pane(
    options: CreatePane,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
    created: std::sync::mpsc::Sender<Result<BrowserPane, String>>,
) -> Result<(), String> {
    // `begin_create_pane` runs on the main thread: a pane asked for before the
    // paint's own start gets the engine started right here.
    if !INITIALIZED.load(Ordering::Acquire) {
        ensure_initialized(&options.app)?;
    }
    if !INITIALIZED.load(Ordering::Acquire) || EXIT_PENDING.load(Ordering::Acquire) {
        return Err("Chromium runtime is not available".to_string());
    }
    let window = options
        .app
        .get_window("main")
        .ok_or_else(|| "창이 없습니다".to_string())?;
    let parent_view = window.ns_view().map_err(|error| error.to_string())?;

    let profile_cache = options
        .profile_cache
        .as_deref()
        .map(Path::canonicalize)
        .transpose()
        .map_err(|error| error.to_string())?;
    let context_settings = RequestContextSettings {
        cache_path: profile_cache
            .as_deref()
            .map(|path| CefString::from(path.to_string_lossy().as_ref()))
            .unwrap_or_default(),
        persist_session_cookies: options.profile_cache.is_some() as i32,
        ..Default::default()
    };
    let mut request_context = request_context_create_context(Some(&context_settings), None)
        .ok_or_else(|| "Chromium request context creation failed".to_string())?;
    let mut window = WindowInfo::default().set_as_child(
        parent_view,
        &Rect {
            x: options.x.round() as i32,
            y: options.y.round() as i32,
            width: options.width.max(1.0).round() as i32,
            height: options.height.max(1.0).round() as i32,
        },
    );
    window.hidden = i32::from(options.hidden);
    let handler = HandlerState {
        app: options.app.clone(),
        label: options.label.clone(),
        closed: Arc::new(AtomicBool::new(false)),
        downloads: Rc::new(RefCell::new(HashMap::new())),
        creation: Rc::new(RefCell::new(Some(PendingPaneCreation {
            options,
            ready,
            created,
        }))),
    };
    let mut client = BrowserClient::new(handler);
    // CEF waits for the persistent profile before invoking on_after_created.
    if browser_host_create_browser(
        Some(&window),
        Some(&mut client),
        Some(&CefString::from("about:blank")),
        Some(&BrowserSettings::default()),
        None,
        Some(&mut request_context),
    ) != 1
    {
        return Err("Chromium child browser creation was rejected".to_string());
    }
    Ok(())
}

fn finish_create_pane(
    options: CreatePane,
    ready: std::sync::mpsc::Sender<Result<(), String>>,
    browser: Browser,
) -> Result<BrowserPane, String> {
    let request_context = browser
        .host()
        .and_then(|host| host.request_context())
        .ok_or_else(|| "Chromium request context is unavailable".to_string())?;
    let pending = Rc::new(RefCell::new(HashMap::new()));
    let navigations = Navigations::default();
    let mut observer = DevToolsObserver::new(pending.clone(), navigations.clone());
    let registration = browser
        .host()
        .and_then(|host| host.add_dev_tools_message_observer(Some(&mut observer)));
    let Some(registration) = registration else {
        return Err("Chromium private DevTools observer creation failed".to_string());
    };
    let mut pane = NativePane {
        app: options.app.clone(),
        label: options.label.clone(),
        browser,
        request_context,
        _observer: registration,
        pending,
        navigations,
        shield: shield::Shield::default(),
    };
    let prepared = (|| {
        pane.set_hidden(options.hidden)?;
        pane.set_bounds(options.x, options.y, options.width, options.height)?;
        for (method, params) in initialization_commands(
            &options.initialization_scripts,
            options.user_agent.as_deref(),
        ) {
            pane.devtools(method, params, Pending::Ready(ready.clone()))?;
        }
        Ok::<(), String>(())
    })();
    if let Err(error) = prepared {
        pane.fail_pending("Chromium initialization failed");
        return Err(error);
    }
    let label = options.label.clone();
    PANES.with(|panes| {
        panes.borrow_mut().insert(label.clone(), pane);
    });
    Ok(BrowserPane::new(&options.app, label))
}

fn on_ui<T, F>(app: &AppHandle, label: &str, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut NativePane) -> Result<T, String> + Send + 'static,
{
    let label = label.to_string();
    run_ui(app, move || {
        PANES.with(|panes| {
            let mut panes = panes.borrow_mut();
            let pane = panes
                .get_mut(&label)
                .ok_or_else(|| "브라우저 판이 이미 닫혔습니다".to_string())?;
            operation(pane)
        })
    })
}

fn run_ui<T, F>(app: &AppHandle, operation: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    if crate::crumbs::is_main_thread() {
        return operation();
    }
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let _scope = crate::crumbs::MainScope::enter(SCOPE_OPERATION);
        let _ = sender.send(operation());
    })
    .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(UI_WAIT)
        .map_err(|_| "Chromium main thread did not answer in time".to_string())?
}

fn cef_owned_string(value: CefStringUserfree) -> String {
    CefString::from(&value).to_string()
}

#[derive(Clone)]
struct HandlerState {
    app: AppHandle,
    label: String,
    closed: Arc<AtomicBool>,
    downloads: Rc<RefCell<HashMap<u32, PathBuf>>>,
    creation: Rc<RefCell<Option<PendingPaneCreation>>>,
}

impl HandlerState {
    fn emit_nav(&self, frame: Option<&mut Frame>, state: &'static str) {
        let Some(frame) = frame.filter(|frame| frame.is_main() == 1) else {
            return;
        };
        let url = cef_owned_string(frame.url());
        let app_state = self.app.state::<AppState>();
        if url == "about:blank"
            && app_state
                .browser_urls()
                .get(&self.label)
                .is_none_or(|target| target != "about:blank")
        {
            return;
        }
        app_state
            .browser_urls()
            .insert(self.label.clone(), url.clone());
        let generation = app_state
            .browser_records()
            .get_mut(&self.label)
            .and_then(|record| {
                if state == "started" {
                    Some(record.nav.started(std::time::Instant::now()))
                } else {
                    record.nav.finished(url == "about:blank");
                    None
                }
            });
        if let Some(generation) = generation {
            crate::cmd::browser::arm_dead_display(&self.app, &self.label, generation);
        }
        let _ = self.app.emit_to(
            "main",
            "browser:nav",
            BrowserNav {
                label: self.label.clone(),
                url,
                state,
            },
        );
    }
}

wrap_client! {
    struct BrowserClient {
        inner: HandlerState,
    }

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(BrowserDisplayHandler::new(self.inner.clone()))
        }
        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(BrowserLifeSpanHandler::new(self.inner.clone()))
        }
        fn load_handler(&self) -> Option<LoadHandler> {
            Some(BrowserLoadHandler::new(self.inner.clone()))
        }
        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(BrowserDownloadHandler::new(self.inner.clone()))
        }
        fn request_handler(&self) -> Option<RequestHandler> {
            Some(BrowserRequestHandler::new(self.inner.clone()))
        }
    }
}

wrap_client! {
    struct BrowserDevToolsClient {}
    impl Client {}
}

wrap_download_handler! {
    struct BrowserDownloadHandler {
        inner: HandlerState,
    }
    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, url: Option<&CefString>, _method: Option<&CefString>) -> i32 {
            url.and_then(|url| url.to_string().parse::<tauri::Url>().ok())
                .is_some_and(|url| browsable_target(&url) || matches!(url.scheme(), "blob" | "data")) as i32
        }
        fn on_before_download(
            &self,
            _browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            suggested_name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let (Some(item), Some(callback)) = (item, callback) else { return 1; };
            let wanted = suggested_name.map(CefString::to_string).unwrap_or_else(|| "download".into());
            let landing = self.inner.app.path().download_dir()
                .map_err(|error| error.to_string())
                .and_then(|dir| crate::shell_runtime::reserved_download_path(&dir, &wanted));
            let Ok(landing) = landing else {
                let _ = self.inner.app.emit_to("main", "browser:download", BrowserDownload {
                    label: self.inner.label.clone(), state: "failed",
                    url: cef_owned_string(item.url()), path: String::new(), file: wanted,
                });
                return 1;
            };
            let path = landing.to_string_lossy().into_owned();
            let file = landing.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
            self.inner.downloads.borrow_mut().insert(item.id(), landing);
            let _ = self.inner.app.emit_to("main", "browser:download", BrowserDownload {
                label: self.inner.label.clone(), state: "started",
                url: cef_owned_string(item.url()), path: path.clone(), file,
            });
            callback.cont(Some(&CefString::from(path.as_str())), 0);
            1
        }
        fn on_download_updated(
            &self,
            _browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = item else { return; };
            if item.is_complete() == 0 && item.is_canceled() == 0 && item.is_interrupted() == 0 {
                return;
            }
            let Some(path) = self.inner.downloads.borrow_mut().remove(&item.id()) else { return; };
            let _ = self.inner.app.emit_to("main", "browser:download", BrowserDownload {
                label: self.inner.label.clone(),
                state: if item.is_complete() == 1 { "finished" } else { "failed" },
                url: cef_owned_string(item.url()), path: path.to_string_lossy().into_owned(), file: String::new(),
            });
        }
    }
}

wrap_display_handler! {
    struct BrowserDisplayHandler {
        inner: HandlerState,
    }
    impl DisplayHandler {
        fn on_title_change(&self, _browser: Option<&mut Browser>, title: Option<&CefString>) {
            let title = title.map(CefString::to_string).unwrap_or_default();
            if let Some(record) = self.inner.app.state::<AppState>().browser_records().get_mut(&self.inner.label) {
                record.title = crate::cmd::browser::terminal_safe(&title, crate::cmd::browser::BROWSER_TITLE_CAP);
            }
            let _ = self.inner.app.emit_to("main", "browser:title", BrowserTitle {
                label: self.inner.label.clone(),
                title,
            });
        }
    }
}

wrap_load_handler! {
    struct BrowserLoadHandler {
        inner: HandlerState,
    }
    impl LoadHandler {
        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition: TransitionType) {
            self.inner.emit_nav(frame, "started");
        }
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _status: i32) {
            self.inner.emit_nav(frame, "finished");
        }
        fn on_load_error(
            &self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            error_code: Errorcode, _error_text: Option<&CefString>, failed_url: Option<&CefString>,
        ) {
            if error_code == Errorcode::ABORTED || frame.is_none_or(|frame| frame.is_main() != 1) {
                return;
            }
            let url = failed_url.map(CefString::to_string).unwrap_or_default();
            let state = self.inner.app.state::<AppState>();
            crate::note_window_event(state.local_data_root(), &format!(
                "Chromium pane {} navigation failed: {:?}", self.inner.label, error_code
            ));
            if state.browser_urls().get(&self.inner.label) != Some(&url) {
                return;
            }
            if let Some(record) = state.browser_records().get_mut(&self.inner.label) {
                record.nav.failed();
            }
            let _ = self.inner.app.emit_to("main", "browser:nav", BrowserNav {
                label: self.inner.label.clone(), url, state: "dead",
            });
        }
    }
}

wrap_task! {
    struct CloseBrowserViewTask {
        browser: Browser,
    }
    impl Task {
        fn execute(&self) {
            let Some(host) = self.browser.host() else { return; };
            let view = host.window_handle().cast::<objc2::runtime::AnyObject>();
            if view.is_null() { return; }
            unsafe {
                let window: *mut objc2::runtime::AnyObject = objc2::msg_send![view, window];
                if crate::browser_runtime::browser_view_owns_first_responder(view) {
                    let _: bool = objc2::msg_send![window, makeFirstResponder: std::ptr::null_mut::<objc2::runtime::AnyObject>()];
                }
                let _: () = objc2::msg_send![view, removeFromSuperview];
            }
        }
    }
}

wrap_life_span_handler! {
    struct BrowserLifeSpanHandler {
        inner: HandlerState,
    }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return; };
            OPEN_BROWSERS.fetch_add(1, Ordering::AcqRel);
            crate::boot_timeline::mark(crate::boot_timeline::Phase::FirstBrowserTab);
            let pending = self.inner.creation.borrow_mut().take();
            let Some(pending) = pending else {
                if let Some(host) = browser.host() { host.close_browser(1); }
                return;
            };
            let result = if EXIT_PENDING.load(Ordering::Acquire) {
                Err("Chromium is shutting down".to_string())
            } else {
                finish_create_pane(pending.options, pending.ready, browser.clone())
            };
            if result.is_err()
                && let Some(host) = browser.host()
            {
                host.close_browser(1);
            }
            if let Err(std::sync::mpsc::SendError(Ok(pane))) = pending.created.send(result) {
                let _ = pane.close();
            }
        }

        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            // Detaching destroys the CEF host view; do it after DoClose unwinds.
            if let Some(browser) = browser {
                let mut task = CloseBrowserViewTask::new(browser.clone());
                if post_task(ThreadId::UI, Some(&mut task)) != 1 {
                    eprintln!("zerocode-shell: Chromium pane close task was rejected");
                }
            }
            1
        }

        fn on_before_popup(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
            _target_frame_name: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
            _popup_features: Option<&PopupFeatures>,
            _window_info: Option<&mut WindowInfo>,
            _client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra_info: Option<&mut Option<DictionaryValue>>,
            _no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            let url = target_url.map(CefString::to_string).unwrap_or_default();
            crate::cmd::browser::workspace_popup(&self.inner.app, &self.inner.label, &url);
            1
        }

        fn on_before_close(&self, _browser: Option<&mut Browser>) {
            if !self.inner.closed.swap(true, Ordering::AcqRel) {
                let _ = OPEN_BROWSERS.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| count.checked_sub(1));
                crate::note_window_event(self.inner.app.state::<AppState>().local_data_root(),
                    &format!("Chromium pane {} closed", self.inner.label));
            }
        }
    }
}

wrap_request_handler! {
    struct BrowserRequestHandler {
        inner: HandlerState,
    }
    impl RequestHandler {
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _user_gesture: i32,
            _is_redirect: i32,
        ) -> i32 {
            let allowed = request
                .and_then(|request| cef_owned_string(request.url()).parse::<tauri::Url>().ok())
                .is_some_and(|url| {
                    browsable_target(&url)
                        || (_frame.as_ref().is_some_and(|frame| frame.is_main() == 0)
                            && (url.as_str() == "about:srcdoc" || matches!(url.scheme(), "blob" | "data")))
                });
            (!allowed) as i32
        }
        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            target_url: Option<&CefString>,
            _target_disposition: WindowOpenDisposition,
            _user_gesture: i32,
        ) -> i32 {
            let url = target_url.map(CefString::to_string).unwrap_or_default();
            crate::cmd::browser::workspace_popup(&self.inner.app, &self.inner.label, &url);
            1
        }
    }
}

wrap_dev_tools_message_observer! {
    struct DevToolsObserver {
        pending: Rc<RefCell<HashMap<i32, PendingCall>>>,
        navigations: Navigations,
    }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self,
            _browser: Option<&mut Browser>,
            message_id: i32,
            success: i32,
            result: Option<&[u8]>,
        ) {
            let pending = self.pending.borrow_mut().remove(&message_id);
            let Some(pending) = pending else {
                let navigation = self.navigations.borrow_mut().remove(&message_id);
                if let Some(answer) = navigation {
                    answer(navigation_answer(success, result));
                }
                return;
            };
            pending.timeout.abort();
            let parsed = result
                .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
                .unwrap_or(Value::Null);
            match pending.action {
                Pending::Ready(callback) => {
                    let answer = if success == 1 {
                        Ok(())
                    } else {
                        Err("Chromium document initialization was rejected".to_string())
                    };
                    let _ = callback.send(answer);
                }
                Pending::Eval(callback) => {
                    let value = if success == 1 {
                        parsed.pointer("/result/value").cloned().unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    };
                    callback(serde_json::to_string(&value).unwrap_or_else(|_| "null".into()));
                }
                Pending::Snapshot(callback) => {
                    let bytes = (success == 1)
                        .then(|| parsed.get("data")?.as_str())
                        .flatten()
                        .ok_or_else(|| "Chromium screenshot did not return PNG data".to_string())
                        .and_then(|encoded| {
                            base64::engine::general_purpose::STANDARD
                                .decode(encoded)
                                .map_err(|error| format!("Chromium screenshot decode failed: {error}"))
                        });
                    callback(bytes);
                }
            }
        }
    }
}

fn navigation_answer(success: i32, result: Option<&[u8]>) -> crate::NavAnswer {
    let parsed = result
        .and_then(|bytes| serde_json::from_slice::<Value>(bytes).ok())
        .unwrap_or(Value::Null);
    crate::page_navigate_answer(success == 1, &parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_superseded_navigation_releases_its_callback_without_answering() {
        let replies = Navigations::default();
        let held = Rc::new(());
        let retained = Rc::downgrade(&held);
        replies.borrow_mut().insert(
            1,
            Box::new(move |_| {
                drop(held);
                panic!("a superseded navigation must not be answered");
            }),
        );
        assert!(retained.upgrade().is_some());
        replies.borrow_mut().insert(2, Box::new(|_| {}));
        assert!(retained.upgrade().is_none());
    }

    #[test]
    fn a_late_navigation_answer_never_takes_the_current_callback() {
        let replies = Navigations::default();
        let answered = Rc::new(std::cell::Cell::new(None));
        let observed = answered.clone();
        replies.borrow_mut().insert(
            1,
            Box::new(|_| panic!("the previous navigation must not be answered")),
        );
        replies
            .borrow_mut()
            .insert(2, Box::new(move |answer| observed.set(Some(answer))));
        assert!(replies.borrow_mut().remove(&1).is_none());
        let answer = replies.borrow_mut().remove(&2).unwrap();
        answer(crate::NavAnswer::Loading);
        assert_eq!(answered.get(), Some(crate::NavAnswer::Loading));
        assert!(replies.borrow_mut().remove(&2).is_none());
    }

    #[test]
    fn initialization_enables_page_before_scripts_and_counts_every_acknowledgement() {
        let scripts = vec!["first()".into(), "second()".into()];
        let commands = initialization_commands(&scripts, Some("fixture-agent"));
        assert_eq!(
            commands
                .iter()
                .map(|(method, _)| *method)
                .collect::<Vec<_>>(),
            [
                "Page.enable",
                "Page.addScriptToEvaluateOnNewDocument",
                "Page.addScriptToEvaluateOnNewDocument",
                "Network.setUserAgentOverride"
            ]
        );
        assert_eq!(commands[1].1["source"], "first()");
        assert_eq!(commands[2].1["source"], "second()");
        assert_eq!(commands[3].1["userAgent"], "fixture-agent");
        assert_eq!(initialization_commands(&[], None).len(), 1);
    }

    #[test]
    fn browser_zoom_uses_chromium_levels() {
        assert_eq!(zoom_level(1.0).unwrap(), 0.0);
        assert!((zoom_level(1.2).unwrap() - 1.0).abs() < 1e-10);
        assert!(zoom_level(0.8).unwrap() < 0.0);
        for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(zoom_level(invalid).is_err());
        }
    }

    #[test]
    fn packaged_paths_point_inside_frameworks() {
        let executable = Path::new("/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell");
        let textual = executable
            .parent()
            .unwrap()
            .join("../Frameworks")
            .join(FRAMEWORK);
        assert!(
            textual
                .to_string_lossy()
                .contains("Contents/MacOS/../Frameworks/Chromium Embedded Framework.framework")
        );
        assert!(HELPER_APP.starts_with("ZeroCode Helper.app/Contents/MacOS/"));
    }

    /// Web Bluetooth is off beside the https features. A page that asks for a
    /// Bluetooth device is the road that opens the adapter, which reads the
    /// devices paired with this Mac, so the feature stays off by name on the
    /// same command line (t-45873).
    #[test]
    fn the_engine_starts_with_web_bluetooth_off_beside_the_https_features() {
        let disabled: Vec<&str> = CEF_DISABLED_FEATURES.split(',').collect();
        for feature in ["HttpsUpgrades", "HttpsFirstBalancedMode", "WebBluetooth"] {
            assert!(
                disabled.contains(&feature),
                "`{feature}` is not turned off in the engine's command line: {CEF_DISABLED_FEATURES}"
            );
        }
    }
}
