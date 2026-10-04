//! What masking and planning a worker's hand-in cost, measured (t-32798).
//!
//! A kept report passes the mask once ([`zerocode_core::private_data`]) on a
//! thread of its own at the lowest priority the system has, so the person never
//! waits for it — but a slow machine still pays the time, and a report is as big
//! as its writer made it. This times the mask on a report of 20 KB and of 5 MB
//! (the sizes a report is, and a large one), in the two shapes that matter: prose
//! with the paths and job names a report is made of and no private value in it,
//! and a dense one that carries one of each kind of value every forty lines. Korean
//! and English are mixed, because a regex that is fast on ASCII can be slow on
//! text that is not.
//!
//! `cargo bench -p zerocode-core --bench hand_in`, then the same binary under
//! `taskpolicy -b` for the efficiency cores.

use std::hint::black_box;
use std::time::Instant;

use zerocode_core::hand_in::{self, Candidate, Kind};
use zerocode_core::private_data;

// Fictional values that belong to nobody; each carries its own waiver so this
// file does not trip the gate whose table it measures.
// pii-scan: allow home-path — a fictional account the mask is measured with
const HOME: &str = "/Users/mallory";
// pii-scan: allow private-ip — a fictional subnet the mask is measured with
const OFFICE_IP: &str = "10.99.44.7";
// pii-scan: allow email — a fictional mailbox the mask is measured with
const PERSON: &str = "chief@northwind-holdings.co.kr";
// pii-scan: allow credential — a fictional key id the mask is measured with
const KEY_ID: &str = "AKIAQ7RVBNMLKJHGFDSZ";

/// A report of about `bytes`, mixing Korean and English, with `private_every`
/// lines between the lines that carry one of each private value (0 = none).
fn report(bytes: usize, private_every: usize) -> String {
    let mut text = String::with_capacity(bytes + 512);
    let mut line = 0usize;
    while text.len() < bytes {
        line += 1;
        match line % 4 {
            0 => text.push_str("- 빌드 줄 일감 `1791083855-w-35393-t32796-red-run` rc=0 (581s) — 헤드 8d042634d, disk-guard 통과\n"),
            1 => text.push_str("crates/zerocode-core/src/orchestration/tests.rs:6430 — the ask-wait replay seed counts the gate's receipt\n"),
            2 => text.push_str("측정: 기본 사양 429 → 578 ms (+149), 효율 코어 4,258 → 5,033 ms; RSS 평탄 7,056 → 7,104 KiB\n"),
            _ => text.push_str("Reported a clean, landed checkout holding only its build; nothing else was kept in it.\n"),
        }
        if private_every > 0 && line.is_multiple_of(private_every) {
            text.push_str(&format!(
                "worktree {HOME}/work/t-1 on {OFFICE_IP} mailed {PERSON} with {KEY_ID} and API_TOKEN=hunter2\n"
            ));
        }
    }
    text
}

/// The p50 and p95 of `rounds` runs of `work`, in milliseconds.
fn times(rounds: usize, mut work: impl FnMut()) -> (f64, f64) {
    let mut spent: Vec<f64> = (0..rounds)
        .map(|_| {
            let began = Instant::now();
            work();
            began.elapsed().as_secs_f64() * 1_000.0
        })
        .collect();
    spent.sort_by(f64::total_cmp);
    (
        spent[spent.len() / 2],
        spent[(spent.len() * 95 / 100).min(spent.len() - 1)],
    )
}

fn main() {
    println!("HAND_IN_BENCH start");
    for (label, bytes, rounds) in [("20 KB", 20 * 1024, 200), ("5 MB", 5 * 1024 * 1024, 9)] {
        for (shape, every) in [("prose", 0), ("dense", 40)] {
            let text = report(bytes, every);
            let (p50, p95) = times(rounds, || {
                black_box(private_data::mask(black_box(&text)));
            });
            let masked = private_data::mask(&text);
            let (clean50, _) = times(rounds, || {
                black_box(private_data::is_clean(black_box(&masked.text)));
            });
            println!(
                "HAND_IN_MASK {label} {shape}: bytes={} mask_p50_ms={p50:.3} mask_p95_ms={p95:.3} is_clean_p50_ms={clean50:.3} values={}",
                text.len(),
                masked.found.total()
            );
        }
    }
    // Planning and parsing: what a payload and a folder of files cost before anything is read.
    let names: Vec<String> = (0..hand_in::FILES_MAX)
        .map(|at| format!("frame-{at}.png"))
        .collect();
    let candidates: Vec<Candidate<'_>> = names
        .iter()
        .map(|name| Candidate {
            name,
            bytes: 400_000,
            kind: Kind::Image,
        })
        .collect();
    let (plan50, _) = times(2_000, || {
        black_box(hand_in::plan(black_box(&candidates)));
    });
    let payload = format!(
        r#"{{"reportPath":"/tmp/t-1/report.md","evidencePaths":[{}],"lifetime":"ephemeral"}}"#,
        (0..hand_in::NAMED_MAX)
            .map(|at| format!(r#""/tmp/t-1/frame-{at}.png""#))
            .collect::<Vec<_>>()
            .join(",")
    );
    let (named50, _) = times(2_000, || {
        black_box(hand_in::named(black_box(&payload), black_box("done")));
    });
    println!(
        "HAND_IN_PLAN files={} plan_p50_ms={plan50:.4} payload_bytes={} named_p50_ms={named50:.4}",
        candidates.len(),
        payload.len()
    );
}
