//! The reflex autopilot (t-10223 §2.4): a goal and an app in; a palette read
//! off the app's window, a plan a model writes from it with no picture of the
//! screen ([`super::plan`]), the door and the start a person's plan passes
//! ([`super::admit_plan`], [`super::launch`]), and the reflex decision
//! carried out on the run.
//!
//! ```text
//! goal, app, display → palette → plan (asked again ≤ retries) → door → start (epoch 1)
//! every collect: receipts → reading → decision → fit to carry out? (verdict)
//!   continue: nothing more    pause: stop — a new plan when the run found
//!   nothing three collects running, else the end    replan: stop → plan →
//!   door → start (a new run, the next epoch)
//! the end: the run's wall · escalated · paused · a person's hand or stop
//! ```
//!
//! What never moves: every plan, a re-plan's too, passes the door and the
//! helper's handshake; a re-plan acts in the scope the person named and no
//! other; the run's one wall is the autopilot's whole wall, and a re-plan
//! runs only the time that is left; `shadow` and `off` change no input —
//! their rows say `applied: false` and why; a person's input, the operator's
//! stop or a `reflex-stop` ends the autopilot and nothing re-plans after it;
//! no picture of the screen goes to any model. The helper's verbs are the
//! five it answers (`reflexStart`, `reflexStatus`, `reflexStop`,
//! `reflexReceipts`, `reflexAck`): a pause is a stop, and its reason is the
//! autopilot's to say (`ended`), never the helper's to be told.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::computer_use::{
    REFLEX_AUTO_L1, REFLEX_COLLECT_MS, REFLEX_ESCALATE_AFTER, REFLEX_LABEL_WINDOW_MS,
    REFLEX_MISSED_OUTCOMES, REFLEX_PLAN_LABEL_MS, REFLEX_PLAN_LEDGER, REFLEX_PRESSED_OUTCOME,
    REFLEX_REPLAN_AFTER_UNKNOWN_PASSES, REFLEX_REPLAN_COMPARE_MS,
};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::reflex::{
    ActionKind, ReflexPlan, Scope, Surface, ValidatedPlan,
};
use zerocode_core::jev::promote::names_a_schema_failure;
use zerocode_core::jev::reflex_decide::{
    self, ANSWERED, CONTINUE, KIND_KEY, LABEL_KIND, NOT_REPLANNED, PAUSE, Pending, REPLAN,
    ROAD_DOOR, ROAD_JEV, ROAD_MEMO, ROAD_SURROGATE, Running, Share, Stamp, WINDOW_CUT, WITHDRAWN,
    Why,
};
use zerocode_core::jev::summary::{AT, LABEL, REQUEST_AT};
use zerocode_core::jev::{JevMode, REFLEX_DECIDE, Run as Asking};

use super::plan::{self, Generator, Palette, Previous, Stage};
use super::{
    Admitted, Asker, Call, Carrier, DoorFacts, FileSink, ReceiptSink, Watch, admit_plan, launch,
};
use crate::computer_use::ComputerUseError;
use crate::computer_use::errand::value::{LiveWriter, Setup};
use crate::systemone::{self, Wire};

/// The key a status carries an autopilot's own account under.
pub(crate) const AUTOPILOT: &str = "autopilot";

/// Why an autopilot ended, beyond the helper's own words for a run it ended
/// or held itself (its wall, a person's input, a full queue), which pass
/// through as the helper said them: the reflex decision paused the run and
/// nothing called for a new plan; the decision stopped answering usably
/// while it was carried out; a person or the operator stopped it; the
/// helper's session went; the wall came before a new plan could run.
pub(crate) const PAUSED: &str = "paused";
pub(crate) const ESCALATED: &str = "escalated";
pub(crate) const STOPPED: &str = "request";
pub(crate) const SESSION: &str = "session";
pub(crate) const NO_TIME: &str = "deadline";

/// Where one autopilot stands, for a status and for its end (§2.4).
#[derive(Debug, Clone, Default, PartialEq)]
struct Tally {
    /// Carried-out decisions, by the road their answer came down: the memo
    /// and the stand-in are named and answer nothing yet (§2.2).
    roads: BTreeMap<&'static str, u64>,
    /// Carried-out decisions, by what they said.
    applied: BTreeMap<&'static str, u64>,
    /// Answers that came back and were not carried out, by why.
    invalid: BTreeMap<&'static str, u64>,
    /// Questions that went and brought no usable answer inside their wall.
    unanswered: u64,
    /// Questions the door refused, by its word — the person's settings.
    door: BTreeMap<String, u64>,
    /// Questions the wire failed, by its word — the network.
    wire: BTreeMap<String, u64>,
    plans: Vec<Value>,
    ended: Option<Value>,
}

impl Tally {
    fn new() -> Self {
        let zero = |words: &[&'static str]| words.iter().map(|word| (*word, 0)).collect();
        Self {
            roads: zero(&[ROAD_MEMO, ROAD_SURROGATE, ROAD_JEV]),
            applied: zero(&[CONTINUE, PAUSE, REPLAN]),
            invalid: zero(&Why::ALL.map(Why::word)),
            ..Self::default()
        }
    }
}

/// A label window still open: one answer, and whether the run it was about
/// found anything since.
#[derive(Debug, Clone, PartialEq)]
struct Open {
    run: String,
    decision: u64,
    request_at: i64,
    chosen: &'static str,
    /// The answer was carried out.
    applied: bool,
    since_ms: u64,
    saw: bool,
}

/// A carried-out re-plan's label: the old plan's last share, waiting on the
/// new plan's first.
#[derive(Debug, Clone, PartialEq)]
struct Replanned {
    run: String,
    decision: u64,
    request_at: i64,
    before: Share,
    /// The new run and the moment it started, once there is one.
    after: Option<(String, u64)>,
    /// No new plan came: there is nothing to compare.
    failed: bool,
}

/// A written plan's own label: its run's share over its first seconds,
/// named by the run in the plan ledger.
#[derive(Debug, Clone, PartialEq)]
struct Written {
    run: String,
    request_at: i64,
    since_ms: u64,
}

/// A decision to carry out after the pass that settled it.
#[derive(Debug, Clone, PartialEq)]
struct Carry {
    run: String,
    decision: u64,
    request_at: i64,
    chosen: &'static str,
}

/// What the autopilot's carrier keeps between questions: the tallies, the
/// streak of unusable answers, the decision to carry out and the label
/// windows open.
#[derive(Debug)]
struct Judge {
    tally: Tally,
    streak: u32,
    carry: Option<Carry>,
    open: Vec<Open>,
}

/// One of the autopilot's runs: its watch and its receipts' sink, the plan
/// it runs and the stamp its questions carry, and what its receipts and
/// readings said.
struct Run {
    id: String,
    stamp: Stamp,
    plan: ReflexPlan,
    /// The plan's click actions: the leaves a pressed target is counted on.
    clicks: BTreeSet<String>,
    watch: Watch,
    sink: Box<dyn ReceiptSink + Send>,
    since_ms: u64,
    /// The last receipt counted.
    counted: u64,
    pressed: u64,
    missed: u64,
    /// `(ms, pressed, missed)` at its start and at each pass.
    series: Vec<(u64, u64, u64)>,
    /// The plan the helper says it runs.
    helper_plan: Option<String>,
    outcomes: Value,
    /// Collects running whose reading found nothing.
    blind: u32,
    /// The helper's word once it ended or held the run by itself.
    helper_ended: Option<String>,
    /// This autopilot stopped it.
    stopped: bool,
    done: bool,
}

impl Run {
    fn new(id: &str, stamp: Stamp, plan: &ReflexPlan, kept: Keeping, now: u64) -> Self {
        let (evidence, sink) = kept;
        let clicks = plan
            .macros
            .iter()
            .flat_map(|item| &item.actions)
            .filter(|action| action.kind == ActionKind::Click)
            .map(|action| action.id.clone())
            .collect();
        Self {
            id: id.to_string(),
            watch: Watch::stamped(id, evidence, stamp.clone()),
            stamp,
            plan: plan.clone(),
            clicks,
            sink,
            since_ms: now,
            counted: 0,
            pressed: 0,
            missed: 0,
            series: vec![(now, 0, 0)],
            helper_plan: None,
            outcomes: json!({}),
            blind: 0,
            helper_ended: None,
            stopped: false,
            done: false,
        }
    }

    /// What one pass read of the run: its receipts counted once each —
    /// a click that landed pressed, a target the hand went for and lost
    /// missed — where the helper stands, and whether its reading found
    /// anything.
    fn observe(&mut self, read: &Value, now: u64) -> bool {
        for receipt in read
            .get("receipts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(seq) = receipt.get("seq").and_then(Value::as_u64) else {
                continue;
            };
            if seq <= self.counted {
                continue;
            }
            self.counted = seq;
            let outcome = receipt
                .get("outcome")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let action = receipt
                .get("actionId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if outcome == REFLEX_PRESSED_OUTCOME && self.clicks.contains(action) {
                self.pressed += 1;
            } else if REFLEX_MISSED_OUTCOMES.contains(&outcome) {
                self.missed += 1;
            }
        }
        self.series.push((now, self.pressed, self.missed));
        if let Some(hash) = read.get("planHash").and_then(Value::as_str) {
            self.helper_plan = Some(hash.to_string());
        }
        if let Some(outcomes) = read.get("outcomes") {
            self.outcomes = outcomes.clone();
        }
        if self.helper_ended.is_none()
            && let Some(state @ ("stopped" | "paused" | "missing")) =
                read.get("state").and_then(Value::as_str)
        {
            self.helper_ended = Some(
                read.get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or(state)
                    .to_string(),
            );
        }
        let found = !reflex_decide::finds_nothing(&reflex_decide::snapshot_of(read).state);
        self.blind = if found { 0 } else { self.blind + 1 };
        found
    }

    /// The share the run's receipts say between two moments.
    fn share_between(&self, from: u64, to: u64) -> Share {
        let at = |when: u64| {
            self.series
                .iter()
                .rev()
                .find(|(ms, _, _)| *ms <= when)
                .map_or((0, 0), |(_, pressed, missed)| (*pressed, *missed))
        };
        let (pressed_from, missed_from) = at(from);
        let (pressed_to, missed_to) = at(to);
        Share::Screen {
            pressed: pressed_to.saturating_sub(pressed_from),
            missed: missed_to.saturating_sub(missed_from),
        }
    }

    fn last_ms(&self) -> u64 {
        self.series.last().map_or(self.since_ms, |(ms, _, _)| *ms)
    }

    /// Whether the hand stood still: no press over the last collect it was
    /// read in, and not yet blind for the collects a new plan waits on — a
    /// pause about it has nothing to stop ([`reflex_decide::idle`]).
    fn quiet(&self) -> bool {
        let pressed = match self.series.as_slice() {
            [.., (_, before, _), (_, after, _)] => after > before,
            _ => false,
        };
        !pressed && self.blind < REFLEX_REPLAN_AFTER_UNKNOWN_PASSES
    }

    /// Whether the hand still stands on the run.
    fn standing(&self) -> bool {
        !self.stopped && self.helper_ended.is_none()
    }
}

/// Where a run keeps its receipts: its evidence file, and the sink.
pub(crate) type Keeping = (Option<PathBuf>, Box<dyn ReceiptSink + Send>);

/// Everything the autopilot reaches outside itself, one seam a test
/// replaces whole: the helper, the question's wire, the seat's word, the
/// generator, the two ledgers, where a run keeps its receipts, the clocks,
/// and the operator's stop as this collect read it.
pub(crate) struct World<'a> {
    pub call: Call<'a>,
    pub ask: &'a Asker,
    /// The seat's mode, from the person's settings: whether a pass asks.
    pub mode: &'a mut dyn FnMut() -> JevMode,
    /// The seat's mode and whether it applies, from one reading of the
    /// settings and the ledger ([`systemone::standing_in`]).
    pub standing: &'a mut dyn FnMut() -> (JevMode, bool),
    pub generator: &'a mut dyn Generator,
    pub decisions: &'a mut dyn FnMut(Vec<Value>),
    pub plans: &'a mut dyn FnMut(Vec<Value>),
    /// Where a run keeps its receipts: its evidence file, and the sink.
    pub keeper: &'a mut dyn FnMut(&str) -> Keeping,
    /// Milliseconds on the steady clock, and since the epoch.
    pub now_ms: u64,
    pub wall_ms: i64,
    pub stopped: Option<String>,
}

/// What the person asked for: the goal, the app, the display, the seconds
/// and whether quotas renew, and — for a bench — the decision's forced word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asked {
    pub goal: String,
    pub app: String,
    pub display: u64,
    pub seconds: u64,
    pub renew: bool,
    pub forced: Option<JevMode>,
}

impl Asked {
    /// `reflex-auto`'s words, as the parser left them.
    pub(crate) fn of(params: &Value) -> Self {
        let text = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        Self {
            goal: text("goal"),
            app: text("app"),
            display: params.get("display").and_then(Value::as_u64).unwrap_or(0),
            seconds: params.get("seconds").and_then(Value::as_u64).unwrap_or(0),
            renew: params.get("renew").and_then(Value::as_bool) == Some(true),
            forced: params
                .get("l1")
                .and_then(Value::as_str)
                .and_then(|word| REFLEX_AUTO_L1.into_iter().find(|mode| mode.key() == word)),
        }
    }
}

/// The autopilot's carrier: every question that settles on one of its runs
/// is judged here — the seat's word read once, the answer's grounds, what
/// it is carried out as — and opens the window its label is read over.
struct Seat<'a> {
    judge: &'a mut Judge,
    mode: &'a mut dyn FnMut() -> JevMode,
    standing: &'a mut dyn FnMut() -> (JevMode, bool),
    forced: Option<JevMode>,
    /// The run standing now: its id, its epoch and the plan the helper runs.
    running: Option<(String, u64, String)>,
    /// The hand on the run standing now stood still ([`Run::quiet`]).
    quiet: bool,
    now_ms: u64,
    wall_ms: i64,
}

impl Carrier for Seat<'_> {
    fn mode(&mut self) -> JevMode {
        self.forced.unwrap_or_else(|| (self.mode)())
    }

    fn now_ms(&self) -> u64 {
        self.now_ms
    }

    fn settled(&mut self, pending: &Pending, row: &mut Value) {
        // A bench's word stands as though the seat had risen (D5): its rows
        // say it was forced, and nothing is read of the person's ledger.
        let (mode, applies) = match self.forced {
            Some(mode) => (mode, mode.applies_with(true)),
            None => (self.standing)(),
        };
        row["mode"] = json!(mode.key());
        row[AT.canonical] = json!(self.wall_ms);
        let run = row["run"].as_str().unwrap_or_default().to_string();
        let outcome = row["outcome"].as_str().unwrap_or_default().to_string();
        let chosen = row.get("chosen").and_then(Value::as_str).and_then(|word| {
            [CONTINUE, PAUSE, REPLAN]
                .into_iter()
                .find(|option| *option == word)
        });
        let judge = &mut *self.judge;
        match (outcome.as_str(), chosen) {
            (ANSWERED, Some(chosen)) => {
                let running = self
                    .running
                    .as_ref()
                    .map(|(run, epoch, plan_hash)| Running {
                        run,
                        epoch: *epoch,
                        plan_hash,
                    });
                let verdict = Stamp::of(row).map_or(Err(Why::EpochMismatch), |stamp| {
                    reflex_decide::verdict(
                        &run,
                        &stamp,
                        running,
                        reflex_decide::age_at(&pending.snapshot, self.now_ms),
                        reflex_decide::idle(chosen, &pending.snapshot.state, self.quiet),
                        applies,
                    )
                });
                reflex_decide::carried(row, verdict);
                match verdict {
                    Ok(()) => {
                        *judge.tally.applied.entry(chosen).or_default() += 1;
                        *judge.tally.roads.entry(ROAD_JEV).or_default() += 1;
                        judge.streak = 0;
                        if chosen != CONTINUE {
                            judge.carry = Some(Carry {
                                run: run.clone(),
                                decision: pending.id,
                                request_at: self.wall_ms,
                                chosen,
                            });
                        }
                    }
                    Err(why) => {
                        *judge.tally.invalid.entry(why.word()).or_default() += 1;
                        // A pause with nothing to stop was a usable answer
                        // not carried out: it neither escalates nor clears.
                        judge.streak = if !applies {
                            0
                        } else if matches!(why, Why::NotAuto | Why::Idle) {
                            judge.streak
                        } else {
                            judge.streak + 1
                        };
                    }
                }
                // Every answer is graded on what the hand did after it; a
                // carried-out re-plan across its two runs, elsewhere.
                if !(verdict.is_ok() && chosen == REPLAN) {
                    judge.open.push(Open {
                        run,
                        decision: pending.id,
                        request_at: self.wall_ms,
                        chosen,
                        applied: verdict.is_ok(),
                        since_ms: self.now_ms,
                        saw: false,
                    });
                }
            }
            (WITHDRAWN, _) => {}
            _ if row["road"] == json!(ROAD_DOOR) => {
                *judge.tally.door.entry(outcome).or_default() += 1;
            }
            (word, _) if word == systemone::TIMEOUT || names_a_schema_failure(word) => {
                judge.tally.unanswered += 1;
                judge.streak = if applies { judge.streak + 1 } else { 0 };
            }
            _ => *judge.tally.wire.entry(outcome).or_default() += 1,
        }
    }
}

/// One autopilot: the goal and the scope it acts in, what it read of the
/// app's window, its runs, and the reflex decision's account of them.
pub(crate) struct Autopilot {
    id: String,
    asked: Asked,
    scope: Scope,
    stage: Stage,
    palette: Palette,
    workspace: Option<PathBuf>,
    facts: DoorFacts,
    /// The steady moment the whole autopilot's wall comes.
    deadline_ms: u64,
    epoch: u64,
    current: Option<Run>,
    /// Runs stopped or ended: their watch until it has every receipt, and
    /// their readings until the labels are written.
    finishing: Vec<Run>,
    judge: Judge,
    replanned: Vec<Replanned>,
    written: Vec<Written>,
    stop: Arc<AtomicBool>,
}

/// An autopilot as the window keeps it: its stop, the run it stands on, its
/// account, and every run it started.
struct Registered {
    stop: Arc<AtomicBool>,
    current: Option<String>,
    report: Value,
    runs: Vec<String>,
}

fn registry() -> &'static Mutex<BTreeMap<String, Registered>> {
    static AUTOPILOTS: OnceLock<Mutex<BTreeMap<String, Registered>>> = OnceLock::new();
    AUTOPILOTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// The account of the autopilot that started `run`, if one did.
pub(crate) fn report(run: &str) -> Option<Value> {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .find(|registered| registered.runs.iter().any(|owned| owned == run))
        .map(|registered| registered.report.clone())
}

/// A stop named one of an autopilot's runs: the autopilot ends at its next
/// collect and writes no plan after it. Answers the run it stands on now,
/// to be stopped at once — `None` for a run no autopilot started.
pub(crate) fn stop_owner(run: &str) -> Option<Option<String>> {
    registry()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .values()
        .find(|registered| registered.runs.iter().any(|owned| owned == run))
        .map(|registered| {
            registered.stop.store(true, Ordering::SeqCst);
            registered.current.clone()
        })
}

/// An autopilot's id: unique in this window.
fn new_autopilot_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    format!(
        "ra-{}-{}",
        crate::project_runtime::now_epoch_ms(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

impl Autopilot {
    /// The autopilot's first plan, run: a generator a person set up, the
    /// door's own questions, the app's window and its palette, the plan the
    /// model writes — asked again with the window's refusal while the
    /// retries last — and the door and the start every plan passes. Nothing
    /// is asked of the helper, the screen or the generator before a
    /// generator is set up and the door is open.
    ///
    /// # Errors
    ///
    /// Why the autopilot did not start — the generator, the door, the app,
    /// the plan — before any hand moved.
    pub(crate) fn start(
        asked: Asked,
        facts: DoorFacts,
        workspace: Option<PathBuf>,
        world: &mut World<'_>,
    ) -> Result<(Self, Value), ComputerUseError> {
        if let Some(why) = world.generator.unready() {
            return Err(ComputerUseError::new(
                error_code::UNSUPPORTED_CAPABILITY,
                format!(
                    "{}: the reflex autopilot writes its plans with the generator the Computer Use pane sets up — the Claude or Codex login the window runs its panes with, or a key the person chose — and no road of it can answer ({why}); reflex-start runs a plan of your own",
                    plan::NO_GENERATOR
                ),
            ));
        }
        super::door_opens(&facts)?;
        let displays = (world.call)("displays", json!({}))?;
        let windows = (world.call)("listWindows", json!({ "app": asked.app }))?;
        let (stage, target) = plan::stage_of(&displays, &windows, asked.display)
            .map_err(ComputerUseError::invalid_argument)?;
        let frame = (world.call)("screenshotDesktop", json!({ "display": asked.display }))?;
        let image = crate::computer_use::screenshot_png(&frame)
            .and_then(|png| crate::computer_use::compare::decode_png(&png))
            .ok_or_else(|| {
                ComputerUseError::new(
                    error_code::SCREENSHOT_FAILED,
                    "the display's capture does not read",
                )
            })?;
        let palette = plan::palette_of(&image, &stage).ok_or_else(|| {
            ComputerUseError::invalid_argument(
                "the app's window shows one colour, or none this capture shows: there is nothing for a plan to find",
            )
        })?;
        let mut autopilot = Self {
            id: new_autopilot_id(),
            scope: Scope {
                surface: Surface::MacosDesktop,
                target,
            },
            deadline_ms: world
                .now_ms
                .saturating_add(asked.seconds.saturating_mul(1_000)),
            asked,
            stage,
            palette,
            workspace,
            facts,
            epoch: 0,
            current: None,
            finishing: Vec::new(),
            judge: Judge {
                tally: Tally::new(),
                streak: 0,
                carry: None,
                open: Vec::new(),
            },
            replanned: Vec::new(),
            written: Vec::new(),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let answer = autopilot.plan_and_start(world, None)?;
        autopilot.publish();
        Ok((autopilot, answer))
    }

    /// Ask for a plan and start it: the model's plan, the door, the helper's
    /// start, a new run under the next epoch — and the plan's ledger row,
    /// whatever became of it.
    fn plan_and_start(
        &mut self,
        world: &mut World<'_>,
        previous: Option<Previous<'_>>,
    ) -> Result<Value, ComputerUseError> {
        let no_time = || {
            ComputerUseError::new(
                NO_TIME,
                "no second of the run's wall is left for another plan",
            )
        };
        if self.deadline_ms.saturating_sub(world.now_ms) < 1_000 {
            return Err(no_time());
        }
        let written = plan::write_plan(
            world.generator,
            &plan::Ask {
                goal: &self.asked.goal,
                scope: &self.scope,
                stage: &self.stage,
                palette: &self.palette,
                previous,
            },
        );
        // The wall counts from the first plan's start: the person's seconds
        // are seconds of acting. A later plan runs what is left once it is
        // written.
        let written_at = world.now_ms.saturating_add(written.rtt_ms);
        if self.epoch == 0 {
            self.deadline_ms = written_at.saturating_add(self.asked.seconds.saturating_mul(1_000));
        }
        let seconds = self.deadline_ms.saturating_sub(written_at) / 1_000;
        self.epoch += 1;
        let model = world.generator.model();
        let wall_ms = world.wall_ms;
        let row = |run: Option<&str>, outcome: &str| {
            plan::ledger_row(
                wall_ms,
                run,
                self.epoch,
                &self.asked.goal,
                &self.palette,
                model.as_deref(),
                &written,
                outcome,
            )
        };
        let validated = match &written.plan {
            Ok(_) if seconds == 0 => {
                (world.plans)(vec![row(None, NO_TIME)]);
                return Err(no_time());
            }
            Ok(validated) => validated.clone(),
            Err(word) => {
                (world.plans)(vec![row(None, word)]);
                return Err(ComputerUseError::new(
                    word.clone(),
                    format!(
                        "no plan passed the contract after {} requests: {}",
                        written.requests,
                        written
                            .refusals
                            .last()
                            .map_or("the generator did not answer", String::as_str)
                    ),
                ));
            }
        };
        let started = self
            .admitted(validated, seconds)
            .and_then(|admitted| launch(&admitted, world.call).map(|answer| (answer, admitted)));
        let (answer, admitted) = match started {
            Ok(started) => started,
            Err(refusal) => {
                (world.plans)(vec![row(None, &refusal.code)]);
                return Err(refusal);
            }
        };
        let id = answer
            .get("runId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        (world.plans)(vec![row(Some(&id), ANSWERED)]);
        let plan = admitted.plan.plan();
        let kept = (world.keeper)(&id);
        let evidence = kept.0.clone();
        let stamp = Stamp {
            epoch: self.epoch,
            plan_hash: plan.plan_hash.clone(),
            forced: self.asked.forced.is_some(),
        };
        self.judge.tally.plans.push(json!({
            "run": id,
            "epoch": self.epoch,
            "planHash": plan.plan_hash,
            "source": written.source,
            "promptVersion": plan::PROMPT_VERSION,
            "requests": written.requests,
            "rttMs": written.rtt_ms,
            "refusals": written.refusals.len(),
        }));
        self.written.push(Written {
            run: id.clone(),
            request_at: wall_ms,
            since_ms: world.now_ms,
        });
        for replanned in self
            .replanned
            .iter_mut()
            .filter(|replanned| replanned.after.is_none())
        {
            replanned.after = Some((id.clone(), world.now_ms));
        }
        if let Some(old) = self.current.take() {
            self.finishing.push(old);
        }
        self.current = Some(Run::new(&id, stamp, plan, kept, world.now_ms));
        Ok(json!({
            "runId": id,
            "state": answer.get("state").cloned().unwrap_or(Value::Null),
            AUTOPILOT: self.id,
            "epoch": self.epoch,
            "planHash": plan.plan_hash,
            "renew": admitted.policy.renew,
            "runNs": admitted.policy.run_ns,
            "evidence": evidence,
            "plan": {
                "requests": written.requests,
                "refusals": written.refusals,
                "rttMs": written.rtt_ms,
            },
        }))
    }

    /// The door every plan passes, for the seconds left of the wall — the
    /// operator's stop read as it stands now — and the plan written beside
    /// the run's receipts, in the sections a Flow document carries.
    fn admitted(&self, plan: ValidatedPlan, seconds: u64) -> Result<Admitted, ComputerUseError> {
        if let Some(workspace) = &self.workspace {
            let _ = std::fs::write(
                workspace.join(format!("reflex-auto-{}-{}.md", self.id, self.epoch)),
                plan.plan().written_sections(),
            );
        }
        admit_plan(
            plan,
            &json!({
                "seconds": seconds,
                "renew": self.asked.renew,
                "display": self.asked.display,
            }),
            self.workspace.clone(),
            &DoorFacts {
                stopped: super::super::guard::stopped_reason()
                    .or_else(|| self.facts.stopped.clone()),
                ..self.facts.clone()
            },
        )
    }

    /// Why the autopilot ended, once it has: `{reason, said}`.
    #[cfg(test)]
    pub(crate) fn ended(&self) -> Option<&Value> {
        self.judge.tally.ended.as_ref()
    }

    /// End the autopilot: the run standing now is stopped, a decision not
    /// yet carried out never is, and a re-plan with no new run has nothing
    /// to be compared with.
    fn end(&mut self, world: &mut World<'_>, reason: &str, said: &str) {
        if self.judge.tally.ended.is_some() {
            return;
        }
        self.judge.tally.ended = Some(json!({ "reason": reason, "said": said }));
        self.judge.carry = None;
        if let Some(mut run) = self.current.take() {
            if !run.stopped {
                let _ = (world.call)("reflexStop", json!({ "run": run.id }));
                run.stopped = true;
            }
            self.finishing.push(run);
        }
        for replanned in &mut self.replanned {
            if replanned.after.is_none() {
                replanned.failed = true;
            }
        }
    }

    /// One collect: every run's watch passes — a stopped run's until its
    /// receipts are on disk — the decision the pass settled carried out, the
    /// escalation counted, the labels whose window closed written. True
    /// once the autopilot ended and every run's watch is done.
    pub(crate) fn tick(&mut self, world: &mut World<'_>) -> bool {
        if self.judge.tally.ended.is_none() {
            if self.stop.load(Ordering::SeqCst) {
                self.end(world, STOPPED, "a person stopped the autopilot");
            } else if let Some(reason) = world.stopped.clone() {
                self.end(world, &reason, "the operator is stopped");
            }
        }
        self.pass_every_run(world);
        if self.judge.tally.ended.is_none()
            && let Some(reason) = self
                .current
                .as_ref()
                .and_then(|run| run.helper_ended.clone())
        {
            // The helper ended or held the run by itself — its wall, a
            // person's input, a full queue — and nothing re-plans after it.
            self.end(world, &reason, "the helper ended the run");
        }
        if let Some(carry) = self.judge.carry.take()
            && self.judge.tally.ended.is_none()
            && self.current.as_ref().is_some_and(|run| run.id == carry.run)
        {
            self.carry_out(world, &carry);
        }
        if self.judge.tally.ended.is_none() && self.judge.streak >= REFLEX_ESCALATE_AFTER {
            self.end(
                world,
                ESCALATED,
                "the reflex decision came back unanswered or unfit to carry out three times running while it was carried out: the run stopped for a person",
            );
        }
        let done = self.judge.tally.ended.is_some()
            && self.current.is_none()
            && self.finishing.iter().all(|run| run.done);
        self.close_labels(world, done);
        self.publish();
        done
    }

    fn pass_every_run(&mut self, world: &mut World<'_>) {
        let running = self
            .current
            .as_ref()
            .filter(|run| run.standing())
            .map(|run| {
                (
                    run.id.clone(),
                    run.stamp.epoch,
                    run.helper_plan
                        .clone()
                        .unwrap_or_else(|| run.stamp.plan_hash.clone()),
                )
            });
        let forced = self.asked.forced;
        let quiet = self.current.as_ref().is_some_and(Run::quiet);
        let runs = self
            .finishing
            .iter_mut()
            .filter(|run| !run.done)
            .chain(self.current.as_mut());
        for run in runs {
            let mut seat = Seat {
                judge: &mut self.judge,
                mode: &mut *world.mode,
                standing: &mut *world.standing,
                forced,
                running: running.clone(),
                quiet,
                now_ms: world.now_ms,
                wall_ms: world.wall_ms,
            };
            let passed = run.watch.pass_carried(
                &mut *world.call,
                run.sink.as_mut(),
                &mut seat,
                world.ask,
                &mut *world.decisions,
            );
            if let Some(read) = &passed.read
                && run.observe(read, world.now_ms)
            {
                for open in self.judge.open.iter_mut().filter(|open| open.run == run.id) {
                    open.saw = true;
                }
            }
            run.done = passed.done;
            super::publish(&run.id, run.watch.report().clone());
        }
    }

    /// Carry out a decision about the run standing now: a pause stops it —
    /// and hands it to a new plan when it found nothing for the collects
    /// the table names — a re-plan stops it and runs the next plan.
    fn carry_out(&mut self, world: &mut World<'_>, carry: &Carry) {
        let Some(mut run) = self.current.take() else {
            return;
        };
        let _ = (world.call)("reflexStop", json!({ "run": run.id }));
        run.stopped = true;
        let blind = run.blind >= REFLEX_REPLAN_AFTER_UNKNOWN_PASSES;
        if carry.chosen == REPLAN {
            let to = run.last_ms();
            self.replanned.push(Replanned {
                run: carry.run.clone(),
                decision: carry.decision,
                request_at: carry.request_at,
                before: run.share_between(to.saturating_sub(REFLEX_REPLAN_COMPARE_MS), to),
                after: None,
                failed: false,
            });
        }
        let (plan, outcomes) = (run.plan.clone(), run.outcomes.clone());
        self.finishing.push(run);
        if carry.chosen == PAUSE && !blind {
            self.end(
                world,
                PAUSED,
                "the reflex decision paused the run while its detectors still found something: it waits for a person",
            );
            return;
        }
        let previous = Previous {
            plan: &plan,
            outcomes: &outcomes,
        };
        if let Err(refusal) = self.plan_and_start(world, Some(previous)) {
            self.end(world, &refusal.code, &refusal.message);
        }
    }

    /// The run named `id`, standing or finished.
    fn run(&self, id: &str) -> Option<&Run> {
        self.current
            .iter()
            .chain(&self.finishing)
            .find(|run| run.id == id)
    }

    /// Write every label whose window closed — or all of them, once the
    /// autopilot is done: a decision's on what its run did in the window
    /// after it, a re-plan's on its two runs, a plan's on its run's first
    /// seconds.
    fn close_labels(&mut self, world: &mut World<'_>, all: bool) {
        let now = world.now_ms;
        let mut decided = Vec::new();
        let mut open = Vec::new();
        for window in std::mem::take(&mut self.judge.open) {
            let run = self.run(&window.run);
            let standing = run.is_some_and(Run::standing);
            let due = now >= window.since_ms + REFLEX_LABEL_WINDOW_MS;
            if standing && !due && !all {
                open.push(window);
                continue;
            }
            let share = run.map_or(
                Share::Screen {
                    pressed: 0,
                    missed: 0,
                },
                |run| {
                    run.share_between(window.since_ms, if standing { now } else { run.last_ms() })
                },
            );
            // A window a run's end cut short grades nothing — unless the end
            // was the answer's own: a pause carried out.
            let marks = if standing || (window.applied && window.chosen == PAUSE) {
                reflex_decide::marks_on(
                    window.chosen,
                    &reflex_decide::Window {
                        share,
                        wrong: 0,
                        blind: !window.saw,
                        live: standing,
                    },
                )
            } else {
                Err(WINDOW_CUT)
            };
            decided.push(reflex_decide::label_row(
                &window.run,
                window.decision,
                window.request_at,
                share.json(),
                marks,
            ));
        }
        self.judge.open = open;
        let mut replanned = Vec::new();
        for label in std::mem::take(&mut self.replanned) {
            let (marks, measured) = match &label.after {
                Some((after, since)) => {
                    let run = self.run(after);
                    if run.is_some_and(Run::standing)
                        && now < since + REFLEX_REPLAN_COMPARE_MS
                        && !all
                    {
                        replanned.push(label);
                        continue;
                    }
                    let after = run.map_or(
                        Share::Screen {
                            pressed: 0,
                            missed: 0,
                        },
                        |run| run.share_between(*since, since + REFLEX_REPLAN_COMPARE_MS),
                    );
                    (
                        reflex_decide::replan_marks(label.before, after),
                        json!({ "before": label.before.json(), "after": after.json() }),
                    )
                }
                None if label.failed || all => {
                    (Err(NOT_REPLANNED), json!({ "before": label.before.json() }))
                }
                None => {
                    replanned.push(label);
                    continue;
                }
            };
            decided.push(reflex_decide::label_row(
                &label.run,
                label.decision,
                label.request_at,
                measured,
                marks,
            ));
        }
        self.replanned = replanned;
        if !decided.is_empty() {
            (world.decisions)(decided);
        }
        let mut written = Vec::new();
        let mut plans = Vec::new();
        for label in std::mem::take(&mut self.written) {
            let run = self.run(&label.run);
            let due = now >= label.since_ms + REFLEX_PLAN_LABEL_MS;
            if run.is_some_and(Run::standing) && !due && !all {
                written.push(label);
                continue;
            }
            let share = run.map_or(
                Share::Screen {
                    pressed: 0,
                    missed: 0,
                },
                |run| run.share_between(label.since_ms, label.since_ms + REFLEX_PLAN_LABEL_MS),
            );
            plans.push(json!({
                (AT.canonical): world.wall_ms,
                (LABEL.canonical): label.run,
                (REQUEST_AT.canonical): label.request_at,
                (KIND_KEY): LABEL_KIND,
                "share": share.json(),
                "cut": !due,
            }));
        }
        self.written = written;
        if !plans.is_empty() {
            (world.plans)(plans);
        }
    }

    /// The autopilot's account, as a status of any of its runs carries it.
    pub(crate) fn rendered(&self) -> Value {
        let tally = &self.judge.tally;
        json!({
            "id": self.id,
            "goalHash": zerocode_core::jev::fingerprint_of(&self.asked.goal),
            "scope": self.scope,
            "current": self.current.as_ref().map(|run| json!({
                "run": run.id, "epoch": run.stamp.epoch, "planHash": run.stamp.plan_hash,
            })),
            "l1": { "forced": self.asked.forced.map(JevMode::key) },
            "roads": tally.roads,
            "applied": tally.applied,
            "invalid": tally.invalid,
            "unanswered": tally.unanswered,
            "door": tally.door,
            "wire": tally.wire,
            "plans": tally.plans,
            "ended": tally.ended,
        })
    }

    fn publish(&self) {
        registry()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                self.id.clone(),
                Registered {
                    stop: Arc::clone(&self.stop),
                    current: self.current.as_ref().map(|run| run.id.clone()),
                    report: self.rendered(),
                    runs: self
                        .finishing
                        .iter()
                        .chain(&self.current)
                        .map(|run| run.id.clone())
                        .collect(),
                },
            );
    }

    /// The helper's session went: nothing more can be read of any run, and
    /// the autopilot ends on it.
    fn lost_the_session(&mut self) {
        if self.judge.tally.ended.is_none() {
            self.judge.tally.ended = Some(json!({
                "reason": SESSION, "said": "the helper's session ended",
            }));
        }
        self.publish();
    }
}

/// `reflex-auto`, as the window answers it: the first plan run on this
/// thread — its refusals said at once — and the autopilot then carried on a
/// thread of its own, every collect, while the helper's session stands.
///
/// # Errors
///
/// Why nothing started.
pub(crate) fn begin(
    params: &Value,
    facts: DoorFacts,
    generator: Setup,
    call: Call<'_>,
) -> Result<Value, ComputerUseError> {
    let asked = Asked::of(params);
    let workspace = super::super::evidence::session_dir(crate::project_runtime::now_epoch_ms());
    let wire = Wire::of_this_machine();
    let ask = super::asker(wire.clone(), workspace.clone());
    let setup = generator;
    let (autopilot, answer) = {
        let mut generator = LiveWriter::window(setup.clone());
        let mut roads = Roads::of(&wire, workspace.clone());
        let mut world = roads.world(call, &ask, &mut generator);
        Autopilot::start(asked, facts, workspace.clone(), &mut world)?
    };
    std::thread::spawn(move || {
        let mut autopilot = autopilot;
        let mut generator = LiveWriter::window(setup);
        let mut roads = Roads::of(&wire, workspace);
        loop {
            if !super::super::session_stands() {
                autopilot.lost_the_session();
                return;
            }
            let mut call = |method: &str, params: Value| super::super::call(method, params);
            let mut world = roads.world(&mut call, &ask, &mut generator);
            if autopilot.tick(&mut world) {
                return;
            }
            std::thread::sleep(Duration::from_millis(REFLEX_COLLECT_MS));
        }
    });
    Ok(answer)
}

/// The window's own roads for an autopilot: the seat's word and standing
/// read off this machine's settings and ledger, the reflex decision's
/// ledger and the plans' beside it, and each run's receipts in the evidence
/// folder.
struct Roads {
    mode: Box<dyn FnMut() -> JevMode + Send>,
    standing: Box<dyn FnMut() -> (JevMode, bool) + Send>,
    decisions: Box<dyn FnMut(Vec<Value>) + Send>,
    plans: Box<dyn FnMut(Vec<Value>) + Send>,
    keeper: Box<dyn FnMut(&str) -> Keeping + Send>,
}

impl Roads {
    fn of(wire: &Wire, workspace: Option<PathBuf>) -> Self {
        let decisions = systemone::ledger_of(wire, &REFLEX_DECIDE);
        let plans = decisions
            .as_deref()
            .and_then(std::path::Path::parent)
            .map(|dir| dir.join(REFLEX_PLAN_LEDGER));
        let (for_mode, for_standing) = (wire.clone(), wire.clone());
        Self {
            mode: Box::new(move || {
                REFLEX_DECIDE.mode_in_run(&for_mode.settings_root(), Asking::Fresh)
            }),
            standing: Box::new(move || {
                systemone::standing_in(&for_standing, &REFLEX_DECIDE, Asking::Fresh)
            }),
            decisions: Box::new(move |rows| super::record_decisions(decisions.as_deref(), rows)),
            plans: Box::new(move |rows| {
                if let Some(ledger) = &plans {
                    systemone::append_rows(ledger, &rows);
                }
            }),
            keeper: Box::new(move |run| {
                let evidence = workspace
                    .as_deref()
                    .map(|dir| super::receipts_file(dir, run));
                (
                    evidence.clone(),
                    Box::new(FileSink(evidence)) as Box<dyn ReceiptSink + Send>,
                )
            }),
        }
    }

    fn world<'a>(
        &'a mut self,
        call: Call<'a>,
        ask: &'a Asker,
        generator: &'a mut dyn Generator,
    ) -> World<'a> {
        World {
            call,
            ask,
            mode: &mut *self.mode,
            standing: &mut *self.standing,
            generator,
            decisions: &mut *self.decisions,
            plans: &mut *self.plans,
            keeper: &mut *self.keeper,
            now_ms: super::steady_ms(),
            wall_ms: crate::project_runtime::now_epoch_ms(),
            stopped: super::super::guard::stopped_reason(),
        }
    }
}

#[cfg(test)]
mod tests;
