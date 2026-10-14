//! The engine's own word on a known navigation, for the wry panes.
//!
//! A handoff waits until the engine itself answered the request it was sent
//! and then reads idle (`browser_nav_state`). The CEF pane answers through
//! its DevTools channel (`chromium_browser`); these are the same two facts
//! for WebKit and WebView2, each read on the main thread.

/// WebKit: the request is sent and its loading state read in one main-thread
/// turn. WebKit counts a request it has taken but not yet started as loading,
/// so `isLoading` right after `loadRequest:` is the answer, and a later
/// `isLoading == NO` is the idle reading.
#[cfg(all(target_os = "macos", not(feature = "chromium-browser")))]
pub(crate) mod webkit {
    use objc2::rc::Retained;
    use objc2_foundation::{NSString, NSURL, NSURLRequest};
    use objc2_web_kit::WKWebView;

    pub(crate) fn request(url: &str) -> Option<Retained<NSURLRequest>> {
        NSURL::URLWithString(&NSString::from_str(url))
            .map(|address| NSURLRequest::requestWithURL(&address))
    }

    /// # Safety
    /// `view` is the pane's live WKWebView, on the main thread.
    pub(crate) unsafe fn send(view: &WKWebView, request: &NSURLRequest) -> crate::NavAnswer {
        unsafe {
            let _navigation = view.loadRequest(request);
            crate::NavAnswer::from_engine(false, view.isLoading())
        }
    }

    /// # Safety
    /// `view` is the pane's live WKWebView, on the main thread.
    pub(crate) unsafe fn loading(view: &WKWebView) -> bool {
        unsafe { view.isLoading() }
    }
}

/// WebView2: `Page.navigate` goes over Chromium's DevTools protocol and is
/// answered through its own completion handler. What is loading comes from
/// WebView2's NavigationStarting and NavigationCompleted, paired by their
/// NavigationId, so a late completion of one navigation never ends another.
/// A pane's loads are unknown when the watch begins — a navigation already
/// under way was not seen starting — and become known only when a navigation
/// seen starting completes, and replaced the document (success, or an error
/// page), with nothing else in flight. A label opened again starts a new
/// watch, so a closed pane's late event never reaches it.
#[cfg(target_os = "windows")]
pub(crate) mod webview2 {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_WEB_ERROR_STATUS, COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED,
        ICoreWebView2,
    };
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, NavigationCompletedEventHandler,
        NavigationStartingEventHandler,
    };
    use windows::core::{BOOL, HSTRING};

    struct Loads {
        watch: u64,
        ids: crate::NavigationIds,
    }

    static NEXT_WATCH: AtomicU64 = AtomicU64::new(1);
    static LOADS: Mutex<BTreeMap<String, Loads>> = Mutex::new(BTreeMap::new());

    fn loads() -> MutexGuard<'static, BTreeMap<String, Loads>> {
        LOADS.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn with_watch(label: &str, watch: u64, change: impl FnOnce(&mut Loads)) {
        if let Some(loads) = loads().get_mut(label).filter(|loads| loads.watch == watch) {
            change(loads);
        }
    }

    pub(crate) fn watch(core: &ICoreWebView2, label: &str) -> windows::core::Result<()> {
        let watch = NEXT_WATCH.fetch_add(1, Ordering::Relaxed);
        loads().insert(
            label.to_string(),
            Loads {
                watch,
                ids: crate::NavigationIds::default(),
            },
        );
        let starting = label.to_string();
        let completed = label.to_string();
        let mut token = 0_i64;
        unsafe {
            core.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut navigation = 0_u64;
                    args.NavigationId(&mut navigation)?;
                    with_watch(&starting, watch, |loads| loads.ids.starting(navigation));
                    Ok(())
                })),
                &mut token,
            )?;
            core.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else {
                        return Ok(());
                    };
                    let mut navigation = 0_u64;
                    let mut success = BOOL::default();
                    let mut status = COREWEBVIEW2_WEB_ERROR_STATUS::default();
                    args.NavigationId(&mut navigation)?;
                    args.IsSuccess(&mut success)?;
                    args.WebErrorStatus(&mut status)?;
                    let replaced = success.as_bool()
                        || status != COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED;
                    with_watch(&completed, watch, |loads| {
                        loads.ids.completed(navigation, replaced)
                    });
                    Ok(())
                })),
                &mut token,
            )?;
        }
        Ok(())
    }

    pub(crate) fn forget(label: &str) {
        loads().remove(label);
    }

    /// `None` while the pane's loads are unknown.
    pub(crate) fn loading(label: &str) -> Option<bool> {
        loads().get(label).and_then(|loads| loads.ids.loading())
    }

    pub(crate) fn send(
        core: &ICoreWebView2,
        url: &str,
        answered: impl FnOnce(crate::NavAnswer) + 'static,
    ) -> windows::core::Result<()> {
        let params = serde_json::json!({ "url": url }).to_string();
        unsafe {
            core.CallDevToolsProtocolMethod(
                &HSTRING::from("Page.navigate"),
                &HSTRING::from(params.as_str()),
                &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                    move |result, json| {
                        let parsed = serde_json::from_str::<serde_json::Value>(&json)
                            .unwrap_or(serde_json::Value::Null);
                        answered(crate::page_navigate_answer(result.is_ok(), &parsed));
                        Ok(())
                    },
                )),
            )
        }
    }
}
