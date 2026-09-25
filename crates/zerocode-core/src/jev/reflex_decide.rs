//! The reflex decision's runner, the part every reader of it shares (t-9205):
//! what a live reflex run's typed state is, how the one question is asked of
//! it, how a run keeps one question in flight and merges the rest, and what a
//! row says an answer was about. The window reads the run and asks
//! (`computer_use::reflex`); nothing here reaches the hand — the seat
//! ([`crate::jev::REFLEX_DECIDE`]) only records, and what it records is the
//! teacher's answer about the state that was asked, never about another.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::choice;
use super::questions::{
    REFLEX_DECIDE_ASKS, REFLEX_DECIDE_OPTIONS, REFLEX_DECIDE_QUESTION,
    REFLEX_DECIDE_RUBRIC_VERSION, REFLEX_DECIDE_STATE_KEYS, reflex_decide_rubric_fingerprint,
};
use crate::computer_use_protocol::reflex::{LIMITS, identifier};

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
    }
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
/// state asked to a later one, and nothing is applied.
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

#[cfg(test)]
mod tests;
