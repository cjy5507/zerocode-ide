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
//! person took, an attempt that closed — all refuse here, by the ledger's own
//! code ([`PressRefusal`]), whatever the screen still shows.
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
    HandoverPlan, NotResumed, QuotaWallChip, RESUME_LINE, ResumePlan, WALL_RETRY_LINE, WallRefusal,
    handover_paragraph, handover_recap, newest_wall, parse_on_quota_wall, person_turn_marker_at,
    person_wall_marker, resume_plan, wall_cause, wall_chip_for,
};

/// Said when an account switch holds its line: the press is not lost, only
/// not made inside a move.
const SWITCH_IN_FLIGHT: &str =
    "an account switch is moving panes right now — ask again in a moment";

/// Said when the pointer's door could not address the pane: nothing was typed.
const NOT_ADDRESSED: &str = "the pane could not be addressed; nothing was typed";

/// Why a press was not made (t-47935): the ledger's reason or the window's
/// own, each a code with the facts its sentence names. The window's two
/// commands answer with it, and the renderer says the code in the person's
/// language — it is never handed a sentence to show or to read. `Display` is
/// the English sentence, for a log.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub(crate) enum PressRefusal {
    Ledger(WallRefusal),
    Window(WindowRefusal),
}

/// The window's own reasons a press is not made: its runtime, the pane's
/// composer, the account switch's line, the pointer's door, and what the
/// handover's two verbs answered.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub(crate) enum WindowRefusal {
    /// Orchestration never started in this window.
    NoRuntime,
    /// The ledger's image or rows could not be read: what the actor said.
    LedgerUnread {
        detail: String,
    },
    /// The four doors of [`composer_at_rest`], one code each.
    ComposerNeverHeard {
        worker: String,
    },
    ComposerHeldByPerson {
        worker: String,
    },
    ComposerShellInFront {
        worker: String,
    },
    ComposerTurnRunning {
        worker: String,
    },
    /// An account switch holds its line
    /// ([`crate::account_switch::unless_switching`]).
    SwitchInFlight,
    /// The reservation was refused and the rules, read again, name no reason:
    /// the worker, its seat or its attempt moved between the two readings.
    AttemptChanged,
    /// The pointer's door could not address the pane: nothing was typed.
    NotAddressed,
    /// `to` is not `agent[:model[:effort]]` the ledger reads.
    TargetUnreadable {
        to: String,
    },
    /// No agent was chosen, and no standing order names one.
    NoAlternative,
    /// The worker went back to work since its wall.
    AtWorkAgain {
        worker: String,
    },
    /// `worker-stop` was refused: its first line.
    StopRefused {
        detail: String,
    },
    /// The attempt was ended and `worker-start` was refused, so the task is
    /// ready again: the start's first line.
    ReplacementNotStarted {
        dispatch: String,
        detail: String,
    },
    /// The window's own plumbing answered in words — the main-window check,
    /// the press's thread — never the ledger's rules: what it said.
    PressFailed {
        detail: String,
    },
}

impl WindowRefusal {
    /// The composer's door that refused `worker`'s continuation.
    fn composer(worker: &str, busy: ComposerBusy) -> Self {
        let worker = worker.to_string();
        match busy {
            ComposerBusy::NeverHeard => Self::ComposerNeverHeard { worker },
            ComposerBusy::HeldByPerson => Self::ComposerHeldByPerson { worker },
            ComposerBusy::ShellInFront => Self::ComposerShellInFront { worker },
            ComposerBusy::TurnRunning => Self::ComposerTurnRunning { worker },
        }
    }
}

impl std::fmt::Display for WindowRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let composer = |f: &mut std::fmt::Formatter<'_>, worker: &str, busy: ComposerBusy| {
            write!(
                f,
                "worker {worker}'s composer is not at rest {}",
                busy.sentence()
            )
        };
        match self {
            Self::NoRuntime => f.write_str(NO_RUNTIME),
            Self::LedgerUnread { detail } => write!(f, "the ledger could not be read: {detail}"),
            Self::ComposerNeverHeard { worker } => composer(f, worker, ComposerBusy::NeverHeard),
            Self::ComposerHeldByPerson { worker } => {
                composer(f, worker, ComposerBusy::HeldByPerson)
            }
            Self::ComposerShellInFront { worker } => {
                composer(f, worker, ComposerBusy::ShellInFront)
            }
            Self::ComposerTurnRunning { worker } => composer(f, worker, ComposerBusy::TurnRunning),
            Self::SwitchInFlight => f.write_str(SWITCH_IN_FLIGHT),
            Self::AttemptChanged => {
                f.write_str("the worker, its seat or its attempt changed — press again")
            }
            Self::NotAddressed => f.write_str(NOT_ADDRESSED),
            Self::TargetUnreadable { to } => {
                write!(f, "the agent to hand over to could not be read: {to}")
            }
            Self::NoAlternative => f.write_str(
                "no alternative is named for this worker — choose the agent its task goes to",
            ),
            Self::AtWorkAgain { worker } => {
                write!(
                    f,
                    "worker {worker} is at work again — it is not handed over"
                )
            }
            Self::StopRefused { detail } => write!(f, "the worker was not stopped: {detail}"),
            Self::ReplacementNotStarted { detail, .. } => write!(
                f,
                "the attempt was ended but the replacement did not start: {detail}"
            ),
            Self::PressFailed { detail } => write!(f, "the press did not go through: {detail}"),
        }
    }
}

impl std::fmt::Display for PressRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ledger(why) => why.fmt(f),
            Self::Window(why) => why.fmt(f),
        }
    }
}

impl From<WallRefusal> for PressRefusal {
    fn from(why: WallRefusal) -> Self {
        Self::Ledger(why)
    }
}

impl From<WindowRefusal> for PressRefusal {
    fn from(why: WindowRefusal) -> Self {
        Self::Window(why)
    }
}

impl From<String> for PressRefusal {
    fn from(detail: String) -> Self {
        Self::Window(WindowRefusal::PressFailed { detail })
    }
}

/// The ledger's refusal of a continuation, by its code.
fn not_resumed(why: NotResumed) -> PressRefusal {
    match why {
        NotResumed::InFlight => WallRefusal::ResumeInFlight.into(),
        NotResumed::Refused(why) => why.into(),
    }
}

/// The ledger could not be read, as the actor said it.
fn unread(error: impl std::fmt::Debug) -> PressRefusal {
    WindowRefusal::LedgerUnread {
        detail: format!("{error:?}"),
    }
    .into()
}

/// The chip `worker_id` wears NOW and the terminal it sits in — read the way
/// the board's list reads it ([`ledger_agents_for_seats`]): the same rows, the
/// same seats, the same function.
fn standing_chip(
    run_id: &str,
    worker_id: &str,
    now_ms: i64,
) -> Result<(QuotaWallChip, u32), PressRefusal> {
    with_ledger_seats(
        |ledger, seats| -> Result<(QuotaWallChip, u32), PressRefusal> {
            let run = ledger.run(run_id).ok_or_else(|| WallRefusal::UnknownRun {
                run: run_id.to_string(),
            })?;
            let worker = run
                .worker(worker_id)
                .ok_or_else(|| WallRefusal::UnknownWorker {
                    worker: worker_id.to_string(),
                })?;
            let term = seated_term(run, worker, seats);
            let chip = wall_chip_for(run, worker, wall_chip_seat(run, term, seats), now_ms)?;
            let term = term.ok_or_else(|| WallRefusal::PaneNotHere {
                worker: worker_id.to_string(),
            })?;
            Ok((chip, term))
        },
    )
    .unwrap_or_else(|| Err(WindowRefusal::NoRuntime.into()))
}

/// Why the ledger would not reserve a continuation it was just asked for: its
/// own rule read again on the rows as they are now — or, the rows agreeing,
/// that the worker, its seat or its attempt moved between the two readings.
fn reservation_refused(held: &LiveRuntime, plan: &ResumePlan, now_ms: i64) -> PressRefusal {
    held.actor
        .view()
        .ok()
        .and_then(|image| {
            let rows = cached_ledger(held, &image);
            let run = rows.run(&plan.run)?;
            resume_plan(run, &plan.worker, &plan.marker, now_ms)
                .err()
                .map(not_resumed)
        })
        .unwrap_or_else(|| WindowRefusal::AttemptChanged.into())
}

/// 「지금 다시 시도」: type one continuation at a worker standing at its wall.
///
/// Answers the receipt row's id and the line that was typed. `Err` is the
/// reason, by its code, that the window says beside the button: a refusal
/// before the reservation leaves no row, and a failure after it leaves the
/// row settled `not_submitted` with whether anything was typed — the receipt
/// the next press reads, so it is never told a story.
pub(crate) fn retry(
    host: &dyn Host,
    run_id: &str,
    worker_id: &str,
    now_ms: i64,
) -> Result<serde_json::Value, PressRefusal> {
    let (chip, term) = standing_chip(run_id, worker_id, now_ms)?;
    if let Some(why) = chip.retry_refusal {
        return Err(why.into());
    }
    // The composer has to be at rest by the pointer's own reading: never a
    // pane at a question of its own, never one a person's hand interrupted.
    let heard = pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .get(&term)
        .copied()
        .map(PaneTurn::as_read);
    composer_at_rest(host, term, heard).map_err(|busy| WindowRefusal::composer(worker_id, busy))?;
    let held = runtime().ok_or(WindowRefusal::NoRuntime)?;
    let plan = {
        let image = held.actor.view().map_err(unread)?;
        let rows = cached_ledger(&held, &image);
        let walled = rows.run(run_id).ok_or_else(|| WallRefusal::UnknownRun {
            run: run_id.to_string(),
        })?;
        let marker = person_wall_marker(walled, worker_id).ok_or_else(|| WallRefusal::NoWall {
            worker: worker_id.to_string(),
        })?;
        resume_plan(walled, worker_id, &marker, now_ms).map_err(not_resumed)?
    };
    crate::account_switch::unless_switching(|| -> Result<serde_json::Value, PressRefusal> {
        let ledger_run = plan.run.clone();
        // Reserved BEFORE the words go to the pump: a window that dies between
        // the two leaves a row that says so, and no later press types at the
        // same wall while it stands.
        let (Some(receipt), _) = held
            .actor
            .resume_begin(plan.clone(), now_ms)
            .map_err(unread)?
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
            return Err(WindowRefusal::NotAddressed.into());
        }
        Ok(serde_json::json!({ "receipt": receipt, "line": WALL_RETRY_LINE }))
    })
    .unwrap_or_else(|| Err(WindowRefusal::SwitchInFlight.into()))
}

/// 「다시 띄우기」 (t-69010): type one continuation at a worker whose turn
/// failed, on the road 「지금 다시 시도」 takes — the composer at rest by the
/// pointer's own reading, [`resume_plan`] on the press's marker
/// ([`person_turn_marker_at`]: the attempt and the failure's letter the person
/// was shown, which the plan holds against the failure that stands now — the
/// review R2, P2-10), the receipt reserved before a word is typed, the words
/// through the pointer's one guarded door. Answers the receipt and the line;
/// `Err` is the reason, by its code.
pub(crate) fn retry_failed_turn(
    host: &dyn Host,
    run_id: &str,
    worker_id: &str,
    (dispatch, letter): (&str, &str),
    now_ms: i64,
) -> Result<serde_json::Value, PressRefusal> {
    let term = with_ledger_seats(|ledger, seats| -> Result<u32, PressRefusal> {
        let run = ledger.run(run_id).ok_or_else(|| WallRefusal::UnknownRun {
            run: run_id.to_string(),
        })?;
        let worker = run
            .worker(worker_id)
            .ok_or_else(|| WallRefusal::UnknownWorker {
                worker: worker_id.to_string(),
            })?;
        Ok(
            seated_term(run, worker, seats).ok_or_else(|| WallRefusal::PaneNotHere {
                worker: worker_id.to_string(),
            })?,
        )
    })
    .unwrap_or_else(|| Err(WindowRefusal::NoRuntime.into()))?;
    let heard = pane_turns()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .get(&term)
        .copied()
        .map(PaneTurn::as_read);
    composer_at_rest(host, term, heard).map_err(|busy| WindowRefusal::composer(worker_id, busy))?;
    let held = runtime().ok_or(WindowRefusal::NoRuntime)?;
    let plan = {
        let image = held.actor.view().map_err(unread)?;
        let rows = cached_ledger(&held, &image);
        let failed = rows.run(run_id).ok_or_else(|| WallRefusal::UnknownRun {
            run: run_id.to_string(),
        })?;
        let marker = person_turn_marker_at(dispatch, letter);
        resume_plan(failed, worker_id, &marker, now_ms).map_err(not_resumed)?
    };
    crate::account_switch::unless_switching(|| -> Result<serde_json::Value, PressRefusal> {
        let ledger_run = plan.run.clone();
        let (Some(receipt), _) = held
            .actor
            .resume_begin(plan.clone(), now_ms)
            .map_err(unread)?
        else {
            return Err(reservation_refused(&held, &plan, now_ms));
        };
        let delivery = host.point(term, RESUME_LINE, true);
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
            return Err(WindowRefusal::NotAddressed.into());
        }
        Ok(serde_json::json!({ "receipt": receipt, "line": RESUME_LINE }))
    })
    .unwrap_or_else(|| Err(WindowRefusal::SwitchInFlight.into()))
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
) -> Result<serde_json::Value, PressRefusal> {
    let (chip, term) = standing_chip(run_id, worker_id, now_ms)?;
    if let Some(why) = chip.hand_over_refusal.clone() {
        return Err(why.into());
    }
    let to = match to.map(str::trim).filter(|word| !word.is_empty()) {
        Some(word) => parse_on_quota_wall(word).map_err(|_| WindowRefusal::TargetUnreadable {
            to: word.to_string(),
        })?,
        None => chip
            .declared_alt
            .clone()
            .ok_or(WindowRefusal::NoAlternative)?,
    };
    let held = runtime().ok_or(WindowRefusal::NoRuntime)?;
    let (plan, capability) = {
        let image = held.actor.view().map_err(unread)?;
        let rows = cached_ledger(&held, &image);
        let walled = rows.run(run_id).ok_or_else(|| WallRefusal::UnknownRun {
            run: run_id.to_string(),
        })?;
        let worker = walled
            .worker(worker_id)
            .ok_or_else(|| WallRefusal::UnknownWorker {
                worker: worker_id.to_string(),
            })?;
        let dispatch = worker
            .dispatch
            .as_deref()
            .and_then(|id| walled.dispatch(id))
            .ok_or_else(|| WallRefusal::NoOpenAttempt {
                worker: worker_id.to_string(),
            })?;
        let wall = newest_wall(walled, &dispatch.id).ok_or_else(|| WallRefusal::NoWall {
            worker: worker_id.to_string(),
        })?;
        let task = walled
            .task(&dispatch.task)
            .ok_or_else(|| WallRefusal::UnknownTask {
                task: dispatch.task.clone(),
            })?;
        let (_, team, pane, capability) =
            presentable_seat(walled).ok_or_else(|| WallRefusal::CoordinatorSeatElsewhere {
                run: run_id.to_string(),
            })?;
        let generation = walled
            .coordinator_live()
            .map(|seat| seat.generation)
            .ok_or_else(|| WallRefusal::NoLiveCoordinator {
                run: run_id.to_string(),
            })?;
        let cause = wall_cause(walled, &wall).ok_or_else(|| WallRefusal::WallRowGone {
            wall: wall.wall.clone(),
        })?;
        let checkout = worker
            .checkout
            .clone()
            .ok_or_else(|| WallRefusal::NoCheckout {
                worker: worker_id.to_string(),
            })?;
        // A worker that went back to work since the wall — its reset came and
        // it carried on by itself — is not the worker this press was about.
        if host.quiet_since(term, worker.started_ms, now_ms).is_none() {
            return Err(WindowRefusal::AtWorkAgain {
                worker: worker_id.to_string(),
            }
            .into());
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
    crate::account_switch::unless_switching(|| -> Result<serde_json::Value, PressRefusal> {
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
            return Err(WindowRefusal::StopRefused {
                detail: first_line(&stopped.stderr),
            }
            .into());
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
            return Err(WindowRefusal::ReplacementNotStarted {
                dispatch: plan.dispatch.clone(),
                detail: first_line(&started.stderr),
            }
            .into());
        }
        let said: serde_json::Value = serde_json::from_str(&started.stdout).unwrap_or_default();
        Ok(serde_json::json!({
            "ended": plan.dispatch,
            "replacement": said["workerId"],
            "dispatch": said["dispatchId"],
            "agent": plan.to.agent,
        }))
    })
    .unwrap_or_else(|| Err(WindowRefusal::SwitchInFlight.into()))
}
