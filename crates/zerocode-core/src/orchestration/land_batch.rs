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
use sha2::{Digest, Sha256};

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

/// Every flag the verb reads. Anything else is refused rather than ignored.
const ACCEPTED: [&str; 16] = [
    "--run",
    "--batch",
    "--members",
    "--message-dir",
    "--prepare",
    "--record",
    "--rc",
    "--log",
    "--took-ms",
    "--promote",
    "--bisect",
    "--restack",
    "--drop",
    "--abort",
    "--record-merged",
    RETRY_REQUEST,
];

/// The one platform judgment: the queue runs on macOS only for now. The publisher's tests run
/// on Unix only, the build line that runs the gate is macOS-only, and a fast-forward on
/// Windows can stop half way over a file another program holds open.
#[must_use]
pub const fn integration_queue_supported() -> bool {
    cfg!(target_os = "macos")
}

/// Whether `text` is a batch id this window could have minted: `lb-<digits>-<digits>`.
#[must_use]
pub fn is_batch_id(text: &str) -> bool {
    let Some(rest) = text.strip_prefix(BATCH_PREFIX) else {
        return false;
    };
    let mut parts = rest.split('-');
    let digits = |part: Option<&str>| {
        part.is_some_and(|part| {
            !part.is_empty() && part.len() <= 20 && part.bytes().all(|b| b.is_ascii_digit())
        })
    };
    digits(parts.next()) && digits(parts.next()) && parts.next().is_none()
}

/// Whether `text` is a whole git object id: 40 or 64 lowercase hex digits. A batch stacks exact
/// commits, never an abbreviation git would have to resolve.
#[must_use]
pub fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The publish operation of one generation: SHA-256 over the run, the batch and the generation.
/// The same generation tried again asks with the same id, and a restack gets a new one.
#[must_use]
pub fn operation_id(run: &str, batch: &str, generation: u32) -> String {
    format!(
        "{:x}",
        Sha256::digest(format!("{run}\0{batch}\0{generation}").as_bytes())
    )
}

/// The next step a culprit search gates, or `None` once the interval is closed. `lo` is the
/// newest step known green (0 is the base itself), `hi` the oldest step known red.
#[must_use]
pub const fn bisect_next(lo: u32, hi: u32) -> Option<u32> {
    if hi <= lo.saturating_add(1) {
        None
    } else {
        Some(lo + (hi - lo) / 2)
    }
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

/// The person's press. Only the window's board command, behind its confirmation, calls this: a
/// verb, a request body or a worker has no road to it, and a source contract holds it there.
#[must_use]
pub fn person_press() -> PressedBy {
    PressedBy::Person
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
    /// The payload of the letter that carries this record.
    fn payload(&self) -> serde_json::Value {
        let mut body = serde_json::to_value(&self.record).unwrap_or_default();
        if let Some(object) = body.as_object_mut() {
            object.insert("batch".into(), self.batch.clone().into());
            object.insert("generation".into(), self.generation.into());
            object.insert(
                "by".into(),
                serde_json::to_value(&self.by).unwrap_or_default(),
            );
            object.insert("at".into(), self.at.into());
        }
        serde_json::json!({ (INTEGRATION_BATCH_KEY): body })
    }

    /// The record a ledger letter carries, or `None` for any other letter.
    #[must_use]
    pub fn of(row: &Message) -> Option<Self> {
        if row.kind != MessageKind::LandCheck || row.from != LEDGER_ITSELF {
            return None;
        }
        let payload: serde_json::Value = serde_json::from_str(row.payload.as_str()).ok()?;
        let body = payload.get(INTEGRATION_BATCH_KEY)?;
        let record = serde_json::from_value(serde_json::json!({
            "phase": body.get("phase")?,
            "value": body.get("value")?,
        }))
        .ok()?;
        Some(Self {
            batch: body.get("batch")?.as_str()?.to_string(),
            generation: u32::try_from(body.get("generation")?.as_u64()?).ok()?,
            by: serde_json::from_value(body.get("by")?.clone()).ok()?,
            at: body.get("at")?.as_i64()?,
            record,
        })
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
    fn chosen(record: &BatchRecord, task: &str) -> Option<Self> {
        let Record::Chosen {
            members,
            base_branch,
            base,
            repository,
        } = &record.record
        else {
            return None;
        };
        Some(Self {
            batch: record.batch.clone(),
            task: task.to_string(),
            generation: record.generation,
            members: members.clone(),
            base_branch: base_branch.clone(),
            base: base.clone(),
            repository: repository.clone(),
            chosen_by: record.by.clone(),
            chosen_at: record.at,
            operations: Vec::new(),
            dropped: Vec::new(),
            stacking: None,
            stacked: None,
            stack_halt: None,
            stack_abandoned: false,
            prepared: None,
            tip_gate: None,
            search: None,
            culprit: None,
            promoting: None,
            promote_halt: None,
            promotion: None,
            cancelled: None,
            merged_recorded: false,
            swept: false,
            standing: Standing::Chosen,
            last_at: record.at,
        })
    }

    /// How many members this generation stacks.
    #[must_use]
    pub fn size(&self) -> u32 {
        u32::try_from(self.members.len()).unwrap_or(u32::MAX)
    }

    /// Whether the batch was promoted or cancelled. An ended batch takes no more presses.
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

    fn ended_word(&self) -> &'static str {
        if self.promotion.is_some() {
            "promoted"
        } else {
            "cancelled"
        }
    }

    fn apply(&mut self, record: &BatchRecord) {
        self.last_at = record.at;
        match &record.record {
            Record::MergedRecorded { .. } => self.merged_recorded = true,
            Record::Swept { .. } => self.swept = true,
            Record::Restacked {
                members,
                dropped,
                base,
            } => {
                if record.generation == self.generation.saturating_add(1) {
                    self.generation = record.generation;
                    self.members.clone_from(members);
                    self.base.clone_from(base);
                    self.dropped.extend(dropped.iter().cloned());
                    self.stacking = None;
                    self.stacked = None;
                    self.stack_halt = None;
                    self.stack_abandoned = false;
                    self.prepared = None;
                    self.tip_gate = None;
                    self.search = None;
                    self.culprit = None;
                    self.promoting = None;
                    self.promote_halt = None;
                }
            }
            _ if record.generation != self.generation => {}
            Record::Chosen { .. } => {}
            Record::Stacking {
                operation,
                lease_until_ms,
            } => {
                self.operations.push(operation.clone());
                self.stacking = Some(Stacking {
                    operation: operation.clone(),
                    lease_until_ms: *lease_until_ms,
                    by: record.by.clone(),
                });
            }
            Record::Stacked {
                steps,
                tip,
                tree,
                took_ms,
                overlaps,
            } => {
                self.stacked = Some(Stack {
                    steps: steps.clone(),
                    tip: tip.clone(),
                    tree: tree.clone(),
                    took_ms: *took_ms,
                    overlaps: *overlaps,
                });
            }
            Record::StackStopped {
                step,
                task,
                why,
                files,
                total,
            } => {
                self.stack_halt = Some(StackHalt {
                    step: *step,
                    task: task.clone(),
                    why: *why,
                    files: files.clone(),
                    total: *total,
                });
            }
            Record::StackAbandoned { .. } => self.stack_abandoned = true,
            Record::GatePrepared {
                check,
                step,
                target,
                tree,
                folder,
                log,
            } => {
                self.prepared = Some(PreparedGate {
                    check: check.clone(),
                    step: *step,
                    target: target.clone(),
                    tree: tree.clone(),
                    folder: folder.clone(),
                    log: log.clone(),
                });
            }
            Record::Gated {
                check,
                step,
                tree,
                verdict,
                rc,
                took_ms,
                log,
                ..
            } => {
                if self
                    .prepared
                    .as_ref()
                    .is_some_and(|held| held.check == *check)
                {
                    self.prepared = None;
                }
                if *step == self.size() {
                    self.tip_gate = Some(TipGate {
                        check: check.clone(),
                        tree: tree.clone(),
                        verdict: *verdict,
                        rc: *rc,
                        took_ms: *took_ms,
                        log: log.clone(),
                    });
                    self.promote_halt = None;
                }
            }
            Record::BisectStarted { lo, hi, next } => {
                self.search = Some(Search {
                    lo: *lo,
                    hi: *hi,
                    next: *next,
                    steps: Vec::new(),
                    by: record.by.clone(),
                });
            }
            Record::BisectStep {
                step,
                check,
                verdict,
                lo,
                hi,
                next,
            } => {
                if self
                    .prepared
                    .as_ref()
                    .is_some_and(|held| held.check == *check)
                {
                    self.prepared = None;
                }
                if let Some(search) = &mut self.search {
                    search.lo = *lo;
                    search.hi = *hi;
                    search.next = *next;
                    search.steps.push(SearchStep {
                        step: *step,
                        check: check.clone(),
                        verdict: *verdict,
                    });
                }
            }
            Record::CulpritFound {
                task,
                step,
                green,
                red,
                main_first,
            } => {
                self.culprit = Some(Culprit {
                    task: task.clone(),
                    step: *step,
                    green: green.clone(),
                    red: red.clone(),
                    main_first: *main_first,
                });
                if let Some(search) = &mut self.search {
                    search.next = None;
                }
            }
            Record::Promoting {
                target,
                expect,
                base_branch,
                lease_until_ms,
            } => {
                self.promoting = Some(Promoting {
                    by: record.by.clone(),
                    at: record.at,
                    target: target.clone(),
                    expect: expect.clone(),
                    base_branch: base_branch.clone(),
                    lease_until_ms: *lease_until_ms,
                });
                self.promote_halt = None;
            }
            Record::Promoted {
                from,
                to,
                members,
                settled_later,
            } => {
                self.promoting = None;
                self.promotion = Some(Promotion {
                    by: record.by.clone(),
                    at: record.at,
                    from: from.clone(),
                    to: to.clone(),
                    members: members.clone(),
                    settled_later: *settled_later,
                });
            }
            Record::PromoteStopped {
                why,
                files,
                total,
                seen,
            } => {
                self.promoting = None;
                self.promote_halt = Some(PromoteHalt {
                    why: *why,
                    files: files.clone(),
                    total: *total,
                    seen: seen.clone(),
                });
            }
            Record::PromoteAbandoned { .. } => self.promoting = None,
            Record::Cancelled {} => self.cancelled = Some(record.by.clone()),
        }
    }

    fn standing_now(&self) -> Standing {
        if self.cancelled.is_some() {
            Standing::Cancelled
        } else if self.promotion.is_some() {
            Standing::Promoted
        } else if self.promoting.is_some() {
            Standing::Promoting
        } else if self.culprit.is_some() {
            Standing::CulpritFound
        } else if self.search.is_some() {
            Standing::Searching
        } else if self.stack_halt.is_some() {
            Standing::StackStopped
        } else if self.stack_abandoned {
            Standing::StackAbandoned
        } else if self.stacking.is_none() {
            Standing::Chosen
        } else if self.stacked.is_none() {
            Standing::Stacking
        } else if self.promote_halt.is_some() {
            Standing::PromoteStopped
        } else if self.prepared.is_some() {
            Standing::Gating
        } else {
            match self.tip_gate.as_ref().map(|gate| gate.verdict) {
                Some(Verdict::Green) => Standing::Green,
                Some(Verdict::Red) => Standing::Red,
                None => Standing::Stacked,
            }
        }
    }
}

/// Every batch of a run, folded from its letters in the order they were written.
#[must_use]
pub fn batches(run: &Run) -> Vec<BatchState> {
    let mut held: Vec<BatchState> = Vec::new();
    for row in run.messages() {
        let Some(record) = BatchRecord::of(row) else {
            continue;
        };
        if let Some(state) = held.iter_mut().find(|one| one.batch == record.batch) {
            state.apply(&record);
        } else if let Some(state) =
            BatchState::chosen(&record, row.task.as_deref().unwrap_or_default())
        {
            held.push(state);
        }
    }
    for state in &mut held {
        state.standing = state.standing_now();
    }
    held
}

/// One batch of a run, as its records leave it.
#[must_use]
pub fn batch_state(run: &Run, batch: &str) -> Option<BatchState> {
    batches(run).into_iter().find(|one| one.batch == batch)
}

fn last_record(run: &Run, batch: &str) -> Option<BatchRecord> {
    run.messages()
        .iter()
        .rev()
        .filter_map(BatchRecord::of)
        .find(|record| record.batch == batch)
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

/// The same judgment the verb makes, for a press that did not come through the verb — the
/// window's board button, whose presser is the person.
pub fn press(
    ledger: &Ledger,
    run_id: &str,
    batch: &str,
    asked: Press,
    by: PressedBy,
) -> Result<LandBatchAsk, String> {
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    let state =
        batch_state(run, batch).ok_or_else(|| format!("run {run_id} has no batch {batch}"))?;
    press_on(run_id, state, asked, by)
}

fn press_on(
    run_id: &str,
    state: BatchState,
    asked: Press,
    by: PressedBy,
) -> Result<LandBatchAsk, String> {
    let action = match asked {
        Press::Prepare => {
            let (step, target) = gate_ready(&state)?;
            Action::Prepare {
                batch: state,
                step,
                target,
            }
        }
        Press::Record {
            check,
            rc,
            log,
            took_ms,
        } => {
            still_open(&state)?;
            if state
                .prepared
                .as_ref()
                .is_none_or(|held| held.check != check)
            {
                return Err(format!(
                    "batch {} has no prepared gate {check} — record the check its --prepare \
                     answered",
                    state.batch
                ));
            }
            Action::Record {
                batch: state,
                check,
                rc,
                log,
                took_ms,
            }
        }
        Press::Promote => {
            promotable(&state)?;
            Action::Promote { batch: state }
        }
        Press::Bisect => {
            searchable(&state)?;
            Action::Bisect { batch: state }
        }
        Press::Restack { drop } => {
            restackable(&state, &drop)?;
            Action::Restack { batch: state, drop }
        }
        Press::Cancel => {
            cancellable(&state)?;
            Action::Cancel { batch: state }
        }
    };
    Ok(LandBatchAsk {
        run: run_id.to_string(),
        by,
        action,
    })
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
    /// Carried by the window, which answers once it has done the work. Boxed: the ask carries
    /// the batch as the ledger read it.
    Carry(Box<LandBatchAsk>),
}

/// The ledger's half of `land-batch`: which press, by whom, on what.
///
/// The coordinator's verb, like `land-check`: a worker that wants its work landed says so in
/// its report. Nothing is written here except the merged records `--record-merged` asks for;
/// every other record comes back from the window as a receipt.
pub(super) fn plan(
    ledger: &mut Ledger,
    run_id: &str,
    team: &Team,
    pane: &str,
    words: &Words,
    supported: bool,
    now_ms: i64,
) -> Result<Planned, String> {
    if let Some(flag) = words
        .values
        .iter()
        .map(|(name, _)| name)
        .chain(words.flags.iter())
        .find(|name| !ACCEPTED.contains(&name.as_str()))
    {
        return Err(format!("{LAND_BATCH_VERB} does not take {flag}"));
    }
    if !words.positional.is_empty() {
        return Err(format!(
            "{LAND_BATCH_VERB} takes no bare words — name the tasks with --members t-1,t-2"
        ));
    }
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    if let Some(worker) = run.worker_in_pane(&team.id, pane) {
        return Err(format!(
            "{LAND_BATCH_VERB} is the coordinator's verb, and this pane is worker {} — say in \
             your report that the work is ready and let the coordinator batch it",
            worker.id
        ));
    }
    let named: Vec<&str> = PRESSES
        .into_iter()
        .filter(|flag| words.has(flag) || words.value(flag).is_some())
        .collect();
    let press = match named.as_slice() {
        [] => return read(run, words).map(Planned::Said),
        [one] => *one,
        many => {
            return Err(format!(
                "one press at a time, and this line asks {}",
                many.join(" and ")
            ));
        }
    };
    if !supported {
        return Err(format!(
            "{LAND_BATCH_VERB} can be turned on only on macOS for now, and this machine is \
             not one — nothing was written"
        ));
    }
    let by = coordinator(ledger, run_id, team, pane)?;
    if press == "--members" {
        let names = words.list("--members");
        let message_dir = words.value("--message-dir").ok_or(
            "land-batch --members needs --message-dir <the folder holding one <task>.txt \
             landing message per task>",
        )?;
        if !std::path::Path::new(message_dir).is_absolute() {
            return Err(format!(
                "--message-dir is an absolute path, and {message_dir:?} is not one"
            ));
        }
        let (members, checkout) = choose(run, &names, &batches(run))?;
        return Ok(Planned::Carry(Box::new(LandBatchAsk {
            run: run_id.to_string(),
            by,
            action: Action::Choose {
                members,
                checkout,
                message_dir: message_dir.to_string(),
            },
        })));
    }
    let batch = words
        .value("--batch")
        .ok_or_else(|| format!("land-batch {press} needs --batch <id>"))?;
    if !is_batch_id(batch) {
        return Err(format!(
            "--batch names a batch this window chose ({BATCH_PREFIX}<ms>-<n>), and {batch:?} \
             is not one"
        ));
    }
    let state =
        batch_state(run, batch).ok_or_else(|| format!("run {run_id} has no batch {batch}"))?;
    let asked = match press {
        "--record-merged" => {
            return record_merged(ledger, run_id, team, pane, &state, by, now_ms)
                .map(Planned::Said);
        }
        "--prepare" => Press::Prepare,
        "--record" => recorded(words)?,
        "--promote" => Press::Promote,
        "--bisect" => Press::Bisect,
        "--restack" => Press::Restack {
            drop: words.list("--drop"),
        },
        _ => Press::Cancel,
    };
    press_on(run_id, state, asked, by).map(|ask| Planned::Carry(Box::new(ask)))
}

fn read(run: &Run, words: &Words) -> Result<serde_json::Value, String> {
    let held = batches(run);
    let Some(batch) = words.value("--batch") else {
        let held = serde_json::to_value(&held).map_err(|why| why.to_string())?;
        return Ok(serde_json::json!({ "batches": held }));
    };
    let state = held
        .into_iter()
        .find(|one| one.batch == batch)
        .ok_or_else(|| format!("run {} has no batch {batch}", run.id))?;
    let state = serde_json::to_value(&state).map_err(|why| why.to_string())?;
    Ok(serde_json::json!({ "batch": state }))
}

fn recorded(words: &Words) -> Result<Press, String> {
    let check = words.value("--record").unwrap_or_default();
    if !land_check::is_check_id(check) {
        return Err(format!(
            "--record names the gate check its --prepare answered ({}<ms>-<n>), and {check:?} \
             is not one",
            land_check::CHECK_PREFIX
        ));
    }
    let rc = words
        .value("--rc")
        .ok_or("land-batch --record needs --rc <the whole gate set's exit code>")?;
    let rc = rc
        .parse::<i32>()
        .map_err(|_| format!("--rc is an exit code, and {rc:?} is not one"))?;
    let log = words
        .value("--log")
        .ok_or("land-batch --record needs --log <the per-job RC summary>")?;
    if !std::path::Path::new(log).is_absolute() {
        return Err(format!("--log is an absolute path, and {log:?} is not one"));
    }
    let took_ms = match words.value("--took-ms") {
        Some(took) => Some(
            took.parse::<i64>()
                .ok()
                .filter(|took| *took >= 0)
                .ok_or_else(|| format!("--took-ms is milliseconds, and {took:?} is not"))?,
        ),
        None => None,
    };
    Ok(Press::Record {
        check: check.to_string(),
        rc,
        log: log.to_string(),
        took_ms,
    })
}

/// The coordinator as a presser: the run's live seat, and only from that seat.
fn coordinator(
    ledger: &Ledger,
    run_id: &str,
    team: &Team,
    pane: &str,
) -> Result<PressedBy, String> {
    let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
    let Some(seat) = run.coordinator_live() else {
        return Err(format!(
            "{LAND_BATCH_VERB} is the live coordinator seat's verb, and run {run_id} has \
             nobody sitting — sit first with `run-use {run_id}`"
        ));
    };
    if sender(ledger, &team.leader_pane, run_id, (&team.id, pane)) != run.address() {
        return Err(format!(
            "{LAND_BATCH_VERB} is the coordinator's verb: run {run_id} is coordinated from {} \
             (generation {}), and pane {}/{pane} is not that seat",
            seat.seat, seat.generation, team.id
        ));
    }
    Ok(PressedBy::Coordinator {
        run: run_id.to_string(),
        seat: seat.seat.clone(),
        generation: seat.generation,
    })
}

/// Every named task as a member, or the first reason one is not ready. All or nothing: a
/// refusal leaves no half a batch, because nothing is written until every member stands.
fn choose(
    run: &Run,
    names: &[String],
    held: &[BatchState],
) -> Result<(Vec<Member>, String), String> {
    if names.is_empty() {
        return Err(
            "--members names the tasks to stack, in the order to stack them: --members t-1,t-2"
                .into(),
        );
    }
    if names.len() > BATCH_MEMBERS_MAX {
        return Err(format!(
            "a batch holds at most {BATCH_MEMBERS_MAX} tasks, and {} were named — nothing was \
             chosen",
            names.len()
        ));
    }
    for (at, name) in names.iter().enumerate() {
        if names[..at].contains(name) {
            return Err(format!(
                "task {name} is named twice — a task is stacked once"
            ));
        }
    }
    let members = names
        .iter()
        .map(|name| member_ready(run, name, held))
        .collect::<Result<Vec<_>, _>>()?;
    let checkout = names.iter().find_map(|name| checkout_of(run, name)).ok_or(
        "no member has a checkout written down — no worker of theirs was seated in one, so \
             there is no repository to stack in",
    )?;
    Ok((members, checkout))
}

/// One task as a member: its newest attempt ended in success with a whole commit handed in,
/// the coordinator's review of that attempt and head says verified and not merged, and no
/// open batch holds it.
fn member_ready(run: &Run, name: &str, held: &[BatchState]) -> Result<Member, String> {
    let task = run
        .task(name)
        .ok_or_else(|| format!("unknown task: {name}"))?;
    if let Some(batch) = held.iter().find(|one| one.task == name) {
        return Err(format!(
            "task {name} is the task of batch {}, not work to land",
            batch.batch
        ));
    }
    if let Some(batch) = held
        .iter()
        .find(|one| !one.ended() && one.members.iter().any(|member| member.task == name))
    {
        return Err(format!(
            "task {name} is already in open batch {} — finish or cancel that one first",
            batch.batch
        ));
    }
    if matches!(task.status, TaskStatus::Failed | TaskStatus::Closed) {
        return Err(format!(
            "task {name} is {} — only finished, verified work is stacked",
            task.status.as_str()
        ));
    }
    let attempt = run
        .newest_attempt(name)
        .ok_or_else(|| format!("task {name} has no attempt, so nothing was handed in to stack"))?;
    if attempt.is_open() || attempt.succeeded != Some(true) {
        return Err(format!(
            "task {name}'s newest attempt {} has not ended in success, so it has no handed-in \
             head to stack",
            attempt.id
        ));
    }
    let Some(head) = attempt.source.as_deref().filter(|head| is_object_id(head)) else {
        return Err(format!(
            "task {name}'s newest attempt {} handed in no whole commit id — a batch stacks \
             exact commits",
            attempt.id
        ));
    };
    let review = run.review_of(task);
    if review.merged {
        return Err(format!(
            "task {name} is already merged by the coordinator's review — it is on the base \
             branch, not a batch member"
        ));
    }
    if !review.verified {
        return Err(format!(
            "task {name} is not verified: no coordinator review of attempt {} at {} says \
             verified",
            attempt.id,
            short(head)
        ));
    }
    Ok(Member {
        task: name.to_string(),
        head: head.to_string(),
    })
}

fn checkout_of(run: &Run, task: &str) -> Option<String> {
    run.dispatches
        .iter()
        .rev()
        .filter(|attempt| attempt.task == task)
        .find_map(|attempt| {
            run.worker(&attempt.worker)
                .and_then(|worker| worker.checkout.clone())
        })
}

fn still_open(state: &BatchState) -> Result<(), String> {
    if state.ended() {
        return Err(format!(
            "batch {} has ended ({}) — nothing more is pressed on it",
            state.batch,
            state.ended_word()
        ));
    }
    Ok(())
}

fn not_promoting(state: &BatchState) -> Result<(), String> {
    if state.promoting.is_some() {
        return Err(format!(
            "batch {} is promoting — this waits until the promote has ended, because only then \
             is its outcome known",
            state.batch
        ));
    }
    Ok(())
}

/// The gate a press prepares now: the next step of a culprit search, or the tip.
fn gate_ready(state: &BatchState) -> Result<(u32, String), String> {
    still_open(state)?;
    not_promoting(state)?;
    let Some(stack) = &state.stacked else {
        return Err(format!(
            "batch {} has nothing stacked to gate in generation {}",
            state.batch, state.generation
        ));
    };
    if state.searching()
        && let Some(step) = state.search.as_ref().and_then(|search| search.next)
    {
        let target = state.step_commit(step).ok_or_else(|| {
            format!(
                "batch {}'s search asks for step {step}, which its stack does not have",
                state.batch
            )
        })?;
        return Ok((step, target.to_string()));
    }
    Ok((state.size(), stack.tip.clone()))
}

fn promotable(state: &BatchState) -> Result<(), String> {
    still_open(state)?;
    if state.promoting.is_some() {
        return Err(format!("batch {} is already promoting", state.batch));
    }
    let Some(stack) = &state.stacked else {
        return Err(format!(
            "batch {} has nothing stacked in generation {}, so there is no green tip to promote",
            state.batch, state.generation
        ));
    };
    match &state.tip_gate {
        Some(gate) if gate.verdict == Verdict::Green && gate.tree == stack.tree => Ok(()),
        Some(gate) if gate.verdict == Verdict::Green => Err(format!(
            "batch {}'s green gate {} checked tree {}, not this generation's tip tree {} — \
             gate the tip again",
            state.batch,
            gate.check,
            short(&gate.tree),
            short(&stack.tree)
        )),
        Some(gate) => Err(format!(
            "batch {}'s gate {} of the tip is red — a promote needs a green gate of this exact \
             tree; search for the culprit or restack",
            state.batch, gate.check
        )),
        None => Err(format!(
            "batch {} has no green gate of generation {}'s tip tree {} — gate it with --prepare \
             and --record first",
            state.batch,
            state.generation,
            short(&stack.tree)
        )),
    }
}

fn searchable(state: &BatchState) -> Result<(), String> {
    still_open(state)?;
    not_promoting(state)?;
    if state.search.is_some() {
        return Err(match &state.culprit {
            Some(culprit) => format!(
                "batch {}'s search already named {} — restack without it",
                state.batch, culprit.task
            ),
            None => format!("batch {} is already searching", state.batch),
        });
    }
    match &state.tip_gate {
        Some(gate) if gate.verdict == Verdict::Red => Ok(()),
        _ => Err(format!(
            "a culprit search starts from a red gate of the tip, and batch {} has none",
            state.batch
        )),
    }
}

fn restackable(state: &BatchState, drop: &[String]) -> Result<(), String> {
    still_open(state)?;
    not_promoting(state)?;
    if state.stacking_open() {
        return Err(format!(
            "batch {} is still stacking generation {}",
            state.batch, state.generation
        ));
    }
    if let Some(stray) = drop
        .iter()
        .find(|name| !state.members.iter().any(|member| member.task == **name))
    {
        return Err(format!(
            "task {stray} is not a member of batch {}'s generation {}",
            state.batch, state.generation
        ));
    }
    if state
        .members
        .iter()
        .all(|member| drop.contains(&member.task))
    {
        return Err(format!(
            "dropping every member leaves nothing to stack — cancel batch {} instead",
            state.batch
        ));
    }
    Ok(())
}

fn cancellable(state: &BatchState) -> Result<(), String> {
    still_open(state)?;
    not_promoting(state)
}

fn stacking_now(state: &BatchState) -> Result<(), String> {
    if !state.stacking_open() {
        return Err(format!(
            "generation {} of batch {} is not stacking",
            state.generation, state.batch
        ));
    }
    Ok(())
}

fn promoting_now(state: &BatchState) -> Result<&Promoting, String> {
    state
        .promoting
        .as_ref()
        .ok_or_else(|| format!("batch {} is not promoting", state.batch))
}

/// Whether the window's receipt may be written: the record the batch's records allow next.
fn admit(run: &Run, held: &[BatchState], receipt: &LandBatchReceipt) -> Result<(), String> {
    let batch = receipt.batch.as_str();
    let Some(state) = held.iter().find(|one| one.batch == batch) else {
        let Record::Chosen {
            members,
            base_branch,
            base,
            repository,
        } = &receipt.record
        else {
            return Err(format!("run {} has no batch {batch}", run.id));
        };
        if receipt.generation != 1 {
            return Err(format!(
                "a batch is chosen as generation 1, and this receipt says {}",
                receipt.generation
            ));
        }
        if receipt.by == PressedBy::Ledger {
            return Err(
                "a batch is chosen by somebody — the coordinator or the person, never the ledger"
                    .into(),
            );
        }
        if base_branch.trim().is_empty() || !is_object_id(base) || repository.trim().is_empty() {
            return Err(format!(
                "batch {batch} names no base branch, base commit or repository"
            ));
        }
        let names: Vec<String> = members.iter().map(|member| member.task.clone()).collect();
        let (ready, _) = choose(run, &names, held)?;
        if ready != *members {
            return Err(format!(
                "the heads handed in for batch {batch} moved since the press — nothing was chosen"
            ));
        }
        return Ok(());
    };
    match &receipt.record {
        Record::Chosen { .. } => Err(format!("batch {batch} is already chosen")),
        Record::Swept { .. } if !state.ended() => Err(format!(
            "batch {batch} is open — only an ended batch is swept"
        )),
        Record::Swept { .. } if state.swept => Err(format!("batch {batch} is already swept")),
        Record::Swept { .. } => Ok(()),
        Record::MergedRecorded { .. } if state.promotion.is_none() => Err(format!(
            "batch {batch} has not been promoted — merged records follow a promote"
        )),
        Record::MergedRecorded { .. } if state.merged_recorded => Err(format!(
            "batch {batch}'s merged records are already written"
        )),
        Record::MergedRecorded { .. } => Ok(()),
        Record::Restacked {
            members,
            dropped,
            base,
        } => {
            let next = state.generation.saturating_add(1);
            if receipt.generation != next {
                return Err(format!(
                    "a restack of batch {batch} makes generation {next}, and this receipt says {}",
                    receipt.generation
                ));
            }
            let dropping: Vec<String> = dropped.iter().map(|one| one.task.clone()).collect();
            restackable(state, &dropping)?;
            let kept: Vec<&Member> = state
                .members
                .iter()
                .filter(|member| !dropping.contains(&member.task))
                .collect();
            if members.len() != kept.len()
                || members.iter().zip(&kept).any(|(one, other)| one != *other)
            {
                return Err(format!(
                    "generation {next} of batch {batch} keeps every member it does not drop, in \
                     order"
                ));
            }
            if !is_object_id(base) {
                return Err(format!("the restack of batch {batch} names no base commit"));
            }
            Ok(())
        }
        _ if receipt.generation != state.generation => Err(format!(
            "batch {batch} is at generation {}, and this receipt is for generation {} — a \
             record of another generation changes nothing",
            state.generation, receipt.generation
        )),
        _ if state.ended() => Err(format!(
            "batch {batch} has ended ({}) — nothing more is written about it",
            state.ended_word()
        )),
        Record::Stacking { operation, .. } => {
            if state.stacking.is_some() {
                return Err(format!(
                    "generation {} of batch {batch} has already begun stacking",
                    state.generation
                ));
            }
            if !is_object_id(operation) {
                return Err(format!("{operation:?} is not a publish operation id"));
            }
            Ok(())
        }
        Record::Stacked {
            steps, tip, tree, ..
        } => {
            stacking_now(state)?;
            if steps.len() != state.members.len()
                || steps.last() != Some(tip)
                || !steps.iter().all(|step| is_object_id(step))
                || !is_object_id(tree)
            {
                return Err(format!(
                    "batch {batch}'s stack names one commit per member, ending at the tip, and \
                     the tip's tree"
                ));
            }
            Ok(())
        }
        Record::StackStopped { step, .. } => {
            stacking_now(state)?;
            if !(1..=state.size()).contains(step) {
                return Err(format!("step {step} is not a step of batch {batch}"));
            }
            Ok(())
        }
        Record::StackAbandoned { .. } => stacking_now(state),
        Record::GatePrepared {
            check,
            step,
            target,
            ..
        } => {
            let (want, at) = gate_ready(state)?;
            if *step != want || *target != at {
                return Err(format!(
                    "batch {batch}'s next gate is step {want} at {}, not step {step} at {}",
                    short(&at),
                    short(target)
                ));
            }
            if !land_check::is_check_id(check) {
                return Err(format!("{check:?} is not a check id"));
            }
            Ok(())
        }
        Record::Gated {
            check,
            step,
            target,
            verdict,
            rc,
            ..
        } => {
            let Some(prepared) = state.prepared.as_ref().filter(|held| held.check == *check) else {
                return Err(format!("batch {batch} has no prepared gate {check}"));
            };
            if prepared.step != *step || prepared.target != *target {
                return Err(format!(
                    "gate {check} was prepared for step {} at {}, not step {step} at {}",
                    prepared.step,
                    short(&prepared.target),
                    short(target)
                ));
            }
            if *verdict != Verdict::of(*rc) {
                return Err(format!(
                    "rc {rc} is {}: only rc 0 is green",
                    Verdict::of(*rc).word()
                ));
            }
            Ok(())
        }
        Record::Promoting { target, expect, .. } => {
            promotable(state)?;
            if receipt.by == PressedBy::Ledger {
                return Err(
                    "a promote is pressed by somebody — the person or the coordinator".into(),
                );
            }
            let tip = state
                .stacked
                .as_ref()
                .map(|stack| stack.tip.as_str())
                .unwrap_or_default();
            if target != tip || *expect != state.base {
                return Err(format!(
                    "batch {batch} promotes {} over {}, not {} over {}",
                    short(tip),
                    short(&state.base),
                    short(target),
                    short(expect)
                ));
            }
            Ok(())
        }
        Record::Promoted { from, to, .. } => {
            let promoting = promoting_now(state)?;
            if receipt.by != promoting.by {
                return Err(format!(
                    "batch {batch}'s promote was pressed by {}, and a promoted record keeps that \
                     presser",
                    promoting.by.said()
                ));
            }
            if *from != promoting.expect || *to != promoting.target {
                return Err(format!(
                    "batch {batch}'s promote moves {} to {}, not {} to {}",
                    short(&promoting.expect),
                    short(&promoting.target),
                    short(from),
                    short(to)
                ));
            }
            Ok(())
        }
        Record::PromoteStopped { .. } | Record::PromoteAbandoned { .. } => {
            let promoting = promoting_now(state)?;
            if receipt.by != promoting.by && receipt.by != PressedBy::Ledger {
                return Err(format!(
                    "batch {batch}'s promote was pressed by {}",
                    promoting.by.said()
                ));
            }
            Ok(())
        }
        Record::BisectStarted { lo, hi, next } => {
            searchable(state)?;
            if *lo != 0 || *hi != state.size() || *next != bisect_next(0, state.size()) {
                return Err(format!(
                    "a search of batch {batch} starts with the base green and step {} red",
                    state.size()
                ));
            }
            Ok(())
        }
        Record::BisectStep { .. } | Record::CulpritFound { .. } => Err(format!(
            "the ledger writes {} itself, from the gate's record",
            receipt.record.phase()
        )),
        Record::Cancelled {} => {
            cancellable(state)?;
            if receipt.by == PressedBy::Ledger {
                return Err(
                    "a cancel is pressed by somebody — the person or the coordinator".into(),
                );
            }
            Ok(())
        }
    }
}

/// What one admitted receipt writes: itself — or, for the gate of a culprit search, the
/// search's step and, once the interval closes, the culprit.
fn derived(
    before: Option<&BatchState>,
    receipt: &LandBatchReceipt,
    now_ms: i64,
) -> Vec<BatchRecord> {
    let written = |record: Record, by: PressedBy| BatchRecord {
        batch: receipt.batch.clone(),
        generation: receipt.generation,
        by,
        at: now_ms,
        record,
    };
    let Some(state) = before else {
        return vec![written(receipt.record.clone(), receipt.by.clone())];
    };
    let search = state.search.as_ref().filter(|_| state.culprit.is_none());
    match (&receipt.record, search) {
        (
            Record::Gated {
                check,
                step,
                verdict,
                ..
            },
            Some(search),
        ) => {
            let (lo, hi) = match verdict {
                Verdict::Green => (*step, search.hi),
                Verdict::Red => (search.lo, *step),
            };
            let next = bisect_next(lo, hi);
            let stepped = written(
                Record::BisectStep {
                    step: *step,
                    check: check.clone(),
                    verdict: *verdict,
                    lo,
                    hi,
                    next,
                },
                receipt.by.clone(),
            );
            if next.is_some() {
                return vec![stepped];
            }
            let found = culprit_of(state, lo, hi, Some((*step, check.as_str())));
            vec![stepped, written(found, PressedBy::Ledger)]
        }
        (Record::BisectStarted { next: None, .. }, _) => vec![
            written(receipt.record.clone(), receipt.by.clone()),
            written(culprit_of(state, 0, state.size(), None), PressedBy::Ledger),
        ],
        _ => vec![written(receipt.record.clone(), receipt.by.clone())],
    }
}

/// The culprit of a closed interval: member `hi`, with the checks that bound it. A culprit that
/// is the first member is also what a red base would look like, and the record says so.
fn culprit_of(state: &BatchState, lo: u32, hi: u32, newest: Option<(u32, &str)>) -> Record {
    let size = state.size();
    let check_of = |step: u32| -> Option<String> {
        if let Some((at, check)) = newest
            && at == step
        {
            return Some(check.to_string());
        }
        if step == size {
            return state.tip_gate.as_ref().map(|gate| gate.check.clone());
        }
        state.search.as_ref().and_then(|search| {
            search
                .steps
                .iter()
                .find(|tried| tried.step == step)
                .map(|tried| tried.check.clone())
        })
    };
    let task = usize::try_from(hi)
        .ok()
        .and_then(|at| at.checked_sub(1))
        .and_then(|at| state.members.get(at))
        .map(|member| member.task.clone())
        .unwrap_or_default();
    Record::CulpritFound {
        task,
        step: hi,
        green: if lo > 0 { check_of(lo) } else { None },
        red: check_of(hi).unwrap_or_default(),
        main_first: hi == 1,
    }
}

fn short(text: &str) -> &str {
    text.get(..7).unwrap_or(text)
}

fn branch_name(base_branch: &str) -> &str {
    base_branch
        .strip_prefix("refs/heads/")
        .unwrap_or(base_branch)
}

fn files_said(total: u64, files: &[String], noun: &str) -> String {
    let plural = if total == 1 { "" } else { "s" };
    format!("{total} {noun}{plural}: {}", files.join(", "))
}

/// The one line an agent reads for a record: `land-batch lb-…: stacked 4 of 4 · tip 1a2b3c4 ·
/// tree 5d6e7f8`. Built from the record and the batch it is written about, so the line and the
/// payload cannot disagree.
fn line_of(record: &BatchRecord, before: Option<&BatchState>) -> String {
    let batch = &record.batch;
    let size = before.map_or(0, BatchState::size);
    let branch = before.map_or("main", |state| branch_name(&state.base_branch));
    let base = before.map_or("", |state| short(&state.base));
    let said = match &record.record {
        Record::Chosen {
            members,
            base_branch,
            base,
            ..
        } => format!(
            "chosen {} tasks on {} {}",
            members.len(),
            branch_name(base_branch),
            short(base)
        ),
        Record::Stacking { .. } => {
            format!("stacking {size} tasks · generation {}", record.generation)
        }
        Record::Stacked {
            steps, tip, tree, ..
        } => format!(
            "stacked {} of {size} · tip {} · tree {}",
            steps.len(),
            short(tip),
            short(tree)
        ),
        Record::StackStopped {
            step,
            task,
            why,
            files,
            total,
        } => match why {
            StackStop::Conflict => format!(
                "conflict at {task} (step {step} of {size}) · {}",
                files_said(*total, files, "clashing file")
            ),
            StackStop::GitSilent => {
                format!("git gave no answer at {task} (step {step} of {size})")
            }
            StackStop::GitRefused => {
                format!("git refused to stack {task} (step {step} of {size})")
            }
            StackStop::PublishRefused => {
                format!("the integration ref was not published after step {step} of {size}")
            }
        },
        Record::StackAbandoned { .. } => format!(
            "stacking was cut off before the integration ref moved · generation {} tombstoned",
            record.generation
        ),
        Record::GatePrepared {
            check,
            step,
            target,
            tree,
            folder,
            ..
        } => format!(
            "gate {check} prepared for step {step} of {size} · commit {} · tree {} · folder \
             {folder}",
            short(target),
            short(tree)
        ),
        Record::Gated {
            verdict,
            tree,
            rc,
            took_ms,
            log,
            ..
        } => {
            let mut parts = vec![
                verdict.word().to_string(),
                format!("tree {}", short(tree)),
                format!("rc {rc}"),
            ];
            if let Some(took) = took_ms {
                parts.push(format!("{}.{} s", took / 1000, (took % 1000) / 100));
            }
            parts.push(format!("log {log}"));
            parts.join(" · ")
        }
        Record::Promoting {
            target,
            expect,
            base_branch,
            ..
        } => format!(
            "promoting {} {} → {} · pressed by {}",
            branch_name(base_branch),
            short(expect),
            short(target),
            record.by.said()
        ),
        Record::Promoted {
            from,
            to,
            members,
            settled_later,
        } => {
            let mut said = format!(
                "{branch} {} → {} (fast-forward, {} tasks) · pressed by {}",
                short(from),
                short(to),
                members.len(),
                record.by.said()
            );
            if *settled_later {
                said.push_str(" · settled by the next run");
            }
            if record.by == PressedBy::Person {
                said.push_str(" · write the merged records with `");
                said.push_str(LAND_BATCH_VERB);
                said.push_str(" --batch ");
                said.push_str(batch);
                said.push_str(" --record-merged`");
            }
            said
        }
        Record::PromoteStopped {
            why,
            files,
            total,
            seen,
        } => {
            let seen = seen.as_deref().map_or("?", short);
            let why = match why {
                PromoteStop::MainMoved => {
                    format!("{branch} moved ({base} → {seen}) · restack on the new {branch}")
                }
                PromoteStop::UnfinishedFiles => format!(
                    "{} in the {branch} checkout · clean up and press again",
                    files_said(*total, files, "unfinished file")
                ),
                PromoteStop::TwoCheckouts => format!(
                    "{branch} is checked out in {}",
                    files_said(*total, files, "place")
                ),
                PromoteStop::GitRefused => "git refused the fast-forward".to_string(),
                PromoteStop::GitSilent => "git gave no answer".to_string(),
                PromoteStop::UnknownState => format!(
                    "{branch} is at {seen}, neither the base nor the tip · look before anything \
                     else moves"
                ),
            };
            format!("promote stopped: {why}")
        }
        Record::PromoteAbandoned { .. } => {
            format!("promote was cut off before {branch} moved · the batch is green again")
        }
        Record::BisectStarted { next, .. } => match next {
            Some(step) => format!("culprit search started · gate step {step} of {size} next"),
            None => "culprit search started".to_string(),
        },
        Record::BisectStep {
            step,
            verdict,
            next,
            ..
        } => match next {
            Some(next) => format!(
                "step {step} of {size} {} · gate step {next} next",
                verdict.word()
            ),
            None => format!("step {step} of {size} {}", verdict.word()),
        },
        Record::CulpritFound {
            task,
            step,
            main_first,
            ..
        } => {
            let lo = step.saturating_sub(1);
            let green = if lo == 0 {
                "the base green".to_string()
            } else {
                format!("step {lo} green")
            };
            let mut said =
                format!("culprit {task} (step {step} of {size}) · {green}, step {step} red");
            if *main_first {
                said.push_str(" · check ");
                said.push_str(branch);
                said.push_str(" itself first");
            }
            said
        }
        Record::Restacked {
            members,
            dropped,
            base,
        } => {
            let names: Vec<&str> = dropped.iter().map(|one| one.task.as_str()).collect();
            let without = if names.is_empty() {
                String::new()
            } else {
                format!(" without {}", names.join(", "))
            };
            format!(
                "restacked as generation {}{without} · {} tasks on {branch} {}",
                record.generation,
                members.len(),
                short(base)
            )
        }
        Record::Cancelled {} => format!(
            "cancelled by {} · generation {} tombstoned",
            record.by.said(),
            record.generation
        ),
        Record::MergedRecorded { tasks } => {
            format!("merged records written for {} tasks", tasks.len())
        }
        Record::Swept { refs } => format!("{refs} refs and receipts swept"),
    };
    format!("{LAND_BATCH_VERB} {batch}: {said}")
}

/// A member's result with the merge written in, the way a coordinator records a landing:
/// verified and merged, the merge commit it landed as and where the base branch is now.
fn merged_result(run: &Run, landed: &Landed, main_at: &str) -> Option<String> {
    let task = run.task(&landed.task)?;
    let mut result = serde_json::from_str::<serde_json::Value>(task.result.as_str())
        .ok()
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let object = result.as_object_mut()?;
    object.insert("verified".into(), true.into());
    object.insert("merged".into(), true.into());
    object.insert("mergedAs".into(), landed.merged_as.clone().into());
    object.insert("mainAt".into(), main_at.into());
    Some(result.to_string())
}

/// `--record-merged`: the coordinator writes the merged records of a promote the person
/// pressed. Every author is judged first, the way `task-update` judges one, so a refusal
/// leaves every task as it was.
fn record_merged(
    ledger: &mut Ledger,
    run_id: &str,
    team: &Team,
    pane: &str,
    state: &BatchState,
    by: PressedBy,
    now_ms: i64,
) -> Result<serde_json::Value, String> {
    let Some(promotion) = &state.promotion else {
        return Err(format!(
            "batch {} has not been promoted — merged records follow a promote",
            state.batch
        ));
    };
    if state.merged_recorded {
        return Err(format!(
            "batch {}'s merged records are already written",
            state.batch
        ));
    }
    let mut writing = Vec::new();
    for landed in &promotion.members {
        let run = ledger.run(run_id).ok_or_else(|| unknown_run(run_id))?;
        let attempt = run
            .newest_attempt(&landed.task)
            .map(|attempt| attempt.id.clone());
        let result = merged_result(run, landed, &promotion.to)
            .ok_or_else(|| format!("unknown task: {}", landed.task))?;
        let observed = Observed {
            attempt: attempt.as_deref(),
            source: Some(landed.head.as_str()),
            review: true,
            bare_verification: verifies_on_nothing_new(&result),
            tested_head: None,
        };
        let author = correction_author(
            ledger,
            &team.leader_pane,
            run_id,
            &landed.task,
            (&team.id, pane),
            observed,
        )?;
        writing.push((landed.task.clone(), result, author));
    }
    for (task, result, author) in writing {
        ledger.update_task(run_id, &task, None, Some(result), author, now_ms)?;
    }
    let tasks: Vec<String> = promotion
        .members
        .iter()
        .map(|landed| landed.task.clone())
        .collect();
    let receipt = LandBatchReceipt {
        run: run_id.to_string(),
        batch: state.batch.clone(),
        generation: state.generation,
        by,
        record: Record::MergedRecorded {
            tasks: tasks.clone(),
        },
    };
    let written = ledger.land_batched(&receipt, now_ms)?;
    Ok(serde_json::json!({
        "batch": state.batch,
        "merged": tasks,
        "letters": written.letters,
    }))
}

impl Ledger {
    /// One record the window saw happen, judged against the batch's records and written as the
    /// ledger's own letter to the coordinator of the run — or refused with the reason, writing
    /// nothing. The same record twice writes once.
    pub fn land_batched(
        &mut self,
        receipt: &LandBatchReceipt,
        now_ms: i64,
    ) -> Result<BatchWritten, String> {
        if !is_batch_id(&receipt.batch) {
            return Err(format!("{:?} is not a batch id", receipt.batch));
        }
        let run = self
            .run(&receipt.run)
            .ok_or_else(|| unknown_run(&receipt.run))?;
        let held = batches(run);
        let before = held.iter().find(|one| one.batch == receipt.batch).cloned();
        if let Some(state) = &before
            && last_record(run, &receipt.batch).is_some_and(|last| {
                last.generation == receipt.generation
                    && last.by == receipt.by
                    && last.record == receipt.record
            })
        {
            return Ok(BatchWritten {
                letters: Vec::new(),
                state: state.clone(),
            });
        }
        admit(run, &held, receipt)?;
        let task = match &before {
            Some(state) => state.task.clone(),
            None => self.open_batch_task(receipt, now_ms)?,
        };
        let mut letters = Vec::new();
        let mut state = before;
        for record in derived(state.as_ref(), receipt, now_ms) {
            let line = line_of(&record, state.as_ref());
            letters.push(self.write_batch_record(&receipt.run, &task, &record, line, now_ms)?);
            state = self
                .run(&receipt.run)
                .and_then(|run| batch_state(run, &receipt.batch));
        }
        let state = state.ok_or_else(|| format!("batch {} was not written", receipt.batch))?;
        match &receipt.record {
            Record::Promoted {
                from, to, members, ..
            } => {
                let tasks: Vec<&str> = members.iter().map(|landed| landed.task.as_str()).collect();
                let summary = serde_json::json!({
                    "batch": receipt.batch,
                    "from": from,
                    "to": to,
                    "tasks": tasks,
                });
                self.update_task(
                    &receipt.run,
                    &task,
                    Some(TaskStatus::Completed),
                    Some(summary.to_string()),
                    ResultAuthor::Ledger,
                    now_ms,
                )?;
                if let PressedBy::Coordinator {
                    seat, generation, ..
                } = &receipt.by
                    && self
                        .run(&receipt.run)
                        .and_then(Run::coordinator_live)
                        .is_some_and(|held| held.seat == *seat && held.generation == *generation)
                {
                    let tasks = self.merged_by_promote(
                        &receipt.run,
                        members,
                        to,
                        seat,
                        *generation,
                        now_ms,
                    )?;
                    if !tasks.is_empty() {
                        let record = BatchRecord {
                            batch: receipt.batch.clone(),
                            generation: receipt.generation,
                            by: receipt.by.clone(),
                            at: now_ms,
                            record: Record::MergedRecorded { tasks },
                        };
                        let line = line_of(&record, Some(&state));
                        letters.push(self.write_batch_record(
                            &receipt.run,
                            &task,
                            &record,
                            line,
                            now_ms,
                        )?);
                    }
                }
            }
            Record::Cancelled {} => {
                self.close_task(
                    &receipt.run,
                    &task,
                    Closure::Outdated {
                        why: format!("cancelled by {}", receipt.by.said()).into(),
                    },
                )?;
            }
            _ => {}
        }
        let state = self
            .run(&receipt.run)
            .and_then(|run| batch_state(run, &receipt.batch))
            .unwrap_or(state);
        Ok(BatchWritten { letters, state })
    }

    /// The task a batch's letters are written against: titled after its members, held
    /// blocked so no beat sends a worker at it, and written by the ledger itself.
    fn open_batch_task(
        &mut self,
        receipt: &LandBatchReceipt,
        now_ms: i64,
    ) -> Result<String, String> {
        let Record::Chosen { members, .. } = &receipt.record else {
            return Err(format!(
                "run {} has no batch {}",
                receipt.run, receipt.batch
            ));
        };
        let names: Vec<&str> = members.iter().map(|member| member.task.as_str()).collect();
        let title = format!("묶음 {}: {}", receipt.batch, names.join(", "));
        let spec = format!(
            "Integration batch {}: stacks {} on the base branch as one merge each, gates the tip \
             once and fast-forwards the base branch when it is green. Written by the ledger; no \
             worker carries it.",
            receipt.batch,
            names.join(", ")
        );
        let task = self.create_task(&receipt.run, spec, title, Vec::new(), None, now_ms)?;
        self.update_task(
            &receipt.run,
            &task,
            Some(TaskStatus::Blocked),
            None,
            ResultAuthor::Ledger,
            now_ms,
        )?;
        Ok(task)
    }

    fn write_batch_record(
        &mut self,
        run_id: &str,
        task: &str,
        record: &BatchRecord,
        line: String,
        now_ms: i64,
    ) -> Result<String, String> {
        let run = self.run(run_id).ok_or_else(|| unknown_run(run_id))?;
        let draft = Draft {
            from: LEDGER_ITSELF.to_string(),
            to: run.address(),
            kind: MessageKind::LandCheck,
            body: line.into(),
            subject: Text::default(),
            priority: Priority::Normal,
            payload: record.payload().to_string().into(),
            thread: None,
            task: Some(task.to_string()),
            dispatch: None,
        };
        self.post(run_id, draft, now_ms)
    }

    /// The merged records of a promote the coordinator pressed, written by the same press as
    /// that seat: each member whose newest attempt still hands in the head that landed.
    fn merged_by_promote(
        &mut self,
        run_id: &str,
        landed: &[Landed],
        main_at: &str,
        seat: &str,
        generation: u32,
        now_ms: i64,
    ) -> Result<Vec<String>, String> {
        let mut written = Vec::new();
        for one in landed {
            let Some(run) = self.run(run_id) else {
                break;
            };
            let Some(attempt) = run.newest_attempt(&one.task) else {
                continue;
            };
            if !attempt
                .source
                .as_deref()
                .is_some_and(|source| same_source(source, &one.head))
            {
                continue;
            }
            let attempt = attempt.id.clone();
            let Some(result) = merged_result(run, one, main_at) else {
                continue;
            };
            let author = ResultAuthor::Coordinator {
                seat: seat.to_string(),
                generation: Some(generation),
                attempt: Some(attempt),
                source: Some(one.head.clone()),
                completed_ms: None,
                verified_ms: None,
            };
            self.update_task(run_id, &one.task, None, Some(result), author, now_ms)?;
            written.push(one.task.clone());
        }
        Ok(written)
    }
}
