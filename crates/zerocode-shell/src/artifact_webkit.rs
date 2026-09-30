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

use std::cell::OnceCell;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol as _};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSPaperOrientation,
    NSPrintInfo, NSPrintJobSavingURL, NSPrintOperation, NSPrintSaveJob, NSWindow,
    NSWindowStyleMask,
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
    /// The file the job writes: nobody's once the side that waited for it is gone.
    file: PathBuf,
}

/// Say that the print job ended. When nobody is waiting any more — the render's
/// budget ran out and its receiver left before the job did — the file the job has
/// just written is nobody's, and it is removed here rather than left in the temp
/// folder for good.
fn deliver(tx: &Sender<Result<(), String>>, file: &Path, success: bool) {
    let answer = if success {
        Ok(())
    } else {
        Err("인쇄 작업이 실패했습니다".to_string())
    };
    if tx.send(answer).is_err() {
        let _ = std::fs::remove_file(file);
    }
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
        fn did_run(&self, _operation: &AnyObject, success: Bool, _context: *mut c_void) {
            let ivars = self.ivars();
            deliver(&ivars.tx, &ivars.file, success.as_bool());
        }
    }
);

impl PrintDone {
    fn new(mtm: MainThreadMarker, tx: Sender<Result<(), String>>, file: PathBuf) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PrintDoneIvars { tx, file });
        // SAFETY: the signature of NSObject's `init` is correct.
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    /// The window print jobs are run modal for: one of our own, never shown, that
    /// nothing else uses. `runOperationModalForWindow:` holds its window for the
    /// life of the job, and that must never be the person's — the export runs for
    /// seconds while they type. Main thread only, like everything here.
    static PRINT_WINDOW: OnceCell<Retained<NSWindow>> = const { OnceCell::new() };
}

fn print_window(mtm: MainThreadMarker) -> Retained<NSWindow> {
    PRINT_WINDOW.with(|held| {
        held.get_or_init(|| {
            let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0, 1.0));
            // SAFETY: released-when-closed is switched off below, as a window made
            // outside a window controller must have it.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    frame,
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    true,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window
        })
        .clone()
    })
}

/// Print the page in `view` into the PDF file at `out`, paginated on `paper`:
/// `WKWebView.printOperationWithPrintInfo` run as a save job with no panel — the
/// page's `@media print` rules apply, text stays text, and WebKit breaks the
/// pages. The job is asynchronous by WebKit's design (`run` on the main thread
/// prints nothing): the job ends by `tx` — `Ok` once the file is written — and a
/// job whose waiter is gone removes its own file ([`deliver`]). It is run modal
/// for a window of its own ([`print_window`]), never the person's.
pub(crate) fn print_pdf(
    mtm: MainThreadMarker,
    view: &WKWebView,
    out: &Path,
    paper: Paper,
    tx: Sender<Result<(), String>>,
) -> Result<(), String> {
    print_pdf_for(mtm, view, &print_window(mtm), out, paper, tx)
}

/// [`print_pdf`] with the window the job is run modal for named. The window is
/// a parameter so that the proof harness can run the same job for the view's
/// own window, as the first version did, and measure what the private window
/// saves; the export itself always goes through [`print_pdf`].
pub(crate) fn print_pdf_for(
    mtm: MainThreadMarker,
    view: &WKWebView,
    doc_window: &NSWindow,
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
    let done = PrintDone::new(mtm, tx, out.to_path_buf());
    // SAFETY: an object pointer is an `AnyObject`; the selector is the one
    // `PrintDone` implements, with the signature AppKit calls it with.
    let delegate: &AnyObject = unsafe { &*Retained::as_ptr(&done).cast::<AnyObject>() };
    unsafe {
        operation.runOperationModalForWindow_delegate_didRunSelector_contextInfo(
            doc_window,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    /// A job that ends after its waiter left removes the file it wrote; one that
    /// ends while somebody waits leaves the file to them, and a failed one says so.
    #[test]
    fn a_print_job_that_ends_after_its_waiter_left_removes_its_own_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("zerocode-export-late.pdf");

        std::fs::write(&file, b"%PDF-").expect("write");
        let (tx, rx) = channel();
        drop(rx);
        deliver(&tx, &file, true);
        assert!(
            !file.exists(),
            "a file nobody waits for was left in the temp folder"
        );

        std::fs::write(&file, b"%PDF-").expect("write");
        let (tx, rx) = channel();
        deliver(&tx, &file, true);
        assert_eq!(rx.recv().expect("an answer"), Ok(()));
        assert!(file.exists(), "the waiter's file was removed under it");

        let (tx, rx) = channel();
        deliver(&tx, &file, false);
        assert!(rx.recv().expect("an answer").is_err());
    }
}
