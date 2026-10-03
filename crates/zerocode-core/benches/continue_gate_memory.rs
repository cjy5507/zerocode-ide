//! What the gate's memory does over a long run, measured (t-26583).
//!
//! The gate holds three things that a window left open for a month keeps feeding:
//! a worker's record of its calls and costs (`StepBook`), the day's spend (`DaySpend`) and
//! the launch ledger (`LaunchLedger`). Each is meant to be bounded — a window of
//! marks, a ring of costs, a bucket a minute, a few thousand stamps — so that the
//! thousandth hour holds what the first did. This feeds each of them far more than
//! a month would and prints the process's resident size after every step: the
//! numbers should stop rising after the first, whatever is fed.
//!
//! `cargo bench -p zerocode-core --bench continue_gate_memory`

use std::process::Command;

use zerocode_core::continue_gate::StepBook;
use zerocode_core::continue_gate::day::DaySpend;
use zerocode_core::hook::{Activity, Phase, Tool};
use zerocode_core::launch_budget::{Ask, LaunchLedger, Limits, Outcome};

/// This process's resident set, in kibibytes, as `ps` reports it.
fn resident_kib() -> u64 {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .expect("ps prints a number")
}

fn started() -> Activity {
    Activity {
        verb: Tool::Bash,
        target: Some("cargo test -p zerocode-core".into()),
        phase: Phase::Started,
        reads: Vec::new(),
        writes: Vec::new(),
        vcs: Vec::new(),
        cwd: None,
    }
}

fn main() {
    let baseline = resident_kib();
    println!("process resident at the start: {baseline} KiB");

    // One worker's record, fed the calls and the costs of weeks of work.
    let activity = started();
    let mut book = StepBook::default();
    let mut fed = 0_u64;
    println!("\nStepBook (a window of 24 marks, a ring of 64 costs):");
    for step in 1..=4_u64 {
        let batch = 250_000 * step;
        while fed < batch {
            let payload = format!(
                r#"{{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{{"command":"cargo test case_{}"}}}}"#,
                fed % 997
            );
            book.note_activity(&activity, &payload);
            book.note_cost(Some(0.1 + (fed % 7) as f64 * 0.01));
            fed += 1;
        }
        let resident = resident_kib();
        println!(
            "  after {fed:>9} calls and costs: {resident:>7} KiB resident ({:+} over the start), {} steps counted",
            i64::try_from(resident).expect("fits") - i64::try_from(baseline).expect("fits"),
            book.metrics().steps
        );
    }

    // The day's spend, fed thirty days of a call a second.
    let mut day = DaySpend::default();
    println!("\nDaySpend (a bucket a minute, a rolling day):");
    let mut at_ms = 1_800_000_000_000_i64;
    for week in 1..=4_i64 {
        for _ in 0..(7 * 24 * 3_600) {
            day.add(at_ms, 0.01);
            at_ms += 1_000;
        }
        let bytes = serde_json::to_string(&day).expect("serializes").len();
        println!(
            "  after {week} week(s) of a call a second: {:>7} KiB resident, the day written down is {bytes} bytes, spent in the last day ${:.2}",
            resident_kib(),
            day.spent(at_ms)
        );
    }

    // The launch ledger, fed far more launches than any hour holds.
    let mut ledger = LaunchLedger::default();
    let no_ceiling = Limits {
        concurrent: None,
        per_hour: None,
        per_day: None,
    };
    println!("\nLaunchLedger (a day of stamps at most 20,000, 256 jobs remembered):");
    let mut now_ms = 1_800_000_000_000_i64;
    let mut launched = 0_u64;
    for step in 1..=4_u64 {
        let batch = 250_000 * step;
        while launched < batch {
            let job = format!("job-{}", launched % 5_000);
            let ask = Ask {
                provider: "claude",
                job: Some(&job),
                fresh_ms: Some(60_000),
            };
            let permit = ledger
                .reserve(&ask, &no_ceiling, now_ms)
                .expect("no ceiling, no wall");
            ledger.finish(permit, Outcome::Done, now_ms);
            now_ms += 100;
            launched += 1;
        }
        let bytes = serde_json::to_string(&ledger).expect("serializes").len();
        println!(
            "  after {launched:>9} launches: {:>7} KiB resident, the ledger written down is {bytes} bytes, {} stamps in the last day",
            resident_kib(),
            ledger.counters(now_ms).last_day
        );
    }
}
