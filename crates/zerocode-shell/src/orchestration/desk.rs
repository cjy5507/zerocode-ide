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

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::Serialize;
use zerocode_core::orchestration::task_cost::TaskCost;
use zerocode_core::orchestration::{
    Delivery, Ledger, Message, MessageKind, Run, Task, TaskStatus, WorktreeRoom, worktree_room,
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
    /// The letters their coordinators owe an answer, oldest first
    /// ([`DeskLetters::mail`]) — the 「답할 우편」.
    pub(crate) mail: Vec<DeskMail>,
    /// The ledger's news their coordinators have not acknowledged, one line
    /// per notice or per quiet episode, oldest first ([`DeskLetters::news`])
    /// — the 「소식」.
    pub(crate) news: Vec<DeskMail>,
    /// The desk's numbers, counted here once: the screen draws them and never
    /// counts letters again (t-9456).
    pub(crate) counts: DeskCounts,
    /// The batches a coordinator holds open whose notices have all folded
    /// ([`NewsTable::folded_ack`], t-9548): the folded count offers each
    /// one's acknowledgement, since no line of news stands to offer it.
    pub(crate) folded_batches: Vec<DeskBatch>,
}

/// A batch a coordinator holds open — what an acknowledgement names — and
/// how many letters it holds, the ledger acknowledging a batch, never one
/// letter of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DeskBatch {
    pub(crate) run: String,
    pub(crate) delivery_id: String,
    pub(crate) batch: usize,
}

/// The desk's three numbers (t-9456).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub(crate) struct DeskCounts {
    /// Letters owed an answer — the 「답할 우편」 number.
    pub(crate) mail: usize,
    /// Lines of news — the 「소식」 number.
    pub(crate) news: usize,
    /// Notices owed an acknowledgement that no line of news stands for:
    /// older than [`DESK_NEWS`]'s day, or a silence that is over. Counted,
    /// never drawn one by one; the ledger still holds every row.
    pub(crate) folded: usize,
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
    /// A classifier decline's category (`cyber`, `reasoning_extraction`,
    /// …), on a decline and on a switch of model it caused (t-6747).
    pub(crate) category: Option<String>,
    /// Whether the provider routes that category to another model, and the
    /// decline ladder's rung the notice stands on (`handover`, `notify`).
    pub(crate) routed: Option<bool>,
    pub(crate) rung: Option<String>,
    /// A switch of model: the model that answered in the bound one's place,
    /// and how long its CLI keeps it (`session`, `local`).
    pub(crate) switched_to: Option<String>,
    pub(crate) scope: Option<String>,
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
    /// How many of the ledger's rows this line stands for: one, or every
    /// notice of one quiet episode ([`NewsLine::PerEpisode`]) — the line
    /// wears the newest of them.
    pub(crate) notices: usize,
}

/// How the desk's 「소식」 lists the notices a coordinator has not
/// acknowledged — one table (t-9456): which kinds are news, which of them
/// fold into ONE line per quiet episode, and how long any notice stands as a
/// line before it folds into [`DeskCounts::folded`]. Folding hides nothing
/// from the ledger: the inbox, `check` and `inbox` still hold every row.
struct NewsTable {
    /// How long a notice stands as a line, from when the ledger wrote it.
    ///
    /// A day, measured (2026-09-26, this machine's ledger): of 570 notices
    /// the coordinators' inboxes handed over, the age at hand-over was p50
    /// 0.10 h, p90 11.64 h and p99 17.95 h — a day stands past 99 in 100 of
    /// them, and the one night that filled the desk with 46 letters was a
    /// seven-hour gap. What is older is history, and `inbox` reads history.
    stands_ms: i64,
    kinds: [(MessageKind, NewsLine); 6],
    /// Where a folded notice can still be acknowledged from the desk: in the
    /// batch its coordinator holds open, handed over and not acknowledged
    /// (t-9548) — the folded count then offers that whole batch, as a line
    /// in it would. A notice not yet handed over offers nothing (the window
    /// taking it would leave its coordinator never reading it), and one
    /// acknowledged is owed nothing.
    folded_ack: &'static str,
}

/// How many lines one kind of notice earns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NewsLine {
    /// One line per notice: each is news of its own — a wall, a death, a
    /// ring of waiting, a decline, a switch of model.
    PerNotice,
    /// One line per quiet episode, while the silence goes on
    /// ([`Run::quiet_notice_stands`]): the ledger tells the same silence
    /// again every five minutes it lasts, so its notices are copies of one
    /// fact (t-9456 measured 41 of them for ten workers' silences on one
    /// desk). A silence that is over — the attempt ended, the worker spoke
    /// — earns no line at all.
    PerEpisode,
}

const DESK_NEWS: NewsTable = NewsTable {
    stands_ms: 24 * 60 * 60 * 1000,
    kinds: [
        (MessageKind::QuotaWalled, NewsLine::PerNotice),
        (MessageKind::WorkerDied, NewsLine::PerNotice),
        (MessageKind::WentQuiet, NewsLine::PerEpisode),
        (MessageKind::Deadlocked, NewsLine::PerNotice),
        (MessageKind::ClassifierDeclined, NewsLine::PerNotice),
        (MessageKind::ModelDeviated, NewsLine::PerNotice),
    ],
    folded_ack: "delivered",
};

impl NewsTable {
    fn line(&self, kind: MessageKind) -> Option<NewsLine> {
        self.kinds
            .iter()
            .find(|(held, _)| *held == kind)
            .map(|(_, line)| *line)
    }
}

/// Where the letters of one address stand in its inbox.
struct InboxState<'a> {
    pending: HashSet<&'a str>,
    open: HashSet<&'a str>,
    batch: Option<&'a Delivery>,
}

impl<'a> InboxState<'a> {
    fn of(run: &'a Run, address: &str) -> Self {
        let batch = run.open_delivery(address);
        Self {
            pending: run
                .pending_messages(address, &[])
                .into_iter()
                .map(|message| message.id.as_str())
                .collect(),
            open: batch
                .map(|held| held.messages.iter().map(String::as_str).collect())
                .unwrap_or_default(),
            batch,
        }
    }

    /// `pending` (not yet handed over), `delivered` (in the batch the
    /// coordinator holds, unacknowledged) or `acked`.
    fn delivery(&self, id: &str) -> &'static str {
        if self.pending.contains(id) {
            "pending"
        } else if self.open.contains(id) {
            "delivered"
        } else {
            "acked"
        }
    }
}

/// What the coordinator of one run owes, read off its inbox once.
#[derive(Debug, Default)]
pub(crate) struct DeskLetters {
    /// Owed an answer, oldest first: a question put to it, however it was
    /// delivered, until it is answered or can no longer be — the ledger's
    /// own reading of a wait (`Run::awaits_answer`, the one
    /// `Run::awaiting_reply` asks) and the reply verb's own rule
    /// (`Run::question_is_answerable`). The ledger's notices are never owed
    /// an answer.
    pub(crate) mail: Vec<DeskMail>,
    /// Owed an acknowledgement, oldest first: one line per notice or per
    /// quiet episode, as [`DESK_NEWS`] says.
    pub(crate) news: Vec<DeskMail>,
    /// The notices owed an acknowledgement that no line stands for.
    pub(crate) folded: usize,
    /// The open batch holding folded notices when no line stands in it
    /// ([`NewsTable::folded_ack`]).
    pub(crate) folded_batch: Option<DeskBatch>,
}

/// What the coordinator of `run` owes at `now_ms`.
pub(crate) fn desk_letters(run: &Run, now_ms: i64) -> DeskLetters {
    let address = run.address();
    let inbox = InboxState::of(run, &address);
    let mut owed = DeskLetters::default();
    let mut episodes: HashMap<&str, usize> = HashMap::new();
    let mut folded_in_open = false;
    for message in run.messages().iter().filter(|one| one.to == address) {
        if run.awaits_answer(message) {
            if run.question_is_answerable(message).is_ok() {
                owed.mail.push(mail_row(run, message, &inbox));
            }
            continue;
        }
        let Some(line) = DESK_NEWS.line(message.kind) else {
            continue;
        };
        if inbox.delivery(&message.id) == "acked" {
            continue;
        }
        let stands = now_ms.saturating_sub(message.created_ms) < DESK_NEWS.stands_ms
            && (line == NewsLine::PerNotice || run.quiet_notice_stands(message));
        if !stands {
            owed.folded += 1;
            folded_in_open |= inbox.delivery(&message.id) == DESK_NEWS.folded_ack;
            continue;
        }
        let row = mail_row(run, message, &inbox);
        match (line, message.dispatch.as_deref()) {
            (NewsLine::PerEpisode, Some(attempt)) => match episodes.get(attempt) {
                Some(&at) => {
                    let id = owed.news[at].id.clone();
                    let notices = owed.news[at].notices + 1;
                    owed.news[at] = DeskMail { id, notices, ..row };
                }
                None => {
                    episodes.insert(attempt, owed.news.len());
                    owed.news.push(DeskMail {
                        id: episode_line_id(attempt, message),
                        ..row
                    });
                }
            },
            _ => owed.news.push(row),
        }
    }
    owed.news.sort_by_key(|line| line.created_ms);
    // Folded notices in the batch the coordinator holds open are offered
    // beside the folded count — where no line of news stands in that batch to
    // offer it already (a question in it offers its answer, not the batch).
    if folded_in_open
        && let Some(batch) = inbox.batch
        && !owed
            .news
            .iter()
            .any(|line| line.delivery_id.as_deref() == Some(batch.id.as_str()))
    {
        owed.folded_batch = Some(DeskBatch {
            run: run.id.clone(),
            delivery_id: batch.id.clone(),
            batch: batch.messages.len(),
        });
    }
    owed
}

/// The key the ledger's quiet notices carry the moment their silence began
/// under — the same on every notice of one episode.
const EPISODE_STARTED: &str = "episodeStartedMs";

/// A quiet episode's line is named by its attempt and the moment its silence
/// began, so it keeps its name while the ledger tells the silence again
/// every few minutes, and an early notice acknowledged does not rename it
/// (t-9548). A notice written before the moment was recorded names the line
/// by itself.
fn episode_line_id(attempt: &str, notice: &Message) -> String {
    serde_json::from_str::<serde_json::Value>(notice.body.as_str())
        .ok()
        .and_then(|body| {
            body.get(EPISODE_STARTED)
                .and_then(serde_json::Value::as_i64)
        })
        .map_or_else(
            || notice.id.clone(),
            |started| format!("{attempt}@{started}"),
        )
}

fn mail_row(run: &Run, message: &Message, inbox: &InboxState) -> DeskMail {
    let delivery = inbox.delivery(&message.id);
    let batch = inbox.batch;
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
        category: said["category"].as_str().map(str::to_string),
        routed: said["routed"].as_bool(),
        rung: said["rung"].as_str().map(str::to_string),
        switched_to: said["to"].as_str().map(str::to_string),
        scope: said["scope"].as_str().map(str::to_string),
        created_ms: message.created_ms,
        delivery,
        delivery_id: batch.filter(|_| delivered).map(|held| held.id.clone()),
        batch: batch.filter(|_| delivered).map(|held| held.messages.len()),
        notices: 1,
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
    /// What the task cost, for a finished one ([`finished`], t-9470) —
    /// `None` while it is still moving.
    pub(crate) cost: Option<TaskCost>,
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

/// The stages a finished task stands in — reported, merged — the ones whose
/// rows carry the task's cost (t-9470).
const FINISHED_STAGES: [&str; 2] = ["reported", "merged"];

/// The most rows one stage carries across the wire. Its count carries the
/// rest: a run of two hundred finished tasks is two hundred numbers nobody
/// reads one by one, and the newest two dozen are what a coordinator checks.
pub(crate) const STAGE_ROWS: usize = 24;

/// A task's one stage word. A standing decision outranks everything — the
/// task will not move until it is answered — and a task waiting on a
/// dependency that failed is held back, not merely pending: it will never
/// become ready by itself. A completed task is merged only where a
/// coordinator wrote so against the task's newest attempt
/// (`Run::review_of`, `ReviewFacts::merged`); a worker's report alone —
/// whatever keys its body carries — is "reported".
fn stage_of(run: &Run, task: &Task) -> &'static str {
    if run.pending_gate_on(&task.id).is_some() {
        return "gate";
    }
    match task.status {
        TaskStatus::Pending if run.blocked_by(task).is_empty() => "pending",
        TaskStatus::Pending | TaskStatus::Blocked => "blocked",
        TaskStatus::Ready => "ready",
        TaskStatus::Dispatched => "dispatched",
        TaskStatus::Completed if run.review_of(task).merged => "merged",
        TaskStatus::Completed => "reported",
        TaskStatus::Failed => "failed",
    }
}

/// Whether a task is finished: reported, or merged ([`FINISHED_STAGES`]).
pub(crate) fn finished(run: &Run, task: &Task) -> bool {
    FINISHED_STAGES.contains(&stage_of(run, task))
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
/// holds a coordinator seat named `team/pane`; `cost` what a finished task
/// cost, asked only of the rows the desk carries.
pub(crate) fn desk_snapshot(
    ledger: &Ledger,
    holds_seat: impl Fn(&str) -> bool,
    mut cost: impl FnMut(&Run, &Task) -> TaskCost,
) -> DeskSnapshot {
    let now_ms = crate::now_epoch_ms();
    let mut runs = Vec::new();
    let mut staged: Vec<(&Run, &Task, DeskTask)> = Vec::new();
    let mut mail = Vec::new();
    let mut news = Vec::new();
    let mut folded = 0;
    let mut folded_batches = Vec::new();
    for run in ledger.runs().iter().filter(|run| in_play(run)) {
        let owed = desk_letters(run, now_ms);
        mail.extend(owed.mail);
        news.extend(owed.news);
        folded += owed.folded;
        folded_batches.extend(owed.folded_batch);
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
                run,
                task,
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
                    cost: None,
                },
            ));
        }
    }
    let mut stages = Vec::with_capacity(STAGES.len());
    let mut tasks = Vec::new();
    for stage in STAGES {
        let mut rows: Vec<&(&Run, &Task, DeskTask)> = staged
            .iter()
            .filter(|(_, _, row)| row.stage == stage)
            .collect();
        stages.push(StageCount {
            stage,
            count: rows.len(),
        });
        if OPEN_STAGES.contains(&stage) {
            rows.sort_by_key(|(_, _, row)| row.created_ms);
        } else {
            rows.sort_by_key(|(_, _, row)| std::cmp::Reverse(row.created_ms));
        }
        rows.truncate(STAGE_ROWS);
        // A finished task's cost, asked of the rows the desk sends and no
        // other — a stage of two hundred is two dozen rows and a count.
        let finished = FINISHED_STAGES.contains(&stage);
        tasks.extend(rows.into_iter().map(|(run, task, row)| DeskTask {
            cost: finished.then(|| cost(run, task)),
            ..row.clone()
        }));
    }
    mail.sort_by_key(|letter| letter.created_ms);
    news.sort_by_key(|line| line.created_ms);
    DeskSnapshot {
        runs,
        tasks,
        stages,
        counts: DeskCounts {
            mail: mail.len(),
            news: news.len(),
            folded,
        },
        mail,
        news,
        folded_batches,
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

/// The person's ledger store as it stands, read into a projection: its
/// snapshot is taken into a scratch file, and only that copy grows the
/// columns this build reads that an older window's store may not have yet —
/// the same additive step the window's own open takes. The store itself is
/// only ever read. For the measurements over the ledger that already
/// happened (the desk's, t-9456; the costs', t-9470).
#[cfg(test)]
pub(crate) fn projection_at_rest(store: &str) -> zerocode_core::orchestration::LedgerProjectionV1 {
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let copy = scratch.path().join("authority.sqlite");
    rusqlite::Connection::open_with_flags(
        store,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .expect("the store opens read-only")
    .execute("VACUUM INTO ?1", [copy.display().to_string()])
    .expect("a snapshot of the store");
    let connection = rusqlite::Connection::open(&copy).expect("the snapshot opens");
    zerocode_orchestrator::ledger_store::ensure_ledger_columns(&connection)
        .expect("the snapshot grows this build's columns");
    zerocode_orchestrator::ledger_store::read(
        &connection,
        "main-ledger",
        zerocode_core::orchestration::PROJECTION_SCHEMA,
    )
    .expect("the store reads")
    .expect("the store holds the main ledger")
    .projection
}

#[cfg(test)]
mod tests {
    use super::*;
    use zerocode_core::orchestration::{
        AckedRow, Draft, Priority, ResultAuthor, TaskStatus, Text, worker_address,
    };

    const HOUR_MS: i64 = 60 * 60 * 1000;
    /// The ledger's stall reminder cadence (`QUIET_REMINDER_MS`), so each
    /// beat below tells the coordinator once more.
    const REMINDED_MS: i64 = 300_000;

    /// A cost nobody asked about — for the tests that read the rest.
    fn no_cost(_: &Run, _: &Task) -> TaskCost {
        TaskCost::default()
    }

    /// The desk as the window serializes it. The red tests below read the
    /// JSON the screen reads, so they speak to any shape of the snapshot.
    fn desk_json(ledger: &Ledger) -> serde_json::Value {
        serde_json::to_value(desk_snapshot(ledger, |_| false, no_cost))
            .expect("the desk serializes")
    }

    /// Every letter the desk draws, in either of its lists.
    fn drawn(desk: &serde_json::Value) -> Vec<serde_json::Value> {
        ["mail", "news"]
            .iter()
            .flat_map(|field| desk[*field].as_array().cloned().unwrap_or_default())
            .collect()
    }

    /// A run with somebody still at it (a second worker, so it stays in
    /// play), and a worker in `%2` carrying a task: the run and that worker.
    fn a_worker_carrying_a_task(ledger: &mut Ledger, at: i64) -> (String, String) {
        let run_id = ledger.create_run("desk-news", at);
        ledger
            .start_worker(&run_id, "codex", ("team-news", "%3"), None, at + 1)
            .expect("somebody is still at the run");
        let task = ledger
            .create_task(
                &run_id,
                "stall".into(),
                "stall".into(),
                vec![],
                None,
                at + 2,
            )
            .expect("a task");
        let worker = ledger
            .start_worker(&run_id, "claude", ("team-news", "%2"), Some(&task), at + 3)
            .expect("the worker")
            .worker;
        (run_id, worker)
    }

    fn a_notice(run_id: &str, kind: MessageKind, body: &str) -> Draft {
        Draft {
            from: zerocode_core::orchestration::LEDGER_ITSELF.to_string(),
            to: format!("run:{run_id}"),
            kind,
            body: Text::from(body),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: Text::default(),
            thread: None,
            task: None,
            dispatch: None,
        }
    }

    /// t-9456, the night the desk said 「답할 우편 46」 and no question stood:
    /// a worker's silence is told again every five minutes while its attempt
    /// is open, and every one of those notices stood as a letter to answer
    /// long after the worker had died. Once the attempt ends the silence is
    /// over — the ledger writes nothing more about it, and the desk draws
    /// none of it; the death itself is the news, once.
    #[test]
    fn a_silence_that_is_over_leaves_the_desk_and_nothing_is_written_after_it() {
        let start = crate::now_epoch_ms() - 3 * HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, worker) = a_worker_carrying_a_task(&mut ledger, start);
        for beat in 0..3 {
            assert_eq!(
                ledger.workers_stalled(
                    &[(worker.clone(), start + 10)],
                    start + 200_000 + beat * REMINDED_MS
                ),
                1,
                "stall {beat} told nobody"
            );
        }
        assert_eq!(
            ledger
                .terminal_gone("team-news", "%2", start + HOUR_MS)
                .as_deref(),
            Some(worker.as_str()),
            "the pane died while it carried the task"
        );
        let told = |ledger: &Ledger| {
            ledger
                .run(&run_id)
                .expect("the run")
                .messages()
                .iter()
                .filter(|one| matches!(one.kind, MessageKind::WentQuiet | MessageKind::WorkerDied))
                .count()
        };
        let written = told(&ledger);
        assert_eq!(written, 4, "three stalls and one death");
        for beat in 1..=10 {
            let now_ms = start + HOUR_MS + beat * REMINDED_MS;
            assert_eq!(
                ledger.workers_stalled(&[(worker.clone(), start + 10)], now_ms),
                0
            );
            assert_eq!(
                ledger.stall_causes_judged(
                    &[zerocode_core::orchestration::StallJudged {
                        worker: worker.clone(),
                        stalled_since_ms: start + 10 + beat,
                        cause: "finished_turn".into(),
                        confidence: 0.9,
                    }],
                    now_ms,
                ),
                0
            );
        }
        assert_eq!(
            told(&ledger),
            written,
            "a notice was written about an attempt that had ended"
        );

        let desk = desk_json(&ledger);
        let letters = drawn(&desk);
        assert!(
            !letters.iter().any(|one| one["kind"] == "went_quiet"),
            "a silence that is over still stands on the desk: {desk}"
        );
        assert_eq!(
            letters
                .iter()
                .filter(|one| one["kind"] == "worker_died")
                .count(),
            1,
            "the death is news, once: {desk}"
        );
        assert_eq!(
            desk["counts"]["mail"], 0,
            "nothing here waits on an answer: {desk}"
        );
        assert_eq!(
            desk["counts"]["folded"], 3,
            "the three stalls fold into the count: {desk}"
        );
    }

    /// One silence, told ten times, is one line — and the worker's own word
    /// ends it: the line leaves, and its notices fold into the count.
    #[test]
    fn one_silence_told_ten_times_is_one_line_until_the_worker_speaks() {
        let start = crate::now_epoch_ms() - 3 * HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, worker) = a_worker_carrying_a_task(&mut ledger, start);
        for beat in 0..10 {
            assert_eq!(
                ledger.workers_stalled(
                    &[(worker.clone(), start + 10)],
                    start + 200_000 + beat * REMINDED_MS
                ),
                1
            );
        }
        let desk = desk_json(&ledger);
        let quiet: Vec<serde_json::Value> = drawn(&desk)
            .into_iter()
            .filter(|one| one["kind"] == "went_quiet")
            .collect();
        assert_eq!(quiet.len(), 1, "one silence, one line: {desk}");
        assert_eq!(
            quiet[0]["notices"], 10,
            "the line says how many notices it stands for: {desk}"
        );
        assert_eq!(quiet[0]["worker"], worker.as_str());
        assert_eq!(desk["counts"]["news"], 1, "{desk}");
        assert_eq!(
            desk["counts"]["mail"], 0,
            "a silence is not a letter to answer: {desk}"
        );

        ledger
            .post(
                &run_id,
                Draft {
                    from: worker_address(&worker),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::Status,
                    body: "still at it".into(),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: None,
                    dispatch: None,
                },
                start + 2 * HOUR_MS,
            )
            .expect("the worker speaks");
        let desk = desk_json(&ledger);
        assert!(
            !drawn(&desk).iter().any(|one| one["kind"] == "went_quiet"),
            "the worker spoke and its silence still stands: {desk}"
        );
        assert_eq!(desk["counts"]["folded"], 10, "{desk}");
    }

    /// 「답할 우편」 is the letters that wait on an answer and nothing else: a
    /// question nobody has answered. The ledger's notices, a status, an
    /// answered question and a reply that wears the question kind are not.
    #[test]
    fn only_a_question_waiting_on_its_answer_is_mail_to_answer() {
        let start = crate::now_epoch_ms() - HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, _) = a_worker_carrying_a_task(&mut ledger, start);
        let address = format!("run:{run_id}");
        let from_pane = |pane: &str, kind: MessageKind, body: &str, thread: Option<String>| Draft {
            from: format!("pane:team-news/{pane}"),
            to: address.clone(),
            kind,
            body: Text::from(body),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: Text::default(),
            thread,
            task: None,
            dispatch: None,
        };
        let waiting = ledger
            .post(
                &run_id,
                from_pane("%7", MessageKind::Question, "main에 올릴까요?", None),
                start + 1,
            )
            .expect("a question");
        let answered = ledger
            .post(
                &run_id,
                from_pane("%8", MessageKind::Question, "끝났나요?", None),
                start + 2,
            )
            .expect("another question");
        ledger
            .post(
                &run_id,
                Draft {
                    from: address.clone(),
                    to: "pane:team-news/%8".into(),
                    kind: MessageKind::Question,
                    body: "네".into(),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: Some(answered.clone()),
                    task: None,
                    dispatch: None,
                },
                start + 3,
            )
            .expect("its answer");
        ledger
            .post(
                &run_id,
                from_pane("%8", MessageKind::Question, "고마워요", Some(answered)),
                start + 4,
            )
            .expect("a reply wearing the question kind");
        ledger
            .post(
                &run_id,
                from_pane("%7", MessageKind::Status, "heads-up", None),
                start + 5,
            )
            .expect("a status");
        for (at, kind) in [
            (6, MessageKind::WorkerDied),
            (7, MessageKind::QuotaWalled),
            (8, MessageKind::Deadlocked),
            (9, MessageKind::ClassifierDeclined),
            (10, MessageKind::ModelDeviated),
        ] {
            ledger
                .post(
                    &run_id,
                    a_notice(&run_id, kind, r#"{"workerId":"w-elsewhere"}"#),
                    start + at,
                )
                .expect("a notice");
        }
        let desk = desk_json(&ledger);
        let mail = desk["mail"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            mail.iter()
                .map(|one| one["id"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec![waiting.as_str()],
            "only the waiting question is owed an answer: {desk}"
        );
        assert_eq!(desk["counts"]["mail"], 1, "{desk}");
        assert_eq!(
            desk["counts"]["news"], 5,
            "the notices are news, one line each: {desk}"
        );
    }

    /// A notice stands as a line for a day from when the ledger wrote it;
    /// after that it folds into the count — the inbox still holds it.
    #[test]
    fn a_notice_older_than_a_day_folds_into_the_count() {
        let now = crate::now_epoch_ms();
        let mut ledger = Ledger::new();
        let (run_id, _) = a_worker_carrying_a_task(&mut ledger, now - 30 * HOUR_MS);
        let old = ledger
            .post(
                &run_id,
                a_notice(&run_id, MessageKind::WorkerDied, r#"{"workerId":"w-old"}"#),
                now - 25 * HOUR_MS,
            )
            .expect("a day-old notice");
        let fresh = ledger
            .post(
                &run_id,
                a_notice(&run_id, MessageKind::WorkerDied, r#"{"workerId":"w-new"}"#),
                now - HOUR_MS,
            )
            .expect("an hour-old notice");
        let desk = desk_json(&ledger);
        let ids: Vec<String> = drawn(&desk)
            .iter()
            .filter_map(|one| one["id"].as_str().map(str::to_string))
            .collect();
        assert_eq!(ids, vec![fresh], "{desk}");
        assert_eq!(desk["counts"]["folded"], 1, "{desk}");
        let run = ledger.run(&run_id).expect("the run");
        assert!(
            run.pending_messages(&run.address(), &[])
                .iter()
                .any(|one| one.id == old),
            "folding took the notice out of the inbox"
        );
    }

    /// One quiet episode is one line with one name (t-9548): the ledger tells
    /// the silence again every five minutes, and the line keeps the name its
    /// silence began with — never the newest notice's id, which moved every
    /// reminder and made the screen draw a new line each time.
    #[test]
    fn an_episode_line_keeps_its_name_while_the_silence_is_told_again() {
        let start = crate::now_epoch_ms() - 3 * HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, worker) = a_worker_carrying_a_task(&mut ledger, start);
        let mut names = Vec::new();
        for beat in 0..3 {
            assert_eq!(
                ledger.workers_stalled(
                    &[(worker.clone(), start + 10)],
                    start + 200_000 + beat * REMINDED_MS
                ),
                1
            );
            let desk = desk_json(&ledger);
            let quiet: Vec<serde_json::Value> = drawn(&desk)
                .into_iter()
                .filter(|one| one["kind"] == "went_quiet")
                .collect();
            assert_eq!(quiet.len(), 1, "{desk}");
            names.push(quiet[0]["id"].as_str().unwrap_or_default().to_string());
        }
        assert!(
            names.windows(2).all(|pair| pair[0] == pair[1]) && !names[0].is_empty(),
            "the line was renamed by a reminder: {names:?}"
        );
        let newest = ledger
            .run(&run_id)
            .expect("the run")
            .messages()
            .iter()
            .rev()
            .find(|one| one.kind == MessageKind::WentQuiet)
            .map(|one| one.id.clone())
            .expect("a notice");
        assert_ne!(names[0], newest, "the line wears the newest notice's id");
    }

    /// Notices that have all folded, in a batch their coordinator holds open,
    /// are offered for acknowledgement beside the folded count (t-9548) — the
    /// one batch, whole, as a line in it would offer it. A folded notice not
    /// yet handed over offers nothing, an acknowledged one is not owed at
    /// all, and a batch a line still stands in is that line's to offer.
    #[test]
    fn folded_notices_in_the_open_batch_offer_that_batch_and_no_other_folds_do() {
        let now = crate::now_epoch_ms();
        let mut ledger = Ledger::new();
        let (run_id, _) = a_worker_carrying_a_task(&mut ledger, now - 30 * HOUR_MS);
        let notice = |ledger: &mut Ledger, at: i64| {
            ledger
                .post(
                    &run_id,
                    a_notice(&run_id, MessageKind::WorkerDied, r#"{"workerId":"w-gone"}"#),
                    at,
                )
                .expect("a notice")
        };
        let old_a = notice(&mut ledger, now - 27 * HOUR_MS);
        let old_b = notice(&mut ledger, now - 26 * HOUR_MS);
        let address = format!("run:{run_id}");
        let handed = |ledger: &Ledger, batch: &[&String]| {
            let mut projected = ledger.export();
            let inbox = projected
                .inboxes
                .iter_mut()
                .find(|row| row.run == run_id && row.address == address)
                .expect("the run's inbox");
            inbox.pending.retain(|id| !batch.contains(&id));
            inbox.open = Some(Delivery {
                id: "d-folded".into(),
                messages: batch.iter().map(|id| (*id).clone()).collect(),
                holder: None,
                opened_ms: None,
            });
            Ledger::rebuild(projected).expect("a batch handed over")
        };

        let unread = desk_snapshot(&ledger, |_| true, no_cost);
        assert_eq!(unread.counts.folded, 2);
        assert!(
            unread.folded_batches.is_empty(),
            "a notice nobody was handed is offered: {:?}",
            unread.folded_batches
        );

        let held = desk_snapshot(&handed(&ledger, &[&old_a, &old_b]), |_| true, no_cost);
        assert_eq!(held.counts.folded, 2);
        assert!(held.news.is_empty(), "{:?}", held.news);
        assert_eq!(
            held.folded_batches,
            vec![DeskBatch {
                run: run_id.clone(),
                delivery_id: "d-folded".into(),
                batch: 2,
            }]
        );

        let question = ledger
            .post(
                &run_id,
                Draft {
                    from: "pane:team-news/%9".into(),
                    to: address.clone(),
                    kind: MessageKind::Question,
                    body: "main에 올릴까요?".into(),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: None,
                    dispatch: None,
                },
                now - 2 * HOUR_MS,
            )
            .expect("a question");
        let asked = desk_snapshot(
            &handed(&ledger, &[&old_a, &old_b, &question]),
            |_| true,
            no_cost,
        );
        assert_eq!(asked.mail.len(), 1, "{:?}", asked.mail);
        assert_eq!(
            asked.folded_batches.len(),
            1,
            "a question in the batch offers its answer, not the batch: {:?}",
            asked.folded_batches
        );

        let fresh = notice(&mut ledger, now - HOUR_MS);
        let standing = desk_snapshot(
            &handed(&ledger, &[&old_a, &old_b, &question, &fresh]),
            |_| true,
            no_cost,
        );
        assert_eq!(standing.news.len(), 1, "{:?}", standing.news);
        assert_eq!(standing.news[0].delivery_id.as_deref(), Some("d-folded"));
        assert!(
            standing.folded_batches.is_empty(),
            "the batch is offered twice: {:?}",
            standing.folded_batches
        );
    }

    /// A finished task's row carries its cost and a moving one's none, and
    /// the cost is asked only of the rows the desk sends — a stage of thirty
    /// finished tasks is worked out twenty-four times, not thirty.
    #[test]
    fn a_finished_row_carries_its_cost_and_only_the_rows_the_desk_sends_are_costed() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("costed", 1);
        ledger
            .start_worker(&run, "claude", ("team-costed", "%2"), None, 2)
            .expect("somebody is at it");
        let finished_count = STAGE_ROWS as i64 + 6;
        for at in 0..finished_count + 3 {
            let id = ledger
                .create_task(&run, "x".into(), format!("t{at}"), vec![], None, 100 + at)
                .expect("a task");
            if at < finished_count {
                ledger
                    .update_task(
                        &run,
                        &id,
                        Some(TaskStatus::Completed),
                        None,
                        ResultAuthor::Ledger,
                    )
                    .expect("done");
            }
        }
        let mut asked = Vec::new();
        let desk = desk_snapshot(
            &ledger,
            |_| false,
            |_, task| {
                asked.push(task.id.clone());
                TaskCost {
                    attempts: 7,
                    ..TaskCost::default()
                }
            },
        );
        assert_eq!(
            asked.len(),
            STAGE_ROWS,
            "the cost was asked of rows the desk does not send"
        );
        for row in &desk.tasks {
            let attempts = row.cost.as_ref().map(|one| one.attempts);
            if FINISHED_STAGES.contains(&row.stage) {
                assert_eq!(attempts, Some(7), "{row:?}");
            } else {
                assert_eq!(attempts, None, "{row:?}");
            }
        }
        let said = serde_json::to_value(&desk).expect("the desk serializes");
        let costed = said["tasks"]
            .as_array()
            .and_then(|rows| rows.iter().find(|one| one["stage"] == "reported"))
            .expect("a reported row");
        assert_eq!(costed["cost"]["attempts"], 7, "{costed}");
        assert!(
            costed["cost"]["generation"]["sessionsKnown"].is_u64(),
            "{costed}"
        );
        let moving = said["tasks"]
            .as_array()
            .and_then(|rows| rows.iter().find(|one| one["stage"] == "ready"))
            .expect("a ready row");
        assert!(moving["cost"].is_null(), "{moving}");
    }

    #[test]
    fn a_question_the_ledger_would_refuse_to_answer_is_not_owed_on_the_desk() {
        let mut ledger = Ledger::new();
        let run_id = ledger.create_run("mail", 1);
        let worker = ledger
            .start_worker(&run_id, "codex", ("team", "%2"), None, 2)
            .expect("worker")
            .worker;
        let question = ledger
            .post(
                &run_id,
                Draft {
                    from: format!("worker:{worker}"),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::Question,
                    body: "still there?".into(),
                    subject: "".into(),
                    priority: Priority::Normal,
                    payload: "".into(),
                    thread: None,
                    task: None,
                    dispatch: None,
                },
                3,
            )
            .expect("question");
        assert_eq!(
            desk_letters(ledger.run(&run_id).expect("run"), 4)
                .mail
                .len(),
            1
        );
        ledger.begin_release(&worker).expect("release begins");
        assert_eq!(ledger.finish_release(&worker, None).as_str(), "released");
        let run = ledger.run(&run_id).expect("run");
        assert_eq!(run.messages().len(), 1, "the question stays in the ledger");
        assert_eq!(run.messages()[0].id, question);
        assert!(
            desk_letters(run, 4).mail.is_empty(),
            "unanswerable question is still owed"
        );
    }

    #[test]
    fn old_questions_disappear_after_rebuild_without_rewriting_mail() {
        let mut ledger = Ledger::new();
        let run_id = ledger.create_run("old mail", 1);
        let mut workers = Vec::new();
        for (index, pane) in ["%2", "%3", "%4"].into_iter().enumerate() {
            let worker = ledger
                .start_worker(&run_id, "codex", ("team", pane), None, index as i64 + 2)
                .expect("worker")
                .worker;
            ledger
                .post(
                    &run_id,
                    Draft {
                        from: format!("worker:{worker}"),
                        to: format!("run:{run_id}"),
                        kind: MessageKind::Question,
                        body: "still needed?".into(),
                        subject: "".into(),
                        priority: Priority::Normal,
                        payload: "".into(),
                        thread: None,
                        task: None,
                        dispatch: None,
                    },
                    10 + index as i64,
                )
                .expect("question");
            workers.push(worker);
        }
        let pane_question = ledger
            .post(
                &run_id,
                Draft {
                    from: "pane:team/%5".into(),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::Question,
                    body: "pane question?".into(),
                    subject: "".into(),
                    priority: Priority::Normal,
                    payload: "".into(),
                    thread: None,
                    task: None,
                    dispatch: None,
                },
                20,
            )
            .expect("pane question");
        assert_eq!(
            desk_letters(ledger.run(&run_id).expect("run"), 20)
                .mail
                .len(),
            4
        );
        for worker in &workers {
            ledger.begin_release(worker).expect("release begins");
            ledger.finish_release(worker, None);
        }
        ledger
            .post(
                &run_id,
                Draft {
                    from: format!("run:{run_id}"),
                    to: "pane:team/%5".into(),
                    kind: MessageKind::Question,
                    body: "answered".into(),
                    subject: "".into(),
                    priority: Priority::Normal,
                    payload: "".into(),
                    thread: Some(pane_question),
                    task: None,
                    dispatch: None,
                },
                21,
            )
            .expect("pane answer");
        let run = ledger.run(&run_id).expect("run");
        assert_eq!(run.messages().len(), 5, "old questions were rewritten");
        assert!(desk_letters(run, 22).mail.is_empty());
        let rebuilt = Ledger::rebuild(ledger.export()).expect("same ledger rebuilt");
        assert!(
            desk_letters(rebuilt.run(&run_id).expect("run"), 22)
                .mail
                .is_empty()
        );
    }

    #[test]
    fn an_acknowledged_unanswered_question_stays_owed_but_an_unacked_notice_stays_separate() {
        let mut ledger = Ledger::new();
        let run_id = ledger.create_run("acknowledged question", 1);
        let question = ledger
            .post(
                &run_id,
                Draft {
                    from: "pane:team/%2".into(),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::Question,
                    body: "still unanswered?".into(),
                    subject: "".into(),
                    priority: Priority::Normal,
                    payload: "".into(),
                    thread: None,
                    task: None,
                    dispatch: None,
                },
                2,
            )
            .expect("question");
        let notice = ledger
            .post(
                &run_id,
                Draft {
                    from: "pane:team/%2".into(),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::WorkerDied,
                    body: r#"{"workerId":"w-example"}"#.into(),
                    subject: "".into(),
                    priority: Priority::Normal,
                    payload: "".into(),
                    thread: None,
                    task: None,
                    dispatch: None,
                },
                3,
            )
            .expect("notice");
        let mut projected = ledger.export();
        let inbox = projected
            .inboxes
            .iter_mut()
            .find(|row| row.run == run_id && row.address == format!("run:{run_id}"))
            .expect("run inbox");
        inbox.pending.retain(|id| id != &question);
        projected.acked.push(AckedRow {
            run: run_id.clone(),
            address: format!("run:{run_id}"),
            delivery: "d-acknowledged".into(),
            messages: Some(vec![question.clone()]),
            seq: 0,
            current: true,
        });
        let rebuilt = Ledger::rebuild(projected).expect("acked ledger");
        let run = rebuilt.run(&run_id).expect("run");
        let owed = desk_letters(run, 4);
        assert_eq!(owed.mail.len(), 1);
        assert_eq!(owed.mail[0].id, question);
        assert_eq!(owed.mail[0].delivery, "acked");
        assert_eq!(owed.news.len(), 1);
        assert_eq!(owed.news[0].id, notice);
        assert_eq!(owed.news[0].delivery, "pending");
    }

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
        let claimed = task(&mut ledger, "claimed", vec![], 19);
        ledger
            .start_worker(&run, "codex", ("team-desk", "%2"), Some(&carried), 20)
            .expect("a worker carries one");
        /* The real road (t-6815): a worker's own `worker_done`, its body
         * saying merged, through `Ledger::post` — stored as the worker's
         * claim, so the desk reads "reported", never "merged". */
        let claimer = ledger
            .start_worker(&run, "claude", ("team-desk", "%3"), Some(&claimed), 21)
            .expect("a worker carries the claimed one");
        ledger
            .post(
                &run,
                Draft {
                    from: worker_address(&claimer.worker),
                    to: ledger.run(&run).expect("the run").address(),
                    kind: MessageKind::WorkerDone,
                    body: Text::from(r#"{"ok":true,"merged":true,"verified":true}"#.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(claimed.clone()),
                    dispatch: claimer.dispatch.clone(),
                },
                22,
            )
            .expect("the worker's report");
        for (id, status, result) in [
            (&reported, TaskStatus::Completed, "{}"),
            (&merged, TaskStatus::Completed, r#"{"merged":true}"#),
            (&failed, TaskStatus::Failed, ""),
            (&held, TaskStatus::Blocked, ""),
        ] {
            ledger
                .update_task(
                    &run,
                    id,
                    Some(status),
                    Some(result.to_string()),
                    ResultAuthor::Coordinator {
                        seat: "team-desk/%1".to_string(),
                        generation: Some(1),
                        attempt: None,
                        source: None,
                    },
                )
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

        let desk = desk_snapshot(&ledger, |_| false, no_cost);
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
            (&claimed, "reported"),
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
                ("reported", 2),
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
                .update_task(
                    &run,
                    id,
                    Some(TaskStatus::Completed),
                    None,
                    ResultAuthor::Ledger,
                )
                .expect("done");
        }
        let desk = desk_snapshot(&ledger, |_| true, no_cost);
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

    /// The desk over the ledger that already happened (t-9456): the store
    /// read into a snapshot, the mail written after an instant dropped, the
    /// batches handed over after it put back in their queues and their
    /// receipts with them — and then the desk this build draws, as numbers
    /// only (no body, id or path leaves). Run on the base and on the change,
    /// it is the before and after the report reads. Every id the ledger mints
    /// comes off one counter, so a batch numbered past the newest message of
    /// the instant was handed over after it; take the instant at a message's
    /// own stamp and that is exact. Workers and attempts are read as they
    /// stand now, so an instant is exact only where every attempt its notices
    /// name had already ended or is still open — the report says which
    /// instant was checked that way.
    ///
    /// ```sh
    /// ZEROCODE_DESK_REPLAY_STORE="$HOME/Library/Application Support/dev.zerocode.app/authority/authority.sqlite" \
    /// ZEROCODE_DESK_REPLAY_AT=<a message's created_ms> \
    ///   cargo test -p zerocode-shell --bin zerocode-shell \
    ///   orchestration::desk::tests::the_desk_over_the_ledger_that_already_happened \
    ///   -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a measurement over the person's own ledger, printed; not a check"]
    fn the_desk_over_the_ledger_that_already_happened() {
        let store = std::env::var("ZEROCODE_DESK_REPLAY_STORE")
            .expect("ZEROCODE_DESK_REPLAY_STORE names the authority store");
        let at: i64 = std::env::var("ZEROCODE_DESK_REPLAY_AT")
            .ok()
            .and_then(|said| said.parse().ok())
            .unwrap_or(i64::MAX);
        let mut projected = projection_at_rest(&store);
        let number = |id: &str| {
            id.rsplit_once('-')
                .and_then(|(_, number)| number.parse::<u64>().ok())
        };
        projected.messages.retain(|row| row.created_ms <= at);
        let minted = projected
            .messages
            .iter()
            .filter_map(|row| number(&row.id))
            .max()
            .unwrap_or(0);
        let later = |delivery: &str| number(delivery).is_some_and(|held| held > minted);
        let written: HashSet<(String, String)> = projected
            .messages
            .iter()
            .map(|row| (row.run.clone(), row.id.clone()))
            .collect();
        let mut back: Vec<(String, String, Vec<String>)> = projected
            .acked
            .iter()
            .filter(|row| later(&row.delivery))
            .map(|row| {
                (
                    row.run.clone(),
                    row.address.clone(),
                    row.messages.clone().unwrap_or_default(),
                )
            })
            .collect();
        projected.acked.retain(|row| !later(&row.delivery));
        projected
            .served
            .retain(|row| row.filed_ms.is_none_or(|filed| filed <= at));
        for inbox in &mut projected.inboxes {
            if let Some(open) = inbox.open.take_if(|open| later(&open.id)) {
                back.push((inbox.run.clone(), inbox.address.clone(), open.messages));
            }
            let mut queue: Vec<String> = back
                .iter()
                .filter(|(run, address, _)| *run == inbox.run && *address == inbox.address)
                .flat_map(|(_, _, ids)| ids.iter().cloned())
                .collect();
            queue.append(&mut inbox.pending);
            queue.retain(|id| written.contains(&(inbox.run.clone(), id.clone())));
            inbox.pending = queue;
        }
        let ledger = Ledger::rebuild(projected).expect("the ledger as it stood");
        let desk = desk_json(&ledger);
        let letters = drawn(&desk);
        // The board's beat builds this snapshot every second: what it costs.
        let mut took: Vec<u128> = (0..200)
            .map(|_| {
                let from = std::time::Instant::now();
                std::hint::black_box(desk_snapshot(&ledger, |_| false, no_cost));
                from.elapsed().as_micros()
            })
            .collect();
        took.sort_unstable();
        let of_kind = |kind: &str| letters.iter().filter(|one| one["kind"] == kind).count();
        let quiet_rows: u64 = letters
            .iter()
            .filter(|one| one["kind"] == "went_quiet")
            .map(|one| one["notices"].as_u64().unwrap_or(1))
            .sum();
        println!(
            "{}",
            serde_json::json!({
                "at": at,
                "mailToAnswer": desk["counts"]["mail"].as_u64().unwrap_or(desk["mail"].as_array().map_or(0, Vec::len) as u64),
                "news": desk["counts"]["news"],
                "folded": desk["counts"]["folded"],
                "drawn": letters.len(),
                "questions": of_kind("question"),
                "wentQuietLines": of_kind("went_quiet"),
                "wentQuietRows": quiet_rows,
                "workerDied": of_kind("worker_died"),
                "snapshotMicros": { "p50": took[took.len() / 2], "p95": took[took.len() * 95 / 100] },
            })
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
