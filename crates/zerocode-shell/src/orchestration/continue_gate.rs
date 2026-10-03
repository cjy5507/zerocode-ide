//! The continue gate on the beat (t-26583): every live worker judged every
//! second from what the window already holds of it, and what the judgment calls
//! for carried out.
//!
//! The judgment and the plan are pure and live in core
//! ([`zerocode_core::continue_gate`]); the memory is [`super::gate_book`]; this is
//! the beat's half — which panes are judged, what is read for them, and what is
//! done. Three rules keep it cheap and keep it safe:
//!
//! - **Nothing runs while a lock is held.** A beat decides under the book's lock
//!   (a judgment and a plan, microseconds), carries the plan out without it (a
//!   git snapshot, a verb, a letter — each of which can take a moment) and
//!   records what happened under it again. The hook road feeds the same book from
//!   its own task and must never wait on a git process.
//! - **Everything done is behind one door** ([`GateDoor`]): a snapshot, a letter
//!   to the coordinator, a stop. The production door is the window's ([`LiveDoor`]);
//!   a test's is a list.
//! - **A stop is the one act that cannot be taken back**, and is taken only in
//!   the mode a person chose for it, after the worker's tree was saved, through
//!   the same seat and the same verb a coordinator ends a worker with — and it
//!   leaves a decision gate in front of the task, so the next summons is a
//!   person's decision and not a loop.

use std::path::Path;

use zerocode_core::continue_gate::plan::Act;
use zerocode_core::continue_gate::{Acted, Allowance, Cap, Judgement, Mode, Settings, Verdict};
use zerocode_core::launch::LaunchOverride;
use zerocode_core::orchestration::GateReceipt;

use super::gate_book::{self, GateReading, PaneGate, SnapshotNote};
use super::gate_meter::{CostNote, Meter};
use super::step_effort::WorkerPane;
use crate::agent_teams::Host;

/// What the gate does to the world, behind one door so a test can stand in for it.
pub(super) trait GateDoor {
    /// Saves `pane`'s tree as checkpoint `number`: the ref, or `None` for a tree
    /// with nothing to save.
    fn snapshot(&self, pane: &WorkerPane, number: u32) -> Result<Option<String>, String>;

    /// Writes the receipt in the ledger's own voice; whether a row was written.
    fn tell(&self, receipt: GateReceipt, now_ms: i64) -> bool;

    /// Ends the worker and puts a decision gate in front of its task.
    fn stop(&self, pane: &WorkerPane, why: &str, now_ms: i64) -> Result<(), String>;
}

/// What one beat knows about the spend around a worker.
pub(super) struct Around {
    pub(super) allowance: Allowance,
    pub(super) task_spent: f64,
    pub(super) day_spent: f64,
    pub(super) cost: CostNote,
}

/// What a judgment calls for.
struct Decided {
    judgement: Judgement,
    acts: Vec<Act>,
    /// The number the next snapshot of this attempt carries.
    number: u32,
}

/// What carrying the plan out came to.
struct Done {
    snapshot: Option<SnapshotNote>,
    acted: Acted,
}

/// A judgment and a plan from the pane's record — the part that runs under the
/// book's lock.
fn decide(gate: &mut PaneGate, settings: Settings, allowance: &Allowance, now_ms: i64) -> Decided {
    let judgement = gate.book.judge(allowance, now_ms);
    let acts = gate.standing.plan(&judgement, settings.mode, now_ms);
    Decided {
        number: gate.book.metrics().checkpoints.saturating_add(1),
        judgement,
        acts,
    }
}

/// The plan carried out, in the order that makes it safe: the tree saved first,
/// then the worker ended, then the coordinator told what was done.
fn carry_out(
    door: &dyn GateDoor,
    pane: &WorkerPane,
    decided: &Decided,
    earlier: Option<&SnapshotNote>,
    now_ms: i64,
) -> Done {
    let snapshot =
        decided
            .acts
            .contains(&Act::Snapshot)
            .then(|| match door.snapshot(pane, decided.number) {
                Ok(reference) => SnapshotNote {
                    at_ms: now_ms,
                    reference,
                    error: None,
                },
                Err(why) => SnapshotNote {
                    at_ms: now_ms,
                    reference: None,
                    error: Some(why),
                },
            });
    let acted = if decided.acts.contains(&Act::Stop) {
        match door.stop(pane, &stop_reason(&decided.judgement), now_ms) {
            Ok(()) => Acted::Stopped,
            Err(_) => Acted::StopFailed,
        }
    } else {
        Acted::Told
    };
    if let Some(verdict) = decided.acts.iter().find_map(|act| match act {
        Act::Tell(verdict) => Some(*verdict),
        Act::Snapshot | Act::Stop => None,
    }) {
        let saved = snapshot
            .as_ref()
            .or(earlier)
            .and_then(|note| note.reference.clone());
        door.tell(
            GateReceipt {
                key: format!("gate-{}-{}", pane.dispatch, verdict.word()),
                worker: pane.worker.clone(),
                dispatch: pane.dispatch.clone(),
                verdict,
                acted,
                reasons: decided.judgement.reasons.clone(),
                metrics: decided.judgement.metrics.clone(),
                snapshot: saved,
            },
            now_ms,
        );
    }
    Done { snapshot, acted }
}

/// What happened, into the pane's record — and the reading the board shows.
fn record(
    gate: &mut PaneGate,
    decided: Decided,
    done: &Done,
    settings: Settings,
    around: &Around,
    now_ms: i64,
) {
    if let Some(note) = &done.snapshot {
        gate.snapshot = Some(note.clone());
        // An attempt was made: the count starts over whether or not git agreed,
        // so a checkout that cannot be saved is not asked again every beat.
        gate.book.checkpointed(now_ms);
    }
    gate.cost = around.cost;
    gate.reading = Some(GateReading {
        mode: settings.mode,
        verdict: decided.judgement.verdict,
        reasons: decided.judgement.reasons,
        metrics: decided.judgement.metrics,
        cost: around.cost,
        task_spent_usd: around.task_spent,
        task_limit_usd: settings.task_usd,
        day_spent_usd: around.day_spent,
        day_limit_usd: settings.day_usd,
        snapshot: gate.snapshot.clone(),
        at_ms: now_ms,
    });
}

/// The ledger's note of why a worker was ended: the budget it would have spent.
fn stop_reason(judgement: &Judgement) -> String {
    judgement
        .reasons
        .iter()
        .find(|reason| reason.code.verdict() == Verdict::Stop)
        .map_or_else(
            || "gate".to_string(),
            |reason| {
                format!(
                    "gate: {} — ${:.2} of ${:.2}",
                    reason.code.word(),
                    reason.value,
                    reason.limit
                )
            },
        )
}

/// One worker's turn at the gate, whole — what the tests drive. The sweep runs
/// the same three parts with the book's lock released around the middle.
#[cfg(test)]
pub(super) fn step(
    gate: &mut PaneGate,
    pane: &WorkerPane,
    settings: Settings,
    around: &Around,
    door: &dyn GateDoor,
    now_ms: i64,
) {
    let decided = decide(gate, settings, &around.allowance, now_ms);
    let done = carry_out(door, pane, &decided, gate.snapshot.as_ref(), now_ms);
    record(gate, decided, &done, settings, around, now_ms);
}

/// The budgets that apply to one worker, from what a person set and what the
/// window has watched spent.
fn allowance_of(
    settings: Settings,
    task_spent: f64,
    day_spent: f64,
    own_ahead: f64,
    day_ahead: f64,
) -> Allowance {
    Allowance {
        task: settings.task_usd.map(|limit_usd| Cap {
            limit_usd,
            spent_usd: task_spent,
            ahead_usd: own_ahead,
        }),
        day: settings.day_usd.map(|limit_usd| Cap {
            limit_usd,
            spent_usd: day_spent,
            ahead_usd: day_ahead,
        }),
    }
}

/// One beat of the gate: every live worker read, judged, and acted on.
pub(super) fn sweep(host: &dyn Host, overrides: &[(String, LaunchOverride)], now_ms: i64) {
    let settings = gate_book::settings();
    if settings.mode == Mode::Off {
        gate_book::book().clear_readings();
        return;
    }
    let Some(held) = super::runtime() else {
        return;
    };
    let Ok(image) = held.actor.view() else {
        return;
    };
    let Ok(ledger) = super::cached_ledger(&held, &image) else {
        return;
    };
    let teams = crate::agent_teams::teams();
    let seats = super::index_team_seats(&teams);
    drop(teams);
    let panes = super::step_effort::worker_panes(&ledger, &seats);
    drop(ledger);
    drop(image);
    if panes.is_empty() {
        return;
    }
    let notes: Vec<CostNote> = panes.iter().map(|pane| watch(host, pane, now_ms)).collect();
    let terms: Vec<u32> = panes.iter().map(|pane| pane.term).collect();
    let door = LiveDoor { host, overrides };
    for (pane, cost) in panes.iter().zip(notes) {
        let (decided, around) = {
            let mut book = gate_book::book();
            let day_ahead = book.ahead_of(&terms);
            let day_spent = book.day_spent(now_ms);
            let task_spent = book.task_spent(&pane.task);
            let Some(gate) = book.pane_mut(pane.term) else {
                continue;
            };
            let around = Around {
                allowance: allowance_of(
                    settings,
                    task_spent,
                    day_spent,
                    gate.book.ahead_usd(),
                    day_ahead,
                ),
                task_spent,
                day_spent,
                cost,
            };
            (decide(gate, settings, &around.allowance, now_ms), around)
        };
        let earlier = gate_book::book()
            .pane_mut(pane.term)
            .and_then(|gate| gate.snapshot.clone());
        let done = carry_out(&door, pane, &decided, earlier.as_ref(), now_ms);
        if let Some(gate) = gate_book::book().pane_mut(pane.term) {
            record(gate, decided, &done, settings, &around, now_ms);
        }
    }
}

/// The calls a worker's CLI recorded since the last look, into the book: the
/// transcript is read with no lock held, and only what it said goes under it.
fn watch(host: &dyn Host, pane: &WorkerPane, now_ms: i64) -> CostNote {
    let path = host
        .provider_session(pane.term)
        .and_then(|session| session.transcript_path);
    let mut meter = {
        let mut book = gate_book::book();
        let gate = book.pane(pane.term, "", now_ms);
        gate.for_attempt(&pane.dispatch);
        gate.meter.take().unwrap_or_else(|| Meter::new(&pane.agent))
    };
    let costs = meter.poll(path.as_deref(), now_ms);
    let note = meter.note(path.is_some());
    let mut book = gate_book::book();
    book.add_costs(pane.term, &pane.task, &costs, now_ms);
    if let Some(gate) = book.pane_mut(pane.term) {
        gate.meter = Some(meter);
    }
    note
}

/// The production door: git in the worker's checkout, the actor for the letter,
/// and the coordinator's seat for the stop.
struct LiveDoor<'a> {
    host: &'a dyn Host,
    overrides: &'a [(String, LaunchOverride)],
}

impl GateDoor for LiveDoor<'_> {
    fn snapshot(&self, pane: &WorkerPane, number: u32) -> Result<Option<String>, String> {
        let checkout = pane
            .checkout
            .as_deref()
            .ok_or_else(|| "the worker reported no checkout".to_string())?;
        super::gate_snapshot::save(Path::new(checkout), &pane.worker, number)
    }

    fn tell(&self, receipt: GateReceipt, now_ms: i64) -> bool {
        super::record_gate_judgement(receipt, now_ms).unwrap_or(false)
    }

    fn stop(&self, pane: &WorkerPane, why: &str, now_ms: i64) -> Result<(), String> {
        super::stop_worker_for_gate(
            self.host,
            self.overrides,
            &super::GateStop {
                run: &pane.run,
                worker: &pane.worker,
                dispatch: &pane.dispatch,
                task: &pane.task,
                why,
            },
            now_ms,
        )
    }
}

#[cfg(test)]
mod tests;
