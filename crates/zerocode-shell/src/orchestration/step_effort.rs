//! A worker's effort, moved between two of its turns (t-5637): the beat's
//! step-effort seat — `zerocode_core::jev::STEP_EFFORT`, `smart.stepEffort`
//! — which reads the turn a worker just ended off its own transcript, asks
//! Jev which way the effort should go, and, when the seat acts, moves it
//! through the door the agent's row names (`capabilities::TurnMoves`) before
//! the mail pointer can start the next turn.
//!
//! The rule and the question are pure (`zerocode_core::step_effort`); this
//! module is the beat's half: which panes are asked about, when a move may
//! be typed, and what each move came to. Three things it never does:
//!
//! - **type at a person.** Only a live worker's pane carrying an open
//!   attempt is read, never one a person has taken over (`taken_over`), and
//!   nothing is typed unless the window measured the pane's last turn ending
//!   at rest with an agent still in front ([`super::composer_at_rest`]) — the
//!   same licence the mail pointer needs.
//! - **decide by an agent's name.** The door is the row's `moves.effort`: a
//!   picker it drives one rung a time for this session only (Claude Code), a
//!   relaunch it can only record (Codex), or nothing measured.
//! - **act on a recording seat.** Under `shadow`, and under `auto` until its
//!   own evidence raises it, the row is written and the composer untouched.
//!
//! A question waits up to [`STEP_EFFORT_DEADLINE`] off the beat; the beat
//! reads its answer on a later beat, and the rule's own move stands in for
//! one that never came. The picker's keys go only after the screen shows
//! its legend, and only for [`PICKER_WAIT_MS`]; a picker that never appears
//! is a row that says so, not a key sent blind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::capabilities::{MoveRoad, TurnMoves, agent_capabilities};
use zerocode_core::jev::door::{REDACTED_LINES_KEY, REQUESTS_KEY};
use zerocode_core::jev::{JevMode, STEP_EFFORT};
use zerocode_core::orchestration::Ledger;
use zerocode_core::step_effort::{
    self, Followed, Move, STEP_EFFORT_LABEL_WINDOW_MS, STEP_EFFORT_RUBRIC_VERSION, Standing,
    StepAsk, StepLook,
};
use zerocode_core::transcript::{effort_in, tail_lines, turns_in};
use zerocode_pty::DeliveryOutcome;

use super::{PaneTurn, composer_at_rest};
use crate::agent_teams::Host;
use crate::systemone::{SCHEMA, Wire, request_body};

/// How long one step-effort question may wait for its answer — the row's
/// own wall (`STEP_EFFORT_APPLY_DEADLINE_MS`).
pub(crate) const STEP_EFFORT_DEADLINE: Duration =
    Duration::from_millis(zerocode_core::jev::STEP_EFFORT_APPLY_DEADLINE_MS);

/// How long after the picker's command was delivered the beat keeps looking
/// for its legend on the screen before it gives the move up. A composer
/// drew Claude Code's picker within a beat on this machine; three seconds
/// is three beats of grace, not a wait anybody notices.
pub(crate) const PICKER_WAIT_MS: i64 = 3_000;

/// The row's outcome for a question Jev answered in shape.
const ANSWERED: &str = "answered";

/// The words a label row keeps for what the door did with the move.
mod door_outcome {
    /// The picker's keys went in, for this session only.
    pub(super) const KEYED: &str = "keyed";
    /// The line went in.
    pub(super) const TYPED: &str = "typed";
    /// The seat was recording, or the move was `hold`: nothing typed.
    pub(super) const RECORDED: &str = "recorded";
    /// The row's door is a relaunch: nothing to type at a running session.
    pub(super) const RELAUNCH: &str = "relaunch";
    /// The guarded door did not deliver the command.
    pub(super) const WITHHELD: &str = "withheld";
    /// The command was delivered and the picker never showed its legend.
    pub(super) const PICKER_MISSING: &str = "picker_missing";
    /// The next turn started before the composer was at rest for the move.
    pub(super) const TURN_STARTED: &str = "turn_started";
    /// The ladder does not know the word the move would land on.
    pub(super) const NO_RUNG: &str = "no_rung";
}

/// What this window remembers about the moves it has judged.
#[derive(Default)]
pub(super) struct MoveBook {
    /// One seat per open attempt, by dispatch.
    seats: HashMap<String, Seat>,
}

/// One attempt's standing with the seat.
#[derive(Default)]
struct Seat {
    /// How many prompts its transcript held when the seat last judged a
    /// turn of it — a turn is judged once, when it is first seen ended.
    prompts_seen: usize,
    /// Whether the seat has seen this attempt at all: the first sight only
    /// counts its prompts, so a window that starts beside an idle worker
    /// does not judge a turn that ended before it was watching.
    seen: bool,
    /// The effort word before a raise the seat applied, while that raise
    /// stands — what the next progress brings the worker back toward.
    raised: Option<String>,
    /// The move on its way, when one is.
    open: Option<Open>,
}

/// A move between the turn it read and the label it earns.
struct Open {
    /// The key the row and its label share: one attempt, one turn.
    key: String,
    run: String,
    worker: String,
    dispatch: String,
    agent: String,
    /// The move the rule made — the product's own answer.
    ruled: Move,
    /// The effort the turn ran at, and the rung the move would land on.
    from: Option<String>,
    to: Option<String>,
    /// How many prompts the transcript held when the turn was read: one
    /// more is the next turn, which the label reads.
    prompts_at: usize,
    stage: Stage,
}

/// Where an open move stands.
enum Stage {
    /// The question is on its way, off the beat.
    Asking,
    /// The move to carry out, and whose word it is.
    Decided { chosen: Move, source: &'static str },
    /// The command went to the guarded door; the receipt says when.
    Opened {
        chosen: Move,
        receipt: mpsc::Receiver<DeliveryOutcome>,
        since_ms: i64,
        /// The picker's command was delivered and the beat is waiting for
        /// its legend, since when.
        picker_since_ms: Option<i64>,
    },
    /// The move is done with the door — applied or not — and waits for the
    /// next turn to end so the label can say what it did.
    Settled {
        chosen: Move,
        applied: bool,
        door_outcome: &'static str,
        at_ms: i64,
    },
}

/// The shared answer cell an off-beat question writes into: the move the
/// seat decided and whose word it was.
type Decision = (Move, &'static str);

/// Read every live worker's last turn, move what the seat says, and label
/// what earlier moves came to — one beat.
pub(super) fn sweep(host: &dyn Host, now_ms: i64) {
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
    let Some(wire) = host.jev_wire() else {
        return;
    };
    let mode = STEP_EFFORT.mode_in(&wire.settings_root());
    if !mode.asks() {
        return;
    }
    let Some(ledger_path) = crate::systemone::ledger_of(&wire, &STEP_EFFORT) else {
        return;
    };
    let panes = worker_panes(&ledger, &seats);
    let live: std::collections::HashSet<&str> =
        panes.iter().map(|pane| pane.dispatch.as_str()).collect();
    {
        let mut book = held.moves.lock().unwrap_or_else(|held| held.into_inner());
        book.seats
            .retain(|dispatch, _| live.contains(dispatch.as_str()));
    }
    for pane in panes {
        if !host.pane_exists(pane.term) {
            continue;
        }
        let Some(caps) = agent_capabilities(&pane.agent) else {
            continue;
        };
        let Some(road) = caps.moves.effort else {
            continue;
        };
        let Some(lines) = host
            .provider_session(pane.term)
            .and_then(|session| session.transcript_path)
            .and_then(|path| tail_lines(Path::new(&path)))
        else {
            continue;
        };
        let raw = lines.join("\n");
        let turns = turns_in(&raw);
        let prompts = turns.iter().filter(|turn| turn.role == "user").count();
        let heard = super::pane_turns()
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .get(&pane.term)
            .copied();
        let at_rest = composer_at_rest(host, pane.term, heard).is_ok();
        let open = {
            let mut book = held.moves.lock().unwrap_or_else(|held| held.into_inner());
            let seat = book.seats.entry(pane.dispatch.clone()).or_default();
            if !seat.seen {
                seat.seen = true;
                // A turn under way when the seat first looks is the next one
                // to judge; a turn that had already ended is not — it ended
                // before anybody was watching.
                seat.prompts_seen = if matches!(heard, Some(PaneTurn::Running)) {
                    prompts.saturating_sub(1)
                } else {
                    prompts
                };
            }
            seat.open.take()
        };
        let look = Look {
            pane: &pane,
            road,
            moves: caps.moves,
            raw: &raw,
            turns: &turns,
            prompts,
            at_rest,
            heard,
        };
        match open {
            Some(open) => advance(host, &held, &wire, &ledger_path, &look, open, now_ms),
            None => judge(host, &held, &wire, &ledger_path, mode, &look, now_ms),
        }
    }
}

/// One live worker pane the seat may read: the attempt, the pane, the agent
/// and the words it was summoned with.
struct WorkerPane {
    run: String,
    worker: String,
    dispatch: String,
    task: String,
    agent: String,
    term: u32,
    /// The effort the summons carried — the floor a lower never goes under.
    floor: Option<String>,
    checkout: Option<String>,
}

/// Every live worker carrying an open attempt in a pane this window holds
/// and no person has taken over.
fn worker_panes(ledger: &Ledger, seats: &super::TeamSeatIndex) -> Vec<WorkerPane> {
    ledger
        .runs()
        .iter()
        .flat_map(|run| {
            run.workers.iter().filter_map(|worker| {
                if !worker.state.is_live() || !worker.state.may_occupy_pane() || worker.taken_over {
                    return None;
                }
                let dispatch = run.dispatch(worker.dispatch.as_deref()?)?;
                if !dispatch.is_open()
                    || run
                        .worker_in_pane(&worker.team, &worker.pane)
                        .is_none_or(|current| current.id != worker.id)
                {
                    return None;
                }
                let term = seats
                    .get(worker.team.as_str())?
                    .get(worker.pane.as_str())
                    .copied()?;
                Some(WorkerPane {
                    run: run.id.clone(),
                    worker: worker.id.clone(),
                    dispatch: dispatch.id.clone(),
                    task: dispatch.task.clone(),
                    agent: worker.agent.clone(),
                    term,
                    floor: worker.effort.clone(),
                    checkout: worker.checkout.clone(),
                })
            })
        })
        .collect()
}

/// What one beat knows about one pane, read once and handed around.
struct Look<'a> {
    pane: &'a WorkerPane,
    road: MoveRoad,
    moves: TurnMoves,
    /// The tail's records as read, for the witnesses that read raw lines.
    raw: &'a str,
    turns: &'a [zerocode_core::transcript::TranscriptTurn],
    prompts: usize,
    at_rest: bool,
    heard: Option<PaneTurn>,
}

/// Read the turn that just ended, when one has, and put the move to Jev.
fn judge(
    host: &dyn Host,
    held: &super::LiveRuntime,
    wire: &Wire,
    ledger_path: &Path,
    mode: JevMode,
    look: &Look<'_>,
    now_ms: i64,
) {
    let (prompts_seen, raised) = {
        let book = held.moves.lock().unwrap_or_else(|held| held.into_inner());
        let seat = book.seats.get(&look.pane.dispatch);
        (
            seat.map_or(0, |seat| seat.prompts_seen),
            seat.and_then(|seat| seat.raised.clone()),
        )
    };
    // A turn is read once it has ENDED: a new prompt in the record and the
    // pane measured at rest. `Running` is a turn under way; never-heard is
    // a pane nothing will measure, and neither is a turn to read.
    if look.prompts <= prompts_seen || !matches!(look.heard, Some(PaneTurn::Ended { .. })) {
        return;
    }
    {
        let mut book = held.moves.lock().unwrap_or_else(|held| held.into_inner());
        if let Some(seat) = book.seats.get_mut(&look.pane.dispatch) {
            seat.prompts_seen = look.prompts;
        }
    }
    let signals = step_effort::signals_of(look.turns);
    let current = effort_in(look.raw);
    let standing = Standing {
        current: current.as_deref(),
        floor: look.pane.floor.as_deref(),
        raised: raised.is_some(),
        stall_cause: super::stall_cause::answered_cause(&held.stalls, &look.pane.dispatch),
    };
    let ruled = step_effort::ruled(&signals, &standing, &look.moves);
    if ruled == Move::Hold {
        return;
    }
    let ask = step_effort::ask(&StepLook {
        agent: &look.pane.agent,
        signals: &signals,
        standing: &standing,
    });
    let stall_cause = standing.stall_cause;
    let to = match ruled {
        Move::Raise => current
            .as_deref()
            .and_then(|word| look.moves.rung_from(word, true)),
        Move::Lower => current
            .as_deref()
            .and_then(|word| look.moves.rung_from(word, false)),
        Move::Hold => None,
    }
    .map(str::to_string);
    let key = format!("{}@{}", look.pane.dispatch, look.prompts);
    let row = json!({
        "at": now_ms,
        "move": key,
        "run": look.pane.run,
        "worker": look.pane.worker,
        "dispatch": look.pane.dispatch,
        "task": look.pane.task,
        "agent": look.pane.agent,
        "mode": mode.key(),
        "rubricVersion": STEP_EFFORT_RUBRIC_VERSION,
        "door": look.road.door(),
        "turn": look.prompts,
        "from": current,
        "to": to,
        "floor": look.pane.floor,
        "raised": raised.is_some(),
        "ruled": ruled.word(),
        "signals": {
            "repeats": signals.repeats,
            "toolFailures": signals.tool_failures,
            "readOnly": signals.read_only,
            "stallCause": stall_cause.map(zerocode_core::stall_cause::Cause::word),
        },
        REQUESTS_KEY: 0,
        REDACTED_LINES_KEY: 0,
    });
    let open = Open {
        key,
        run: look.pane.run.clone(),
        worker: look.pane.worker.clone(),
        dispatch: look.pane.dispatch.clone(),
        agent: look.pane.agent.clone(),
        ruled,
        from: current,
        to,
        prompts_at: look.prompts,
        stage: Stage::Asking,
    };
    {
        let mut book = held.moves.lock().unwrap_or_else(|held| held.into_inner());
        if let Some(seat) = book.seats.get_mut(&look.pane.dispatch) {
            seat.open = Some(open);
        }
    }
    let wire = wire.clone();
    let ledger_path = ledger_path.to_path_buf();
    let book = Arc::clone(&held.moves);
    let dispatch = look.pane.dispatch.clone();
    let checkout = look.pane.checkout.clone().map(PathBuf::from);
    host.off_the_beat(Box::new(move || {
        let (row, decision) = settle(&wire, row, &ask, ruled, checkout.as_deref());
        crate::systemone::record_rows(&STEP_EFFORT, &ledger_path, &[row], now_ms);
        let mut held = book.lock().unwrap_or_else(|held| held.into_inner());
        if let Some(open) = held
            .seats
            .get_mut(&dispatch)
            .and_then(|seat| seat.open.as_mut())
            && matches!(open.stage, Stage::Asking)
        {
            open.stage = Stage::Decided {
                chosen: decision.0,
                source: decision.1,
            };
        }
    }));
}

/// Ask once and finish the row: what it cost, what came back, and the move
/// the seat decided — the answer's when it answered in shape, the rule's
/// otherwise.
fn settle(
    wire: &Wire,
    mut row: Value,
    ask: &StepAsk,
    ruled: Move,
    checkout: Option<&Path>,
) -> (Value, Decision) {
    let began = Instant::now();
    let answer = wire.ask(
        &STEP_EFFORT,
        checkout,
        request_body(&ask.state, &ask.questions),
        STEP_EFFORT_DEADLINE,
    );
    row["elapsedMs"] = json!(u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX));
    row["requestBytes"] = json!(answer.request_bytes);
    row[REQUESTS_KEY] = json!(answer.spent.requests);
    row[REDACTED_LINES_KEY] = json!(answer.spent.redacted_lines);
    let read = answer.answer.and_then(|body| {
        serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|parsed| ask.read(parsed.get("answers")?).ok())
            .ok_or_else(|| SCHEMA.to_string())
    });
    match read {
        Ok(choice) => {
            row["outcome"] = json!(ANSWERED);
            row["chosen"] = json!(choice.chosen.word());
            row["probabilities"] = json!(choice.probabilities);
            row["confidence"] = json!(choice.confidence);
            // The one number this ledger exists to produce beside the label:
            // how often the judgment and the rule read a turn the same way.
            row["agreedWithRule"] = json!(choice.chosen == ruled);
            (row, (choice.chosen, ANSWERED))
        }
        Err(token) => {
            row["outcome"] = json!(token);
            (row, (ruled, "ruled"))
        }
    }
}

/// Carry an open move one stage on: type it when the seat acts and the
/// composer rests, drive the picker once its legend is up, and label it once
/// the next turn has ended.
fn advance(
    host: &dyn Host,
    held: &super::LiveRuntime,
    wire: &Wire,
    ledger_path: &Path,
    look: &Look<'_>,
    mut open: Open,
    now_ms: i64,
) {
    let settled = |chosen: Move, applied: bool, door_outcome: &'static str| Stage::Settled {
        chosen,
        applied,
        door_outcome,
        at_ms: now_ms,
    };
    let next_turn_started = look.prompts > open.prompts_at;
    open.stage = match open.stage {
        Stage::Asking => Stage::Asking,
        Stage::Decided { chosen, source } => {
            // A hold has nothing to type, and a recording seat types nothing.
            if chosen == Move::Hold || !crate::systemone::applies(wire, &STEP_EFFORT) {
                settled(chosen, false, door_outcome::RECORDED)
            } else if !look.road.moves_between_turns() {
                settled(chosen, false, door_outcome::RELAUNCH)
            } else if next_turn_started {
                settled(chosen, false, door_outcome::TURN_STARTED)
            } else if !look.at_rest {
                Stage::Decided { chosen, source }
            } else {
                let to = match chosen {
                    Move::Raise => open
                        .from
                        .as_deref()
                        .and_then(|word| look.moves.rung_from(word, true)),
                    Move::Lower => open
                        .from
                        .as_deref()
                        .and_then(|word| look.moves.rung_from(word, false)),
                    Move::Hold => None,
                };
                open.to = to.map(str::to_string);
                let line = match look.road {
                    MoveRoad::Line(_) => to.and_then(|word| look.road.line(word)),
                    MoveRoad::Picker(keys) => Some(keys.open.to_string()),
                    MoveRoad::Shown(_) | MoveRoad::Relaunch => None,
                };
                match line {
                    None => settled(chosen, false, door_outcome::NO_RUNG),
                    Some(line) => match host.point(look.pane.term, &line, true) {
                        Some(receipt) => Stage::Opened {
                            chosen,
                            receipt,
                            since_ms: now_ms,
                            picker_since_ms: None,
                        },
                        None => settled(chosen, false, door_outcome::WITHHELD),
                    },
                }
            }
        }
        Stage::Opened {
            chosen,
            receipt,
            since_ms,
            picker_since_ms,
        } => match picker_since_ms {
            // The picker's command is in; its legend is what licenses the keys.
            Some(waiting_since) => match look.road.picker() {
                Some(keys)
                    if host
                        .capture(look.pane.term)
                        .is_some_and(|screen| screen.contains(keys.legend)) =>
                {
                    let step = match chosen {
                        Move::Raise => keys.raise,
                        Move::Lower => keys.lower,
                        Move::Hold => "",
                    };
                    if host.send(look.pane.term, step)
                        && host.send(look.pane.term, keys.session_only)
                    {
                        settled(chosen, true, door_outcome::KEYED)
                    } else {
                        settled(chosen, false, door_outcome::WITHHELD)
                    }
                }
                _ if now_ms.saturating_sub(waiting_since) > PICKER_WAIT_MS => {
                    settled(chosen, false, door_outcome::PICKER_MISSING)
                }
                _ => Stage::Opened {
                    chosen,
                    receipt,
                    since_ms,
                    picker_since_ms,
                },
            },
            None => match receipt.try_recv() {
                Ok(DeliveryOutcome::Delivered) => match look.road {
                    MoveRoad::Picker(_) => Stage::Opened {
                        chosen,
                        receipt,
                        since_ms,
                        picker_since_ms: Some(now_ms),
                    },
                    MoveRoad::Line(_) | MoveRoad::Shown(_) | MoveRoad::Relaunch => {
                        settled(chosen, true, door_outcome::TYPED)
                    }
                },
                Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                    settled(chosen, false, door_outcome::WITHHELD)
                }
                Err(mpsc::TryRecvError::Empty) => Stage::Opened {
                    chosen,
                    receipt,
                    since_ms,
                    picker_since_ms,
                },
            },
        },
        Stage::Settled {
            chosen,
            applied,
            door_outcome,
            at_ms,
        } => {
            let ended = next_turn_started && matches!(look.heard, Some(PaneTurn::Ended { .. }));
            let past = now_ms.saturating_sub(at_ms) > STEP_EFFORT_LABEL_WINDOW_MS;
            if !ended && !past {
                Stage::Settled {
                    chosen,
                    applied,
                    door_outcome,
                    at_ms,
                }
            } else {
                let followed = if ended {
                    Followed::of(&step_effort::signals_of(look.turns))
                } else {
                    Followed::Nothing
                };
                let landed = ended.then(|| effort_in(look.raw)).flatten();
                let mut label = json!({
                    "at": now_ms,
                    "label": open.key,
                    "run": open.run,
                    "worker": open.worker,
                    "dispatch": open.dispatch,
                    "agent": open.agent,
                    "ruled": open.ruled.word(),
                    "chosen": chosen.word(),
                    "door": look.road.door(),
                    "doorOutcome": door_outcome,
                    "applied": applied,
                    "from": open.from,
                    "to": open.to,
                    "landed": landed,
                    "followed": followed.word(),
                    "afterMs": now_ms.saturating_sub(at_ms),
                });
                // The mark the judge counts: a move that bet on progress was
                // right when the next turn progressed. A move nobody applied
                // is graded the same way — the label says what the turn did
                // at the effort it kept, which is what a `shadow` row is for.
                if let (Some(expected), true) = (step_effort::expected_followed(chosen), ended) {
                    label[zerocode_core::jev::summary::AGREED.canonical] =
                        json!(expected == followed);
                }
                crate::systemone::record_rows(&STEP_EFFORT, ledger_path, &[label], now_ms);
                let mut book = held.moves.lock().unwrap_or_else(|held| held.into_inner());
                if let Some(seat) = book.seats.get_mut(&open.dispatch)
                    && applied
                {
                    match chosen {
                        Move::Raise => seat.raised = open.from.clone(),
                        Move::Lower => seat.raised = None,
                        Move::Hold => {}
                    }
                }
                return;
            }
        }
    };
    let mut book = held.moves.lock().unwrap_or_else(|held| held.into_inner());
    if let Some(seat) = book.seats.get_mut(&open.dispatch) {
        seat.open = Some(open);
    }
}
