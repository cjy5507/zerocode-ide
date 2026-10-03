//! A busy pane's output path, measured (t-26594).
//!
//! The answer door reads a pane's screen only when a pane waits or an answer is
//! about to be typed — never per output chunk — so a busy pane's throughput is
//! the parser's and nothing else's. This is the number that claim is checked
//! against: how fast one terminal parses a stream of what an agent's streaming
//! answer looks like to it. Run before and after a change that says it does
//! not touch this path, normally and on the efficiency cores:
//!
//! ```text
//! cargo test -p zerocode-pty --test busy_output -- --ignored --nocapture --exact a_busy_panes_output_is_parsed_at_this_rate
//! taskpolicy -b cargo test -p zerocode-pty --test busy_output -- --ignored --nocapture --exact a_busy_panes_output_is_parsed_at_this_rate_over_a_few_runs
//! ```

use std::time::Instant;

use zerocode_pty::Terminal;

/// One read's worth: what a pty hands the pump at a time at most.
const CHUNK_BYTES: usize = 64 * 1024;

/// 48 MiB through one terminal per run — long enough that the parser, not the
/// clock's resolution, is what is measured.
const CHUNKS: usize = 768;

/// Runs per measurement; the median is the number, the spread says how much
/// to trust it.
const RUNS: usize = 7;

/// The shorter measurement, for a job with a time cap: on the efficiency cores
/// one run takes over half a minute, and seven of them are minutes.
const SHORT_RUNS: usize = 3;

/// A burst of rows shaped like an agent's streaming output: a colour, a bullet,
/// dim body text, a reset, a row each.
fn burst() -> Vec<u8> {
    let mut chunk = Vec::with_capacity(CHUNK_BYTES);
    let mut row = 0_u32;
    while chunk.len() + 160 < CHUNK_BYTES {
        chunk.extend_from_slice(
            format!(
                "\x1b[38;5;{}m● row {row} of the answer \x1b[0m\x1b[2mthe quick brown fox jumps over the lazy dog\x1b[0m\r\n",
                30 + row % 200
            )
            .as_bytes(),
        );
        row += 1;
    }
    chunk
}

fn measure(runs: usize) {
    let chunk = burst();
    let mut rates = Vec::new();
    for _ in 0..runs {
        let mut terminal = Terminal::new(40, 120);
        let began = Instant::now();
        for _ in 0..CHUNKS {
            terminal.feed(&chunk);
        }
        let seconds = began.elapsed().as_secs_f64();
        rates.push((chunk.len() * CHUNKS) as f64 / (1024.0 * 1024.0) / seconds);
    }
    rates.sort_by(f64::total_cmp);
    println!(
        "MEASURE feed MiB/s runs={rates:.1?} median={:.1} min={:.1} max={:.1}",
        rates[runs / 2],
        rates[0],
        rates[runs - 1]
    );
}

#[test]
#[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
fn a_busy_panes_output_is_parsed_at_this_rate() {
    measure(RUNS);
}

#[test]
#[ignore = "a measurement, run on purpose: -- --ignored --nocapture"]
fn a_busy_panes_output_is_parsed_at_this_rate_over_a_few_runs() {
    measure(SHORT_RUNS);
}
