//! The two buttons on a quota-wall chip (t-39771): 「지금 다시 시도」 and
//! 「다른 에이전트로 넘기기」.
//!
//! **A person's press, and nothing else's.** Both roads here are called by one
//! window command each (`cmd/board.rs`), main window only, and the renderer
//! calls that command only after the person said yes in the window's confirm.
//! The beat never reaches them — `tick` and everything it calls name neither —
//! and no verb, request body or worker endpoint can produce the marker a press
//! stands on ([`zerocode_core::orchestration::PERSON_MARKER_SOURCE`]). A source
//! contract pins both.
//!
//! Neither is a new authority. The chip is a reading of rows the ledger already
//! keeps ([`zerocode_core::orchestration::wall_chip_for`]), and each road reads
//! it again at the moment of the press: a wall that lifted since the chip was
//! drawn, a worker an account switch put to sleep or seated again, a pane the
//! person took, an attempt that closed — all refuse here, in the ledger's own
//! words, whatever the screen still shows.
//!
//! - **지금 다시 시도** is the continuation road the beat walks for a transient
//!   API error (t-4537): the composer at rest by the pointer's own reading,
//!   [`resume_plan`], the receipt row reserved BEFORE a word is typed, the
//!   words through the pointer's one guarded door ([`Host::point`]), and the
//!   beat's `settle_resumes` writing what came of it. A press waives two of the
//!   plan's refusals (the declared order, the standing wall) and nothing else:
//!   the ceiling, the spacing, the row in flight and a key typed at already all
//!   stand — one press per wall episode of one attempt.
//! - **다른 에이전트로 넘기기** is the hand recipe the handover walk carries
//!   out under a declared order: `worker-stop … --reason quota-wall`, then
//!   `worker-start --retry-of … --inherit-checkout`, from the run's coordinator
//!   seat, with the argv the walk builds ([`handover_stop_argv`],
//!   [`handover_start_argv`]) — the same verbs and request names
//!   (`handover-<attempt>-stop|start`), so a press made twice, or a press and a
//!   walk that meet on one attempt, come to ONE replacement. It commits nothing
//!   for the worker: the replacement finds the tree as it was left.
//!
//! Both stand in the account switch's line only by `try_lock`
//! ([`crate::account_switch::unless_switching`]): a move in flight is a reason
//! to say so and ask again, never a reason to wait inside it.

use super::*;
use zerocode_core::orchestration::{
    HandoverPlan, NotResumed, QuotaWallChip, RESUME_IN_FLIGHT, ResumePlan, WALL_RETRY_LINE,
    handover_paragraph, handover_recap, newest_wall, parse_on_quota_wall, person_wall_marker,
    resume_plan, wall_cause, wall_chip_for,
};

/// Said when an account switch holds its line: the press is not lost, only
/// not made inside a move.
const SWITCH_IN_FLIGHT: &str =
    "an account switch is moving panes right now — ask again in a moment";

/// Said when the pointer's door could not address the pane: nothing was typed.
const NOT_ADDRESSED: &str = "the pane could not be addressed; nothing was typed";

/// The ledger's refusal of a continuation, in its words.
fn not_resumed(why: NotResumed) -> String {
    match why {
        NotResumed::InFlight => RESUME_IN_FLIGHT.to_string(),
        NotResumed::Refused(why) => why,
    }
}

/// The chip `worker_id` wears NOW and the terminal it sits in — read the way
/// the board's list reads it ([`ledger_agents_for_seats`]): the same rows, the
/// same seats, the same function.
fn standing_chip(
    run_id: &str,
    worker_id: &str,
    now_ms: i64,
) -> Result<(QuotaWallChip, u32), String> {
    with_ledger_seats(|ledger, seats| {
        let run = ledger
            .run(run_id)
            .ok_or_else(|| format!("unknown run: {run_id}"))?;
        let worker = run
            .worker(worker_id)
            .ok_or_else(|| format!("unknown worker: {worker_id}"))?;
        let term = seated_term(run, worker, seats);
        let chip = wall_chip_for(run, worker, wall_chip_seat(run, term, seats), now_ms)?;
        let term =
            term.ok_or_else(|| format!("this window holds no pane of worker {worker_id}"))?;
        Ok((chip, term))
    })
    .unwrap_or_else(|| Err(NO_RUNTIME.to_string()))
}

/// Why the ledger would not reserve a continuation it was just asked for: its
/// own rule read again on the rows as they are now — or, the rows agreeing,
/// that the worker, its seat or its attempt moved between the two readings.
fn reservation_refused(held: &LiveRuntime, plan: &ResumePlan, now_ms: i64) -> String {
    held.actor
        .view()
        .ok()
        .and_then(|image| {
            let rows = cached_ledger(held, &image).ok()?;
            let run = rows.run(&plan.run)?;
            resume_plan(run, &plan.worker, &plan.marker, now_ms)
                .err()
                .map(not_resumed)
        })
        .unwrap_or_else(|| "the worker, its seat or its attempt changed — press again".to_string())
}

/// 「지금 다시 시도」: type one continuation at a worker standing at its wall.
///
/// Answers the receipt row's id and the line that was typed. `Err` is the
/// reason, in words the window shows beside the button: a refusal before the
/// reservation leaves no row, and a failure after it leaves the row settled
/// `not_submitted` with whether anything was typed — the receipt the next
/// press reads, so it is never told a story.
pub(crate) fn retry(
    host: &dyn Host,
    run_id: &str,
    worker_id: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    let (chip, term) = standing_chip(run_id, worker_id, now_ms)?;
    if let Some(why) = chip.retry_refusal {
        return Err(why);
    }
    // The composer has to be at rest by the pointer's own reading: never a
    // pane at a question of its own, never one a person's hand interrupted.
    let heard = pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .get(&term)
        .copied()
        .map(PaneTurn::as_read);
    composer_at_rest(host, term, heard)
        .map_err(|why| format!("worker {worker_id}'s composer is not at rest {why}"))?;
    let held = runtime().ok_or(NO_RUNTIME)?;
    let plan = {
        let image = held.actor.view().map_err(|error| format!("{error:?}"))?;
        let rows = cached_ledger(&held, &image).map_err(|error| format!("{error:?}"))?;
        let walled = rows
            .run(run_id)
            .ok_or_else(|| format!("unknown run: {run_id}"))?;
        let marker = person_wall_marker(walled, worker_id).ok_or_else(|| {
            format!("worker {worker_id} stands at no quota wall the ledger has witnessed")
        })?;
        resume_plan(walled, worker_id, &marker, now_ms).map_err(not_resumed)?
    };
    crate::account_switch::unless_switching(|| {
        let ledger_run = plan.run.clone();
        // Reserved BEFORE the words go to the pump: a window that dies between
        // the two leaves a row that says so, and no later press types at the
        // same wall while it stands.
        let (Some(receipt), _) = held
            .actor
            .resume_begin(plan.clone(), now_ms)
            .map_err(|error| format!("{error:?}"))?
        else {
            return Err(reservation_refused(&held, &plan, now_ms));
        };
        let delivery = host.point(term, WALL_RETRY_LINE, true);
        let not_addressed = delivery.is_none();
        resumes_in_flight()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .insert(
                receipt.clone(),
                Resuming::new(ledger_run, term, delivery, NOT_ADDRESSED),
            );
        if not_addressed {
            settle_resumes(now_ms);
            return Err(NOT_ADDRESSED.to_string());
        }
        Ok(serde_json::json!({ "receipt": receipt, "line": WALL_RETRY_LINE }))
    })
    .unwrap_or_else(|| Err(SWITCH_IN_FLIGHT.to_string()))
}

/// The attempt a walk or an earlier press already replaced `dispatch_id`
/// with, by the link the replacement carries: `(worker, dispatch)`.
fn replacement_of(run_id: &str, dispatch_id: &str) -> Option<(String, String)> {
    with_ledger_seats(|ledger, _| {
        ledger.run(run_id).and_then(|run| {
            run.dispatches
                .iter()
                .find(|one| one.retry_of.as_deref() == Some(dispatch_id))
                .map(|one| (one.worker.clone(), one.id.clone()))
        })
    })
    .flatten()
}

/// 「다른 에이전트로 넘기기」: end the walled attempt and start `to` on its task
/// in the same checkout.
///
/// `to` is `agent[:model[:effort]]` as `--on-quota-wall` spells it; left out,
/// it is the alternative a standing order names, and a worker with neither is
/// refused by name — the press never picks an agent for the person. The
/// ledger still judges the alternative the way it judges any summons: one
/// that is itself walled is refused with its figure.
pub(crate) fn hand_over(
    host: &dyn Host,
    overrides: &[(String, LaunchOverride)],
    run_id: &str,
    worker_id: &str,
    to: Option<&str>,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    let (chip, term) = standing_chip(run_id, worker_id, now_ms)?;
    if let Some(why) = chip.hand_over_refusal.clone() {
        return Err(why);
    }
    let to = match to.map(str::trim).filter(|word| !word.is_empty()) {
        Some(word) => parse_on_quota_wall(word)?,
        None => chip.declared_alt.clone().ok_or_else(|| {
            "no alternative is named for this worker — choose the agent its task goes to"
                .to_string()
        })?,
    };
    let held = runtime().ok_or(NO_RUNTIME)?;
    let (plan, capability) = {
        let image = held.actor.view().map_err(|error| format!("{error:?}"))?;
        let rows = cached_ledger(&held, &image).map_err(|error| format!("{error:?}"))?;
        let walled = rows
            .run(run_id)
            .ok_or_else(|| format!("unknown run: {run_id}"))?;
        let worker = walled
            .worker(worker_id)
            .ok_or_else(|| format!("unknown worker: {worker_id}"))?;
        let dispatch = worker
            .dispatch
            .as_deref()
            .and_then(|id| walled.dispatch(id))
            .ok_or_else(|| format!("worker {worker_id} carries no open attempt"))?;
        let wall = newest_wall(walled, &dispatch.id).ok_or_else(|| {
            format!("worker {worker_id} stands at no quota wall the ledger has witnessed")
        })?;
        let task = walled
            .task(&dispatch.task)
            .ok_or_else(|| format!("unknown task: {}", dispatch.task))?;
        let (_, team, pane, capability) = presentable_seat(walled)
            .ok_or_else(|| format!("this window does not hold run {run_id}'s coordinator seat"))?;
        let generation = walled
            .coordinator_live()
            .map(|seat| seat.generation)
            .ok_or_else(|| format!("run {run_id} has no live coordinator seat"))?;
        let cause = wall_cause(walled, &wall)
            .ok_or_else(|| format!("the row of wall {} is gone from the ledger", wall.wall))?;
        let checkout = worker
            .checkout
            .clone()
            .ok_or_else(|| format!("worker {worker_id} reported no checkout"))?;
        // A worker that went back to work since the wall — its reset came and
        // it carried on by itself — is not the worker this press was about.
        if host.quiet_since(term, worker.started_ms, now_ms).is_none() {
            return Err(format!(
                "worker {worker_id} is at work again — it is not handed over"
            ));
        }
        (
            HandoverPlan {
                run: walled.id.clone(),
                worker: worker.id.clone(),
                agent: worker.agent.clone(),
                dispatch: dispatch.id.clone(),
                task: task.id.clone(),
                spec: task.spec.clone(),
                checkout,
                cause,
                to,
                wip_commit: false,
                team,
                pane,
                worker_team: worker.team.clone(),
                worker_pane: worker.pane.clone(),
                worker_started_ms: worker.started_ms,
                policy: walled.handover.clone(),
                coordinator_generation: generation,
            },
            capability,
        )
    };
    // Where it left off, read while its pane still stands: the stop closes the
    // pane and, with it, the window's record of which transcript was its own.
    let recap = LiveDoor { host, overrides }
        .predecessor_tail(&plan)
        .and_then(|(transcript, tail)| handover_recap(&transcript, &tail));
    let brief = format!(
        "{}{}",
        handover_paragraph(&plan, None, recap.as_ref(), now_ms),
        plan.spec.as_str()
    );
    crate::account_switch::unless_switching(|| {
        let stopped = run(
            host,
            overrides.to_vec(),
            &plan.team,
            &plan.pane,
            &capability,
            &handover_stop_argv(&plan.worker, plan.cause.reason(), &plan.dispatch),
            now_ms,
        );
        if stopped.exit_code != 0 {
            return Err(first_line(&stopped.stderr));
        }
        let started = run(
            host,
            overrides.to_vec(),
            &plan.team,
            &plan.pane,
            &capability,
            &handover_start_argv(&plan.to, &plan.task, &plan.dispatch, brief),
            now_ms,
        );
        if started.exit_code != 0 {
            // A walk that met this press on the same attempt started the one
            // replacement; the press says whose it is rather than a failure.
            if let Some((replacement, dispatch)) = replacement_of(&plan.run, &plan.dispatch) {
                return Ok(serde_json::json!({
                    "ended": plan.dispatch,
                    "replacement": replacement,
                    "dispatch": dispatch,
                    "already": true,
                }));
            }
            // The attempt is ended and the task is ready again: the ledger
            // says so, and the person is told in the same breath, so the next
            // move is not made on a belief that nothing moved.
            return Err(format!(
                "the attempt was ended but the replacement did not start: {}",
                first_line(&started.stderr)
            ));
        }
        let said: serde_json::Value = serde_json::from_str(&started.stdout).unwrap_or_default();
        Ok(serde_json::json!({
            "ended": plan.dispatch,
            "replacement": said["workerId"],
            "dispatch": said["dispatchId"],
            "agent": plan.to.agent,
        }))
    })
    .unwrap_or_else(|| Err(SWITCH_IN_FLIGHT.to_string()))
}
