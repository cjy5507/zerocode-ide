//! Why a quiet worker stopped, asked of Jev when the marker table cannot say
//! (t-4538, docs/design/jev-settings-20260917.md §2 and §9).
//!
//! The stall sweep ([`super::notify_stalled_workers`]) asks the measured
//! marker table about every quiet worker pane first — a quota wall, a
//! transient API error — and a silence neither names is `went_quiet` news.
//! Under a person's `shadow` or `auto` on the Jev use table's stall row
//! (`zerocode_core::jev::STALL`, `smart.stallCause`), this module puts that
//! silence to Jev once: the pane's screen and its transcript's tail, through
//! the Jev door, and the answer is one row in `stall-cause.jsonl`. It changes
//! nothing the beat does — the news is written as before, nothing is typed,
//! nothing ends.
//!
//! Later beats read what followed off the ledger — the coordinator's mail, a
//! continuation's receipt, the worker's report, a stop, the terminal's death
//! (`zerocode_core::stall_cause::followed`) — and write it as the row's label:
//! the coordinator's actual action beside Jev's answer, which is the evidence
//! a later stage would promote the use on.
//!
//! A question waits up to [`STALL_CAUSE_DEADLINE`] for its answer, so it is
//! asked off the beat ([`Host::off_the_beat`]); the beat never waits on a
//! socket, and never reads a screen or a transcript for a silence it has
//! already looked at.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::jev::{JevMode, STALL, summary};
use zerocode_core::orchestration::Ledger;
use zerocode_core::stall_cause::{self, STALL_CAUSE_RUBRIC_VERSION, StallAsk, StallLook};

use crate::agent_teams::Host;
use crate::systemone::{SCHEMA, Wire, request_body};

/// How long one stall question may wait for its answer.
///
/// A record-only question holds nothing but its own thread, so its wall is
/// set by the answers it would lose rather than by anything it delays. zo's
/// record-only wall (8 s, the chat probe's) cut 4 of the recall ledger's 150
/// requests, while routing's slowest answered one took 6,798 ms
/// (docs/design/jev-token-diet-20260917.md §1.5) — the slow tail runs past
/// 8 s. Thirty seconds is a sixth of the 180 s the pane already stood quiet
/// before it was asked about (`QUIET_GRACE_MS`); an answer slower than that is
/// the row's `timeout`, which says the service was slow as plainly.
pub(crate) const STALL_CAUSE_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::STALL_APPLY_DEADLINE_MS);

/// The row's outcome for a question Jev answered in shape.
const ANSWERED: &str = "answered";

/// The row's outcome for a silence with nothing on its screen or in its
/// record to ask about.
const NO_LOOK: &str = "no_look";

/// One silence the sweep may ask about: a quiet worker carrying an open
/// attempt, whose own words the marker table could not name.
pub(super) struct Silence {
    pub(super) run: String,
    pub(super) worker: String,
    pub(super) dispatch: String,
    pub(super) task: String,
    pub(super) agent: String,
    pub(super) term: u32,
    pub(super) since_ms: i64,
    /// The checkout its pane sits in, as the window reported it — the
    /// workspace the words come from, which the door asks consent for.
    pub(super) checkout: Option<String>,
}

impl Silence {
    /// The key a question's row and its label share: one attempt, one
    /// silence.
    fn key(&self) -> String {
        format!("{}@{}", self.dispatch, self.since_ms)
    }
}

/// An answered question still waiting for what followed it.
struct Waiting {
    key: String,
    run: String,
    worker: String,
    dispatch: String,
    asked_ms: i64,
    /// The cause the answer named, so its label can say whether what
    /// followed is what that cause predicts (`agreed`).
    cause: stall_cause::Cause,
    /// How sure the answer was, as the notice to the coordinator says it.
    confidence: f64,
}

/// What this window remembers about the silences it asked about.
#[derive(Default)]
pub(super) struct StallBook {
    /// The attempts whose silence has been looked at, by dispatch. One stays
    /// while its worker stays quiet, so a silence is looked at once, when it
    /// is first seen; the next silence of the same attempt is a new one.
    seen: HashSet<String>,
    /// The cause the seat named for an attempt still quiet, by dispatch —
    /// what the sweep acts on when the seat acts ([`acting_cause`]). Kept
    /// exactly as long as `seen` keeps the silence.
    answered: HashMap<String, (stall_cause::Cause, f64)>,
    /// The ledger the rows go to, and the answered rows without a label yet.
    /// `None` until a beat has read that ledger's tail, so the rows a window
    /// restart left unlabeled are labeled too.
    waiting: Option<(PathBuf, Vec<Waiting>)>,
}

/// Put each silence the sweep found that nobody has looked at yet to Jev,
/// off the beat. `still_quiet` is every attempt the sweep found quiet this
/// beat, whatever its cause: an attempt that is not among them has been heard
/// from, and its next silence is a new one.
pub(super) fn ask_about(
    host: &dyn Host,
    book: &Arc<Mutex<StallBook>>,
    silences: Vec<Silence>,
    still_quiet: &HashSet<String>,
    now_ms: i64,
) {
    let fresh: Vec<Silence> = {
        let mut held = book.lock().unwrap_or_else(|held| held.into_inner());
        held.seen.retain(|dispatch| still_quiet.contains(dispatch));
        held.answered
            .retain(|dispatch, _| still_quiet.contains(dispatch));
        silences
            .into_iter()
            .filter(|one| held.seen.insert(one.dispatch.clone()))
            .collect()
    };
    if fresh.is_empty() {
        return;
    }
    let Some(wire) = host.jev_wire() else {
        return;
    };
    let mode = STALL.mode_in(&wire.settings_root());
    let Some(ledger) = crate::systemone::ledger_of(&wire, &STALL).filter(|_| mode.asks()) else {
        return;
    };
    for one in fresh {
        let transcript = host
            .provider_session(one.term)
            .and_then(|session| session.transcript_path)
            .and_then(|path| zerocode_core::transcript::tail_lines(Path::new(&path)))
            .unwrap_or_default();
        // The table's second cause, asked of the same tail whether or not the
        // run declared a continuation: a silence the table names is its own.
        if crate::quota_wall::transient_error_in(&one.agent, &transcript).is_some() {
            continue;
        }
        let screen = host.capture(one.term).unwrap_or_default();
        let asked = stall_cause::ask(&StallLook {
            agent: &one.agent,
            quiet_ms: now_ms.saturating_sub(one.since_ms),
            screen: &screen,
            transcript: &transcript,
        });
        let question = Question {
            silence: one,
            mode,
            asked_ms: now_ms,
            asked,
        };
        let wire = wire.clone();
        let ledger = ledger.clone();
        let book = Arc::clone(book);
        host.off_the_beat(Box::new(move || {
            let (row, waiting) = settle(&wire, question);
            crate::systemone::record_rows(&STALL, &ledger, &[row], now_ms);
            // A book that has not read the ledger's tail yet reads this row
            // there; one that has takes it here, once.
            if let Some(waiting) = waiting {
                let mut held = book.lock().unwrap_or_else(|held| held.into_inner());
                held.answered.insert(
                    waiting.dispatch.clone(),
                    (waiting.cause, waiting.confidence),
                );
                if let Some((_, rows)) = &mut held.waiting
                    && !rows.iter().any(|row| row.key == waiting.key)
                {
                    rows.push(waiting);
                }
            }
        }));
    }
}

/// One question on its way: the silence, the switch it was asked under, when,
/// and what it asks — `None` when the pane showed nothing to ask about.
struct Question {
    silence: Silence,
    mode: JevMode,
    asked_ms: i64,
    asked: Option<StallAsk>,
}

/// Ask one question and write down what came of it: the row, and — for an
/// answer — the wait for its label.
fn settle(wire: &Wire, question: Question) -> (Value, Option<Waiting>) {
    let Question {
        silence,
        mode,
        asked_ms,
        asked,
    } = question;
    let mut row = json!({
        "at": asked_ms,
        "stall": silence.key(),
        "run": silence.run,
        "worker": silence.worker,
        "dispatch": silence.dispatch,
        "task": silence.task,
        "agent": silence.agent,
        "mode": mode.key(),
        "rubricVersion": STALL_CAUSE_RUBRIC_VERSION,
        "stalledSinceMs": silence.since_ms,
        "quietMs": asked_ms.saturating_sub(silence.since_ms),
    });
    let Some(ask) = asked else {
        row["outcome"] = json!(NO_LOOK);
        return (row, None);
    };
    let began = Instant::now();
    let answer = wire.ask(
        &STALL,
        silence.checkout.as_deref().map(Path::new),
        request_body(&ask.state, &ask.questions),
        STALL_CAUSE_DEADLINE,
    );
    row["elapsedMs"] = json!(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
    row["requestBytes"] = json!(answer.request_bytes);
    answer.spent.stamp(&mut row);
    let read = answer.answer.and_then(|body| {
        serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|parsed| ask.read(parsed.get("answers")?).ok())
            .ok_or_else(|| SCHEMA.to_string())
    });
    match read {
        Ok(choice) => {
            row["outcome"] = json!(ANSWERED);
            row["cause"] = json!(choice.cause.word());
            row["probabilities"] = json!(choice.probabilities);
            row["confidence"] = json!(choice.confidence);
            let waiting = Waiting {
                key: silence.key(),
                run: silence.run,
                worker: silence.worker,
                dispatch: silence.dispatch,
                asked_ms,
                cause: choice.cause,
                confidence: choice.confidence,
            };
            (row, Some(waiting))
        }
        Err(token) => {
            row["outcome"] = json!(token);
            (row, None)
        }
    }
}

/// The cause the seat named for `dispatch`, when the seat ACTS — a person's
/// `on`, or `auto` raised by the judge its own ledger recorded — and the
/// attempt is still the silence it was asked about. `None` otherwise: a
/// recording seat's answer is a row, never an order.
pub(super) fn acting_cause(
    host: &dyn Host,
    book: &Arc<Mutex<StallBook>>,
    dispatch: &str,
) -> Option<stall_cause::Cause> {
    let wire = host.jev_wire()?;
    if !crate::systemone::applies(&wire, &STALL) {
        return None;
    }
    let held = book.lock().unwrap_or_else(|held| held.into_inner());
    held.answered.get(dispatch).map(|(cause, _)| *cause)
}

/// The cause the seat last named for `dispatch`'s silence, whatever the seat's
/// standing — a fact the step-effort seat reads (`super::step_effort`) so it
/// holds a worker's effort where it is under a silence more reasoning does
/// not change. `None` for an attempt nobody asked about, or one heard from
/// since.
pub(super) fn answered_cause(
    book: &Arc<Mutex<StallBook>>,
    dispatch: &str,
) -> Option<stall_cause::Cause> {
    let held = book.lock().unwrap_or_else(|held| held.into_inner());
    held.answered.get(dispatch).map(|(cause, _)| *cause)
}

/// What the seat read off the panes still quiet this beat, for the
/// coordinator — when the seat ACTS, and for every cause the sweep does not
/// act on itself (a transient error is typed a continuation instead). One
/// reading per silence: the ledger keeps the notice keyed by the silence's
/// start, so the same answer on the next beat is the same fact.
pub(super) fn acting_judgments(
    host: &dyn Host,
    book: &Arc<Mutex<StallBook>>,
    quiet: impl Iterator<Item = (String, String, i64)>,
) -> Vec<zerocode_core::orchestration::StallJudged> {
    let Some(wire) = host.jev_wire() else {
        return Vec::new();
    };
    if !crate::systemone::applies(&wire, &STALL) {
        return Vec::new();
    }
    let held = book.lock().unwrap_or_else(|held| held.into_inner());
    quiet
        .filter_map(|(worker, dispatch, since_ms)| {
            let (cause, confidence) = held.answered.get(&dispatch)?;
            if *cause == stall_cause::Cause::TransientApiError {
                return None;
            }
            Some(zerocode_core::orchestration::StallJudged {
                worker,
                stalled_since_ms: since_ms,
                cause: cause.word().to_string(),
                confidence: *confidence,
            })
        })
        .collect()
}

/// The answered rows in the tail of `ledger` that no label row names yet.
fn unlabeled_in(ledger: &Path) -> Vec<Waiting> {
    let Some(lines) = zerocode_core::transcript::tail_lines(ledger) else {
        return Vec::new();
    };
    let rows: Vec<Value> = lines
        .iter()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect();
    let labeled: HashSet<&str> = rows
        .iter()
        .filter_map(|row| row["label"].as_str())
        .collect();
    rows.iter()
        .filter(|row| row["outcome"] == ANSWERED)
        .filter_map(|row| {
            let key = row["stall"].as_str()?;
            if labeled.contains(key) {
                return None;
            }
            Some(Waiting {
                key: key.to_string(),
                run: row["run"].as_str()?.to_string(),
                worker: row["worker"].as_str()?.to_string(),
                dispatch: row["dispatch"].as_str()?.to_string(),
                asked_ms: row["at"].as_i64()?,
                // A row an older window wrote without its cause is labeled
                // without a mark, as `unknown` is.
                cause: row["cause"]
                    .as_str()
                    .and_then(stall_cause::Cause::from_word)
                    .unwrap_or(stall_cause::Cause::Unknown),
                confidence: row["confidence"].as_f64().unwrap_or_default(),
            })
        })
        .collect()
}

/// Write the label of every answered question whose silence has been
/// followed by something, or whose window has closed — read off `ledger`,
/// this beat's image. A run the ledger no longer holds cannot say what
/// followed, and its row stays unlabeled rather than guessed at.
pub(super) fn label(host: &dyn Host, book: &Arc<Mutex<StallBook>>, ledger: &Ledger, now_ms: i64) {
    let (path, labels) = {
        let mut held = book.lock().unwrap_or_else(|held| held.into_inner());
        if held.waiting.is_none() {
            let Some(path) = host
                .jev_wire()
                .as_ref()
                .and_then(|wire| crate::systemone::ledger_of(wire, &STALL))
            else {
                return;
            };
            let rows = unlabeled_in(&path);
            held.waiting = Some((path, rows));
        }
        let Some((path, waiting)) = held.waiting.as_mut() else {
            return;
        };
        if waiting.is_empty() {
            return;
        }
        let mut labels = Vec::new();
        waiting.retain(|one| {
            let Some(run) = ledger.run(&one.run) else {
                return false;
            };
            let Some((what, at)) =
                stall_cause::followed(run, &one.worker, &one.dispatch, one.asked_ms, now_ms)
            else {
                return true;
            };
            let mut label = json!({
                "at": now_ms,
                "label": one.key,
                "run": one.run,
                "worker": one.worker,
                "dispatch": one.dispatch,
                "followed": what.word(),
                "followedAtMs": at,
                "afterMs": at.saturating_sub(one.asked_ms),
            });
            // The mark the judge counts (§4): the answer was right when its
            // cause predicts what the ledger then showed — whether the
            // silence needed the coordinator's hand. A side that says
            // nothing (`unknown`, a terminal that died) leaves its own word
            // instead of a mark it has no right to.
            match stall_cause::mark(one.cause, what) {
                Ok(agreed) => {
                    label[summary::AGREED.canonical] = json!(agreed);
                    // The seat's baseline on the same silence (t-6342).
                    if let Some(baseline) = stall_cause::baseline_mark(what) {
                        label[summary::BASELINE_AGREED.canonical] = json!(baseline);
                    }
                }
                Err(why) => label[summary::NOT_COMPARED.canonical] = json!(why),
            }
            labels.push(label);
            false
        });
        (path.clone(), labels)
    };
    crate::systemone::record_rows(&STALL, &path, &labels, now_ms);
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::*;

    /// A window that restarts reads the rows it left without a label back
    /// off the ledger's tail: answered ones only, and none a label already
    /// names.
    #[test]
    fn a_restart_finds_the_answers_still_waiting_for_their_label() {
        let dir = tempfile::tempdir().expect("a zo home");
        let ledger = dir.path().join(STALL.ledger);
        let row = |key: &str, outcome: &str| {
            json!({ "at": 5, "stall": key, "run": "run-1", "worker": "w-2",
                    "dispatch": "dp-3", "outcome": outcome })
        };
        crate::systemone::append_rows(
            &ledger,
            &[
                row("dp-3@1", ANSWERED),
                row("dp-3@2", "not_consented"),
                row("dp-3@3", ANSWERED),
                json!({ "at": 9, "label": "dp-3@1", "followed": "mail" }),
            ],
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&ledger)
            .and_then(|mut file| file.write_all(b"{\"torn\": "))
            .expect("a torn last line");

        let waiting = unlabeled_in(&ledger);
        let keys: Vec<&str> = waiting.iter().map(|one| one.key.as_str()).collect();
        assert_eq!(keys, ["dp-3@3"]);
        assert_eq!((waiting[0].run.as_str(), waiting[0].asked_ms), ("run-1", 5));
        assert!(unlabeled_in(&dir.path().join("gone.jsonl")).is_empty());
    }
}
