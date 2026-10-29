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
use zerocode_core::orchestration::evidence::{TaskHandIn, hand_in_of};
use zerocode_core::orchestration::settle::{
    Left, Named, PRESS_TASKS_MAX, awaits_review, left_behind_by,
};
use zerocode_core::orchestration::task_cost::TaskCost;
use zerocode_core::orchestration::{
    Closure, Delivery, Gate, GateStatus, LEDGER_ITSELF, Ledger, Message, MessageKind, NextStep,
    Run, SETTLE_QUIET_DAYS, Task, TaskStatus, WorkProgress, WorkStage, WorktreeRoom,
    board_pass_result, last_touched_ms, worktree_room,
};
use zerocode_core::plain_text::TextLint;

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
    /// The integration batches of those runs (t-45315): every open one, and
    /// one that ended within the news's own window, so its end is seen.
    pub(crate) land_batches: Vec<DeskLandBatch>,
    /// Their work nobody is coming back to (t-71220): every task of each
    /// pile, which a press names whole — the rows above carry at most
    /// [`STAGE_ROWS`] of them a stage.
    pub(crate) left: DeskLeft,
}

/// The piles of work nobody is coming back to, read once
/// ([`zerocode_core::orchestration::settle::left_behind`]): what a 「정리」 closes,
/// what stays open for a look, and what a 「검토 생략」 waives. Oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct DeskLeft {
    pub(crate) settle: Vec<Named>,
    pub(crate) look: Vec<Named>,
    pub(crate) unreviewed: Vec<Named>,
    /// How many tasks one press may name ([`PRESS_TASKS_MAX`]): a longer pile
    /// is cleared that many at a time, oldest first.
    pub(crate) press_max: usize,
    /// The quiet time the piles are read with ([`SETTLE_QUIET_DAYS`]), in days.
    pub(crate) quiet_days: i64,
}

/// One integration batch the board draws a card for: the batch as the
/// ledger holds it, and whether its repository has the queue on — an open
/// batch in a repository that turned it off keeps its cancel alone. `on` is
/// the project's setting, laid over on the beat outside the ledger's view
/// (`land_batch::dress_desk`); the snapshot itself says off.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DeskLandBatch {
    pub(crate) run: String,
    pub(crate) on: bool,
    pub(crate) batch: zerocode_core::orchestration::land_batch::BatchState,
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
    /// Since when a late landing has stood (`landing_stalled`, t-34501).
    pub(crate) since_ms: Option<i64>,
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
    /// A landing check's verdict, in the window's word (`passed`, `merged`, `conflict`, `failed`,
    /// `timed_out`, `unstartable`, `abandoned`, `error`). Only on a `land_check` letter (t-42447).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) verdict: Option<String>,
    /// A landing check's exit code, where its command ran to an end (t-42447).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rc: Option<i64>,
    /// How many files a landing check's merge clashed on (t-42447).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) clashes: Option<u64>,
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
    kinds: [(MessageKind, NewsLine); 9],
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
        (MessageKind::LandingStalled, NewsLine::PerNotice),
        (MessageKind::LandCheck, NewsLine::PerNotice),
        (MessageKind::TurnFailed, NewsLine::PerNotice),
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
    /// (`Run::question_is_answerable`), less the questions the ledger's own
    /// counts no longer hold open (`Ledger::still_owed`): written back about
    /// in a letter naming them (t-59383), or expired — three days unanswered,
    /// or a day after the coordinator's chair that asked emptied with nothing
    /// said from it since (t-59383 r2). The ledger's notices are never owed
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

/// What the coordinator of `run` owes at `now_ms`. The ledger is read for
/// the letters a question's answer may be filed in outside `run` — the
/// asker's own run, when another run's coordinator asked.
pub(crate) fn desk_letters(ledger: &Ledger, run: &Run, now_ms: i64) -> DeskLetters {
    let address = run.address();
    let inbox = InboxState::of(run, &address);
    let mut owed = DeskLetters::default();
    let mut episodes: HashMap<&str, usize> = HashMap::new();
    let mut folded_in_open = false;
    let mut asked = Vec::new();
    for message in run.messages().iter().filter(|one| one.to == address) {
        if zerocode_core::orchestration::gate_reuse::is_gate_evidence(message) {
            continue;
        }
        // An integration batch's records are its card, not lines of news.
        if zerocode_core::orchestration::land_batch::BatchRecord::of(message).is_some() {
            continue;
        }
        if run.awaits_answer(message) {
            if run.question_is_answerable(message).is_ok() {
                asked.push(message);
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
    // The same count the ledger keeps (t-59383): a question the seat wrote
    // back about in a letter naming it, or one that expired, is owed nothing
    // more.
    owed.mail = ledger
        .still_owed(run, asked, now_ms)
        .into_iter()
        .map(|question| mail_row(run, question, &inbox))
        .collect();
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
    // A landing check's evidence is the letter's payload; its body is the one line an agent reads.
    let evidence: serde_json::Value = if message.kind == MessageKind::LandCheck {
        serde_json::from_str(message.payload.as_str()).unwrap_or_default()
    } else {
        serde_json::Value::Null
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
        since_ms: said["sinceMs"].as_i64(),
        category: said["category"].as_str().map(str::to_string),
        routed: said["routed"].as_bool(),
        rung: said["rung"].as_str().map(str::to_string),
        switched_to: said["to"].as_str().map(str::to_string),
        scope: said["scope"].as_str().map(str::to_string),
        verdict: evidence["state"].as_str().map(str::to_string),
        rc: evidence["rc"].as_i64(),
        clashes: evidence["conflicts"]["total"].as_u64(),
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
    /// Which terminal of this window sits in that seat — the pane the sidebar,
    /// the tab strip and the desk call 코디 (t-45938). `None` where this window
    /// holds no live seat of the run.
    pub(crate) term: Option<u32>,
    /// Whether anybody sits in the run's coordinator seat, in this window or
    /// another. A run in play with nobody seated is the empty seat the sidebar
    /// says once, until a coordinator sits again.
    pub(crate) held: bool,
    /// The seat's generation — how many coordinators have sat in this run, the
    /// last one included; 0 where none ever has.
    pub(crate) generation: u32,
    /// When the last holder's pane was proven gone (a leader exit or a window
    /// restart), while nobody has sat since.
    pub(crate) vacated_ms: Option<i64>,
    /// What this run's coordinator owes, counted off the same letters the
    /// desk lists ([`desk_letters`]): questions to answer (the 「답할 우편」)
    /// and notices to acknowledge (the 「소식」) — the badges on the sidebar's
    /// coordinator card (t-56141).
    pub(crate) mail: usize,
    pub(crate) news: usize,
    /// Its tasks verified and not yet merged — the stage the desk calls
    /// `verified` — and the members of its integration batches still open:
    /// the sidebar's landing line (t-56141).
    pub(crate) landing: usize,
    pub(crate) batched: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DeskTask {
    pub(crate) run: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) stage: &'static str,
    /// What is to be done next ([`next_step`], t-73910 W4-1): the window
    /// translates the word, and keeps no table of steps of its own.
    pub(crate) next_step: NextStep,
    /// The task this one was written under (`task-create --parent`) and that
    /// task's name: the goal the board's 「지금 도는 일」 folds its rows under
    /// (t-67607). `None` for a task written at the top.
    pub(crate) parent: Option<String>,
    pub(crate) parent_title: Option<String>,
    /// The decision standing in front of it, oldest first
    /// (`Run::pending_gate_on`).
    pub(crate) gate: Option<DeskGate>,
    /// Its dependencies that failed (`Run::blocked_by`).
    pub(crate) blocked_by: Vec<String>,
    /// Why a closed task is closed — folded into a task, handed to a run, or
    /// outdated with its line. `None` for every task that is not closed.
    pub(crate) closed: Option<Closure>,
    pub(crate) created_ms: i64,
    /// What the task cost, for a finished one ([`finished`], t-9470) —
    /// `None` while it is still moving.
    pub(crate) cost: Option<TaskCost>,
    /// What the writing lint counted in the summary the task's own worker
    /// handed in, for a finished task whose worker wrote one (t-32786,
    /// [`super::writing_book`]). The board shows the counts beside the row;
    /// nothing here refuses a report.
    pub(crate) writing: Option<TextLint>,
    /// What was kept of what this task's workers handed in (t-32798) — laid on by
    /// [`super::hand_in_keep::dress`] from the artifact store's book; `None` for a
    /// task nobody handed anything in on by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) kept: Option<zerocode_core::hand_in::Facts>,
    /// The pass conditions the task's spec writes, each with the state its last
    /// report left it in, and the notes that report carried (t-26587,
    /// [`zerocode_core::orchestration::evidence::hand_in_of`]). Empty for a
    /// task that writes no conditions and whose report left no notes.
    #[serde(skip_serializing_if = "TaskHandIn::is_empty")]
    pub(crate) hand_in: TaskHandIn,
    /// Where the task's work stands, as the ledger reads it once (t-47538, [`Run::progress_of`]): the
    /// sources a review named and a command to read or check the work with. Asked of the rows the desk
    /// sends, and read once for a completed task, whose stage it decides.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) progress: Option<WorkProgress>,
    /// Whether the commit its work landed as (`progress.merge_head`) is on
    /// the remote's default branch, as the beat last asked git (t-73910 W4-2,
    /// [`crate::remote_landing`]); `None` until it was asked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) on_remote: Option<crate::remote_landing::OnRemote>,
    /// When its work moved (t-67482): the task board's schedule draws the row
    /// on the time axis from these. Asked of the rows the desk sends; `None`
    /// for a task nobody has carried yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) times: Option<TaskTimes>,
    /// The pile it is left in when nobody is coming back to it (t-71220,
    /// [`left_behind`]); `None` for a task somebody is on, or over.
    pub(crate) left: Option<Left>,
    /// When it was last touched — written, or an attempt begun or ended: what
    /// a pile orders by and the age a row says.
    pub(crate) touched_ms: i64,
}

/// When a task's work moved, as the ledger keeps it: its first attempt's
/// start, the newest attempt that ended handing its work in (`worker_done`),
/// the coordinator's first `verified` of the review that stands, its
/// landing — the completion the coordinator wrote over a verified, merged
/// review ([`Run::completion_ms`]) — and the newest end of any attempt. Each
/// is `None` until it happened. Beside them the release the coordinator wrote
/// the work shipped in: a version, for when that release was cut is git's to
/// say (t-71840).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct TaskTimes {
    pub(crate) started_ms: Option<i64>,
    pub(crate) handed_ms: Option<i64>,
    pub(crate) verified_ms: Option<i64>,
    pub(crate) landed_ms: Option<i64>,
    /// Handed in, failed or stopped: where a finished task's bar ends when
    /// nothing later was written of it — a failed one's, above all (t-71840).
    pub(crate) ended_ms: Option<i64>,
    /// [`crate::artifact_flow::ledger_release`], read only of a review that
    /// says deployed.
    pub(crate) release: Option<String>,
}

/// A task's times, read off its attempts and its review — the readings the
/// cost (`task_cost::attempts`) and the stage (`Run::review_of`) already use.
fn task_times(run: &Run, task: &Task) -> Option<TaskTimes> {
    let attempts = zerocode_core::orchestration::task_cost::attempts(run, &task.id);
    let started_ms = attempts.iter().map(|attempt| attempt.started_ms).min()?;
    let review = run.review_of(task);
    Some(TaskTimes {
        started_ms: Some(started_ms),
        handed_ms: attempts
            .iter()
            .filter(|attempt| attempt.succeeded == Some(true))
            .filter_map(|attempt| attempt.ended_ms)
            .max(),
        verified_ms: review.verified_ms.filter(|_| review.verified),
        landed_ms: run.completion_ms(task),
        ended_ms: attempts.iter().filter_map(|attempt| attempt.ended_ms).max(),
        release: review
            .deployed
            .then(|| crate::artifact_flow::ledger_release(task))
            .flatten(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DeskGate {
    pub(crate) id: String,
    /// The decision in the asker's words — for a hold, the ledger's own
    /// English sentence, which the board keeps behind 「원문 보기」.
    pub(crate) question: String,
    /// What kind of decision this is, so the board can say it in the
    /// person's language (t-67607) rather than print the ledger's sentence:
    /// `asked` — somebody put it in front of the task, in their own words;
    /// `unexamined` — the ledger holds the task for a worker that exited
    /// before reporting, and nobody has looked at its checkout yet; or what
    /// the window found once it looked (`unlanded`, `uncommitted`,
    /// `unknown` — `zerocode_core::orchestration::Examined`).
    pub(crate) kind: &'static str,
    /// The answers its asker offered (`gate-create --options`), as advice:
    /// the board draws each as a button beside a free answer.
    pub(crate) options: Vec<String>,
    /// The worker the ledger holds the task for, and that worker's checkout.
    pub(crate) worker: Option<String>,
    pub(crate) checkout: Option<String>,
    /// The window's reading of that checkout as the ledger mailed it
    /// (`checkoutExamined`): the branch, base and commits left unlanded, the
    /// changes left uncommitted, or why it could not tell.
    pub(crate) examined: Option<serde_json::Value>,
}

/// The two answers a hold takes from the board (t-67607), in the ledger's own
/// words: 거두기 lands what the worker left and frees the task, 다시 띄우기
/// hands the task out afresh. The words are the core's, never a copy.
pub(crate) const HOLD_ACTS: [(&str, &str); 2] = [
    ("harvest", zerocode_core::orchestration::HARVEST),
    ("dispatch", zerocode_core::orchestration::DISPATCH_AGAIN),
];

/// The window's last reading of each held checkout, by the gate that holds
/// it (t-67607). The ledger mails that reading to the run's home in its own
/// voice — a status carrying `checkoutExamined` beside `gateId`
/// (`Ledger::checkout_examined`) — and rewrites the gate's question as prose,
/// so the letter is the one structured copy. Read only for a run with a hold
/// standing; letters are oldest first, so the newest reading wins.
fn examined_holds(run: &Run) -> HashMap<String, serde_json::Value> {
    let mut held = HashMap::new();
    let holding = run
        .gates
        .iter()
        .any(|gate| gate.status == GateStatus::Pending && gate.held_for.is_some());
    if !holding {
        return held;
    }
    for message in run.messages() {
        if message.kind != MessageKind::Status || message.from != LEDGER_ITSELF {
            continue;
        }
        let body = message.body.as_str();
        if !body.contains("\"checkoutExamined\"") {
            continue;
        }
        let Ok(said) = serde_json::from_str::<serde_json::Value>(body) else {
            continue;
        };
        if let (Some(gate), Some(examined)) =
            (said["gateId"].as_str(), said.get("checkoutExamined"))
        {
            held.insert(gate.to_string(), examined.clone());
        }
    }
    held
}

/// One standing decision as the board draws it: its kind, and for a hold the
/// worker, its checkout and what the window found there.
fn desk_gate(run: &Run, gate: &Gate, examined: &HashMap<String, serde_json::Value>) -> DeskGate {
    let reading = gate
        .held_for
        .as_ref()
        .and_then(|_| examined.get(&gate.id))
        .cloned();
    let kind = match (
        gate.held_for.as_ref(),
        reading.as_ref().and_then(|said| said["kind"].as_str()),
    ) {
        (None, _) => "asked",
        (Some(_), None) => "unexamined",
        (Some(_), Some("unlanded")) => "unlanded",
        (Some(_), Some("uncommitted")) => "uncommitted",
        (Some(_), Some(_)) => "unknown",
    };
    DeskGate {
        id: gate.id.clone(),
        question: gate.question.as_str().to_string(),
        kind,
        options: gate
            .options
            .iter()
            .map(|option| option.as_str().to_string())
            .collect(),
        checkout: gate
            .held_for
            .as_deref()
            .and_then(|worker| run.worker(worker))
            .and_then(|seat| seat.checkout.clone()),
        worker: gate.held_for.clone(),
        examined: reading,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct StageCount {
    pub(crate) stage: &'static str,
    pub(crate) count: usize,
    /// How many of them nobody is coming back to (t-71220): counted in
    /// `count`, and not anybody's turn — what the board's 「답할 일」 leaves out.
    pub(crate) left: usize,
}

/// The pipeline's stages in the order the board draws them — a task's one
/// word. The flow reads left to right (written down with its dependencies
/// unmet, ready, carried, reported, verified, merged, deployed); the ones after
/// it are where a task stands still or is over: a decision in front of it, held
/// back, a review that names an older attempt or commit than the work handed in
/// (t-47538), failed, completed with nothing a coordinator could ever review
/// (`ReviewFacts::unreviewable`), completed with the review the person waived
/// (`ReviewFacts::waived`, t-71220), verified with nothing to land, completed on
/// what the ledger cannot read as any of these (`WorkStage::Unknown`), closed
/// (over without being done or failed — `TaskStatus::Closed`).
pub(crate) const STAGES: [&str; 16] = [
    "pending",
    "ready",
    "dispatched",
    "reported",
    "verified",
    "merged",
    "deployed",
    "gate",
    "blocked",
    "stale",
    "failed",
    "unreviewable",
    "waived",
    "nothing_to_land",
    "unknown",
    "closed",
];

/// The stages a task is still moving through: listed oldest first, the order
/// the ledger hands work out in. The rest are endings, listed newest first.
const OPEN_STAGES: [&str; 5] = ["pending", "ready", "dispatched", "gate", "blocked"];

/// The stages a completed task stands in — every one [`completed_stage`]
/// answers — the ones whose rows carry the task's cost (t-9470).
pub(super) const FINISHED_STAGES: [&str; 9] = [
    "reported",
    "verified",
    "merged",
    "deployed",
    "stale",
    "unreviewable",
    "waived",
    "nothing_to_land",
    "unknown",
];

/// The most rows one stage carries across the wire. Its count carries the
/// rest: a run of two hundred finished tasks is two hundred numbers nobody
/// reads one by one, and the newest two dozen are what a coordinator checks.
pub(crate) const STAGE_ROWS: usize = 24;

/// A task's one stage word, and — for a completed task, whose stage it decides —
/// where its work stands. A standing decision outranks everything — the task
/// will not move until it is answered — and a task waiting on a dependency that
/// failed is held back, not merely pending: it will never become ready by
/// itself. Those, and a task carried, failed or closed, are the task's own
/// status. Where a completed task's work stands is the ledger's one reading
/// (`Run::progress_of`, t-47538), never a second one here: a worker's report
/// alone is "reported", which is the board's 검증 대기; verified, merged and
/// deployed only on a coordinator's review of the newest attempt and the source
/// it handed in; work nothing was handed in on is its own stage (t-19328).
fn staged(run: &Run, task: &Task) -> (&'static str, Option<WorkProgress>) {
    if run.pending_gate_on(&task.id).is_some() {
        return ("gate", None);
    }
    let stage = match task.status {
        TaskStatus::Pending if run.blocked_by(task).is_empty() => "pending",
        TaskStatus::Pending | TaskStatus::Blocked => "blocked",
        TaskStatus::Ready => "ready",
        TaskStatus::Dispatched => "dispatched",
        TaskStatus::Completed => {
            let progress = run.progress_of(task);
            return (completed_stage(progress.stage), Some(progress));
        }
        TaskStatus::Failed => "failed",
        TaskStatus::Closed => "closed",
    };
    (stage, None)
}

/// A completed task's stage, out of where its work stands. Completed is the
/// coordinator's word that the task is done, so a newest attempt still open or
/// ended without a report does not move it back into 진행 or 실패: the ledger
/// cannot say where the work stands, and the desk says so.
const fn completed_stage(work: WorkStage) -> &'static str {
    match work {
        WorkStage::ReportOnly => "reported",
        WorkStage::Verified => "verified",
        WorkStage::Merged => "merged",
        WorkStage::Deployed => "deployed",
        WorkStage::Stale => "stale",
        WorkStage::Unreviewable => "unreviewable",
        WorkStage::Waived => "waived",
        WorkStage::NothingToLand => "nothing_to_land",
        WorkStage::Closed => "closed",
        WorkStage::Working
        | WorkStage::Failed
        | WorkStage::Stopped
        | WorkStage::Superseded
        | WorkStage::Unknown => "unknown",
    }
}

/// Whether a stage word says finished work waits on a review — the desk's word
/// for [`awaits_review`], read through the one mapping of a work stage to its
/// word ([`completed_stage`]), so the flow can ask it of the stage a line read
/// (t-71220).
pub(crate) fn word_awaits_review(word: &str) -> bool {
    WorkStage::ALL
        .into_iter()
        .any(|stage| awaits_review(stage) && completed_stage(stage) == word)
}

/// What comes next for a task at a desk stage: the work's own step
/// ([`WorkStage::next_step`]) where its stage is where completed work stands,
/// and for the task's own status otherwise. A completed task the ledger
/// cannot read (`unknown`) asks for nothing it would have to guess.
pub(crate) fn next_step(stage: &str, progress: Option<&WorkProgress>) -> NextStep {
    match stage {
        "pending" => NextStep::AwaitDependencies,
        "ready" => NextStep::Summon,
        "dispatched" => NextStep::Wait,
        "gate" => NextStep::Decide,
        "blocked" => NextStep::MendDependency,
        "failed" => WorkStage::Failed.next_step(),
        "closed" => WorkStage::Closed.next_step(),
        word => progress
            .map(|progress| progress.stage)
            .filter(|work| completed_stage(*work) == word && word != "unknown")
            .map_or(NextStep::Unknown, WorkStage::next_step),
    }
}

/// A task's one stage word ([`STAGES`]) — the reading the desk, the board and
/// the artifact tab's flow (t-68458) all draw, so no surface keeps a second one.
pub(crate) fn stage_of(run: &Run, task: &Task) -> &'static str {
    staged(run, task).0
}

/// Whether a task is finished: completed, in any of the stages that reads as
/// ([`FINISHED_STAGES`]).
pub(crate) fn finished(run: &Run, task: &Task) -> bool {
    FINISHED_STAGES.contains(&stage_of(run, task))
}

/// Whether a run is in play: somebody is coordinating it, or somebody is
/// still working for it. A finished run with nobody at it is history — on the
/// desk, and for the board's rows of finished work (`ledger_agents_for_seats`).
pub(crate) fn in_play(run: &Run) -> bool {
    run.coordinator_live().is_some()
        || run
            .workers
            .iter()
            .any(|worker| worker.state.still_summoned())
}

/// The desk's reading of `ledger`. `seat_term` answers which terminal of this
/// window holds a coordinator seat named `team/pane`, `None` where none does;
/// `cost` what a finished task cost, asked only of the rows the desk carries.
pub(crate) fn desk_snapshot(
    ledger: &Ledger,
    seat_term: impl Fn(&str) -> Option<u32>,
    mut cost: impl FnMut(&Run, &Task) -> TaskCost,
) -> DeskSnapshot {
    let now_ms = crate::now_epoch_ms();
    let mut runs = Vec::new();
    let mut staged_tasks: Vec<(&Run, &Task, DeskTask)> = Vec::new();
    let mut mail = Vec::new();
    let mut news = Vec::new();
    let mut folded = 0;
    let mut folded_batches = Vec::new();
    let mut land_batches = Vec::new();
    for run in ledger.runs().iter().filter(|run| in_play(run)) {
        let owed = desk_letters(ledger, run, now_ms);
        let (run_mail, run_news) = (owed.mail.len(), owed.news.len());
        mail.extend(owed.mail);
        news.extend(owed.news);
        folded += owed.folded;
        folded_batches.extend(owed.folded_batch);
        let batches_before = land_batches.len();
        land_batches.extend(
            zerocode_core::orchestration::land_batch::batches(run)
                .into_iter()
                .filter(|batch| {
                    !batch.ended() || now_ms.saturating_sub(batch.last_at) < DESK_NEWS.stands_ms
                })
                .map(|batch| DeskLandBatch {
                    run: run.id.clone(),
                    on: false,
                    batch,
                }),
        );
        let batched = land_batches[batches_before..]
            .iter()
            .filter(|held| !held.batch.ended())
            .map(|held| held.batch.members.len())
            .sum::<usize>();
        // The seat as the ledger keeps it: the live one names its pane, an
        // empty one keeps its generation and when its holder was proven gone.
        let chair = run.coordinator.as_ref();
        let live = run.coordinator_live();
        let term = live.and_then(|held| seat_term(&held.seat));
        let tasks_before = staged_tasks.len();
        let examined = examined_holds(run);
        // Each task's stage is read once: who is coming back to finished work
        // is read off the progress its stage was read from (t-71220).
        let read: Vec<(&Task, (&'static str, Option<WorkProgress>))> = run
            .tasks
            .iter()
            .map(|task| (task, staged(run, task)))
            .collect();
        let awaiting: HashSet<&str> = read
            .iter()
            .filter(|(_, (_, progress))| {
                progress
                    .as_ref()
                    .is_some_and(|progress| awaits_review(progress.stage))
            })
            .map(|(task, _)| task.id.as_str())
            .collect();
        let left = left_behind_by(run, now_ms, |task| awaiting.contains(task.id.as_str()));
        for (task, (stage, progress)) in read {
            let next_step = next_step(stage, progress.as_ref());
            staged_tasks.push((
                run,
                task,
                DeskTask {
                    run: run.id.clone(),
                    id: task.id.clone(),
                    title: task.display_name().to_string(),
                    stage,
                    next_step,
                    parent: task.parent.clone(),
                    parent_title: task
                        .parent
                        .as_deref()
                        .and_then(|parent| run.task(parent))
                        .map(|parent| parent.display_name().to_string()),
                    progress,
                    on_remote: None,
                    gate: run
                        .pending_gate_on(&task.id)
                        .map(|gate| desk_gate(run, gate, &examined)),
                    blocked_by: run.blocked_by(task),
                    closed: task.closed.clone(),
                    created_ms: task.created_ms,
                    cost: None,
                    writing: None,
                    kept: None,
                    hand_in: TaskHandIn::default(),
                    times: None,
                    left: left.get(task.id.as_str()).copied(),
                    touched_ms: last_touched_ms(run, task),
                },
            ));
        }
        runs.push(DeskRun {
            run: run.id.clone(),
            name: run.name.clone(),
            seat: term.is_some(),
            term,
            held: live.is_some(),
            generation: chair.map_or(0, |held| held.generation),
            vacated_ms: chair.and_then(|held| held.vacated_ms),
            mail: run_mail,
            news: run_news,
            landing: staged_tasks[tasks_before..]
                .iter()
                .filter(|(_, _, row)| row.stage == "verified")
                .count(),
            batched,
        });
    }
    let mut stages = Vec::with_capacity(STAGES.len());
    let mut tasks = Vec::new();
    for stage in STAGES {
        let (mut rows, mut behind): (Vec<_>, Vec<_>) = staged_tasks
            .iter()
            .filter(|(_, _, row)| row.stage == stage)
            .partition(|(_, _, row)| row.left.is_none());
        stages.push(StageCount {
            stage,
            count: rows.len() + behind.len(),
            left: behind.len(),
        });
        if OPEN_STAGES.contains(&stage) {
            rows.sort_by_key(|(_, _, row)| row.created_ms);
        } else {
            rows.sort_by_key(|(_, _, row)| std::cmp::Reverse(row.created_ms));
        }
        // Work nobody is coming back to has rows of its own, oldest first, so
        // a pile of it never takes the rows of the work somebody is on.
        behind.sort_by_key(|(_, _, row)| row.touched_ms);
        rows.truncate(STAGE_ROWS);
        behind.truncate(STAGE_ROWS);
        rows.extend(behind);
        // A finished task's cost, asked of the rows the desk sends and no
        // other — a stage of two hundred is two dozen rows and a count.
        let finished = FINISHED_STAGES.contains(&stage);
        // The hand-in view is read from the ledger's own message, so it is asked
        // of the rows the desk sends too — the same rule as the cost above.
        tasks.extend(rows.into_iter().map(|(run, task, row)| DeskTask {
            cost: finished.then(|| cost(run, task)),
            hand_in: hand_in_of(run, task),
            progress: row.progress.clone().or_else(|| Some(run.progress_of(task))),
            times: task_times(run, task),
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
        land_batches,
        left: desk_left(&staged_tasks),
    }
}

/// Every task of the desk's runs left behind, by pile and oldest first — what
/// a press names whole.
fn desk_left(staged: &[(&Run, &Task, DeskTask)]) -> DeskLeft {
    let mut behind: Vec<&DeskTask> = staged
        .iter()
        .map(|(_, _, row)| row)
        .filter(|row| row.left.is_some())
        .collect();
    behind.sort_by_key(|row| row.touched_ms);
    let mut left = DeskLeft {
        press_max: PRESS_TASKS_MAX,
        quiet_days: SETTLE_QUIET_DAYS,
        ..DeskLeft::default()
    };
    for row in behind {
        let pile = match row.left {
            Some(Left::Settle) => &mut left.settle,
            Some(Left::Look) => &mut left.look,
            Some(Left::Unreviewed) => &mut left.unreviewed,
            None => continue,
        };
        pile.push(Named {
            run: row.run.clone(),
            task: row.id.clone(),
            was: None,
        });
    }
    left
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
    let seat = with_cached_run(run_id, |run| {
        run.and_then(Run::coordinator_live)
            .map(|seat| seat.seat.clone())
    })?
    .ok_or("the run has no live coordinator seat")?;
    let (team, pane) = seat
        .split_once('/')
        .ok_or("the coordinator seat names no pane")?;
    let capability = crate::agent_teams::current_pane_capability(team, pane)
        .ok_or("this window does not hold the run's coordinator seat")?;
    super::coordinator_handover::command(team, pane, capability, argv, now_ms)
}

/// `run_id` as the cached ledger has it — the image the desk itself draws
/// from — handed to `read`, `None` for a run the ledger does not know.
fn with_cached_run<T>(run_id: &str, read: impl FnOnce(Option<&Run>) -> T) -> Result<T, String> {
    let held = super::runtime().ok_or("orchestration runtime unavailable")?;
    let image = held.actor.view().map_err(|error| format!("{error:?}"))?;
    let ledger = super::cached_ledger(&held, &image).map_err(|error| format!("{error:?}"))?;
    Ok(read(ledger.run(run_id)))
}

/// Whether a decision takes the person's own words (review t-68962 P3 ⑤):
/// one somebody put in front of a task does — its resolution is free text by
/// design — and a hold the ledger opened (`held_for`) does not. That one
/// takes one of the ledger's two answers ([`HOLD_ACTS`]), the rule the board
/// keeps by never offering 직접 답하기 there; a hand-made call is held to it
/// here. A gate the run does not hold is left to the ledger, which refuses
/// what it cannot find.
fn hold_takes_words(run: &Run, gate: &str) -> Result<(), String> {
    match run.gates.iter().find(|one| one.id == gate) {
        Some(one) if one.held_for.is_some() => {
            Err("a hold the ledger opened takes harvest or dispatch, not free words".into())
        }
        _ => Ok(()),
    }
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

/// Close a question put to `run_id`'s coordinator without an answer, as that
/// seat (t-83281) — the ledger's `question-close`, whose rules stand whole:
/// only the seat asked or the seat that asked closes it, the reason is the
/// record it leaves, and a question closed already answers with the standing
/// close (`already`) and writes nothing.
pub(crate) fn close_question(
    run_id: &str,
    message: &str,
    reason: &str,
    retry_request: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if retry_request.trim().is_empty() {
        return Err("retryRequest is required".into());
    }
    if reason.trim().is_empty() {
        return Err("a close without an answer needs its reason".into());
    }
    as_the_coordinator(
        run_id,
        [
            "question-close",
            "--run",
            run_id,
            "--message",
            message,
            "--reason",
            reason,
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

/// Answer a decision from the board (t-67607), as `run_id`'s coordinator seat
/// — the ledger's `gate-resolve`. A hold the ledger opened takes one of the
/// two answers the ledger itself offers ([`HOLD_ACTS`]); a decision somebody
/// put in front of a task takes the person's own words (`answer`), since its
/// resolution is free text by design — and only such a decision does
/// ([`hold_takes_words`]). Any other act is refused here rather than sent.
/// Only on the person's press; the answer the ledger gives back is what the
/// row shows.
pub(crate) fn resolve_hold(
    run_id: &str,
    gate: &str,
    act: &str,
    answer: Option<&str>,
    retry_request: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if retry_request.trim().is_empty() {
        return Err("retryRequest is required".into());
    }
    let resolution = if act == "answer" {
        answer
            .map(str::trim)
            .filter(|words| !words.is_empty())
            .ok_or("an answer needs words")?
    } else {
        HOLD_ACTS
            .iter()
            .find(|(name, _)| *name == act)
            .map(|(_, words)| *words)
            .ok_or_else(|| format!("a hold takes harvest or dispatch, not {act:?}"))?
    };
    if act == "answer" {
        with_cached_run(run_id, |run| {
            run.map_or(Ok(()), |run| hold_takes_words(run, gate))
        })??;
    }
    as_the_coordinator(
        run_id,
        [
            "gate-resolve",
            "--run",
            run_id,
            "--gate",
            gate,
            "--resolution",
            resolution,
            "--retry-request",
            retry_request,
        ]
        .map(str::to_string)
        .to_vec(),
        now_ms,
    )
}

/// Pass a reported task from the board (t-67607, 「통과」 on a 검토 row), as
/// `run_id`'s coordinator seat — the ledger's `task-update` with a verified
/// review that names the attempt and the source the board drew. The ledger
/// refuses the pass when either has moved since, which is what keeps a press
/// from passing work that changed after the person read it — and when the
/// coordinator withdrew its verification of that attempt and source, since
/// the board's pass names no new review (t-81913). A withdrawn row draws no
/// pass at all; the ledger's refusal is what a press that slips through is
/// answered with.
pub(crate) fn pass_review(
    run_id: &str,
    task: &str,
    attempt: &str,
    source: &str,
    retry_request: &str,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    if retry_request.trim().is_empty() {
        return Err("retryRequest is required".into());
    }
    if attempt.trim().is_empty() || source.trim().is_empty() {
        return Err("a pass names the attempt and the source it read".into());
    }
    let argv = with_cached_run(run_id, |run| {
        pass_argv(run, run_id, task, attempt, source, retry_request, now_ms)
    })??;
    as_the_coordinator(run_id, argv, now_ms)
}

/// The board's pass as a `task-update` line (t-82501): the result the task
/// holds now with the pass added ([`board_pass_result`]) — `task-update`
/// replaces a result whole, so a pass that sent its one key would take the
/// worker's summary, tests and report with it. Read from the image the board
/// drew from; a task that image does not hold is not written over blind.
fn pass_argv(
    run: Option<&Run>,
    run_id: &str,
    task: &str,
    attempt: &str,
    source: &str,
    retry_request: &str,
    now_ms: i64,
) -> Result<Vec<String>, String> {
    let held = run
        .and_then(|run| run.task(task))
        .ok_or_else(|| format!("the board holds no task {task} in {run_id}"))?;
    let result = board_pass_result(held.result.as_str(), now_ms);
    Ok([
        "task-update",
        "--run",
        run_id,
        "--task",
        task,
        "--result",
        &result,
        "--attempt",
        attempt,
        "--source",
        source,
        "--retry-request",
        retry_request,
    ]
    .map(str::to_string)
    .to_vec())
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

#[cfg(any(unix, test))]
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
        AckedRow, Draft, Ending, Priority, ResultAuthor, TaskStatus, Text, worker_address,
    };

    const HOUR_MS: i64 = 60 * 60 * 1000;
    /// The spacing of the repeats below: the five-minute cadence a ledger
    /// written before t-15313 told one silence again at. The ledger tells
    /// an episode once now ([`told_again`] says why the desk still folds).
    const REMINDED_MS: i64 = 300_000;

    /// A cost nobody asked about — for the tests that read the rest.
    fn no_cost(_: &Run, _: &Task) -> TaskCost {
        TaskCost::default()
    }

    /// The desk as the window serializes it. The red tests below read the
    /// JSON the screen reads, so they speak to any shape of the snapshot.
    fn desk_json(ledger: &Ledger) -> serde_json::Value {
        serde_json::to_value(desk_snapshot(ledger, |_| None, no_cost)).expect("the desk serializes")
    }

    /// The number on `run_id`'s coordinator card (the 「답할 우편」 it owes)
    /// and the desk's count of the same over every run in play.
    fn mail_counts(ledger: &Ledger, run_id: &str) -> (serde_json::Value, serde_json::Value) {
        let desk = desk_json(ledger);
        let row = desk["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|row| row["run"] == run_id))
            .cloned()
            .unwrap_or_else(|| panic!("the run has no card: {desk}"));
        (row["mail"].clone(), desk["counts"]["mail"].clone())
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

    /// The silence of `worker`'s open attempt, told for the `beat`-th time
    /// at `at`: the stall sweep's own notice first, then repeats in the
    /// ledger's voice.
    ///
    /// The ledger tells one episode once since t-15313, but the desk still
    /// meets repeats: a store written before then carries a notice every
    /// five minutes of every silence it saw, and the stall seat's reading
    /// is a second notice about the same silence. So the folding is fed its
    /// repeats by hand, in the shape those rows have.
    fn told_again(ledger: &mut Ledger, run_id: &str, worker: &str, beat: i64, at: i64) {
        if beat == 0 {
            assert_eq!(
                ledger.workers_stalled(&[(worker.to_string(), at - 200_000)], at),
                1,
                "the stall told nobody"
            );
            return;
        }
        let run = ledger.run(run_id).expect("the run");
        let dispatch = run
            .worker(worker)
            .and_then(|held| held.dispatch.clone())
            .expect("an open attempt");
        let task = run
            .dispatch(&dispatch)
            .map(|held| held.task.clone())
            .expect("the attempt's task");
        let body = serde_json::json!({
            "workerId": worker,
            "reason": "stalled",
            "stalledSinceMs": at - 200_000,
            "observedAtMs": at,
            "notification": true,
        });
        ledger
            .post(
                run_id,
                Draft {
                    from: zerocode_core::orchestration::LEDGER_ITSELF.to_string(),
                    to: format!("run:{run_id}"),
                    kind: MessageKind::WentQuiet,
                    body: Text::from(body.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(task),
                    dispatch: Some(dispatch),
                },
                at,
            )
            .expect("a repeat in the ledger's voice");
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
    /// a worker's silence was told again every five minutes while its attempt
    /// was open (once per episode since t-15313; the rows stay in old
    /// stores), and every one of those notices stood as a letter to answer
    /// long after the worker had died. Once the attempt ends the silence is
    /// over — the ledger writes nothing more about it, and the desk draws
    /// none of it; the death itself is the news, once.
    #[test]
    fn a_silence_that_is_over_leaves_the_desk_and_nothing_is_written_after_it() {
        let start = crate::now_epoch_ms() - 3 * HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, worker) = a_worker_carrying_a_task(&mut ledger, start);
        for beat in 0..3 {
            told_again(
                &mut ledger,
                &run_id,
                &worker,
                beat,
                start + 200_000 + beat * REMINDED_MS,
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
            told_again(
                &mut ledger,
                &run_id,
                &worker,
                beat,
                start + 200_000 + beat * REMINDED_MS,
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
        // The run's own row counts the same letters, for the sidebar's coordinator card (t-56141).
        assert_eq!(
            (&desk["runs"][0]["mail"], &desk["runs"][0]["news"]),
            (&serde_json::json!(1), &serde_json::json!(5)),
            "the run's counts are not the desk's: {desk}"
        );
    }

    /// The coordinator card counts what the ledger counts (t-59383): a
    /// question the seat wrote back about, in a letter naming it, is owed
    /// nothing more — the letter filed in this run to the asker's pane, or
    /// in the asker's own run to its chair. Three questions from another
    /// run's coordinator, two letters naming one each, one letter naming
    /// none: the card goes from 3 to 1.
    #[test]
    fn a_question_the_seat_wrote_back_about_leaves_the_card() {
        let start = crate::now_epoch_ms() - HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, worker) = a_worker_carrying_a_task(&mut ledger, start);
        let asking = ledger.create_run("asker", start + 4);
        let (here, there) = (format!("run:{run_id}"), format!("run:{asking}"));
        // The panes the two chairs sit in: this run's, and the asking run's.
        let (seat_here, seat_there) = ("team-here/%1", "team-there/%1");
        let (from_here, asker) = (format!("pane:{seat_here}"), format!("pane:{seat_there}"));
        let letter = |from: &str, to: &str, kind: MessageKind, body: &str| Draft {
            from: from.to_string(),
            to: to.to_string(),
            kind,
            body: Text::from(body),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: Text::default(),
            thread: None,
            task: None,
            dispatch: None,
        };
        let signed = |ledger: &mut Ledger, run: &str, seat: &str, at: i64, draft: Draft| {
            ledger
                .post_as(run, draft, Some(seat), start + at)
                .expect("posted")
        };
        // Each chair signs once as its run: how a run remembers who sat in it.
        let go = letter(&here, &worker_address(&worker), MessageKind::Status, "go");
        signed(&mut ledger, &run_id, seat_here, 5, go);
        let on_duty = letter(&there, &there, MessageKind::Status, "on duty");
        signed(&mut ledger, &asking, seat_there, 6, on_duty);
        let asked: Vec<String> = (7..10)
            .map(|at| {
                let question = letter(&asker, &here, MessageKind::Question, "which?");
                signed(&mut ledger, &run_id, seat_there, at, question)
            })
            .collect();
        assert_eq!(
            mail_counts(&ledger, &run_id),
            (serde_json::json!(3), serde_json::json!(3))
        );

        let filed_there = format!("{} 답: 네", asked[0]);
        let filed_here = format!("{}에 답: 아니요", asked[1]);
        let writes = [
            (
                &asking,
                letter(&from_here, &there, MessageKind::Status, &filed_there),
            ),
            (
                &run_id,
                letter(&here, &asker, MessageKind::Status, &filed_here),
            ),
            (
                &asking,
                letter(&from_here, &there, MessageKind::Status, "셋째는 내일"),
            ),
        ];
        for (at, (run, draft)) in (10..).zip(writes) {
            signed(&mut ledger, run, seat_here, at, draft);
        }
        assert_eq!(
            mail_counts(&ledger, &run_id),
            (serde_json::json!(1), serde_json::json!(1)),
            "a question written back about is still on the card"
        );
    }

    /// t-59383 r2 (A9, A10): the card and the desk count what the ledger
    /// counts once nobody waits on a question. The pile in small: this run's
    /// chair asked by two coordinators of other runs — one whose chair is
    /// held, one whose chair emptied 25 hours ago — three questions each, 4
    /// and 15 days old and one younger (2 hours for the held chair, 30 for the
    /// emptied one, asked before it emptied); and its own worker's question,
    /// 4 days old, riding an attempt still open. Seven on the card before;
    /// two after — the held chair's two-hour question and the worker's.
    #[test]
    fn questions_nobody_waits_on_leave_the_card_and_the_desk() {
        let now = crate::now_epoch_ms();
        let day = 24 * HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, worker) = a_worker_carrying_a_task(&mut ledger, now - 16 * day);
        for (name, seat) in [
            ("held-chair", "team-held/%1"),
            ("emptied-chair", "team-gone/%1"),
        ] {
            let chaired = ledger.create_run(name, now - 16 * day);
            ledger
                .seat_coordinator(&chaired, seat, None, now - 16 * day)
                .expect("the chair");
        }
        let asked = |from: &str| Draft {
            from: from.to_string(),
            to: format!("run:{run_id}"),
            kind: MessageKind::Question,
            body: Text::from("which?"),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: Text::default(),
            thread: None,
            task: None,
            dispatch: None,
        };
        let attempt = ledger
            .run(&run_id)
            .and_then(|run| run.worker(&worker))
            .and_then(|held| held.dispatch.clone())
            .expect("an open attempt");
        let questions = [
            (asked("pane:team-held/%1"), 2 * HOUR_MS),
            (asked("pane:team-held/%1"), 4 * day),
            (asked("pane:team-held/%1"), 15 * day),
            (asked("pane:team-gone/%1"), 30 * HOUR_MS),
            (asked("pane:team-gone/%1"), 4 * day),
            (asked("pane:team-gone/%1"), 15 * day),
            (
                Draft {
                    dispatch: Some(attempt),
                    ..asked(&worker_address(&worker))
                },
                4 * day,
            ),
        ];
        for (question, age) in questions {
            ledger.post(&run_id, question, now - age).expect("asked");
        }
        ledger.team_dissolved("team-gone", now - 25 * HOUR_MS);
        assert_eq!(
            mail_counts(&ledger, &run_id),
            (serde_json::json!(2), serde_json::json!(2)),
            "questions nobody waits on are still on the card"
        );
    }

    /// t-59383 r2 (A7, A8) at the edges, on the desk's own clock: a
    /// stranger's question is owed at 71 hours and not at 73; a question
    /// from a chair that emptied after it is owed 23 hours after the chair
    /// emptied and not at 25; a held chair's question is owed until its own
    /// three days are up.
    #[test]
    fn the_desk_owes_a_question_until_three_days_or_a_day_after_its_askers_chair_emptied() {
        let mut ledger = Ledger::new();
        let (run_id, _) = a_worker_carrying_a_task(&mut ledger, 1_000);
        for (name, seat) in [
            ("held-chair", "team-held/%1"),
            ("emptied-chair", "team-gone/%1"),
        ] {
            let chaired = ledger.create_run(name, 1_000);
            ledger
                .seat_coordinator(&chaired, seat, None, 1_000)
                .expect("the chair");
        }
        let asked_at = 10_000;
        let ask = |ledger: &mut Ledger, from: &str| {
            let question = Draft {
                from: from.to_string(),
                to: format!("run:{run_id}"),
                kind: MessageKind::Question,
                body: Text::from("which?"),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::default(),
                thread: None,
                task: None,
                dispatch: None,
            };
            ledger.post(&run_id, question, asked_at).expect("asked")
        };
        let stranger = ask(&mut ledger, "pane:team-news/%9");
        let gone = ask(&mut ledger, "pane:team-gone/%1");
        let held = ask(&mut ledger, "pane:team-held/%1");
        let emptied = asked_at + HOUR_MS;
        ledger.team_dissolved("team-gone", emptied);
        let run = ledger.run(&run_id).expect("the run");
        let owed = |at: i64| -> Vec<String> {
            desk_letters(&ledger, run, at)
                .mail
                .into_iter()
                .map(|row| row.id)
                .collect()
        };
        assert_eq!(
            owed(emptied + 23 * HOUR_MS),
            vec![stranger.clone(), gone, held.clone()]
        );
        assert_eq!(
            owed(emptied + 25 * HOUR_MS),
            vec![stranger.clone(), held.clone()],
            "a question from a chair empty for 25 hours is still owed"
        );
        assert_eq!(owed(asked_at + 71 * HOUR_MS), vec![stranger, held]);
        assert!(
            owed(asked_at + 73 * HOUR_MS).is_empty(),
            "questions 73 hours old are still owed"
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

    /// t-42447: a landing check's letter is news on the desk, and it carries its verdict, its exit
    /// code and the number of files its merge clashed on — so the screen says the outcome without
    /// reading the ledger's JSON. Other letters carry none of those three keys.
    #[test]
    fn a_land_check_letter_is_news_with_its_verdict_its_exit_code_and_its_clashes() {
        let now = crate::now_epoch_ms();
        let mut ledger = Ledger::new();
        let (run_id, _) = a_worker_carrying_a_task(&mut ledger, now - HOUR_MS);
        let mut passed = a_notice(
            &run_id,
            MessageKind::LandCheck,
            "land-check t-1: passed · rc 0",
        );
        passed.payload = Text::from(r#"{"state":"passed","rc":0,"check":"lc-1-1","task":"t-1"}"#);
        let passed = ledger
            .post(&run_id, passed, now - 2_000)
            .expect("a passed check's letter");
        let mut clash = a_notice(&run_id, MessageKind::LandCheck, "land-check t-1: conflict");
        clash.payload = Text::from(
            r#"{"state":"conflict","conflicts":{"total":2,"files":["a.txt","b.txt"]},"check":"lc-1-2","task":"t-1"}"#,
        );
        let clash = ledger
            .post(&run_id, clash, now - 1_000)
            .expect("a clash's letter");
        let worker = a_notice(&run_id, MessageKind::WorkerDied, r#"{"workerId":"w-gone"}"#);
        let died = ledger
            .post(&run_id, worker, now - 500)
            .expect("a death's letter");
        let desk = desk_json(&ledger);
        let lines = drawn(&desk);
        let of = |id: &str| {
            lines
                .iter()
                .find(|one| one["id"].as_str() == Some(id))
                .cloned()
                .unwrap_or_else(|| panic!("the letter {id} is not drawn: {desk}"))
        };
        let passed_line = of(&passed);
        assert_eq!(passed_line["kind"], "land_check", "{passed_line}");
        assert_eq!(passed_line["verdict"], "passed", "{passed_line}");
        assert_eq!(passed_line["rc"], 0, "{passed_line}");
        assert!(passed_line.get("clashes").is_none(), "{passed_line}");
        let clash_line = of(&clash);
        assert_eq!(clash_line["verdict"], "conflict", "{clash_line}");
        assert_eq!(clash_line["clashes"], 2, "{clash_line}");
        assert!(clash_line.get("rc").is_none(), "{clash_line}");
        let died_line = of(&died);
        assert!(died_line.get("verdict").is_none(), "{died_line}");
        assert!(died_line.get("clashes").is_none(), "{died_line}");
    }

    #[test]
    fn gate_bookkeeping_does_not_become_a_failed_merge_on_the_desk() {
        let now = crate::now_epoch_ms();
        let mut ledger = Ledger::new();
        let (run_id, _) = a_worker_carrying_a_task(&mut ledger, now - HOUR_MS);
        for phase in ["registered", "started", "completed", "reuse"] {
            let mut gate = a_notice(&run_id, MessageKind::LandCheck, "gate evidence");
            gate.payload =
                serde_json::json!({"gateEvidenceV1":{"phase":phase,"attempt":"none","value":{}}})
                    .to_string()
                    .into();
            ledger.post(&run_id, gate, now - 1_000).unwrap();
        }
        let mut failed = a_notice(&run_id, MessageKind::LandCheck, "real merge failed");
        failed.payload = r#"{"state":"failed","rc":7,"check":"lc-fixture"}"#.into();
        let failed = ledger.post(&run_id, failed, now - 500).unwrap();
        let desk = desk_json(&ledger);
        let lines = drawn(&desk);
        assert_eq!(
            lines.len(),
            1,
            "gate bookkeeping appeared as merge failures: {desk}"
        );
        assert_eq!(lines[0]["id"], failed);
        assert_eq!(lines[0]["verdict"], "failed");
        assert_eq!(lines[0]["rc"], 7);
    }

    /// One quiet episode is one line with one name (t-9548): a silence told
    /// again — every five minutes by a ledger written before t-15313 — keeps
    /// the name it began with, never the newest notice's id, which moved
    /// every reminder and made the screen draw a new line each time.
    #[test]
    fn an_episode_line_keeps_its_name_while_the_silence_is_told_again() {
        let start = crate::now_epoch_ms() - 3 * HOUR_MS;
        let mut ledger = Ledger::new();
        let (run_id, worker) = a_worker_carrying_a_task(&mut ledger, start);
        let mut names = Vec::new();
        for beat in 0..3 {
            told_again(
                &mut ledger,
                &run_id,
                &worker,
                beat,
                start + 200_000 + beat * REMINDED_MS,
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

        let unread = desk_snapshot(&ledger, |_| Some(1), no_cost);
        assert_eq!(unread.counts.folded, 2);
        assert!(
            unread.folded_batches.is_empty(),
            "a notice nobody was handed is offered: {:?}",
            unread.folded_batches
        );

        let held = desk_snapshot(&handed(&ledger, &[&old_a, &old_b]), |_| Some(1), no_cost);
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
            |_| Some(1),
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
            |_| Some(1),
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
                        10,
                    )
                    .expect("done");
            }
        }
        let mut asked = Vec::new();
        let desk = desk_snapshot(
            &ledger,
            |_| None,
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
            .and_then(|rows| rows.iter().find(|one| one["stage"] == "unknown"))
            .expect("a finished row");
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
            desk_letters(&ledger, ledger.run(&run_id).expect("run"), 4)
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
            desk_letters(&ledger, run, 4).mail.is_empty(),
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
            desk_letters(&ledger, ledger.run(&run_id).expect("run"), 20)
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
        assert!(desk_letters(&ledger, run, 22).mail.is_empty());
        let rebuilt = Ledger::rebuild(ledger.export()).expect("same ledger rebuilt");
        assert!(
            desk_letters(&rebuilt, rebuilt.run(&run_id).expect("run"), 22)
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
        let owed = desk_letters(&rebuilt, run, 4);
        assert_eq!(owed.mail.len(), 1);
        assert_eq!(owed.mail[0].id, question);
        assert_eq!(owed.mail[0].delivery, "acked");
        assert_eq!(owed.news.len(), 1);
        assert_eq!(owed.news[0].id, notice);
        assert_eq!(owed.news[0].delivery, "pending");
    }

    /// Who sits in each run's coordinator seat, as the desk's run list says it
    /// (t-45938) — the facts the sidebar's role chips and its 「코디 (비어 있음)」
    /// read: the terminal of this window in a live seat and the seat's
    /// generation; a seat held in another window, held and not this window's; a
    /// run in play whose seat a restart left empty, held by nobody since then;
    /// and the same run once a coordinator sits again, in another pane.
    #[test]
    fn the_run_list_names_the_terminal_in_each_live_seat_and_a_seat_left_empty() {
        let mut ledger = Ledger::new();
        let seated = ledger.create_run("seated", 1);
        ledger
            .seat_coordinator(&seated, "team-a/%1", None, 2)
            .expect("a seat");
        let task = ledger
            .create_task(&seated, "work".into(), "work".into(), vec![], None, 3)
            .expect("a task");
        ledger
            .start_worker(&seated, "claude", ("team-a", "%2"), Some(&task), 4)
            .expect("a worker");
        // The checkout lets the restart keep the worker asleep, so the run stays in play.
        assert!(ledger.worker_seated(("team-a", "%2"), "/Users/dev/acme/work"));
        let elsewhere = ledger.create_run("elsewhere", 1);
        ledger
            .seat_coordinator(&elsewhere, "team-b/%1", None, 2)
            .expect("a seat in another window");
        let terms = |seat: &str| (seat == "team-a/%1").then_some(7);
        let run_of = |desk: &DeskSnapshot, id: &str| {
            desk.runs
                .iter()
                .find(|one| one.run == id)
                .cloned()
                .unwrap_or_else(|| panic!("no run {id}: {:?}", desk.runs))
        };

        let desk = desk_snapshot(&ledger, terms, no_cost);
        let here = run_of(&desk, &seated);
        assert_eq!(
            (
                here.term,
                here.seat,
                here.held,
                here.generation,
                here.vacated_ms
            ),
            (Some(7), true, true, 1, None),
            "the live seat names the terminal sitting in it: {here:?}"
        );
        let there = run_of(&desk, &elsewhere);
        assert_eq!(
            (there.term, there.seat, there.held),
            (None, false, true),
            "a seat held in another window: {there:?}"
        );
        let json = serde_json::to_value(&desk).expect("the desk serializes");
        let wire = json["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|one| one["run"] == seated.as_str()))
            .cloned()
            .expect("the run on the wire");
        assert_eq!(
            (
                &wire["term"],
                &wire["held"],
                &wire["generation"],
                &wire["vacated_ms"]
            ),
            (
                &serde_json::json!(7),
                &serde_json::json!(true),
                &serde_json::json!(1),
                &serde_json::Value::Null
            ),
            "the window reads these keys: {wire}"
        );

        let _restarted = ledger.window_restarted(50);
        let left = run_of(&desk_snapshot(&ledger, terms, no_cost), &seated);
        assert_eq!(
            (
                left.term,
                left.seat,
                left.held,
                left.generation,
                left.vacated_ms
            ),
            (None, false, false, 1, Some(50)),
            "a restart leaves the seat empty, and says since when: {left:?}"
        );

        ledger
            .seat_coordinator(&seated, "team-c/%1", None, 60)
            .expect("a coordinator sits again");
        let back = desk_snapshot(&ledger, |seat| (seat == "team-c/%1").then_some(9), no_cost);
        let again = run_of(&back, &seated);
        assert_eq!(
            (again.term, again.held, again.generation, again.vacated_ms),
            (Some(9), true, 2, None),
            "the new pane in the seat, the next generation: {again:?}"
        );
    }

    /// A hold the ledger opened for a worker that exited before reporting
    /// reaches the board as its kind and its facts, not only as the ledger's
    /// English sentence (t-67607): `unexamined` with the worker and its
    /// checkout first, then what the window found there, read off the
    /// ledger's own `checkoutExamined` letter. A gate somebody opened is
    /// `asked`, and its words are the asker's.
    #[test]
    fn a_hold_reaches_the_board_as_its_kind_and_its_facts() {
        use zerocode_core::orchestration::Examined;

        let mut ledger = Ledger::new();
        let run = ledger.create_run("holds", 1);
        let held = ledger
            .create_task(&run, "do held".to_string(), "held".into(), vec![], None, 10)
            .expect("a task");
        let asked = ledger
            .create_task(
                &run,
                "do asked".to_string(),
                "asked".into(),
                vec![],
                None,
                11,
            )
            .expect("a task");
        ledger
            .seat_coordinator(&run, "team-desk/%1", None, 12)
            .expect("a coordinator");
        let started = ledger
            .start_worker(&run, "codex", ("team-desk", "%2"), Some(&held), 20)
            .expect("a worker carries it");
        assert!(ledger.worker_seated(("team-desk", "%2"), "/wt/left-behind"));
        ledger
            .terminal_gone_with_archive("team-desk", "%2", None, 30)
            .expect("a death before reporting");
        ledger
            .create_gate(&run, &asked, "rpc or rest?".into(), vec![], 31)
            .expect("a gate somebody opened");
        let gate_of = |ledger: &Ledger, id: &str| -> serde_json::Value {
            desk_json(ledger)["tasks"]
                .as_array()
                .expect("the desk's rows")
                .iter()
                .find(|row| row["id"] == id)
                .map(|row| row["gate"].clone())
                .unwrap_or_else(|| panic!("{id} is missing from the desk"))
        };

        let before = gate_of(&ledger, &held);
        assert_eq!(before["kind"], "unexamined", "{before}");
        assert_eq!(before["worker"], started.worker.as_str(), "{before}");
        assert_eq!(before["checkout"], "/wt/left-behind", "{before}");
        assert!(
            before["question"]
                .as_str()
                .is_some_and(|question| question.contains("nobody has looked")),
            "the ledger's own sentence rides along for 「원문 보기」: {before}"
        );
        let other = gate_of(&ledger, &asked);
        assert_eq!(
            (other["kind"].as_str(), other["question"].as_str()),
            (Some("asked"), Some("rpc or rest?")),
            "{other}"
        );

        let found = Examined::Unlanded {
            branch: "wt/left-behind".to_string(),
            base: "main".to_string(),
            commits: 2,
        };
        assert_eq!(ledger.checkout_examined(&started.worker, &found, 40), 1);
        let after = gate_of(&ledger, &held);
        assert_eq!(after["kind"], "unlanded", "{after}");
        assert_eq!(after["examined"]["commits"], 2, "{after}");
        assert_eq!(after["examined"]["branch"], "wt/left-behind", "{after}");
        assert_eq!(after["examined"]["base"], "main", "{after}");
    }

    /// Free words answer a decision somebody opened, never a hold the ledger
    /// opened (review t-68962 P3 ⑤): the worker that exited before reporting
    /// left a hold that takes harvest or dispatch; the person's own question
    /// takes their words.
    #[test]
    fn free_words_answer_somebodys_decision_and_never_a_ledger_hold() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("holds", 1);
        let held = ledger
            .create_task(&run, "do held".to_string(), "held".into(), vec![], None, 10)
            .expect("a task");
        let asked = ledger
            .create_task(
                &run,
                "do asked".to_string(),
                "asked".into(),
                vec![],
                None,
                11,
            )
            .expect("a task");
        ledger
            .seat_coordinator(&run, "team-desk/%1", None, 12)
            .expect("a coordinator");
        ledger
            .start_worker(&run, "codex", ("team-desk", "%2"), Some(&held), 20)
            .expect("a worker carries it");
        assert!(ledger.worker_seated(("team-desk", "%2"), "/wt/left-behind"));
        ledger
            .terminal_gone_with_archive("team-desk", "%2", None, 30)
            .expect("a death before reporting");
        let opened = ledger
            .create_gate(&run, &asked, "rpc or rest?".into(), vec![], 31)
            .expect("a gate somebody opened");
        let the_run = ledger.run(&run).expect("the run");
        let hold = the_run
            .gates
            .iter()
            .find(|gate| gate.held_for.is_some())
            .map(|gate| gate.id.clone())
            .expect("the ledger held the task for its checkout");
        let refused =
            hold_takes_words(the_run, &hold).expect_err("free words on the ledger's hold");
        assert!(refused.contains("harvest or dispatch"), "{refused}");
        assert_eq!(hold_takes_words(the_run, &opened), Ok(()));
        assert_eq!(
            hold_takes_words(the_run, "gate-unknown"),
            Ok(()),
            "left to the ledger"
        );
    }

    /// The two answers a hold takes from the board are the core's own words,
    /// an answer to somebody's decision has words, and an act the board does
    /// not name is refused before any seat is asked.
    #[test]
    fn a_hold_takes_the_ledgers_two_answers_and_nothing_else() {
        assert_eq!(
            HOLD_ACTS,
            [("harvest", "harvest"), ("dispatch", "dispatch again")]
        );
        let refused = resolve_hold("run-x", "gate-1", "close it", None, "ui-hold-1", 1)
            .expect_err("an act the board does not name");
        assert!(refused.contains("harvest or dispatch"), "{refused}");
        let unnamed = resolve_hold("run-x", "gate-1", "harvest", None, " ", 1)
            .expect_err("a press without a request name");
        assert!(unnamed.contains("retryRequest"), "{unnamed}");
        let silent = resolve_hold("run-x", "gate-1", "answer", Some("  "), "ui-hold-2", 1)
            .expect_err("an answer without words");
        assert!(silent.contains("needs words"), "{silent}");
        // And a pass that cannot name what it read is not sent at all.
        let blind = pass_review("run-x", "t-1", "", "1234567", "ui-pass-1", 1)
            .expect_err("a pass without its attempt");
        assert!(blind.contains("attempt and the source"), "{blind}");
        // Nor a close without its reason, which is the record it leaves (t-83281).
        let unreasoned = close_question("run-x", "m-1", "  ", "ui-close-1", 1)
            .expect_err("a close without a reason");
        assert!(unreasoned.contains("reason"), "{unreasoned}");
        let unnamed = close_question("run-x", "m-1", "이미 끝난 일", " ", 1)
            .expect_err("a close without a request name");
        assert!(unnamed.contains("retryRequest"), "{unnamed}");
    }

    /// The board's pass is written over the result the task holds (t-82501):
    /// the worker's summary, tests and report stand beside the verification
    /// and the moment of the press, and a task the board's image does not
    /// hold is not written over blind.
    #[test]
    fn the_boards_pass_sends_the_workers_result_with_its_verification_added() {
        let mut ledger = Ledger::new();
        let run_id = ledger.create_run("desk-pass", 1);
        let task = ledger
            .create_task(&run_id, "pass".into(), "pass".into(), vec![], None, 2)
            .expect("a task");
        let worker_said = serde_json::json!({
            "ok": true,
            "head": "abc1234",
            "summary": "빈 장바구니에서도 가격 가드가 선다",
            "tests": "price_guard 12 passed",
            "reportPath": "/Users/dev/acme-web/reviews/t-2041/REPORT.md",
        });
        ledger
            .update_task(
                &run_id,
                &task,
                None,
                Some(worker_said.to_string()),
                ResultAuthor::Worker {
                    worker: "w-2043".into(),
                    dispatch: None,
                },
                3,
            )
            .expect("the worker's report");

        let argv = pass_argv(
            ledger.run(&run_id),
            &run_id,
            &task,
            "dp-2043",
            "abc1234",
            "ui-pass-1",
            1_234,
        )
        .expect("a pass of a task the board holds");
        let named = |flag: &str| {
            argv.iter()
                .position(|word| word == flag)
                .and_then(|at| argv.get(at + 1))
                .cloned()
                .unwrap_or_else(|| panic!("{flag} missing from {argv:?}"))
        };
        assert_eq!(argv[0], "task-update");
        assert_eq!(
            [named("--task"), named("--attempt"), named("--source")],
            [task.clone(), "dp-2043".into(), "abc1234".into()]
        );
        assert_eq!(named("--retry-request"), "ui-pass-1");
        let sent: serde_json::Value =
            serde_json::from_str(&named("--result")).expect("the result is JSON");
        for key in ["ok", "head", "summary", "tests", "reportPath"] {
            assert_eq!(
                sent[key], worker_said[key],
                "the pass drops `{key}`: {sent}"
            );
        }
        assert_eq!(sent["verified"], true, "{sent}");
        assert_eq!(
            sent[zerocode_core::orchestration::BOARD_PASS_KEY],
            1_234,
            "{sent}"
        );

        let unheld = pass_argv(
            ledger.run(&run_id),
            &run_id,
            "t-404",
            "dp-2043",
            "abc1234",
            "ui-pass-2",
            1_235,
        )
        .expect_err("a task the board does not hold");
        assert!(unheld.contains("t-404"), "{unheld}");
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
        let folded = task(&mut ledger, "folded", vec![], 21);
        ledger
            .close_task(
                &run,
                &folded,
                Closure::Folded {
                    into: ready.clone(),
                },
            )
            .expect("a closing");
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
        let handed = |ledger: &mut Ledger, id: &str, pane: &str, head: &str, at: i64| {
            let started = ledger
                .start_worker(&run, "claude", ("team-desk", pane), Some(id), at)
                .expect("a worker carries it");
            ledger
                .post(
                    &run,
                    Draft {
                        from: worker_address(&started.worker),
                        to: ledger.run(&run).expect("the run").address(),
                        kind: MessageKind::WorkerDone,
                        body: Text::from(format!(r#"{{"ok":true,"head":"{head}"}}"#)),
                        subject: Text::default(),
                        priority: Priority::Normal,
                        payload: Text::default(),
                        thread: None,
                        task: Some(id.to_string()),
                        dispatch: started.dispatch.clone(),
                    },
                    at + 1,
                )
                .expect("the report");
            started.dispatch.expect("an attempt")
        };
        handed(&mut ledger, &reported, "%4", "1234567", 23);
        let landed = handed(&mut ledger, &merged, "%5", "89abcde", 25);
        ledger
            .update_task(
                &run,
                &merged,
                None,
                Some(r#"{"verified":true,"mergeHead":"89abcde"}"#.to_string()),
                ResultAuthor::Coordinator {
                    seat: "team-desk/%1".to_string(),
                    generation: Some(1),
                    attempt: Some(landed),
                    source: Some("89abcde".to_string()),
                    completed_ms: None,
                    verified_ms: None,
                },
                27,
            )
            .expect("a review");
        for (id, status, result) in [
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
                        completed_ms: None,
                        verified_ms: None,
                    },
                    10,
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

        let desk = desk_snapshot(&ledger, |_| None, no_cost);
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
            (&folded, "closed"),
        ] {
            assert_eq!(stage(id), word, "{id}");
        }
        // A closed task says why, and is not the stranded kind of blocked.
        let folded_row = desk
            .tasks
            .iter()
            .find(|one| one.id == folded)
            .expect("folded");
        assert_eq!(
            folded_row.closed,
            Some(Closure::Folded {
                into: ready.clone()
            })
        );
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
                ("verified", 0),
                ("merged", 1),
                ("deployed", 0),
                ("gate", 1),
                ("blocked", 2),
                ("stale", 0),
                ("failed", 1),
                ("unreviewable", 0),
                ("waived", 0),
                ("nothing_to_land", 0),
                ("unknown", 0),
                ("closed", 1),
            ]
        );
    }

    /// The board's 검증 대기 is the work a coordinator can still review
    /// (t-19328, t-15558 slice 2). A completed task whose every attempt ended
    /// handing nothing in has no source a review could name, so it could take
    /// no review record for ever — and stood in `reported` all the same.
    /// Before, every completed task no coordinator had merged; after, only
    /// what can still be reviewed, and the rest in a stage that says so.
    #[test]
    fn work_nothing_can_review_leaves_reported_for_a_stage_of_its_own() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("old work", 1);
        // Somebody is at the run, so it is in play.
        let carried = ledger
            .create_task(&run, "x".into(), "carried".into(), vec![], None, 2)
            .expect("a task");
        ledger
            .start_worker(&run, "codex", ("team-old", "%2"), Some(&carried), 3)
            .expect("a worker carries it");
        let coordinator = ResultAuthor::Coordinator {
            seat: "team-old/%1".to_string(),
            generation: Some(1),
            attempt: None,
            source: None,
            completed_ms: None,
            verified_ms: None,
        };
        // Five tasks the coordinator wrote down as done by hand after attempts
        // that ended handing nothing in.
        let mut by_hand = Vec::new();
        for n in 0..5_i64 {
            let task = ledger
                .create_task(
                    &run,
                    "x".into(),
                    format!("by hand {n}"),
                    vec![],
                    None,
                    10 + n,
                )
                .expect("a task");
            let pane = format!("%{}", 10 + n);
            let worker = ledger
                .start_worker(
                    &run,
                    "codex",
                    ("team-old", pane.as_str()),
                    Some(&task),
                    20 + n,
                )
                .expect("a worker")
                .worker;
            ledger
                .end_attempt(&worker, Ending::Stopped, "lost", 30 + n)
                .expect("the attempt ends");
            ledger
                .update_task(
                    &run,
                    &task,
                    Some(TaskStatus::Completed),
                    None,
                    coordinator.clone(),
                    10,
                )
                .expect("done by hand");
            by_hand.push(task);
        }
        // Two a worker handed a report in on: a coordinator can still review
        // those, so they keep the word they had.
        let mut handed = Vec::new();
        for n in 0..2_i64 {
            let task = ledger
                .create_task(
                    &run,
                    "x".into(),
                    format!("handed in {n}"),
                    vec![],
                    None,
                    40 + n,
                )
                .expect("a task");
            let pane = format!("%{}", 20 + n);
            let started = ledger
                .start_worker(
                    &run,
                    "claude",
                    ("team-old", pane.as_str()),
                    Some(&task),
                    50 + n,
                )
                .expect("a worker");
            ledger
                .post(
                    &run,
                    Draft {
                        from: worker_address(&started.worker),
                        to: ledger.run(&run).expect("the run").address(),
                        kind: MessageKind::WorkerDone,
                        body: Text::from(r#"{"ok":true}"#.to_string()),
                        subject: Text::default(),
                        priority: Priority::Normal,
                        payload: Text::default(),
                        thread: None,
                        task: Some(task.clone()),
                        dispatch: started.dispatch.clone(),
                    },
                    60 + n,
                )
                .expect("the worker's report");
            handed.push(task);
        }

        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let count = |stage: &str| {
            desk.stages
                .iter()
                .find(|one| one.stage == stage)
                .map(|one| one.count)
                .unwrap_or_else(|| panic!("{stage} is not a stage: {:?}", desk.stages))
        };
        let stage_of_row = |id: &str| {
            desk.tasks
                .iter()
                .find(|one| one.id == id)
                .map(|one| one.stage)
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        // The number before: every completed task no coordinator merged.
        let held = ledger.run(&run).expect("the run");
        let before = held
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Completed && !held.review_of(task).merged)
            .count();
        assert_eq!(before, 7, "검증 대기 before: the old reading");
        assert_eq!(
            (count("reported"), count("unreviewable")),
            (2, 5),
            "검증 대기 after: only what a coordinator can still review"
        );
        for id in &by_hand {
            assert_eq!(stage_of_row(id), "unreviewable", "{id}");
        }
        for id in &handed {
            assert_eq!(stage_of_row(id), "reported", "{id}");
        }
        assert_eq!(stage_of_row(&carried), "dispatched");
        // It is finished work all the same: its rows carry what the task cost.
        assert!(
            desk.tasks
                .iter()
                .filter(|row| row.stage == "unreviewable")
                .all(|row| row.cost.is_some()),
            "an unreviewable row lost its cost"
        );
        assert!(finished(held, held.task(&by_hand[0]).expect("a task")));
    }

    /// t-73910 W4-1: every desk stage says what comes next. A task's own
    /// status decides it before completion; after, the work's own step does,
    /// and only where the row's word is that work's word — a word and a
    /// reading that disagree, or a completed task the ledger cannot read, ask
    /// for nothing.
    #[test]
    fn every_desk_stage_says_what_comes_next_and_completed_work_reads_its_own() {
        let progress = |stage: WorkStage| WorkProgress {
            stage,
            claimed: None,
            attempt: None,
            source: None,
            merge_head: None,
            reviewed: None,
            withdrawn: None,
            next: None,
        };
        for stage in STAGES {
            if !FINISHED_STAGES.contains(&stage) {
                let step = next_step(stage, None);
                assert_eq!(
                    step == NextStep::Unknown,
                    stage == "unknown",
                    "{stage}: {step:?}"
                );
            }
        }
        for (word, work, step) in [
            ("reported", WorkStage::ReportOnly, NextStep::Review),
            ("verified", WorkStage::Verified, NextStep::Land),
            ("merged", WorkStage::Merged, NextStep::Ship),
            ("stale", WorkStage::Stale, NextStep::ReviewNewHead),
            ("deployed", WorkStage::Deployed, NextStep::Over),
            ("nothing_to_land", WorkStage::NothingToLand, NextStep::Over),
            ("unknown", WorkStage::Working, NextStep::Unknown),
            ("verified", WorkStage::ReportOnly, NextStep::Unknown),
        ] {
            assert_eq!(
                next_step(word, Some(&progress(work))),
                step,
                "{word} over {work:?}"
            );
        }
        assert_eq!(next_step("ready", None), NextStep::Summon);
        assert_eq!(next_step("gate", None), NextStep::Decide);
        assert_eq!(next_step("failed", None), NextStep::RetryOrClose);
        assert_eq!(
            serde_json::to_value(NextStep::ReviewNewHead).expect("json"),
            "review_new_head"
        );
    }

    /// Work nobody is coming back to stands apart on the desk (t-71220): each
    /// row says the pile it is in, each stage counts how many of its tasks are
    /// left, and the piles carry every task a press names — so the board's
    /// 「지금 도는 일」 and its 「답할 일」 leave them out without counting again.
    #[test]
    fn the_desk_reads_which_work_nobody_is_coming_back_to() {
        let now = crate::now_epoch_ms();
        let quiet = SETTLE_QUIET_DAYS * 86_400_000;
        let task = |ledger: &mut Ledger, run: &str, name: &str, at: i64| {
            ledger
                .create_task(run, name.into(), name.into(), vec![], None, at)
                .expect("a task")
        };
        let mut ledger = Ledger::new();
        let seated = ledger.create_run("seated", now - 2 * quiet);
        ledger
            .seat_coordinator(&seated, "team-desk/%1", None, now - 2 * quiet)
            .expect("the coordinator sits");
        let old = task(&mut ledger, &seated, "old", now - 2 * quiet);
        let fresh = task(&mut ledger, &seated, "fresh", now - HOUR_MS);
        // Nobody sits in this run; its worker, still summoned, keeps it in play.
        let empty = ledger.create_run("empty", now - HOUR_MS);
        let done = task(&mut ledger, &empty, "done", now - HOUR_MS);
        let started = ledger
            .start_worker(
                &empty,
                "claude",
                ("team-empty", "%2"),
                Some(&done),
                now - HOUR_MS + 1,
            )
            .expect("a worker carries it");
        ledger
            .post(
                &empty,
                Draft {
                    from: worker_address(&started.worker),
                    to: ledger.run(&empty).expect("the run").address(),
                    kind: MessageKind::WorkerDone,
                    body: Text::from(r#"{"ok":true,"head":"d1d1d1d"}"#.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(done.clone()),
                    dispatch: started.dispatch.clone(),
                },
                now - HOUR_MS + 2,
            )
            .expect("the report");

        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let row = |id: &str| {
            desk.tasks
                .iter()
                .find(|row| row.id == id)
                .unwrap_or_else(|| panic!("{id} has no row: {:#?}", desk.tasks))
        };
        assert_eq!(row(&old).left, Some(Left::Settle));
        assert_eq!(row(&fresh).left, None, "the coordinator's own week");
        assert_eq!(
            (row(&done).stage, row(&done).left),
            ("reported", Some(Left::Unreviewed))
        );
        let counted = |stage: &str| {
            desk.stages
                .iter()
                .find(|one| one.stage == stage)
                .map(|one| (one.count, one.left))
        };
        assert_eq!(counted("ready"), Some((2, 1)));
        assert_eq!(counted("reported"), Some((1, 1)));
        fn ids(pile: &[Named]) -> Vec<&str> {
            pile.iter().map(|one| one.task.as_str()).collect()
        }
        assert_eq!(ids(&desk.left.settle), [old.as_str()]);
        assert!(desk.left.look.is_empty());
        assert_eq!(ids(&desk.left.unreviewed), [done.as_str()]);
        assert_eq!(
            (desk.left.press_max, desk.left.quiet_days),
            (PRESS_TASKS_MAX, SETTLE_QUIET_DAYS)
        );
        let json = serde_json::to_value(&desk).expect("the desk serializes");
        let rows = json["tasks"].as_array().expect("rows");
        let left_of = |id: &str| {
            rows.iter()
                .find(|row| row["id"] == id)
                .map(|row| row["left"].clone())
        };
        assert_eq!(left_of(&old), Some(serde_json::json!("settle")));
        assert_eq!(left_of(&fresh), Some(serde_json::Value::Null));
        assert_eq!(json["left"]["unreviewed"][0]["task"], done.as_str());
    }

    /// A task the coordinator verified and marked as having no code to land — research, a review, a
    /// design — is neither 검증 대기 nor 병합: it stands in a stage of its own, which does not say the
    /// work is in main (t-34501). Only the coordinator's word makes it so; a worker's claim leaves the
    /// task where a report puts it, and a task that did land stays merged.
    #[test]
    fn verified_work_with_nothing_to_land_stands_in_its_own_stage_and_only_on_the_coordinators_word()
     {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("research", 1);
        let coordinator = |source: &str| ResultAuthor::Coordinator {
            seat: "team-r/%1".to_string(),
            generation: Some(1),
            attempt: None,
            source: Some(source.to_string()),
            completed_ms: None,
            verified_ms: None,
        };
        let mut handed_in = |title: &str, at: i64, body: &str| {
            let task = ledger
                .create_task(&run, "x".into(), title.into(), vec![], None, at)
                .expect("a task");
            let started = ledger
                .start_worker(&run, "claude", ("team-r", "%9"), Some(&task), at + 1)
                .expect("a worker");
            ledger
                .post(
                    &run,
                    Draft {
                        from: worker_address(&started.worker),
                        to: ledger.run(&run).expect("the run").address(),
                        kind: MessageKind::WorkerDone,
                        body: Text::from(body.to_string()),
                        subject: Text::default(),
                        priority: Priority::Normal,
                        payload: Text::default(),
                        thread: None,
                        task: Some(task.clone()),
                        dispatch: started.dispatch.clone(),
                    },
                    at + 2,
                )
                .expect("the report");
            (task, started.dispatch.expect("an attempt"))
        };
        let (research, research_attempt) =
            handed_in("research", 10, r#"{"ok":true,"head":"abc1234"}"#);
        let (claimed, _) = handed_in(
            "claimed",
            20,
            r#"{"ok":true,"head":"def5678","nothingToLand":true}"#,
        );
        let (landed, landed_attempt) = handed_in("landed", 30, r#"{"ok":true,"head":"0123456"}"#);
        let write = |ledger: &mut Ledger, task: &str, attempt: &str, source: &str, keys: &str| {
            let mut author = coordinator(source);
            if let ResultAuthor::Coordinator { attempt: held, .. } = &mut author {
                *held = Some(attempt.to_string());
            }
            ledger
                .update_task(&run, task, None, Some(keys.to_string()), author, 100)
                .expect("a review");
        };
        write(
            &mut ledger,
            &research,
            &research_attempt,
            "abc1234",
            r#"{"verified":true,"nothingToLand":true}"#,
        );
        write(
            &mut ledger,
            &landed,
            &landed_attempt,
            "0123456",
            r#"{"verified":true,"mergeHead":"0123456","nothingToLand":true}"#,
        );
        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let stage = |id: &str| {
            desk.tasks
                .iter()
                .find(|one| one.id == id)
                .map(|one| one.stage)
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        assert_eq!(stage(&research), "nothing_to_land");
        assert_eq!(
            stage(&claimed),
            "reported",
            "a worker's claim is not the coordinator's fact"
        );
        assert_eq!(
            stage(&landed),
            "merged",
            "work that landed is merged whatever else was written"
        );
        let held = ledger.run(&run).expect("the run");
        assert!(finished(held, held.task(&research).expect("a task")));
    }

    /// Where finished work stands is told apart on the desk as the ledger knows it (t-47538): a task
    /// verified and not landed is 검증됨, never 보고됨; one deployed is 배포됨, never 병합; one whose
    /// review names an older commit than its attempt handed in waits for a review again (`stale`),
    /// never 보고됨 as though nobody had looked; and a task completed with no attempt at all, or on a
    /// coordinator's record that names no source it reviewed, is `unknown` — never a success stage.
    /// Reported, merged and unreviewable work keep the stages they had, and every one of them is
    /// finished work the desk costs.
    #[test]
    fn verified_deployed_stale_and_unknown_work_each_stand_in_their_own_stage() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("stages", 1);
        let coordinator = |attempt: Option<&str>, source: Option<&str>| ResultAuthor::Coordinator {
            seat: "team-s/%1".to_string(),
            generation: Some(1),
            attempt: attempt.map(str::to_string),
            source: source.map(str::to_string),
            completed_ms: None,
            verified_ms: None,
        };
        let handed_in = |ledger: &mut Ledger, title: &str, at: i64, head: &str| {
            let task = ledger
                .create_task(&run, "x".into(), title.into(), vec![], None, at)
                .expect("a task");
            let pane = format!("%{at}");
            let started = ledger
                .start_worker(
                    &run,
                    "claude",
                    ("team-s", pane.as_str()),
                    Some(&task),
                    at + 1,
                )
                .expect("a worker");
            ledger
                .post(
                    &run,
                    Draft {
                        from: worker_address(&started.worker),
                        to: ledger.run(&run).expect("the run").address(),
                        kind: MessageKind::WorkerDone,
                        body: Text::from(format!(r#"{{"ok":true,"head":"{head}"}}"#)),
                        subject: Text::default(),
                        priority: Priority::Normal,
                        payload: Text::default(),
                        thread: None,
                        task: Some(task.clone()),
                        dispatch: started.dispatch.clone(),
                    },
                    at + 2,
                )
                .expect("the report");
            (task, started.dispatch.expect("an attempt"))
        };
        let review = |ledger: &mut Ledger, task: &str, attempt: &str, head: &str, keys: &str| {
            ledger
                .update_task(
                    &run,
                    task,
                    None,
                    Some(keys.to_string()),
                    coordinator(Some(attempt), Some(head)),
                    100,
                )
                .expect("a review");
        };
        let (reported, _) = handed_in(&mut ledger, "reported", 10, "1111111");
        let (verified, attempt) = handed_in(&mut ledger, "verified", 20, "2222222");
        review(
            &mut ledger,
            &verified,
            &attempt,
            "2222222",
            r#"{"verified":true}"#,
        );
        let (merged, attempt) = handed_in(&mut ledger, "merged", 30, "3333333");
        review(
            &mut ledger,
            &merged,
            &attempt,
            "3333333",
            r#"{"verified":true,"mergeHead":"3333333"}"#,
        );
        let (deployed, attempt) = handed_in(&mut ledger, "deployed", 40, "4444444");
        review(
            &mut ledger,
            &deployed,
            &attempt,
            "4444444",
            r#"{"verified":true,"merged":true,"deployed":true}"#,
        );
        let (stale, stale_attempt) = handed_in(&mut ledger, "stale", 50, "5555555");
        review(
            &mut ledger,
            &stale,
            &stale_attempt,
            "5555555",
            r#"{"verified":true}"#,
        );
        let by_hand = ledger
            .create_task(&run, "x".into(), "by hand".into(), vec![], None, 60)
            .expect("a task");
        ledger
            .update_task(
                &run,
                &by_hand,
                Some(TaskStatus::Completed),
                None,
                coordinator(None, None),
                61,
            )
            .expect("done by hand");
        let unbound = ledger
            .create_task(&run, "x".into(), "unbound".into(), vec![], None, 70)
            .expect("a task");
        ledger
            .update_task(
                &run,
                &unbound,
                Some(TaskStatus::Completed),
                Some(r#"{"merged":true}"#.to_string()),
                coordinator(None, None),
                71,
            )
            .expect("an old record");
        let mut moved = ledger.export();
        moved
            .dispatches
            .iter_mut()
            .find(|row| row.id == stale_attempt)
            .expect("the attempt")
            .source = Some("6666666".to_string());
        let ledger = Ledger::rebuild(moved).expect("a readable ledger");

        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let stage = |id: &str| {
            desk.tasks
                .iter()
                .find(|one| one.id == id)
                .map(|one| one.stage)
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        for (id, word) in [
            (&reported, "reported"),
            (&verified, "verified"),
            (&merged, "merged"),
            (&deployed, "deployed"),
            (&stale, "stale"),
            (&by_hand, "unknown"),
            (&unbound, "unknown"),
        ] {
            assert_eq!(stage(id), word, "{id}");
        }
        // The run's row counts what waits to land — verified, not merged — and nothing else:
        // the sidebar's landing line (t-56141). No batch is open here.
        assert_eq!(
            desk.runs
                .iter()
                .map(|one| (one.run.as_str(), one.landing, one.batched))
                .collect::<Vec<_>>(),
            vec![(run.as_str(), 1, 0)],
            "{:?}",
            desk.runs
        );
        for word in [
            "reported", "verified", "merged", "deployed", "stale", "unknown",
        ] {
            assert!(
                STAGES.contains(&word) && FINISHED_STAGES.contains(&word),
                "{word} is not a finished stage the desk sends"
            );
            assert_eq!(
                desk.stages.iter().filter(|one| one.stage == word).count(),
                1,
                "{word} is counted more than once, or not at all"
            );
        }
        let held = ledger.run(&run).expect("the run");
        for id in [
            &reported, &verified, &merged, &deployed, &stale, &by_hand, &unbound,
        ] {
            assert!(finished(held, held.task(id).expect("a task")), "{id}");
        }
    }

    /// A verification the coordinator withdrew stands on the desk as a report that waits on a new
    /// review (t-81913): the row keeps its 검토 stage and step, and its progress carries the
    /// withdrawal's words, which the board draws in place of 「통과」 — the ledger refuses a bare
    /// pass of that attempt and source.
    #[test]
    fn a_withdrawn_verification_waits_on_the_desk_with_its_words() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("withdrawn", 1);
        let task = ledger
            .create_task(&run, "x".into(), "withdrawn".into(), vec![], None, 10)
            .expect("a task");
        let started = ledger
            .start_worker(&run, "claude", ("team-w", "%10"), Some(&task), 11)
            .expect("a worker");
        ledger
            .post(
                &run,
                Draft {
                    from: worker_address(&started.worker),
                    to: ledger.run(&run).expect("the run").address(),
                    kind: MessageKind::WorkerDone,
                    body: Text::from(r#"{"ok":true,"head":"1111111"}"#.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(task.clone()),
                    dispatch: started.dispatch.clone(),
                },
                12,
            )
            .expect("the report");
        let attempt = started.dispatch.expect("an attempt");
        let why = "the independent review found the price guard untested";
        ledger
            .update_task(
                &run,
                &task,
                None,
                Some(
                    serde_json::json!({ "verified": false, "verificationWithdrawn": why })
                        .to_string(),
                ),
                ResultAuthor::Coordinator {
                    seat: "team-w/%1".to_string(),
                    generation: Some(1),
                    attempt: Some(attempt.clone()),
                    source: Some("1111111".to_string()),
                    completed_ms: None,
                    verified_ms: None,
                },
                20,
            )
            .expect("the withdrawal");

        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let row = desk
            .tasks
            .iter()
            .find(|one| one.id == task)
            .expect("the row");
        assert_eq!((row.stage, row.next_step), ("reported", NextStep::Review));
        let sent = serde_json::to_value(row).expect("a row the window reads");
        assert_eq!(sent["progress"]["withdrawn"], why, "{sent}");
        assert_eq!(sent["progress"]["attempt"], attempt.as_str(), "{sent}");
        assert_eq!(sent["progress"]["source"], "1111111", "{sent}");
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
                    10,
                )
                .expect("done");
        }
        let desk = desk_snapshot(&ledger, |_| Some(1), no_cost);
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
        let finished: Vec<&DeskTask> = desk
            .tasks
            .iter()
            .filter(|one| one.stage == "unknown")
            .collect();
        assert_eq!(count("unknown"), Some(10));
        assert!(
            finished
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
                std::hint::black_box(desk_snapshot(&ledger, |_| None, no_cost));
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

    /// The schedule's worker tasks over the ledger that already happened
    /// (t-71840): a copy of the person's store read into a snapshot, the desk
    /// this build draws from it, and what git at this checkout's repository
    /// says of those tasks' landings — written to a file the window's own
    /// schedule reads, as it stood before the change and after it
    /// (`ui/tests/board-schedule-ledger.mjs`). Ids, stages and times only: no
    /// title, body or path leaves.
    ///
    /// ```sh
    /// ZEROCODE_DESK_REPLAY_STORE=<a copy of authority.sqlite> \
    /// ZEROCODE_SCHEDULE_REPLAY_OUT=<replay.json> \
    ///   cargo test -p zerocode-shell --bin zerocode-shell \
    ///   orchestration::desk::tests::the_schedule_over_the_ledger_that_already_happened \
    ///   -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a measurement over a copy of the person's own ledger, written out; not a check"]
    fn the_schedule_over_the_ledger_that_already_happened() {
        let store = std::env::var("ZEROCODE_DESK_REPLAY_STORE")
            .expect("ZEROCODE_DESK_REPLAY_STORE names a copy of the authority store");
        let out = std::env::var("ZEROCODE_SCHEDULE_REPLAY_OUT")
            .expect("ZEROCODE_SCHEDULE_REPLAY_OUT names the replay file");
        let ledger = Ledger::rebuild(projection_at_rest(&store)).expect("the ledger as it stood");
        let now_ms = crate::now_epoch_ms();
        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let git = crate::artifact_flow::git_facts(&repo, now_ms);
        let landings = crate::artifact_flow::landings(
            &git,
            &desk
                .tasks
                .iter()
                .map(|task| crate::artifact_flow::LandingAsk {
                    task: &task.id,
                    merge_head: task
                        .progress
                        .as_ref()
                        .and_then(|progress| progress.merge_head.as_deref()),
                    release: task
                        .times
                        .as_ref()
                        .and_then(|times| times.release.as_deref()),
                })
                .collect::<Vec<_>>(),
        );
        let tasks: Vec<serde_json::Value> = desk
            .tasks
            .iter()
            .map(|task| {
                serde_json::json!({
                    "run": task.run, "id": task.id, "title": task.id,
                    "stage": task.stage, "times": task.times,
                })
            })
            .collect();
        let merges = git.unreleased.len()
            + git
                .releases
                .iter()
                .map(|release| release.merges.len())
                .sum::<usize>();
        let replay = serde_json::json!({
            "now_ms": now_ms,
            "tasks": tasks,
            "landings": landings,
            "git": { "merges": merges, "releases": git.releases.len() },
        });
        std::fs::write(&out, replay.to_string()).expect("the replay is written");
        println!(
            "{}",
            serde_json::json!({
                "now_ms": now_ms,
                "tasks": desk.tasks.len(),
                "timed": desk.tasks.iter().filter(|task| task.times.is_some()).count(),
                "landings": landings.len(),
                "git": replay["git"],
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

    /// What the desk and the board's ledger rows cost over one synthetic ledger (t-47538): four
    /// hundred tasks in every stage a run holds — waiting, carried, failed, reported, verified,
    /// merged and deployed — timed two hundred times each, and the bytes each answer carries to the
    /// window. The same test runs on the base and on the change; it reads no ledger of the person's.
    ///
    /// ```sh
    /// cargo test -p zerocode-shell --bin zerocode-shell \
    ///   orchestration::desk::tests::the_desk_and_the_rows_over_one_synthetic_ledger -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "a measurement, printed; not a check"]
    fn the_desk_and_the_rows_over_one_synthetic_ledger() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("measure", 1);
        let coordinator = |attempt: &str, source: &str| ResultAuthor::Coordinator {
            seat: "team-m/%0".to_string(),
            generation: Some(1),
            attempt: Some(attempt.to_string()),
            source: Some(source.to_string()),
            completed_ms: None,
            verified_ms: None,
        };
        for n in 0..400_i64 {
            let task = ledger
                .create_task(&run, "x".into(), format!("t{n}"), vec![], None, 10 + n)
                .expect("a task");
            if n % 8 == 0 {
                continue;
            }
            let pane = format!("%{}", 100 + n);
            let started = ledger
                .start_worker(
                    &run,
                    "claude",
                    ("team-m", pane.as_str()),
                    Some(&task),
                    1_000 + n,
                )
                .expect("a worker");
            let attempt = started.dispatch.clone().expect("an attempt");
            match n % 8 {
                1 => continue,
                7 => {
                    ledger
                        .end_attempt(&started.worker, Ending::Stopped, "lost", 2_000 + n)
                        .expect("the attempt ends");
                    continue;
                }
                _ => {}
            }
            let head = format!("{n:07x}abcdef");
            ledger
                .post(
                    &run,
                    Draft {
                        from: worker_address(&started.worker),
                        to: ledger.run(&run).expect("the run").address(),
                        kind: MessageKind::WorkerDone,
                        body: Text::from(format!(r#"{{"ok":true,"head":"{head}"}}"#)),
                        subject: Text::default(),
                        priority: Priority::Normal,
                        payload: Text::default(),
                        thread: None,
                        task: Some(task.clone()),
                        dispatch: Some(attempt.clone()),
                    },
                    3_000 + n,
                )
                .expect("the report");
            let keys = match n % 8 {
                3 => r#"{"verified":true}"#.to_string(),
                4 => format!(r#"{{"verified":true,"mergeHead":"{head}"}}"#),
                5 => r#"{"verified":true,"merged":true,"deployed":true}"#.to_string(),
                _ => continue,
            };
            ledger
                .update_task(
                    &run,
                    &task,
                    None,
                    Some(keys),
                    coordinator(&attempt, &head),
                    4_000 + n,
                )
                .expect("a review");
        }
        let seats = super::super::TeamSeatIndex::new();
        let spread = |mut took: Vec<u128>| {
            took.sort_unstable();
            serde_json::json!({ "p50": took[took.len() / 2], "p95": took[took.len() * 95 / 100] })
        };
        let timed = |work: &dyn Fn()| {
            (0..200)
                .map(|_| {
                    let from = std::time::Instant::now();
                    work();
                    from.elapsed().as_micros()
                })
                .collect::<Vec<_>>()
        };
        let desk = timed(&|| {
            std::hint::black_box(desk_snapshot(&ledger, |_| None, no_cost));
        });
        let rows = timed(&|| {
            std::hint::black_box(super::super::ledger_agents_for_seats(&ledger, &seats));
        });
        let desk_bytes = serde_json::to_vec(&desk_snapshot(&ledger, |_| None, no_cost))
            .expect("the desk serializes")
            .len();
        let listed = super::super::ledger_agents_for_seats(&ledger, &seats);
        let row_bytes = serde_json::to_vec(&listed)
            .expect("the rows serialize")
            .len();
        println!(
            "{}",
            serde_json::json!({
                "tasks": 400,
                "rows": listed.len(),
                "deskMicros": spread(desk),
                "rowsMicros": spread(rows),
                "deskBytes": desk_bytes,
                "rowBytes": row_bytes,
            })
        );
    }

    /// The task board's schedule draws a task on the time axis from when its
    /// work moved (t-67482): its first attempt's start, the report that ended
    /// it, the coordinator's first `verified` and the landing it wrote — read
    /// off the ledger the desk already holds. A task nobody carried has none.
    #[test]
    fn a_desk_row_carries_when_its_work_moved() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("timed", 1);
        let landed = ledger
            .create_task(&run, "land it".into(), "landed".into(), vec![], None, 10)
            .expect("a task");
        let waiting = ledger
            .create_task(&run, "wait".into(), "waiting".into(), vec![], None, 11)
            .expect("a task");
        let started = ledger
            .start_worker(&run, "claude", ("team-timed", "%2"), Some(&landed), 12)
            .expect("a worker carries it");
        ledger
            .post(
                &run,
                Draft {
                    from: worker_address(&started.worker),
                    to: ledger.run(&run).expect("the run").address(),
                    kind: MessageKind::WorkerDone,
                    body: Text::from(r#"{"ok":true,"head":"89abcde"}"#.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(landed.clone()),
                    dispatch: started.dispatch.clone(),
                },
                20,
            )
            .expect("the report");
        ledger
            .update_task(
                &run,
                &landed,
                Some(TaskStatus::Completed),
                Some(r#"{"verified":true,"merged":true,"mergeHead":"89abcde"}"#.to_string()),
                ResultAuthor::Coordinator {
                    seat: "team-timed/%1".to_string(),
                    generation: Some(1),
                    attempt: started.dispatch.clone(),
                    source: Some("89abcde".to_string()),
                    completed_ms: None,
                    verified_ms: None,
                },
                30,
            )
            .expect("a review");
        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let times = |id: &str| {
            desk.tasks
                .iter()
                .find(|row| row.id == id)
                .map(|row| row.times.clone())
                .unwrap_or_else(|| panic!("{id} missing"))
        };
        assert_eq!(
            times(&landed),
            Some(TaskTimes {
                started_ms: Some(12),
                handed_ms: Some(20),
                verified_ms: Some(30),
                landed_ms: Some(30),
                ended_ms: Some(20),
                release: None,
            })
        );
        assert_eq!(times(&waiting), None, "nobody carried it");
        let wire = serde_json::to_value(&desk).expect("the desk serializes");
        let row = wire["tasks"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["id"] == landed.as_str()))
            .cloned()
            .expect("the landed row");
        assert_eq!(
            row["times"],
            serde_json::json!({
                "started_ms": 12, "handed_ms": 20, "verified_ms": 30, "landed_ms": 30,
                "ended_ms": 20, "release": null,
            }),
            "the window reads these keys"
        );
    }

    /// A finished task's bar ends where its work ended (t-71840), so the desk
    /// carries what the ledger knows of that end beyond a landing: when a
    /// failed attempt ended, and the release a deployed review names — its
    /// version only; when that release was cut is git's to say. A review that
    /// does not say deployed names no release.
    #[test]
    fn a_desk_row_carries_where_a_finished_task_ended() {
        let mut ledger = Ledger::new();
        let run = ledger.create_run("ended", 1);
        let failed = ledger
            .create_task(&run, "fail it".into(), "failed".into(), vec![], None, 10)
            .expect("a task");
        let shipped = ledger
            .create_task(&run, "ship it".into(), "shipped".into(), vec![], None, 11)
            .expect("a task");
        let lost = ledger
            .start_worker(&run, "claude", ("team-ended", "%2"), Some(&failed), 12)
            .expect("a worker carries it");
        // The worker reports it could not finish: that ends its attempt, unhanded.
        ledger
            .post(
                &run,
                Draft {
                    from: worker_address(&lost.worker),
                    to: ledger.run(&run).expect("the run").address(),
                    kind: MessageKind::WorkerDone,
                    body: Text::from(r#"{"ok":false}"#.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(failed.clone()),
                    dispatch: lost.dispatch.clone(),
                },
                30,
            )
            .expect("the failed report");
        ledger
            .update_task(
                &run,
                &failed,
                Some(TaskStatus::Failed),
                Some(r#"{"ok":false}"#.to_string()),
                ResultAuthor::Coordinator {
                    seat: "team-ended/%1".to_string(),
                    generation: Some(1),
                    attempt: lost.dispatch.clone(),
                    source: None,
                    completed_ms: None,
                    verified_ms: None,
                },
                40,
            )
            .expect("a failure");
        let carried = ledger
            .start_worker(&run, "claude", ("team-ended", "%3"), Some(&shipped), 13)
            .expect("a worker carries it");
        ledger
            .post(
                &run,
                Draft {
                    from: worker_address(&carried.worker),
                    to: ledger.run(&run).expect("the run").address(),
                    kind: MessageKind::WorkerDone,
                    body: Text::from(r#"{"ok":true,"head":"0123abc"}"#.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(shipped.clone()),
                    dispatch: carried.dispatch.clone(),
                },
                25,
            )
            .expect("the report");
        ledger
            .update_task(
                &run,
                &shipped,
                Some(TaskStatus::Completed),
                Some(
                    r#"{"verified":true,"merged":true,"deployed":true,"release":"v1.2.3"}"#
                        .to_string(),
                ),
                ResultAuthor::Coordinator {
                    seat: "team-ended/%1".to_string(),
                    generation: Some(1),
                    attempt: carried.dispatch.clone(),
                    source: Some("0123abc".to_string()),
                    completed_ms: None,
                    verified_ms: None,
                },
                50,
            )
            .expect("a review");
        let desk = desk_snapshot(&ledger, |_| None, no_cost);
        let times = |id: &str| {
            desk.tasks
                .iter()
                .find(|row| row.id == id)
                .and_then(|row| row.times.clone())
                .unwrap_or_else(|| panic!("{id} has no times"))
        };
        let failed_times = times(&failed);
        assert_eq!(
            (
                failed_times.handed_ms,
                failed_times.ended_ms,
                failed_times.release.as_deref()
            ),
            (None, Some(30), None),
            "a failed task handed nothing in, its attempt ended at 30 and it shipped nothing"
        );
        let shipped_times = times(&shipped);
        assert_eq!(
            (shipped_times.ended_ms, shipped_times.release.as_deref()),
            (Some(25), Some("v1.2.3")),
            "{shipped_times:?}"
        );
    }
    /// The inbox page (t-67483) is a view over this snapshot and the panes:
    /// the number it says for 질문 is the coordinator cards' sum and the desk's
    /// 「답할 우편」, its 넘김 is the desk's `reported` stage, and its 막힘 and
    /// 알림 together are the desk's 「소식」 plus the panes and the landing
    /// line — one count, made in one place. Three questions over two runs
    /// (2 + 1), a hand-in, a quota wall and a worker's death in the news, and
    /// three panes (a stop, its own question, a finish).
    #[test]
    fn inbox_counts_equal_card_and_desk_counts() {
        use crate::cmd::waiting::WaitingRow;
        let start = crate::now_epoch_ms() - HOUR_MS;
        let mut ledger = Ledger::new();
        let (first, _) = a_worker_carrying_a_task(&mut ledger, start);
        // A hand-in somebody is coming back to has a live seat to come back to.
        ledger
            .seat_coordinator(&first, "team-news/%1", None, start + 49)
            .expect("a seat");
        let second = ledger.create_run("desk-second", start + 50);
        ledger
            .start_worker(&second, "codex", ("team-second", "%3"), None, start + 51)
            .expect("somebody is at the second run");
        let ask = |ledger: &mut Ledger, run: &str, team: &str, at: i64| {
            ledger
                .post(
                    run,
                    Draft {
                        from: format!("pane:{team}/%7"),
                        to: format!("run:{run}"),
                        kind: MessageKind::Question,
                        body: Text::from("main에 올릴까요?"),
                        subject: Text::default(),
                        priority: Priority::Normal,
                        payload: Text::default(),
                        thread: None,
                        task: None,
                        dispatch: None,
                    },
                    start + at,
                )
                .expect("a question");
        };
        ask(&mut ledger, &first, "team-news", 60);
        ask(&mut ledger, &first, "team-news", 61);
        ask(&mut ledger, &second, "team-second", 62);
        for (at, kind) in [
            (63, MessageKind::QuotaWalled),
            (64, MessageKind::WorkerDied),
        ] {
            ledger
                .post(
                    &first,
                    a_notice(&first, kind, r#"{"workerId":"w-elsewhere"}"#),
                    start + at,
                )
                .expect("a notice");
        }
        let task = ledger
            .create_task(
                &first,
                "hand".into(),
                "hand".into(),
                vec![],
                None,
                start + 65,
            )
            .expect("a task");
        let carrier = ledger
            .start_worker(
                &first,
                "claude",
                ("team-news", "%4"),
                Some(&task),
                start + 66,
            )
            .expect("a worker carries it");
        ledger
            .post(
                &first,
                Draft {
                    from: worker_address(&carrier.worker),
                    to: ledger.run(&first).expect("the run").address(),
                    kind: MessageKind::WorkerDone,
                    body: Text::from(r#"{"ok":true,"head":"1234567"}"#.to_string()),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(task.clone()),
                    dispatch: carrier.dispatch.clone(),
                },
                start + 67,
            )
            .expect("the report");
        let panes: Vec<WaitingRow> = [("blocked", 3), ("question", 4), ("finished", 5)]
            .into_iter()
            .map(|(waiting, at)| WaitingRow {
                pane: format!("term:{at}"),
                agent: "claude",
                worktree: format!("/Users/dev/acme/wt-{at}"),
                waiting,
                since_ms: start + at,
            })
            .collect();

        let desk = desk_snapshot(
            &ledger,
            |seat| (seat == "team-news/%1").then_some(7),
            no_cost,
        );
        let inbox = crate::orchestration::inbox::inbox_of(&desk, &panes);
        let cards: usize = desk.runs.iter().map(|run| run.mail).sum();
        assert_eq!(desk.counts.mail, 3, "{:?}", desk.counts);
        assert_eq!(
            (inbox.counts.question, cards),
            (3, 3),
            "the page's 질문 is the cards' sum (2 + 1) and the desk's 「답할 우편」"
        );
        let reported = desk
            .stages
            .iter()
            .find(|stage| stage.stage == "reported")
            .map(|stage| stage.count);
        assert_eq!(
            (inbox.counts.handoff, reported),
            (1, Some(1)),
            "the page's 넘김 is the desk's reported stage"
        );
        let letters = |tab: &str| {
            inbox
                .items
                .iter()
                .filter(|item| item.tab == tab && item.letter.is_some())
                .count()
        };
        assert_eq!(
            letters("blocked") + letters("notice"),
            desk.counts.news,
            "the news is split between 막힘 and 알림, never counted twice or dropped"
        );
        assert_eq!(
            (inbox.counts.blocked, inbox.counts.notice),
            (1 + 2, 1 + 1),
            "막힘 = the wall + the stopped pane + the pane's own question; \
             알림 = the death + the finish"
        );
        assert_eq!(
            inbox.counts.waiting,
            inbox.counts.question + inbox.counts.handoff + inbox.counts.blocked,
            "the sidebar's number is everything but the notices"
        );
        assert_eq!(inbox.counts.all, inbox.items.len(), "{:?}", inbox.counts);
        let order: Vec<&str> = inbox.items.iter().map(|item| item.tab).collect();
        let mut sorted = order.clone();
        sorted.sort_by_key(|tab| {
            crate::orchestration::inbox::TABS
                .iter()
                .position(|one| one == tab)
        });
        assert_eq!(order, sorted, "the tabs stand in the page's order");
    }
}
