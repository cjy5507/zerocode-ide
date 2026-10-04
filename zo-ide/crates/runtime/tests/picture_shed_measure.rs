//! t-37798 — measurements behind the picture budget. Ignored by default: they
//! print numbers, they do not assert a contract (the contract is in
//! `picture_shed_wire.rs` and `picture_shed_recovery.rs`).
//!
//! ```text
//! cargo test -p runtime --test picture_shed_measure -- --ignored --nocapture --test-threads=1
//! taskpolicy -b <that test binary> --ignored --nocapture --test-threads=1   # efficiency cores
//! ```
//!
//! * `measure_cache_reads_over_a_forty_step_session` — a 40-step synthetic Computer
//!   Use conversation lowered request by request the way the clients lower it,
//!   with the prompt-cache model of `wire_prefix_stability_measure`: what a request
//!   reads from the provider's cache, what it must write, and how often the prefix
//!   is rewritten — with no limit (what main sends until it hits 413), with the
//!   budget, and with one picture left out per request.
//! * `measure_shedding_time` — what leaving pictures out costs a request.
//! * `measure_screenshot_bytes` — what a screenshot costs on the wire in PNG and
//!   in JPEG, and what lightening takes.
//! * `measure_memory_over_a_long_session` — the wire form stays bounded while the
//!   stored history grows.

#![allow(
    clippy::cast_precision_loss,
    reason = "a measurement harness prints sizes as floats; every count here is far below f64's exact range"
)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use api::{InputContentBlock, InputMessage, ToolResultContentBlock};
use base64::Engine as _;
use runtime::image_guard::{lighten_screenshot, SCREENSHOT_JPEG_QUALITY};
use runtime::{
    convert_messages_for, mark_conversation_cache_breakpoints, plan_picture_shed, ContentBlock,
    ConversationMessage, PictureBudget, WireTarget,
};

const MODEL: &str = "claude-opus-5-5";

/// The window helper's PNG budget (`MAX_SCREENSHOT_PNG_BYTES`): a screenshot the
/// Computer tool hands over is at most this many file bytes, 1.2 MB of base64.
const PICTURE_FILE_BYTES: usize = 900_000;

/// The repo's own estimate of what one picture costs in tokens, whatever its bytes.
const PICTURE_TOKENS: u64 = 1_600;

fn png_head() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(1280, 800))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode the PNG header");
    out.into_inner()
}

fn picture(head: &[u8], bytes: usize, salt: u8) -> String {
    let mut file = head.to_vec();
    file.resize(bytes.max(head.len()), salt);
    base64::engine::general_purpose::STANDARD.encode(file)
}

/// A Computer Use history of `steps` looks, each a tool call and a picture.
fn history(steps: usize, picture_bytes: usize) -> Vec<ConversationMessage> {
    let head = png_head();
    let mut messages = vec![ConversationMessage::user_text(
        "Open the settings page and check the sync option.",
    )];
    for index in 0..steps {
        let id = format!("toolu_{index:04}");
        messages.push(ConversationMessage::assistant(vec![ContentBlock::ToolUse {
            id: id.clone(),
            name: "Computer".to_string(),
            input: "{\"action\":\"screenshot\"}".to_string(),
        }]));
        messages.push(ConversationMessage::tool_result_with_images(
            id,
            "Computer",
            format!("step {index}: screenshot taken, the page shows the sync option"),
            false,
            vec![(
                "image/png".to_string(),
                picture(&head, picture_bytes, u8::try_from(index % 200).expect("salt")),
            )],
        ));
    }
    messages
}

fn message_tokens(message: &InputMessage) -> u64 {
    message
        .content
        .iter()
        .map(|block| match block {
            InputContentBlock::Text { text, .. } => text.len() as u64 / 4 + 1,
            InputContentBlock::Image { .. } => PICTURE_TOKENS,
            InputContentBlock::Document { .. } => 0,
            InputContentBlock::ToolUse { name, input, .. } => {
                (name.len() as u64 + input.to_string().len() as u64) / 4 + 8
            }
            InputContentBlock::ToolResult { content, .. } => {
                8 + content
                    .iter()
                    .map(|part| match part {
                        ToolResultContentBlock::Text { text } => text.len() as u64 / 4 + 1,
                        ToolResultContentBlock::Json { value } => value.to_string().len() as u64 / 4 + 1,
                        ToolResultContentBlock::Image { .. } => PICTURE_TOKENS,
                    })
                    .sum::<u64>()
            }
            InputContentBlock::Thinking { thinking, signature } => {
                (thinking.len() + signature.len()) as u64 / 4 + 1
            }
            InputContentBlock::RedactedThinking { data } => data.len() as u64 / 4 + 1,
        })
        .sum()
}

fn has_breakpoint(message: &InputMessage) -> bool {
    message.content.iter().any(|block| {
        matches!(
            block,
            InputContentBlock::Text { cache_control: Some(_), .. }
                | InputContentBlock::ToolUse { cache_control: Some(_), .. }
                | InputContentBlock::ToolResult { cache_control: Some(_), .. }
                | InputContentBlock::Image { cache_control: Some(_), .. }
        )
    })
}

fn fingerprint(message: &InputMessage) -> u64 {
    let mut hasher = DefaultHasher::new();
    serde_json::to_vec(message).expect("serialize").hash(&mut hasher);
    hasher.finish()
}

/// One request as the cache model sees it.
struct Wire {
    fingerprints: Vec<u64>,
    breakpoints: Vec<usize>,
    /// Tokens of messages `..=i`.
    prefix_tokens: Vec<u64>,
    bytes: usize,
    pictures: usize,
    left_out: usize,
}

fn wire(history: &[ConversationMessage], target: WireTarget) -> Wire {
    let plain = convert_messages_for(history, target);
    let mut marked = plain.clone();
    mark_conversation_cache_breakpoints(&mut marked);
    let mut running = 0;
    let prefix_tokens = plain
        .iter()
        .map(|message| {
            running += message_tokens(message);
            running
        })
        .collect();
    let (mut pictures, mut left_out) = (0, 0);
    for block in plain.iter().flat_map(|message| &message.content) {
        if let InputContentBlock::ToolResult { content, .. } = block {
            for part in content {
                match part {
                    ToolResultContentBlock::Image { .. } => pictures += 1,
                    ToolResultContentBlock::Text { text } if text.starts_with("[picture left out") => {
                        left_out += 1;
                    }
                    _ => {}
                }
            }
        }
    }
    Wire {
        fingerprints: plain.iter().map(fingerprint).collect(),
        breakpoints: marked
            .iter()
            .enumerate()
            .filter(|(_, message)| has_breakpoint(message))
            .map(|(index, _)| index)
            .collect(),
        prefix_tokens,
        bytes: serde_json::to_vec(&plain).expect("serialize").len(),
        pictures,
        left_out,
    }
}

/// Tokens the provider's cache would serve `current`: the longest prefix that ends
/// at a breakpoint an EARLIER request wrote and is byte-identical to this one's.
fn cache_read_tokens(current: &Wire, earlier: &[Wire]) -> u64 {
    earlier
        .iter()
        .flat_map(|request| {
            request.breakpoints.iter().filter_map(move |&breakpoint| {
                let same = breakpoint < current.fingerprints.len()
                    && request.fingerprints[..=breakpoint] == current.fingerprints[..=breakpoint];
                same.then(|| current.prefix_tokens[breakpoint])
            })
        })
        .max()
        .unwrap_or(0)
}

#[derive(Default)]
struct Totals {
    read: u64,
    uncached: u64,
    rewrites: u32,
    refused: u32,
    max_bytes: usize,
}

fn run_policy(label: &str, full: &[ConversationMessage], steps: usize, target: WireTarget, ceiling: u64) {
    let mut earlier: Vec<Wire> = Vec::new();
    let mut totals = Totals::default();
    let mut first_over = None;
    for looks in 1..=steps {
        let request = wire(&full[..1 + 2 * looks], target);
        let total_tokens = *request.prefix_tokens.last().unwrap_or(&0);
        let read = cache_read_tokens(&request, &earlier);
        let refused = request.bytes as u64 > ceiling;
        if refused && first_over.is_none() {
            first_over = Some((looks, request.bytes));
        }
        totals.refused += u32::from(refused);
        totals.max_bytes = totals.max_bytes.max(request.bytes);
        if let Some(previous) = earlier.last() {
            let stable = previous.fingerprints.len().saturating_sub(1);
            if previous.fingerprints[..stable] != request.fingerprints[..stable.min(request.fingerprints.len())] {
                totals.rewrites += 1;
            }
        }
        totals.read += read;
        totals.uncached += total_tokens - read;
        if looks == 28 || looks == steps {
            println!(
                "  step {looks:>2}: body {:>6.2} MiB, {:>2} pictures, {:>2} left out, cache read {:>6} of {:>6} tokens",
                request.bytes as f64 / 1_048_576.0,
                request.pictures,
                request.left_out,
                read,
                total_tokens
            );
        }
        earlier.push(request);
    }
    println!(
        "{label}: cache read {} tokens, uncached {} tokens, prefix rewrites {}, requests over the ceiling {}, largest body {:.2} MiB{}",
        totals.read,
        totals.uncached,
        totals.rewrites,
        totals.refused,
        totals.max_bytes as f64 / 1_048_576.0,
        first_over.map_or(String::new(), |(step, bytes)| format!(
            ", first over the ceiling at step {step} ({:.2} MiB)",
            bytes as f64 / 1_048_576.0
        )),
    );
}

#[test]
#[ignore = "measurement: prints numbers"]
fn measure_cache_reads_over_a_forty_step_session() {
    let steps = 40;
    let full = history(steps, PICTURE_FILE_BYTES);
    let ceiling = api::max_request_bytes_for_model(MODEL).expect("catalog ceiling");
    let budget = PictureBudget::for_ceiling(ceiling);
    println!("\n40-step Computer Use session, {PICTURE_FILE_BYTES}-byte screenshots, ceiling {ceiling} bytes");
    println!("before — no budget (what main sends, until the provider refuses it):");
    run_policy("  no limit", &full, steps, WireTarget::for_model("some-unlisted-local-model"), ceiling);
    println!("after — the budget (75% trigger, 50% steps):");
    run_policy("  budget", &full, steps, WireTarget::for_model(MODEL), ceiling);
    println!("for contrast — one picture left out per request (step of one byte):");
    run_policy(
        "  per request",
        &full,
        steps,
        WireTarget::for_model(MODEL).with_picture_budget(Some(PictureBudget {
            trigger: budget.trigger,
            step: 1,
        })),
        ceiling,
    );
}

fn median_and_max(mut samples: Vec<Duration>) -> (f64, f64) {
    samples.sort();
    (
        samples[samples.len() / 2].as_secs_f64() * 1_000.0,
        samples[samples.len() - 1].as_secs_f64() * 1_000.0,
    )
}

fn time<T>(runs: usize, mut work: impl FnMut() -> T) -> (f64, f64) {
    let samples = (0..runs)
        .map(|_| {
            let started = Instant::now();
            let result = work();
            let elapsed = started.elapsed();
            drop(result);
            elapsed
        })
        .collect();
    median_and_max(samples)
}

#[test]
#[ignore = "measurement: prints numbers"]
fn measure_shedding_time() {
    let steps = 40;
    let full = history(steps, PICTURE_FILE_BYTES);
    let target = WireTarget::for_model(MODEL);
    let runs = 15;
    let (plan_median, plan_max) = time(runs, || plan_picture_shed(&full, target));
    let (before_median, before_max) =
        time(runs, || convert_messages_for(&full, WireTarget::for_model("some-unlisted-local-model")));
    let (after_median, after_max) = time(runs, || convert_messages_for(&full, target));
    let plan = plan_picture_shed(&full, target);
    println!(
        "\nshedding time at step {steps} ({} pictures, {} left out, body {:.1} -> {:.1} MiB), {runs} runs, ms (median / max):",
        plan.pictures,
        plan.left_out,
        plan.body_before as f64 / 1_048_576.0,
        plan.body_after as f64 / 1_048_576.0
    );
    println!("  the decision alone (plan_picture_shed):        {plan_median:>8.3} / {plan_max:>8.3}");
    println!("  lowering without the budget (every picture):   {before_median:>8.3} / {before_max:>8.3}");
    println!("  lowering with the budget (older ones are text): {after_median:>8.3} / {after_max:>8.3}");
}

/// A frame of smooth colour with grain in the low bits.
fn photo_like(width: u32, height: u32) -> image::RgbImage {
    let mut state: u32 = 0x2468_ACE1;
    image::RgbImage::from_fn(width, height, |x, y| {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let grain = u8::try_from((state >> 16) % 5).unwrap_or(0);
        image::Rgb([
            u8::try_from(x * 255 / width).unwrap_or(u8::MAX).saturating_add(grain),
            u8::try_from(y * 255 / height).unwrap_or(u8::MAX).saturating_add(grain),
            u8::try_from((x + y) * 255 / (width + height)).unwrap_or(u8::MAX).saturating_add(grain),
        ])
    })
}

/// Flat interface: a light window with dark lines of text-like detail.
fn flat_interface(width: u32, height: u32) -> image::RgbImage {
    let mut frame = image::RgbImage::from_pixel(width, height, image::Rgb([245, 245, 245]));
    for line in 0..40 {
        let y0 = 40 + line * 18;
        for x in 60..(width - 60) {
            if (x / 5 + line) % 3 != 0 {
                for y in y0..(y0 + 8).min(height) {
                    frame.put_pixel(x, y, image::Rgb([32, 32, 40]));
                }
            }
        }
    }
    frame
}

/// Incompressible noise: the worst case for both formats.
fn noise(width: u32, height: u32) -> image::RgbImage {
    let mut state: u32 = 0x1357_9BDF;
    image::RgbImage::from_fn(width, height, |_, _| {
        state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        let bytes = state.to_le_bytes();
        image::Rgb([bytes[0], bytes[1], bytes[2]])
    })
}

fn encode_png(frame: &image::RgbImage) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(frame.clone())
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

fn encode_jpeg(frame: &image::RgbImage, quality: u8) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    frame.write_with_encoder(encoder).expect("jpeg");
    out.into_inner()
}

#[test]
#[ignore = "measurement: prints numbers"]
fn measure_screenshot_bytes() {
    let ceiling = api::max_request_bytes_for_model(MODEL).expect("catalog ceiling");
    let trigger = PictureBudget::for_ceiling(ceiling).trigger;
    println!("\nwhat a screenshot costs on the wire (synthetic frames; the helper's own PNG encoder is");
    println!("CoreGraphics, whose files run larger than the image crate's). Base64 is 4/3 of the file.");
    println!(
        "{:<13} {:>10} {:>10} {:>10} {:>10} {:>10} {:>12} {:>14} {:>10}",
        "frame", "pixels", "png KiB", "jpg q85", "jpg q90", "q90/png", "fits in 75%", "lighten ms", "lightened"
    );
    for (name, make) in [
        ("flat UI", flat_interface as fn(u32, u32) -> image::RgbImage),
        ("photo-like", photo_like),
        ("noise", noise),
    ] {
        for (width, height) in [(1280, 800), (960, 600)] {
            let frame = make(width, height);
            let png = encode_png(&frame);
            let q85 = encode_jpeg(&frame, 85);
            let q90 = encode_jpeg(&frame, SCREENSHOT_JPEG_QUALITY);
            let started = Instant::now();
            let lightened = lighten_screenshot(&png);
            let lighten_ms = started.elapsed().as_secs_f64() * 1_000.0;
            let wire_png = png.len() as u64 / 3 * 4;
            println!(
                "{name:<13} {:>10} {:>10.0} {:>10.0} {:>10.0} {:>9.0}% {:>12} {:>14.1} {:>10}",
                format!("{width}x{height}"),
                png.len() as f64 / 1024.0,
                q85.len() as f64 / 1024.0,
                q90.len() as f64 / 1024.0,
                q90.len() as f64 * 100.0 / png.len() as f64,
                trigger / wire_png.max(1),
                lighten_ms,
                lightened.map_or_else(|| "no".to_string(), |jpeg| format!("{} KiB", jpeg.len() / 1024)),
            );
        }
    }
}

fn rss_kib() -> u64 {
    let pid = std::process::id().to_string();
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&output.stdout).trim().parse().unwrap_or(0)
}

#[test]
#[ignore = "measurement: prints numbers"]
fn measure_memory_over_a_long_session() {
    let steps = 200;
    let full = history(steps, PICTURE_FILE_BYTES);
    let stored_mib = (steps * PICTURE_FILE_BYTES / 3 * 4) as f64 / 1_048_576.0;
    println!("\nmemory over a {steps}-step session ({stored_mib:.0} MiB of pictures stored, kept whole):");
    println!("{:>6} {:>22} {:>22}", "step", "no budget: body / rss", "budget: body / rss");
    for looks in [25, 50, 100, 150, 200] {
        let history = &full[..1 + 2 * looks];
        let idle = rss_kib();
        let unlimited = convert_messages_for(history, WireTarget::for_model("some-unlisted-local-model"));
        let unlimited_body = serde_json::to_vec(&unlimited).expect("serialize").len();
        let unlimited_rss = rss_kib().saturating_sub(idle);
        drop(unlimited);
        let idle = rss_kib();
        let budgeted = convert_messages_for(history, WireTarget::for_model(MODEL));
        let budgeted_body = serde_json::to_vec(&budgeted).expect("serialize").len();
        let budgeted_rss = rss_kib().saturating_sub(idle);
        drop(budgeted);
        println!(
            "{looks:>6} {:>10.1} MiB / +{:>5} MiB {:>10.1} MiB / +{:>5} MiB",
            unlimited_body as f64 / 1_048_576.0,
            unlimited_rss / 1024,
            budgeted_body as f64 / 1_048_576.0,
            budgeted_rss / 1024
        );
    }
}
