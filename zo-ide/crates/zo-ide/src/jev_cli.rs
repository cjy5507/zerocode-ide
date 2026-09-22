//! `zo jev summary` — every Jev seat's ledger, counted
//! (docs/design/jev-settings-20260917.md §5) — and `zo jev ask|choose|score`,
//! an agent's own question put to the agent tool seat from a shell (t-6040).
//!
//! The window asks `summary` rather than counting for itself: the window↔zo
//! line is an exec boundary, and a screen that counted the rows would be a
//! second reader of the same files, free to disagree with the judge that
//! promotes a seat on them.
//!
//! The three verbs are the same seat zo's `Jev` tool opens from inside a
//! turn (`tools::jev_decide`): one constructor checks the question, one
//! function decides it, one struct is the verdict. What this file adds is a
//! shell's contract — items on the command line or on standard input, `--json`
//! for a program, and an exit code that says the answer (an `ask` exits 0 for
//! yes and 1 for no) so a worker's script can branch on it without parsing.

use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tools::jev_summary::{self, SeatReport};
use tools::{JevAnswer, JevCaller, JevQuestion, JevShape, JevVerdict};

use crate::autonomy::limits::HEADLESS_LOOP_EXIT_DONE;

pub const USAGE: &str = "\
zo jev summary [--cwd <dir>] [--computer-use <sessions-dir>] [--recent <n>] [--json]
zo jev ask <question> [--context <text>] [--cwd <dir>] [--json]
zo jev choose <question> (--option <text>... | --stdin) [--context <text>] [--cwd <dir>] [--json]
zo jev score <question> --level <text>... (--item <text>... | --stdin) [--cwd <dir>] [--json]

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

  ask / choose / score: put your own question to Jev, TypeSafe's typed
  judge, through the door every seat passes (smart.agentTool in the ZeroCode
  settings: off sends nothing, shadow records and withholds, on answers;
  consent is per workspace there too). `ask` answers yes or no. `choose`
  picks one of your options. `score` grades every item on your own levels,
  written low to high (2-10), and prints one line per item: score,
  0-to-1 reading, confidence, item. --context is the facts to read first.
  --stdin reads the options (choose) or items (score) from standard input,
  one per line or as one JSON array of strings. --json prints the verdict
  as the tool would return it.
  Exit codes: ask 0 = yes, 1 = no, 2 = nothing answered (off, refused,
  failed, or a bad command line); choose and score 0 = answered, 2 = not.
";

/// What `zo jev` refused, and the exit code the verb refuses with:
/// `summary`'s stays what it was, and the three question verbs refuse with
/// the same 2 they answer nothing with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub message: String,
    pub exit: u8,
}

/// The exit code `summary` and a usage error have always had.
const SUMMARY_REFUSED_EXIT: u8 = 1;

/// An `ask` that answered no.
const ASK_NO_EXIT: u8 = 1;

/// A question verb that has no answer to give — or was not a question.
const NO_ANSWER_EXIT: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SummaryRequest {
    cwd: Option<PathBuf>,
    sessions: Option<PathBuf>,
    /// How many of each seat's last requests to list; none by default.
    recent: usize,
    json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JudgeRequest {
    shape: JevShape,
    question: String,
    context: Option<String>,
    options: Vec<String>,
    levels: Vec<String>,
    items: Vec<String>,
    /// Read the options or items from standard input.
    stdin: bool,
    cwd: Option<PathBuf>,
    json: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    Summary(SummaryRequest),
    Judge(JudgeRequest),
}

fn usage(exit: u8) -> Refused {
    Refused { message: USAGE.to_string(), exit }
}

fn parse(args: &[String]) -> Result<Request, Refused> {
    match args.first().map(String::as_str) {
        Some("summary") => parse_summary(&args[1..]).map(Request::Summary),
        Some(verb) if JevShape::from_word(verb).is_some() => {
            let shape = JevShape::from_word(verb).unwrap_or(JevShape::Ask);
            parse_judge(shape, &args[1..]).map(Request::Judge)
        }
        Some("-h" | "--help") | None => Err(usage(SUMMARY_REFUSED_EXIT)),
        Some(other) => Err(Refused {
            message: format!("unknown verb '{other}'\n\n{USAGE}"),
            exit: SUMMARY_REFUSED_EXIT,
        }),
    }
}

fn parse_summary(args: &[String]) -> Result<SummaryRequest, Refused> {
    let refuse = |message: String| Refused { message, exit: SUMMARY_REFUSED_EXIT };
    let mut request = SummaryRequest { cwd: None, sessions: None, recent: 0, json: false };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--json" => request.json = true,
            "--cwd" => {
                let dir = rest.next().ok_or_else(|| refuse("--cwd needs a directory".to_string()))?;
                request.cwd = Some(PathBuf::from(dir));
            }
            "--computer-use" => {
                let dir = rest.next().ok_or_else(|| refuse("--computer-use needs a directory".to_string()))?;
                request.sessions = Some(PathBuf::from(dir));
            }
            "--recent" => {
                let count = rest.next().ok_or_else(|| refuse("--recent needs a count".to_string()))?;
                request.recent =
                    count.parse().map_err(|_| refuse(format!("--recent needs a count, not '{count}'")))?;
            }
            other => return Err(refuse(format!("unknown argument '{other}'\n\n{USAGE}"))),
        }
    }
    Ok(request)
}

fn parse_judge(shape: JevShape, args: &[String]) -> Result<JudgeRequest, Refused> {
    let refuse = |message: String| Refused { message, exit: NO_ANSWER_EXIT };
    let mut request = JudgeRequest {
        shape,
        question: String::new(),
        context: None,
        options: Vec::new(),
        levels: Vec::new(),
        items: Vec::new(),
        stdin: false,
        cwd: None,
        json: false,
    };
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let mut value = |flag: &str| {
            rest.next().cloned().ok_or_else(|| refuse(format!("{flag} needs a value")))
        };
        match arg.as_str() {
            "--json" => request.json = true,
            "--stdin" => request.stdin = true,
            "--cwd" => request.cwd = Some(PathBuf::from(value("--cwd")?)),
            "--context" => request.context = Some(value("--context")?),
            "--option" => request.options.push(value("--option")?),
            "--level" => request.levels.push(value("--level")?),
            "--item" => request.items.push(value("--item")?),
            flag if flag.starts_with("--") => {
                return Err(refuse(format!("unknown argument '{flag}'\n\n{USAGE}")));
            }
            word if request.question.is_empty() => request.question = word.to_string(),
            word => {
                return Err(refuse(format!(
                    "one question only; '{word}' came after it (use --option, --level or --item for the parts)"
                )));
            }
        }
    }
    if request.question.trim().is_empty() {
        return Err(refuse(format!("{} needs a question\n\n{USAGE}", shape.word())));
    }
    Ok(request)
}

/// The options or items standard input carried: one JSON array of strings,
/// or one per line with blank lines dropped.
#[must_use]
pub fn parts_from_text(text: &str) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.starts_with('[') {
        if let Ok(parts) = serde_json::from_str::<Vec<String>>(trimmed) {
            return parts;
        }
    }
    text.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_string).collect()
}

/// What the command printed, and the exit code that says the answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub text: String,
    pub exit: u8,
}

/// # Errors
///
/// The usage, or what was wrong with the arguments — with the verb's own
/// refusal code.
pub fn run(args: &[String], cwd: &Path, now_ms: i64, offset_s: i64) -> Result<Report, Refused> {
    match parse(args)? {
        Request::Summary(request) => Ok(run_summary(&request, cwd, now_ms, offset_s)),
        Request::Judge(request) => {
            let parts = if request.stdin {
                let mut text = String::new();
                std::io::stdin()
                    .read_to_string(&mut text)
                    .map_err(|error| Refused { message: format!("could not read stdin: {error}"), exit: NO_ANSWER_EXIT })?;
                parts_from_text(&text)
            } else {
                Vec::new()
            };
            run_judge(&request, parts, cwd)
        }
    }
}

fn run_summary(request: &SummaryRequest, cwd: &Path, now_ms: i64, offset_s: i64) -> Report {
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
    Report {
        text: if request.json {
            render_json(&seats).to_string()
        } else {
            render_text(&seats)
        },
        exit: HEADLESS_LOOP_EXIT_DONE,
    }
}

/// Decide one question verb: `parts` is what standard input carried, read
/// as the options of a `choose` or the items of a `score`.
fn run_judge(request: &JudgeRequest, parts: Vec<String>, cwd: &Path) -> Result<Report, Refused> {
    let (mut options, mut items) = (request.options.clone(), request.items.clone());
    match request.shape {
        JevShape::Choose => options.extend(parts),
        JevShape::Score => items.extend(parts),
        JevShape::Ask => {}
    }
    let question = JevQuestion::new(
        request.shape,
        &request.question,
        request.context.as_deref(),
        &options,
        &request.levels,
        &items,
    )
    .map_err(|refused| Refused { message: refused.0, exit: NO_ANSWER_EXIT })?;
    let cwd = request.cwd.clone().unwrap_or_else(|| cwd.to_path_buf());
    let verdict = tools::jev_decide(&cwd, JevCaller::Cli, &question);
    Ok(report_of(&verdict, request.json))
}

/// The verdict as a shell sees it: its text and its exit code.
#[must_use]
pub fn report_of(verdict: &JevVerdict, json: bool) -> Report {
    Report {
        text: if json {
            serde_json::to_string_pretty(verdict).unwrap_or_default()
        } else {
            render_verdict_text(verdict)
        },
        exit: exit_of(verdict),
    }
}

/// The exit code a verdict answers with: an `ask` says yes or no with it,
/// the others say whether there is an answer at all.
#[must_use]
pub fn exit_of(verdict: &JevVerdict) -> u8 {
    match &verdict.answer {
        Some(JevAnswer::Ask { yes: false, .. }) => ASK_NO_EXIT,
        Some(JevAnswer::Ask { yes: true, .. } | JevAnswer::Choose { .. } | JevAnswer::Score { .. }) => {
            HEADLESS_LOOP_EXIT_DONE
        }
        None => NO_ANSWER_EXIT,
    }
}

/// The verdict as a person reads it: the answer first, the numbers under it.
fn render_verdict_text(verdict: &JevVerdict) -> String {
    let mut out = String::new();
    let mut numbers = format!("{} ms", verdict.elapsed_ms);
    if let Some(tokens) = verdict.input_tokens {
        let _ = write!(numbers, " · {tokens} tokens");
    }
    if let Some(cost) = verdict.cost_usd {
        let _ = write!(numbers, " · ${cost:.6}");
    }
    match &verdict.answer {
        Some(JevAnswer::Ask { yes, p_yes, confidence }) => {
            let _ = writeln!(out, "{}", if *yes { "yes" } else { "no" });
            let _ = writeln!(out, "p(yes) {p_yes:.2} · confidence {confidence:.2} · {numbers}");
        }
        Some(JevAnswer::Choose { chosen, index, probabilities, confidence }) => {
            let _ = writeln!(out, "{chosen}");
            let _ = writeln!(
                out,
                "option {} of {} · p {:.2} · confidence {confidence:.2} · {numbers}",
                index + 1,
                probabilities.len(),
                probabilities.get(*index).copied().unwrap_or(0.0)
            );
        }
        Some(JevAnswer::Score { items }) => {
            for item in items {
                let _ = writeln!(out, "{:.2}\t{:.2}\t{:.2}\t{}", item.score, item.normalised, item.confidence, item.item);
            }
            let _ = writeln!(out, "{} scored · {numbers}", items.len());
        }
        None => {
            let _ = writeln!(out, "no answer: {}", verdict.outcome);
        }
    }
    let _ = write!(out, "{}", verdict.note);
    out
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
                // Which id the seat asks with — the person's pin, or the
                // alias — and which version the newest answer named: the two
                // halves of "is the seat still measuring what it was fitted
                // to" (t-6187).
                "askedModel": seat.asked_model,
                "model": seat.model,
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
                "verdict": seat.judged.as_ref().map(|judged| json!({
                    "verdict": judged.verdict.token(),
                    "line": judged.verdict.line().map(tools::jev_summary::line_token),
                    // The version the rows were cut away from, beside the
                    // line the seat holds on: a thin sample says why.
                    "cutModel": judged.cut,
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
        if let Some(judged) = seat.judged.as_ref() {
            let verdict = judged.verdict;
            let _ = write!(
                notes,
                "{}{}{}",
                if notes.is_empty() { "" } else { " · " },
                verdict.token(),
                verdict.line().map_or_else(String::new, |line| format!(" ({})", line.token()))
            );
            if let Some(cut) = judged.cut.as_deref() {
                let _ = write!(notes, " · cut at {cut}");
            }
        }
        if let Some(model) = seat.model.as_deref() {
            let _ = write!(
                notes,
                "{}asks {} · answered by {model}",
                if notes.is_empty() { "" } else { " · " },
                seat.asked_model
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
