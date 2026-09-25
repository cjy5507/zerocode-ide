//! The forked phone step (t-6044, `zerocode_core::jev::BRANCHING`): the one
//! point of the walk where a judgment's second and third choices are tried
//! before its first is made canonical.
//!
//! [`step`] sits inside [`super::walk`]'s press — after the screen seat's
//! answer has been read and its press floor passed, before the hand goes
//! out — and decides the number that hand presses. Under [`Branching::OFF`]
//! it decides nothing and the walk's own press stands, byte for byte. Under
//! a seat that only records it asks the comparison over the candidates'
//! actions alone and presses the walk's own number. Under an acting seat it
//! saves the device ([`super::World::save`]), presses each candidate and
//! reads the screen it leads to, puts the device back, asks which result is
//! the closest to the goal, and presses that one — and every way that road
//! can fail (no snapshot, no clock, a load that did not take, a refusal, an
//! answer under the floor) ends with the first candidate pressed, which is
//! the number the walk had already chosen.
//!
//! The rules are the core's ([`zerocode_core::branching`]): which candidates,
//! what the question carries, how long a fork may hold the walk, how the
//! next step grades the pick. This file holds the road — a save, presses,
//! looks, loads — and the row.

use std::time::Instant;

use serde_json::{Value, json};
use zerocode_core::branching::{
    BRANCHING_RUBRIC_VERSION, BranchChoice, BranchLook, Candidate, NextStep, Outcome, agreed, ask,
    fork_budget_ms, fork_wanted,
};
use zerocode_core::computer_use_protocol::marks::legend_line;
use zerocode_core::guarded::ControlKind;
use zerocode_core::jev::promote::SEAT_RECORDING;
use zerocode_core::jev::summary::{AGREED, AT, ELAPSED_MS};
use zerocode_core::jev::{BRANCHING, BRANCHING_APPLY_DEADLINE_MS};
use zerocode_core::screen_action::{ActionChoice, option_of};

use super::{
    ActionJudge, BARRED, Barred, CONTROL_KIND, Errand, Mode, REASON, Screen, Seen, Walked, World,
    control_kind, press_rule,
};

/// The branching seat's standing for one walk: whether a forked step is
/// asked about at all, and whether the comparison's pick is the one pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Branching {
    /// What the person set `smart.jevBranching` to.
    pub mode: Mode,
    /// Whether the seat acts: an `auto` its own ledger raised
    /// (`crate::systemone::applies`). Read by the caller, off the same wire
    /// the questions go down.
    pub acting: bool,
    /// The act line the seat's graded answers drew (t-9468,
    /// `crate::systemone::act_line`): the pick is pressed from it in place
    /// of the press floor. `None` presses from the floor, as ever.
    pub act_line: Option<u16>,
}

impl Branching {
    /// Nobody asked for a fork: today's walk exactly.
    pub const OFF: Self = Self {
        mode: Mode::Off,
        acting: false,
        act_line: None,
    };
}

/// A device saved where it stands, as [`super::World::save`] hands it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    /// The snapshot's name on the device.
    pub name: String,
    /// How long the save took — what the fork's budget reads as the cost of
    /// each load, since nothing else about this device has been measured.
    pub took_ms: u64,
}

/// What the comparison answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Compared {
    /// A validated choice over the candidates that were offered.
    Chose(BranchChoice),
    /// Nothing usable, named by the ledger's own failure token.
    Refused(String),
}

/// The refusal a judge with no comparison to give answers with.
pub const NO_COMPARISON: &str = "no_comparison";

/// Why a fork stepped back to a single press, when it did — the row's
/// `barred`, beside the walk's own [`Barred`] words where they fit.
const NO_SNAPSHOT: &str = "no_snapshot";
const RESTORE_FAILED: &str = "restore_failed";
const ONE_EXPLORED: &str = "one_explored";

/// The row key an answered row is named by.
const KEY: &str = "branching";

/// The row's outcome for a question Jev answered in shape.
const ANSWERED: &str = "answered";

/// The three words every ledger of this family spells a row's use with.
const USE_SHADOW: &str = Mode::Shadow.key();
const USE_APPLIED: &str = zerocode_core::jev::ROUTE_USE_APPLIED;
const USE_FALLBACK: &str = zerocode_core::jev::ROUTE_USE_FALLBACK;

/// A forked step waiting for the walk's next step to grade it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending {
    /// The row's place among [`Walked::forks`].
    pub row: usize,
    /// Whether the comparison named the candidate that was actually pressed.
    pub same_pick: bool,
}

/// What the walk hands the forked step: its seat's standing, the errand, the
/// screen it just looked at, the screen seat's answer and the number it
/// chose, and how long that look took — the walk's one measured step.
pub struct Step<'a> {
    pub branching: Branching,
    pub at: &'a Errand<'a>,
    pub attempt: usize,
    pub screen: Option<&'a Screen>,
    pub choice: &'a ActionChoice,
    pub chosen: usize,
    pub look_ms: u64,
}

/// What the forked step came to: the number the walk presses, whether the
/// step already pressed it (a load that failed left the device where a
/// candidate's press put it), the seat's row when one was written, and
/// whether the comparison named the number pressed.
pub struct Stepped {
    pub mark: usize,
    pub pressed: Option<bool>,
    pub row: Option<Value>,
    pub same_pick: bool,
}

impl Stepped {
    /// Today's press, with nothing written.
    const fn today(mark: usize) -> Self {
        Self {
            mark,
            pressed: None,
            row: None,
            same_pick: true,
        }
    }
}

/// One row, with the words every ledger of this family uses.
fn row(step: &Step<'_>, seen: &Seen, today: usize, candidates: usize, more: Value) -> Value {
    let (platform, device) = match seen {
        Seen::Phone { platform, device } => (platform.as_str(), device.as_str()),
        Seen::Page { .. } | Seen::Desk { .. } => ("", ""),
    };
    let now_ms = crate::project_runtime::now_epoch_ms();
    // The goal is a person's sentence and the device's name can be a
    // person's; the row carries their fingerprints (t-6155 F12), which is
    // enough to group the steps of one walk on one device.
    let fingerprint = |words: &str| {
        if words.is_empty() {
            String::new()
        } else {
            zerocode_core::jev::fingerprint_of(words)
        }
    };
    let mut row = json!({
        AT.canonical: now_ms,
        KEY: format!("{}@{now_ms}", step.attempt),
        "flowFingerprint": fingerprint(step.at.goal),
        "errand": step.at.key(),
        "attempt": step.attempt,
        "mode": step.branching.mode.key(),
        "rubricVersion": BRANCHING_RUBRIC_VERSION,
        "platform": platform,
        "deviceFingerprint": fingerprint(device),
        "today": option_of(today),
        "candidates": candidates,
    });
    if let (Some(row), Some(more)) = (row.as_object_mut(), more.as_object()) {
        for (key, value) in more {
            row.insert(key.clone(), value.clone());
        }
    }
    row
}

/// What the screen shows, as the question reads a screen: its legend lines.
fn legend_of(screen: &Screen) -> Vec<String> {
    screen.items.iter().filter_map(legend_line).collect()
}

/// The legend line of the control numbered `mark` on `screen`.
fn action_of(screen: &Screen, mark: usize) -> String {
    screen
        .items
        .iter()
        .find(|item| item.get("mark").and_then(Value::as_u64) == u64::try_from(mark).ok())
        .and_then(legend_line)
        .unwrap_or_else(|| option_of(mark))
}

/// Ask the comparison and read it against the row: the answer's number when
/// it is one and clears the press floor, else the first candidate — with the
/// row's outcome, use and reason said either way.
fn compared(
    judge: &mut dyn ActionJudge,
    asked: &zerocode_core::branching::BranchAsk,
    screen: &Screen,
    today: usize,
    acting: bool,
    line: Option<u16>,
    said: &mut Value,
) -> usize {
    let judging = Instant::now();
    let answered = judge.compare(asked);
    let judgment_ms = u64::try_from(judging.elapsed().as_millis()).unwrap_or(u64::MAX);
    said[ELAPSED_MS.canonical] = json!(judgment_ms);
    if let Some(spent) = judge.spent() {
        spent.stamp(said);
    }
    match answered {
        Compared::Refused(token) => {
            said["outcome"] = json!(token);
            said["routeUse"] = json!(USE_FALLBACK);
            today
        }
        Compared::Chose(choice) => {
            said["outcome"] = json!(ANSWERED);
            said["chosen"] = json!(option_of(choice.mark));
            said["confidence"] = json!(choice.confidence);
            said["probabilities"] = json!(choice.probabilities);
            if !acting {
                said["routeUse"] = json!(USE_SHADOW);
                said[REASON] = json!(SEAT_RECORDING);
                return today;
            }
            // The walk's one press rule, read for the pick (t-6187): a
            // control a press cannot take back asks nine in ten here too.
            let (permitted, kind) =
                press_rule(&BRANCHING, line, screen, choice.mark, choice.confidence);
            said[CONTROL_KIND] = json!(kind.word());
            if !permitted {
                said[BARRED] = json!(Barred::LowConfidence.as_str());
                said["routeUse"] = json!(USE_FALLBACK);
                return today;
            }
            said["routeUse"] = json!(USE_APPLIED);
            choice.mark
        }
    }
}

/// The forked step, or today's press.
///
/// The road, in order: nothing under `off`, on a screen that is not a
/// phone's, or for a judgment that ranked one control; the question over
/// actions alone under a seat that records; and under an acting seat a save,
/// then per candidate a press, a look and a load, then the comparison, then
/// the pick — every exit past the save going through the load, so the device
/// the walk goes on from is the one it forked from, except after a load that
/// failed, where the row says which candidate's screen the device stands on.
pub fn step(step: &Step<'_>, judge: &mut dyn ActionJudge, world: &mut dyn World) -> Stepped {
    let today = step.chosen;
    if !step.branching.mode.asks() {
        return Stepped::today(today);
    }
    let Some(screen) = step
        .screen
        .filter(|screen| matches!(screen.at, Seen::Phone { .. }))
    else {
        return Stepped::today(today);
    };
    // A fork presses each candidate for real before a snapshot puts the
    // device back, and a snapshot cannot take back what left the device — a
    // payment, a delete, a message sent. A control a press cannot take back
    // is never explored (t-6187); the step is taken once, as today, when
    // fewer than two candidates are left.
    let mut marks = fork_wanted(step.choice);
    marks.retain(|mark| control_kind(screen, *mark) == ControlKind::Plain);
    if marks.len() < 2 {
        return Stepped::today(today);
    }
    let before = legend_of(screen);
    let mut candidates: Vec<Candidate> = marks
        .iter()
        .map(|mark| Candidate {
            mark: *mark,
            action: action_of(screen, *mark),
            result: None,
        })
        .collect();
    let mut said = json!({ "explored": 0 });

    // A seat that records asks over the actions alone and presses today's.
    if !step.branching.acting {
        let asked = ask(&BranchLook {
            goal: step.at.goal,
            at: screen.at.asked(),
            before: &before,
            candidates: &candidates,
        });
        let mut same_pick = true;
        if let Some(asked) = asked {
            // The pick is today's whatever the answer; what the mark reads is
            // whether the comparison named it. A refusal named nothing and
            // shares today's fate.
            let _ = compared(
                judge,
                &asked,
                screen,
                today,
                false,
                step.branching.act_line,
                &mut said,
            );
            same_pick = said
                .get("chosen")
                .and_then(Value::as_str)
                .is_none_or(|chosen| chosen == option_of(today));
        }
        return Stepped {
            mark: today,
            pressed: None,
            row: Some(row(step, &screen.at, today, marks.len(), said)),
            same_pick,
        };
    }

    // An acting seat forks for real: the device is saved first, because the
    // budget is read off what the save cost.
    let forking = Instant::now();
    let Some(saved) = world.save() else {
        said[BARRED] = json!(NO_SNAPSHOT);
        said["routeUse"] = json!(USE_FALLBACK);
        return Stepped {
            mark: today,
            pressed: None,
            row: Some(row(step, &screen.at, today, marks.len(), said)),
            same_pick: true,
        };
    };
    said["saveMs"] = json!(saved.took_ms);
    let budget = fork_budget_ms(marks.len(), step.look_ms, saved.took_ms);
    said["budgetMs"] = json!(budget);
    if world.left_ms() <= budget {
        world.forget(&saved);
        said[BARRED] = json!(Barred::NoBudget.as_str());
        said["routeUse"] = json!(USE_FALLBACK);
        said["forkMs"] = json!(elapsed_ms(forking));
        return Stepped {
            mark: today,
            pressed: None,
            row: Some(row(step, &screen.at, today, marks.len(), said)),
            same_pick: true,
        };
    }

    let mut step_ms = Vec::new();
    let mut restore_ms = Vec::new();
    let mut explored = 0usize;
    for (index, candidate) in candidates.iter_mut().enumerate() {
        // The clock: past the budget, or without room to ask and still press,
        // the candidates left are not explored.
        if elapsed_ms(forking) >= budget
            || world.left_ms() <= step.look_ms.saturating_add(BRANCHING_APPLY_DEADLINE_MS)
        {
            break;
        }
        let stepping = Instant::now();
        let pressed = crate::run_evidence::observing(
            json!({ "fork": { "candidate": index, "of": marks.len(), "mark": candidate.mark } }),
            || world.press(candidate.mark),
        );
        if !pressed {
            // A press the door refused changed nothing: no look, no load.
            step_ms.push(elapsed_ms(stepping));
            continue;
        }
        let after = world.look();
        step_ms.push(elapsed_ms(stepping));
        candidate.result = Some(match &after {
            Some(after) => Outcome {
                moved: !screen.same_as(after),
                controls: legend_of(after),
                count: after.items.len(),
            },
            // A screen that could not be read after the press is a result
            // too: nothing the walk could go on from.
            None => Outcome {
                moved: false,
                controls: Vec::new(),
                count: 0,
            },
        });
        explored += 1;
        let restoring = Instant::now();
        let restored = world.restore(&saved);
        restore_ms.push(elapsed_ms(restoring));
        if !restored {
            // The device stands on this candidate's screen: that press is
            // the step, and the row says so.
            world.forget(&saved);
            said["explored"] = json!(explored);
            said["stepMs"] = json!(step_ms);
            said["restoreMs"] = json!(restore_ms);
            said["outcome"] = json!(RESTORE_FAILED);
            said["chosen"] = json!(option_of(candidate.mark));
            said["routeUse"] = json!(USE_FALLBACK);
            said["forkMs"] = json!(elapsed_ms(forking));
            return Stepped {
                mark: candidate.mark,
                pressed: Some(true),
                row: Some(row(step, &screen.at, today, marks.len(), said)),
                same_pick: true,
            };
        }
    }
    said["explored"] = json!(explored);
    said["stepMs"] = json!(step_ms);
    said["restoreMs"] = json!(restore_ms);

    let mut pick = today;
    if explored < 2 {
        said[BARRED] = json!(ONE_EXPLORED);
        said["routeUse"] = json!(USE_FALLBACK);
    } else {
        let tried: Vec<Candidate> = candidates
            .into_iter()
            .filter(|candidate| candidate.result.is_some())
            .collect();
        if let Some(asked) = ask(&BranchLook {
            goal: step.at.goal,
            at: screen.at.asked(),
            before: &before,
            candidates: &tried,
        }) {
            pick = compared(
                judge,
                &asked,
                screen,
                today,
                true,
                step.branching.act_line,
                &mut said,
            );
        }
    }
    world.forget(&saved);
    said["forkMs"] = json!(elapsed_ms(forking));
    Stepped {
        mark: pick,
        pressed: None,
        row: Some(row(step, &screen.at, today, marks.len(), said)),
        same_pick: true,
    }
}

fn elapsed_ms(since: Instant) -> u64 {
    u64::try_from(since.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Grade the forked step waiting on the walk's next step, when one is
/// waiting: the row learns what the next step showed, the mark the rule
/// leaves ([`agreed`]), and — for a pick that differed from today's and went
/// on — that the fork rescued the step.
pub fn settle(walked: &mut Walked, pending: Option<Pending>, next: NextStep) {
    let Some(pending) = pending else {
        return;
    };
    let Some(row) = walked.forks.get_mut(pending.row) else {
        return;
    };
    row["next"] = json!(next.word());
    let Some(mark) = agreed(pending.same_pick, next) else {
        return;
    };
    row[AGREED.canonical] = json!(mark);
    let applied = row["routeUse"] == json!(USE_APPLIED);
    row["rescued"] = json!(mark && applied && row["chosen"] != row["today"]);
}

#[cfg(test)]
mod tests;
