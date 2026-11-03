//! Real CEF, hosted the way the window hosts it (an Alloy child view) inside an
//! NSWindow that is never put on screen, in an app with no Dock icon. It answers
//! one question the fake window backend cannot: what does Chromium itself do with
//! each kind of popup when `on_before_popup` cancels it (`--mode cancel`, what the
//! window did before t-90671), when it lets CEF build it (`--mode real`, a person's
//! popup), and when the first request also waits for a later verdict (`--mode
//! settled`, an agent's popup, which `with_agent_popup` may still refuse).
//!
//! Clicks are DevTools `Runtime.evaluate` calls with `userGesture`, delivered to
//! this probe's own page. No desktop input is sent to anything.

use cef::*;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{msg_send, sel};
use serde_json::json;
use std::{
    cell::RefCell,
    ffi::CString,
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

const CASES: &[(&str, bool)] = &[
    ("open-empty-location", true),
    ("open-blank-write", true),
    ("form-post-named", true),
    ("form-post-blank", true),
    ("opener-callback", true),
    ("link-blank", true),
    ("open-noopener", true),
    ("form-noopener", true),
    ("link-download", true),
    ("slow-download-closed", true),
    ("refused-settle", true),
    ("refused-data", true),
    ("refused-later", true),
    ("no-gesture", false),
];
const CASE_MS: i64 = 3_000;
const PROBE_AGENT: &str = "Mozilla/5.0 ZeroCodeProbeAgent";
/// How long `--mode settled` keeps the first request waiting. The window's own
/// settlement is one task hop and one main-thread hop; this is far longer, so a
/// wait that depended on the popup's document would show.
const SETTLE_MS: i64 = 250;

/// The popup's navigation waits here until its scripts and agent have been sent.
/// Nothing waits for their answers, since a popup with no document answers nothing
/// until this very request commits. In `--mode settled` it also waits for the
/// verdict, which may refuse the request.
#[derive(Default)]
struct Gate {
    open: AtomicBool,
    refused: AtomicBool,
    held: Mutex<Vec<HeldRequest>>,
}

struct HeldRequest(Callback);
// SAFETY: CefCallback is one of CEF's thread-safe reference-counted objects.
unsafe impl Send for HeldRequest {}

impl Gate {
    fn release(&self) {
        let mut held = self.held.lock().unwrap();
        self.open.store(true, Ordering::Release);
        for request in held.drain(..) {
            request.0.cont();
        }
    }

    fn refuse(&self) {
        let mut held = self.held.lock().unwrap();
        self.refused.store(true, Ordering::Release);
        for request in held.drain(..) {
            request.0.cancel();
        }
    }
}

#[derive(Default)]
struct Probe {
    real: bool,
    settled: bool,
    origin: String,
    parent: usize,
    events: Option<std::fs::File>,
    opener: Option<Browser>,
    popups: Vec<Browser>,
    downloads: PathBuf,
    case: usize,
    closing: bool,
    open: usize,
}

thread_local! {
    static PROBE: RefCell<Probe> = RefCell::new(Probe::default());
}

fn with<T>(run: impl FnOnce(&mut Probe) -> T) -> T {
    PROBE.with(|probe| run(&mut probe.borrow_mut()))
}

fn note(row: serde_json::Value) {
    with(|probe| {
        if let Some(file) = probe.events.as_mut() {
            let _ = writeln!(file, "{row}");
        }
    });
}

fn text(value: Option<&CefString>) -> String {
    value.map(CefString::to_string).unwrap_or_default()
}

fn owned(value: CefStringUserfree) -> String {
    CefString::from(&value).to_string()
}

/// The window's own `browsable_target`, byte for byte in what it admits.
fn browsable(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or_default();
    match scheme {
        "http" | "https" => !url.contains("tauri.localhost"),
        "file" => true,
        "about" => url == "about:blank",
        _ => false,
    }
}

fn send(browser: &Browser, method: &str, params: serde_json::Value, id: i32) {
    let message = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
        .expect("a DevTools message");
    if let Some(host) = browser.host() {
        host.send_dev_tools_message(Some(&message));
    }
}

fn dress_popup(browser: &Browser, gate: Arc<Gate>) {
    send(browser, "Page.enable", json!({}), 1);
    send(
        browser,
        "Page.addScriptToEvaluateOnNewDocument",
        json!({"source": "window.__zcGuest = 1;", "runImmediately": true}),
        2,
    );
    send(
        browser,
        "Network.setUserAgentOverride",
        json!({"userAgent": PROBE_AGENT}),
        3,
    );
    // Not on their answers: a popup with no document yet answers nothing until
    // its first navigation commits, which is the request held here.
    if !with(|probe| probe.settled) {
        gate.release();
        return;
    }
    let refuse = with(|probe| current_case(probe) == Some("refused-settle"));
    let mut task = SettleGate::new(gate, browser.clone(), refuse);
    post_delayed_task(ThreadId::UI, Some(&mut task), SETTLE_MS);
}

fn current_case(probe: &Probe) -> Option<&'static str> {
    let at = probe.case.checked_sub(1)?;
    CASES.get(at).map(|case| case.0)
}

// What the window's agent settlement does: the verdict opens the gate, or refuses
// the request and closes the popup.
wrap_task! {
    struct SettleGate { gate: Arc<Gate>, browser: Browser, refuse: bool }
    impl Task {
        fn execute(&self) {
            note(json!({"ev": "settled", "refused": self.refuse}));
            if self.refuse {
                self.gate.refuse();
                if let Some(host) = self.browser.host() {
                    host.close_browser(1);
                }
            } else {
                self.gate.release();
            }
        }
    }
}

// A person closing the empty tab while its file is still coming.
wrap_task! {
    struct CloseDownloadingPopup { browser: Browser }
    impl Task {
        fn execute(&self) {
            note(json!({"ev": "closing_while_downloading"}));
            if let Some(host) = self.browser.host() {
                host.close_browser(1);
            }
        }
    }
}

wrap_client! {
    struct ProbeClient { role: &'static str, gate: Arc<Gate> }
    impl Client {
        fn life_span_handler(&self) -> Option<LifeSpanHandler> { Some(ProbeLife::new(self.role, self.gate.clone())) }
        fn load_handler(&self) -> Option<LoadHandler> { Some(ProbeLoad::new(self.role)) }
        fn request_handler(&self) -> Option<RequestHandler> { Some(ProbeRequest::new(self.role, self.gate.clone())) }
        fn download_handler(&self) -> Option<DownloadHandler> { Some(ProbeDownload::new(self.role)) }
    }
}

wrap_download_handler! {
    struct ProbeDownload { role: &'static str }
    impl DownloadHandler {
        fn can_download(&self, _browser: Option<&mut Browser>, _url: Option<&CefString>, _method: Option<&CefString>) -> i32 { 1 }
        fn on_before_download(
            &self,
            browser: Option<&mut Browser>,
            _item: Option<&mut DownloadItem>,
            name: Option<&CefString>,
            callback: Option<&mut BeforeDownloadCallback>,
        ) -> i32 {
            let Some(callback) = callback else { return 1 };
            let (root, case) = with(|probe| (probe.downloads.clone(), current_case(probe)));
            let landing = root.join(text(name));
            let frame = browser.as_deref().and_then(|browser| browser.main_frame()).map(|frame| owned(frame.url()));
            note(json!({"ev": "download_started", "role": self.role, "frame": frame}));
            callback.cont(Some(&CefString::from(landing.to_string_lossy().as_ref())), 0);
            if case == Some("slow-download-closed")
                && self.role == "popup"
                && let Some(browser) = browser
            {
                let mut task = CloseDownloadingPopup::new(browser.clone());
                post_delayed_task(ThreadId::UI, Some(&mut task), 300);
            }
            1
        }
        fn on_download_updated(
            &self,
            browser: Option<&mut Browser>,
            item: Option<&mut DownloadItem>,
            _callback: Option<&mut DownloadItemCallback>,
        ) {
            let Some(item) = item else { return };
            if item.is_complete() == 0 && item.is_canceled() == 0 && item.is_interrupted() == 0 {
                return;
            }
            let state = if item.is_complete() == 1 { "complete" } else { "failed" };
            note(json!({"ev": "download_done", "role": self.role, "state": state}));
            // What the window does: the popup with no page in it goes when the file is done.
            if self.role == "popup"
                && let Some(browser) = browser
                && browser.main_frame().is_none_or(|frame| {
                    let url = owned(frame.url());
                    url.is_empty() || url == "about:blank"
                })
                && let Some(host) = browser.host()
            {
                note(json!({"ev": "download_popup_closed"}));
                host.close_browser(1);
            }
        }
    }
}

wrap_load_handler! {
    struct ProbeLoad { role: &'static str }
    impl LoadHandler {
        fn on_load_start(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, _transition: TransitionType) {
            let Some(frame) = frame.filter(|frame| frame.is_main() == 1) else { return };
            note(json!({"ev": "load_start", "role": self.role, "url": owned(frame.url())}));
        }
        fn on_load_error(
            &self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            error_code: Errorcode, _error_text: Option<&CefString>, failed_url: Option<&CefString>,
        ) {
            if !frame.is_some_and(|frame| frame.is_main() == 1) { return; }
            note(json!({"ev": "load_error", "role": self.role, "url": text(failed_url), "code": format!("{error_code:?}")}));
        }
        fn on_load_end(&self, _browser: Option<&mut Browser>, frame: Option<&mut Frame>, status: i32) {
            let Some(frame) = frame.filter(|frame| frame.is_main() == 1) else { return };
            let url = owned(frame.url());
            note(json!({"ev": "load_end", "role": self.role, "url": url, "status": status}));
            if self.role == "popup" {
                // The window hands a popup's first document its scripts the same way.
                frame.execute_java_script(Some(&CefString::from("window.__zcGuest = 1;")), None, 1);
            }
            if self.role == "opener" && url.contains("/opener?") {
                fire_case();
            }
        }
    }
}

wrap_resource_request_handler! {
    struct ProbeResource { gate: Arc<Gate> }
    impl ResourceRequestHandler {
        fn on_before_resource_load(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            callback: Option<&mut Callback>,
        ) -> ReturnValue {
            let Some(callback) = callback else { return ReturnValue::CONTINUE; };
            let mut held = self.gate.held.lock().unwrap();
            if self.gate.refused.load(Ordering::Acquire) { return ReturnValue::CANCEL; }
            if self.gate.open.load(Ordering::Acquire) { return ReturnValue::CONTINUE; }
            held.push(HeldRequest(callback.clone()));
            ReturnValue::CONTINUE_ASYNC
        }
    }
}

wrap_request_handler! {
    struct ProbeRequest { role: &'static str, gate: Arc<Gate> }
    impl RequestHandler {
        fn resource_request_handler(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _request: Option<&mut Request>,
            is_navigation: i32,
            _is_download: i32,
            _initiator: Option<&CefString>,
            _disable_default: Option<&mut i32>,
        ) -> Option<ResourceRequestHandler> {
            (self.role == "popup" && is_navigation == 1 && !self.gate.open.load(Ordering::Acquire))
                .then(|| ProbeResource::new(self.gate.clone()))
        }
        fn on_before_browse(
            &self,
            _browser: Option<&mut Browser>,
            frame: Option<&mut Frame>,
            request: Option<&mut Request>,
            _gesture: i32,
            _redirect: i32,
        ) -> i32 {
            let url = request.map(|request| owned(request.url())).unwrap_or_default();
            let method = "";
            let main = frame.is_some_and(|frame| frame.is_main() == 1);
            let allowed = browsable(&url);
            note(json!({"ev": "before_browse", "role": self.role, "url": url, "main": main, "allowed": allowed, "m": method}));
            (!allowed) as i32
        }
        fn on_open_urlfrom_tab(
            &self,
            _browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            url: Option<&CefString>,
            disposition: WindowOpenDisposition,
            gesture: i32,
        ) -> i32 {
            note(json!({"ev": "open_url_from_tab", "role": self.role, "url": text(url),
                "disposition": format!("{disposition:?}"), "gesture": gesture}));
            0
        }
    }
}

wrap_task! {
    struct DetachView { browser: Browser }
    impl Task {
        fn execute(&self) {
            let Some(host) = self.browser.host() else { return };
            let view = host.window_handle().cast::<AnyObject>();
            if !view.is_null() {
                unsafe { let _: () = msg_send![view, removeFromSuperview]; }
            }
        }
    }
}

wrap_life_span_handler! {
    struct ProbeLife { role: &'static str, gate: Arc<Gate> }
    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            let Some(browser) = browser else { return };
            let url = browser.main_frame().map(|frame| owned(frame.url())).unwrap_or_default();
            note(json!({"ev": "after_created", "role": self.role, "url": url}));
            with(|probe| probe.open += 1);
            if self.role == "popup" {
                dress_popup(browser, self.gate.clone());
                with(|probe| probe.popups.push(browser.clone()));
            } else {
                with(|probe| probe.opener = Some(browser.clone()));
            }
        }
        fn do_close(&self, browser: Option<&mut Browser>) -> i32 {
            note(json!({"ev": "do_close", "role": self.role}));
            if let Some(browser) = browser {
                let mut task = DetachView::new(browser.clone());
                post_task(ThreadId::UI, Some(&mut task));
            }
            1
        }
        fn on_before_popup(
            &self,
            browser: Option<&mut Browser>,
            _frame: Option<&mut Frame>,
            _popup_id: i32,
            target_url: Option<&CefString>,
            target_frame_name: Option<&CefString>,
            disposition: WindowOpenDisposition,
            gesture: i32,
            _features: Option<&PopupFeatures>,
            window_info: Option<&mut WindowInfo>,
            client: Option<&mut Option<Client>>,
            _settings: Option<&mut BrowserSettings>,
            _extra: Option<&mut Option<DictionaryValue>>,
            no_javascript_access: Option<&mut i32>,
        ) -> i32 {
            let url = text(target_url);
            note(json!({"ev": "before_popup", "url": url, "frame": text(target_frame_name),
                "disposition": format!("{disposition:?}"), "gesture": gesture,
                "no_js": no_javascript_access.as_ref().map(|value| **value)}));
            let (real, parent) = with(|probe| (probe.real, probe.parent));
            if !(url.is_empty() || browsable(&url)) {
                return 1;
            }
            if !real {
                // What the window did before: cancel, and walk the opener to the address.
                if !url.is_empty()
                    && let Some(frame) = browser.and_then(|browser| browser.main_frame())
                {
                    frame.load_url(Some(&CefString::from(url.as_str())));
                }
                return 1;
            }
            let (Some(window_info), Some(client)) = (window_info, client) else { return 1 };
            let mut info = WindowInfo::default().set_as_child(
                parent as _,
                &Rect { x: 0, y: 0, width: 800, height: 600 },
            );
            info.hidden = 1;
            *window_info = info;
            let popup = ProbeClient::new("popup", Arc::new(Gate::default()));
            *client = Some(popup);
            0
        }
        fn on_before_popup_aborted(&self, _browser: Option<&mut Browser>, popup_id: i32) {
            note(json!({"ev": "popup_aborted", "id": popup_id}));
        }
        fn on_before_close(&self, browser: Option<&mut Browser>) {
            note(json!({"ev": "before_close", "role": self.role}));
            let gone = browser.map(|browser| browser.identifier());
            let role = self.role;
            let done = with(|probe| {
                probe.popups.retain(|one| Some(one.identifier()) != gone);
                // Before t-90671 a popped page's window.close() closed the opener
                // itself. The run goes on in a new opener, so one case cannot hide
                // the rest.
                if role == "opener" {
                    probe.opener = None;
                }
                probe.open = probe.open.saturating_sub(1);
                probe.closing && probe.open == 0
            });
            if done { quit_message_loop(); }
        }
    }
}

wrap_task! {
    struct NextCase {}
    impl Task {
        fn execute(&self) { next_case(); }
    }
}

fn after(ms: i64) {
    let mut task = NextCase::new();
    post_delayed_task(ThreadId::UI, Some(&mut task), ms);
}

fn next_case() {
    let (popups, opener, case, origin) = with(|probe| {
        (
            std::mem::take(&mut probe.popups),
            probe.opener.clone(),
            probe.case,
            probe.origin.clone(),
        )
    });
    for popup in popups {
        if let Some(host) = popup.host() {
            host.close_browser(1);
        }
    }
    let Some(opener) = opener else {
        if case >= CASES.len() {
            with(|probe| probe.closing = true);
            quit_message_loop();
        } else {
            start();
        }
        return;
    };
    let Some(&(name, _)) = CASES.get(case) else {
        with(|probe| probe.closing = true);
        if let Some(host) = opener.host() {
            host.close_browser(1);
        }
        return;
    };
    note(json!({"ev": "case", "case": name}));
    if let Some(frame) = opener.main_frame() {
        frame.load_url(Some(&CefString::from(
            format!("{origin}/opener?case={name}").as_str(),
        )));
    }
    with(|probe| probe.case += 1);
    after(CASE_MS);
}

fn fire_case() {
    let (opener, case) = with(|probe| (probe.opener.clone(), probe.case));
    let (Some(opener), Some(&(_, gesture))) =
        (opener, case.checked_sub(1).and_then(|at| CASES.get(at)))
    else {
        return;
    };
    send(
        &opener,
        "Runtime.evaluate",
        json!({"expression": "window.__fire()", "userGesture": gesture}),
        10,
    );
}

wrap_browser_process_handler! {
    struct ProbeProcess {}
    impl BrowserProcessHandler {
        fn on_context_initialized(&self) { start(); }
    }
}

wrap_app! {
    struct ProbeApp {}
    impl App {
        fn browser_process_handler(&self) -> Option<BrowserProcessHandler> { Some(ProbeProcess::new()) }
        fn on_before_command_line_processing(&self, _process: Option<&CefString>, line: Option<&mut CommandLine>) {
            if let Some(line) = line {
                line.append_switch(Some(&CefString::from("use-mock-keychain")));
            }
        }
    }
}

fn start() {
    let (parent, origin) = with(|probe| (probe.parent, probe.origin.clone()));
    let window = WindowInfo::default().set_as_child(
        parent as _,
        &Rect {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        },
    );
    let mut client = ProbeClient::new("opener", Arc::new(Gate::default()));
    let created = browser_host_create_browser(
        Some(&window),
        Some(&mut client),
        Some(&CefString::from(
            format!("{origin}/report?case=boot&via=boot").as_str(),
        )),
        Some(&BrowserSettings::default()),
        None,
        None,
    );
    assert_eq!(created, 1, "the first browser was refused");
    after(1_000);
}

/// CEF insists on an NSApplication that speaks its protocol; this is the same
/// contract the window adds to its own application class.
fn application_class() -> &'static AnyClass {
    extern "C-unwind" fn handling(_this: *mut AnyObject, _sel: Sel) -> Bool {
        Bool::NO
    }
    extern "C-unwind" fn set_handling(_this: *mut AnyObject, _sel: Sel, _value: Bool) {}
    let parent = AnyClass::get(c"NSApplication").expect("AppKit");
    let mut builder = ClassBuilder::new(c"ZeroCodePopupProbeApp", parent).expect("one class");
    unsafe {
        builder.add_method(
            sel!(isHandlingSendEvent),
            handling as extern "C-unwind" fn(_, _) -> _,
        );
        builder.add_method(
            sel!(setHandlingSendEvent:),
            set_handling as extern "C-unwind" fn(_, _, _),
        );
    }
    builder.register()
}

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let option = |name: &str| {
        arguments
            .iter()
            .position(|one| one == name)
            .and_then(|at| arguments.get(at + 1))
            .cloned()
    };
    let mode = option("--mode").expect("--mode cancel|real|settled");
    let origin = option("--origin").expect("--origin");
    let events = PathBuf::from(option("--events").expect("--events"));
    let root = PathBuf::from(option("--root").expect("--root"));

    let executable = std::env::current_exe().expect("the probe's own path");
    let contents = executable.parent().unwrap().parent().unwrap();
    let framework = contents.join("Frameworks/Chromium Embedded Framework.framework");
    let library = CString::new(
        framework
            .join("Chromium Embedded Framework")
            .to_string_lossy()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        unsafe { load_library(Some(&*library.as_ptr())) },
        1,
        "CEF did not load"
    );
    let _ = api_hash(sys::CEF_API_VERSION_LAST, 0);

    let class = application_class();
    let application: *mut AnyObject = unsafe { msg_send![class, sharedApplication] };
    // Prohibited: no Dock icon, no menu bar, never the front app.
    let _: Bool = unsafe { msg_send![application, setActivationPolicy: 2isize] };
    let window = offscreen_window();
    let parent: *mut AnyObject = unsafe { msg_send![window, contentView] };

    with(|probe| {
        probe.real = mode == "real" || mode == "settled";
        probe.settled = mode == "settled";
        probe.origin = origin;
        probe.parent = parent as usize;
        probe.downloads = root.clone();
        probe.events = Some(std::fs::File::create(&events).expect("the events file"));
    });

    let args = cef::args::Args::new();
    let settings = Settings {
        no_sandbox: 0,
        browser_subprocess_path: CefString::from(
            contents
                .join("Frameworks/ZeroCode Helper.app/Contents/MacOS/ZeroCode Helper")
                .to_string_lossy()
                .as_ref(),
        ),
        framework_dir_path: CefString::from(framework.to_string_lossy().as_ref()),
        root_cache_path: CefString::from(root.to_string_lossy().as_ref()),
        cache_path: CefString::from(root.to_string_lossy().as_ref()),
        ..Default::default()
    };
    let mut app = ProbeApp::new();
    assert_eq!(
        cef::initialize(
            Some(args.as_main_args()),
            Some(&settings),
            Some(&mut app),
            std::ptr::null_mut(),
        ),
        1,
        "CEF did not start"
    );
    run_message_loop();
    cef::shutdown();
}

/// A window that is never ordered in: it has a view to host in and no pixels on
/// any screen.
fn offscreen_window() -> *mut AnyObject {
    use objc2::Encode;
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Point(f64, f64);
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Size(f64, f64);
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Frame(Point, Size);
    unsafe impl Encode for Point {
        const ENCODING: objc2::Encoding =
            objc2::Encoding::Struct("CGPoint", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for Size {
        const ENCODING: objc2::Encoding =
            objc2::Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
    }
    unsafe impl Encode for Frame {
        const ENCODING: objc2::Encoding =
            objc2::Encoding::Struct("CGRect", &[Point::ENCODING, Size::ENCODING]);
    }
    let class = AnyClass::get(c"NSWindow").expect("AppKit");
    unsafe {
        let window: *mut AnyObject = msg_send![class, alloc];
        let frame = Frame(Point(-20_000.0, -20_000.0), Size(800.0, 600.0));
        msg_send![window, initWithContentRect: frame, styleMask: 0usize, backing: 2usize, defer: false]
    }
}
