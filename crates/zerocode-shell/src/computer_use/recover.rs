//! Clearing what stopped a walk (docs/design/jev-browser-action-20260917.md
//! §2.3): when a recorded flow stops because a step or a check failed, the
//! screen in front of it already numbers every control a person could press,
//! so one of those numbers — chosen by a judgment, never invented by one — is
//! pressed and the stopped step is walked again.
//!
//! The shape is the repeat's ([`super::repeat`]): this module decides WHETHER
//! and WITH WHAT, and the world it acts on is one narrow seam ([`Recovery`])
//! a test replaces whole. It walks no step itself — pressing goes down the
//! same door, through the same pin and the same fingerprint, as every other
//! press — and it invents no resume point: the step to walk again from is the
//! one the report already computed (`RecipeStop::resumes_after`).
//!
//! Three things are load-bearing and each has a test of its own:
//!
//! 1. **Only two stops are recovered.** `StepFailed` and `CheckFailed` resume
//!    AT the step that stopped, which is what clearing an obstacle means. Every
//!    other stop either belongs to a person, was set by one, or resumes after
//!    the stopped step — and walking on from there after an unrelated press
//!    would skip work the recipe asked for.
//! 2. **Money is never recovered.** A document that moves money is barred
//!    whatever its policy, and a `guarded` Flow is barred whatever its steps.
//! 3. **Pressing is not succeeding.** The row says what was chosen and, apart
//!    from it, whether the walk that followed got past the step that stopped.

use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::browser_action::{
    ActionAsk, ActionChoice, ActionLook, BROWSER_ACTION_RUBRIC_VERSION, Chosen, ask,
};
use zerocode_core::computer_flow::{FlowSpec, Policy};
use zerocode_core::computer_recipe::{RecipeLine, RecipeStop, RecipeTool};
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
use zerocode_core::jev::{BROWSER, JevMode};

/// How long one judgment may hold a stopped walk. The same number the
/// router's applied mode waits: a judgment that has not answered by then is
/// slower than the fallback it would replace.
pub const BROWSER_ACTION_DEADLINE: Duration = Duration::from_millis(1_500);

/// How many numbers one walk may spend clearing one stop. Two, because a
/// third guess on a screen that did not move twice is a loop, not a recovery.
pub const MAX_RECOVERY_ATTEMPTS: usize = 2;

/// The ledger, beside the walk's own evidence — the Jev use table's name for
/// the browser row's ledger.
pub const RECOVER_LEDGER: &str = BROWSER.ledger;

/// What the person set: the Jev use table's browser row
/// ([`zerocode_core::jev::BROWSER`], `smart.browserAction`). The ladder is the
/// routing card's, because it is the same question: does anything go to the
/// vendor, and may it change what the product does. `off` asks nothing and is
/// what an unknown word reads as; `shadow` and `auto` ask and record while the
/// walk stops exactly as it would have — nothing promotes a press — and `on`
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

/// The screen a stopped walk is looking at.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Screen {
    pub host: String,
    pub path: String,
    pub items: Vec<Value>,
}

/// What a recovery does to the world, and nothing else — each a seam.
pub trait Recovery {
    /// The page and the controls it is showing now, or `None` when the screen
    /// cannot be read (a recovery that cannot see does not press).
    fn look(&mut self) -> Option<Screen>;
    /// Press one number down the ordinary door. `true` when the door took it;
    /// a stale pin, a host outside the recording and a shut door are all
    /// `false`, and none of them is retried.
    fn press(&mut self, mark: usize) -> bool;
    /// Walk the document again from `step`, answering the report.
    fn walk_from(&mut self, step: usize) -> Option<Value>;
    /// Milliseconds the call has left.
    fn left_ms(&mut self) -> u64;
}

/// Where a walk stopped, as a recovery reads it.
#[derive(Debug, Clone, Copy)]
pub struct Stopped<'a> {
    /// What the flow is for — its name, or the check that stopped.
    pub goal: &'a str,
    pub stop: RecipeStop,
    /// The stopped step's VERB alone; a `type` line's argv carries a value.
    pub step: &'a str,
    pub refusal: &'a str,
    /// The step to walk again from — the report's own `next`.
    pub next: usize,
    pub flow: Option<&'a FlowSpec>,
    /// Whether the document has a money step at all.
    pub moves_money: bool,
}

/// Why a stop was not recovered. Each is a row the ledger writes rather than
/// a silence, so a person can see the gate held.
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
    /// Not enough of the call's clock left to ask and still walk.
    NoBudget,
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
        }
    }
}

/// Whether this stop may be recovered at all, before anything is looked at or
/// asked. Pure: the whole gate reads its answer from here.
#[must_use]
pub fn barred(mode: Mode, at: &Stopped<'_>, left_ms: u64) -> Option<Barred> {
    if !mode.asks() {
        return Some(Barred::Off);
    }
    if !matches!(at.stop, RecipeStop::StepFailed | RecipeStop::CheckFailed) {
        return Some(Barred::NotRecoverable);
    }
    if at.moves_money || at.flow.is_some_and(|spec| spec.money.is_some()) {
        return Some(Barred::MovesMoney);
    }
    if at.flow.is_some_and(|spec| spec.policy == Policy::Guarded) {
        return Some(Barred::Guarded);
    }
    if at.next == 0 {
        return Some(Barred::NoResume);
    }
    let asking = u64::try_from(BROWSER_ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX);
    if left_ms <= asking {
        return Some(Barred::NoBudget);
    }
    None
}

/// What the person set, read from the settings file the TypeSafe card writes —
/// the browser row's switch, read by the row's own parser. Unreadable, missing
/// or misspelt all read as [`JevMode::Off`]: a walk never starts sending a
/// screen anywhere because a file could not be parsed.
#[must_use]
pub fn mode_now() -> Mode {
    let root = crate::api_routers::zo_settings_path()
        .and_then(|path| crate::api_routers::read_zo_settings_root(&path).ok())
        .map_or(Value::Null, Value::Object);
    BROWSER.mode_in(&root)
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

/// Read a stopped walk, or `None` when there is nothing here to recover.
///
/// It answers `None` for a walk that finished, for a report naming no stop or
/// no resume point, and — the rule that is not obvious — for a stopped step
/// that did not go through the BROWSER door. Numbering a page's controls is
/// that door's; a desktop step's recovery would read another surface entirely
/// and is a different feature.
///
/// Whether the stop MAY be recovered is [`barred`]'s answer, not this one:
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

/// How a row says the judgment was used: recorded beside the stop, as the
/// record-only modes do.
const USE_SHADOW: &str = JevMode::Shadow.key();
const USE_APPLIED: &str = "applied";
const USE_FALLBACK: &str = "fallback";

/// What a recovery came to: the rows it wrote and, when it pressed and walked
/// again, the report of the walk that followed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Recovered {
    pub rows: Vec<Value>,
    pub report: Option<Value>,
}

/// One row, with the words every ledger of this family uses.
fn row(mode: Mode, at: &Stopped<'_>, attempt: usize, more: Value) -> Value {
    let mut row = json!({
        "flow": at.goal,
        "stop": at.stop.as_str(),
        "step": at.step,
        "mode": mode.key(),
        "rubricVersion": BROWSER_ACTION_RUBRIC_VERSION,
        "attempt": attempt,
    });
    if let (Some(row), Some(more)) = (row.as_object_mut(), more.as_object()) {
        for (key, value) in more {
            row.insert(key.clone(), value.clone());
        }
    }
    row
}

/// Try to clear what stopped the walk.
///
/// Off, barred, unreadable, unanswered, refused at the door, or out of
/// attempts, the answer carries no report and the caller stops exactly as it
/// would have without a judgment at all. That equivalence is the feature's
/// first promise and `off_and_shadow_change_nothing_about_the_walk` holds it.
pub fn recover(
    mode: Mode,
    at: &Stopped<'_>,
    judge: &mut dyn ActionJudge,
    world: &mut dyn Recovery,
) -> Recovered {
    let mut recovered = Recovered::default();
    let left = world.left_ms();
    if let Some(barred) = barred(mode, at, left) {
        // Off is the default and says nothing; the other gates are worth a row.
        if barred != Barred::Off {
            recovered.rows.push(row(
                mode,
                at,
                0,
                json!({ "outcome": "barred", "barred": barred.as_str() }),
            ));
        }
        return recovered;
    }

    let mut tried: Vec<usize> = Vec::new();
    for attempt in 1..=MAX_RECOVERY_ATTEMPTS {
        if world.left_ms() <= u64::try_from(BROWSER_ACTION_DEADLINE.as_millis()).unwrap_or(u64::MAX)
        {
            recovered.rows.push(row(
                mode,
                at,
                attempt,
                json!({ "outcome": "barred", "barred": Barred::NoBudget.as_str() }),
            ));
            return recovered;
        }
        let Some(screen) = world.look() else {
            recovered
                .rows
                .push(row(mode, at, attempt, json!({ "outcome": "no_look" })));
            return recovered;
        };
        let Some(asked) = ask(&ActionLook {
            goal: at.goal,
            stopped: at.stop.as_str(),
            step: at.step,
            refusal: at.refusal,
            host: &screen.host,
            path: &screen.path,
            tried: &tried,
            items: &screen.items,
        }) else {
            recovered
                .rows
                .push(row(mode, at, attempt, json!({ "outcome": "no_candidate" })));
            return recovered;
        };

        let candidates = asked.marks().len();
        let judged = judge.choose(&asked);
        let spent = judge.spent();
        let choice = match judged {
            Judged::Chose(choice) => choice,
            Judged::Refused(token) => {
                recovered.rows.push(row(
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
                return recovered;
            }
        };
        let chosen = match choice.chosen {
            Chosen::Mark(mark) => mark,
            Chosen::GiveUp => {
                recovered.rows.push(row(
                    mode,
                    at,
                    attempt,
                    with_spent(
                        json!({
                            "outcome": "answered",
                            "candidates": candidates,
                            "chosen": zerocode_core::browser_action::GIVE_UP,
                            "confidence": choice.confidence,
                            "routeUse": USE_FALLBACK,
                        }),
                        spent,
                    ),
                ));
                return recovered;
            }
        };
        let mut said = with_spent(
            json!({
                "outcome": "answered",
                "candidates": candidates,
                "chosen": format!("mark:{chosen}"),
                "confidence": choice.confidence,
                "probabilities": choice.probabilities,
            }),
            spent,
        );
        let note = |said: &mut Value, key: &str, value: Value| {
            if let Some(said) = said.as_object_mut() {
                said.insert(key.to_string(), value);
            }
        };

        // A record-only mode records what it would have pressed and presses
        // nothing.
        if !mode.applies() {
            note(&mut said, "routeUse", json!(USE_SHADOW));
            recovered.rows.push(row(mode, at, attempt, said));
            return recovered;
        }

        if !world.press(chosen) {
            note(&mut said, "routeUse", json!(USE_FALLBACK));
            note(&mut said, "pressed", json!(false));
            recovered.rows.push(row(mode, at, attempt, said));
            return recovered;
        }
        note(&mut said, "pressed", json!(true));
        note(&mut said, "routeUse", json!(USE_APPLIED));
        note(&mut said, "resumedFrom", json!(at.next));

        let walked = world.walk_from(at.next);
        // Pressing is not succeeding: the row's `recheck` is the walk that
        // followed, read from its own report, never the door's exit code.
        let cleared = walked
            .as_ref()
            .is_some_and(|report| cleared_past(report, at.next));
        note(&mut said, "recheck", json!(cleared));
        recovered.rows.push(row(mode, at, attempt, said));
        recovered.report = walked;
        if cleared {
            return recovered;
        }
        tried.push(chosen);
    }
    recovered
}

/// Append the rows to the walk's evidence folder. A folder that will not take
/// them is said once on stderr and never raised: a recovery's record is not
/// worth failing a walk that already happened.
pub fn write_rows(dir: Option<&std::path::Path>, rows: &[Value]) {
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
        .open(dir.join(RECOVER_LEDGER))
        .and_then(|mut file| file.write_all(said.as_bytes()));
    if let Err(why) = written {
        eprintln!("recipe-run: the recovery's record was not written: {why}");
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

pub mod live;
pub mod walk;

#[cfg(test)]
mod tests;
