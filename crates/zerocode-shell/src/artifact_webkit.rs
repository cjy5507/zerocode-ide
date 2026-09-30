//! The macOS WebKit half of the page renderer (t-18558): raw objc2 on one
//! `WKWebView`, with no Tauri and no other module of this crate in it — so the
//! very same functions run behind the window's hidden pane
//! ([`crate::artifact_render`]) and behind the small proof harness that drives a
//! real `WKWebView` without the person's window.
//!
//! Every function is called on the main thread (the `with_webview` closure
//! hands one over). WebKit answers a call by a completion that lands on the
//! main thread too, and each completion sends its answer down the channel the
//! async side waits on with a deadline — nothing here blocks the main thread.

use std::ffi::c_void;
use std::path::Path;
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol as _};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPaperOrientation, NSPrintInfo,
    NSPrintJobSavingURL, NSPrintOperation, NSPrintSaveJob, NSWindow,
};
use objc2_foundation::{
    NSDictionary, NSError, NSNumber, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use objc2_web_kit::{WKSnapshotConfiguration, WKWebView};

/// One frame of `view` as PNG bytes — the road `cmd::fs::snapshot_webview_png`
/// and the export both walk. A `width` (points) is WebKit's own resize of the
/// picture: the thumbnails take a page laid out for reading at card size, the
/// export takes it at the pixel width it states. `None` keeps the view's size.
pub(crate) fn take_snapshot_png(
    mtm: MainThreadMarker,
    view: &WKWebView,
    width: Option<f64>,
    tx: Sender<Result<Vec<u8>, String>>,
) {
    let block = RcBlock::new(move |image: *mut NSImage, error: *mut NSError| {
        let answer = if image.is_null() {
            Err(if error.is_null() {
                "스냅샷이 비어 왔습니다".to_string()
            } else {
                unsafe { &*error }.localizedDescription().to_string()
            })
        } else {
            // TIFF is the representation NSImage always holds; the bitmap rep
            // re-encodes it as the PNG the <img> side of the editor actually
            // wants.
            unsafe { &*image }
                .TIFFRepresentation()
                .and_then(|tiff| NSBitmapImageRep::imageRepWithData(&tiff))
                .and_then(|rep| unsafe {
                    rep.representationUsingType_properties(
                        NSBitmapImageFileType::PNG,
                        &NSDictionary::new(),
                    )
                })
                .map(|png| png.to_vec())
                .ok_or_else(|| "PNG 인코딩에 실패했습니다".to_string())
        };
        let _ = tx.send(answer);
    });
    let configuration = width.map(|points| {
        let configuration = unsafe { WKSnapshotConfiguration::new(mtm) };
        unsafe {
            configuration.setSnapshotWidth(Some(&NSNumber::new_f64(points)));
        }
        configuration
    });
    unsafe {
        view.takeSnapshotWithConfiguration_completionHandler(configuration.as_deref(), &block);
    }
}

/// How many device pixels one point of `view` is — the window's backing scale
/// (2 on a Retina screen), 1 for a view no window holds yet.
pub(crate) fn backing_scale(view: &WKWebView) -> f64 {
    view.window()
        .map_or(1.0, |window| window.backingScaleFactor())
}

/// Run `script` in the page and send its string answer — an empty string for
/// `undefined` or `null`, the page's own message for a script that throws.
/// `evaluateJavaScript` does not wait for a promise, so a script that needs
/// time arms itself and is asked again.
pub(crate) fn evaluate_string(view: &WKWebView, script: &str, tx: Sender<Result<String, String>>) {
    let block = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        let answer = if !error.is_null() {
            Err(unsafe { &*error }.localizedDescription().to_string())
        } else if result.is_null() {
            Ok(String::new())
        } else {
            unsafe { &*result }
                .downcast_ref::<NSString>()
                .map(ToString::to_string)
                .ok_or_else(|| "스크립트의 답이 글이 아닙니다".to_string())
        };
        let _ = tx.send(answer);
    });
    unsafe {
        view.evaluateJavaScript_completionHandler(&NSString::from_str(script), Some(&block));
    }
}

/// Ask WebKit to paint page backgrounds when it prints — `shouldPrintBackgrounds`
/// (macOS 13.3+), on this view's own preferences, so a PDF keeps the colours the
/// person saw. `false` when this macOS does not know the setting.
pub(crate) fn allow_print_backgrounds(view: &WKWebView) -> bool {
    let preferences = unsafe { view.configuration().preferences() };
    if !preferences.respondsToSelector(sel!(setShouldPrintBackgrounds:)) {
        return false;
    }
    unsafe { preferences.setShouldPrintBackgrounds(true) };
    true
}

/// The paper the PDF is cut into, in points, portrait.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Paper {
    pub(crate) width: f64,
    pub(crate) height: f64,
    /// The margin on all four sides.
    pub(crate) margin: f64,
}

struct PrintDoneIvars {
    tx: Sender<Result<(), String>>,
}

define_class!(
    // SAFETY:
    // - The superclass NSObject does not have any subclassing requirements.
    // - `PrintDone` does not implement `Drop`.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "ZerocodeArtifactPrintDone"]
    #[ivars = PrintDoneIvars]
    struct PrintDone;

    impl PrintDone {
        // The selector `runOperationModalForWindow:delegate:didRunSelector:contextInfo:`
        // calls once the job is over: (operation, success, contextInfo).
        #[unsafe(method(printOperationDidRun:success:contextInfo:))]
        fn did_run(&self, _operation: &AnyObject, success: bool, _context: *mut c_void) {
            let _ = self.ivars().tx.send(if success {
                Ok(())
            } else {
                Err("인쇄 작업이 실패했습니다".to_string())
            });
        }
    }
);

impl PrintDone {
    fn new(mtm: MainThreadMarker, tx: Sender<Result<(), String>>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PrintDoneIvars { tx });
        // SAFETY: the signature of NSObject's `init` is correct.
        unsafe { msg_send![super(this), init] }
    }
}

/// Print the page in `view` into the PDF file at `out`, paginated on `paper`:
/// `WKWebView.printOperationWithPrintInfo` run as a save job with no panel — the
/// page's `@media print` rules apply, text stays text, and WebKit breaks the
/// pages. The job is asynchronous by WebKit's design (`run` on the main thread
/// prints nothing): the job ends by `tx` — `Ok` once the file is written.
pub(crate) fn print_pdf(
    mtm: MainThreadMarker,
    view: &WKWebView,
    window: &NSWindow,
    out: &Path,
    paper: Paper,
    tx: Sender<Result<(), String>>,
) -> Result<(), String> {
    if !view.respondsToSelector(sel!(printOperationWithPrintInfo:)) {
        return Err("이 macOS의 WebKit은 PDF 인쇄를 지원하지 않습니다".to_string());
    }
    let path = out
        .to_str()
        .ok_or_else(|| "PDF 임시 경로가 글자로 읽히지 않습니다".to_string())?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    let info = NSPrintInfo::new();
    info.setPaperSize(NSSize::new(paper.width, paper.height));
    info.setOrientation(NSPaperOrientation::Portrait);
    info.setTopMargin(paper.margin);
    info.setBottomMargin(paper.margin);
    info.setLeftMargin(paper.margin);
    info.setRightMargin(paper.margin);
    info.setHorizontallyCentered(false);
    info.setVerticallyCentered(false);
    // A save job writes the file itself, at the URL its dictionary names.
    info.setJobDisposition(unsafe { NSPrintSaveJob });
    // SAFETY: the dictionary holds print attributes; a URL is the value
    // `NSPrintJobSavingURL` asks for.
    unsafe {
        let attributes = info.dictionary();
        let _: () = msg_send![&*attributes, setObject: &*url, forKey: NSPrintJobSavingURL];
    }
    // SAFETY: the selector exists (asked above).
    let operation: Retained<NSPrintOperation> = unsafe { view.printOperationWithPrintInfo(&info) };
    operation.setShowsPrintPanel(false);
    operation.setShowsProgressPanel(false);
    // A print view with no frame prints nothing.
    if let Some(printing) = operation.view() {
        printing.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(paper.width, paper.height),
        ));
    }
    let done = PrintDone::new(mtm, tx);
    // SAFETY: an object pointer is an `AnyObject`; the selector is the one
    // `PrintDone` implements, with the signature AppKit calls it with.
    let delegate: &AnyObject = unsafe { &*Retained::as_ptr(&done).cast::<AnyObject>() };
    unsafe {
        operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            window,
            Some(delegate),
            Some(sel!(printOperationDidRun:success:contextInfo:)),
            std::ptr::null_mut(),
        );
    }
    // AppKit keeps its delegate unretained and calls it once, when the job
    // ends — maybe long after this returns. The object is a few bytes, so it
    // lives for the process rather than risk a call into a freed one.
    std::mem::forget(done);
    Ok(())
}
