//! Guest script policy, shared by the window and the native compatibility probe.

/// The observation ring, planted in every browser pane at document start —
/// after the guest cloak (docs/design/
/// browser-door-for-agents.md §2.1). One file, pure JS, no IPC: the page only
/// writes three bounded rings in the shared guest state slot, and Rust reads
/// them by polling `take` over the same eval callback the menu road uses
/// (`browser_menu_take`). The script guards itself to the main frame and to a
/// single planting; wry re-runs it on every top-level navigation, which is
/// exactly when a fresh document needs a fresh ring.
pub(super) const BROWSER_CLOAK_JS: &str = include_str!("../../../ui/browser-cloak.js");
pub(super) const BROWSER_RING_JS: &str = include_str!("../../../ui/browser-ring.js");

/// The only guest document-start scripts, in execution order. Both the
/// builder and native controller consume this registry; these are never
/// WINDOW_PARTS or assets loaded by the main UI.
pub(super) const BROWSER_GUEST_SCRIPTS: &[&str] = &[BROWSER_CLOAK_JS, BROWSER_RING_JS];

/// One name for the guest's non-enumerable ring/menu slot. Templates and the
/// JS harness both read this file; no brand-specific globals reach the page.
pub(super) const BROWSER_GUEST_KEY: &str = include_str!("../../../ui/browser-guest-key.txt");

pub(super) fn browser_guest_script(source: &str) -> String {
    source.replace("__GUEST_STATE__", BROWSER_GUEST_KEY.trim())
}

/// Tauri prepends core/plugin scripts with non-configurable globals. Erasing
/// them from a running page is impossible. A macOS guest is born on blank;
/// replace its controller's scripts and remove its IPC handlers before the
/// first destination request. Future navigations inherit this controller.
/// Native app notification APIs and the main webview keep their controller.
#[cfg(target_os = "macos")]
// Only the wry pane cleans its host controller this way; the CEF pane never
// carries Tauri's host scripts. Compiled-but-unused in the Chromium build,
// where the wry caller is cfg'd out (t-3621).
#[cfg_attr(
    all(target_os = "macos", feature = "chromium-browser"),
    allow(dead_code)
)]
pub(super) fn prepare_browser_guest(
    pane: &tauri::Webview,
    timeout: std::time::Duration,
) -> Result<(), String> {
    use objc2::MainThreadOnly;
    use objc2_foundation::NSString;
    use objc2_web_kit::{WKUserScript, WKUserScriptInjectionTime, WKWebView};

    let (answer, read) = std::sync::mpsc::sync_channel(1);
    pane.with_webview(move |platform| {
        // `with_webview` executes on the main thread and owns a live
        // WKWebView for this closure. Every Objective-C call stays here.
        let view: &WKWebView = unsafe { &*platform.inner().cast() };
        unsafe {
            let controller = view.configuration().userContentController();
            controller.removeAllUserScripts();
            controller.removeAllScriptMessageHandlers();
            for source in BROWSER_GUEST_SCRIPTS {
                let script = WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                    WKUserScript::alloc(view.mtm()),
                    &NSString::from_str(&browser_guest_script(source)),
                    WKUserScriptInjectionTime::AtDocumentStart,
                    true,
                );
                controller.addUserScript(&script);
            }
        }
        let _ = answer.send(());
    })
    .map_err(|error| error.to_string())?;
    read.recv_timeout(timeout)
        .map_err(|error| format!("guest script isolation timed out: {error}"))
}
