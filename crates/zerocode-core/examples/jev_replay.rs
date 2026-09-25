//! Replay a Jev seat's real ledger through the real judge, row by row, and
//! say where it rose and where it fell — the harness behind t-5875's claim
//! that `auto` actually promotes.
//!
//! It reads the person's own ledger and never writes to it. What it writes is
//! a COPY, under a directory of the caller's choosing, with the rise and fall
//! rows the judge decided on as it went: `zo jev summary --cwd <that copy's
//! root>` then reads the replayed file exactly as it reads a live one, so the
//! screen a person is shown and the transitions quoted here come off the same
//! bytes.
//!
//! The three functions it walks — [`promote::judgment_due`],
//! [`promote::judge_seat`] and [`promote::transition_row`] — are the whole of
//! what the two writers in the product do with a ledger
//! (`zerocode_shell::systemone::record_rows` for the seats the window owns,
//! `tools::shadow_ledger::judge_seat_ledger` for the seats zo owns). There is
//! no second judge here, which is the only reason a replay proves anything.
//!
//! ```text
//! cargo run -p zerocode-core --example jev_replay -- \
//!     placement ~/.zo/jev/worker-placement.jsonl --out /tmp/replay
//! ```

use std::path::{Path, PathBuf};

use serde_json::Value;
use zerocode_core::jev::promote;
use zerocode_core::jev::summary::{self, TRANSITION};
use zerocode_core::jev::{JevUse, jev_use};

const USAGE: &str = "usage: jev_replay <seat-id> <ledger.jsonl> [--out <dir>] [--quiet]";

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut rest = args.iter();
    let seat_id = rest.next().ok_or_else(|| USAGE.to_string())?;
    let ledger = PathBuf::from(rest.next().ok_or_else(|| USAGE.to_string())?);
    let (mut out, mut quiet) = (None, false);
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--out" => out = Some(PathBuf::from(rest.next().ok_or("--out needs a directory")?)),
            "--quiet" => quiet = true,
            other => return Err(format!("unknown argument '{other}'\n{USAGE}")),
        }
    }
    let seat = jev_use(seat_id).ok_or_else(|| format!("no seat named '{seat_id}'"))?;
    let source = read_rows(&ledger)?;
    let replayed = replay(seat, &source, quiet);
    if let Some(dir) = out {
        let copy = dir.join(seat.ledger);
        std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
        let mut text = String::new();
        for row in &replayed {
            text.push_str(&row.to_string());
            text.push('\n');
        }
        std::fs::write(&copy, text).map_err(|error| error.to_string())?;
        println!("wrote {} rows to {}", replayed.len(), copy.display());
    }
    Ok(())
}

/// The ledger's rows, oldest first, with the transitions its own judge once
/// wrote left out — a replay decides those again, and keeping the old ones
/// would have the seat start from a standing it has not re-earned.
fn read_rows(path: &Path) -> Result<Vec<Value>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|row| TRANSITION.read(row).is_none())
        .collect())
}

/// Feed the rows in one at a time, judging where the seat's own cadence says
/// to, and return the ledger as the judge would have left it.
fn replay(seat: &JevUse, source: &[Value], quiet: bool) -> Vec<Value> {
    let mut live: Vec<Value> = Vec::with_capacity(source.len());
    for row in source {
        let now_ms = summary::AT.read(row).and_then(Value::as_i64).unwrap_or(0) + 1;
        live.push(row.clone());
        let version = promote::on_the_newest_version(seat, &live);
        if !promote::judgment_due_on(seat, &version, &live) {
            continue;
        }
        let Some(judged) = promote::judge_seat_on(seat, &version, &live) else {
            continue;
        };
        let asked = live
            .iter()
            .filter(|row| summary::asked_something(row).is_some())
            .count();
        let transition = promote::transition_row(seat, now_ms, judged.verdict, &judged.window);
        if !quiet || transition.is_some() {
            println!(
                "row {asked:>5}  {:<5} {:<17} answered {}/{} bound {}‰ p95 {} agrees {} of {}",
                judged.verdict.token(),
                judged.verdict.line().map_or("", promote::Line::token),
                judged.window.answered,
                judged.window.asked(),
                judged
                    .window
                    .answered_lower_bound()
                    .map_or(0, promote::permille),
                judged.window.p95_ms.unwrap_or(0),
                judged.agreement.agreed,
                judged.agreement.compared,
            );
        }
        if let Some(transition) = transition {
            println!("        ↳ {transition}");
            live.push(transition);
        }
    }
    let stand = promote::standing(seat, &live);
    let rises = count_of(&live, promote::ROSE);
    let falls = count_of(&live, promote::FELL);
    println!(
        "{}: {rises} rise, {falls} fall, standing {} at the end ({} window, {} forgiven, {} comparisons wanted)",
        seat.id,
        stand.token(),
        promote::window_wanted_for(seat).unwrap_or(0),
        seat.window_forgives.unwrap_or(0),
        seat.agreement_rows_wanted.unwrap_or(0),
    );
    live
}

fn count_of(rows: &[Value], word: &str) -> usize {
    rows.iter()
        .filter(|row| TRANSITION.read(row).and_then(Value::as_str) == Some(word))
        .count()
}
