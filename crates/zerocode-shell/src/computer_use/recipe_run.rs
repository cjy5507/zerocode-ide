//! `recipe-run` (docs/design/computer-use-full-operator.md §7.2): a saved
//! recipe walked in one call. Every decision is the core's
//! (`zerocode_core::computer_recipe`), every step goes down the lone
//! command's road the caller hands in — the desktop's or the browser's, by
//! the line's own door; the stop included, which refuses a step at the door
//! and leaves its line — and the loop is the one every walk takes
//! (`zerocode_core::computer_use::walk`): the recipe's lines, a Flow's
//! baseline probe before them and its oracle after them are three walks of
//! that one loop. What is here reads the desk around each step — the
//! pointer, the screen where a press lands, the pages the browser panes show
//! — times what each step spent where (plan D4), binds a Flow's acts to the
//! apps and hosts it was recorded in (D8), asks its event lines once before
//! the first act and every line after the last (D9) — with the verify
//! budget after a walk that reached its end, the probe's zero wait after
//! one cut short —, sends its app acts with or without their picture by its
//! evidence level (D10), stands the money gate before a Flow's money step
//! (docs/design/flow-engine-guarded-money-path.md — the transaction given
//! and confirmed, unseen by the ledger, shown by the page, one ledger line
//! before the hand), and writes the report the one evidence writer keeps as
//! the walk's record. Nothing here writes evidence: the caller hands the
//! report to the writer. The one file the walk writes itself is a guarded
//! Flow's ledger (`guarded::Ledger`), because its line must be on disk
//! before the hand moves.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde_json::{Map, Value, json};
use zerocode_core::agent_browser;
use zerocode_core::agent_emulator;
use zerocode_core::computer_flow::{
    Check, CheckKind, EvidenceLevel, Fingerprint, FlowRecord, FlowSpec, Money, Observed, Policy,
    Presence, Target, judge, page_host, parse_flow,
};
use zerocode_core::computer_recipe::{
    Plan, RECIPE_CHECKS, RECIPE_CURSOR_SLOP_POINTS, RecipeLine, RecipeStop, RecipeTool, preflight,
    recipe_landing_box, recipe_line_argv, recipe_line_holds_ms, recipe_lines, recipe_step_argv,
};
use zerocode_core::computer_use::{
    COMPUTER_CLI, ComputerCommand, ComputerMethod, FLOW_BASELINE_PROBE_MS, FLOW_VERIFY_MS, Next,
    WalkBudget, Walked, hold_of, look_until_settled, named_point, parse_command, verb_method, walk,
    walk_budget_ms, walk_step_report,
};
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::ITEMS_KEY;
use zerocode_core::computer_use_protocol::{
    Verification, answer_envelope, error_code, unverified_reason,
};
use zerocode_hookd::TeamAnswer;

use super::guarded::{self, By, Ledger, Line, Refusal, State, Transaction};
use crate::cmd::browser::{WAIT_TIMED_OUT, pressed_rect};

/// What a walk reads from the desk besides the steps it runs — each a seam a
/// test replaces.
pub trait Desk {
    /// Where the pointer is, in screen points, when the helper can say.
    fn pointer(&mut self) -> Option<(f64, f64)>;
    /// A picture of the desktop in `region` (`[x, y, width, height]`, screen
    /// points) — of whichever display shows it — and its frame.
    fn picture(&mut self, region: [f64; 4]) -> Option<(Vec<u8>, ShotFrame)>;
    /// Whether anything changed between two pictures of one frame.
    fn changed(&self, before: &[u8], after: &[u8], frame: ShotFrame) -> Option<bool>;
    /// Milliseconds since the walk began.
    fn elapsed_ms(&self) -> u64;
    /// The wall clock, for the record's `at_epoch_ms`.
    fn now_epoch_ms(&self) -> i64;
    fn pause(&mut self, pause: Duration);
    /// The pages the window's browser panes show — `(label, url)` — from the
    /// window's own records, never a webview asked (the URL crash of
    /// 2026-09-07): what a Flow's fingerprint is matched against, and what a
    /// browser act is bound to before it is sent.
    fn pages(&mut self) -> Vec<(String, String)>;
}

/// The desk as it is: the helper's pointer and pictures, the one diff
/// `observe --diff` uses — never `observe` itself, whose looks feed the stuck
/// detector and fill the viewers' last-look table — and, in a window, the
/// browser panes' recorded pages.
pub struct LiveDesk {
    began: std::time::Instant,
    window: Option<tauri::AppHandle>,
}

impl LiveDesk {
    #[must_use]
    pub fn new() -> Self {
        Self {
            began: std::time::Instant::now(),
            window: None,
        }
    }

    /// The desk of a window: its browser panes' pages are read from it.
    #[must_use]
    pub fn in_window(mut self, app: tauri::AppHandle) -> Self {
        self.window = Some(app);
        self
    }
}

impl Default for LiveDesk {
    fn default() -> Self {
        Self::new()
    }
}

impl Desk for LiveDesk {
    fn pointer(&mut self) -> Option<(f64, f64)> {
        let at = super::call("cursorPosition", json!({})).ok()?;
        Some((at.get("x")?.as_f64()?, at.get("y")?.as_f64()?))
    }

    fn picture(&mut self, [x, y, width, height]: [f64; 4]) -> Option<(Vec<u8>, ShotFrame)> {
        let region = json!({ "x": x, "y": y, "width": width, "height": height });
        let answer = super::call("screenshotDesktop", json!({ "region": region })).ok()?;
        Some((
            super::screenshot_png(&answer)?,
            ShotFrame::from_answer(&answer)?,
        ))
    }

    fn changed(&self, before: &[u8], after: &[u8], frame: ShotFrame) -> Option<bool> {
        super::observe::changes(Some(before), after, frame).map(|(regions, _)| !regions.is_empty())
    }

    fn elapsed_ms(&self) -> u64 {
        u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn now_epoch_ms(&self) -> i64 {
        crate::project_runtime::now_epoch_ms()
    }

    fn pause(&mut self, pause: Duration) {
        std::thread::sleep(pause);
    }

    fn pages(&mut self) -> Vec<(String, String)> {
        use crate::ShellStateExt;
        use tauri::Manager;
        let Some(app) = &self.window else {
            return Vec::new();
        };
        let mut pages: Vec<(String, String)> = app
            .state::<crate::AppState>()
            .browser_urls()
            .iter()
            .map(|(label, url)| (label.clone(), url.clone()))
            .collect();
        pages.sort();
        pages
    }
}

/// A walk that could not start: nothing moved. `code` is the error code the
/// answer carries — a recipe that does not read, a fingerprint the live
/// desk does not match (`flow_stale`), a money line without its transaction
/// (`flow_money_unbound`), a confirmation of another transaction
/// (`flow_confirm_unbound`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub code: &'static str,
    pub message: String,
}

impl Refused {
    fn invalid(message: String) -> Self {
        Self {
            code: error_code::INVALID_ARGUMENT,
            message,
        }
    }
}

/// One walk as the caller asks for it: the command with its values and
/// start, the recipe (the file it was read from, and its text), the bridge's
/// deadline, and the workspace the request came from. Evidence identity is
/// attached to the actual step call, never predicted from the folder's length.
pub struct Run<'a> {
    pub command: &'a ComputerCommand,
    /// An internal recovery cursor, within the original requested range.
    /// The caller's command remains unchanged; a resumed tail is not a new request.
    pub resume_from: Option<usize>,
    pub file: &'a str,
    pub text: &'a str,
    pub deadline_ms: u64,
    pub cwd: Option<&'a Path>,
    /// The evidence folder, when the walk has one — named in a money
    /// ledger's lines, so a transaction points at its proof.
    pub evidence_dir: Option<&'a Path>,
}

/// The transaction `--confirm` named, if the run was given one.
fn confirm_flag(command: &ComputerCommand) -> Option<&str> {
    command.params.get("confirm").and_then(Value::as_str)
}

/// The values `--params` gave the run.
fn values_of(command: &ComputerCommand) -> Map<String, Value> {
    command
        .params
        .get("recipeParams")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn requested_start(command: &ComputerCommand) -> usize {
    command
        .params
        .get("start")
        .and_then(Value::as_u64)
        .and_then(|start| usize::try_from(start).ok())
        .unwrap_or(1)
}

/// A recipe read and decided from `command`'s start with its values — what
/// runs, before anything moves.
pub fn decide(
    command: &ComputerCommand,
    text: &str,
) -> Result<
    (
        Vec<RecipeLine>,
        usize,
        zerocode_core::computer_recipe::Preflight,
    ),
    Refused,
> {
    let lines = recipe_lines(text).map_err(Refused::invalid)?;
    let start = requested_start(command);
    let end = command
        .params
        .get("end")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(lines.len());
    if end < start || end > lines.len() {
        return Err(Refused::invalid(
            "--end must name a recipe step at or after --start".into(),
        ));
    }
    let decided = preflight(&lines[..end], start, &values_of(command)).map_err(Refused::invalid)?;
    Ok((lines, start, decided))
}

/// The first action a walk from `command`'s start would take, as it is sent
/// and as its line is logged — what a walk refused at a stopped door leaves,
/// as that action would have left itself alone. None when the walk stops for
/// the person or a look before it acts.
#[must_use]
pub fn first_act(command: &ComputerCommand, text: &str) -> Option<(Vec<String>, Vec<String>)> {
    let (_, _, decided) = decide(command, text).ok()?;
    let framed = walk_level(parse_flow(text).ok().flatten().as_ref()).frames();
    decided
        .planned
        .iter()
        .find_map(|(line, plan)| match plan {
            Plan::Run(argv)
                if line.tool == RecipeTool::Computer
                    && parse_command(argv).is_ok_and(|command| command.method.acts()) =>
            {
                Some(Some((
                    recipe_step_argv(argv, framed),
                    recipe_step_argv(&line.argv, framed),
                )))
            }
            Plan::Run(_) | Plan::Skip(_) => None,
            Plan::Stop(_) => Some(None),
        })
        .flatten()
}

/// The terms a walk goes under (plan D8·D10): the fingerprint its acts are
/// bound to when its recipe is a Flow, and whether the one evidence writer
/// keeps a frame of each step — its evidence level's word, which decides the
/// flags its app acts are sent with (`recipe_line_argv`).
#[derive(Clone, Copy)]
struct Terms<'f> {
    bound: Option<&'f Fingerprint>,
    framed: bool,
    /// The gate before the Flow's money step, when it has one (design §2–§4).
    money: Option<&'f MoneyGate>,
    /// Only a live recipe with a known origin can offer a one-line retry.
    retry: Option<(&'f str, &'f Path)>,
}

/// The evidence level a walk keeps (plan D10): its Flow's, or the default —
/// `full` — for a recipe without one.
fn walk_level(flow: Option<&FlowSpec>) -> EvidenceLevel {
    flow.map_or_else(EvidenceLevel::default, |spec| spec.evidence)
}

/// What one walked step spent where, in milliseconds: the act's round trip
/// down its road, the settle looked through after a press, the read-back
/// judged after a typing, and the pointer read after the step. Their sum is
/// within the walk's own `ms` for the step.
#[derive(Debug, Default, Clone, Copy)]
struct Phases {
    act: u64,
    settle: u64,
    verify: u64,
    pointer: u64,
}

impl Phases {
    fn value(self) -> Value {
        json!({
            "act": self.act,
            "settle": self.settle,
            "verify": self.verify,
            "pointer": self.pointer,
        })
    }
}

/// What stands before a Flow's money step (design §1–§4), bound before the
/// walk from the words the run was given: the transaction, who confirmed it
/// (or nobody yet), the Flow's ledger — and, once the gate let the hand
/// through, who it was that confirmed, for the outcome line.
struct MoneyGate {
    policy: Policy,
    step: usize,
    name: String,
    txn: Transaction,
    by: Option<By>,
    ledger: Ledger,
    evidence: Option<String>,
    acting: Cell<Option<By>>,
}

/// The gate's refusal at the money step: the step's error code, its
/// message (the card), and the stop it makes.
type Gated = (&'static str, String, RecipeStop);

impl MoneyGate {
    /// Bind the gate, or refuse the run before anything moves: the three
    /// parameters must have values (`flow_money_unbound`) and a `--confirm`
    /// must name this transaction (`flow_confirm_unbound`).
    fn bind(spec: &FlowSpec, money: &Money, run: &Run<'_>) -> Result<Self, Refused> {
        let txn = guarded::transaction(money, &values_of(run.command)).map_err(|why| Refused {
            code: error_code::FLOW_MONEY_UNBOUND,
            message: why,
        })?;
        let flag = confirm_flag(run.command);
        let by = guarded::confirmation(&spec.confirm, &txn, flag).map_err(|code| Refused {
            code,
            message: format!(
                "--confirm {} is not a confirmation of this transaction ({}): a confirmation is the transaction's own",
                flag.unwrap_or_default(),
                txn.txn
            ),
        })?;
        Ok(Self {
            policy: spec.policy,
            step: money.step,
            name: run
                .command
                .params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            txn,
            by,
            ledger: Ledger::beside(Path::new(run.file)),
            evidence: run.evidence_dir.map(|dir| dir.display().to_string()),
            acting: Cell::new(None),
        })
    }

    /// One ledger line for this transaction.
    fn line(&self, state: State, by: By, at_epoch_ms: i64) -> Line {
        Line {
            txn: self.txn.txn.clone(),
            amount: self.txn.amount.clone(),
            recipient: self.txn.recipient.clone(),
            at_epoch_ms,
            state,
            evidence: self.evidence.clone(),
            step: Some(self.step),
            by,
        }
    }

    /// The transaction as the card names it.
    fn said(&self) -> String {
        format!(
            "transaction {}: {} to {}",
            self.txn.txn, self.txn.amount, self.txn.recipient
        )
    }

    /// The gate before the money step, in the design's order (§2–§4): a
    /// rehearsal ends here; a guarded step needs its confirmation, a
    /// transaction the ledger has not seen spent, a page that agrees — then
    /// its `confirmed` and `acting` lines, on disk before the hand. `probe`
    /// is the walk's own check road, asked at no wait.
    fn before(
        &self,
        line: &RecipeLine,
        probe: impl FnOnce(&[Check]) -> BTreeMap<usize, Observed>,
        at_epoch_ms: i64,
    ) -> Result<(), Gated> {
        if self.policy == Policy::Dry {
            return Err((
                error_code::RECIPE_STOPPED,
                format!(
                    "a `{}` Flow rehearses up to its money step and never presses it: {} stays unmoved",
                    Policy::Dry.as_str(),
                    self.said()
                ),
                RecipeStop::MoneyStep,
            ));
        }
        let Some(by) = self.by else {
            return Err((
                error_code::RECIPE_STOPPED,
                format!(
                    "{} waits for the person's confirmation — `{COMPUTER_CLI} recipe-run --name {} --start {} --confirm {} --params …` presses it",
                    self.said(),
                    self.name,
                    self.step,
                    self.txn.txn
                ),
                RecipeStop::MoneyStep,
            ));
        };
        let seen = |state: State| {
            (
                error_code::FLOW_TXN_SEEN,
                format!(
                    "{} is already {} in {}: a transaction moves once",
                    self.said(),
                    state.as_str(),
                    self.ledger.path().display()
                ),
                RecipeStop::StepFailed,
            )
        };
        let unwritten = |why: String| {
            (
                error_code::ACCESSIBILITY_ERROR,
                format!(
                    "the ledger {} could not be written, so the hand stays still: {why}",
                    self.ledger.path().display()
                ),
                RecipeStop::StepFailed,
            )
        };
        if let Some(state) = self
            .ledger
            .seen(&self.txn.txn)
            .map_err(|error| unwritten(error.to_string()))?
            .filter(|state| state.spent())
        {
            return Err(seen(state));
        }
        guarded::page_agrees(line, &self.txn, probe)
            .map_err(|why| (error_code::FLOW_PAGE_DISAGREES, why, RecipeStop::StepFailed))?;
        // Who let the hand through, then the hand: two lines in one durable
        // write, refused whole if another writer's acting line came first.
        self.ledger
            .record(&[
                self.line(State::Confirmed, by, at_epoch_ms),
                self.line(State::Acting, by, at_epoch_ms),
            ])
            .map_err(|refusal| match refusal {
                Refusal::Seen(state) => seen(state),
                Refusal::Io(error) => unwritten(error.to_string()),
            })?;
        self.acting.set(Some(by));
        Ok(())
    }

    /// After the oracle: the outcome line of a transaction the gate let
    /// through — `acted` when every line watching the money passed, else
    /// `failed` — and what the report says of it. A ledger that cannot take
    /// the outcome is said so, never silently.
    fn after(&self, record: &FlowRecord, at_epoch_ms: i64) -> Option<Value> {
        let by = self.acting.get()?;
        let state = if record.spec.money_acted(&record.verdict) {
            State::Acted
        } else {
            State::Failed
        };
        let mut said = json!({
            "txn": self.txn.txn,
            "state": state.as_str(),
            "by": by,
            "ledger": self.ledger.path().display().to_string(),
        });
        if let Err(refusal) = self.ledger.record(&[self.line(state, by, at_epoch_ms)]) {
            said["unwritten"] = json!(refusal.to_string());
        }
        Some(said)
    }
}

/// Walk the recipe as `run` asks — its values, its start — each step through
/// `step` (the lone command's road for the line's door, handing a guarded
/// press back), given the door, the line to run and the line to log: the
/// recipe's own words, so a value it was given never reaches the log. A walk
/// that stops at the person's step leaves that line with `leave` — the
/// person may do it by hand, and a recipe saved from the log must keep it.
/// `begin` is told the walk's evidence level once the walk may start — after
/// the preflight and a Flow's fingerprint, before its first act — so the
/// caller can tell the writer. Answers the report, or why nothing was walked.
pub fn run(
    run: &Run<'_>,
    mut step: impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
    leave: impl FnOnce(&[String], &[String], &TeamAnswer),
    begin: impl FnOnce(EvidenceLevel),
    desk: impl Desk,
    caller_waits: impl Fn() -> bool,
) -> Result<Value, Refused> {
    let desk = RefCell::new(desk);
    let elapsed = || desk.borrow().elapsed_ms();
    let began = elapsed();
    let flow = parse_flow(run.text).map_err(Refused::invalid)?;
    // The money gate binds before the preflight: a transaction missing its
    // values is `flow_money_unbound`, not a parameter the preflight lists.
    let gate = flow
        .as_ref()
        .and_then(|spec| spec.money.as_ref().map(|money| (spec, money)))
        .map(|(spec, money)| MoneyGate::bind(spec, money, run))
        .transpose()?;
    if gate.is_none() && confirm_flag(run.command).is_some() {
        return Err(Refused::invalid(
            "--confirm confirms a Flow's money step; this recipe has no money line".into(),
        ));
    }
    if run
        .resume_from
        .is_some_and(|step| step < requested_start(run.command))
    {
        return Err(Refused::invalid(
            "a recovery cannot resume before the requested range".into(),
        ));
    }
    let resumed = run.resume_from.map(|step| {
        let mut command = run.command.clone();
        command.params["start"] = json!(step);
        command
    });
    let (lines, start, decided) = decide(resumed.as_ref().unwrap_or(run.command), run.text)?;
    if let Some(spec) = &flow {
        spec.fingerprint
            .matches(&live_fingerprint(&mut *desk.borrow_mut()))
            .map_err(|stale| Refused {
                code: error_code::FLOW_STALE,
                message: stale.said(),
            })?;
    }
    let level = walk_level(flow.as_ref());
    begin(level);
    let mut phases = json!({ "resolve": elapsed().saturating_sub(began) });
    // The baseline (D8): every event line asked once with no wait, so an
    // event that already holds is known to be stale, not this run's.
    let mut baseline: BTreeMap<usize, Presence> = BTreeMap::new();
    if let Some(spec) = &flow {
        let probing = elapsed();
        let events = spec
            .checks
            .iter()
            .filter(|check| check.kind == CheckKind::Event);
        for (id, seen) in check_walk(
            events,
            FLOW_BASELINE_PROBE_MS,
            run.deadline_ms,
            &mut step,
            &desk,
            &caller_waits,
        ) {
            if let Observed::Seen(presence) = seen {
                baseline.insert(id, presence);
            }
        }
        phases["baseline"] = json!(elapsed().saturating_sub(probing));
    }
    let at_epoch_ms = desk.borrow().now_epoch_ms();
    let (walked, watched) = walk_lines(
        &decided.planned,
        run.deadline_ms,
        &mut step,
        &desk,
        &caller_waits,
        Terms {
            bound: flow.as_ref().map(|spec| &spec.fingerprint),
            framed: level.frames(),
            money: gate.as_ref(),
            retry: run
                .cwd
                .filter(|_| run.command.params.get("arena").is_none())
                .and_then(|cwd| {
                    let name = run.command.params.get("name")?.as_str()?;
                    (!name.is_empty() && !cwd.as_os_str().is_empty()).then_some((name, cwd))
                }),
        },
    );
    if let Some((n, kind)) = walked.halted
        && matches!(kind, RecipeStop::PersonsTurn | RecipeStop::PersonsLastStep)
        && let Some((line, Plan::Stop(_))) = decided.planned.get(n - 1)
    {
        let words = recipe_step_argv(&line.argv, level.frames());
        let stopped = json!({
            "ok": false,
            "error": { "code": error_code::RECIPE_STOPPED, "message": kind.as_str() },
        });
        leave(
            &words,
            &words,
            &TeamAnswer {
                stdout: String::new(),
                stderr: format!("{stopped}\n"),
                exit_code: 1,
            },
        );
    }
    // The oracle (D9): every line asked again and judged afresh — never from
    // the last verdict — with the verify budget after a walk that reached
    // its end. A walk cut short did not do what its recipe says; its verdict
    // is fail whatever its lines read (`verdict_of`), so they are asked with
    // the probe's zero wait: what is there now, not the verify budget spent
    // waiting for what the walk never did (t-4230).
    let budget_ms = if walked.finished() {
        FLOW_VERIFY_MS
    } else {
        FLOW_BASELINE_PROBE_MS
    };
    let record = flow.map(|spec| {
        let verifying = elapsed();
        let observed = check_walk(
            spec.checks.iter(),
            budget_ms,
            run.deadline_ms,
            &mut step,
            &desk,
            &caller_waits,
        );
        phases["oracle"] = json!(elapsed().saturating_sub(verifying));
        let verdict = judge(&spec.checks, &baseline, &observed);
        FlowRecord {
            spec,
            baseline,
            observed,
            verdict,
        }
    });
    let reporting = elapsed();
    let mut report = report(
        run.command,
        run.file,
        &lines,
        start,
        &decided,
        &walked,
        run.deadline_ms,
    );
    report["kind"] = json!("recipe-run");
    report["evidence_ids"] = json!(true);
    report["at_epoch_ms"] = json!(at_epoch_ms);
    report["handWatched"] = Value::Bool(watched);
    // The run's whole time, its verification included: speed is verified
    // completion time.
    report["elapsedMs"] = json!(elapsed());
    if let Some(record) = record {
        if let Some(money) = gate
            .as_ref()
            .and_then(|gate| gate.after(&record, desk.borrow().now_epoch_ms()))
        {
            report["money"] = money;
        }
        report["flow"] = serde_json::to_value(record).unwrap_or(Value::Null);
    }
    phases["report"] = json!(elapsed().saturating_sub(reporting));
    report["phases"] = phases;
    Ok(report)
}

/// The live fingerprint before the first act: the hosts of the pages the
/// window's panes show (the apps are read from each act's answer as the
/// walk goes), on this build's protocol.
fn live_fingerprint(desk: &mut impl Desk) -> Fingerprint {
    let pages: Vec<Value> = desk
        .pages()
        .into_iter()
        .map(|(_, url)| json!({ "url": url }))
        .collect();
    Fingerprint::observe(&[Value::Array(pages)])
}

/// The verdict a walk's report carries for the folder, if it walked a Flow:
/// the judge's word on the oracle lines — and the walk itself finished, since
/// a Flow cut short did not do what its recipe says, whatever its lines
/// read afterwards. When not passed: the stop that cut the walk short, and
/// the lines that did not pass.
#[must_use]
pub fn verdict_of(report: &Value) -> Option<(bool, Option<String>)> {
    let verdict = report.pointer("/flow/verdict")?;
    let pass = verdict.get("pass")?.as_bool()?
        && report["done"] == Value::Bool(true)
        && report
            .get("complete")
            .is_none_or(|complete| complete == &Value::Bool(true));
    if pass {
        return Some((true, None));
    }
    let mut why: Vec<String> = Vec::new();
    if report.get("complete") == Some(&Value::Bool(false)) && report["done"] == Value::Bool(true) {
        why.push("only the selected recipe range ran".into());
    }
    if let Some(stop) = report.get("stop").filter(|stop| !stop.is_null()) {
        why.push(format!(
            "stopped at step {} ({})",
            report["stoppedAt"],
            stop["kind"].as_str().unwrap_or_default()
        ));
    }
    why.extend(
        verdict
            .get("lines")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|line| line.get("status").and_then(Value::as_str) != Some("pass"))
            .map(|line| {
                format!(
                    "{} {}: {}",
                    line["id"],
                    line["status"].as_str().unwrap_or_default(),
                    line["said"].as_str().unwrap_or_default()
                )
            }),
    );
    Some((false, Some(why.join("; "))))
}

/// The kind of stop a refused step makes: the person's press handed back,
/// the operator stopped, a check the screen failed, or a step refused. A
/// browser step is never a check the walk judges (its `wait` is a visibility
/// wait, not an oracle — review 20) and hands nothing back: refused, it
/// failed.
fn stop_for(tool: RecipeTool, code: &str, check: bool) -> RecipeStop {
    match (tool, code) {
        // Browser and emulator checks are judged in the oracle pass;
        // a refusal in the ordinary steps means that step failed.
        (RecipeTool::Browser | RecipeTool::Emulator, _) => RecipeStop::StepFailed,
        (_, error_code::CONFIRMATION_REQUIRED) => RecipeStop::PersonsLastStep,
        // A password field: the person types it, then the walk goes on.
        (_, error_code::SECURE_INPUT) => RecipeStop::PersonsTurn,
        (_, error_code::STOPPED | error_code::BUDGET_EXCEEDED | error_code::PERSON_ASKED) => {
            RecipeStop::Stopped
        }
        _ if check => RecipeStop::CheckFailed,
        _ => RecipeStop::StepFailed,
    }
}

/// The browser door's answer as the walk reads it — an envelope like the
/// desktop's: the door answers a sentence (or a JSON object, `find`) on
/// stdout and refuses on stderr with no code of its own.
fn browser_envelope(answer: &TeamAnswer) -> Value {
    if answer.exit_code == 0 {
        let said = answer.stdout.trim();
        let result = serde_json::from_str::<Value>(said)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({ "said": said }));
        json!({ "ok": true, "result": result })
    } else {
        json!({
            "ok": false,
            "error": { "code": error_code::UNANSWERED, "message": answer.stderr.trim() },
        })
    }
}

/// A step's report when the walk refused it before sending: an envelope of
/// its own, so the report reads like any refusal's.
fn refused_report(n: usize, argv: &[String], code: &str, message: String) -> Value {
    let envelope = json!({ "ok": false, "error": { "code": code, "message": message } });
    walk_step_report(n, argv, false, Some(&envelope), "")
}

/// Why a browser act may not be sent under `print`: it aims at a host the
/// recording never saw — `goto` by the URL it goes to, the page verbs by
/// the pane's recorded page — or at a page whose host the window does not
/// know (fail closed).
fn browser_act_outside(
    print: &Fingerprint,
    argv: &[String],
    desk: &mut impl Desk,
) -> Option<String> {
    let verb = argv.first().map_or("", String::as_str);
    let host = if verb == "goto" {
        argv.get(2).and_then(|url| page_host(url))
    } else {
        let label = argv.get(1)?;
        desk.pages()
            .into_iter()
            .find(|(shown, _)| shown == label)
            .and_then(|(_, url)| page_host(&url))
    };
    match host {
        Some(host) if print.allows(&Target::Host(host.clone())) => None,
        Some(host) => Some(format!(
            "{verb} aims at {host}, a host the recording never saw"
        )),
        None => Some(format!(
            "{verb} aims at a page whose host the window does not know"
        )),
    }
}

/// The app a desktop act answered from, when it is not one of the
/// recording's: read from the answer the act already gave, never a tree
/// walked again.
fn app_outside(print: &Fingerprint, result: Option<&Value>) -> Option<String> {
    Fingerprint::observe(std::slice::from_ref(result?))
        .apps
        .into_iter()
        .find(|app| !print.allows(&Target::App(app.clone())))
}

/// A rectangle `{x, y, width, height}` as `[x, y, width, height]`.
fn rect_of(frame: &Value) -> Option<[f64; 4]> {
    Some([
        frame.get("x")?.as_f64()?,
        frame.get("y")?.as_f64()?,
        frame.get("width")?.as_f64()?,
        frame.get("height")?.as_f64()?,
    ])
}

/// What an act pressed, for the report's ring (plan D11): a browser click's
/// rect in CSS pixels with the page's ratio, read from the door's sentence;
/// a mark click's rect from the mark table the answer carries, with its
/// label and number; a point press's landing box — the one the walk judges
/// the press in. Nothing is asked of the page or the tree.
fn target_of(
    tool: RecipeTool,
    argv: &[String],
    command: Option<&ComputerCommand>,
    result: Option<&Value>,
    answer: &TeamAnswer,
    landing: Option<[f64; 4]>,
) -> Option<Value> {
    match tool {
        RecipeTool::Browser => {
            if argv.first().map(String::as_str) != Some("click") {
                return None;
            }
            let (rect, dpr) = pressed_rect(&answer.stdout)?;
            Some(json!({ "space": "css", "rect": rect, "dpr": dpr, "label": argv.get(2) }))
        }
        // The emulator answers device pixels when it names a pressed rect:
        // its picture is the device, so the report rings it 1:1 (`space: px`).
        RecipeTool::Emulator => {
            let rect = result
                .and_then(|result| result.get("rect"))
                .and_then(rect_of)?;
            Some(json!({ "space": "px", "rect": rect }))
        }
        RecipeTool::Computer => {
            let command = command.filter(|command| command.method.acts())?;
            let mark = result.and_then(|result| result.get("mark"));
            let rect = mark
                .and_then(|mark| mark.get("frame"))
                .and_then(rect_of)
                .or(landing)?;
            let label = mark
                .and_then(|mark| mark.get("label"))
                .and_then(Value::as_str)
                .or_else(|| {
                    ["text", "label"]
                        .into_iter()
                        .find_map(|key| command.params.get(key).and_then(Value::as_str))
                });
            let number = mark
                .and_then(|mark| mark.get("mark"))
                .and_then(Value::as_u64);
            Some(json!({ "space": "points", "rect": rect, "label": label, "mark": number }))
        }
    }
}

/// The marks a look answered, as the report draws them: each mark's number,
/// its label and its rectangle as the look spoke it — kept from the step
/// before the act, never walked again.
fn marks_of(result: &Value) -> Option<Value> {
    let items = result.get(ITEMS_KEY)?.as_array()?;
    let marks: Vec<Value> = items
        .iter()
        .filter_map(|item| {
            Some(json!({
                "mark": item.get("mark")?.as_u64()?,
                "label": item.get("label"),
                "element_px": rect_of(item)?,
            }))
        })
        .collect();
    Some(Value::Array(marks))
}

/// A check's command line asked with `budget_ms`: a desktop check's hold
/// flag set to it (the table's flag for its verb), a browser `wait`'s
/// timeout word set to it within the door's bounds, a `find` as it is.
fn check_argv(check: &Check, budget_ms: u64) -> Vec<String> {
    let mut argv = check.argv.clone();
    match check.tool {
        RecipeTool::Computer => {
            if let Some(flag) = argv
                .first()
                .and_then(|verb| verb_method(verb))
                .and_then(hold_of)
                .and_then(|hold| hold.asked_by)
                .map(|asked_by| format!("--{}", asked_by.flag))
            {
                if let Some(at) = argv.iter().position(|word| *word == flag) {
                    argv.drain(at..(at + 2).min(argv.len()));
                }
                argv.extend([flag, budget_ms.to_string()]);
            }
            // A check frames nothing (`captures`): sent as an unframed step.
            recipe_step_argv(&argv, false)
        }
        RecipeTool::Browser => {
            if argv.first().is_some_and(|verb| verb == "wait") {
                let bounded = budget_ms.clamp(
                    agent_browser::BROWSER_WAIT_MIN_MS,
                    agent_browser::BROWSER_WAIT_MAX_MS,
                );
                argv.truncate(3);
                argv.push(bounded.to_string());
            }
            argv
        }
        // Mobile checks use the same JSON envelope as every emulator step.
        RecipeTool::Emulator => recipe_line_argv(RecipeTool::Emulator, &argv, false),
    }
}

/// What one asked check came to: a desktop check answers ok when its
/// subject is there and `timeout` when not; a browser `wait` likewise by
/// exit code and its named refusal; a `find` answers how many. Any other
/// refusal is no observation.
fn presence_of(check: &Check, answer: &TeamAnswer) -> Observed {
    let seen = |present: bool, count: Option<usize>| Observed::Seen(Presence { present, count });
    match check.tool {
        RecipeTool::Computer => {
            let envelope = answer_envelope(&answer.stdout, &answer.stderr);
            let code = envelope
                .as_ref()
                .and_then(|envelope| envelope.pointer("/error/code"))
                .and_then(Value::as_str);
            if answer.exit_code == 0
                && envelope
                    .as_ref()
                    .and_then(|envelope| envelope.get("ok"))
                    .and_then(Value::as_bool)
                    == Some(true)
            {
                seen(true, None)
            } else if code == Some(error_code::TIMEOUT) {
                seen(false, None)
            } else {
                Observed::NotEvaluable(
                    envelope
                        .as_ref()
                        .and_then(|envelope| envelope.pointer("/error/message"))
                        .and_then(Value::as_str)
                        .unwrap_or("the check answered nothing")
                        .to_string(),
                )
            }
        }
        RecipeTool::Browser => {
            let verb = check.argv.first().map_or("", String::as_str);
            let refused = answer.stderr.trim();
            match (verb, answer.exit_code == 0) {
                ("wait", true) => seen(true, None),
                ("wait", false) if refused.contains(WAIT_TIMED_OUT) => seen(false, None),
                ("find", true) => serde_json::from_str::<Value>(answer.stdout.trim())
                    .ok()
                    .and_then(|found| found.get("count")?.as_u64())
                    .and_then(|count| usize::try_from(count).ok())
                    .map_or_else(
                        || Observed::NotEvaluable("find answered no count".to_string()),
                        |count| seen(count > 0, Some(count)),
                    ),
                _ => Observed::NotEvaluable(if refused.is_empty() {
                    "the check answered nothing".to_string()
                } else {
                    refused.to_string()
                }),
            }
        }
        RecipeTool::Emulator => {
            let envelope = answer_envelope(&answer.stdout, &answer.stderr);
            let count = envelope
                .as_ref()
                .filter(|value| {
                    answer.exit_code == 0 && value.get("ok").and_then(Value::as_bool) == Some(true)
                })
                .and_then(|value| value.pointer("/result/count"))
                .and_then(Value::as_u64)
                .and_then(|count| usize::try_from(count).ok());
            count.map_or_else(
                || {
                    Observed::NotEvaluable(
                        envelope
                            .as_ref()
                            .and_then(|value| value.pointer("/error/message"))
                            .and_then(Value::as_str)
                            .unwrap_or("the mobile check answered no count")
                            .to_string(),
                    )
                },
                |count| seen(count > 0, Some(count)),
            )
        }
    }
}

/// The checks asked once each — the one walk, each line held by its door's
/// table with `budget_ms` — and what each came to. A check the walk could
/// not begin (out of time, the caller gone) is not evaluable.
fn check_walk<'c>(
    checks: impl Iterator<Item = &'c Check>,
    budget_ms: u64,
    deadline_ms: u64,
    step: &mut impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
    desk: &RefCell<impl Desk>,
    caller_waits: &impl Fn() -> bool,
) -> BTreeMap<usize, Observed> {
    let checks: Vec<&Check> = checks.collect();
    let mut observed = BTreeMap::new();
    let walked = walk(
        &checks,
        &WalkBudget {
            deadline_ms,
            out_of_time: RecipeStop::Budget,
        },
        |check: &&Check, argv| recipe_line_holds_ms(check.tool, argv),
        |_, check| Next::Run(check_argv(check, budget_ms)),
        |n, check, argv| {
            let answer =
                crate::run_evidence::observing(json!({"judgment": {"asked": false}}), || {
                    step(check.tool, argv, argv)
                });
            observed.insert(check.id, presence_of(check, &answer));
            (json!({ "n": n }), None)
        },
        caller_waits,
        || desk.borrow().elapsed_ms(),
    );
    if let Some((n, _)) = walked.halted {
        for check in &checks[n - 1..] {
            observed.entry(check.id).or_insert_with(|| {
                Observed::NotEvaluable("the run had no time left to ask".to_string())
            });
        }
    }
    observed
}

/// One check line asked once, past the walk's own two askings — the seam a
/// repeat's trigger takes (`repeat`): the same road, the same table and the
/// same reading as the baseline probe and the oracle, with `budget_ms` as
/// its wait, on the caller's clock (`desk`) against the call's deadline. A
/// trigger's look is nobody's step, so it names no evidence line.
pub fn ask(
    check: &Check,
    budget_ms: u64,
    deadline_ms: u64,
    step: &mut impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
    desk: &RefCell<impl Desk>,
    caller_waits: &impl Fn() -> bool,
) -> Observed {
    check_walk(
        std::iter::once(check),
        budget_ms,
        deadline_ms,
        step,
        desk,
        caller_waits,
    )
    .remove(&check.id)
    .unwrap_or_else(|| Observed::NotEvaluable("the check was not asked".to_string()))
}

/// The walk over the decided lines. Each step runs down its door's road,
/// then its landing is judged — a press by the pixels about its point, a
/// typed text by the helper's read-back — and the person's hand: the pointer
/// is read before the walk and after every step, and must be where the step
/// left it (the point it names, where an app's click put it, or where it
/// was). Under a Flow's fingerprint (`terms.bound`) a browser act aimed
/// outside the recorded hosts is refused before it is sent, and a desktop
/// act that answered from an app outside the recording stops the walk after
/// it; each app act is sent with its picture or without by whether the
/// walk's steps are framed (`terms.framed`); and the Flow's money gate
/// (`terms.money`) stands before its money step.
fn walk_lines(
    planned: &[(RecipeLine, Plan)],
    deadline_ms: u64,
    step: &mut impl FnMut(RecipeTool, &[String], &[String]) -> TeamAnswer,
    desk: &RefCell<impl Desk>,
    caller_waits: &impl Fn() -> bool,
    terms: Terms<'_>,
) -> (Walked<RecipeStop>, bool) {
    let Terms {
        bound,
        framed,
        money: gate,
        retry,
    } = terms;
    let expected: RefCell<Option<(f64, f64)>> = RefCell::new(desk.borrow_mut().pointer());
    let watched = expected.borrow().is_some();
    // The marks the step before answered, for the act that follows it.
    let mut marks: Option<Value> = None;
    let elapsed = || desk.borrow().elapsed_ms();
    let walked = walk(
        planned,
        &WalkBudget {
            deadline_ms,
            out_of_time: RecipeStop::Budget,
        },
        |(line, _), argv| recipe_line_holds_ms(line.tool, argv),
        |_, (line, plan)| match plan {
            Plan::Run(argv) => Next::Run(recipe_line_argv(line.tool, argv, framed)),
            Plan::Skip(why) => Next::Skip(why),
            Plan::Stop(kind) => Next::Halt(*kind),
        },
        |n, (line, _), argv| {
            let tool = line.tool;
            let verb = argv.first().map_or("", String::as_str);
            // A browser verb is never a desktop method, whatever it is called.
            let command = (tool == RecipeTool::Computer)
                .then(|| parse_command(argv).ok())
                .flatten();
            let method = command.as_ref().map(|command| command.method);
            let check = method.is_some_and(|method| RECIPE_CHECKS.contains(&method));
            let point = command.as_ref().and_then(named_point);
            let region = point
                .filter(|_| method == Some(ComputerMethod::MouseClick))
                .map(|(x, y)| recipe_landing_box(x, y));
            let mut phases = Phases::default();
            let finish = |mut report: Value, phases: Phases| {
                report["tool"] = json!(tool.as_str());
                report["phases"] = phases.value();
                report
            };
            if let Some(print) = bound
                && tool == RecipeTool::Browser
                && agent_browser::acts(verb)
                && let Some(why) = browser_act_outside(print, argv, &mut *desk.borrow_mut())
            {
                let report = refused_report(n, argv, error_code::FLOW_ENV_MISMATCH, why);
                return (finish(report, phases), Some(RecipeStop::StepFailed));
            }
            // The money gate (design §2–§4), at the money step alone: what it
            // spent is the step's `gate` ms, beside its four phases.
            let mut gate_ms = None;
            if let Some(gate) = gate.filter(|_| line.money) {
                let gating = elapsed();
                let at_epoch_ms = desk.borrow().now_epoch_ms();
                let passed = gate.before(
                    line,
                    |witnesses| {
                        check_walk(
                            witnesses.iter(),
                            FLOW_BASELINE_PROBE_MS,
                            deadline_ms,
                            step,
                            desk,
                            caller_waits,
                        )
                    },
                    at_epoch_ms,
                );
                gate_ms = Some(elapsed().saturating_sub(gating));
                if let Err((code, message, kind)) = passed {
                    let mut report = refused_report(n, argv, code, message);
                    report["gate"] = json!(gate_ms);
                    return (finish(report, phases), Some(kind));
                }
            }
            let seed = region.and_then(|region| desk.borrow_mut().picture(region));
            let acting = elapsed();
            // This token travels with the one observation the road hands to
            // the writer. A lost append loses its mapping, not the next line's.
            let evidence_id = uuid::Uuid::new_v4().to_string();
            let mut observed = json!({"judgment": {"asked": false}, "evidence_id": evidence_id});
            if !(check || tool == RecipeTool::Browser && agent_browser::is_check(verb))
                && let Some((name, cwd)) = retry
            {
                observed["retry"] = json!({"name": name, "step": line.step, "cwd": cwd});
            }
            let answer = crate::run_evidence::observing(observed, || {
                step(tool, argv, &recipe_line_argv(tool, &line.argv, framed))
            });
            phases.act = elapsed().saturating_sub(acting);
            let envelope = match tool {
                // The emulator door, asked with `--json`, answers the same
                // `{ok,result}` envelope the desktop does.
                RecipeTool::Computer | RecipeTool::Emulator => {
                    answer_envelope(&answer.stdout, &answer.stderr)
                }
                RecipeTool::Browser => Some(browser_envelope(&answer)),
            };
            let mut report = walk_step_report(
                n,
                argv,
                answer.exit_code == 0,
                envelope.as_ref(),
                &answer.stderr,
            );
            // The walk's answer is where it stopped; a step's result is the
            // lone command's to show.
            let result = report
                .as_object_mut()
                .and_then(|report| report.remove("result"));
            report["evidence_id"] = json!(evidence_id);
            if let Some(gate_ms) = gate_ms {
                report["gate"] = json!(gate_ms);
            }
            if check {
                report["check"] = Value::Bool(true);
            }
            if let Some(shown) = marks.take() {
                report["marks"] = shown;
            }
            marks = result.as_ref().and_then(marks_of);
            if let Some(target) = target_of(
                tool,
                argv,
                command.as_ref(),
                result.as_ref(),
                &answer,
                region,
            ) {
                report["target"] = target;
            }
            if report["ok"] != Value::Bool(true) {
                let code = report
                    .pointer("/error/code")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let kind = stop_for(tool, code, check);
                return (finish(report, phases), Some(kind));
            }
            // A desktop or emulator act that answered from an app outside the
            // recording stops the walk after it — bound act by act, the app
            // read from the answer the act already gave (D8, the same path).
            let acts_on_app = method.is_some_and(ComputerMethod::acts)
                || (tool == RecipeTool::Emulator && agent_emulator::acts(verb));
            if let Some(print) = bound
                && acts_on_app
                && let Some(app) = app_outside(print, result.as_ref())
            {
                report["error"] = json!({
                    "code": error_code::FLOW_ENV_MISMATCH,
                    "message": format!("the act answered from {app}, an app the recording never saw"),
                });
                return (finish(report, phases), Some(RecipeStop::NeedsALook));
            }
            let landed = match (seed, region) {
                // Pixels about the pressed point changed within the settle.
                (Some((before, frame)), Some(region)) => {
                    let began = elapsed();
                    let landed = look_until_settled(
                        || desk.borrow_mut().picture(region).ok_or(()),
                        |(now, _)| desk.borrow().changed(&before, now, frame),
                        || Duration::from_millis(elapsed().saturating_sub(began)),
                        |pause| desk.borrow_mut().pause(pause),
                    )
                    .ok()
                    .and_then(|(now, _)| desk.borrow().changed(&before, &now, frame));
                    phases.settle = elapsed().saturating_sub(began);
                    landed
                }
                // The helper read the field back: what it says, it saw — and
                // a field that still reads what it read before the keys is one
                // they never reached.
                _ => {
                    let began = elapsed();
                    let landed = result
                        .as_ref()
                        .and_then(|result| {
                            result
                                .get("verification")
                                .or_else(|| result.pointer("/action/verification"))
                        })
                        .cloned()
                        .and_then(|said| serde_json::from_value::<Verification>(said).ok())
                        .and_then(|said| match said {
                            Verification::Verified { .. } => Some(true),
                            Verification::Unverified { reason, .. }
                                if reason == unverified_reason::VALUE_UNCHANGED =>
                            {
                                Some(false)
                            }
                            Verification::Unverified { .. } => None,
                        });
                    phases.verify = elapsed().saturating_sub(began);
                    landed
                }
            };
            report["landed"] = landed.map_or(Value::Null, Value::Bool);
            let was = *expected.borrow();
            if let Some(was) = was {
                let began = elapsed();
                let now = desk.borrow_mut().pointer();
                phases.pointer = elapsed().saturating_sub(began);
                let should = if method.is_some_and(ComputerMethod::moves_the_pointer) {
                    point
                } else if method.is_some_and(ComputerMethod::moves_the_pointer_unnamed) {
                    now
                } else {
                    Some(was)
                };
                if let (Some((now_x, now_y)), Some((x, y))) = (now, should)
                    && (now_x - x).hypot(now_y - y) > RECIPE_CURSOR_SLOP_POINTS
                {
                    return (finish(report, phases), Some(RecipeStop::PersonMoved));
                }
                *expected.borrow_mut() = now.or(should);
            }
            (
                finish(report, phases),
                (landed == Some(false)).then_some(RecipeStop::NothingChanged),
            )
        },
        caller_waits,
        elapsed,
    );
    (walked, watched)
}

/// The walk's answer: every step it ran or skipped (by its position and the
/// number a person reads), where it stopped and why, and the step to run
/// again from.
fn report(
    command: &ComputerCommand,
    file: &str,
    lines: &[RecipeLine],
    start: usize,
    decided: &zerocode_core::computer_recipe::Preflight,
    walked: &Walked<RecipeStop>,
    deadline_ms: u64,
) -> Value {
    let line_of = |n: usize| decided.planned.get(n - 1).map(|(line, _)| line);
    let steps: Vec<Value> = walked
        .reports
        .iter()
        .map(|report| {
            let mut report = report.clone();
            if let Some(line) = report["n"]
                .as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .and_then(line_of)
            {
                report["step"] = json!(line.step);
                report["shown"] = json!(line.shown);
                report["tool"] = json!(line.tool.as_str());
            }
            if let Some(object) = report.as_object_mut() {
                object.remove("n");
            }
            report
        })
        .collect();
    let (stopped_at, stop) = match &walked.halted {
        Some((n, kind)) => {
            let line = line_of(*n);
            let error = walked
                .reports
                .iter()
                .find(|report| report["n"] == json!(n))
                .and_then(|report| report.get("error"))
                .cloned();
            (
                line.map(|line| line.step),
                Some(json!({
                    "kind": kind.as_str(),
                    "shown": line.map(|line| line.shown),
                    "code": error.as_ref().and_then(|error| error.get("code")).cloned(),
                    "message": error.as_ref().and_then(|error| error.get("message")).cloned(),
                    "advice": kind.advice(),
                })),
            )
        }
        None => (None, None),
    };
    let next = match (&walked.halted, stopped_at) {
        (Some((_, kind)), Some(at)) => {
            let next = at + usize::from(kind.resumes_after());
            (next <= lines.len()).then_some(next)
        }
        _ => None,
    };
    json!({
        "name": command.params.get("name").cloned().unwrap_or(Value::Null),
        "file": file,
        "steps": lines.len(),
        "start": start,
        "ran": steps,
        "done": walked.finished(),
        "complete": walked.finished() && requested_start(command) == 1
            && command.params.get("end").and_then(Value::as_u64)
                .is_none_or(|end| usize::try_from(end).ok() == Some(lines.len())),
        "stoppedAt": stopped_at,
        "stop": stop,
        "next": next,
        "elapsedMs": walked.elapsed_ms,
        "budgetMs": walk_budget_ms(deadline_ms),
        "unusedParams": decided.unused,
        "laterParams": decided.later,
    })
}

/// The report as a person reads it in a terminal.
#[must_use]
pub fn text(report: &Value) -> String {
    let mut out: Vec<String> = report["ran"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| {
            let (at, verb) = (&step["shown"], step["verb"].as_str().unwrap_or_default());
            match step.get("skipped").and_then(Value::as_str) {
                Some(why) => format!("{at}. skipped ({why})"),
                None if step["ok"] == Value::Bool(true) => {
                    format!("{at}. {verb} ok {} ms", step["ms"])
                }
                None => format!(
                    "{at}. {verb} refused {}",
                    step.pointer("/error/code")
                        .and_then(Value::as_str)
                        .unwrap_or(error_code::UNANSWERED)
                ),
            }
        })
        .collect();
    if let Some((pass, reason)) = verdict_of(report) {
        out.push(format!(
            "flow verdict: {}{}",
            if pass { "pass" } else { "fail" },
            reason.map(|why| format!(" — {why}")).unwrap_or_default()
        ));
    }
    if let Some(money) = report["money"].as_object() {
        out.push(format!(
            "money: transaction {} {} (ledger {})",
            money["txn"].as_str().unwrap_or_default(),
            money["state"].as_str().unwrap_or_default(),
            money["ledger"].as_str().unwrap_or_default()
        ));
    }
    match report["stop"].as_object() {
        Some(stop) => out.push(format!(
            "stopped at step {}: {}{} — {} (next: {})",
            report["stoppedAt"],
            stop["kind"].as_str().unwrap_or_default(),
            stop["message"]
                .as_str()
                .map(|said| format!(" — {said}"))
                .unwrap_or_default(),
            stop["advice"].as_str().unwrap_or_default(),
            report["next"]
        )),
        None => out.push(format!(
            "done: {} steps in {} ms",
            report["steps"], report["elapsedMs"]
        )),
    }
    out.join("\n")
}

/// The fake desk the walk's tests read, and the documents they walk —
/// shared with the recipes' tests, which replay a saved document through
/// the same walk.
#[cfg(test)]
pub(crate) mod bench {
    use super::*;
    use std::cell::{Cell, RefCell};
    use zerocode_core::computer_flow::FlowSpec;
    use zerocode_core::computer_recipe::{RECIPE_HEADING_STEPS, RecipeTool};
    use zerocode_core::computer_use::COMPUTER_USE_DEADLINE_SECONDS;

    pub(crate) const DEADLINE: u64 = COMPUTER_USE_DEADLINE_SECONDS * 1_000;
    /// The fake clock's epoch: what `now_epoch_ms` answers at clock zero.
    const EPOCH_MS: i64 = 1_000;

    /// A desk that answers what the test says, and counts what was asked. The
    /// pointer is a cell the runner may move, as a step or a person would;
    /// the pages are the window's browser panes as the test sets them.
    pub(crate) struct FakeDesk<'a> {
        pub(crate) pointer: &'a Cell<Option<(f64, f64)>>,
        pub(crate) picture: bool,
        pub(crate) changed: Option<bool>,
        pub(crate) clock: &'a Cell<u64>,
        pub(crate) pictures: &'a Cell<usize>,
        pub(crate) regions: &'a RefCell<Vec<[f64; 4]>>,
        pub(crate) pages: &'a RefCell<Vec<(String, String)>>,
    }

    impl Desk for FakeDesk<'_> {
        fn pointer(&mut self) -> Option<(f64, f64)> {
            self.pointer.get()
        }
        fn picture(&mut self, region: [f64; 4]) -> Option<(Vec<u8>, ShotFrame)> {
            self.pictures.set(self.pictures.get() + 1);
            self.regions.borrow_mut().push(region);
            self.picture.then(|| (vec![0], ShotFrame::UNIT))
        }
        fn changed(&self, _: &[u8], _: &[u8], _: ShotFrame) -> Option<bool> {
            self.changed
        }
        fn elapsed_ms(&self) -> u64 {
            self.clock.get()
        }
        fn now_epoch_ms(&self) -> i64 {
            EPOCH_MS + i64::try_from(self.clock.get()).unwrap_or(i64::MAX)
        }
        fn pause(&mut self, pause: Duration) {
            self.clock
                .set(self.clock.get() + u64::try_from(pause.as_millis()).unwrap());
        }
        fn pages(&mut self) -> Vec<(String, String)> {
            self.pages.borrow().clone()
        }
    }

    pub(crate) struct Bench {
        pub(crate) pointer: Cell<Option<(f64, f64)>>,
        pub(crate) clock: Cell<u64>,
        pub(crate) pictures: Cell<usize>,
        pub(crate) regions: RefCell<Vec<[f64; 4]>>,
        pub(crate) pages: RefCell<Vec<(String, String)>>,
    }

    impl Bench {
        pub(crate) fn new() -> Self {
            Self::at(None)
        }
        pub(crate) fn at(pointer: Option<(f64, f64)>) -> Self {
            Self {
                pointer: Cell::new(pointer),
                clock: Cell::new(0),
                pictures: Cell::new(0),
                regions: RefCell::new(Vec::new()),
                pages: RefCell::new(Vec::new()),
            }
        }
        /// The browser panes the window shows, `(label, url)`.
        pub(crate) fn showing(self, pages: &[(&str, &str)]) -> Self {
            self.show(pages);
            self
        }
        pub(crate) fn show(&self, pages: &[(&str, &str)]) {
            *self.pages.borrow_mut() = pages
                .iter()
                .map(|(label, url)| ((*label).to_string(), (*url).to_string()))
                .collect();
        }
        pub(crate) fn desk(&self, picture: bool, changed: Option<bool>) -> FakeDesk<'_> {
            FakeDesk {
                pointer: &self.pointer,
                picture,
                changed,
                clock: &self.clock,
                pictures: &self.pictures,
                regions: &self.regions,
                pages: &self.pages,
            }
        }
    }

    /// A recipe of desktop lines.
    pub(crate) fn doc(lines: &[&str]) -> String {
        let lines: Vec<(RecipeTool, &str)> = lines
            .iter()
            .map(|line| (RecipeTool::Computer, *line))
            .collect();
        mixed_doc(&lines)
    }

    /// A recipe whose lines name their door.
    pub(crate) fn mixed_doc(lines: &[(RecipeTool, &str)]) -> String {
        let steps: String = lines
            .iter()
            .enumerate()
            .map(|(at, (tool, line))| format!("{}. `{} {line}`\n", at + 1, tool.command_word()))
            .collect();
        format!("# Test\n\n{RECIPE_HEADING_STEPS}\n\n{steps}")
    }

    /// A Flow: the recipe's lines and the two sections its spec writes.
    pub(crate) fn flow_doc(lines: &[(RecipeTool, &str)], spec: &FlowSpec) -> String {
        format!("{}\n{}", mixed_doc(lines), spec.written())
    }

    pub(crate) fn command(extra: &[&str]) -> ComputerCommand {
        let mut argv = vec!["recipe-run".to_string(), "--name".into(), "test".into()];
        argv.extend(extra.iter().map(|word| (*word).to_string()));
        parse_command(&argv).expect("a recipe run")
    }

    /// The walk of `text` from `command`, with no evidence folder.
    pub(crate) fn walk_of<'a>(command: &'a ComputerCommand, text: &'a str) -> Run<'a> {
        Run {
            command,
            resume_from: None,
            file: "f.md",
            text,
            deadline_ms: DEADLINE,
            cwd: None,
            evidence_dir: None,
        }
    }

    pub(crate) fn said(result: &Value) -> zerocode_hookd::TeamAnswer {
        zerocode_hookd::TeamAnswer {
            stdout: format!("{}\n", json!({ "ok": true, "result": result })),
            stderr: String::new(),
            exit_code: 0,
        }
    }

    pub(crate) fn ok() -> zerocode_hookd::TeamAnswer {
        said(&json!({}))
    }

    pub(crate) fn refused(code: &str) -> zerocode_hookd::TeamAnswer {
        zerocode_hookd::TeamAnswer {
            stdout: String::new(),
            stderr: format!(
                "{}\n",
                json!({ "ok": false, "error": { "code": code, "message": "no" } })
            ),
            exit_code: 1,
        }
    }

    /// The browser door's answer: a sentence, no envelope.
    pub(crate) fn browser_said(text: &str) -> zerocode_hookd::TeamAnswer {
        zerocode_hookd::TeamAnswer {
            stdout: format!("{text}\n"),
            stderr: String::new(),
            exit_code: 0,
        }
    }

    pub(crate) fn browser_refused(why: &str) -> zerocode_hookd::TeamAnswer {
        zerocode_hookd::TeamAnswer {
            stdout: String::new(),
            stderr: format!("zerocode-browser: {why}\n"),
            exit_code: 1,
        }
    }

    pub(crate) fn words(list: &[&str]) -> Vec<String> {
        list.iter().map(|word| (*word).to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::bench::*;
    use super::*;
    use crate::computer_use::guarded::{By, LEDGER_EXTENSION, Line, State};
    use std::cell::Cell;
    use std::collections::{BTreeMap, BTreeSet};
    use zerocode_core::computer_flow::{
        Check, CheckKind, Confirm, EvidenceLevel, Fingerprint, FlowSpec, Money, Policy,
    };
    use zerocode_core::computer_recipe::{
        RECIPE_HEADING_STEPS, RECIPE_MARK_MONEY, RECIPE_MARK_SEPARATOR, RecipeTool,
    };
    use zerocode_core::computer_use::{
        COMPUTER_USE_PROTOCOL_VERSION, FLOW_BASELINE_PROBE_MS, FLOW_VERIFY_MS, WALKED_STEP_FLAGS,
        WALKED_STEP_UNFRAMED_FLAGS,
    };

    /// The walk as the older tests call it: desktop lines, one road, no
    /// evidence folder, no Flow.
    fn run_lines(
        command: &ComputerCommand,
        (file, text): (&str, &str),
        deadline_ms: u64,
        mut step: impl FnMut(&[String], &[String]) -> zerocode_hookd::TeamAnswer,
        leave: impl FnOnce(&[String], &[String], &zerocode_hookd::TeamAnswer),
        desk: impl Desk,
        caller_waits: impl Fn() -> bool,
    ) -> Result<Value, Refused> {
        run(
            &Run {
                command,
                resume_from: None,
                file,
                text,
                deadline_ms,
                cwd: None,
                evidence_dir: None,
            },
            |_, argv, logged| step(argv, logged),
            leave,
            |_| {},
            desk,
            caller_waits,
        )
    }

    /// The hermetic half of the exit number: an N-step recipe with no stop is
    /// one call, each step down the runner once, filled and answering an
    /// envelope — one report — and each logged in the recipe's own words.
    #[test]
    fn a_recipe_walks_every_step_through_the_one_runner_in_one_call() {
        let bench = Bench::new();
        let text = doc(&[
            "activate --app TextEdit",
            "key --key cmd+n",
            "type --text {{text-3}}",
            "wait-for --app TextEdit --text {{text-3}} --timeout-ms 2000",
            "key --key cmd+a",
            "type --text {{text-6}}",
            "key --key cmd+s",
            "wait --ms 10",
        ]);
        let mut ran: Vec<Vec<String>> = Vec::new();
        let mut logged: Vec<Vec<String>> = Vec::new();
        let report = run_lines(
            &command(&["--params", r#"{"text-3":"hello","text-6":"bye"}"#]),
            ("f.md", &text),
            DEADLINE,
            |argv, line| {
                ran.push(argv.to_vec());
                logged.push(line.to_vec());
                if argv[0] == "type" {
                    said(&json!({ "verification": { "state": "verified", "property": "value" } }))
                } else {
                    ok()
                }
            },
            |_, _, _| {},
            bench.desk(true, Some(true)),
            || true,
        )
        .expect("walked");
        assert_eq!(ran.len(), 8, "every step, once");
        assert!(
            ran.iter()
                .all(|argv| argv.last().is_some_and(|word| word == "--json"))
        );
        assert_eq!(ran[2][..3], ["type", "--text", "hello"]);
        assert_eq!(
            ran[3][4], "hello",
            "a check looks for the value it was given"
        );
        assert_eq!(
            (logged[2][2].as_str(), logged[3][4].as_str()),
            ("{{text-3}}", "{{text-3}}"),
            "the log keeps the recipe's words, never a value it was given"
        );
        assert_eq!(
            (report["done"].clone(), report["next"].clone()),
            (json!(true), Value::Null)
        );
        assert_eq!(report["ran"].as_array().map(Vec::len), Some(8));
        assert_eq!(
            report["ran"][2]["landed"], true,
            "a typed text read back from its field"
        );
        assert_eq!(
            bench.pictures.get(),
            0,
            "a typed text is not looked for in pixels"
        );
        assert_eq!(report["ran"][3]["check"], true);
        assert!(
            report["ran"][0].get("result").is_none(),
            "a step's result is the lone command's"
        );
        assert!(text_mentions(&report, "done: 8 steps"));
    }

    fn text_mentions(report: &Value, words: &str) -> bool {
        text(report).contains(words)
    }

    #[test]
    fn a_recipe_stops_before_the_persons_turn_and_resumes_after_it() {
        let bench = Bench::new();
        let text = doc(&[
            "key --key tab",
            "key --key tab",
            "handoff --reason 2FA",
            "type --text {{code}}",
        ]);
        let mut ran = 0;
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| {
                ran += 1;
                ok()
            },
            |_, _, _| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked up to the person's turn, though a later value is missing");
        assert_eq!(ran, 2, "the runner never sees the handoff");
        assert_eq!(report["stoppedAt"], 3);
        assert_eq!(report["stop"]["kind"], "persons_turn");
        assert_eq!(report["next"], 4);
        assert_eq!(report["laterParams"], json!(["code"]));
        let mut typed = Vec::new();
        let resumed = run_lines(
            &command(&["--start", "4", "--params", r#"{"code":"123456"}"#]),
            ("f.md", &text),
            DEADLINE,
            |argv, _| {
                typed.push(argv[2].clone());
                ok()
            },
            |_, _, _| {},
            bench.desk(false, None),
            || true,
        )
        .expect("resumed");
        assert_eq!(typed, ["123456"]);
        assert_eq!(resumed["done"], true);
    }

    /// A walk that stops at the person's step leaves that line — the person
    /// may do it by hand, and a recipe saved from the log keeps it; a stop for
    /// a fresh look leaves nothing (the look's own command will).
    #[test]
    fn a_walk_stopped_at_the_persons_step_leaves_that_line() {
        for (line, left) in [
            ("handoff --reason 2FA", Some("persons_turn")),
            (
                "mouse-click --x 1 --y 1 --confirming payment",
                Some("persons_last_step"),
            ),
            ("click --app X --element-index 3", None),
        ] {
            let bench = Bench::new();
            let mut seen = Vec::new();
            run_lines(
                &command(&[]),
                ("f.md", &doc(&["key --key tab", line])),
                DEADLINE,
                |_, _| ok(),
                |argv, logged, answer| {
                    assert_eq!(argv, logged, "the line as the recipe says it");
                    let said = zerocode_core::computer_use_protocol::answer_envelope(
                        &answer.stdout,
                        &answer.stderr,
                    )
                    .expect("an envelope");
                    assert_eq!(said["error"]["code"], error_code::RECIPE_STOPPED);
                    seen.push((argv[0].clone(), said["error"]["message"].clone()));
                },
                bench.desk(false, None),
                || true,
            )
            .expect("walked");
            assert_eq!(
                seen.first().and_then(|(_, kind)| kind.as_str()),
                left,
                "{line}"
            );
            assert!(seen.len() <= 1);
        }
    }

    #[test]
    fn a_failed_check_a_refused_step_and_a_held_press_stop_the_walk_at_once() {
        for (code, line, kind, next) in [
            (
                "timeout",
                "wait-for --app X --text Done --timeout-ms 100",
                "check_failed",
                2,
            ),
            (
                "element_not_found",
                "click --app X --x 1 --y 1",
                "step_failed",
                2,
            ),
            (
                "confirmation_required",
                "mouse-click --x 1 --y 1",
                "persons_last_step",
                3,
            ),
            ("secure_input", "type --text hunter2", "persons_turn", 3),
            ("stopped", "key --key a", "stopped", 2),
            ("budget_exceeded", "key --key a", "stopped", 2),
            ("person_asked", "key --key a", "stopped", 2),
        ] {
            let bench = Bench::new();
            let text = doc(&["key --key tab", line, "key --key tab"]);
            let mut ran = 0;
            let report = run_lines(
                &command(&[]),
                ("f.md", &text),
                DEADLINE,
                |argv, _| {
                    ran += 1;
                    if argv[0] == "key" && argv[2] == "tab" {
                        ok()
                    } else {
                        refused(code)
                    }
                },
                |_, _, _| {},
                bench.desk(false, None),
                || true,
            )
            .expect("walked");
            assert_eq!(ran, 2, "{code}: no step after the stop runs");
            assert_eq!(report["stop"]["kind"], kind, "{code}");
            assert_eq!(report["stop"]["code"], code);
            assert_eq!(report["next"], next, "{code}");
        }
    }

    /// An ok answer is not a finished step: a program the table stopped, or
    /// an app that showed no window, stops the walk where it happened.
    #[test]
    fn a_step_whose_work_did_not_happen_stops_the_walk() {
        for (line, result) in [
            (
                "run --program /bin/sh --timeout-ms 100",
                json!({ "spawned": true, "exitCode": null, "timedOut": true }),
            ),
            ("launch --app Mail", json!({ "ready": false, "windows": 0 })),
            ("quit --app TextEdit", json!({ "terminated": false })),
        ] {
            let bench = Bench::new();
            let mut ran = 0;
            let report = run_lines(
                &command(&[]),
                ("f.md", &doc(&[line, "type --text hi"])),
                DEADLINE,
                |_, _| {
                    ran += 1;
                    said(&result)
                },
                |_, _, _| {},
                bench.desk(false, None),
                || true,
            )
            .expect("walked");
            assert_eq!(ran, 1, "{line}: the next step never runs on it");
            assert_eq!(report["stop"]["kind"], "step_failed", "{line}");
            assert_eq!(report["stop"]["code"], error_code::UNFINISHED, "{line}");
            assert_eq!(report["next"], 1, "{line}");
        }
    }

    #[test]
    fn an_act_that_changed_nothing_about_its_point_stops_the_walk_and_an_unseen_screen_does_not() {
        let bench = Bench::at(Some((5.0, 5.0)));
        let text = doc(&["mouse-click --x 5 --y 5", "key --key tab"]);
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| ok(),
            |_, _, _| {},
            bench.desk(true, Some(false)),
            || true,
        )
        .expect("walked");
        assert_eq!(report["stop"]["kind"], "nothing_changed");
        assert_eq!(
            report["next"], 2,
            "the click ran: fix it by hand, then go on"
        );
        assert!(
            bench.pictures.get() > 2,
            "a mark is looked for until the settle: {}",
            bench.pictures.get()
        );
        assert!(
            bench
                .regions
                .borrow()
                .iter()
                .all(|region| *region == recipe_landing_box(5.0, 5.0)),
            "only the box about the pressed point is looked at"
        );

        let bench = Bench::new();
        let report = run_lines(
            &command(&[]),
            ("f.md", &doc(&["key --key tab", "key --key tab"])),
            DEADLINE,
            |_, _| ok(),
            |_, _, _| {},
            bench.desk(true, Some(false)),
            || true,
        )
        .expect("walked");
        assert_eq!(bench.pictures.get(), 0, "a key's effect is not looked for");
        assert_eq!(report["done"], true);

        let bench = Bench::at(Some((5.0, 5.0)));
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| ok(),
            |_, _, _| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            report["done"], true,
            "a screen the walk cannot see does not stop it"
        );
        assert_eq!(report["ran"][0]["landed"], Value::Null);
    }

    /// A typed text the field still does not show — the keys never reached
    /// it — stops the walk after the typing, as a press that changed nothing
    /// does; a read-back the helper could not make does not.
    #[test]
    fn a_typed_text_the_field_never_showed_stops_the_walk() {
        for (reason, kind) in [
            ("value_unchanged", json!("nothing_changed")),
            ("readback_unsupported", Value::Null),
        ] {
            let bench = Bench::new();
            let report = run_lines(
                &command(&[]),
                ("f.md", &doc(&["type --text hi", "key --key return"])),
                DEADLINE,
                |argv, _| {
                    if argv[0] == "type" {
                        said(
                            &json!({ "verification": { "state": "unverified", "reason": reason } }),
                        )
                    } else {
                        ok()
                    }
                },
                |_, _, _| {},
                bench.desk(false, None),
                || true,
            )
            .expect("walked");
            assert_eq!(report["stop"]["kind"].clone(), kind, "{reason}");
        }
    }

    /// The pointer is read before the walk and after every step: a person's
    /// hand on it during any step — a key's, a typing's — stops the walk, and
    /// a step that moved it itself does not.
    #[test]
    fn a_persons_hand_on_the_pointer_stops_the_walk() {
        let bench = Bench::at(Some((300.0, 40.0)));
        let text = doc(&["mouse-click --x 10 --y 10", "type --text hi"]);
        let mut ran = 0;
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| {
                ran += 1;
                ok()
            },
            |_, _, _| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            ran, 1,
            "the typing never lands where the person's pointer went"
        );
        assert_eq!(report["stop"]["kind"], "person_moved");
        assert_eq!(report["next"], 2, "the click ran; look, then go on");
        let still = Bench::at(Some((11.0, 10.0)));
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| ok(),
            |_, _, _| {},
            still.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            report["done"], true,
            "within the slop the pointer is where the walk left it"
        );
        assert_eq!(report["handWatched"], true);

        // A keyboard recipe: no step names a point, yet the hand is watched.
        let keys = Bench::at(Some((50.0, 50.0)));
        let mut ran = 0;
        let report = run_lines(
            &command(&[]),
            (
                "f.md",
                &doc(&["key --key cmd+n", "type --text hi", "key --key cmd+s"]),
            ),
            DEADLINE,
            |_, _| {
                ran += 1;
                if ran == 2 {
                    keys.pointer.set(Some((400.0, 300.0)));
                }
                ok()
            },
            |_, _, _| {},
            keys.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            (ran, report["stop"]["kind"].clone()),
            (2, json!("person_moved"))
        );
        assert_eq!(report["next"], 3);

        // An app's click moves the pointer to a point the walk cannot name.
        let clicked = Bench::at(Some((50.0, 50.0)));
        let report = run_lines(
            &command(&[]),
            (
                "f.md",
                &doc(&[
                    "mouse-click --x 50 --y 50",
                    "click --app Mail --x 40 --y 60",
                    "type --text hi",
                ]),
            ),
            DEADLINE,
            |argv, _| {
                if argv[0] == "click" {
                    clicked.pointer.set(Some((720.0, 460.0)));
                }
                ok()
            },
            |_, _, _| {},
            clicked.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            report["done"], true,
            "the walk's own app click is not a person: {report}"
        );

        let blind = Bench::new();
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| ok(),
            |_, _, _| {},
            blind.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            (report["done"].clone(), report["handWatched"].clone()),
            (json!(true), json!(false)),
            "a desk that cannot say where the pointer is says the hand was not watched"
        );
    }

    /// A step that cannot end inside the run waits for the next call — never
    /// cut to fit — and a stop lands at a step's own door, which leaves its
    /// line like any refused action.
    #[test]
    fn a_walk_ends_inside_its_budget_and_at_the_persons_stop() {
        let bench = Bench::new();
        let text = doc(&["key --key a", "wait-for --app X --text Done", "key --key b"]);
        let mut sent = Vec::new();
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |argv, _| {
                bench.clock.set(walk_budget_ms(DEADLINE) - 3_000);
                sent.push(argv.to_vec());
                ok()
            },
            |_, _, _| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(sent.len(), 1, "the check would outlast the run: {sent:?}");
        assert_eq!(
            (report["stop"]["kind"].clone(), report["next"].clone()),
            (json!("budget"), json!(2))
        );

        let late = Bench::new();
        late.clock.set(walk_budget_ms(DEADLINE) + 1);
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| ok(),
            |_, _, _| {},
            late.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            (report["stop"]["kind"].clone(), report["next"].clone()),
            (json!("budget"), json!(1))
        );

        let stopped = Bench::new();
        let mut ran = 0;
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| {
                ran += 1;
                refused(error_code::STOPPED)
            },
            |_, _, _| {},
            stopped.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            (ran, report["stop"]["kind"].clone(), report["next"].clone()),
            (1, json!("stopped"), json!(1)),
            "the first act went to its door, was refused there, and nothing after it ran"
        );
        let gone = Bench::new();
        let report = run_lines(
            &command(&[]),
            ("f.md", &text),
            DEADLINE,
            |_, _| ok(),
            |_, _, _| {},
            gone.desk(false, None),
            || false,
        )
        .expect("walked");
        assert_eq!(
            report["ran"].as_array().map(Vec::len),
            Some(0),
            "a caller gone gets no steps"
        );
    }

    #[test]
    fn an_automatic_resume_completes_the_original_full_request() {
        let host = "example.test";
        let page = format!("https://{host}");
        let mut spec = flow(vec![check(
            1,
            CheckKind::State,
            true,
            RecipeTool::Browser,
            &["find", "page", "Done"],
        )]);
        spec.fingerprint.hosts.insert(host.into());
        let text = flow_doc(
            &[
                (RecipeTool::Browser, "click page #first"),
                (RecipeTool::Browser, "click page #second"),
            ],
            &spec,
        );
        let given = command(&[]);
        let bench = Bench::new().showing(&[("page", &page)]);
        let mut sent = Vec::new();
        let first = run(
            &walk_of(&given, &text),
            |_, argv, _| {
                sent.push(argv.to_vec());
                if argv[0] == "find" {
                    browser_said(r#"{"count":0}"#)
                } else if argv[2] == "#second" {
                    browser_refused("missing")
                } else {
                    browser_said("clicked")
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .unwrap();
        assert_eq!(first["done"], false);
        assert_eq!(first["stoppedAt"], 2);
        assert!(!verdict_of(&first).unwrap().0);

        // The recovery cursor is separate from the person's full request.
        let resumed = Run {
            resume_from: Some(2),
            ..walk_of(&given, &text)
        };
        let second = run(
            &resumed,
            |_, argv, _| {
                sent.push(argv.to_vec());
                if argv[0] == "find" {
                    browser_said(r#"{"count":1}"#)
                } else {
                    browser_said("clicked")
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .unwrap();
        assert_eq!(second["done"], true);
        assert_eq!(second["flow"]["verdict"]["pass"], true);
        assert_eq!(sent.iter().filter(|argv| argv[0] == "find").count(), 2);
        assert_eq!(second["complete"], true);
        assert!(verdict_of(&second).unwrap().0);
        assert!(given.params.get("start").is_none());

        let tail = command(&["--start", "2"]);
        let manual = run(
            &walk_of(&tail, &text),
            |_, argv, _| {
                if argv[0] == "find" {
                    browser_said(r#"{"count":1}"#)
                } else {
                    browser_said("clicked")
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .unwrap();
        assert_eq!(manual["done"], true);
        assert_eq!(manual["complete"], false);
        assert!(!verdict_of(&manual).unwrap().0);
        assert!(
            run(
                &Run {
                    resume_from: Some(1),
                    ..walk_of(&tail, &text)
                },
                |_, _, _| panic!("an out-of-range recovery must not act"),
                |_, _, _| {},
                |_| {},
                bench.desk(false, None),
                || true,
            )
            .is_err()
        );
    }

    #[test]
    fn a_single_step_retry_walks_only_that_line_and_cannot_pass_the_whole_flow() {
        let bench = Bench::new();
        let text = doc(&["key --key a", "key --key b", "key --key c"]);
        let mut pressed = Vec::new();
        let report = run_lines(
            &command(&["--start", "2", "--end", "2"]),
            ("f.md", &text),
            DEADLINE,
            |_, argv| {
                pressed.push(argv.to_vec());
                ok()
            },
            |_, _, _| {},
            bench.desk(false, None),
            || true,
        )
        .unwrap();
        assert_eq!(pressed.len(), 1);
        assert_eq!(report["ran"][0]["step"], 2);
        assert_eq!(report["complete"], false);
        let mut with_verdict = report;
        with_verdict["flow"] = json!({ "verdict": { "pass": true, "lines": [] } });
        assert_eq!(verdict_of(&with_verdict).map(|(pass, _)| pass), Some(false));
        assert!(decide(&command(&["--start", "3", "--end", "2"]), &text).is_err());
        assert!(decide(&command(&["--end", "9"]), &text).is_err());
    }

    #[test]
    fn missing_parameters_and_unreadable_lines_refuse_before_anything_moves() {
        let bench = Bench::new();
        let mut ran = 0;
        let mut walk_it = |text: &str, extra: &[&str]| {
            run_lines(
                &command(extra),
                ("f.md", text),
                DEADLINE,
                |_, _| {
                    ran += 1;
                    ok()
                },
                |_, _, _| {},
                bench.desk(false, None),
                || true,
            )
        };
        let missing = walk_it(&doc(&["type --text {{a}}", "type --text {{b}}"]), &[]).unwrap_err();
        assert!(
            missing.message.contains("{{a}}") && missing.message.contains("{{b}}"),
            "{missing:?}"
        );
        let broken = walk_it(&doc(&["key --key a", "mouse-click --x one --y 1"]), &[]).unwrap_err();
        assert!(broken.message.starts_with("step 2"), "{broken:?}");
        assert!(walk_it(&doc(&["key --key a"]), &["--start", "5"]).is_err());
        let lifted =
            walk_it(&doc(&["key --key a", "key --key return --allow-self"]), &[]).unwrap_err();
        assert!(lifted.message.contains("allow-self"), "{lifted:?}");
        let unused =
            walk_it(&doc(&["key --key a"]), &["--params", r#"{"spare":"x"}"#]).expect("walked");
        assert_eq!(unused["unusedParams"], json!(["spare"]));
        assert_eq!(ran, 1, "only the walk that could start moved anything");
        assert_eq!(
            first_act(
                &command(&["--params", r#"{"k":"a"}"#]),
                &doc(&["screenshot", "wait --ms 1", "key --key {{k}}"])
            ),
            Some((
                vec![
                    "key".to_string(),
                    "--key".into(),
                    "a".into(),
                    "--json".into()
                ],
                vec![
                    "key".to_string(),
                    "--key".into(),
                    "{{k}}".into(),
                    "--json".into()
                ]
            )),
            "a refused walk's first act, as sent and as logged"
        );
        assert_eq!(
            first_act(&command(&[]), &doc(&["handoff --reason x", "key --key a"])),
            None,
            "a walk that stops first sends nothing"
        );
    }

    fn set(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|word| (*word).to_string()).collect()
    }

    /// The Flow every Flow test walks under: one desktop app, one host, this
    /// build's protocol, and the checks given.
    fn flow(checks: Vec<Check>) -> FlowSpec {
        FlowSpec {
            policy: Policy::Dry,
            evidence: EvidenceLevel::Full,
            fingerprint: Fingerprint {
                apps: set(&["com.apple.mail"]),
                hosts: set(&["stg.example"]),
                protocol: COMPUTER_USE_PROTOCOL_VERSION,
                assets: BTreeSet::new(),
            },
            checks,
            money: None,
            confirm: Confirm::Person,
            trigger: None,
        }
    }

    fn check(id: usize, kind: CheckKind, required: bool, tool: RecipeTool, line: &[&str]) -> Check {
        Check {
            id,
            kind,
            required,
            tool,
            argv: words(line),
        }
    }

    #[test]
    fn emulator_checks_distinguish_absence_from_an_unreadable_answer() {
        for verb in ["find", "foreground"] {
            let check = check(1, CheckKind::State, true, RecipeTool::Emulator, &[verb]);
            for count in [0, 1, 3] {
                let answer = said(&json!({"count": count}));
                assert_eq!(
                    presence_of(&check, &answer),
                    Observed::Seen(Presence {
                        present: count > 0,
                        count: Some(count)
                    })
                );
            }
            for answer in [
                said(&json!({})),
                said(&json!({"count": -1})),
                refused("emulator_error"),
            ] {
                assert!(matches!(
                    presence_of(&check, &answer),
                    Observed::NotEvaluable(_)
                ));
            }
            assert!(check_argv(&check, 500).contains(&"--json".to_string()));
        }
    }

    #[test]
    fn emulator_checks_judge_after_two_mobile_steps_and_rejudge_the_record() {
        for count in [0, 1] {
            let spec = flow(vec![check(
                1,
                CheckKind::State,
                true,
                RecipeTool::Emulator,
                &[
                    "find",
                    "--platform",
                    "ios",
                    "--device",
                    "phone",
                    "--text",
                    "Done",
                ],
            )]);
            let text = flow_doc(
                &[
                    (
                        RecipeTool::Emulator,
                        "button --platform ios --device phone --name home",
                    ),
                    (
                        RecipeTool::Emulator,
                        "button --platform ios --device phone --name home",
                    ),
                ],
                &spec,
            );
            let command = command(&[]);
            let bench = Bench::new();
            let mut sent = Vec::new();
            let report = run(
                &walk_of(&command, &text),
                |tool, argv, _| {
                    assert_eq!(tool, RecipeTool::Emulator);
                    sent.push(argv[0].clone());
                    if argv[0] == "find" {
                        said(&json!({"count": count}))
                    } else {
                        ok()
                    }
                },
                |_, _, _| {},
                |_| {},
                bench.desk(false, None),
                || true,
            )
            .unwrap();
            assert_eq!(sent, ["button", "button", "find"]);
            assert_eq!(report["done"], true);
            assert_eq!(report["flow"]["verdict"]["pass"], count > 0);
            let record: zerocode_core::computer_flow::FlowRecord =
                serde_json::from_value(report["flow"].clone()).unwrap();
            assert_eq!(record.rejudged(), record.verdict);
        }
    }

    /// The value a `--flag` carries in a sent line.
    fn flag_value<'a>(argv: &'a [String], flag: &str) -> Option<&'a str> {
        argv.iter()
            .position(|word| word == flag)
            .and_then(|at| argv.get(at + 1))
            .map(String::as_str)
    }

    #[test]
    fn flow_retry_requires_a_live_recipe_and_its_original_workspace() {
        for extra in [vec![], vec!["--arena", "/recorded"]] {
            for cwd in [None, Some(Path::new("/original"))] {
                let command = command(&extra);
                let text = doc(&["key --key tab", "wait-for --app Mail --text ready"]);
                let mut walk = walk_of(&command, &text);
                walk.cwd = cwd;
                let bench = Bench::new();
                let mut observations = Vec::new();
                run(
                    &walk,
                    |_, _, _| {
                        observations.push(crate::run_evidence::observation().unwrap());
                        ok()
                    },
                    |_, _, _| {},
                    |_| {},
                    bench.desk(false, None),
                    || true,
                )
                .unwrap();
                assert_eq!(
                    observations[0].get("retry").is_some(),
                    extra.is_empty() && cwd.is_some()
                );
                assert!(
                    observations[1].get("retry").is_none(),
                    "a check is not a retry action"
                );
                assert!(crate::run_evidence::observation().is_none());
            }
        }
    }

    /// Every walked step reports what it spent where — the act's round trip
    /// down the road, the settle looked through after a press, the read-back
    /// judged, the pointer read — in the four phases, whose sum never exceeds
    /// the walk's own `ms` for the step; the walk says when it began and what
    /// it spent before and after its steps; and each step names the evidence
    /// identity its actual call passed to the writer.
    #[test]
    fn each_walked_step_reports_its_phases_and_the_walk_keeps_ms() {
        let bench = Bench::at(Some((5.0, 5.0)));
        let lines: Vec<String> = (0..20)
            .map(|at| match at % 4 {
                0 => "mouse-click --x 5 --y 5".to_string(),
                1 => "type --text hi".to_string(),
                2 => "key --key tab".to_string(),
                _ => "wait --ms 1".to_string(),
            })
            .collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let text = doc(&refs);
        let command = command(&[]);
        let walk = walk_of(&command, &text);
        let report = run(
            &walk,
            |_, argv, _| {
                // The road takes its time: the fake clock moves for every act.
                bench.clock.set(bench.clock.get() + 7);
                if argv[0] == "type" {
                    said(&json!({ "verification": { "state": "verified", "property": "value" } }))
                } else {
                    ok()
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(true, Some(true)),
            || true,
        )
        .expect("walked");
        assert_eq!(report["kind"], "recipe-run");
        assert!(report["at_epoch_ms"].is_i64(), "{report}");
        assert!(
            report["phases"]["resolve"].is_u64() && report["phases"]["report"].is_u64(),
            "the walk's own stages: {}",
            report["phases"]
        );
        let ran = report["ran"].as_array().expect("steps");
        assert_eq!(ran.len(), 20);
        let (mut phases_total, mut ms_total) = (0, 0);
        for step in ran {
            assert_eq!(step["tool"], "computer");
            let phases = step["phases"].as_object().expect("phases");
            assert_eq!(
                phases.keys().cloned().collect::<BTreeSet<_>>(),
                set(&["act", "settle", "verify", "pointer"]),
                "{step}"
            );
            let spent: u64 = phases.values().map(|ms| ms.as_u64().unwrap()).sum();
            let ms = step["ms"].as_u64().expect("the walk's ms");
            assert!(spent <= ms, "phases add up past the step: {step}");
            assert!(
                phases["act"].as_u64().unwrap() >= 7,
                "the act is the road's round trip: {step}"
            );
            assert!(
                step["evidence_id"].is_string() && step.get("evidence_n").is_none(),
                "{step}"
            );
            phases_total += spent;
            ms_total += ms;
        }
        eprintln!("20 steps: phases {phases_total} ms of {ms_total} ms walked");
        assert!(phases_total <= ms_total);
    }

    /// A Flow is walked only where it was recorded: a page on a host the
    /// recording never saw — read from the window's records, no pane asked —
    /// refuses the run by name before its first act, and so does a policy
    /// this wave does not build; the same pages on the recorded host walk.
    #[test]
    fn a_flow_run_refuses_a_stale_fingerprint_before_the_first_act() {
        let spec = flow(vec![check(
            1,
            CheckKind::State,
            true,
            RecipeTool::Computer,
            &["wait-for", "--app", "Mail", "--text", "Done"],
        )]);
        let text = flow_doc(&[(RecipeTool::Browser, "click browser-1 #pay")], &spec);
        let command = command(&[]);
        let bench = Bench::new().showing(&[("browser-1", "https://other.example/login")]);
        let mut sent = 0;
        let mut begun = None;
        let refused = run(
            &walk_of(&command, &text),
            |_, _, _| {
                sent += 1;
                ok()
            },
            |_, _, _| {},
            |level| begun = Some(level),
            bench.desk(false, None),
            || true,
        )
        .expect_err("a host the recording never saw");
        assert_eq!(refused.code, error_code::FLOW_STALE);
        assert!(refused.message.contains("other.example"), "{refused:?}");
        assert_eq!((sent, begun), (0, None), "nothing moved, no walk began");

        let guarded = FlowSpec {
            policy: Policy::Guarded,
            ..spec.clone()
        };
        let text = flow_doc(&[(RecipeTool::Browser, "click browser-1 #pay")], &guarded);
        let bench = Bench::new().showing(&[("browser-1", "https://stg.example/pay")]);
        let refused = run(
            &walk_of(&command, &text),
            |_, _, _| {
                sent += 1;
                ok()
            },
            |_, _, _| {},
            |level| begun = Some(level),
            bench.desk(false, None),
            || true,
        )
        .expect_err("a guarded Flow without a money line has nothing to guard");
        assert_eq!(refused.code, error_code::INVALID_ARGUMENT);
        assert!(refused.message.contains("money"), "{refused:?}");
        assert_eq!((sent, begun), (0, None));

        let text = flow_doc(&[(RecipeTool::Browser, "click browser-1 #pay")], &spec);
        let report = run(
            &walk_of(&command, &text),
            |tool, argv, _| {
                sent += 1;
                if tool == RecipeTool::Browser {
                    browser_said("클릭 이벤트를 보냈습니다")
                } else {
                    assert_eq!(argv[0], "wait-for");
                    ok()
                }
            },
            |_, _, _| {},
            |level| begun = Some(level),
            bench.desk(false, None),
            || true,
        )
        .expect("the recorded host walks");
        assert_eq!(report["done"], true, "{report}");
        assert_eq!(
            begun,
            Some(EvidenceLevel::Full),
            "the walk began at its level"
        );
        assert_eq!(sent, 2, "the click, then the one state line judged");
        assert_eq!(report["ran"][0]["tool"], "browser");
        assert_eq!(
            report["flow"]["verdict"]["pass"], true,
            "{}",
            report["flow"]
        );
    }

    /// The environment is bound act by act: a browser press aimed at a pane
    /// that has left the recorded hosts is refused before it is sent, and a
    /// desktop act that answered an app outside the recording stops the walk
    /// right after it — found after the fact, said after the fact.
    #[test]
    fn a_flow_run_refuses_an_act_outside_its_recorded_hosts_and_apps() {
        let spec = flow(vec![check(
            1,
            CheckKind::State,
            true,
            RecipeTool::Computer,
            &["wait-for", "--app", "Mail", "--text", "Done"],
        )]);
        let text = flow_doc(
            &[
                (RecipeTool::Browser, "click browser-1 #login"),
                (RecipeTool::Browser, "click browser-1 #pay"),
                (RecipeTool::Computer, "key --key a"),
            ],
            &spec,
        );
        let command = command(&[]);
        let bench = Bench::new().showing(&[("browser-1", "https://stg.example/login")]);
        let mut acts: Vec<Vec<String>> = Vec::new();
        let report = run(
            &walk_of(&command, &text),
            |tool, argv, _| {
                if tool == RecipeTool::Browser {
                    acts.push(argv.to_vec());
                    // The login sent the pane somewhere the recording never saw.
                    bench.show(&[("browser-1", "https://phish.example/pay")]);
                    browser_said("클릭 이벤트를 보냈습니다")
                } else if argv[0] == "wait-for" {
                    ok()
                } else {
                    acts.push(argv.to_vec());
                    ok()
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            acts.len(),
            1,
            "only the first press reached its pane: {acts:?}"
        );
        assert_eq!(report["ran"][1]["ok"], false);
        assert_eq!(
            report["ran"][1]["error"]["code"],
            error_code::FLOW_ENV_MISMATCH,
            "{report}"
        );
        assert!(
            report["ran"][1]["error"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("phish.example")),
            "{report}"
        );
        assert_eq!(report["stop"]["kind"], "step_failed");
        assert_eq!(report["stop"]["code"], error_code::FLOW_ENV_MISMATCH);
        assert_eq!(
            report["next"], 2,
            "fix the environment, then run the press again"
        );
        let (pass, reason) = verdict_of(&report).expect("a Flow leaves a verdict");
        assert!(
            !pass,
            "a stopped Flow is not green, whatever its lines read"
        );
        assert!(
            reason
                .as_deref()
                .is_some_and(|why| why.contains("stopped at step 2")),
            "{reason:?}"
        );

        let text = flow_doc(
            &[
                (RecipeTool::Computer, "click --app Mail --x 1 --y 1"),
                (RecipeTool::Computer, "key --key a"),
            ],
            &spec,
        );
        let bench = Bench::new();
        let mut acts = 0;
        let report = run(
            &walk_of(&command, &text),
            |_, argv, _| match argv[0].as_str() {
                "click" => {
                    acts += 1;
                    said(&json!({ "app": { "name": "Other", "bundleId": "com.other.app", "pid": 3 } }))
                }
                "wait-for" => ok(),
                _ => {
                    acts += 1;
                    ok()
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(acts, 1, "the key never follows a press into the wrong app");
        assert_eq!(report["ran"][0]["ok"], true, "the press happened: {report}");
        assert_eq!(report["stop"]["kind"], "needs_a_look", "{report}");
        assert_eq!(report["stop"]["code"], error_code::FLOW_ENV_MISMATCH);
        assert!(
            report["stop"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("com.other.app")),
            "{report}"
        );
        assert_eq!(report["next"], 2);
    }

    /// Before its first act a Flow asks every event line once with no wait —
    /// the baseline — and after its last step asks every line again with the
    /// verify budget; the judge is the core's: an event already there at the
    /// baseline is stale, one that appeared is this run's, a state line is
    /// judged as found; and the verdict the folder gets is the judge's.
    #[test]
    fn a_flow_run_probes_the_baseline_then_judges_each_oracle_line() {
        let spec = flow(vec![
            check(
                1,
                CheckKind::Event,
                true,
                RecipeTool::Computer,
                &["wait-for", "--app", "Mail", "--text", "Sent"],
            ),
            check(
                2,
                CheckKind::Event,
                false,
                RecipeTool::Browser,
                &["find", "browser-1", "RCPT-"],
            ),
            check(
                3,
                CheckKind::State,
                false,
                RecipeTool::Computer,
                &["wait-for", "--app", "Mail", "--text", "Inbox"],
            ),
        ]);
        let text = flow_doc(&[(RecipeTool::Computer, "key --key a")], &spec);
        let command = command(&[]);
        let bench = Bench::new().showing(&[("browser-1", "https://stg.example/")]);
        let mut sent: Vec<Vec<String>> = Vec::new();
        let mut observations = Vec::new();
        // Already there before the walk: the event is not this run's.
        let mut walk = walk_of(&command, &text);
        walk.cwd = Some(Path::new("/original"));
        let report = run(
            &walk,
            |_, argv, _| {
                sent.push(argv.to_vec());
                observations.push(crate::run_evidence::observation().unwrap());
                match argv[0].as_str() {
                    "find" => browser_said(r#"{"count":2,"index":1}"#),
                    _ => ok(),
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        let verbs: Vec<&str> = sent.iter().map(|argv| argv[0].as_str()).collect();
        assert_eq!(
            verbs,
            ["wait-for", "find", "key", "wait-for", "find", "wait-for"],
            "the event lines probed, the walk, then every line judged"
        );
        assert_eq!(
            flag_value(&sent[0], "--timeout-ms"),
            Some(FLOW_BASELINE_PROBE_MS.to_string().as_str()),
            "the probe waits nothing: {:?}",
            sent[0]
        );
        assert_eq!(
            flag_value(&sent[3], "--timeout-ms"),
            Some(FLOW_VERIFY_MS.to_string().as_str()),
            "a verify line gets the verify budget: {:?}",
            sent[3]
        );
        assert_eq!(
            flag_value(&sent[5], "--timeout-ms"),
            Some(FLOW_VERIFY_MS.to_string().as_str())
        );
        assert!(report["ran"][0]["evidence_id"].is_string());
        assert!(
            report["ran"][0].get("evidence_n").is_none(),
            "the writer assigns the physical row"
        );
        assert_eq!(
            observations[2]["retry"],
            json!({"name": "test", "step": 1, "cwd": "/original"})
        );
        assert!(
            observations
                .iter()
                .enumerate()
                .all(|(at, observation)| at == 2 || observation.get("retry").is_none()),
            "baseline and oracle are not recipe lines"
        );
        assert!(
            crate::run_evidence::observation().is_none(),
            "retry origin cannot leak into recovery or a later call"
        );
        let flow = &report["flow"];
        let baseline = flow["baseline"].as_object().expect("the baseline");
        assert_eq!(
            baseline.keys().cloned().collect::<Vec<_>>(),
            ["1", "2"],
            "only the event lines are probed: {flow}"
        );
        assert_eq!(flow["baseline"]["2"]["count"], 2);
        let status = |id: usize| flow["verdict"]["lines"][id - 1]["status"].clone();
        assert_eq!(
            (status(1), status(2), status(3)),
            (json!("stale"), json!("stale"), json!("pass")),
            "{flow}"
        );
        assert_eq!(flow["verdict"]["pass"], false);
        let (pass, reason) = verdict_of(&report).expect("a Flow leaves a verdict");
        assert!(!pass);
        assert!(
            reason.as_deref().is_some_and(|why| why.contains('1')),
            "the failing required line is named: {reason:?}"
        );
        assert!(
            report["phases"]["baseline"].is_u64() && report["phases"]["oracle"].is_u64(),
            "the verify time is written apart from the acts: {}",
            report["phases"]
        );

        // Absent before, present after: this run's event — and the judge
        // runs afresh, never from the last verdict.
        let mut sent: Vec<Vec<String>> = Vec::new();
        let report = run(
            &walk_of(&command, &text),
            |_, argv, _| {
                let probing = flag_value(argv, "--timeout-ms")
                    == Some(FLOW_BASELINE_PROBE_MS.to_string().as_str());
                sent.push(argv.to_vec());
                match argv[0].as_str() {
                    "wait-for" if probing => refused(error_code::TIMEOUT),
                    "find" if sent.len() == 2 => browser_said(r#"{"count":0,"index":0}"#),
                    "find" => browser_said(r#"{"count":1,"index":1}"#),
                    _ => ok(),
                }
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        let flow = &report["flow"];
        assert_eq!(flow["baseline"]["1"]["present"], false);
        let status = |id: usize| flow["verdict"]["lines"][id - 1]["status"].clone();
        assert_eq!(
            (status(1), status(2), status(3)),
            (json!("pass"), json!("pass"), json!("pass")),
            "{flow}"
        );
        assert_eq!(verdict_of(&report), Some((true, None)));
        assert_eq!(report["done"], true);
        let record: zerocode_core::computer_flow::FlowRecord =
            serde_json::from_value(flow.clone()).expect("the record verify re-judges");
        assert_eq!(record.rejudged(), record.verdict);
        let _: BTreeMap<usize, zerocode_core::computer_flow::Presence> = record.baseline;
    }

    /// An emulator step is walked through the emulator door: it is sent with
    /// `--json` so the device answers an envelope, it reports its four phases,
    /// a pressed rect the answer carries is the report's ring in device pixels,
    /// and — bound act by act like a desktop act — a tap that answered from an
    /// app the recording never saw stops the walk right after it (found after
    /// the fact, said after the fact).
    #[test]
    fn an_emulator_step_reports_its_phases_and_binds_to_the_recorded_package() {
        let spec = FlowSpec {
            money: None,
            confirm: zerocode_core::computer_flow::Confirm::default(),
            trigger: None,
            policy: Policy::Dry,
            evidence: EvidenceLevel::Full,
            fingerprint: Fingerprint {
                apps: set(&["com.example.wallet"]),
                hosts: BTreeSet::new(),
                protocol: COMPUTER_USE_PROTOCOL_VERSION,
                assets: BTreeSet::new(),
            },
            // A Flow names at least one oracle line; a state check, judged in
            // the oracle pass, leaves the acts' binding the point of the test.
            checks: vec![check(
                1,
                CheckKind::State,
                true,
                RecipeTool::Computer,
                &["wait-for", "--app", "Mail", "--text", "Done"],
            )],
        };
        let text = flow_doc(
            &[
                (
                    RecipeTool::Emulator,
                    "tap --platform android --device phone --x 0.5 --y 0.5",
                ),
                (
                    RecipeTool::Emulator,
                    "tap --platform android --device phone --x 0.1 --y 0.1",
                ),
                (RecipeTool::Computer, "key --key a"),
            ],
            &spec,
        );
        let command = command(&[]);
        let bench = Bench::new();
        let mut acts: Vec<Vec<String>> = Vec::new();
        let report = run(
            &walk_of(&command, &text),
            |tool, argv, _| match tool {
                RecipeTool::Emulator => {
                    acts.push(argv.to_vec());
                    if acts.len() == 1 {
                        // Lands in the recorded app and answers a pressed rect
                        // in device pixels.
                        said(&json!({
                            "performed": true,
                            "app": { "package": "com.example.wallet" },
                            "rect": { "x": 40, "y": 80, "width": 20, "height": 10 },
                        }))
                    } else {
                        said(&json!({
                            "performed": true,
                            "app": { "package": "com.phish.app" },
                        }))
                    }
                }
                RecipeTool::Computer => ok(),
                RecipeTool::Browser => unreachable!("no browser step"),
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert!(
            acts[0].contains(&"--json".to_string()),
            "an emulator step is sent with --json: {:?}",
            acts[0]
        );
        assert_eq!(report["ran"][0]["tool"], "emulator");
        let phases = report["ran"][0]["phases"].as_object().expect("phases");
        assert_eq!(
            phases.keys().cloned().collect::<BTreeSet<_>>(),
            set(&["act", "settle", "verify", "pointer"]),
            "{}",
            report["ran"][0]
        );
        // The pressed rect is device pixels — 1:1, its own space.
        assert_eq!(report["ran"][0]["target"]["space"], "px");
        assert_eq!(
            report["ran"][0]["target"]["rect"],
            json!([40.0, 80.0, 20.0, 10.0])
        );
        // The second tap answered from an app the recording never saw: the
        // walk stops after it, and the key never follows into the wrong app.
        assert_eq!(acts.len(), 2, "the key never runs: {acts:?}");
        assert_eq!(report["ran"][1]["ok"], true, "the press happened: {report}");
        assert_eq!(
            report["ran"][1]["error"]["code"],
            error_code::FLOW_ENV_MISMATCH,
            "{report}"
        );
        assert!(
            report["ran"][1]["error"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("com.phish.app")),
            "{report}"
        );
        assert_eq!(report["stop"]["kind"], "needs_a_look");
        assert_eq!(report["stop"]["code"], error_code::FLOW_ENV_MISMATCH);
        assert_eq!(report["next"], 3, "fix the app, then run the tap again");
    }

    /// Twenty emulator taps walked with a mock clock: the phases each step
    /// reports and the walk's own `ms`, summed — printed, not checked
    /// (plan §5 numbers, simulated).
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn measure_emulator_walk_of_twenty_steps() {
        let bench = Bench::new();
        let lines: Vec<(RecipeTool, &str)> = (0..20)
            .map(|_| {
                (
                    RecipeTool::Emulator,
                    "tap --platform android --device phone --x 0.5 --y 0.5",
                )
            })
            .collect();
        let text = mixed_doc(&lines);
        let command = command(&[]);
        let report = run(
            &walk_of(&command, &text),
            |_, _, _| {
                // The road takes its time: the mock clock moves each act.
                bench.clock.set(bench.clock.get() + 7);
                said(&json!({
                    "performed": true,
                    "app": { "package": "com.example.wallet" },
                    "rect": { "x": 40, "y": 80, "width": 20, "height": 10 },
                }))
            },
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        let ran = report["ran"].as_array().expect("steps");
        let (mut phases_total, mut ms_total) = (0u64, 0u64);
        for step in ran {
            phases_total += step["phases"]
                .as_object()
                .expect("phases")
                .values()
                .map(|ms| ms.as_u64().unwrap_or(0))
                .sum::<u64>();
            ms_total += step["ms"].as_u64().unwrap_or(0);
        }
        eprintln!(
            "20 emulator steps: phases {phases_total} ms of {ms_total} ms walked ({} steps)",
            ran.len()
        );
    }

    /// The guarded Flow of the design (docs/design/flow-engine-guarded-money-
    /// path.md §6): the recipient and the amount typed from the parameters,
    /// the press marked money, one event line watching the transfer.
    fn money_flow(policy: Policy, confirm: Confirm) -> FlowSpec {
        FlowSpec {
            policy,
            money: Some(Money {
                id: "txn".into(),
                amount: "amount".into(),
                recipient: "recipient".into(),
                step: 3,
            }),
            confirm,
            ..flow(vec![check(
                1,
                CheckKind::Event,
                true,
                RecipeTool::Browser,
                &["find", "browser-1", "송금이 완료되었습니다"],
            )])
        }
    }

    fn money_doc(spec: &FlowSpec) -> String {
        format!(
            "# Pay\n\n{RECIPE_HEADING_STEPS}\n\n\
             1. `zerocode-browser type browser-1 #recipient {{{{recipient}}}}`\n\
             2. `zerocode-browser type browser-1 #amount {{{{amount}}}}`\n\
             3. `zerocode-browser click browser-1 #send`{RECIPE_MARK_SEPARATOR}{RECIPE_MARK_MONEY}\n\n{}",
            spec.written()
        )
    }

    const PARAMS: &str = r#"{"txn":"TXN-1","amount":"50000","recipient":"Kim"}"#;

    /// The pane as a fake bank shows it: it agrees with the transaction (or
    /// not), and the transfer completes once the press lands (or never). What
    /// was sent is kept, and the ledger's text at the moment of the press.
    struct Bank {
        agrees: bool,
        completes: bool,
        sent: RefCell<Vec<Vec<String>>>,
        pressed: Cell<bool>,
        ledger_at_press: RefCell<Option<String>>,
        ledger: std::path::PathBuf,
    }

    impl Bank {
        fn new(dir: &std::path::Path, agrees: bool, completes: bool) -> Self {
            Self {
                agrees,
                completes,
                sent: RefCell::new(Vec::new()),
                pressed: Cell::new(false),
                ledger_at_press: RefCell::new(None),
                ledger: dir.join(format!("pay.{LEDGER_EXTENSION}")),
            }
        }

        fn road(&self, argv: &[String]) -> zerocode_hookd::TeamAnswer {
            self.sent.borrow_mut().push(argv.to_vec());
            match argv[0].as_str() {
                "type" => browser_said("입력했습니다"),
                "click" => {
                    *self.ledger_at_press.borrow_mut() = std::fs::read_to_string(&self.ledger).ok();
                    self.pressed.set(true);
                    browser_said("클릭 이벤트를 보냈습니다")
                }
                "find" => {
                    let count = match argv[2].as_str() {
                        "송금이 완료되었습니다" => {
                            usize::from(self.pressed.get() && self.completes)
                        }
                        _ => usize::from(self.agrees),
                    };
                    browser_said(&format!(r#"{{"count":{count},"index":0}}"#))
                }
                other => panic!("the bank was asked {other}"),
            }
        }

        fn verbs(&self) -> Vec<String> {
            self.sent
                .borrow()
                .iter()
                .map(|argv| {
                    if argv[0] == "find" {
                        format!("find {}", argv[2])
                    } else {
                        argv[0].clone()
                    }
                })
                .collect()
        }

        fn ledger_lines(&self) -> Vec<Line> {
            std::fs::read_to_string(&self.ledger)
                .map(|held| {
                    held.lines()
                        .map(|row| serde_json::from_str(row).expect(row))
                        .collect()
                })
                .unwrap_or_default()
        }
    }

    fn pay<'a>(
        command: &'a ComputerCommand,
        text: &'a str,
        file: &'a str,
        dir: &'a std::path::Path,
    ) -> Run<'a> {
        Run {
            command,
            resume_from: None,
            file,
            text,
            deadline_ms: DEADLINE,
            cwd: None,
            evidence_dir: Some(dir),
        }
    }

    /// A dry Flow rehearses up to its money step and never presses it: the
    /// walk stops before the step, says why, and writes no ledger.
    #[test]
    fn a_dry_flow_stops_before_its_money_step() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pay.md").display().to_string();
        let text = money_doc(&money_flow(Policy::Dry, Confirm::Person));
        let given = bench::command(&["--params", PARAMS]);
        let bench = Bench::new().showing(&[("browser-1", "https://stg.example/pay")]);
        let bank = Bank::new(dir.path(), true, true);
        let report = run(
            &pay(&given, &text, &file, dir.path()),
            |_, argv, _| bank.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(
            bank.verbs(),
            [
                "find 송금이 완료되었습니다",
                "type",
                "type",
                "find 송금이 완료되었습니다"
            ],
            "the baseline, the two typings, the oracle — never the press, and no witness asked of a rehearsal"
        );
        assert_eq!(report["stoppedAt"], 3, "{report}");
        assert_eq!(report["stop"]["kind"], "money_step");
        assert_eq!(
            report["next"], 3,
            "the money step is where a guarded run goes on from"
        );
        assert!(
            report["stop"]["message"]
                .as_str()
                .is_some_and(|said| said.contains(Policy::Dry.as_str())),
            "the card says the rehearsal ended before the money: {}",
            report["stop"]
        );
        assert!(!bank.ledger.exists(), "a rehearsal writes no ledger");
        let (pass, reason) = verdict_of(&report).expect("a Flow leaves a verdict");
        assert!(!pass);
        assert!(
            reason
                .as_deref()
                .is_some_and(|why| why.contains("stopped at step 3")),
            "{reason:?}"
        );
        assert!(text_mentions(&report, "money_step"));
    }

    /// A guarded Flow walks only with its transaction given — the three
    /// parameters, the amount a plain number — and a confirmation that is
    /// the transaction's own; before the press the page must show the amount
    /// and the recipient, or the hand stays still.
    #[test]
    fn a_guarded_flow_needs_its_money_parameters_and_stops_when_the_page_disagrees() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pay.md").display().to_string();
        let text = money_doc(&money_flow(Policy::Guarded, Confirm::Person));
        let bench = Bench::new().showing(&[("browser-1", "https://stg.example/pay")]);
        let mut begun = None;
        for (why, extra, code, names) in [
            (
                "no parameters",
                vec![],
                error_code::FLOW_MONEY_UNBOUND,
                vec!["{{txn}}", "{{amount}}", "{{recipient}}"],
            ),
            (
                "an amount with a thousands mark",
                vec![
                    "--params",
                    r#"{"txn":"TXN-1","amount":"50,000","recipient":"Kim"}"#,
                ],
                error_code::FLOW_MONEY_UNBOUND,
                vec!["50,000"],
            ),
            (
                "a confirmation of another transaction",
                vec!["--params", PARAMS, "--confirm", "TXN-9"],
                error_code::FLOW_CONFIRM_UNBOUND,
                vec!["TXN-9", "TXN-1"],
            ),
        ] {
            let bank = Bank::new(dir.path(), true, true);
            let given = bench::command(&extra);
            let refused = run(
                &pay(&given, &text, &file, dir.path()),
                |_, argv, _| bank.road(argv),
                |_, _, _| {},
                |level| begun = Some(level),
                bench.desk(false, None),
                || true,
            )
            .expect_err(why);
            assert_eq!(refused.code, code, "{why}: {refused:?}");
            for name in names {
                assert!(refused.message.contains(name), "{why}: {refused:?}");
            }
            assert!(bank.sent.borrow().is_empty(), "{why}: nothing moved");
            assert_eq!(begun, None, "{why}: no walk began");
        }

        // Bound but unconfirmed: the walk goes up to the money step and
        // stops there with the transaction on the card and the way on.
        let bank = Bank::new(dir.path(), true, true);
        let given = bench::command(&["--params", PARAMS]);
        let report = run(
            &pay(&given, &text, &file, dir.path()),
            |_, argv, _| bank.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked to the money step");
        assert_eq!(
            bank.verbs(),
            [
                "find 송금이 완료되었습니다",
                "type",
                "type",
                "find 송금이 완료되었습니다"
            ],
            "the press waits for the person; no witness is asked before their word"
        );
        assert_eq!(report["stop"]["kind"], "money_step", "{report}");
        assert_eq!(
            (report["stoppedAt"].clone(), report["next"].clone()),
            (json!(3), json!(3))
        );
        let card = report["stop"]["message"].as_str().expect("a card");
        for word in ["TXN-1", "50000", "Kim", "--confirm TXN-1", "--start 3"] {
            assert!(card.contains(word), "the card lacks {word}: {card}");
        }
        assert!(!bank.ledger.exists(), "nothing acted, nothing written");

        // Confirmed, but the page shows another amount: the witness says so
        // and the hand never moves.
        let bank = Bank::new(dir.path(), false, true);
        let given = bench::command(&["--params", PARAMS, "--confirm", "TXN-1"]);
        let report = run(
            &pay(&given, &text, &file, dir.path()),
            |_, argv, _| bank.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked to the money step");
        assert_eq!(
            bank.verbs(),
            [
                "find 송금이 완료되었습니다",
                "type",
                "type",
                "find 50000",
                "find Kim",
                "find 송금이 완료되었습니다"
            ],
            "both witnesses asked at the pane the press aims at, then no press"
        );
        assert_eq!(report["stop"]["kind"], "step_failed", "{report}");
        assert_eq!(report["stop"]["code"], error_code::FLOW_PAGE_DISAGREES);
        assert!(
            report["stop"]["message"]
                .as_str()
                .is_some_and(|said| said.contains("50000") && said.contains("Kim")),
            "{}",
            report["stop"]
        );
        assert_eq!(report["next"], 3);
        assert!(
            bank.ledger_lines().is_empty(),
            "a disagreeing page writes no ledger line"
        );
        assert_eq!(report["ran"][2]["ok"], false);
        assert_eq!(verdict_of(&report).map(|(pass, _)| pass), Some(false));
    }

    /// The ledger line comes before the hand: `acting` is on disk when the
    /// press goes down the road, `acted` follows when the event line watching
    /// the money passes and `failed` when it does not; a transaction the
    /// ledger has seen acting or acted is refused at the money step; an auto
    /// confirmation under its cap needs nobody, over it the person.
    #[test]
    fn a_guarded_money_step_writes_acting_before_the_hand_and_acted_after_its_event_line() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("pay.md").display().to_string();
        let text = money_doc(&money_flow(Policy::Guarded, Confirm::Person));
        let bench = Bench::new().showing(&[("browser-1", "https://stg.example/pay")]);
        let bank = Bank::new(dir.path(), true, true);
        let given = bench::command(&["--params", PARAMS, "--confirm", "TXN-1"]);
        let began = std::time::Instant::now();
        let report = run(
            &pay(&given, &text, &file, dir.path()),
            |_, argv, _| bank.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        let guarded_ms = began.elapsed().as_secs_f64() * 1_000.0;
        assert_eq!(report["done"], true, "{report}");
        assert_eq!(
            bank.verbs(),
            [
                "find 송금이 완료되었습니다",
                "type",
                "type",
                "find 50000",
                "find Kim",
                "click",
                "find 송금이 완료되었습니다"
            ]
        );
        let at_press = bank
            .ledger_at_press
            .borrow()
            .clone()
            .expect("the ledger was on disk before the press");
        let acting: Vec<Line> = at_press
            .lines()
            .map(|row| serde_json::from_str(row).expect(row))
            .collect();
        assert_eq!(
            acting.iter().map(|row| row.state).collect::<Vec<_>>(),
            [State::Confirmed, State::Acting],
            "who let the hand through, then the hand: {at_press}"
        );
        assert_eq!(
            (
                acting[1].txn.as_str(),
                acting[1].amount.as_str(),
                acting[1].recipient.as_str(),
                acting[1].by,
                acting[1].step,
                acting[1].evidence.as_deref(),
            ),
            (
                "TXN-1",
                "50000",
                "Kim",
                By::Person,
                Some(3),
                Some(dir.path().display().to_string().as_str()),
            )
        );
        let lines = bank.ledger_lines();
        assert_eq!(
            lines.iter().map(|row| row.state).collect::<Vec<_>>(),
            [State::Confirmed, State::Acting, State::Acted],
            "the event line passed: acted"
        );
        assert_eq!(report["money"]["txn"], "TXN-1");
        assert_eq!(report["money"]["state"], "acted", "{}", report["money"]);
        assert_eq!(
            report["money"]["ledger"],
            bank.ledger.display().to_string(),
            "{}",
            report["money"]
        );
        assert_eq!(verdict_of(&report), Some((true, None)));
        assert!(
            report["ran"][2]["gate"].is_u64(),
            "the gate's cost rides the money step's report: {}",
            report["ran"][2]
        );

        // The same transaction again: refused at the money step, nothing pressed.
        let again = Bank::new(dir.path(), true, true);
        let report = run(
            &pay(&given, &text, &file, dir.path()),
            |_, argv, _| again.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked to the money step");
        assert_eq!(
            report["stop"]["code"],
            error_code::FLOW_TXN_SEEN,
            "{report}"
        );
        assert_eq!(report["stop"]["kind"], "step_failed");
        assert!(
            !again.verbs().iter().any(|verb| verb == "click"),
            "{:?}",
            again.verbs()
        );
        assert_eq!(bank.ledger_lines().len(), 3, "a refusal writes nothing");

        // Auto under its cap: nobody asked, the ledger says auto.
        let capped = money_doc(&money_flow(
            Policy::Guarded,
            Confirm::Auto {
                cap: "60000".into(),
            },
        ));
        let auto = Bank::new(dir.path(), true, true);
        let given = bench::command(&[
            "--params",
            r#"{"txn":"TXN-2","amount":"50000","recipient":"Kim"}"#,
        ]);
        let report = run(
            &pay(&given, &capped, &file, dir.path()),
            |_, argv, _| auto.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        assert_eq!(report["done"], true, "{report}");
        let lines = auto.ledger_lines();
        assert_eq!(lines.len(), 6, "{lines:?}");
        assert_eq!(
            lines[3..]
                .iter()
                .map(|row| (row.txn.as_str(), row.state, row.by))
                .collect::<Vec<_>>(),
            [
                ("TXN-2", State::Confirmed, By::Auto),
                ("TXN-2", State::Acting, By::Auto),
                ("TXN-2", State::Acted, By::Auto)
            ]
        );

        // Over the cap: the person's rules — the card, and no press.
        let over = Bank::new(dir.path(), true, true);
        let given = bench::command(&[
            "--params",
            r#"{"txn":"TXN-3","amount":"60000.01","recipient":"Kim"}"#,
        ]);
        let report = run(
            &pay(&given, &capped, &file, dir.path()),
            |_, argv, _| over.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked to the money step");
        assert_eq!(report["stop"]["kind"], "money_step", "{report}");
        assert!(!over.verbs().iter().any(|verb| verb == "click"));
        assert_eq!(over.ledger_lines().len(), 6);

        // The transfer never completes: the ledger says failed, the verdict
        // is not green, and the person looks.
        let stuck = Bank::new(dir.path(), true, false);
        let given = bench::command(&[
            "--params",
            r#"{"txn":"TXN-4","amount":"50000","recipient":"Kim"}"#,
        ]);
        let report = run(
            &pay(&given, &capped, &file, dir.path()),
            |_, argv, _| stuck.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        let lines = stuck.ledger_lines();
        assert_eq!(
            lines[6..].iter().map(|row| row.state).collect::<Vec<_>>(),
            [State::Confirmed, State::Acting, State::Failed],
            "{lines:?}"
        );
        assert_eq!(report["money"]["state"], "failed");
        assert_eq!(verdict_of(&report).map(|(pass, _)| pass), Some(false));

        // The gate's cost against the same walk with nothing to guard.
        let plain = money_doc(&money_flow(Policy::Dry, Confirm::Person));
        let rehearsal = Bank::new(dir.path(), true, true);
        let given = bench::command(&["--params", PARAMS]);
        let began = std::time::Instant::now();
        run(
            &pay(&given, &plain, &file, dir.path()),
            |_, argv, _| rehearsal.road(argv),
            |_, _, _| {},
            |_| {},
            bench.desk(false, None),
            || true,
        )
        .expect("walked");
        let dry_ms = began.elapsed().as_secs_f64() * 1_000.0;
        eprintln!(
            "guarded walk {guarded_ms:.3} ms (two witnesses at no wait, one ledger line before the press, one after) vs dry rehearsal {dry_ms:.3} ms"
        );
    }

    /// A Flow at `evidence: full` sends each app act without the walk's
    /// picture refusal, so the helper's look after the act rides the answer
    /// and the writer keeps it as the step's frame (t-4229); a Flow that
    /// keeps no frames (`verdict-only`, `off`) sends the refusal as before.
    /// Every act still answers an envelope, and the log keeps the line as it
    /// was sent. Twenty acts: 20 of 20 with a picture at `full`, 0 of 20 at
    /// the other levels.
    #[test]
    fn a_full_flow_walk_sends_each_act_with_its_own_picture_and_a_frameless_flow_without() {
        let spelled = |flags: &[&str]| -> Vec<String> {
            flags.iter().map(|flag| format!("--{flag}")).collect()
        };
        let (envelope, refusal) = (
            spelled(WALKED_STEP_FLAGS),
            spelled(WALKED_STEP_UNFRAMED_FLAGS),
        );
        let lines: Vec<(RecipeTool, &str)> = (0..20)
            .map(|_| (RecipeTool::Computer, "click --app Mail --x 1 --y 1"))
            .collect();
        let state = check(
            1,
            CheckKind::State,
            true,
            RecipeTool::Computer,
            &["wait-for", "--app", "Mail", "--text", "Done"],
        );
        let command = command(&[]);
        let mut pictured: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
        for level in EvidenceLevel::ALL {
            let mut spec = flow(vec![state.clone()]);
            spec.evidence = level;
            let text = flow_doc(&lines, &spec);
            let bench = Bench::new();
            let mut acts: Vec<Vec<String>> = Vec::new();
            let mut begun = None;
            let report = run(
                &walk_of(&command, &text),
                |_, argv, logged| {
                    if argv[0] == "click" {
                        assert_eq!(argv, logged, "the log keeps the line as it was sent");
                        acts.push(argv.to_vec());
                        said(&json!({ "app": { "name": "Mail", "bundleId": "com.apple.mail", "pid": 3 } }))
                    } else {
                        ok()
                    }
                },
                |_, _, _| {},
                |level| begun = Some(level),
                bench.desk(false, None),
                || true,
            )
            .expect("walked");
            assert_eq!(report["done"], true, "{report}");
            assert_eq!(begun, Some(level), "the walk began at its level");
            assert!(
                acts.iter()
                    .all(|argv| envelope.iter().all(|flag| argv.contains(flag))),
                "every act answers an envelope: {acts:?}"
            );
            let with_picture = acts
                .iter()
                .filter(|argv| !refusal.iter().any(|flag| argv.contains(flag)))
                .count();
            pictured.insert(level.as_str(), (with_picture, acts.len()));
        }
        assert_eq!(
            pictured["full"],
            (20, 20),
            "a framed walk keeps every act's picture: {pictured:?}"
        );
        assert_eq!(
            (pictured["verdict-only"], pictured["off"]),
            ((0, 20), (0, 20)),
            "a frameless walk takes none: {pictured:?}"
        );
    }

    /// A walk cut short — its first act refused — did not do what its recipe
    /// says, so its verdict is fail already (`verdict_of`); the oracle still
    /// asks every line afresh, but with the probe's zero wait, never the
    /// verify budget (t-4230): what is there now, not thirty seconds of
    /// waiting for what the walk never did. A walk that reached its end asks
    /// with the verify budget, as the test above says.
    #[test]
    fn a_walk_cut_short_judges_its_lines_from_a_zero_wait_probe() {
        let spec = flow(vec![
            check(
                1,
                CheckKind::Event,
                true,
                RecipeTool::Computer,
                &["wait-for", "--app", "Mail", "--text", "Sent"],
            ),
            check(
                2,
                CheckKind::State,
                false,
                RecipeTool::Computer,
                &["wait-for", "--app", "Mail", "--text", "Inbox"],
            ),
        ]);
        let text = flow_doc(
            &[
                (RecipeTool::Computer, "key --key a"),
                (RecipeTool::Computer, "key --key b"),
            ],
            &spec,
        );
        let command = command(&[]);
        let bench = Bench::new();
        let judged = |cut_short: bool| {
            let mut sent: Vec<Vec<String>> = Vec::new();
            let report = run(
                &walk_of(&command, &text),
                |_, argv, _| {
                    sent.push(argv.to_vec());
                    match argv[0].as_str() {
                        "key" if cut_short => refused(error_code::UNANSWERED),
                        "wait-for" => refused(error_code::TIMEOUT),
                        _ => ok(),
                    }
                },
                |_, _, _| {},
                |_| {},
                bench.desk(false, None),
                || true,
            )
            .expect("walked");
            // The baseline probe asks the event line first; the oracle asks
            // every line after the walk.
            let oracle: Vec<String> = sent
                .iter()
                .filter(|argv| argv[0] == "wait-for")
                .skip(1)
                .map(|argv| {
                    flag_value(argv, "--timeout-ms")
                        .expect("a budget")
                        .to_string()
                })
                .collect();
            (report, oracle)
        };
        let (report, oracle) = judged(true);
        assert_eq!(report["done"], false, "{report}");
        assert_eq!(report["stoppedAt"], 1);
        assert_eq!(
            oracle,
            vec![FLOW_BASELINE_PROBE_MS.to_string(); 2],
            "the oracle of a walk cut short waits nothing: {oracle:?}"
        );
        let (pass, reason) = verdict_of(&report).expect("a Flow leaves a verdict");
        assert!(!pass);
        assert!(
            reason
                .as_deref()
                .is_some_and(|why| why.contains("stopped at step 1")),
            "{reason:?}"
        );
        assert_eq!(
            report["flow"]["verdict"]["lines"][0]["status"], "fail",
            "judged afresh, from what is there: {}",
            report["flow"]
        );
        let (report, oracle) = judged(false);
        assert_eq!(report["done"], true, "{report}");
        assert_eq!(
            oracle,
            vec![FLOW_VERIFY_MS.to_string(); 2],
            "a walk that reached its end verifies with the verify budget: {oracle:?}"
        );
    }
}
