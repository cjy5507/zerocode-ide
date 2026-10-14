//! 묶음 통합 큐(t-45315, T3 6단계) — 원장의 반쪽.
//!
//! 코디가 `land-batch`로 준비된 과업 여럿을 고르면, 창이 기준 가지 위에 과업마다 병합 커밋을
//! 앱 전용 ref(`refs/zerocode/stack/`)에 쌓고, 끝 트리를 한 번 게이트하고, 초록이면 사람이나
//! 코디가 기준 가지를 빨리감기한다. 원장은 git도 프로세스도 다루지 않는다. 이 파일이 하는 일은
//! 셋이다. 누가 무엇을 누르는지 정해 창에 건네고(`plan`, [`press`]), 창이 한 일을 상태 기계에
//! 비추어 받아 적고([`Ledger::land_batched`]), 기록을 접어 지금 상태를 답한다([`batches`]).
//!
//! 기록은 착지 전 점검처럼 원장 자신의 `land_check` 편지다. 페이로드의
//! [`INTEGRATION_BATCH_KEY`] 아래에 묶음·세대·단계·누른 쪽·시각과 그 단계의 값이 있다. 새 편지
//! 종류는 없고, 일반 `send --type land_check`는 이미 거절되므로 다른 쪽이 묶음 기록을 꾸밀 수
//! 없다. 이 저장소에서 큐가 켜져 있는지는 창이 프로젝트 설정에서 읽는다. 원장은 설정을 읽지
//! 않으므로 계획은 아무것도 적지 않고 창에 일을 건넨다 — 꺼진 저장소의 누름은 창이 기록을 하나도
//! 받기 전에 거절한다.

use super::*;
/// The verb, spelled once: the table, the plan arm, the window and the board all say it.
pub const LAND_BATCH_VERB: &str = "land-batch";

/// The prefix every batch id starts with. Refs and paths are built from an id only after
/// [`is_batch_id`] said yes.
pub const BATCH_PREFIX: &str = "lb-";

/// The key a batch record sits under in a ledger letter's payload, on the road the gate
/// receipts already take with their own key.
pub const INTEGRATION_BATCH_KEY: &str = "integrationBatchV1";

/// How many tasks one batch stacks at most. A culprit search over eight costs three more gates.
pub const BATCH_MEMBERS_MAX: usize = 8;

/// The presses a line can carry: exactly one, or none for a read.
const PRESSES: [&str; 8] = [
    "--members",
    "--prepare",
    "--record",
    "--promote",
    "--bisect",
    "--restack",
    "--abort",
    "--record-merged",
];

/// The one platform judgment: the queue runs on macOS only for now.
#[must_use]
pub const fn integration_queue_supported() -> bool {
    cfg!(target_os = "macos")
}

/// Whether `text` is a batch id this window could have minted.
#[must_use]
pub fn is_batch_id(_text: &str) -> bool {
    false
}

/// Whether `text` is a whole git object id.
#[must_use]
pub fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The publish operation of one generation.
#[must_use]
pub fn operation_id(_run: &str, _batch: &str, _generation: u32) -> String {
    String::new()
}

/// The next step a culprit search gates.
#[must_use]
pub const fn bisect_next(_lo: u32, _hi: u32) -> Option<u32> {
    None
}

/// Who pressed: the person through the window's own button, the coordinator's verb from the
/// run's live seat, or nobody — the next run settling what a crash left, or a record the
/// ledger derives itself.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PressedBy {
    Person,
    Coordinator {
        run: String,
        seat: String,
        generation: u32,
    },
    Ledger,
}

impl PressedBy {
    /// How a letter names the presser.
    #[must_use]
    pub const fn said(&self) -> &'static str {
        match self {
            Self::Person => "the person",
            Self::Coordinator { .. } => "the coordinator",
            Self::Ledger => "the ledger",
        }
    }
}

/// The person's press.
#[must_use]
pub fn person_press() -> PressedBy {
    PressedBy::Ledger
}

/// One task of a batch, at the exact commit its worker handed in.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub task: String,
    pub head: String,
}

/// One member as it landed: its head and the merge commit that carried it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Landed {
    pub task: String,
    pub head: String,
    pub merged_as: String,
}

/// A member a restack left out, and why.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dropped {
    pub task: String,
    pub why: DropWhy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropWhy {
    /// The culprit search named it.
    Culprit,
    /// The press named it with `--drop`.
    Asked,
    /// It reached the base branch another way while the batch waited.
    OnMain,
}

/// What a gate said. Only exit code 0 is green.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Green,
    Red,
}

impl Verdict {
    #[must_use]
    pub const fn of(rc: i32) -> Self {
        if rc == 0 { Self::Green } else { Self::Red }
    }

    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Green => "green",
            Self::Red => "red",
        }
    }
}

/// Why stacking stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackStop {
    Conflict,
    GitSilent,
    GitRefused,
    PublishRefused,
}

/// Why a promote stopped without moving the base branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromoteStop {
    MainMoved,
    UnfinishedFiles,
    TwoCheckouts,
    GitRefused,
    GitSilent,
    UnknownState,
}

/// One record of a batch: its phase and that phase's value, as the payload carries them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "phase",
    content = "value",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum Record {
    Chosen {
        members: Vec<Member>,
        base_branch: String,
        base: String,
        repository: String,
    },
    Stacking {
        operation: String,
        lease_until_ms: i64,
    },
    Stacked {
        steps: Vec<String>,
        tip: String,
        tree: String,
        took_ms: i64,
        overlaps: u64,
    },
    StackStopped {
        step: u32,
        task: String,
        why: StackStop,
        files: Vec<String>,
        total: u64,
    },
    StackAbandoned {
        operation: String,
    },
    GatePrepared {
        check: String,
        step: u32,
        target: String,
        tree: String,
        folder: String,
        log: String,
    },
    Gated {
        check: String,
        step: u32,
        target: String,
        tree: String,
        verdict: Verdict,
        rc: i32,
        took_ms: Option<i64>,
        log: String,
    },
    Promoting {
        target: String,
        expect: String,
        base_branch: String,
        lease_until_ms: i64,
    },
    Promoted {
        from: String,
        to: String,
        members: Vec<Landed>,
        settled_later: bool,
    },
    PromoteStopped {
        why: PromoteStop,
        files: Vec<String>,
        total: u64,
        seen: Option<String>,
    },
    PromoteAbandoned {
        seen: String,
    },
    BisectStarted {
        lo: u32,
        hi: u32,
        next: Option<u32>,
    },
    BisectStep {
        step: u32,
        check: String,
        verdict: Verdict,
        lo: u32,
        hi: u32,
        next: Option<u32>,
    },
    CulpritFound {
        task: String,
        step: u32,
        green: Option<String>,
        red: String,
        main_first: bool,
    },
    Restacked {
        members: Vec<Member>,
        dropped: Vec<Dropped>,
        base: String,
    },
    Cancelled {},
    MergedRecorded {
        tasks: Vec<String>,
    },
    Swept {
        refs: u64,
    },
}

impl Record {
    /// The phase word the payload says.
    #[must_use]
    pub const fn phase(&self) -> &'static str {
        match self {
            Self::Chosen { .. } => "chosen",
            Self::Stacking { .. } => "stacking",
            Self::Stacked { .. } => "stacked",
            Self::StackStopped { .. } => "stack_stopped",
            Self::StackAbandoned { .. } => "stack_abandoned",
            Self::GatePrepared { .. } => "gate_prepared",
            Self::Gated { .. } => "gated",
            Self::Promoting { .. } => "promoting",
            Self::Promoted { .. } => "promoted",
            Self::PromoteStopped { .. } => "promote_stopped",
            Self::PromoteAbandoned { .. } => "promote_abandoned",
            Self::BisectStarted { .. } => "bisect_started",
            Self::BisectStep { .. } => "bisect_step",
            Self::CulpritFound { .. } => "culprit_found",
            Self::Restacked { .. } => "restacked",
            Self::Cancelled {} => "cancelled",
            Self::MergedRecorded { .. } => "merged_recorded",
            Self::Swept { .. } => "swept",
        }
    }
}

/// One written record: which batch and generation, who pressed, when, and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchRecord {
    pub batch: String,
    pub generation: u32,
    pub by: PressedBy,
    pub at: i64,
    pub record: Record,
}

impl BatchRecord {
    /// The record a ledger letter carries.
    #[must_use]
    pub fn of(_row: &Message) -> Option<Self> {
        None
    }
}

/// What the window hands back about one batch: a record it saw happen, for the ledger to judge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandBatchReceipt {
    pub run: String,
    pub batch: String,
    pub generation: u32,
    pub by: PressedBy,
    pub record: Record,
}

/// What one receipt wrote: the letters, and the batch as it stands after them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchWritten {
    pub letters: Vec<String>,
    pub state: BatchState,
}

/// This generation's stacking, from its first record.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stacking {
    pub operation: String,
    pub lease_until_ms: i64,
    pub by: PressedBy,
}

/// This generation's stack: one merge commit per member, the tip and its tree.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub steps: Vec<String>,
    pub tip: String,
    pub tree: String,
    pub took_ms: i64,
    pub overlaps: u64,
}

/// Where this generation's stacking stopped.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StackHalt {
    pub step: u32,
    pub task: String,
    pub why: StackStop,
    pub files: Vec<String>,
    pub total: u64,
}

/// A gate folder prepared and not yet recorded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedGate {
    pub check: String,
    pub step: u32,
    pub target: String,
    pub tree: String,
    pub folder: String,
    pub log: String,
}

/// The newest gate of this generation's tip.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TipGate {
    pub check: String,
    pub tree: String,
    pub verdict: Verdict,
    pub rc: i32,
    pub took_ms: Option<i64>,
    pub log: String,
}

/// One gate a culprit search ran.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchStep {
    pub step: u32,
    pub check: String,
    pub verdict: Verdict,
}

/// A culprit search: the newest step known green, the oldest known red, and the next to gate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Search {
    pub lo: u32,
    pub hi: u32,
    pub next: Option<u32>,
    pub steps: Vec<SearchStep>,
    pub by: PressedBy,
}

/// The member a search named, with the checks either side of it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Culprit {
    pub task: String,
    pub step: u32,
    pub green: Option<String>,
    pub red: String,
    pub main_first: bool,
}

/// A promote under way: who pressed, what it moves and what it expects to find.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Promoting {
    pub by: PressedBy,
    pub at: i64,
    pub target: String,
    pub expect: String,
    pub base_branch: String,
    pub lease_until_ms: i64,
}

/// The last promote that stopped without moving anything.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromoteHalt {
    pub why: PromoteStop,
    pub files: Vec<String>,
    pub total: u64,
    pub seen: Option<String>,
}

/// The promote that moved the base branch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Promotion {
    pub by: PressedBy,
    pub at: i64,
    pub from: String,
    pub to: String,
    pub members: Vec<Landed>,
    pub settled_later: bool,
}

/// What a batch is doing now, in one word the card and the coordinator read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Standing {
    Chosen,
    Stacking,
    StackStopped,
    StackAbandoned,
    Stacked,
    Gating,
    Green,
    Red,
    Searching,
    CulpritFound,
    Promoting,
    PromoteStopped,
    Promoted,
    Cancelled,
}

/// A batch, folded from its records: who chose which tasks on which base, how far this
/// generation got, and what its gates said.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchState {
    pub batch: String,
    /// The task the ledger opened for the batch: the row its letters are written against.
    pub task: String,
    pub generation: u32,
    /// This generation's members, in stacking order.
    pub members: Vec<Member>,
    pub base_branch: String,
    /// The base this generation stacks on, B.
    pub base: String,
    /// A checkout that names the repository.
    pub repository: String,
    pub chosen_by: PressedBy,
    pub chosen_at: i64,
    /// Every generation's publish operation, oldest first.
    pub operations: Vec<String>,
    /// Every member a restack left out, over all generations.
    pub dropped: Vec<Dropped>,
    pub stacking: Option<Stacking>,
    pub stacked: Option<Stack>,
    pub stack_halt: Option<StackHalt>,
    pub stack_abandoned: bool,
    pub prepared: Option<PreparedGate>,
    pub tip_gate: Option<TipGate>,
    pub search: Option<Search>,
    pub culprit: Option<Culprit>,
    pub promoting: Option<Promoting>,
    pub promote_halt: Option<PromoteHalt>,
    pub promotion: Option<Promotion>,
    pub cancelled: Option<PressedBy>,
    pub merged_recorded: bool,
    pub swept: bool,
    pub standing: Standing,
    pub last_at: i64,
}

impl BatchState {
    /// How many members this generation stacks.
    #[must_use]
    pub fn size(&self) -> u32 {
        u32::try_from(self.members.len()).unwrap_or(u32::MAX)
    }

    /// Whether the batch was promoted or cancelled.
    #[must_use]
    pub const fn ended(&self) -> bool {
        self.promotion.is_some() || self.cancelled.is_some()
    }

    /// Whether this generation's stacking began and has not ended.
    #[must_use]
    pub const fn stacking_open(&self) -> bool {
        self.stacking.is_some()
            && self.stacked.is_none()
            && self.stack_halt.is_none()
            && !self.stack_abandoned
    }

    /// Whether a culprit search is between its start and its answer.
    #[must_use]
    pub const fn searching(&self) -> bool {
        self.search.is_some() && self.culprit.is_none()
    }

    /// S(step) of this generation's stack.
    #[must_use]
    pub fn step_commit(&self, step: u32) -> Option<&str> {
        let at = usize::try_from(step).ok()?.checked_sub(1)?;
        self.stacked.as_ref()?.steps.get(at).map(String::as_str)
    }
}

/// Every batch of a run.
#[must_use]
pub fn batches(_run: &Run) -> Vec<BatchState> {
    Vec::new()
}

/// One batch of a run.
#[must_use]
pub fn batch_state(_run: &Run, _batch: &str) -> Option<BatchState> {
    None
}

/// What the window is asked to do for one press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandBatchAsk {
    pub run: String,
    pub by: PressedBy,
    pub action: Action,
}

/// One press, with the batch as the ledger read it when the press was judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Choose these members in this order and stack them. `checkout` only names the
    /// repository; landing messages are read from `message_dir/<task>.txt`.
    Choose {
        members: Vec<Member>,
        checkout: String,
        message_dir: String,
    },
    /// Prepare a gate folder for S(`step`), which is `target`.
    Prepare {
        batch: BatchState,
        step: u32,
        target: String,
    },
    /// The end of the gate jobs on a prepared folder: the whole set's exit code (0 only when
    /// every job passed) and the per-job RC summary.
    Record {
        batch: BatchState,
        check: String,
        rc: i32,
        log: String,
        took_ms: Option<i64>,
    },
    Promote {
        batch: BatchState,
    },
    Bisect {
        batch: BatchState,
    },
    Restack {
        batch: BatchState,
        drop: Vec<String>,
    },
    Cancel {
        batch: BatchState,
    },
}

/// A press on a batch that exists, as the board's buttons and the verb's flags name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    Prepare,
    Record {
        check: String,
        rc: i32,
        log: String,
        took_ms: Option<i64>,
    },
    Promote,
    Bisect,
    Restack {
        drop: Vec<String>,
    },
    Cancel,
}

/// The same judgment the verb makes, for a press that did not come through the verb.
pub fn press(
    _ledger: &Ledger,
    _run_id: &str,
    _batch: &str,
    _asked: Press,
    _by: PressedBy,
) -> Result<LandBatchAsk, String> {
    Err(format!("{LAND_BATCH_VERB} is not built yet"))
}

/// Whether a line presses anything. A line that presses nothing is a read and needs no name.
pub(super) fn presses(words: &Words) -> bool {
    PRESSES
        .into_iter()
        .any(|flag| words.has(flag) || words.value(flag).is_some())
}

/// What the plan does with one `land-batch` line.
pub(super) enum Planned {
    /// Answered here: a read, or the merged records the coordinator wrote.
    Said(serde_json::Value),
    /// Carried by the window, which answers once it has done the work.
    Carry(LandBatchAsk),
}

/// The ledger's half of `land-batch`.
pub(super) fn plan(
    _ledger: &mut Ledger,
    _run_id: &str,
    _team: &Team,
    _pane: &str,
    _words: &Words,
    _supported: bool,
    _now_ms: i64,
) -> Result<Planned, String> {
    Err(format!("{LAND_BATCH_VERB} is not built yet"))
}

impl Ledger {
    /// One record the window saw happen.
    pub fn land_batched(
        &mut self,
        _receipt: &LandBatchReceipt,
        _now_ms: i64,
    ) -> Result<BatchWritten, String> {
        Err(format!("{LAND_BATCH_VERB} is not built yet"))
    }
}
