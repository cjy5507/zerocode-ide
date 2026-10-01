//! Which agent a summons would have been given, asked of Jev beside the three
//! words the coordinator typed (t-4711, docs/design/jev-settings-20260917.md
//! §2).
//!
//! `worker-start` lands on exactly the `--agent` it was given; the seat acts
//! only where the coordinator wrote `--agent auto` and left the choice to it
//! ([`choose`] — under `on`, or `auto` while its judge has not taken it
//! back; `auto` starts acting, t-11989). For every summons, whatever its
//! agent, this module also puts the summons'
//! SHAPE to Jev once, after the pane really opened: the head of the brief and
//! six facts about the ask, closed over the agents the quota gate says could
//! have carried it this minute and what this ledger's own record of each of
//! them is. The answer is one row in `summon-choice.jsonl`, beside what was
//! actually summoned and whether the two agreed.
//!
//! Asked after the pane opened, and only then: a summons whose split was
//! refused is not a decision anybody made, and a row about it would be
//! evidence about nothing. The checkout the pane landed in is the workspace
//! the door asks consent for — the words travel with the work.
//!
//! A question waits up to [`SUMMON_CHOICE_DEADLINE`] for its answer, so it is
//! asked off the beat ([`Host::off_the_beat`]); the summons has already been
//! answered to its caller by then and nothing waits on this.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::jev::SUMMON;
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
use zerocode_core::orchestration::{PreparedWorkerStart, SummonShadow};
use zerocode_core::summon_choice::{
    self, SUMMON_CHOICE_RUBRIC_VERSION, SummonAsk, WORKER_MODEL_KEY,
};

use crate::agent_teams::Host;
use crate::systemone::{SCHEMA, Wire, request_body};

/// How long one summons question may wait for its answer.
///
/// Record-only today, but the rows are evidence for a stage that would hold a
/// summons while it asked — a person is watching a pane open — so the
/// question is put under a wall a summons could actually wait behind rather
/// than under the generous one a background sweep can afford. zo's routing
/// judgment answered its slowest in 6,798 ms
/// (docs/design/jev-token-diet-20260917.md §1.5); ten seconds keeps that tail
/// and calls anything past it `timeout`, which says the service was slow as
/// plainly as a missing row would not.
pub(crate) const SUMMON_CHOICE_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::SUMMON_APPLY_DEADLINE_MS);

/// The row's outcome for a question Jev answered in shape.
const ANSWERED: &str = "answered";

/// The row's outcome for a summons with nothing to choose between: one agent
/// could have carried it, so there was no question to ask.
const ONE_OPTION: &str = "one_option";

/// Append one row through the window's one Jev-ledger door
/// ([`crate::systemone::record_rows`]) — which also judges the seat when a
/// judgment is due, on the `agreed` marks these rows carry.
fn append(ledger: &Path, row: &Value, now_ms: i64) {
    crate::systemone::record_rows(&SUMMON, ledger, std::slice::from_ref(row), now_ms);
}

/// Who the row is about: the reservation's own ids, which are the only part
/// of a summons this file needs from it.
struct Seat<'a> {
    run: &'a str,
    worker: &'a str,
    dispatch: Option<&'a str>,
    task: Option<&'a str>,
}

impl<'a> Seat<'a> {
    fn of(prepared: &'a PreparedWorkerStart) -> Self {
        Self {
            run: &prepared.run,
            worker: &prepared.worker,
            dispatch: prepared.dispatch.as_deref(),
            task: prepared.task.as_deref(),
        }
    }
}

/// Everything the row says before an answer comes back — the summons as the
/// ledger decided it.
fn opened(seat: &Seat<'_>, shadow: &SummonShadow, mode: &str, now_ms: i64) -> Value {
    json!({
        "at": now_ms,
        // What the outcome path joins a label on (t-16578): the request's
        // own time, as the difficulty and model seats' rows carry it.
        "requestAt": now_ms,
        "applied": false,
        // One summons, one row: the worker is the name every later reader
        // already has for this attempt.
        "summon": seat.worker,
        "run": seat.run,
        "worker": seat.worker,
        "dispatch": seat.dispatch,
        "task": seat.task,
        "mode": mode,
        "rubricVersion": SUMMON_CHOICE_RUBRIC_VERSION,
        // What the coordinator's three words came to, after the quota gate
        // had its say — the thing the judgment is written down beside.
        "agent": shadow.pinned.agent,
        WORKER_MODEL_KEY: shadow.pinned.model,
        "effort": shadow.pinned.effort,
        "modelWasPinned": shadow.model_was_pinned,
        // The shape that was asked about, so a reader of the row never has to
        // trust that the question carried what this says it did.
        "briefChars": shadow.brief_chars,
        "worktree": shadow.worktree,
        "replaces": shadow.replaces_an_attempt,
        "carriesATask": shadow.carries_a_task,
        "attempts": shadow.attempts,
        "failures": shadow.failures,
        REQUESTS_KEY: 0,
        REDACTED_LINES_KEY: 0,
    })
}

/// Put one summons to Jev, off the beat, and write down what came of it.
///
/// `checkout` is where the pane actually landed — the workspace the door asks
/// the person's consent for. Called once per summons that really opened.
pub(super) fn record(
    host: &dyn Host,
    prepared: &PreparedWorkerStart,
    checkout: Option<&str>,
    now_ms: i64,
) {
    let Some(shadow) = prepared.summon_shadow.clone() else {
        return;
    };
    let Some(wire) = host.jev_wire() else {
        return;
    };
    let mode = SUMMON.mode_in(&wire.settings_root());
    let Some(ledger) = crate::systemone::ledger_of(&wire, &SUMMON).filter(|_| mode.asks()) else {
        return;
    };
    let mut row = opened(&Seat::of(prepared), &shadow, mode.key(), now_ms);
    // The agent question rode the assign moment's one request (`--agent
    // auto`): its answer is the receipt's, and nothing is asked again.
    if let Some(receipt) = shadow.receipt.as_ref().and_then(Value::as_object) {
        row["options"] = json!(offered(&shadow));
        for (key, value) in receipt {
            row[key] = value.clone();
        }
        append(&ledger, &row, now_ms);
        return;
    }
    let Some(ask) = summon_choice::ask(&shadow.look(), &shadow.options) else {
        row["outcome"] = json!(ONE_OPTION);
        row["options"] = json!(offered(&shadow));
        append(&ledger, &row, now_ms);
        return;
    };
    let checkout = checkout.map(PathBuf::from);
    host.off_the_beat(Box::new(move || {
        append(
            &ledger,
            &settle(&wire, row, &ask, &shadow, checkout.as_deref()),
            now_ms,
        );
    }));
}

/// What the question said about each agent it offered, in the order it
/// offered them — the numbers, not the sentences they were poured into.
///
/// The row already promises that a reader never has to trust that the
/// question carried what it says it did, and until now `options` kept that
/// promise for the SET and broke it for the evidence: five ids, and no way to
/// tell afterwards what this window had read about them. A row that carries
/// the numbers is also the only row a later replay can be exact about, since
/// the ledger those numbers came from keeps moving.
///
/// Written for a question that was asked and for one that was not: a summons
/// with a single option still names what it knew about it.
fn offered(shadow: &SummonShadow) -> Vec<Value> {
    shadow
        .options
        .iter()
        .map(|agent| {
            json!({
                "id": agent.id,
                "spent": agent.spent_percent,
                "window": agent.window,
                "launched": agent.record.launched,
                "carried": agent.record.carried,
                "finished": agent.record.finished,
                "medianMinutes": agent.record.median_minutes,
            })
        })
        .collect()
}

/// Ask once and finish the row: what it cost, what came back, and whether the
/// judgment landed on the agent the summons already had.
fn settle(
    wire: &Wire,
    mut row: Value,
    ask: &SummonAsk,
    shadow: &SummonShadow,
    checkout: Option<&Path>,
) -> Value {
    // The set the answer will be judged against, and what the question said
    // about each of them — written down before it is asked: a row whose
    // options came from the answer would prove nothing. The order is the
    // order offered, which `the_row_names_every_option_the_question_carried`
    // holds against [`SummonAsk::options`].
    row["options"] = json!(offered(shadow));
    let began = Instant::now();
    let answer = wire.ask(
        &SUMMON,
        checkout,
        request_body(&ask.state, &ask.questions),
        SUMMON_CHOICE_DEADLINE,
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
        Ok(pick) => {
            row["outcome"] = json!(ANSWERED);
            row["chosen"] = json!(pick.chosen);
            // The one number this ledger exists to produce: how often the
            // coordinator's typing and a judgment of the work land on the
            // same agent.
            // No mark on a summons the seat itself chose for: there was no
            // coordinator's word to agree with, and a row that agreed with
            // its own answer would be a judge grading itself.
            // And none on a summons whose own agent was never offered: a
            // judgment that could not have named it did not disagree about
            // it, so the row says WHY in a word instead of a mark it has no
            // right to (t-4839).
            // Nor on one whose model was pinned (t-9087): the pin is the
            // person's word, an apply stage leaves the summons alone, and a
            // mark there grades the pin's own CLI rather than the seat.
            //
            // And no baseline mark on the rows that are compared (t-11989).
            // Today's rule is the launched model's own vendor CLI (t-6342),
            // and a summons that named no model launched on one its
            // difficulty profile took from the named agent's own rows — a
            // table that refuses a model of another agent — so the rule's
            // answer was the coordinator's word by construction, agreed
            // every time, and held the seat under a line no bound clears
            // (1,000‰ on the one such row this machine held, 2026-09-28).
            if !shadow.auto {
                if !ask.offered(&shadow.pinned.agent) {
                    row[summon_choice::NOT_COMPARED_KEY] = json!(summon_choice::NOT_OFFERED);
                } else if shadow.model_was_pinned {
                    row[summon_choice::NOT_COMPARED_KEY] = json!(summon_choice::PINNED);
                } else {
                    row["agreed"] = json!(pick.chosen == shadow.pinned.agent);
                }
            }
            row["probabilities"] = json!(pick.probabilities);
            row["confidence"] = json!(pick.confidence);
        }
        Err(token) => row["outcome"] = json!(token),
    }
    row
}

#[cfg(test)]
mod tests;

/// Label the agent seat's answered rows that ran by the work they launched —
/// the difficulty seat's own reader, over this seat's ledger (t-16578).
pub(super) fn observations(
    ledger: &zerocode_core::orchestration::Ledger,
    costs: &mut super::cost_book::CostBook,
) -> Option<(PathBuf, Vec<Value>)> {
    super::summon_difficulty::observations_of(&SUMMON, ledger, costs)
}

pub(super) fn record_observations(observations: Option<(PathBuf, Vec<Value>)>, now_ms: i64) {
    super::summon_difficulty::record_observations_of(&SUMMON, observations, now_ms);
}

/// The agent seat's row before any answer, on the assign moment's one
/// request: the agents it chose among.
pub(super) fn head(asked: &SummonAsk) -> Value {
    json!({
        "rubricVersion": SUMMON_CHOICE_RUBRIC_VERSION,
        "options": asked.options(),
        "applied": false,
    })
}

/// What the agent question got back — `answers`, or the word nothing came
/// back with — written onto its `row`: the agent, and whether it runs, which
/// is only an answer the seat acts on and its act line lets through.
pub(super) fn answered(
    wire: &Wire,
    asked: &SummonAsk,
    row: &mut Value,
    answers: Result<&Value, &str>,
) {
    let read = answers
        .map_err(str::to_string)
        .and_then(|answers| asked.read(answers).map_err(|err| err.token().to_string()));
    match read {
        Ok(pick) => {
            row["outcome"] = json!(ANSWERED);
            row["chosen"] = json!(pick.chosen);
            row["probabilities"] = json!(pick.probabilities);
            row["confidence"] = json!(pick.confidence);
            row["applied"] = json!(
                crate::systemone::applies(wire, &SUMMON)
                    && SUMMON.acts_on(pick.confidence, crate::systemone::act_line(wire, &SUMMON))
            );
        }
        Err(token) => row["outcome"] = json!(token),
    }
}

/// The seat's own pick for a summons typed `--agent auto`, asked alone —
/// how its own tests ask it. In the product the agent question rides the
/// assign moment's one request (`summon_assign`, t-16578), on the beat
/// because the summons waits for it, and only when the seat acts — a
/// person's `on`, or `auto` raised by the judge its own ledger recorded; a
/// pick its act line does not let through is refused as a failed one is.
/// `None` is a refusal the caller names; a guess is never handed back.
#[cfg(test)]
pub(super) fn choose_with(
    wire: &Wire,
    look: &zerocode_core::summon_choice::SummonLook<'_>,
    options: &[zerocode_core::summon_choice::Summonable],
    origin: [&str; 3],
) -> Option<String> {
    let ask = summon_choice::ask(look, options)?;
    let receipts = super::summon_assign::choose_with(
        wire,
        &zerocode_core::summon_assign::AssignAsk {
            agent: Some(ask),
            ..Default::default()
        },
        origin,
    );
    receipts
        .agent
        .filter(|row| row["applied"] == true)
        .and_then(|row| row["chosen"].as_str().map(str::to_string))
}
