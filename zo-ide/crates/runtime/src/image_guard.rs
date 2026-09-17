//! Provider-side image dimension guard.
//!
//! Anthropic rejects an oversized image with a fatal, non-retryable
//! `400 invalid_request_error`, and it enforces **two** dimension ceilings:
//! [`MAX_IMAGE_DIMENSION`] for an ordinary request, and the much tighter
//! [`MANY_IMAGE_MAX_DIMENSION`] once a request carries many images
//! (`... exceed max allowed size for many-image requests: 2000 pixels`).
//! The guard therefore clamps to [`IMAGE_CLAMP_DIMENSION`] — see that constant
//! for why the strict ceiling is applied unconditionally rather than only when
//! an image count crosses some threshold.
//!
//! Because the full conversation history is re-sent every turn, one oversized
//! image baked into a stored block (a pasted screenshot, a full-page browser
//! capture staged by `read_image`, an MCP screenshot tool) *wedges the whole
//! session*: every subsequent request — including a fresh user message that
//! carries no image at all — re-hits the same 400 until the poison image is
//! removed.
//!
//! This module is the single source of truth for keeping images within the
//! cap. It is wired at two seams:
//!
//! * **Ingest** (`read_image`, `tools` crate): clamp when the image is first
//!   staged, so it never enters history oversized.
//! * **Wire lowering** (`convert_messages`): clamp again when stored history is
//!   lowered to the provider wire form. This is what *un-wedges an
//!   already-poisoned session* without history surgery, and it also covers the
//!   paste and MCP staging paths that do not pass through `read_image`.
//!
//! The dimension check is header-only: on the raw-bytes ingest path
//! ([`guard_image_bytes`]) via [`image::ImageReader::into_dimensions`], and on
//! the hot base64 wire path ([`guard_wire_image_base64`]) by decoding only a
//! bounded header-sized *prefix* of the payload — so a common in-cap image
//! never pays a full multi-MB base64 decode when the whole history is lowered
//! every turn. The expensive full decode + resize + re-encode runs *only* for
//! the rare oversized image (which must be decoded to be downscaled) or an
//! image whose header does not fit the probe window (a correctness fallback).
//! Rescaled output is always re-encoded as PNG (lossless, universally
//! accepted), so a marginal codec (e.g. WEBP encode) can never turn a
//! recoverable image into an unrecoverable one.

use std::io::Cursor;

use base64::Engine as _;

/// Maximum width or height (in pixels) Anthropic accepts when a request carries
/// a *single* image. A dimension strictly greater than this triggers the fatal
/// 400; a dimension equal to it is accepted.
pub const MAX_IMAGE_DIMENSION: u32 = 8000;

/// Maximum width or height Anthropic accepts once a request carries **many**
/// images. Reported verbatim as
/// `messages.73.content.16.image.source.base64.data: At least one of the image
/// dimensions exceed max allowed size for many-image requests: 2000 pixels`.
///
/// This is the ceiling that actually bites in a long session: a screenshot from
/// a Retina display is ~3000px wide, sails past neither check while the
/// conversation is short, and then wedges *every* subsequent turn the moment
/// enough images have accumulated for the request to count as many-image —
/// including turns that add no image at all, because history is re-sent whole.
pub const MANY_IMAGE_MAX_DIMENSION: u32 = 2000;

/// The box the guard actually downscales into: **always** the strict
/// many-image ceiling, never the single-image one.
///
/// Clamping conditionally — on an image count, say — would make an image's wire
/// bytes depend on how many *other* images the conversation happens to carry.
/// The image that tipped the request over the threshold would silently rewrite
/// every image before it, changing the cached prefix and costing a full
/// prompt-cache miss on a turn that added one screenshot. A single
/// count-independent cap keeps the lowered bytes stable for the life of the
/// session.
///
/// Nothing is lost by the tighter box: the provider downsamples to roughly
/// 1568px on the long edge for token accounting regardless, so pixels above
/// that are paid for in upload bytes and redeemed for nothing.
pub const IMAGE_CLAMP_DIMENSION: u32 = MANY_IMAGE_MAX_DIMENSION;

/// Maximum **base64** payload size Anthropic accepts for a single image.
///
/// The provider measures the encoded block, not the decoded pixels, and reports
/// the violation as a fatal, non-retryable
/// `400 invalid_request_error: … image.source.base64: image exceeds 10 MB maximum:
/// 13781780 bytes > 10485760 bytes` — observed in the wild from a `read_image` of
/// a large screenshot, which killed the whole turn ("갑자기 끊김") and, being
/// stored in history, would have re-killed every turn after it.
///
/// A dimension-legal image can still blow this: 8000×8000 is inside the pixel cap
/// and nowhere near inside 10 MB. That is why the size cap is a separate check
/// rather than a consequence of the dimension one.
pub const MAX_IMAGE_BASE64_BYTES: usize = 10 * 1024 * 1024;

/// Shrink target, as a percentage of [`MAX_IMAGE_BASE64_BYTES`]. Re-encoded size is
/// not exactly predictable, so aim under the cap rather than at it.
const BYTE_BUDGET_HEADROOM_PERCENT: usize = 85;

/// How many progressive shrink passes to attempt before giving up. The first pass
/// scales from the measured overshoot, so this is a safety net for images whose
/// re-encoded size does not track pixel count the way the estimate assumes.
const MAX_SHRINK_ATTEMPTS: u32 = 5;

/// Encoded length of `raw_len` bytes of base64 — what the provider actually
/// measures. Standard base64 emits 4 characters per 3 bytes, padded.
#[must_use]
pub const fn base64_len(raw_len: usize) -> usize {
    raw_len.div_ceil(3) * 4
}

/// Header-probe budget in *base64 characters* for the wire fast path (4 chars
/// encode 3 bytes → ~132 KiB decoded). Sized to comfortably contain the
/// dimension fields of every supported format: PNG/GIF/WEBP put them in the
/// first ~40 bytes, while a JPEG's SOF marker can sit behind large APP1 (EXIF
/// thumbnail) / APP2 (ICC) segments — anything past this window falls back to a
/// full decode. Kept a whole number of 4-char base64 groups so a prefix slice
/// decodes cleanly (interior groups carry no padding).
const DIMENSION_PROBE_B64_CHARS: usize = 176 * 1024;

/// Most images the provider accepts in one request — and therefore the most
/// distinct images one lowering pass can present. Used to bound the rescale
/// memo below.
const MAX_IMAGES_PER_REQUEST: usize = 100;

/// Memo of the *expensive* wire outcomes, keyed by a digest of the source
/// base64.
///
/// Clamping to [`IMAGE_CLAMP_DIMENSION`] moves the ordinary screenshot from the
/// cheap header-probe path onto the full decode + resize + re-encode path, and
/// history is lowered on **every** turn — measured at ~350 ms per Retina
/// screenshot, so a session holding a handful of them would pay seconds of CPU
/// per request forever. The bytes of a stored image never change, so the result
/// is a pure function of them and is computed once per process instead.
///
/// Memory is bounded by construction rather than by a tuned budget: an entry is
/// only ever a *clamped* image, which is strictly smaller than the original the
/// session already holds in history, and the map is capped at the most images a
/// single request may carry. The memo can therefore never hold more than the
/// history it mirrors.
static RESCALE_MEMO: std::sync::LazyLock<std::sync::Mutex<RescaleMemo>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(RescaleMemo::default()));

#[derive(Default)]
struct RescaleMemo {
    /// Insertion-ordered so the oldest entry is evicted first. A pass over
    /// history revisits every image, so recency ordering would evict exactly
    /// the entry needed next; insertion order does not.
    entries: Vec<([u8; 32], WireImageOutcome)>,
}

impl RescaleMemo {
    fn get(&self, key: &[u8; 32]) -> Option<WireImageOutcome> {
        self.entries
            .iter()
            .find(|(stored, _)| stored == key)
            .map(|(_, outcome)| outcome.clone())
    }

    fn insert(&mut self, key: [u8; 32], outcome: WireImageOutcome) {
        if self.entries.iter().any(|(stored, _)| *stored == key) {
            return;
        }
        if self.entries.len() >= MAX_IMAGES_PER_REQUEST {
            self.entries.remove(0);
        }
        self.entries.push((key, outcome));
    }
}

/// Digest the source base64 so the memo key is fixed-size and collision-safe.
/// A cheaper fingerprint (length plus a few sampled characters) would risk
/// returning one image's pixels for another — the one failure this cache must
/// not have.
fn memo_key(data_b64: &str) -> [u8; 32] {
    use sha2::Digest as _;
    let mut hasher = sha2::Sha256::new();
    hasher.update(data_b64.as_bytes());
    hasher.finalize().into()
}

/// Run `compute` unless this exact payload has already been guarded.
fn memoized(data_b64: &str, compute: impl FnOnce() -> WireImageOutcome) -> WireImageOutcome {
    let key = memo_key(data_b64);
    if let Some(hit) = RESCALE_MEMO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return hit;
    }
    let outcome = compute();
    RESCALE_MEMO
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, outcome.clone());
    outcome
}

/// Outcome of guarding one image's dimensions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageGuardOutcome {
    /// Within the cap, or the dimensions could not be read at all (so the image
    /// cannot be *proven* oversized). Send the original bytes unchanged — never
    /// destroy a payload we merely failed to inspect.
    Keep,
    /// Confirmed oversized and successfully downscaled to fit within
    /// [`IMAGE_CLAMP_DIMENSION`] on both axes. Always re-encoded as PNG.
    Rescaled {
        /// New media type — always `image/png` after re-encoding.
        media_type: String,
        /// Downscaled PNG bytes (not base64).
        bytes: Vec<u8>,
    },
    /// Header dimensions proved the image oversized, but the pixels could not be
    /// decoded/re-encoded (e.g. a truncated payload). The caller must drop the
    /// pixels — resending them would only re-trigger the fatal 400.
    DropOversized {
        /// The oversized dimensions read from the header, for the placeholder
        /// note the caller substitutes.
        width: u32,
        height: u32,
    },
}

/// Guard one image's raw (decoded) bytes against the dimension cap.
///
/// See the module docs for the header-only fast path and the PNG re-encode
/// policy. This never returns an error: an unreadable header degrades to
/// [`ImageGuardOutcome::Keep`] (we cannot prove it oversized), matching the
/// "never damage what we could not inspect" rule.
#[must_use]
pub fn guard_image_bytes(bytes: &[u8]) -> ImageGuardOutcome {
    let Some((width, height)) = read_dimensions(bytes) else {
        // Dimensions unreadable → cannot prove oversized → leave untouched.
        return ImageGuardOutcome::Keep;
    };
    if width <= IMAGE_CLAMP_DIMENSION && height <= IMAGE_CLAMP_DIMENSION {
        // Dimensions are fine; the payload may still exceed the provider's size
        // cap, which is a separate 400 and just as fatal.
        if base64_len(bytes.len()) <= MAX_IMAGE_BASE64_BYTES {
            return ImageGuardOutcome::Keep;
        }
        return match shrink_to_byte_budget(bytes) {
            Some(png) => ImageGuardOutcome::Rescaled {
                media_type: "image/png".to_string(),
                bytes: png,
            },
            None => ImageGuardOutcome::DropOversized { width, height },
        };
    }
    // Confirmed oversized: pay the full decode + resize + PNG re-encode.
    match downscale_to_png(bytes) {
        // The dimension fit says nothing about the encoded size, so the result
        // goes through the size budget as well — otherwise a clamped
        // screenshot still 400s on bytes.
        Some(png) if base64_len(png.len()) <= MAX_IMAGE_BASE64_BYTES => {
            ImageGuardOutcome::Rescaled {
                media_type: "image/png".to_string(),
                bytes: png,
            }
        }
        Some(png) => match shrink_to_byte_budget(&png) {
            Some(smaller) => ImageGuardOutcome::Rescaled {
                media_type: "image/png".to_string(),
                bytes: smaller,
            },
            None => ImageGuardOutcome::DropOversized { width, height },
        },
        None => ImageGuardOutcome::DropOversized { width, height },
    }
}

/// Guard a base64-encoded wire image.
///
/// The hot path is intentionally bounded: first decode only a fixed-size base64
/// prefix and read dimensions from that header slice. If the image is proven
/// in-cap, return [`WireImageOutcome::Keep`] without allocating/decoding the
/// full image payload. Full base64 decode is reserved for:
///
/// * confirmed-oversized images, which must be decoded to be downscaled; and
/// * rare headers whose dimension fields do not fit in the probe window, where
///   a full decode is the correctness fallback.
///
/// Both of those are memoized (see `RESCALE_MEMO`) — history is lowered every
/// turn, and a stored image's bytes never change, so the work is done once per
/// process rather than once per request.
#[must_use]
pub fn guard_wire_image_base64(data_b64: &str) -> WireImageOutcome {
    // Size first, because on the wire it is free: the provider measures this very
    // string's length, so an over-cap payload is proven without decoding a byte.
    // This is also what un-wedges a session that already stored one — the stored
    // history is re-sent every turn, so without a wire-side size check the 400
    // repeats forever.
    let over_size_cap = data_b64.len() > MAX_IMAGE_BASE64_BYTES;
    if let Some((width, height)) = read_dimensions_from_base64_probe(data_b64) {
        if width <= IMAGE_CLAMP_DIMENSION && height <= IMAGE_CLAMP_DIMENSION && !over_size_cap {
            return WireImageOutcome::Keep;
        }
        return memoized(data_b64, || {
            guard_oversized_wire_image(data_b64, width, height)
        });
    }

    // Probe could not read dimensions (e.g. JPEG SOF after a very large EXIF/ICC
    // segment, or malformed/truncated base64 prefix). Fall back to the old full
    // decode path for correctness; an undecodable full payload stays untouched.
    // Memoized like the oversized path: this branch also pays a full decode, and
    // a payload the probe cannot classify would pay it on every single turn.
    memoized(data_b64, || {
        let Some(bytes) = decode_full_base64(data_b64) else {
            return WireImageOutcome::Keep;
        };
        match guard_image_bytes(&bytes) {
            ImageGuardOutcome::Keep => WireImageOutcome::Keep,
            ImageGuardOutcome::Rescaled { media_type, bytes } => WireImageOutcome::Rescaled {
                media_type,
                data_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
            },
            ImageGuardOutcome::DropOversized { width, height } => {
                WireImageOutcome::Drop { width, height }
            }
        }
    })
}

/// Decode only enough base64 to cover the image header and read dimensions from
/// that partial byte buffer. Returns `None` when the header is not readable from
/// the probe window; callers fall back to full decode in that case.
fn read_dimensions_from_base64_probe(data_b64: &str) -> Option<(u32, u32)> {
    let bytes = data_b64.as_bytes();
    let mut len = bytes.len().min(DIMENSION_PROBE_B64_CHARS);
    if bytes.len() > DIMENSION_PROBE_B64_CHARS {
        // Interior base64 has no padding, so decode a complete number of
        // 4-character groups. (When the whole payload is shorter than the probe,
        // keep its real length so normal tail padding is preserved.)
        len -= len % 4;
    }
    if len == 0 {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&bytes[..len])
        .ok()?;
    read_dimensions(&decoded)
}

fn decode_full_base64(data_b64: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(data_b64.as_bytes())
        .ok()
}

fn guard_oversized_wire_image(data_b64: &str, width: u32, height: u32) -> WireImageOutcome {
    let Some(bytes) = decode_full_base64(data_b64) else {
        // The header prefix proved the payload would violate the provider's
        // dimension cap. If the tail is not valid base64, resending it cannot
        // succeed either; drop with a placeholder instead of re-wedging.
        return WireImageOutcome::Drop { width, height };
    };
    match guard_image_bytes(&bytes) {
        // Reuse the raw-bytes guard so the wire seam and the ingest seam apply
        // one policy — dimensions *and* size — instead of two that can drift.
        ImageGuardOutcome::Rescaled { media_type, bytes } => WireImageOutcome::Rescaled {
            media_type,
            data_b64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        },
        ImageGuardOutcome::Keep => WireImageOutcome::Keep,
        ImageGuardOutcome::DropOversized { width, height } => {
            WireImageOutcome::Drop { width, height }
        }
    }
}

/// Base64-level counterpart of [`ImageGuardOutcome`] for the wire seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireImageOutcome {
    /// Send the original base64 block unchanged.
    Keep,
    /// Replace with a downscaled PNG (base64), setting `media_type`.
    Rescaled {
        /// Always `image/png`.
        media_type: String,
        /// Downscaled PNG, base64-encoded.
        data_b64: String,
    },
    /// Drop the image; substitute a text placeholder built from these
    /// (oversized) dimensions.
    Drop {
        /// Oversized width read from the header.
        width: u32,
        /// Oversized height read from the header.
        height: u32,
    },
}

/// Human-readable placeholder for a dropped oversized image, so the model still
/// learns an image was present and why it is absent.
#[must_use]
pub fn oversized_placeholder(width: u32, height: u32) -> String {
    format!(
        "[image dropped: {width}x{height}px could not be brought under the provider \
         limits ({IMAGE_CLAMP_DIMENSION}px per dimension, {MAX_IMAGE_BASE64_BYTES} bytes \
         base64)]"
    )
}

/// Read an image's dimensions from its header only (no full pixel decode).
/// Returns `None` when the format is unrecognized or the header is malformed.
fn read_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Fully decode, downscale to fit within [`IMAGE_CLAMP_DIMENSION`] on both axes
/// (aspect ratio preserved), and re-encode as PNG. `None` when the pixels
/// cannot be decoded or the PNG encode fails.
fn downscale_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(bytes).ok()?;
    // `resize` fits the image within the target box, preserving aspect ratio;
    // since at least one dimension exceeds the cap it only ever shrinks here.
    let resized = image.resize(
        IMAGE_CLAMP_DIMENSION,
        IMAGE_CLAMP_DIMENSION,
        image::imageops::FilterType::Triangle,
    );
    let mut out = Cursor::new(Vec::new());
    resized.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Shrink `bytes` until its base64 form fits [`MAX_IMAGE_BASE64_BYTES`], keeping
/// aspect ratio and re-encoding as PNG (the module's lossless policy).
///
/// The first pass scales from the measured overshoot — PNG size tracks pixel count
/// closely enough for screenshots, which is what actually hits this cap — and the
/// remaining passes shrink geometrically in case it does not. `None` means the
/// image could not be brought under the cap, and the caller must drop the pixels
/// rather than send a payload the provider will fatally reject.
fn shrink_to_byte_budget(bytes: &[u8]) -> Option<Vec<u8>> {
    // Integer arithmetic: the headroom is a fixed fraction, so there is no reason
    // to route the budget through a float at all.
    let budget = MAX_IMAGE_BASE64_BYTES / 100 * BYTE_BUDGET_HEADROOM_PERCENT;
    let image = image::load_from_memory(bytes).ok()?;
    // Never grow, and never exceed the dimension cap on the way.
    let mut width = image.width().min(IMAGE_CLAMP_DIMENSION);
    let mut height = image.height().min(IMAGE_CLAMP_DIMENSION);
    #[expect(
        clippy::cast_precision_loss,
        reason = "same: a starting scale, not an exact size prediction"
    )]
    let first_scale = (budget as f64 / base64_len(bytes.len()) as f64).sqrt();
    let mut scale = first_scale.clamp(0.05, 1.0);
    for _ in 0..MAX_SHRINK_ATTEMPTS {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "scale is clamped to (0, 1] and dimensions are u32-bounded"
        )]
        let (target_w, target_h) = (
            ((f64::from(width) * scale) as u32).max(1),
            ((f64::from(height) * scale) as u32).max(1),
        );
        let resized = image.resize(
            target_w,
            target_h,
            image::imageops::FilterType::Triangle,
        );
        let mut out = Cursor::new(Vec::new());
        resized.write_to(&mut out, image::ImageFormat::Png).ok()?;
        let encoded = out.into_inner();
        if base64_len(encoded.len()) <= MAX_IMAGE_BASE64_BYTES {
            return Some(encoded);
        }
        width = target_w;
        height = target_h;
        scale = 0.7;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbImage};

    /// Encode a solid-color `w`x`h` image to PNG bytes.
    fn png_bytes(w: u32, h: u32) -> Vec<u8> {
        let image = DynamicImage::ImageRgb8(RgbImage::new(w, h));
        let mut out = Cursor::new(Vec::new());
        image
            .write_to(&mut out, ImageFormat::Png)
            .expect("encode test PNG");
        out.into_inner()
    }

    #[test]
    fn within_cap_image_is_kept_untouched() {
        let bytes = png_bytes(100, 100);
        assert_eq!(guard_image_bytes(&bytes), ImageGuardOutcome::Keep);
    }

    #[test]
    fn exactly_at_cap_is_kept() {
        // The clamp is the max *allowed*; only strictly greater is rejected.
        let bytes = png_bytes(IMAGE_CLAMP_DIMENSION, 10);
        assert_eq!(guard_image_bytes(&bytes), ImageGuardOutcome::Keep);
    }

    #[test]
    fn retina_screenshot_is_clamped_to_the_many_image_ceiling() {
        // The reported wedge: a 3024x1964 Retina screenshot is comfortably
        // inside the single-image ceiling, so the guard used to pass it
        // through. It then 400s with "max allowed size for many-image
        // requests: 2000 pixels" once enough images accumulate, and — because
        // history is re-sent whole — keeps 400ing on every later turn.
        let bytes = png_bytes(3024, 1964);
        let outcome = guard_image_bytes(&bytes);
        let ImageGuardOutcome::Rescaled { bytes, .. } = outcome else {
            panic!("a screenshot over the many-image ceiling must rescale, got {outcome:?}");
        };
        let (w, h) = read_dimensions(&bytes).expect("dims");
        assert!(
            w <= MANY_IMAGE_MAX_DIMENSION && h <= MANY_IMAGE_MAX_DIMENSION,
            "clamped under the many-image ceiling: {w}x{h}"
        );
    }

    #[test]
    fn clamp_is_independent_of_how_many_images_a_request_carries() {
        // The guard must not consult an image count: an image's lowered bytes
        // have to be identical whether it is the only image in the session or
        // the fiftieth, or a later screenshot would rewrite the cached prefix
        // of every earlier one.
        let bytes = png_bytes(3024, 1964);
        let first = guard_image_bytes(&bytes);
        let later = guard_image_bytes(&bytes);
        assert_eq!(first, later, "same input, same lowered bytes");
    }

    #[test]
    fn oversized_image_is_downscaled_within_cap_preserving_aspect() {
        // 9000x300 → long axis pinned to the clamp, short axis scaled by the
        // same ratio (rounding is the resizer's, so allow a pixel of slack).
        let bytes = png_bytes(9000, 300);
        let outcome = guard_image_bytes(&bytes);
        let ImageGuardOutcome::Rescaled { media_type, bytes } = outcome else {
            panic!("oversized image must be rescaled, got {outcome:?}");
        };
        assert_eq!(media_type, "image/png", "rescaled output is always PNG");
        let (w, h) = read_dimensions(&bytes).expect("rescaled PNG has readable dims");
        assert!(w <= IMAGE_CLAMP_DIMENSION && h <= IMAGE_CLAMP_DIMENSION, "fits cap: {w}x{h}");
        assert_eq!(w, IMAGE_CLAMP_DIMENSION, "long axis pinned to the cap");
        let expected_h = u32::try_from(300 * u64::from(IMAGE_CLAMP_DIMENSION) / 9000)
            .expect("scaled height fits u32");
        assert!(
            h.abs_diff(expected_h) <= 1,
            "short axis scaled proportionally: {h} vs ~{expected_h}"
        );
    }

    #[test]
    fn oversized_tall_image_is_downscaled_on_the_height_axis() {
        // Mirrors the real symptom: a full-page browser screenshot far taller
        // than it is wide.
        let bytes = png_bytes(1200, 12000);
        let ImageGuardOutcome::Rescaled { bytes, .. } = guard_image_bytes(&bytes) else {
            panic!("tall screenshot must be rescaled");
        };
        let (w, h) = read_dimensions(&bytes).expect("dims");
        assert_eq!(h, IMAGE_CLAMP_DIMENSION, "height pinned to the cap");
        assert!(w <= IMAGE_CLAMP_DIMENSION);
    }

    #[test]
    fn undecodable_bytes_are_kept_not_destroyed() {
        // Cannot read dimensions → cannot prove oversized → keep.
        assert_eq!(guard_image_bytes(b"not an image at all"), ImageGuardOutcome::Keep);
    }

    #[test]
    fn wire_base64_roundtrips_rescale() {
        let bytes = png_bytes(4000, 500);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let WireImageOutcome::Rescaled { media_type, data_b64 } = guard_wire_image_base64(&b64)
        else {
            panic!("oversized wire image must rescale");
        };
        assert_eq!(media_type, "image/png");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data_b64.as_bytes())
            .expect("valid base64 out");
        let (w, h) = read_dimensions(&decoded).expect("dims");
        assert!(w <= IMAGE_CLAMP_DIMENSION && h <= IMAGE_CLAMP_DIMENSION);
    }

    #[test]
    fn the_wire_seam_rescales_a_given_payload_only_once() {
        // History is lowered on every turn, so an image that must be clamped
        // would otherwise pay a full decode + resize + re-encode per request.
        // Second lowering of the same bytes must be served from the memo — and
        // must return exactly what the first one produced.
        // Dimensions unique to this test: the memo is process-global and the
        // test binary runs threads in parallel, so a fixture shared with
        // another test could arrive already warmed and void the measurement.
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes(3024, 1964));

        let first_start = std::time::Instant::now();
        let first = guard_wire_image_base64(&b64);
        let first_elapsed = first_start.elapsed();
        assert!(matches!(first, WireImageOutcome::Rescaled { .. }), "clamped");

        let second_start = std::time::Instant::now();
        let second = guard_wire_image_base64(&b64);
        let second_elapsed = second_start.elapsed();

        assert_eq!(first, second, "memo returns the identical lowered bytes");
        // Generous ratio: the point is that the second call does not repeat the
        // decode/resize/encode, not that it hits any particular latency.
        assert!(
            second_elapsed * 4 < first_elapsed,
            "second lowering must not repeat the work: {first_elapsed:?} then {second_elapsed:?}"
        );
    }

    #[test]
    fn the_memo_never_returns_one_image_for_another() {
        // The key is a digest of the payload, so two near-identical images —
        // the case a cheap length-and-sample fingerprint would confuse — must
        // still resolve to their own results.
        let a = base64::engine::general_purpose::STANDARD.encode(png_bytes(3020, 1960));
        let b = base64::engine::general_purpose::STANDARD.encode(png_bytes(3020, 1961));
        assert_ne!(a, b, "fixtures differ");
        let guarded_a = guard_wire_image_base64(&a);
        let guarded_b = guard_wire_image_base64(&b);
        assert_ne!(guarded_a, guarded_b, "distinct payloads keep distinct results");
    }

    #[test]
    fn wire_base64_within_cap_is_kept() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(png_bytes(64, 64));
        assert_eq!(guard_wire_image_base64(&b64), WireImageOutcome::Keep);
    }

    #[test]
    fn wire_in_cap_header_uses_probe_without_decoding_invalid_tail() {
        // Regression for the hot-path perf defect: stored history is lowered on
        // every turn, so in-cap images must be classified from a bounded header
        // probe and must not allocate/decode the full multi-MB payload. Make the
        // payload longer than the probe and corrupt the tail; the old full-decode
        // path would fail to decode, while the probe path can still read the PNG
        // dimensions from the first bytes and return Keep immediately.
        let mut bytes = png_bytes(64, 64);
        let target_len = (DIMENSION_PROBE_B64_CHARS / 4 * 3) + 4096;
        bytes.resize(target_len, 0xA5);
        let mut b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        assert!(b64.len() > DIMENSION_PROBE_B64_CHARS);
        b64.replace_range(b64.len() - 1.., "@");

        assert_eq!(guard_wire_image_base64(&b64), WireImageOutcome::Keep);
    }

    #[test]
    fn wire_oversized_header_with_invalid_tail_drops_instead_of_rewedging() {
        // If the bounded probe proves the image is oversized but the full base64
        // payload is corrupt, sending the original would still hit the provider
        // dimension cap. Drop it with a placeholder directive instead.
        let mut bytes = png_bytes(IMAGE_CLAMP_DIMENSION + 1, 1);
        let target_len = (DIMENSION_PROBE_B64_CHARS / 4 * 3) + 4096;
        bytes.resize(target_len, 0xA5);
        let mut b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        assert!(b64.len() > DIMENSION_PROBE_B64_CHARS);
        b64.replace_range(b64.len() - 1.., "@");

        assert_eq!(
            guard_wire_image_base64(&b64),
            WireImageOutcome::Drop {
                width: IMAGE_CLAMP_DIMENSION + 1,
                height: 1,
            }
        );
    }

    #[test]
    fn wire_non_base64_is_kept() {
        assert_eq!(guard_wire_image_base64("@@@ not base64 @@@"), WireImageOutcome::Keep);
    }

    #[test]
    fn placeholder_names_dimensions_and_limit() {
        let note = oversized_placeholder(1200, 12000);
        assert!(note.contains("1200x12000"));
        // The limit it names must be the one actually enforced, so a future
        // change to the clamp cannot leave the note quoting a stale ceiling.
        assert!(note.contains(&IMAGE_CLAMP_DIMENSION.to_string()));
    }
}

#[cfg(test)]
mod size_cap_tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, RgbImage};

    /// A PNG of random noise does not compress, so pixel count buys payload bytes
    /// predictably — which is what lets a test build an image over the cap without
    /// depending on the encoder's compression ratio for flat colour.
    fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut image = RgbImage::new(width, height);
        // A cheap deterministic LCG: no rand dependency, and reproducible.
        let mut state: u32 = 0x1234_5678;
        for pixel in image.pixels_mut() {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let bytes = state.to_le_bytes();
            *pixel = image::Rgb([bytes[0], bytes[1], bytes[2]]);
        }
        let mut out = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut out, ImageFormat::Png)
            .expect("encode test PNG");
        out.into_inner()
    }

    #[test]
    fn base64_len_matches_the_encoder() {
        for raw in [0usize, 1, 2, 3, 4, 5, 100, 1024, 100_000] {
            let bytes = vec![0xAB; raw];
            let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
            assert_eq!(
                base64_len(raw),
                encoded.len(),
                "the provider measures this length, so the estimate must be exact"
            );
        }
    }

    /// The reported failure: a dimension-legal screenshot whose base64 payload was
    /// 13.7 MB. The provider rejects it with a fatal 400 that kills the turn, and
    /// — stored in history — every turn after it. The guard must shrink it instead.
    #[test]
    fn a_dimension_legal_but_oversized_payload_is_rescaled_under_the_cap() {
        // Noise at exactly the dimension clamp encodes well past 10 MB, so this
        // exercises the *size* cap with a fixture the dimension cap has no
        // reason to touch.
        let bytes = noisy_png(IMAGE_CLAMP_DIMENSION, IMAGE_CLAMP_DIMENSION);
        assert!(
            base64_len(bytes.len()) > MAX_IMAGE_BASE64_BYTES,
            "fixture must exceed the size cap: {} bytes base64",
            base64_len(bytes.len())
        );
        let (width, height) = read_dimensions(&bytes).expect("readable header");
        assert!(width <= IMAGE_CLAMP_DIMENSION && height <= IMAGE_CLAMP_DIMENSION);

        match guard_image_bytes(&bytes) {
            ImageGuardOutcome::Rescaled { media_type, bytes } => {
                assert_eq!(media_type, "image/png");
                assert!(
                    base64_len(bytes.len()) <= MAX_IMAGE_BASE64_BYTES,
                    "rescaled payload must fit the cap, got {} bytes base64",
                    base64_len(bytes.len())
                );
            }
            other => panic!("expected a rescale, got {other:?}"),
        }
    }

    /// The wire seam is what un-wedges a session that already stored the poison
    /// image, and it must prove the violation from the string length alone.
    #[test]
    fn the_wire_seam_rescales_an_oversized_stored_payload() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(noisy_png(2600, 2600));
        assert!(encoded.len() > MAX_IMAGE_BASE64_BYTES);
        match guard_wire_image_base64(&encoded) {
            WireImageOutcome::Rescaled { media_type, data_b64 } => {
                assert_eq!(media_type, "image/png");
                assert!(data_b64.len() <= MAX_IMAGE_BASE64_BYTES);
            }
            other => panic!("expected a rescale, got {other:?}"),
        }
    }

    /// An in-cap image must still take the free path: no decode, no re-encode, and
    /// byte-identical output. The whole history goes through this every turn.
    #[test]
    fn an_in_cap_payload_is_untouched_by_the_size_check() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(noisy_png(64, 64));
        assert!(encoded.len() <= MAX_IMAGE_BASE64_BYTES);
        assert_eq!(guard_wire_image_base64(&encoded), WireImageOutcome::Keep);
    }
}
