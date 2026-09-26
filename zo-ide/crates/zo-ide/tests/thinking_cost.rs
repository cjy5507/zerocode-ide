//! Measurements, not contracts — `#[ignore]`d, they print what one reasoning
//! delta and one status frame cost with the thinking word on the row
//! (t-5872). Run with `cargo test -p zo-ide --test thinking_cost -- --ignored
//! --nocapture`.

use std::time::{Duration, Instant};

use zo_ide::tui::composer::Composer;
use zo_ide::tui::thinking::{bound_scan, live_heading, LIVE_HEADING_MAX_COLUMNS, SCAN_KEEP_BYTES};
use zo_ide::tui::view::{self, Frame, Status, StatusDetailsCapitalization};

fn frame<'a>(composer: &'a Composer, status: Option<&'a Status>) -> Frame<'a> {
    Frame {
        composer,
        effort_tier: None,
        effort_effect: None,
        now: Instant::now(),
        status,
        pending_input: None,
        active: None,
        tail_owns_slot: false,
        dialog: None,
        question: None,
        picker: None,
        sessions: None,
        pager: None,
        popup: None,
        mention: None,
        shortcuts: None,
        warnings: None,
        hints: zo_ide::tui::footer_hints::FooterHints::default(),
        model: "claude-fable-5-1",
        effort: "smart",
        model_note: None,
        cwd: "/tmp/x",
        context_left: Some(80),
        context_used_tokens: Some(40_000),
        plan_mode: false,
        goal_status: None,
        loop_status: None,
        dream: None,
        width: 120,
        max_rows: 39,
    }
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

/// One delta's cost on the heading scan, at the buffer's bound, and one
/// frame's cost with the word, the inline route fact and two helper rows on
/// the status row against the plain `Working` frame.
#[test]
#[ignore = "measurement, not a contract"]
fn a_delta_and_a_frame_with_the_thinking_word() {
    // A buffer at its bound: what every delta of a long block scans.
    let sentence = "먼저 실패하는 시험을 읽고, 그다음 렌더러가 무엇을 그리는지 확인해야 한다. ";
    let mut buffer = String::new();
    while buffer.len() < SCAN_KEEP_BYTES {
        buffer.push_str(sentence);
    }
    let deltas = 2_000;
    let mut per_delta = Vec::with_capacity(deltas);
    for _ in 0..deltas {
        let started = Instant::now();
        buffer.push_str("Let me");
        bound_scan(&mut buffer, SCAN_KEEP_BYTES);
        let word = live_heading(&buffer, LIVE_HEADING_MAX_COLUMNS);
        per_delta.push(started.elapsed().as_secs_f64() * 1e6);
        assert!(word.is_some());
    }
    println!(
        "live_heading per delta at {} bytes: median {:.1} µs · max {:.1} µs",
        SCAN_KEEP_BYTES,
        median(per_delta.clone()),
        per_delta.iter().copied().fold(0.0, f64::max)
    );

    let composer = Composer::new();
    let plain = Status::working(Duration::from_secs(12));
    let mut worded = Status::working(Duration::from_secs(12));
    worded.set_header(Some("Reading the failing test before the renderer"));
    worded.set_inline_message(Some("jev · claude-opus-5 max · stuck_check_red".to_string()));
    worded.set_details(
        [
            "agents 2 · running 2 · done 0".to_string(),
            "scout · claude-opus-5 · 3 tool uses · 9s · Read · src/tui/view.rs · \"reading the view\"".to_string(),
            "reviewer · 1 tool use · 4s · Bash · cargo test -p zo-ide".to_string(),
        ],
        StatusDetailsCapitalization::Preserve,
    );
    let frames = 3_000;
    for (name, status) in [("plain Working", &plain), ("word + fact + 2 helper rows", &worded)] {
        let mut per_frame = Vec::with_capacity(frames);
        for _ in 0..frames {
            let started = Instant::now();
            let (rows, _) = view::build(&frame(&composer, Some(status)));
            per_frame.push(started.elapsed().as_secs_f64() * 1e6);
            assert!(!rows.is_empty());
        }
        println!(
            "view::build {name}: median {:.1} µs · p95 {:.1} µs",
            median(per_frame.clone()),
            {
                let mut sorted = per_frame.clone();
                sorted.sort_by(f64::total_cmp);
                sorted[sorted.len() * 95 / 100]
            }
        );
    }
}
