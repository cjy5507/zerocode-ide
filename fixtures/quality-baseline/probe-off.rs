//! Optimized measurement of the same lazy probe entry used by Ui::draw.
use std::hint::black_box;
use std::time::Instant;
#[path = "../../zo-ide/crates/zo-ide/src/tui/paint_probe.rs"]
#[allow(dead_code)]
mod paint_probe;

#[inline(never)]
fn control(iterations: u64) -> u64 {
    let mut sum = 0;
    for _ in 0..iterations { sum += black_box(1); }
    sum
}

#[inline(never)]
fn disabled(iterations: u64, enabled: bool) -> u64 {
    let mut sum = 0;
    for _ in 0..iterations {
        let sample = paint_probe::start(black_box(enabled), || panic!("queue sampled"), || panic!("clock read"));
        paint_probe::frame(None, sample);
        sum += black_box(1);
    }
    sum
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let iterations: u64 = args[1].parse().unwrap();
    let rounds: u32 = args[2].parse().unwrap();
    for round in 0..rounds {
        // Alternate order so warmup does not systematically favour one side.
        let measure = |f: fn(u64) -> u64| { let at = Instant::now(); black_box(f(iterations)); at.elapsed().as_nanos() };
        let probed: fn(u64) -> u64 = |n| disabled(n, black_box(false));
        let (control_ns, disabled_ns) = if round % 2 == 0 {
            (measure(control), measure(probed))
        } else {
            let cost = measure(probed); (measure(control), cost)
        };
        println!("{{\"iterations\":{iterations},\"control_ns\":{control_ns},\"disabled_ns\":{disabled_ns}}}");
    }
}
