//! The window's page renderer (t-18558): an artifact's 「내보내기」 and the door's
//! `zerocode-artifact export --out x.pdf` draw the page as a PDF or a picture with
//! this window's own WebKit — never Chrome, never another program — in a hidden
//! pane, never in the person's tab.
//!
//! - **A pane of its own, the thumbnails' lock.** A second hidden pane
//!   ([`EXPORT_LABEL`]), built by the very code the thumbnail pane is
//!   ([`hidden_pane`]): parked outside the window's bounds, on the browser
//!   panes' allowlist, absent from `browser_panes`. It takes the thumbnails'
//!   [`render_lock`], so at most one hidden page is drawn at a time and the
//!   thumbnails' size, cache and queue are exactly what they were.
//! - **Two roads.** A PDF is WebKit's print operation (`artifact_webkit::print_pdf`):
//!   real A4 pages, the page's `@media print` rules, text that stays text. A
//!   picture is the pane grown to the page's height at a reading width and the
//!   window's own snapshot road (`cmd::fs::snapshot_webview_png`), reused as it is.
//! - **A bounded wait.** The load, then the settle (fonts ready, two animation
//!   frames), then the job, all inside [`plan::RENDER_BUDGET`] — under the door
//!   shim's own deadline, so the door answers rather than times out.
//!
//! The numbers are one table ([`plan`]); the objc2 calls are `artifact_webkit`.

use std::path::Path;
#[cfg(target_os = "macos")]
use std::sync::PoisonError;
#[cfg(target_os = "macos")]
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Mutex, OnceLock};
#[cfg(target_os = "macos")]
use std::time::Duration;
use std::time::Instant;

use serde::Serialize;
use tauri::AppHandle;
use zerocode_core::artifact_publish::{
    ARTIFACT_SHIM_TIMEOUT_SECS, ExportFormat, PageRenderer, Rendered,
};

use crate::artifact_render_plan as plan;
use crate::artifact_thumbs::{file_address, render_lock};
#[cfg(target_os = "macos")]
use crate::artifact_thumbs::{finished_within, hidden_pane};
#[cfg(target_os = "macos")]
use crate::browser_runtime::blank_page;

const _: () = assert!(
    plan::RENDER_BUDGET.as_secs() < ARTIFACT_SHIM_TIMEOUT_SECS,
    "a render that outlasts the door shim's deadline is answered by a timeout"
);

/// The hidden pane the exports draw in. Not a `browser-N`, like the thumbnail
/// pane: the agents' `tabs` does not list it and no `zerocode-browser` verb can
/// steer it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) const EXPORT_LABEL: &str = "artifact-export";

/// Where the export pane's own page loads land.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn export_finished() -> &'static Mutex<Option<String>> {
    static CELL: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CELL.get_or_init(|| Mutex::new(None))
}

/// The reason code the window translates for a kind this build cannot draw.
pub(crate) const NO_WEBKIT: &str = "no-webkit";

/// One kind an export offers, and whether this build can write it.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct FormatChoice {
    pub(crate) format: ExportFormat,
    pub(crate) available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) why: Option<&'static str>,
}

/// The kinds the export menu offers and which of them `renderer` can write: a
/// copy always, a drawing only where a renderer stands that can draw it — the
/// same answer the door refuses by.
pub(crate) fn format_choices(renderer: Option<&dyn PageRenderer>) -> Vec<FormatChoice> {
    ExportFormat::ALL
        .into_iter()
        .map(|format| {
            let available =
                !format.is_drawn() || renderer.is_some_and(|one| one.can_render(format));
            FormatChoice {
                format,
                available,
                why: (!available).then_some(NO_WEBKIT),
            }
        })
        .collect()
}

/// The window's renderer: what the store hands the export once the window is up.
pub(crate) struct WindowRenderer {
    app: AppHandle,
}

impl WindowRenderer {
    pub(crate) fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl PageRenderer for WindowRenderer {
    fn can_render(&self, format: ExportFormat) -> bool {
        cfg!(target_os = "macos") && format.is_drawn()
    }

    fn render(&self, page: &Path, format: ExportFormat) -> Result<Rendered, String> {
        if !self.can_render(format) {
            return Err(format!(
                ".{}는 이 빌드에서 그리지 못합니다 (macOS의 WebKit이 필요합니다)",
                format.word()
            ));
        }
        // Called from a blocking thread — the door's `spawn_blocking`, the
        // export command's — never the main thread or a runtime worker: the
        // pane's WebKit answers on the main thread, and a main thread waiting
        // here would be a deadlock.
        tauri::async_runtime::block_on(render_page(&self.app, page, format))
    }
}

/// Draw the page at `page` in the export pane, under the render budget.
async fn render_page(
    app: &AppHandle,
    page: &Path,
    format: ExportFormat,
) -> Result<Rendered, String> {
    let address = file_address(page)?;
    let deadline = Instant::now() + plan::RENDER_BUDGET;
    // One hidden page at a time — the thumbnails' lock: an export waits for the
    // card in hand, and the cards for it.
    let _turn = tokio::time::timeout(plan::RENDER_BUDGET, render_lock().lock())
        .await
        .map_err(|_| "다른 페이지를 그리는 중이라 차례가 오지 않았습니다".to_string())?;
    draw(app, &address, format, deadline).await
}

#[cfg(not(target_os = "macos"))]
async fn draw(
    _app: &AppHandle,
    _address: &tauri::Url,
    format: ExportFormat,
    _deadline: Instant,
) -> Result<Rendered, String> {
    Err(format!(
        ".{}는 이 플랫폼에서 그리지 못합니다",
        format.word()
    ))
}

#[cfg(target_os = "macos")]
async fn draw(
    app: &AppHandle,
    address: &tauri::Url,
    format: ExportFormat,
    deadline: Instant,
) -> Result<Rendered, String> {
    let pane = hidden_pane(
        app,
        EXPORT_LABEL,
        (plan::PNG_WIDTH_PT, plan::PNG_START_HEIGHT_PT),
        export_finished(),
    )?;
    let drawn = draw_in(&pane, address, format, deadline).await;
    // Leave the pane on the blank page at its start size, whatever happened: a
    // page still loading must not go on spending the person's network, and a
    // pane grown to a tall page must not keep its memory.
    let _ = pane.navigate(blank_page());
    let _ = pane.set_size(start_size());
    drawn
}

#[cfg(target_os = "macos")]
fn start_size() -> tauri::LogicalSize<f64> {
    tauri::LogicalSize::new(plan::PNG_WIDTH_PT, plan::PNG_START_HEIGHT_PT)
}

/// What is left of the render's budget, at most `cap`.
#[cfg(target_os = "macos")]
fn within(deadline: Instant, cap: Duration) -> Duration {
    deadline.saturating_duration_since(Instant::now()).min(cap)
}

#[cfg(target_os = "macos")]
async fn draw_in(
    pane: &tauri::Webview,
    address: &tauri::Url,
    format: ExportFormat,
    deadline: Instant,
) -> Result<Rendered, String> {
    *export_finished()
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = None;
    pane.set_size(start_size())
        .map_err(|error| error.to_string())?;
    pane.navigate(address.clone())
        .map_err(|error| error.to_string())?;
    if !finished_within(export_finished(), within(deadline, plan::LOAD_BUDGET)).await {
        return Err(format!(
            "페이지가 {}초 안에 불러와지지 않았습니다",
            plan::LOAD_BUDGET.as_secs()
        ));
    }
    let measured = settle(pane, deadline).await?;
    match format {
        ExportFormat::Pdf => print(pane, deadline).await,
        ExportFormat::Png => picture(pane, measured, deadline).await,
        ExportFormat::Html => Err("HTML은 그리지 않고 복사합니다".to_string()),
    }
}

/// Wait on a channel WebKit's completion sends down, off the async workers.
#[cfg(target_os = "macos")]
async fn receive<T: Send + 'static>(
    rx: Receiver<T>,
    budget: Duration,
    what: &str,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || rx.recv_timeout(budget))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|_| format!("{what}의 답이 {budget:?} 안에 오지 않았습니다"))
}

/// Run a script in the pane's page and wait for its string answer.
#[cfg(target_os = "macos")]
async fn evaluate(
    pane: &tauri::Webview,
    script: String,
    budget: Duration,
) -> Result<String, String> {
    let (tx, rx) = channel();
    pane.with_webview(move |platform| {
        // Main thread, by `with_webview`'s contract — WebKit requires it.
        let view: &objc2_web_kit::WKWebView = unsafe { &*platform.inner().cast() };
        crate::artifact_webkit::evaluate_string(view, &script, tx);
    })
    .map_err(|error| error.to_string())?;
    receive(rx, budget, "페이지 스크립트").await?
}

/// Let the loaded page settle — fonts ready and two animation frames drawn, or
/// the frame cap — and answer how big it turned out to be. A page that never
/// settles is a clear error, not a hang.
#[cfg(target_os = "macos")]
async fn settle(pane: &tauri::Webview, deadline: Instant) -> Result<plan::Settled, String> {
    evaluate(
        pane,
        plan::arm_script(),
        within(deadline, plan::SCRIPT_BUDGET),
    )
    .await?;
    let started = Instant::now();
    loop {
        let answer = evaluate(
            pane,
            plan::POLL_SCRIPT.to_string(),
            within(deadline, plan::SCRIPT_BUDGET),
        )
        .await?;
        let settled = plan::parse_settled(&answer)
            .ok_or_else(|| format!("페이지가 알 수 없는 답을 했습니다: {answer}"))?;
        if settled.ready {
            return Ok(settled);
        }
        if started.elapsed() >= plan::SETTLE_BUDGET || Instant::now() >= deadline {
            return Err(format!(
                "페이지가 {}초 안에 준비되지 않았습니다 (글꼴과 화면 그리기를 기다렸습니다)",
                plan::SETTLE_BUDGET.as_secs()
            ));
        }
        tokio::time::sleep(plan::SETTLE_POLL).await;
    }
}

/// The PDF: WebKit's print operation as a save job into a temp file, read back
/// and removed. The file is checked to open with `%PDF-` before it is believed.
#[cfg(target_os = "macos")]
async fn print(pane: &tauri::Webview, deadline: Instant) -> Result<Rendered, String> {
    let file = std::env::temp_dir().join(format!("zerocode-export-{}.pdf", uuid::Uuid::new_v4()));
    let (tx, rx) = channel();
    let path = file.clone();
    pane.with_webview(move |platform| {
        // Main thread, by `with_webview`'s contract — WebKit requires it.
        let view: &objc2_web_kit::WKWebView = unsafe { &*platform.inner().cast() };
        // The marker is the compiler's word for what the closure already knows.
        let mtm = unsafe { objc2::MainThreadMarker::new_unchecked() };
        crate::artifact_webkit::allow_print_backgrounds(view);
        let paper = crate::artifact_webkit::Paper {
            width: plan::A4_WIDTH_PT,
            height: plan::A4_HEIGHT_PT,
            margin: plan::PDF_MARGIN_PT,
        };
        if let Err(why) = crate::artifact_webkit::print_pdf(mtm, view, &path, paper, tx.clone()) {
            let _ = tx.send(Err(why));
        }
    })
    .map_err(|error| error.to_string())?;
    let job = receive(rx, within(deadline, plan::PRINT_BUDGET), "인쇄 작업").await;
    let bytes = match job {
        Ok(Ok(())) => std::fs::read(&file).map_err(|error| error.to_string()),
        Ok(Err(why)) | Err(why) => Err(why),
    };
    let _ = std::fs::remove_file(&file);
    let bytes = bytes?;
    if !bytes.starts_with(b"%PDF-") {
        return Err("인쇄 결과가 PDF가 아닙니다".to_string());
    }
    Ok(Rendered {
        pages: plan::pdf_pages(&bytes),
        bytes,
        ..Rendered::default()
    })
}

/// The picture: the pane grown to the page's height at the reading width (at
/// most the cap — above it the top is kept and the answer says so), then one
/// snapshot of all of it on the window's own snapshot road.
#[cfg(target_os = "macos")]
async fn picture(
    pane: &tauri::Webview,
    measured: plan::Settled,
    deadline: Instant,
) -> Result<Rendered, String> {
    let resize = |height: f64| {
        pane.set_size(tauri::LogicalSize::new(plan::PNG_WIDTH_PT, height))
            .map_err(|error| error.to_string())
    };
    let mut planned = plan::plan_png(measured.height);
    resize(planned.height_pt)?;
    // A page sized by its window grows with it: measure once more, and once
    // more only if it did.
    let grown = plan::plan_png(settle(pane, deadline).await?.height);
    if grown.height_pt > planned.height_pt {
        planned = grown;
        resize(planned.height_pt)?;
        settle(pane, deadline).await?;
    }
    let mut asked = plan::PNG_SNAPSHOT_WIDTH;
    let mut bytes = snapshot(pane, asked).await?;
    let (mut width, mut height) =
        plan::png_size(&bytes).ok_or_else(|| "스냅샷이 PNG가 아닙니다".to_string())?;
    // A screen whose scale WebKit folds into the picture: ask again, once, in proportion.
    if let Some(again) = plan::snapshot_correction(asked, width) {
        asked = again;
        bytes = snapshot(pane, asked).await?;
        (width, height) =
            plan::png_size(&bytes).ok_or_else(|| "스냅샷이 PNG가 아닙니다".to_string())?;
    }
    Ok(Rendered {
        bytes,
        width: Some(width),
        height: Some(height),
        truncated: planned.truncated,
        ..Rendered::default()
    })
}

/// One snapshot of the whole pane at `width` pixels, on the window's own road.
#[cfg(target_os = "macos")]
async fn snapshot(pane: &tauri::Webview, width: f64) -> Result<Vec<u8>, String> {
    crate::cmd::fs::snapshot_webview_png(pane.clone(), Some(width)).await
}

/// A renderer with a fixed answer that remembers what it was asked — the seam's
/// stand-in, since a unit test cannot start WebKit.
#[cfg(test)]
pub(crate) mod fake {
    use std::path::PathBuf;

    use super::*;

    pub(crate) struct FakeRenderer {
        pub(crate) can: bool,
        pub(crate) answer: Result<Rendered, String>,
        asked: Mutex<Vec<(PathBuf, ExportFormat)>>,
    }

    impl FakeRenderer {
        pub(crate) fn answering(answer: Result<Rendered, String>) -> Self {
            Self {
                can: true,
                answer,
                asked: Mutex::new(Vec::new()),
            }
        }

        pub(crate) fn drawing(bytes: &[u8]) -> Self {
            Self::answering(Ok(Rendered {
                bytes: bytes.to_vec(),
                pages: Some(2),
                ..Rendered::default()
            }))
        }

        pub(crate) fn asked(&self) -> Vec<(PathBuf, ExportFormat)> {
            self.asked.lock().unwrap().clone()
        }
    }

    impl PageRenderer for FakeRenderer {
        fn can_render(&self, format: ExportFormat) -> bool {
            self.can && format.is_drawn()
        }

        fn render(&self, page: &Path, format: ExportFormat) -> Result<Rendered, String> {
            self.asked
                .lock()
                .unwrap()
                .push((page.to_path_buf(), format));
            self.answer.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeRenderer;
    use super::*;

    /// A copy is always on offer; a drawing only where a renderer stands that
    /// can draw it, with the reason code the window translates when none does.
    #[test]
    fn the_menu_offers_a_copy_always_and_a_drawing_only_where_one_can_be_drawn() {
        let word = |choices: &[FormatChoice]| -> Vec<(&'static str, bool, Option<&'static str>)> {
            choices
                .iter()
                .map(|one| (one.format.word(), one.available, one.why))
                .collect()
        };
        assert_eq!(
            word(&format_choices(None)),
            [
                ("html", true, None),
                ("pdf", false, Some(NO_WEBKIT)),
                ("png", false, Some(NO_WEBKIT)),
            ]
        );
        let drawing = FakeRenderer::drawing(b"%PDF-");
        assert_eq!(
            word(&format_choices(Some(&drawing))),
            [
                ("html", true, None),
                ("pdf", true, None),
                ("png", true, None)
            ]
        );
        let mut blind = FakeRenderer::drawing(b"%PDF-");
        blind.can = false;
        assert_eq!(
            word(&format_choices(Some(&blind))),
            word(&format_choices(None))
        );
        let json = serde_json::to_value(format_choices(None)).unwrap();
        assert_eq!(json[1]["why"], NO_WEBKIT);
        assert!(json[0].get("why").is_none(), "{json}");
    }
}
