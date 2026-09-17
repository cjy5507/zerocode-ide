use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use std::hint::black_box;
use zerocode_pty::{MIN_SCROLLBACK_LINES, Terminal};

const ROWS: usize = 48;
const COLS: usize = 160;

fn ascii_build_log() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(256 * 1024);
    for line in 0..2_048 {
        bytes.extend_from_slice(b"\x1b[38;5;245m");
        bytes.extend_from_slice(format!("[{line:04}] compiling zerocode-pty::hotpath ").as_bytes());
        bytes.extend_from_slice(b"finished checking parser and grid allocations");
        bytes.extend_from_slice(b"\x1b[0m\r\n");
    }
    bytes
}

fn ansi_screen_updates() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(128 * 1024);
    for row in 0..1_024 {
        bytes.extend_from_slice(format!("\x1b[{};1H", row % 48 + 1).as_bytes());
        bytes.extend_from_slice(b"\x1b[2K");
        bytes.extend_from_slice(if row % 2 == 0 {
            b"\x1b[1;32mREADY\x1b[0m"
        } else {
            b"\x1b[1;31mWARN\x1b[0m"
        });
        bytes.extend_from_slice(b" terminal frame update\x1b[K");
    }
    bytes
}

fn unicode_graphemes() -> Vec<u8> {
    let line =
        "상태: e\u{301} ✅ 👩\u{200d}👩\u{200d}👧\u{200d}👦 · 파일 경로가 갱신되었습니다\r\n";
    line.repeat(1_024).into_bytes()
}

fn full_screen_clears() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64 * 1024);
    for frame in 0..1_024 {
        bytes.extend_from_slice(b"\x1b[2J\x1b[H");
        bytes.extend_from_slice(format!("frame {frame:04}: terminal surface reset").as_bytes());
    }
    bytes
}

fn wide_overwrites() -> Vec<u8> {
    let mut bytes = Vec::with_capacity(128 * 1024);
    for _ in 0..4_096 {
        bytes.extend_from_slice("👩\u{200d}👩\u{200d}👧\u{200d}👦".as_bytes());
        bytes.extend_from_slice(b"\rX\r");
    }
    bytes
}

fn plain_scrollback(lines: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lines * 80);
    for line in 0..lines {
        bytes.extend_from_slice(
            format!("scrollback line {line:04}: parser output retained by the grid\r\n").as_bytes(),
        );
    }
    bytes
}

fn feed(c: &mut Criterion, name: &str, input: Vec<u8>) {
    let mut group = c.benchmark_group(name);
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_function("feed", |b| {
        b.iter_batched(
            || Terminal::new(ROWS, COLS),
            |mut terminal| {
                terminal.feed(black_box(&input));
                black_box(terminal.grid().cursor());
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn feed_saturated_scrollback(c: &mut Criterion) {
    let prefill = plain_scrollback(MIN_SCROLLBACK_LINES + ROWS);
    let input = plain_scrollback(2_048);
    let mut group = c.benchmark_group("pty_hotpath_saturated_scrollback");
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_function("feed", |b| {
        b.iter_batched(
            || {
                let mut terminal = Terminal::new(ROWS, COLS);
                terminal.grid_mut().set_scrollback_cap(MIN_SCROLLBACK_LINES);
                terminal.feed(&prefill);
                terminal
            },
            |mut terminal| {
                terminal.feed(black_box(&input));
                black_box(terminal.grid().scrollback_len());
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn feed_and_take_frame(c: &mut Criterion, input: Vec<u8>) {
    let mut group = c.benchmark_group("pty_hotpath_ansi_frame");
    group.throughput(Throughput::Bytes(input.len() as u64));
    group.bench_function("feed_and_take_delta", |b| {
        b.iter_batched(
            || Terminal::new(ROWS, COLS),
            |mut terminal| {
                terminal.feed(black_box(&input));
                black_box(terminal.grid_mut().take_delta());
            },
            BatchSize::SmallInput,
        );
    });
    group.bench_function("background_feed_and_take_news", |b| {
        b.iter_batched(
            || {
                let mut terminal = Terminal::new(ROWS, COLS);
                terminal.grid_mut().take_snapshot();
                terminal
            },
            |mut terminal| {
                terminal.feed(black_box(&input));
                black_box(terminal.grid_mut().take_news());
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

fn pty_hotpath(c: &mut Criterion) {
    feed(c, "pty_hotpath_ascii_build_log", ascii_build_log());
    feed(c, "pty_hotpath_ansi_screen_updates", ansi_screen_updates());
    feed(c, "pty_hotpath_unicode_graphemes", unicode_graphemes());
    feed(c, "pty_hotpath_full_screen_clears", full_screen_clears());
    feed(c, "pty_hotpath_wide_overwrites", wide_overwrites());
    feed_saturated_scrollback(c);
    feed_and_take_frame(c, ansi_screen_updates());
}

criterion_group!(benches, pty_hotpath);
criterion_main!(benches);
