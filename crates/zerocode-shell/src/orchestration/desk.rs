//! The coordinator's desk on the task board (t-6588,
//! docs/design/agent-board-round4.md).
//!
//! What a coordinator typed `worker-list`, `check --peek`, `task-list`,
//! `df -g`, `uptime` and `xcrun simctl list` for all day, answered from what
//! this window already holds: the ledger reading the board's beat publishes
//! ([`super::refresh_board_ledger`]) and the machine that ledger lives on.
//! Nothing here reads the ledger a second way, and nothing here judges a fact
//! the ledger already judges — the disk's word is the rule a `--worktree`
//! summons is refused by ([`zerocode_core::orchestration::worktree_room`]).

use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;
use zerocode_core::orchestration::{
    Ledger, Message, MessageKind, Run, Task, TaskStatus, WorktreeRoom, worktree_room,
};

/// The desk's reading of the ledger, published beside the board's other
/// readings on the standing-order beat ([`super::refresh_board_ledger`]) and
/// read through `board_desk` as an `Arc` — no actor request, no rebuild.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct DeskSnapshot {
    /// The runs in play: a live coordinator seat, or a worker still summoned.
    pub(crate) runs: Vec<DeskRun>,
    /// Their tasks, by stage, at most [`STAGE_ROWS`] a stage.
    pub(crate) tasks: Vec<DeskTask>,
    /// Every task of those runs, counted by stage — the rows above are a
    /// window onto these, never the other way round.
    pub(crate) stages: Vec<StageCount>,
    /// The letters their coordinators owe, oldest first ([`desk_mail`]).
    pub(crate) mail: Vec<DeskMail>,
}

/// One letter a run's coordinator owes: an answer, or an acknowledgement.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DeskMail {
    pub(crate) run: String,
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) from: String,
    /// The worker the letter concerns, where it names one.
    pub(crate) worker: Option<String>,
    pub(crate) task_id: Option<String>,
    pub(crate) task: Option<String>,
    /// A question's own words. The ledger's notices carry their facts
    /// below instead — their bodies are the ledger's JSON, not prose.
    pub(crate) body: String,
    /// Why a worker went quiet, in the ledger's word (`stalled`,
    /// `quota_lifted`, `pane_missing`, `never_spoke`, …).
    pub(crate) reason: Option<String>,
    /// When the provider said a quota wall resets.
    pub(crate) resets_at_ms: Option<i64>,
    pub(crate) created_ms: i64,
    /// Where the letter stands in the coordinator's inbox: `pending` (not
    /// yet handed over), `delivered` (in the batch the coordinator holds,
    /// unacknowledged) or `acked`.
    pub(crate) delivery: &'static str,
    /// The batch it is in, while it is `delivered` — what an
    /// acknowledgement names.
    pub(crate) delivery_id: Option<String>,
    /// How many letters that batch holds: the ledger acknowledges a batch,
    /// never one letter of it.
    pub(crate) batch: Option<usize>,
}

/// The letters a coordinator owes something, one table: a question put to it
/// (owed an answer until one lands, however it was delivered), and the
/// ledger's news that a worker stopped — at its quota wall, dead, quiet, or
/// waiting in a ring (owed an acknowledgement until the batch holding it is
/// acknowledged).
const DESK_MAIL_KINDS: [MessageKind; 5] = [
    MessageKind::Question,
    MessageKind::QuotaWalled,
    MessageKind::WorkerDied,
    MessageKind::WentQuiet,
    MessageKind::Deadlocked,
];

/// Where one letter stands in the inbox that holds it.
fn delivery_of(id: &str, pending: &HashSet<&str>, open: &HashSet<&str>) -> &'static str {
    if pending.contains(id) {
        "pending"
    } else if open.contains(id) {
        "delivered"
    } else {
        "acked"
    }
}

/// What the coordinator of `run` owes, oldest first. A question is owed until
/// it is answered or can no longer be (`Run::answer_to`,
/// `Run::question_is_closed` — the reply verb's own rules); a notice until it
/// is acknowledged.
pub(crate) fn desk_mail(run: &Run) -> Vec<DeskMail> {
    let address = run.address();
    let pending: HashSet<&str> = run
        .pending_messages(&address, &DESK_MAIL_KINDS)
        .into_iter()
        .map(|message| message.id.as_str())
        .collect();
    let batch = run.open_delivery(&address);
    let open: HashSet<&str> = batch
        .map(|held| held.messages.iter().map(String::as_str).collect())
        .unwrap_or_default();
    run.messages()
        .iter()
        .filter(|message| message.to == address && DESK_MAIL_KINDS.contains(&message.kind))
        .filter_map(|message| {
            let delivery = delivery_of(&message.id, &pending, &open);
            let owed = match message.kind {
                MessageKind::Question => {
                    message.thread.is_none()
                        && run.answer_to(message).is_none()
                        && !run.question_is_closed(message)
                }
                _ => delivery != "acked",
            };
            owed.then(|| mail_row(run, message, delivery, batch))
        })
        .collect()
}

fn mail_row(
    run: &Run,
    message: &Message,
    delivery: &'static str,
    batch: Option<&zerocode_core::orchestration::Delivery>,
) -> DeskMail {
    let question = message.kind == MessageKind::Question;
    let said: serde_json::Value = if question {
        serde_json::Value::Null
    } else {
        serde_json::from_str(message.body.as_str()).unwrap_or_default()
    };
    let worker = message
        .from
        .strip_prefix(zerocode_core::orchestration::WORKER_ADDRESS_PREFIX)
        .map(str::to_string)
        .or_else(|| said["workerId"].as_str().map(str::to_string));
    let task_id = message
        .task
        .clone()
        .or_else(|| said["taskId"].as_str().map(str::to_string));
    let delivered = delivery == "delivered";
    DeskMail {
        run: run.id.clone(),
        id: message.id.clone(),
        kind: message.kind.as_str(),
        from: message.from.clone(),
        task: task_id
            .as_deref()
            .and_then(|id| run.task(id))
            .map(|task| task.display_name().to_string()),
        worker,
        task_id,
        body: if question {
            message.body.as_str().to_string()
        } else {
            String::new()
        },
        reason: said["reason"].as_str().map(str::to_string),
        resets_at_ms: said["resetsAtMs"].as_i64(),
        created_ms: message.created_ms,
        delivery,
        delivery_id: batch.filter(|_| delivered).map(|held| held.id.clone()),
        batch: batch.filter(|_| delivered).map(|held| held.messages.len()),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DeskRun {
    pub(crate) run: String,
    pub(crate) name: String,
    /// Whether this window holds the run's live coordinator seat — the seat
    /// an answer from the desk is written as.
    pub(crate) seat: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DeskTask {
    pub(crate) run: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) stage: &'static str,
    /// The decision standing in front of it, oldest first
    /// (`Run::pending_gate_on`).
    pub(crate) gate: Option<DeskGate>,
    /// Its dependencies that failed (`Run::blocked_by`).
    pub(crate) blocked_by: Vec<String>,
    pub(crate) created_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DeskGate {
    pub(crate) id: String,
    pub(crate) question: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct StageCount {
    pub(crate) stage: &'static str,
    pub(crate) count: usize,
}

/// The pipeline's stages in the order the board draws them — a task's one
/// word. The flow reads left to right (written down with its dependencies
/// unmet, ready, carried, reported, merged); the three after it are where a
/// task stands still: a decision in front of it, held back, failed.
pub(crate) const STAGES: [&str; 8] = [
    "pending",
    "ready",
    "dispatched",
    "reported",
    "merged",
    "gate",
    "blocked",
    "failed",
];

/// The stages a task is still moving through: listed oldest first, the order
/// the ledger hands work out in. The rest are endings, listed newest first.
const OPEN_STAGES: [&str; 5] = ["pending", "ready", "dispatched", "gate", "blocked"];

/// The most rows one stage carries across the wire. Its count carries the
/// rest: a run of two hundred finished tasks is two hundred numbers nobody
/// reads one by one, and the newest two dozen are what a coordinator checks.
pub(crate) const STAGE_ROWS: usize = 24;

/// A task's one stage word. A standing decision outranks everything — the
/// task will not move until it is answered — and a task waiting on a
/// dependency that failed is held back, not merely pending: it will never
/// become ready by itself. A completed task is merged only where a
/// coordinator wrote so (`ReviewFacts::merged`); a worker's report alone is
/// "reported".
fn stage_of(run: &Run, task: &Task) -> &'static str {
    if run.pending_gate_on(&task.id).is_some() {
        return "gate";
    }
    match task.status {
        TaskStatus::Pending if run.blocked_by(task).is_empty() => "pending",
        TaskStatus::Pending | TaskStatus::Blocked => "blocked",
        TaskStatus::Ready => "ready",
        TaskStatus::Dispatched => "dispatched",
        TaskStatus::Completed if task.review().merged => "merged",
        TaskStatus::Completed => "reported",
        TaskStatus::Failed => "failed",
    }
}

/// Whether a run is in play: somebody is coordinating it, or somebody is
/// still working for it. A finished run with nobody at it is history.
fn in_play(run: &Run) -> bool {
    run.coordinator_live().is_some()
        || run
            .workers
            .iter()
            .any(|worker| worker.state.still_summoned())
}

/// The desk's reading of `ledger`. `holds_seat` answers whether this window
/// holds a coordinator seat named `team/pane`.
pub(crate) fn desk_snapshot(ledger: &Ledger, holds_seat: impl Fn(&str) -> bool) -> DeskSnapshot {
    let mut runs = Vec::new();
    let mut staged: Vec<(&'static str, DeskTask)> = Vec::new();
    let mut mail = Vec::new();
    for run in ledger.runs().iter().filter(|run| in_play(run)) {
        mail.extend(desk_mail(run));
        runs.push(DeskRun {
            run: run.id.clone(),
            name: run.name.clone(),
            seat: run
                .coordinator_live()
                .is_some_and(|seat| holds_seat(&seat.seat)),
        });
        for task in &run.tasks {
            let stage = stage_of(run, task);
            staged.push((
                stage,
                DeskTask {
                    run: run.id.clone(),
                    id: task.id.clone(),
                    title: task.display_name().to_string(),
                    stage,
                    gate: run.pending_gate_on(&task.id).map(|gate| DeskGate {
                        id: gate.id.clone(),
                        question: gate.question.as_str().to_string(),
                    }),
                    blocked_by: run.blocked_by(task),
                    created_ms: task.created_ms,
                },
            ));
        }
    }
    let mut stages = Vec::with_capacity(STAGES.len());
    let mut tasks = Vec::new();
    for stage in STAGES {
        let mut rows: Vec<DeskTask> = staged
            .iter()
            .filter(|(held, _)| *held == stage)
            .map(|(_, task)| task.clone())
            .collect();
        stages.push(StageCount {
            stage,
            count: rows.len(),
        });
        if OPEN_STAGES.contains(&stage) {
            rows.sort_by_key(|task| task.created_ms);
        } else {
            rows.sort_by_key(|task| std::cmp::Reverse(task.created_ms));
        }
        rows.truncate(STAGE_ROWS);
        tasks.extend(rows);
    }
    mail.sort_by_key(|letter| letter.created_ms);
    DeskSnapshot {
        runs,
        tasks,
        stages,
        mail,
    }
}

/// Plan one verb as `run_id`'s live coordinator seat, on the person's behalf:
/// through the handover's native door, with the seat's own capability and the
/// human principal, so the ledger records the window — not a provider
/// session — as the hand that acted, and signs the letter as the seat
/// (`run:<id>`). Refused where this window does not hold that seat.
fn as_the_coordinator(
    run_id: &str,
    argv: Vec<String>,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    let seat = {
        let held = super::runtime().ok_or("orchestration runtime unavailable")?;
        let image = held.actor.view().map_err(|error| format!("{error:?}"))?;
        let ledger = super::cached_ledger(&held, &image).map_err(|error| format!("{error:?}"))?;
        ledger
            .run(run_id)
            .and_then(Run::coordinator_live)
            .map(|seat| seat.seat.clone())
            .ok_or("the run has no live coordinator seat")?
    };
    let (team, pane) = seat
        .split_once('/')
        .ok_or("the coordinator seat names no pane")?;
    let capability = crate::agent_teams::current_pane_capability(team, pane)
        .ok_or("this window does not hold the run's coordinator seat")?;
    super::coordinator_handover::command(team, pane, capability, argv, now_ms)
}

/// Answer a question put to `run_id`'s coordinator, as that seat — the
/// ledger's `reply`, whose rules stand whole: only the seat asked answers,
/// one question has one answer (the same words again are the retry road), and
/// a question whose asker is gone is closed.
pub(crate) fn reply(
    run_id: &str,
    message: &str,
    body: &str,
    retry_request: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if retry_request.trim().is_empty() {
        return Err("retryRequest is required".into());
    }
    if body.trim().is_empty() {
        return Err("an answer needs words".into());
    }
    as_the_coordinator(
        run_id,
        [
            "reply",
            "--run",
            run_id,
            "--to-message",
            message,
            "--body",
            body,
            "--retry-request",
            retry_request,
        ]
        .map(str::to_string)
        .to_vec(),
        now_ms,
    )
}

/// Acknowledge the batch `run_id`'s coordinator holds — the ledger's unit is
/// the batch, never one letter of it, and the desk says how many letters it
/// holds before the press. `--peek`: nothing new is handed over, so every
/// letter the coordinator has not been given still reaches it. Answers only
/// what was acknowledged; the peek's letters stay in the ledger.
pub(crate) fn acknowledge(
    run_id: &str,
    delivery: &str,
    retry_request: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if retry_request.trim().is_empty() {
        return Err("retryRequest is required".into());
    }
    as_the_coordinator(
        run_id,
        [
            "check",
            "--run",
            run_id,
            "--ack",
            delivery,
            "--peek",
            "--retry-request",
            retry_request,
        ]
        .map(str::to_string)
        .to_vec(),
        now_ms,
    )?;
    Ok(serde_json::json!({ "acknowledged": delivery }))
}

/// The machine strip's one answer: what `df -g`, `uptime` and `simctl` told a
/// coordinator.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct MachineLoad {
    /// `None` where the volume could not be measured — unmeasured, never
    /// empty.
    pub(crate) disk: Option<MachineDisk>,
    /// `None` where the platform has no load average.
    pub(crate) load: Option<LoadAverage>,
    pub(crate) devices: BootedDevices,
}

/// The volume the ledger lives on, in the ledger's own words.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct MachineDisk {
    pub(crate) free_bytes: u64,
    /// The ladder the ledger's own disk notices say it in
    /// (`workspace_space::format_bytes`), so the strip and a refusal never
    /// spell one number two ways.
    pub(crate) free: String,
    /// The path measured.
    pub(crate) at: String,
    /// The verdict the next `--worktree` summons would meet.
    pub(crate) room: WorktreeRoom,
    /// The checkouts that verdict was judged beside.
    pub(crate) held_checkouts: usize,
}

/// The one-minute load average against the cores it is shared by.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(crate) struct LoadAverage {
    pub(crate) one_minute: f64,
    pub(crate) cores: usize,
    /// Busier than its cores: the reading under which the harness's frame
    /// budgets are recorded rather than judged (`ui/tests/machine-load.mjs`)
    /// and the release lane waits for calm — the same line, drawn here.
    pub(crate) loud: bool,
}

/// How many simulators and emulators are up on this machine, whoever booted
/// them. `None` is "this machine cannot say" (no Android SDK, not macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct BootedDevices {
    pub(crate) ios_booted: Option<usize>,
    pub(crate) android_booted: Option<usize>,
}

/// The machine's one-minute load average and its cores — one call, no
/// subprocess. The walk bench reads its load through this too.
pub(crate) fn load_average() -> Option<LoadAverage> {
    #[cfg(unix)]
    {
        let mut samples = [0f64; 3];
        // SAFETY: `getloadavg` writes at most the one element asked for into
        // the buffer it is handed, which outlives the call.
        let answered = unsafe { libc::getloadavg(samples.as_mut_ptr(), 1) };
        if answered < 1 {
            return None;
        }
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
        Some(load_reading(samples[0], cores))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn load_reading(one_minute: f64, cores: usize) -> LoadAverage {
    let capacity = f64::from(u32::try_from(cores).unwrap_or(u32::MAX));
    LoadAverage {
        one_minute,
        cores,
        loud: one_minute > capacity,
    }
}

/// Read the strip: the ledger's volume with the summons' own verdict beside
/// the checkouts live workers hold (as the board's beat last published
/// them), the load, and the booted devices. `simctl` and `adb` are processes,
/// so the caller runs this off the main thread.
pub(crate) fn machine_load(ledger_volume: &Path) -> MachineLoad {
    let held = super::board_ledger_snapshot().held_checkouts;
    let disk = super::free_bytes_at(ledger_volume).map(|free_bytes| MachineDisk {
        free_bytes,
        free: zerocode_core::workspace_space::format_bytes(free_bytes),
        at: ledger_volume.display().to_string(),
        room: worktree_room(free_bytes, held),
        held_checkouts: held,
    });
    let (ios_booted, android_booted) = crate::emulator::booted_devices();
    MachineLoad {
        disk,
        load: load_average(),
        devices: BootedDevices {
            ios_booted,
            android_booted,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::orchestration::TaskStatus;

    /// A run in play with one task in every stage, and a finished run nobody
    /// is at: the desk carries the first and not the second, counts every
    /// task by the one stage word the ledger's own facts give it, and says
    /// whether this window holds the run's coordinator seat.
    #[test]
    fn the_pipeline_is_the_ledgers_own_facts_by_one_word() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("desk", 1);
        let task = |ledger: &mut Ledger, title: &str, deps: Vec<String>, at: i64| {
            ledger
                .create_task(&run, format!("do {title}"), title.into(), deps, None, at)
                .expect("a task")
        };
        let ready = task(&mut ledger, "ready", vec![], 10);
        let carried = task(&mut ledger, "carried", vec![], 11);
        let reported = task(&mut ledger, "reported", vec![], 12);
        let merged = task(&mut ledger, "merged", vec![], 13);
        let failed = task(&mut ledger, "failed", vec![], 14);
        let waiting = task(&mut ledger, "waiting", vec![ready.clone()], 15);
        let stranded = task(&mut ledger, "stranded", vec![failed.clone()], 16);
        let held = task(&mut ledger, "held", vec![], 17);
        let gated = task(&mut ledger, "gated", vec![], 18);
        ledger
            .start_worker(&run, "codex", ("team-desk", "%2"), Some(&carried), 20)
            .expect("a worker carries one");
        for (id, status, result) in [
            (&reported, TaskStatus::Completed, "{}"),
            (&merged, TaskStatus::Completed, r#"{"merged":true}"#),
            (&failed, TaskStatus::Failed, ""),
            (&held, TaskStatus::Blocked, ""),
        ] {
            ledger
                .update_task(&run, id, Some(status), Some(result.to_string()))
                .expect("an update");
        }
        let gate = ledger
            .create_gate(
                &run,
                &gated,
                "harvest or dispatch again?".into(),
                vec![],
                30,
            )
            .expect("a gate");
        let finished = ledger.create_run("history", 2);
        ledger
            .create_task(&finished, "old".into(), "old".into(), vec![], None, 3)
            .expect("an old task");

        let desk = desk_snapshot(&ledger, |_| false);
        assert_eq!(desk.runs.len(), 1, "{:?}", desk.runs);
        assert!(!desk.runs[0].seat, "a seat this window does not hold");
        let stage = |id: &str| {
            desk.tasks
                .iter()
                .find(|one| one.id == id)
                .map(|one| one.stage)
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        for (id, word) in [
            (&ready, "ready"),
            (&carried, "dispatched"),
            (&reported, "reported"),
            (&merged, "merged"),
            (&failed, "failed"),
            (&waiting, "pending"),
            (&stranded, "blocked"),
            (&held, "blocked"),
            (&gated, "gate"),
        ] {
            assert_eq!(stage(id), word, "{id}");
        }
        let gated_row = desk
            .tasks
            .iter()
            .find(|one| one.id == gated)
            .expect("gated");
        assert_eq!(
            gated_row.gate.as_ref().map(|one| one.id.as_str()),
            Some(gate.as_str())
        );
        let stranded_row = desk
            .tasks
            .iter()
            .find(|one| one.id == stranded)
            .expect("stranded");
        assert_eq!(stranded_row.blocked_by, vec![failed.clone()]);
        let counts: Vec<(&str, usize)> = desk
            .stages
            .iter()
            .map(|one| (one.stage, one.count))
            .collect();
        assert_eq!(
            counts,
            vec![
                ("pending", 1),
                ("ready", 1),
                ("dispatched", 1),
                ("reported", 1),
                ("merged", 1),
                ("gate", 1),
                ("blocked", 2),
                ("failed", 1),
            ]
        );
    }

    /// A stage carries at most [`STAGE_ROWS`] rows and its count carries the
    /// rest; open stages list oldest first (the order work is handed out in),
    /// endings newest first.
    #[test]
    fn a_long_stage_is_a_count_and_a_window_onto_it() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("long", 1);
        ledger
            .start_worker(&run, "claude", ("team-long", "%2"), None, 2)
            .expect("somebody is at it");
        let mut made = Vec::new();
        for at in 0..(STAGE_ROWS as i64 + 6) {
            let id = ledger
                .create_task(&run, "x".into(), format!("t{at}"), vec![], None, 100 + at)
                .expect("a task");
            made.push(id);
        }
        for id in &made[..10] {
            ledger
                .update_task(&run, id, Some(TaskStatus::Completed), None)
                .expect("done");
        }
        let desk = desk_snapshot(&ledger, |_| true);
        let ready: Vec<&DeskTask> = desk
            .tasks
            .iter()
            .filter(|one| one.stage == "ready")
            .collect();
        let count = |stage: &str| {
            desk.stages
                .iter()
                .find(|one| one.stage == stage)
                .map(|one| one.count)
        };
        assert_eq!(count("ready"), Some(STAGE_ROWS - 4));
        assert_eq!(ready.len(), STAGE_ROWS - 4);
        assert!(
            ready
                .windows(2)
                .all(|pair| pair[0].created_ms <= pair[1].created_ms)
        );
        let reported: Vec<&DeskTask> = desk
            .tasks
            .iter()
            .filter(|one| one.stage == "reported")
            .collect();
        assert_eq!(count("reported"), Some(10));
        assert!(
            reported
                .windows(2)
                .all(|pair| pair[0].created_ms >= pair[1].created_ms)
        );
    }

    /// "Loud" is the harness's line: busier than the cores, not at them.
    #[test]
    fn a_machine_is_loud_only_past_its_cores() {
        assert!(!load_reading(12.0, 12).loud);
        assert!(load_reading(12.1, 12).loud);
        assert!(!load_reading(0.0, 1).loud);
    }

    /// The strip's answer carries the ledger's own words: the verdict in
    /// snake case and the free space in the ledger's ladder.
    #[test]
    fn the_strip_speaks_the_ledgers_words() {
        let disk = MachineDisk {
            free_bytes: 21 * 1024 * 1024 * 1024,
            free: zerocode_core::workspace_space::format_bytes(21 * 1024 * 1024 * 1024),
            at: "/ledger".into(),
            room: worktree_room(21 * 1024 * 1024 * 1024, 2),
            held_checkouts: 2,
        };
        let said = serde_json::to_value(&disk).expect("serializes");
        assert_eq!(said["room"], "tight");
        assert_eq!(said["free"], "21.0 GB");
    }
}
