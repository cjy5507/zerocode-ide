//! The reflex decision's runner, the part every reader of it shares (t-9205):
//! what a live reflex run's typed state is, how the one question is asked of
//! it, how a run keeps one question in flight and merges the rest, and what a
//! row says an answer was about. The window reads the run and asks
//! (`computer_use::reflex`); what a row records is the teacher's answer about
//! the state that was asked, never about another.
//!
//! What may be carried out, and what an answer is graded by, are here too
//! (t-10223 §2.2): an answer reaches the hand only through [`verdict`] — the
//! run, epoch and plan that asked it still the ones running, its reading
//! still young, the seat applying — and every answer, carried out or not, is
//! graded on what the hand did in the window after it ([`graded`],
//! [`label_row`]). The autopilot that carries answers out is the window's
//! (`computer_use::reflex::autopilot`).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::choice;
use super::questions::{
    REFLEX_DECIDE_ASKS, REFLEX_DECIDE_OPTIONS, REFLEX_DECIDE_QUESTION,
    REFLEX_DECIDE_RUBRIC_VERSION, REFLEX_DECIDE_STATE_KEYS, reflex_decide_rubric_fingerprint,
};
use super::summary::{AGREED, BASELINE_AGREED, LABEL, NOT_COMPARED, REQUEST_AT};
use crate::computer_use::REFLEX_APPLY_MAX_AGE_MS;
use crate::computer_use_protocol::reflex::{LIMITS, identifier};

/// The three options by their words, in the table's order
/// ([`REFLEX_DECIDE_OPTIONS`]): keep acting, stop, have the plan rewritten.
pub const CONTINUE: &str = REFLEX_DECIDE_OPTIONS[0].0;
pub const PAUSE: &str = REFLEX_DECIDE_OPTIONS[1].0;
pub const REPLAN: &str = REFLEX_DECIDE_OPTIONS[2].0;

/// Where the frames a state was read on came from — the run, the eye's stream
/// and geometry, the hold on the hand and the plan. An answer is about the
/// state asked on one scene; a later state of the same scene may take it, a
/// state of another may not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scene {
    pub run: String,
    pub stream: u64,
    pub geometry: u64,
    pub owner: u64,
    pub plan: u64,
}

/// One reading of a run, as the question carries it and as a row keeps it:
/// the typed state (each detector's newest sighting and the outcome counts)
/// and where it came from — the scene, the capture it was read on and how old
/// that capture was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub state: Value,
    pub scene: Option<Scene>,
    pub capture: Option<u64>,
    pub age_ns: Option<u64>,
    /// When the window read the status, in milliseconds of its own steady
    /// clock — the reader's to stamp ([`snapshot_of`] reads none): with
    /// [`Self::age_ns`] it says how old the capture is at any later moment
    /// ([`age_at`]).
    pub read_ms: Option<u64>,
}

impl Snapshot {
    /// The provenance a row keeps beside the state it asked about.
    #[must_use]
    pub fn provenance(&self) -> Value {
        json!({
            "capture": self.capture,
            "ageNs": self.age_ns,
            "run": self.scene.as_ref().map(|scene| scene.run.clone()),
            "stream": self.scene.as_ref().map(|scene| scene.stream),
            "geometry": self.scene.as_ref().map(|scene| scene.geometry),
            "owner": self.scene.as_ref().map(|scene| scene.owner),
            "plan": self.scene.as_ref().map(|scene| scene.plan),
        })
    }
}

/// A helper's reflex status (`reflexStatus`, `reflexReceipts`) read as the
/// question's typed state: at most the table's detectors, each named by its
/// plan id — anything that is not an id is left out, never cut into one — its
/// value or why it is unknown, the track it follows and how old its frame is
/// in milliseconds; and the outcome counts under their words. Nothing else the
/// status carries — the plan's hash, the monitor, the counters, and above all
/// no pixel, no screen's words and no app's name — goes into the state.
#[must_use]
pub fn snapshot_of(status: &Value) -> Snapshot {
    let cap = usize::try_from(LIMITS.max_detectors).unwrap_or(usize::MAX);
    let sightings: Vec<Value> = status
        .get("sightings")
        .and_then(Value::as_array)
        .map(|sightings| {
            sightings
                .iter()
                .filter_map(|sighting| {
                    let detector = sighting
                        .get("detector")
                        .and_then(Value::as_str)
                        .filter(|id| identifier(id))?;
                    let unknown = sighting
                        .get("unknown")
                        .and_then(Value::as_str)
                        .filter(|word| identifier(word));
                    Some(json!({
                        "detector": detector,
                        "value": sighting.get("value").and_then(Value::as_i64),
                        "unknown": unknown,
                        "track": sighting.get("track").and_then(Value::as_u64),
                        "age_ms": sighting
                            .get("ageNs")
                            .and_then(Value::as_u64)
                            .map(|age| age / 1_000_000),
                    }))
                })
                .take(cap)
                .collect()
        })
        .unwrap_or_default();
    let outcomes: BTreeMap<String, u64> = status
        .get("outcomes")
        .and_then(Value::as_object)
        .map(|outcomes| {
            outcomes
                .iter()
                .filter(|(word, _)| identifier(word))
                .filter_map(|(word, count)| Some((word.clone(), count.as_u64()?)))
                .collect()
        })
        .unwrap_or_default();
    let [sightings_key, outcomes_key] = REFLEX_DECIDE_STATE_KEYS;
    let scene = status.get("scene").and_then(|scene| {
        Some(Scene {
            run: status.get("runId")?.as_str()?.to_string(),
            stream: scene.get("stream")?.as_u64()?,
            geometry: scene.get("geometry")?.as_u64()?,
            owner: scene.get("owner")?.as_u64()?,
            plan: scene.get("plan")?.as_u64()?,
        })
    });
    Snapshot {
        state: json!({ sightings_key: sightings, outcomes_key: outcomes }),
        scene,
        capture: status.get("lastCapture").and_then(Value::as_u64),
        age_ns: status.get("lastCaptureAgeNs").and_then(Value::as_u64),
        read_ms: None,
    }
}

/// How old `snapshot`'s capture is at `now_ms` on the reader's clock: the
/// time since the status was read plus the age the capture had then. `None`
/// when either is unknown — a reading nobody can date is not a young one.
#[must_use]
pub fn age_at(snapshot: &Snapshot, now_ms: u64) -> Option<u64> {
    let read = snapshot.read_ms?;
    let then = snapshot.age_ns? / 1_000_000;
    Some(now_ms.saturating_sub(read).saturating_add(then))
}

/// Whether a run's state finds nothing: no detector reads a known value
/// other than none — each is unknown, or reads no target, or there are no
/// sightings at all (§2.2: "every detector unknown or absent").
#[must_use]
pub fn finds_nothing(state: &Value) -> bool {
    let [sightings_key, _] = REFLEX_DECIDE_STATE_KEYS;
    !state
        .get(sightings_key)
        .and_then(Value::as_array)
        .is_some_and(|sightings| {
            sightings.iter().any(|sighting| {
                sighting.get("unknown").is_none_or(Value::is_null)
                    && sighting
                        .get("value")
                        .and_then(Value::as_i64)
                        .is_some_and(|value| value != 0)
            })
        })
}

/// The request's `questions`: the one closed choice, its options asked by
/// their words.
#[must_use]
pub fn questions() -> Value {
    let criteria: Map<String, Value> = REFLEX_DECIDE_OPTIONS
        .iter()
        .map(|(word, covers)| ((*word).to_string(), Value::from(*covers)))
        .collect();
    choice::asked(REFLEX_DECIDE_QUESTION, REFLEX_DECIDE_ASKS, criteria)
}

/// The options an answer is read against: their words.
#[must_use]
pub fn offered() -> BTreeSet<String> {
    REFLEX_DECIDE_OPTIONS
        .iter()
        .map(|(word, _)| (*word).to_string())
        .collect()
}

/// One decision the run asked, or is waiting to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    /// The decision's number in its run, from 1.
    pub id: u64,
    pub snapshot: Snapshot,
}

/// What a new reading of the run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// The same state as the reading before it: no decision.
    Same,
    /// A decision to ask now — nothing was in flight.
    Ask(Pending),
    /// A decision merged into the one waiting behind the question in flight;
    /// the one it replaced, if any, is `coalesced` and asked never.
    Waiting { coalesced: Option<Pending> },
}

/// One run's questions: at most one in flight, and behind it the newest
/// reading that changed — every reading it replaced is merged (`coalesced`)
/// and never asked. A reading the same as the one before it is no decision.
#[derive(Debug, Default)]
pub struct Decider {
    last: Option<Value>,
    in_flight: Option<u64>,
    waiting: Option<Pending>,
    next: u64,
}

impl Decider {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn number(&mut self, snapshot: Snapshot) -> Pending {
        self.next += 1;
        Pending {
            id: self.next,
            snapshot,
        }
    }

    /// Take a new reading of the run.
    pub fn offer(&mut self, snapshot: Snapshot) -> Offer {
        if self.last.as_ref() == Some(&snapshot.state) {
            return Offer::Same;
        }
        self.last = Some(snapshot.state.clone());
        let pending = self.number(snapshot);
        if self.in_flight.is_some() {
            let coalesced = self.waiting.replace(pending);
            return Offer::Waiting { coalesced };
        }
        self.in_flight = Some(pending.id);
        Offer::Ask(pending)
    }

    /// The question in flight came back, whatever it came to: the one waiting
    /// behind it, if any, is asked next.
    pub fn settled(&mut self, id: u64) -> Option<Pending> {
        if self.in_flight != Some(id) {
            return None;
        }
        self.in_flight = None;
        let next = self.waiting.take()?;
        self.in_flight = Some(next.id);
        Some(next)
    }

    /// Whether a question is in flight.
    #[must_use]
    pub const fn asking(&self) -> bool {
        self.in_flight.is_some()
    }

    /// The run ended: the reading still waiting is never asked — it leaves as
    /// merged.
    pub fn close(&mut self) -> Option<Pending> {
        self.waiting.take()
    }
}

/// How a decision's row names the road it took; the rows' roads add up to the
/// run's decisions.
pub const ROAD_JEV: &str = "jev";
/// Refused before the wire: the door's word is the row's outcome.
pub const ROAD_DOOR: &str = "door";
/// Merged into a later reading while a question was in flight.
pub const ROAD_COALESCED: &str = "coalesced";
/// The roads a decision's answer may come down before the wire (t-10223
/// §2.2): the judgment memo, and a local stand-in fitted on the teacher's
/// rows. Named, so an account of the roads counts them, and answering
/// nothing yet.
pub const ROAD_MEMO: &str = "memo";
pub const ROAD_SURROGATE: &str = "surrogate";

/// The outcome of a question sent and answered while the seat still asks.
pub const ANSWERED: &str = "answered";
/// A question sent whose seat was switched off before its answer came: the
/// request stays counted, its answer is not kept.
pub const WITHDRAWN: &str = "withdrawn";

/// What one question came to on the wire, as the window measured it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wired {
    /// The endpoint's body, or the word for why there is none — a door's
    /// refusal before the wire, or the wire's failure after it.
    pub answer: Result<String, String>,
    /// Requests that left for the endpoint: 1 for any question the door let
    /// through, whatever became of it, 0 for one it refused.
    pub attempts: u32,
    pub request_bytes: usize,
    pub rtt_ms: u64,
}

/// The row a decision the run asked leaves: the state it asked about and
/// where that state came from, fixed when it was asked; what the wire came
/// to; and — for an answer — the teacher's word about THAT state, marked
/// `staleForCurrent` when the run's scene is no longer the one it was asked
/// on. A newer capture of the same scene keeps it; nothing here rewrites the
/// state asked to a later one. It leaves `applied` false: whether an answer
/// was carried out is the autopilot's to write ([`carried`]).
#[must_use]
pub fn asked_row(
    run: &str,
    pending: &Pending,
    wired: &Wired,
    now: Option<&Scene>,
    still_asking: bool,
    run_ended: bool,
) -> Value {
    let mut row = json!({
        "run": run,
        "decision": pending.id,
        "road": if wired.attempts > 0 { ROAD_JEV } else { ROAD_DOOR },
        "rubricVersion": REFLEX_DECIDE_RUBRIC_VERSION,
        "rubric": reflex_decide_rubric_fingerprint(),
        "state": pending.snapshot.state,
        "provenance": pending.snapshot.provenance(),
        "attempts": wired.attempts,
        "requestBytes": wired.request_bytes,
        "rttMs": wired.rtt_ms,
        "applied": false,
    });
    let body = match &wired.answer {
        Err(token) => {
            row["outcome"] = json!(token);
            return row;
        }
        Ok(body) => body,
    };
    if !still_asking {
        row["outcome"] = json!(WITHDRAWN);
        return row;
    }
    let read = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|parsed| parsed.get("answers").cloned())
        .ok_or(choice::ChoiceRefusal::NoAnswer)
        .and_then(|answers| choice::read(&answers, REFLEX_DECIDE_QUESTION, &offered()));
    match read {
        Ok(pick) => {
            row["outcome"] = json!(ANSWERED);
            row["chosen"] = json!(pick.chosen);
            row["probabilities"] = json!(pick.probabilities);
            row["confidence"] = json!(pick.confidence);
            // The teacher's word on the state asked, not a verdict on the run:
            // a receipt's `done` is not a label.
            row["labelSource"] = json!("teacher");
            row["staleForCurrent"] = json!(pending.snapshot.scene.as_ref() != now);
            row["late"] = json!(run_ended);
        }
        Err(refusal) => row["outcome"] = json!(refusal.token()),
    }
    row
}

/// The row a merged decision leaves: its number and where its reading came
/// from — never asked, so no state and no answer.
#[must_use]
pub fn coalesced_row(run: &str, pending: &Pending) -> Value {
    json!({
        "run": run,
        "decision": pending.id,
        "road": ROAD_COALESCED,
        "rubricVersion": REFLEX_DECIDE_RUBRIC_VERSION,
        "provenance": pending.snapshot.provenance(),
        "attempts": 0,
        "applied": false,
    })
}

// ---- what may be carried out ------------------------------------------------

/// What the autopilot that asked a question remembers of its run beside it
/// (§2.2, 2b): the epoch of the plan the run was started with, that plan's
/// hash, and whether a bench forced the seat's mode (D5). Every row the run's
/// questions leave keeps it in its provenance, so the verdict, the label and
/// a replay read the same grounds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    pub epoch: u64,
    pub plan_hash: String,
    pub forced: bool,
}

impl Stamp {
    /// Write this stamp into a decision row's provenance.
    pub fn stamp(&self, row: &mut Value) {
        let Some(provenance) = row.get_mut("provenance").and_then(Value::as_object_mut) else {
            return;
        };
        provenance.insert("epoch".into(), json!(self.epoch));
        provenance.insert("planHash".into(), json!(self.plan_hash));
        provenance.insert("forced".into(), json!(self.forced));
    }

    /// The stamp a decision row's provenance keeps, if it keeps one.
    #[must_use]
    pub fn of(row: &Value) -> Option<Self> {
        let provenance = row.get("provenance")?;
        Some(Self {
            epoch: provenance.get("epoch")?.as_u64()?,
            plan_hash: provenance.get("planHash")?.as_str()?.to_string(),
            forced: provenance.get("forced")?.as_bool()?,
        })
    }
}

/// The run a decision would be carried out on now: its id, the epoch of the
/// plan it was started with, and the hash of the plan the helper says it
/// runs (its status's `planHash`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running<'a> {
    pub run: &'a str,
    pub epoch: u64,
    pub plan_hash: &'a str,
}

/// Why an answered decision was not carried out (§2.2, 2h) — one word on its
/// row, and one count of the autopilot's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The seat does not apply: `shadow`, `off` since it was asked, or an
    /// `auto` its evidence has not raised.
    NotAuto,
    /// Its reading was older than [`REFLEX_APPLY_MAX_AGE_MS`] by the time the
    /// answer came back, or could not be dated.
    Stale,
    /// Another run or epoch than the one running asked it: the run it was
    /// about was stopped, re-planned or ended.
    EpochMismatch,
    /// The helper runs another plan than the one the run was started with.
    PlanMismatch,
}

impl Why {
    pub const ALL: [Self; 4] = [
        Self::NotAuto,
        Self::Stale,
        Self::EpochMismatch,
        Self::PlanMismatch,
    ];

    /// The word a row and a status name it by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::NotAuto => "not_auto",
            Self::Stale => "stale",
            Self::EpochMismatch => "epoch_mismatch",
            Self::PlanMismatch => "plan_mismatch",
        }
    }
}

/// Whether an answer that came back inside its wall may be carried out
/// (§2.2, 1d) — the answer's grounds before the seat's word: about the run
/// and epoch running now (`asked_run` and the row's [`Stamp`]), the plan the
/// helper runs, and a reading no older than [`REFLEX_APPLY_MAX_AGE_MS`]
/// (`age_ms`, [`age_at`]); then a seat that applies. So a row under `shadow`
/// still says whether the answer would have been fit to carry out.
///
/// # Errors
///
/// [`Why`] names the first ground the answer fails.
pub fn verdict(
    asked_run: &str,
    stamp: &Stamp,
    running: Option<Running<'_>>,
    age_ms: Option<u64>,
    applies: bool,
) -> Result<(), Why> {
    let running = running
        .filter(|running| running.run == asked_run && running.epoch == stamp.epoch)
        .ok_or(Why::EpochMismatch)?;
    if running.plan_hash != stamp.plan_hash {
        return Err(Why::PlanMismatch);
    }
    if age_ms.is_none_or(|age| age > REFLEX_APPLY_MAX_AGE_MS) {
        return Err(Why::Stale);
    }
    if !applies {
        return Err(Why::NotAuto);
    }
    Ok(())
}

/// Write a verdict onto its decision's row: `applied`, and the word of why
/// not.
pub fn carried(row: &mut Value, verdict: Result<(), Why>) {
    row["applied"] = json!(verdict.is_ok());
    if let Err(why) = verdict {
        row["why"] = json!(why.word());
    }
}

// ---- what an answer is graded by ---------------------------------------------

/// A reflex label's kind (§2.2): what the hand did after the answer, graded
/// by rule — a later fact, never a second reader's choice.
pub const LABEL_KIND: &str = "executed_outcome";
/// The key a label row carries its kind under.
pub const KIND_KEY: &str = "kind";
/// Why a label grades nothing ([`NOT_COMPARED`]): the window offered no
/// target and the hand went for none, so no share can be read of it.
pub const NOTHING_OFFERED: &str = "nothing_offered";
/// The run stopped at the answer, and a screen shows a stopped hand nothing:
/// only a bench's oracle still counts the targets it drew.
pub const HAND_STOPPED: &str = "hand_stopped";
/// A re-plan nobody carried out: there is no second plan to compare.
pub const NOT_REPLANNED: &str = "not_replanned";
/// The run ended before the window closed.
pub const WINDOW_CUT: &str = "window_cut";

/// How much of what a window offered the hand pressed, read one of two ways
/// and never the two mixed (§2.2, 2e): a target's life is a fraction of a
/// second and a phase a few, so a count of hits follows how many targets
/// there were, and a share does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// A bench's: the targets its fixture's oracle drew, and those it saw hit.
    Bench { hit: u64, offered: u64 },
    /// A screen's: the clicks that landed, and the targets the hand went for
    /// and did not press (`computer_use::REFLEX_MISSED_OUTCOMES`).
    Screen { pressed: u64, missed: u64 },
}

impl Share {
    /// Hits over what could have been hit; `None` over nothing.
    #[must_use]
    pub fn ratio(self) -> Option<f64> {
        let (part, whole) = match self {
            Self::Bench { hit, offered } => (hit, offered),
            Self::Screen { pressed, missed } => (pressed, pressed.saturating_add(missed)),
        };
        #[allow(clippy::cast_precision_loss)] // counts of one window, far under 2^52
        (whole > 0).then(|| part as f64 / whole as f64)
    }

    /// The share as a label row names it, its kind first.
    #[must_use]
    pub fn json(self) -> Value {
        match self {
            Self::Bench { hit, offered } => {
                json!({ "kind": "bench", "hit": hit, "offered": offered })
            }
            Self::Screen { pressed, missed } => {
                json!({ "kind": "screen", "pressed": pressed, "missed": missed })
            }
        }
    }
}

/// What the hand did in one label window ([`crate::computer_use::REFLEX_LABEL_WINDOW_MS`]
/// after an answer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub share: Share,
    /// Presses that landed on something no target was: a bench's oracle
    /// counts them; a screen cannot tell one from a hit and counts none.
    pub wrong: u64,
    /// Every reading through the window found nothing ([`finds_nothing`]).
    pub blind: bool,
    /// The hand stayed live through the window: nothing stopped the run.
    pub live: bool,
}

/// Whether `chosen` was right about `window` (§2.2, D3), or the word for why
/// nothing can say: `continue` was right when the hand pressed some of what
/// it was offered and nothing else, and wrong when it found nothing at all or
/// pressed none of it; `pause` was right when there was nothing to press. A
/// re-plan is graded across two runs ([`replan_graded`]).
///
/// # Errors
///
/// The [`NOT_COMPARED`] word for a window nothing can be read of.
pub fn graded(chosen: &str, window: &Window) -> Result<bool, &'static str> {
    match chosen {
        CONTINUE if window.blind => Ok(false),
        CONTINUE => {
            let ratio = window.share.ratio().ok_or(NOTHING_OFFERED)?;
            Ok(window.wrong == 0 && ratio > 0.0)
        }
        PAUSE => match window.share {
            Share::Bench { offered, .. } => Ok(offered == 0),
            Share::Screen { .. } if !window.live => Err(HAND_STOPPED),
            Share::Screen { pressed, missed } => Ok(window.blind && pressed + missed == 0),
        },
        _ => Err(NOT_REPLANNED),
    }
}

/// Whether a re-plan was right (§2.2): the new plan's share over its first
/// [`crate::computer_use::REFLEX_REPLAN_COMPARE_MS`] above the old plan's
/// over its last.
///
/// # Errors
///
/// [`NOTHING_OFFERED`] when either side offered nothing.
pub fn replan_graded(before: Share, after: Share) -> Result<bool, &'static str> {
    let (Some(before), Some(after)) = (before.ratio(), after.ratio()) else {
        return Err(NOTHING_OFFERED);
    };
    Ok(after > before)
}

/// An answer's marks on one window: whether it was right, and whether the
/// seat's baseline — `continue`, the hand with no seat — was right about the
/// same window. A baseline mark rides only beside a mark of the seat's own.
///
/// # Errors
///
/// The [`NOT_COMPARED`] word of [`graded`].
pub fn marks_on(chosen: &str, window: &Window) -> Result<(bool, Option<bool>), &'static str> {
    graded(chosen, window).map(|agreed| (agreed, graded(CONTINUE, window).ok()))
}

/// A carried-out re-plan's marks: [`replan_graded`], and the baseline's —
/// keeping the old plan was right exactly when the re-plan was not.
///
/// # Errors
///
/// The [`NOT_COMPARED`] word of [`replan_graded`].
pub fn replan_marks(before: Share, after: Share) -> Result<(bool, Option<bool>), &'static str> {
    replan_graded(before, after).map(|agreed| (agreed, Some(!agreed)))
}

/// The label row that grades one decision (§2.2): the request named as the
/// seat's row says ([`crate::jev::JevUse::request_name`] — its run and its
/// decision, joined by `:`) and by the time it was asked (its row's `at`,
/// [`REQUEST_AT`]), its kind, what was measured, and its marks — or the word
/// for why it carries none.
#[must_use]
pub fn label_row(
    run: &str,
    decision: u64,
    request_at: i64,
    measured: Value,
    marks: Result<(bool, Option<bool>), &'static str>,
) -> Value {
    let mut row = json!({
        (LABEL.canonical): format!("{run}:{decision}"),
        (REQUEST_AT.canonical): request_at,
        (KIND_KEY): LABEL_KIND,
        "share": measured,
    });
    match marks {
        Ok((agreed, baseline)) => {
            row[AGREED.canonical] = json!(agreed);
            if let Some(baseline) = baseline {
                row[BASELINE_AGREED.canonical] = json!(baseline);
            }
        }
        Err(word) => row[NOT_COMPARED.canonical] = json!(word),
    }
    row
}

#[cfg(test)]
mod tests;
