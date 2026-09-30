//! The numbers and the pure rules of the page renderer (t-18558): every
//! constant the PDF and the picture are decided by, in one table, and the
//! judgments that need no WebKit — how tall a picture may be, what the settle
//! script answers, how many pages a PDF has, how big a PNG is.
//!
//! No Tauri and no other module of this crate is in here, so the window's
//! renderer ([`crate::artifact_render`]), the proof harness that drives a real
//! `WKWebView` without the person's window, and the unit tests read the very same
//! numbers.

use std::time::Duration;

// ---- the page (PDF) -------------------------------------------------------

/// A4 portrait, in points (210 × 297 mm at 72 to the inch).
pub(crate) const A4_WIDTH_PT: f64 = 595.276;
pub(crate) const A4_HEIGHT_PT: f64 = 841.89;
/// The margin on every side of a PDF page, in points. Zero: the page is laid
/// out the way it was on screen, and a page that wants a margin says so in its
/// own `@page` or padding rather than have the export frame a dark page in
/// white paper.
pub(crate) const PDF_MARGIN_PT: f64 = 0.0;

// ---- the picture (PNG) ----------------------------------------------------

/// The width a page is laid out at for its picture, in points — a reading
/// width, so a responsive page shows its desktop layout.
pub(crate) const PNG_WIDTH_PT: f64 = 1280.0;
/// Pixels to the point in the picture: 2 gives a 2560-pixel-wide file.
pub(crate) const PNG_SCALE: f64 = 2.0;
/// The tallest a picture is, in points — 16384 pixels at [`PNG_SCALE`], the
/// most a bitmap this size is asked to hold. A taller page keeps its top and
/// the result says it was cut.
pub(crate) const PNG_MAX_HEIGHT_PT: f64 = 8192.0;
/// The height the pane starts at, before the page's own height is known.
pub(crate) const PNG_START_HEIGHT_PT: f64 = 800.0;

// ---- the wait -------------------------------------------------------------

/// How long the pane is given to finish loading the page.
pub(crate) const LOAD_BUDGET: Duration = Duration::from_secs(12);
/// How long a loaded page is given to settle: its fonts ready and two animation
/// frames drawn.
pub(crate) const SETTLE_BUDGET: Duration = Duration::from_secs(6);
/// How often the settle is asked again.
pub(crate) const SETTLE_POLL: Duration = Duration::from_millis(50);
/// How long the two animation frames are waited for before the page counts as
/// settled without them: a view WebKit thinks hidden runs few frames or none,
/// and that is not a page that never settles.
pub(crate) const FRAME_CAP_MS: u32 = 1500;
/// How long a print job is given from its start to its file.
pub(crate) const PRINT_BUDGET: Duration = Duration::from_secs(15);
/// The whole render, from asking for the pane to holding the bytes. Under the
/// door shim's own deadline (`ARTIFACT_SHIM_TIMEOUT_SECS`, asserted where both
/// are in sight), so the door answers rather than times out.
pub(crate) const RENDER_BUDGET: Duration = Duration::from_secs(25);

// ---- the settle -----------------------------------------------------------

/// The script that starts a page settling: fonts ready and two animation
/// frames, or the frame cap. `evaluateJavaScript` does not wait for a promise,
/// so it only arms a flag that [`POLL_SCRIPT`] reads.
#[must_use]
pub(crate) fn arm_script() -> String {
    format!(
        "(() => {{ window.__zcSettled = false; \
         const frames = new Promise((resolve) => {{ let seen = 0; \
         const step = () => {{ seen += 1; if (seen >= 2) resolve(); else requestAnimationFrame(step); }}; \
         requestAnimationFrame(step); setTimeout(resolve, {FRAME_CAP_MS}); }}); \
         const fonts = document.fonts && document.fonts.ready ? document.fonts.ready : Promise.resolve(); \
         const settled = () => {{ window.__zcSettled = true; }}; \
         Promise.all([fonts, frames]).then(settled, settled); \
         return 'armed'; }})()"
    )
}

/// The script that asks whether the page has settled, and how big it is:
/// `settled,width,height` — one for settled, zero for not yet.
pub(crate) const POLL_SCRIPT: &str = "(() => { const d = document.documentElement; const b = document.body; \
     return [window.__zcSettled === true ? 1 : 0, \
     Math.max(d.scrollWidth, b ? b.scrollWidth : 0), \
     Math.max(d.scrollHeight, b ? b.scrollHeight : 0)].join(','); })()";

/// What [`POLL_SCRIPT`] answered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Settled {
    pub(crate) ready: bool,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// Read [`POLL_SCRIPT`]'s answer; `None` for anything else.
#[must_use]
pub(crate) fn parse_settled(answer: &str) -> Option<Settled> {
    let mut parts = answer.trim().split(',');
    let ready = parts.next()?.trim();
    let width: f64 = parts.next()?.trim().parse().ok()?;
    let height: f64 = parts.next()?.trim().parse().ok()?;
    if parts.next().is_some() || !width.is_finite() || !height.is_finite() {
        return None;
    }
    Some(Settled {
        ready: match ready {
            "1" => true,
            "0" => false,
            _ => return None,
        },
        width,
        height,
    })
}

// ---- the picture's height -------------------------------------------------

/// How tall the picture is, and whether the page was cut to fit under the cap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PngPlan {
    pub(crate) height_pt: f64,
    pub(crate) truncated: bool,
}

/// The picture's height for a page `content_height_pt` tall: the page's whole
/// height up to [`PNG_MAX_HEIGHT_PT`], rounded up to a whole point; above the
/// cap the top is kept and the plan says it was cut. Never below one point.
#[must_use]
pub(crate) fn plan_png(content_height_pt: f64) -> PngPlan {
    let content = if content_height_pt.is_finite() {
        content_height_pt.max(1.0).ceil()
    } else {
        PNG_START_HEIGHT_PT
    };
    PngPlan {
        height_pt: content.min(PNG_MAX_HEIGHT_PT),
        truncated: content > PNG_MAX_HEIGHT_PT,
    }
}

/// The `snapshotWidth` (points) that makes the picture [`PNG_WIDTH_PT`] ×
/// [`PNG_SCALE`] pixels wide on a screen `backing` device pixels to the point.
#[must_use]
pub(crate) fn snapshot_points(backing: f64) -> f64 {
    PNG_WIDTH_PT * PNG_SCALE
        / if backing.is_finite() && backing >= 1.0 {
            backing
        } else {
            1.0
        }
}

// ---- the files ------------------------------------------------------------

/// The pixel size a PNG's header states, or `None` for anything that is not a
/// PNG with an `IHDR` first.
#[must_use]
pub(crate) fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || !bytes.starts_with(SIGNATURE) || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let read = |from: usize| {
        u32::from_be_bytes([
            bytes[from],
            bytes[from + 1],
            bytes[from + 2],
            bytes[from + 3],
        ])
    };
    Some((read(16), read(20)))
}

/// The number of pages a PDF holds, counted as its `/Type /Page` objects (not
/// `/Pages`); `None` for anything that does not open with `%PDF-` or names no
/// page — a PDF that keeps its page objects packed away answers `None` rather
/// than a wrong number.
#[must_use]
pub(crate) fn pdf_pages(bytes: &[u8]) -> Option<u32> {
    const MARK: &[u8] = b"/Type";
    if !bytes.starts_with(b"%PDF-") {
        return None;
    }
    let mut pages = 0u32;
    let mut at = 0usize;
    while let Some(found) = bytes[at..]
        .windows(MARK.len())
        .position(|window| window == MARK)
    {
        let mut next = at + found + MARK.len();
        at = next;
        while bytes.get(next).is_some_and(u8::is_ascii_whitespace) {
            next += 1;
        }
        if bytes[next.min(bytes.len())..].starts_with(b"/Page")
            && !bytes
                .get(next + b"/Page".len())
                .is_some_and(u8::is_ascii_alphanumeric)
        {
            pages += 1;
        }
    }
    (pages > 0).then_some(pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page is cut only above the cap, keeps its top, and is never empty.
    #[test]
    fn a_picture_is_the_pages_whole_height_up_to_the_cap() {
        assert_eq!(
            plan_png(3400.2),
            PngPlan {
                height_pt: 3401.0,
                truncated: false
            }
        );
        assert_eq!(
            plan_png(PNG_MAX_HEIGHT_PT),
            PngPlan {
                height_pt: PNG_MAX_HEIGHT_PT,
                truncated: false
            }
        );
        assert_eq!(
            plan_png(PNG_MAX_HEIGHT_PT + 0.5),
            PngPlan {
                height_pt: PNG_MAX_HEIGHT_PT,
                truncated: true
            },
            "a page a fraction over the cap was not cut"
        );
        assert_eq!(plan_png(40_000.0).height_pt, PNG_MAX_HEIGHT_PT);
        assert!(plan_png(40_000.0).truncated);
        for odd in [0.0, -5.0] {
            assert_eq!(plan_png(odd).height_pt, 1.0, "{odd}");
        }
        assert_eq!(plan_png(f64::NAN).height_pt, PNG_START_HEIGHT_PT);
    }

    /// The cap keeps a picture inside what a bitmap can hold, at the states
    /// scale, and A4 is A4.
    #[test]
    fn the_tables_numbers_agree_with_each_other() {
        assert!(PNG_MAX_HEIGHT_PT * PNG_SCALE <= 16_384.0);
        assert!(PNG_START_HEIGHT_PT < PNG_MAX_HEIGHT_PT);
        assert!((A4_HEIGHT_PT / A4_WIDTH_PT - 297.0 / 210.0).abs() < 1e-3);
        assert!(PDF_MARGIN_PT * 2.0 < A4_WIDTH_PT);
        assert!(SETTLE_BUDGET > Duration::from_millis(u64::from(FRAME_CAP_MS)));
        for phase in [LOAD_BUDGET, SETTLE_BUDGET, PRINT_BUDGET] {
            assert!(
                phase < RENDER_BUDGET,
                "{phase:?} leaves the whole render no room"
            );
        }
    }

    /// The width a picture states is what a screen of any backing scale is
    /// asked for, and a nonsense scale asks for the plain one.
    #[test]
    fn the_snapshot_width_gives_the_stated_pixels_on_any_screen() {
        assert_eq!(snapshot_points(2.0), PNG_WIDTH_PT);
        assert_eq!(snapshot_points(1.0), PNG_WIDTH_PT * PNG_SCALE);
        assert_eq!(snapshot_points(0.0), PNG_WIDTH_PT * PNG_SCALE);
        assert_eq!(snapshot_points(f64::NAN), PNG_WIDTH_PT * PNG_SCALE);
    }

    #[test]
    fn the_settle_answer_reads_three_numbers_and_nothing_else() {
        assert_eq!(
            parse_settled("1,1280,3400"),
            Some(Settled {
                ready: true,
                width: 1280.0,
                height: 3400.0
            })
        );
        assert_eq!(
            parse_settled(" 0,640,480\n").map(|one| one.ready),
            Some(false)
        );
        for bad in [
            "",
            "1,1280",
            "1,1280,3400,9",
            "2,1,1",
            "x,1,1",
            "1,a,1",
            "1,1,inf",
        ] {
            assert_eq!(parse_settled(bad), None, "{bad:?}");
        }
    }

    /// The script waits for fonts and two frames, and gives the frames a cap.
    #[test]
    fn the_arm_script_waits_for_fonts_and_two_frames_under_a_cap() {
        let script = arm_script();
        assert!(script.contains("document.fonts.ready"), "{script}");
        assert_eq!(
            script.matches("requestAnimationFrame").count(),
            2,
            "{script}"
        );
        assert!(script.contains("seen >= 2"), "{script}");
        assert!(
            script.contains(&format!("setTimeout(resolve, {FRAME_CAP_MS})")),
            "{script}"
        );
        assert!(script.contains("__zcSettled = true"), "{script}");
        assert!(POLL_SCRIPT.contains("__zcSettled === true"));
        assert_eq!(
            script.matches('{').count(),
            script.matches('}').count(),
            "the script's braces do not pair: {script}"
        );
    }

    #[test]
    fn a_png_states_its_size_in_its_header() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&2560u32.to_be_bytes());
        png.extend_from_slice(&6800u32.to_be_bytes());
        assert_eq!(png_size(&png), Some((2560, 6800)));
        assert_eq!(png_size(&png[..20]), None, "a header cut short");
        assert_eq!(png_size(b"<!doctype html><main>hi</main>........"), None);
        png[12] = b'X';
        assert_eq!(png_size(&png), None, "a first chunk that is not IHDR");
    }

    #[test]
    fn a_pdf_counts_its_page_objects_and_not_its_page_tree() {
        let pdf = b"%PDF-1.4\n1 0 obj\n<< /Type /Pages /Count 3 /Kids [2 0 R] >>\nendobj\n\
            2 0 obj\n<< /Type /Page /Parent 1 0 R >>\nendobj\n\
            3 0 obj\n<< /Type/Page /Parent 1 0 R >>\nendobj\n\
            4 0 obj\n<< /Type\n/Page\n/Parent 1 0 R >>\nendobj\n\
            5 0 obj\n<< /Type /PageLabels >>\nendobj\n";
        assert_eq!(pdf_pages(pdf), Some(3));
        assert_eq!(pdf_pages(b"<html>/Type /Page</html>"), None, "not a PDF");
        assert_eq!(pdf_pages(b"%PDF-1.4\n<< /Type /Catalog >>"), None);
        assert_eq!(
            pdf_pages(b"%PDF-1.4\n<< /Type"),
            None,
            "a mark at the very end"
        );
    }
}
