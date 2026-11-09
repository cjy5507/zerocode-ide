//! How fast a flooding pane drains at the window's own cadence.
//!
//! One child `cat`s a file into a [`PtyLane`]; the lane is pumped the way the
//! window's pump loop does it (`crates/zerocode-shell/src/main.rs`): a round
//! every 16 ms, a full byte budget sent straight back for more until the
//! round's 8 ms drain is spent, then the rest of the frame slept out. Prints
//! the time to the end of the output, its rate, and the rounds it took.
//!
//! `cargo run --release -p zerocode-pty --example pty_flood -- <file> [runs]`

use std::time::{Duration, Instant};

use zerocode_pty::{PTY_OUTPUT_BYTE_BUDGET, PtyLane};

const PUMP_INTERVAL: Duration = Duration::from_millis(16);
const DRAIN_BUDGET: Duration = Duration::from_millis(8);

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: pty_flood <file> [runs]");
    let runs: usize = args
        .next()
        .map_or(5, |runs| runs.parse().expect("runs is a number"));
    for _ in 0..runs {
        let mut lane = PtyLane::spawn("cat", std::slice::from_ref(&path), None, &[], 48, 160)
            .expect("cat starts");
        let began = Instant::now();
        let (mut bytes, mut rounds) = (0_usize, 0_usize);
        'flood: loop {
            let round = Instant::now();
            rounds += 1;
            loop {
                let pumped = lane.pump();
                bytes += pumped.bytes;
                if pumped.ended {
                    break 'flood;
                }
                if pumped.bytes < PTY_OUTPUT_BYTE_BUDGET || round.elapsed() >= DRAIN_BUDGET {
                    break;
                }
            }
            if let Some(rest) = PUMP_INTERVAL.checked_sub(round.elapsed()) {
                std::thread::sleep(rest);
            }
        }
        let took = began.elapsed();
        println!(
            "{bytes} bytes in {:.3} s = {:.1} MB/s over {rounds} rounds",
            took.as_secs_f64(),
            bytes as f64 / took.as_secs_f64() / 1e6,
        );
    }
}
