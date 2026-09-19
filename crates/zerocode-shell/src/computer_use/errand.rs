//! Walking a screen by judgment (docs/design/jev-browser-action-20260917.md
//! §2.3): the screen in front of a walk already numbers every control a person
//! could press, so one of those numbers — chosen by a judgment, never invented
//! by one — is pressed, and what follows is looked at again.
//!
//! It was written as `recover`, for one errand only: a recorded flow that
//! stopped because a step or a check failed. It is named `errand` now because
//! a second errand asks the same question of the same screen, and a module
//! named after the first of two would have to lie about the other:
//!
//! - [`Why::Cleared`] — a recorded walk stopped. One press should let the
//!   stopped step run again, and the document's own re-walk says whether it
//!   did. Two presses at most: a third guess on a screen that did not move
//!   twice is a loop, not a recovery.
//! - [`Why::Goal`] — nothing failed. A goal was named in a person's own words
//!   at a point where the procedure BRANCHES — where what to press next cannot
//!   be written down in advance because it depends on what the screen came
//!   back with. The deterministic run before and after it is the caller's
//!   (a batch, a recipe); this is the one step of it that needed reading.
//!
//! The shape is the repeat's ([`super::repeat`]): this module decides WHETHER
//! and WITH WHAT, and the world it acts on is one narrow seam ([`World`]) a
//! test replaces whole. It walks no step itself — pressing goes down the same
//! door, through the same pin and the same fingerprint, as every other press —
//! and it invents no resume point: for a stopped walk, the step to walk again
//! from is the one the report already computed (`RecipeStop::resumes_after`).
//!
//! Four things are load-bearing and each has a test of its own:
//!
//! 1. **A stop is cleared only where clearing means what it says.**
//!    `StepFailed` and `CheckFailed` resume AT the step that stopped. Every
//!    other stop either belongs to a person, was set by one, or resumes after
//!    the stopped step — and walking on from there after an unrelated press
//!    would skip work the recipe asked for. A goal is not a stop and is not
//!    read through that gate at all.
//! 2. **Money is never walked by judgment.** A document that moves money is
//!    barred whatever its policy, a `guarded` Flow is barred whatever its
//!    steps, and a goal walk — which has no document to read a money step out
//!    of — is barred from the money door by the same gate, since it reaches
//!    the door through the same [`barred`].
//! 3. **Pressing is not succeeding.** The row says what was chosen and, apart
//!    from it, whether what followed got past what was in the way.
//! 4. **A screen that does not move is where a walk stops.** Every errand
//!    keeps it: the numbers already spent are the numbers spent ON THIS
//!    SCREEN, they are offered again the moment the screen moves, and two
//!    presses that change nothing end the walk. That, not the step budget, is
//!    what makes an unattended walk terminate.

use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::computer_flow::{FlowSpec, Policy};
use zerocode_core::computer_recipe::{RecipeLine, RecipeStop, RecipeTool};
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
use zerocode_core::jev::{BROWSER, DESKTOP, JevMode, JevUse};
use zerocode_core::screen_action::{
    ActionAsk, ActionChoice, ActionLook, Chosen, SCREEN_ACTION_RUBRIC_VERSION, Where, ask,
};

/// How long one judgment may hold a walk. The same number the router's
/// applied mode waits: a judgment that has not answered by then is slower
/// than the fallback it would replace.
pub const ACTION_DEADLINE: Duration = Duration::from_millis(1_500);

/// How many numbers one walk may spend clearing one stop. Two, because a
/// third guess on a screen that did not move twice is a loop, not a recovery.
pub const MAX_RECOVERY_ATTEMPTS: usize = 2;

/// How many presses in a row may leave the screen exactly as it was before a
/// walk gives up on it. Two — [`MAX_RECOVERY_ATTEMPTS`]' own reason, kept for
/// every errand: a second guess is worth making, a third on a screen that has
/// not moved twice is a loop. How many presses a goal walk may spend in all
/// is the verb's, measured and clamped there
/// (`zerocode_core::computer_use::WALK_STEPS_DEFAULT`); this is the rule that
/// ends a walk before that number is anywhere near reached.
pub const SAME_SCREEN_LIMIT: usize = MAX_RECOVERY_ATTEMPTS;

/// The seat a surface's judgments sit in, and whose ledger they are written
/// to — the Jev use table's rows, never a name spelled here.
#[must_use]
pub const fn seat_of(surface: Surface) -> &'static JevUse {
    match surface {
        Surface::Page => &BROWSER,
        Surface::Desk => &DESKTOP,
    }
}

/// Which screen a walk is looking at. The two have separate seats on the
/// settings card because what they send is different in kind, not because the
/// question differs — it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    /// A browser pane.
    Page,
    /// The desktop's accessibility tree.
    Desk,
}

/// What the person set for this surface's seat. The ladder is the routing
/// card's, because it is the same question: does anything go to the vendor,
/// and may it change what the product does. `off` asks nothing and is what an
/// unknown word reads as; `shadow` and `auto` ask and record while the walk
/// goes exactly as it would have — nothing promotes a press — and `on`
/// presses what it chose.
pub type Mode = JevMode;

/// What a judgment answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Judged {
    /// A validated choice over the set that was offered.
    Chose(ActionChoice),
    /// Nothing usable, named by the ledger's own failure token
    /// (`no_key`, `timeout`, `schema`, `unauthorized`, `http_503`, …).
    Refused(String),
}

/// What asking a judgment cost at the Jev door — the wire's own account.
pub use crate::systemone::Spent;

/// Choosing one of the numbers a look handed out — the seam the window's
/// System One wire sits behind and a test replaces.
pub trait ActionJudge {
    fn choose(&mut self, ask: &ActionAsk) -> Judged;

    /// What the last [`Self::choose`] sent through the Jev door, for the row.
    /// A judge that sends nowhere — a test's — has nothing to say.
    fn spent(&self) -> Option<Spent> {
        None
    }
}

/// The address of the screen a walk is looking at, owned. The borrowed twin
/// the question reads is [`Where`]; this is what a world hands back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seen {
    Page { host: String, path: String },
    Desk { app: String, window: String },
}

impl Seen {
    fn asked(&self) -> Where<'_> {
        match self {
            Self::Page { host, path } => Where::Page { host, path },
            Self::Desk { app, window } => Where::Desk { app, window },
        }
    }
}

impl Default for Seen {
    fn default() -> Self {
        Self::Page {
            host: String::new(),
            path: String::new(),
        }
    }
}

/// The screen a walk is looking at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Screen {
    pub at: Seen,
    pub items: Vec<Value>,
}

impl Screen {
    /// Whether two looks show the same screen — the same words, in the same
    /// order, at the same address.
    ///
    /// The comparison is the question's own view of the screen (the legend
    /// line each control is described by), not the raw items: a caret that
    /// blinked or a pixel that moved is not a screen that moved, and reading
    /// the raw items would make every look different and quietly disable the
    /// rule that ends an unattended walk.
    #[must_use]
    pub fn same_as(&self, other: &Self) -> bool {
        use zerocode_core::computer_use_protocol::marks::legend_line;
        self.at == other.at
            && self.items.len() == other.items.len()
            && self
                .items
                .iter()
                .zip(&other.items)
                .all(|(mine, theirs)| legend_line(mine) == legend_line(theirs))
    }
}

/// What a walk does to the world, and nothing else — each a seam.
pub trait World {
    /// The screen and the controls it is showing now, or `None` when the
    /// screen cannot be read (a walk that cannot see does not press).
    fn look(&mut self) -> Option<Screen>;
    /// Press one number down the ordinary door. `true` when the door took it;
    /// a stale pin, a host outside the recording and a shut door are all
    /// `false`, and none of them is retried.
    fn press(&mut self, mark: usize) -> bool;
    /// Milliseconds the call has left.
    fn left_ms(&mut self) -> u64;
    /// Walk the document again from `step`, answering the report. Only a
    /// [`Why::Cleared`] errand asks this, and only a world that HAS a
    /// document can answer it; a goal walk's world has none and keeps the
    /// answer this default gives.
    fn walk_from(&mut self, step: usize) -> Option<Value> {
        let _ = step;
        None
    }
    /// Whether the caller's own success condition is met — the deterministic
    /// one written down before the walk started, asked of the screen and
    /// never of a judgment. A world whose caller named no condition answers
    /// `None`, and then a goal walk has only the judgment's own `done` to end
    /// on, which is the weaker of the two and says so in the row.
    fn reached(&mut self) -> Option<bool> {
        None
    }
}

/// Why a walk is asking the screen anything.
#[derive(Debug, Clone, Copy)]
pub enum Why<'a> {
    /// A recorded walk stopped here.
    Cleared {
        stop: RecipeStop,
        /// The stopped step's VERB alone; a `type` line's argv carries a value.
        step: &'a str,
        refusal: &'a str,
        /// The step to walk again from — the report's own `next`.
        next: usize,
    },
    /// A goal named at a branch in an otherwise deterministic procedure.
    Goal {
        /// The most presses this walk may spend, as the caller asked and the
        /// verb's parser already clamped.
        steps: usize,
    },
}

/// What a walk is asking about, as a judgment reads it.
#[derive(Debug, Clone, Copy)]
pub struct Errand<'a> {
    /// What the walk is for — the flow's name and the check that stopped, or
    /// the sentence a person wrote.
    pub goal: &'a str,
    pub why: Why<'a>,
    pub flow: Option<&'a FlowSpec>,
    /// Whether the document has a money step at all.
    pub moves_money: bool,
}

impl Errand<'_> {
    /// The most presses this errand may spend.
    #[must_use]
    pub const fn steps(&self) -> usize {
        match self.why {
            Why::Cleared { .. } => MAX_RECOVERY_ATTEMPTS,
            Why::Goal { steps } => steps,
        }
    }

    /// The word a row uses for this errand.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        match self.why {
            Why::Cleared { .. } => "clear",
            Why::Goal { .. } => "goal",
        }
    }

    /// The errand as the question reads it.
    fn asked(&self) -> zerocode_core::screen_action::Errand<'_> {
        match self.why {
            Why::Cleared {
                stop,
                step,
                refusal,
                ..
            } => zerocode_core::screen_action::Errand::Clear {
                stopped: stop.as_str(),
                step,
                refusal,
            },
            Why::Goal { .. } => zerocode_core::screen_action::Errand::Goal,
        }
    }
}

/// Why a walk was not judged at all. Each is a row the ledger writes rather
/// than a silence, so a person can see the gate held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Barred {
    /// Nobody asked for it.
    Off,
    /// This stop is a person's, was set by one, or resumes past its step.
    NotRecoverable,
    /// The document moves money.
    MovesMoney,
    /// The Flow presses past its own gate.
    Guarded,
    /// No step to walk again from.
    NoResume,
    /// Not enough of the call's clock left to ask and still act.
    NoBudget,
    /// The caller asked for no presses at all.
    NoSteps,
}

impl Barred {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => JevMode::Off.key(),
            Self::NotRecoverable => "not_recoverable",
            Self::MovesMoney => "moves_money",
            Self::Guarded => "guarded",
            Self::NoResume => "no_resume",
            Self::NoBudget => "no_budget",
            Self::NoSteps => "no_steps",
        }
    }
}

/// Whether this errand may be walked at all, before anything is looked at or
/// asked. Pure: the whole gate reads its answer from here, and both errands
/// read it from the same place — which is how a goal walk cannot quietly
/// acquire a narrower set of gates than a recovery has.
#[must_use]
pub fn barred(mode: Mode, at: &Errand<'_>, left_ms: u64) -> Option<Barred> {
    if !mode.asks() {
        return Some(Barred::Off);
    }
    if let Why::Cleared { stop, next, .. } = at.why {
        if !matches!(stop, RecipeStop::StepFailed | RecipeStop::CheckFailed) {
            return Some(Barred::NotRecoverable);
        }
        if next == 0 {
            return Some(Barred::NoResume);
        }
    }
    if at.moves_money || at.flow.is_some_and(|spec| spec.money.is_some()) {
        return Some(Barred::MovesMoney);
    }
    if at.flow.is_some_and(|spec| spec.policy == Policy::Guarded) {
        return Some(Barred::Guarded);
    }
    if at.steps() == 0 {
        return Some(Barred::NoSteps);
    }
    let asking = u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    if left_ms <= asking {
        return Some(Barred::NoBudget);
    }
    None
}

/// What the person set for `seat`, read from the settings file the TypeSafe
/// card writes — by the row's own parser. Unreadable, missing or misspelt all
/// read as [`JevMode::Off`]: a walk never starts sending a screen anywhere
/// because a file could not be parsed.
#[must_use]
pub fn mode_now(seat: &JevUse) -> Mode {
    let root = crate::api_routers::zo_settings_path()
        .and_then(|path| crate::api_routers::read_zo_settings_root(&path).ok())
        .map_or(Value::Null, Value::Object);
    seat.mode_in(&root)
}

/// What a walk's report and its lines say about where it stopped. Owned,
/// because a report is a value and a borrow of it would outlive the read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoppedRead {
    pub stop: RecipeStop,
    /// The step that stopped, counted from 1.
    pub at: usize,
    /// The step to walk again from — the report's own `next`.
    pub next: usize,
    /// The stopped step's verb alone.
    pub step: String,
    /// What it was refused with.
    pub refusal: String,
    /// The browser pane the stopped step aimed at.
    pub pane: String,
}

/// Read a stopped walk, or `None` when there is nothing here to clear.
///
/// It answers `None` for a walk that finished, for a report naming no stop or
/// no resume point, and — the rule that is not obvious — for a stopped step
/// that did not go through the BROWSER door. Numbering a page's controls is
/// that door's; a desktop step's screen is numbered by another door and
/// reaches this loop as a goal walk of its own, not as a recovery.
///
/// Whether the stop MAY be cleared is [`barred`]'s answer, not this one:
/// reading and allowing are kept apart so a test can hold each on its own.
#[must_use]
pub fn read_report(report: &Value, lines: &[RecipeLine]) -> Option<StoppedRead> {
    if report["done"] == json!(true) {
        return None;
    }
    let stop = RecipeStop::from_word(report.pointer("/stop/kind")?.as_str()?)?;
    let at = usize::try_from(report["stoppedAt"].as_u64()?).ok()?;
    let next = usize::try_from(report["next"].as_u64()?).ok()?;
    let line = lines.iter().find(|line| line.step == at)?;
    if line.tool != RecipeTool::Browser {
        return None;
    }
    // `<verb> <pane> …`: the verb alone, never the rest — a `type` line's
    // argv carries the text a person typed.
    let step = line.argv.first()?.clone();
    let pane = line.argv.get(1)?.clone();
    Some(StoppedRead {
        stop,
        at,
        next,
        step,
        refusal: report
            .pointer("/stop/message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        pane,
    })
}

/// How a row says the judgment was used: recorded beside the walk, as the
/// record-only modes do.
const USE_SHADOW: &str = JevMode::Shadow.key();
const USE_APPLIED: &str = "applied";
const USE_FALLBACK: &str = "fallback";

/// What a walk came to: the rows it wrote, the report of the document's
/// re-walk when there was one, and — for a goal — whether it got there.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Walked {
    pub rows: Vec<Value>,
    pub report: Option<Value>,
    /// How many presses the walk actually made.
    pub pressed: usize,
    /// Whether the goal was reached, and by what: `Some(true)` only when the
    /// caller's own condition said so or, failing that, when the judgment
    /// said `done`. `None` for an errand that has no goal to reach.
    pub reached: Option<bool>,
}

/// One row, with the words every ledger of this family uses.
fn row(mode: Mode, at: &Errand<'_>, attempt: usize, more: Value) -> Value {
    let mut row = json!({
        "flow": at.goal,
        "errand": at.key(),
        "mode": mode.key(),
        "rubricVersion": SCREEN_ACTION_RUBRIC_VERSION,
        "attempt": attempt,
    });
    if let Why::Cleared { stop, step, .. } = at.why
        && let Some(row) = row.as_object_mut()
    {
        row.insert("stop".to_string(), json!(stop.as_str()));
        row.insert("step".to_string(), json!(step));
    }
    if let (Some(row), Some(more)) = (row.as_object_mut(), more.as_object()) {
        for (key, value) in more {
            row.insert(key.clone(), value.clone());
        }
    }
    row
}

/// Put one more word on a row already being written.
fn note(said: &mut Value, key: &str, value: Value) {
    if let Some(said) = said.as_object_mut() {
        said.insert(key.to_string(), value);
    }
}

/// Walk the screen by judgment until the errand is served or a gate ends it.
///
/// Off, barred, unreadable, unanswered, refused at the door, stuck on a screen
/// that will not move, or out of steps, the answer carries no report and the
/// caller ends up exactly where it would have without a judgment at all. That
/// equivalence is the feature's first promise and
/// `off_and_shadow_change_nothing_about_the_walk` holds it.
pub fn run(
    mode: Mode,
    at: &Errand<'_>,
    judge: &mut dyn ActionJudge,
    world: &mut dyn World,
) -> Walked {
    let mut walked = Walked::default();
    if matches!(at.why, Why::Goal { .. }) {
        walked.reached = Some(false);
    }
    let left = world.left_ms();
    if let Some(barred) = barred(mode, at, left) {
        // Off is the default and says nothing; the other gates are worth a row.
        if barred != Barred::Off {
            walked.rows.push(row(
                mode,
                at,
                0,
                json!({ "outcome": "barred", "barred": barred.as_str() }),
            ));
        }
        return walked;
    }

    // The numbers already spent ON THE SCREEN IN FRONT OF THE WALK, and the
    // screen they were spent on. A screen that moved is a new screen: its
    // numbers mean something else, so the list starts again.
    let mut tried: Vec<usize> = Vec::new();
    let mut before: Option<Screen> = None;
    let mut still = 0usize;
    for attempt in 1..=at.steps() {
        if world.left_ms() <= u64::try_from(ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX) {
            walked.rows.push(row(
                mode,
                at,
                attempt,
                json!({ "outcome": "barred", "barred": Barred::NoBudget.as_str() }),
            ));
            return walked;
        }
        let looking = std::time::Instant::now();
        let Some(screen) = world.look() else {
            walked
                .rows
                .push(row(mode, at, attempt, json!({ "outcome": "no_look" })));
            return walked;
        };
        let look_ms = u64::try_from(looking.elapsed().as_millis()).unwrap_or(u64::MAX);
        match before.as_ref() {
            Some(was) if was.same_as(&screen) => {
                still += 1;
                if still >= SAME_SCREEN_LIMIT {
                    walked.rows.push(row(
                        mode,
                        at,
                        attempt,
                        json!({ "outcome": "stuck", "pressed": walked.pressed }),
                    ));
                    return walked;
                }
            }
            Some(_) => {
                still = 0;
                tried.clear();
            }
            None => {}
        }
        let Some(asked) = ask(&ActionLook {
            goal: at.goal,
            errand: at.asked(),
            at: screen.at.asked(),
            tried: &tried,
            items: &screen.items,
        }) else {
            walked
                .rows
                .push(row(mode, at, attempt, json!({ "outcome": "no_candidate" })));
            return walked;
        };
        before = Some(screen);

        let candidates = asked.marks().len();
        let judging = std::time::Instant::now();
        let judged = judge.choose(&asked);
        let judgment_ms = u64::try_from(judging.elapsed().as_millis()).unwrap_or(u64::MAX);
        let spent = judge.spent();
        let choice = match judged {
            Judged::Chose(choice) => choice,
            Judged::Refused(token) => {
                walked.rows.push(row(
                    mode,
                    at,
                    attempt,
                    with_spent(
                        json!({
                            "outcome": token,
                            "candidates": candidates,
                            "routeUse": USE_FALLBACK,
                        }),
                        spent,
                    ),
                ));
                return walked;
            }
        };
        let mut said = with_spent(
            json!({
                "outcome": "answered",
                "candidates": candidates,
                "confidence": choice.confidence,
                "probabilities": choice.probabilities,
            }),
            spent,
        );
        let chosen = match choice.chosen {
            Chosen::Mark(mark) => mark,
            Chosen::GiveUp | Chosen::Done => {
                let ended = match choice.chosen {
                    Chosen::Done => zerocode_core::screen_action::DONE,
                    _ => zerocode_core::screen_action::GIVE_UP,
                };
                note(&mut said, "chosen", json!(ended));
                note(&mut said, "routeUse", json!(USE_FALLBACK));
                if choice.chosen == Chosen::Done {
                    // The weaker of the two ends: nothing was checked, the
                    // judgment simply says it is there. The row says which
                    // end it was, so evidence never reads one as the other.
                    note(&mut said, "reachedBy", json!("judgment"));
                    walked.reached = Some(true);
                }
                walked.rows.push(row(mode, at, attempt, said));
                return walked;
            }
        };
        note(&mut said, "chosen", json!(format!("mark:{chosen}")));

        // A record-only mode records what it would have pressed and presses
        // nothing.
        if !mode.applies() {
            note(&mut said, "routeUse", json!(USE_SHADOW));
            walked.rows.push(row(mode, at, attempt, said));
            return walked;
        }

        let pressed = crate::run_evidence::observing(
            json!({
                "look_ms": look_ms,
                "judgment": { "asked": true, "ms": judgment_ms, "confidence": choice.confidence },
            }),
            || world.press(chosen),
        );
        if !pressed {
            note(&mut said, "routeUse", json!(USE_FALLBACK));
            note(&mut said, "pressed", json!(false));
            walked.rows.push(row(mode, at, attempt, said));
            return walked;
        }
        tried.push(chosen);
        walked.pressed += 1;
        note(&mut said, "pressed", json!(true));
        note(&mut said, "routeUse", json!(USE_APPLIED));

        match at.why {
            Why::Cleared { next, .. } => {
                note(&mut said, "resumedFrom", json!(next));
                let after = world.walk_from(next);
                // Pressing is not succeeding: the row's `recheck` is the walk
                // that followed, read from its own report, never the door's
                // exit code.
                let cleared = after
                    .as_ref()
                    .is_some_and(|report| cleared_past(report, next));
                note(&mut said, "recheck", json!(cleared));
                walked.rows.push(row(mode, at, attempt, said));
                walked.report = after;
                if cleared {
                    return walked;
                }
            }
            Why::Goal { .. } => {
                // The caller's own condition, asked of the screen. It is the
                // only verified end a goal walk has; `done` is a judgment's
                // word about itself.
                let reached = world.reached();
                if let Some(reached) = reached {
                    note(&mut said, "recheck", json!(reached));
                }
                walked.rows.push(row(mode, at, attempt, said));
                if reached == Some(true) {
                    walked.reached = Some(true);
                    return walked;
                }
            }
        }
    }
    walked
}

/// Append the rows to the walk's evidence folder, under the ledger `seat`
/// names. A folder that will not take them is said once on stderr and never
/// raised: a walk's record is not worth failing a walk that already happened.
pub fn write_rows(seat: &JevUse, dir: Option<&std::path::Path>, rows: &[Value]) {
    let (Some(dir), false) = (dir, rows.is_empty()) else {
        return;
    };
    let mut said = String::new();
    for row in rows {
        said.push_str(&row.to_string());
        said.push('\n');
    }
    use std::io::Write as _;
    let written = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(seat.ledger))
        .and_then(|mut file| file.write_all(said.as_bytes()));
    if let Err(why) = written {
        eprintln!("recipe-run: the walk's record was not written: {why}");
    }
}

/// A row's words with what the judgment cost at the Jev door, when the judge
/// says — under the keys every Jev ledger spells them with.
fn with_spent(mut said: Value, spent: Option<Spent>) -> Value {
    if let (Some(spent), Some(fields)) = (spent, said.as_object_mut()) {
        fields.insert(REQUESTS_KEY.to_string(), json!(spent.requests));
        fields.insert(REDACTED_LINES_KEY.to_string(), json!(spent.redacted_lines));
    }
    said
}

/// Whether a report shows the walk getting past the step it had stopped at:
/// it finished, or it stopped somewhere later. Stopping at the same step
/// again is the obstacle still standing.
fn cleared_past(report: &Value, was: usize) -> bool {
    if report["done"] == json!(true) {
        return true;
    }
    report["stoppedAt"]
        .as_u64()
        .and_then(|at| usize::try_from(at).ok())
        .is_none_or(|at| at > was)
}

pub mod desk;
pub mod live;
pub mod walk;

#[cfg(test)]
mod tests;
