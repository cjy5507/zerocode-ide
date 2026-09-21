//! `zo jev summary` — every Jev seat's ledger, counted
//! (docs/design/jev-settings-20260917.md §5).
//!
//! The window asks this rather than counting for itself: the window↔zo line
//! is an exec boundary, and a screen that counted the rows would be a second
//! reader of the same files, free to disagree with the judge that promotes a
//! seat on them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tools::jev_summary::{self, SeatReport};

pub const USAGE: &str = "\
zo jev summary [--cwd <dir>] [--computer-use <sessions-dir>] [--recent <n>] [--json]

  summary: count every Jev seat's ledger — today and the last seven days.
  Per seat: its mode (off/shadow/on/auto), rows, how many answered and the
  95% lower bound on that share, the p50 and p95 of the calls that went over
  the wire (a memo hit answered without asking, so it is not one), the
  refusal and failure tokens with their counts, the lines the door withheld,
  what the billed tokens cost, and — for a seat whose `auto` may rise — the
  share it must clear and how many rows stand before the next judgment.
  A seat no ledger has been written for says so; it is not a seat that
  answered nothing. --computer-use names the window's Computer Use sessions
  folder, where the screen seats append beside each walk's evidence
  (<dir>/<session>/<ledger>); every session's rows are counted. --recent
  lists each seat's last <n> requests under its numbers — what was asked,
  what it answered, whether that was acted on, and what the seat's own
  writer later said of it — read from the same rows in the same pass.
  Every seat also carries its last seven local days, one count per day.
";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Request {
    cwd: Option<PathBuf>,
    sessions: Option<PathBuf>,
    /// How many of each seat's last requests to list; none by default.
    recent: usize,
    json: bool,
}

fn parse(args: &[String]) -> Result<Request, String> {
    match args.first().map(String::as_str) {
        Some("summary") => {}
        Some("-h" | "--help") | None => return Err(USAGE.to_string()),
        Some(other) => return Err(format!("unknown verb '{other}'\n\n{USAGE}")),
    }
    let mut request = Request { cwd: None, sessions: None, recent: 0, json: false };
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => request.json = true,
            "--cwd" => {
                let dir = rest.next().ok_or_else(|| "--cwd needs a directory".to_string())?;
                request.cwd = Some(PathBuf::from(dir));
            }
            "--computer-use" => {
                let dir = rest.next().ok_or_else(|| "--computer-use needs a directory".to_string())?;
                request.sessions = Some(PathBuf::from(dir));
            }
            "--recent" => {
                let count = rest.next().ok_or_else(|| "--recent needs a count".to_string())?;
                request.recent =
                    count.parse().map_err(|_| format!("--recent needs a count, not '{count}'"))?;
            }
            other => return Err(format!("unknown argument '{other}'\n\n{USAGE}")),
        }
    }
    Ok(request)
}

/// What the command printed.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub text: String,
}

/// # Errors
///
/// The usage, or what was wrong with the arguments.
pub fn run(args: &[String], cwd: &Path, now_ms: i64, offset_s: i64) -> Result<Report, String> {
    let request = parse(args)?;
    let cwd = request.cwd.clone().unwrap_or_else(|| cwd.to_path_buf());
    let settings = tools::merged_settings_root(&cwd);
    let roots = jev_summary::ledger_roots(&cwd);
    let seats = jev_summary::report_with_recent(
        &roots,
        request.sessions.as_deref(),
        settings.as_ref(),
        now_ms,
        offset_s,
        request.recent,
    );
    Ok(Report {
        text: if request.json {
            render_json(&seats).to_string()
        } else {
            render_text(&seats)
        },
    })
}

fn tally_json(tally: &jev_summary::SeatTally) -> Value {
    json!({
        "rows": tally.rows,
        "answered": tally.answered,
        "refused": tally.refused,
        "applied": tally.applied,
        // The door's refusals by token, beside `failures` which holds every
        // token: a screen says "no key 3 · not consented 60" off this list
        // without a table of the door's words of its own.
        "refusals": tally
            .refusals()
            .map(|(token, count)| json!({ "token": token, "rows": count }))
            .collect::<Vec<Value>>(),
        "answeredShare": tally.answered_share(),
        "answeredLowerBound": tally.answered_lower_bound(),
        "called": tally.called,
        "requests": tally.requests,
        "redactedLines": tally.redacted_lines,
        "inputTokens": tally.input_tokens,
        "p50Ms": tally.p50_ms,
        "p95Ms": tally.p95_ms,
        "failures": tally
            .failures
            .iter()
            .map(|(token, count)| json!({ "token": token, "rows": count }))
            .collect::<Vec<Value>>(),
    })
}

fn agreement_json(agreement: &jev_summary::SeatAgreement) -> Value {
    json!({
        "compared": agreement.compared,
        "agreed": agreement.agreed,
        "lowerBound": agreement.lower_bound(),
    })
}

fn decision_json(decision: &jev_summary::SeatDecision) -> Value {
    json!({
        "at": decision.at,
        "outcome": decision.outcome,
        "elapsedMs": decision.elapsed_ms,
        "cached": decision.cached,
        "asked": decision.asked,
        "answered": decision.answered,
        "confidence": decision.confidence,
        "applied": decision.applied,
        "agreed": decision.agreed,
        "followed": decision.followed,
    })
}

fn render_json(seats: &[SeatReport]) -> Value {
    json!({
        "windowDays": jev_summary::WINDOW_DAYS,
        "judgedEveryRows": jev_summary::JUDGED_EVERY_ROWS,
        "seats": seats
            .iter()
            .map(|seat| json!({
                "id": seat.id,
                "setting": seat.setting,
                "mode": seat.mode.key(),
                "ledger": seat.ledger,
                "found": seat.found.as_ref().map(|path| path.display().to_string()).map_or(Value::Null, Value::from),
                "today": tally_json(&seat.today),
                "week": tally_json(&seat.week),
                "costUsd": seat.cost_usd,
                "riseFloorPermille": seat.rise_floor_permille,
                "clearsRiseFloor": seat.clears_rise_floor,
                "rowsToNextJudgment": seat.rows_to_next_judgment(),
                // The window the verdict was read on, and the agreement over
                // it — the numbers a seat is promoted on, which are not the
                // week's.
                "judged": seat.judged.as_ref().map(|judged| {
                    let mut agreement = agreement_json(&judged.agreement);
                    // Control rows joined to the window for the comparison —
                    // the probe run once more beside a judgment an active
                    // turn acted on. They are in no other number here.
                    agreement["controlRows"] = Value::from(judged.control_rows);
                    json!({
                        "window": tally_json(&judged.window),
                        "windowWanted": judged.window_wanted,
                        "agreement": agreement,
                    })
                }),
                // Today and the six days before it, oldest first — the trend
                // beside the week, counted by the week's own counter.
                "days": seat
                    .days
                    .iter()
                    .map(|day| json!({
                        "startMs": day.start_ms,
                        "tally": tally_json(&day.tally),
                        "agreement": agreement_json(&day.agreement),
                    }))
                    .collect::<Vec<Value>>(),
                // The last requests, newest first, when `--recent` asked.
                "recent": seat.recent.iter().map(decision_json).collect::<Vec<Value>>(),
                // Every `agreed` mark of the week, counted whether or not the
                // seat rises — the recall seat's only agreement number, and
                // the routing seat's turn labels beside its probe axes.
                "agreementWeek": {
                    "compared": seat.agreement_week.compared,
                    "agreed": seat.agreement_week.agreed,
                    "lowerBound": seat.agreement_week.lower_bound(),
                },
                "stand": seat.stand.token(),
                "applies": seat.applies,
                "verdict": seat.verdict().map(|verdict| json!({
                    "verdict": verdict.token(),
                    "line": verdict.line().map(tools::jev_summary::line_token),
                })),
            }))
            .collect::<Vec<Value>>(),
    })
}

fn render_text(seats: &[SeatReport]) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<12} {:<7} {:<9} {:>7} {:>7} {:>8} {:>7} {:>7}  notes",
        "seat", "mode", "stands", "today", "7d", "answered", "p50", "p95"
    );
    for seat in seats {
        let share = seat
            .week
            .answered_share()
            .map_or_else(|| "—".to_string(), |share| format!("{:.1}%", share * 100.0));
        let mut notes = String::new();
        if seat.found.is_none() {
            notes.push_str("never asked");
        }
        if let (Some(floor), Some(judged)) = (seat.rise_floor_permille, seat.judged.as_ref()) {
            let _ = write!(
                notes,
                "{}rise {} needs {:.1}% over {}/{} rows",
                if notes.is_empty() { "" } else { " · " },
                judged
                    .window
                    .answered_lower_bound()
                    .map_or_else(|| "—".to_string(), |bound| format!("{:.1}%", bound * 100.0)),
                f64::from(floor) / 10.0,
                judged.window.asked(),
                judged.window_wanted,
            );
            let _ = write!(
                notes,
                " · agrees {} of {}",
                judged.agreement.agreed, judged.agreement.compared
            );
            // The rows that comparison borrowed from the control sample, when
            // it borrowed any: an acting seat's own rows compare nothing.
            if judged.control_rows > 0 {
                let _ = write!(
                    notes,
                    " ({} control row{})",
                    judged.control_rows,
                    if judged.control_rows == 1 { "" } else { "s" }
                );
            }
        }
        if let Some(verdict) = seat.verdict() {
            let _ = write!(
                notes,
                "{}{}{}",
                if notes.is_empty() { "" } else { " · " },
                verdict.token(),
                verdict.line().map_or_else(String::new, |line| format!(" ({})", line.token()))
            );
        }
        if let Some(owed) = seat.rows_to_next_judgment() {
            let _ = write!(notes, " · {owed} rows to judgment");
        }
        let _ = writeln!(
            out,
            "{:<12} {:<7} {:<9} {:>7} {:>7} {:>8} {:>7} {:>7}  {}",
            seat.id,
            seat.mode.key(),
            if seat.applies { "applying" } else { seat.stand.token() },
            seat.today.rows,
            seat.week.rows,
            share,
            seat.week.p50_ms.map_or_else(|| "—".to_string(), |ms| format!("{ms}")),
            seat.week.p95_ms.map_or_else(|| "—".to_string(), |ms| format!("{ms}")),
            notes
        );
    }
    out
}

#[cfg(test)]
mod tests;
