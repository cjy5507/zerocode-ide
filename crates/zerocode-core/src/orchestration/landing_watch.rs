//! 늦은 착지를 코디네이터에게, 뒤처진 브랜치를 워커에게 알린다 (t-34501 2단계, t-22105).
//!
//! 원장은 워커가 보고했는지, 코디네이터가 검증하고 병합했다고 적었는지를 안다. git은 모른다.
//! 창은 git을 안다(사이드바의 분류). 이 파일은 둘을 **맞대어 보고**, 어긋나거나 오래 멈춘 것을
//! 알린다. 판정은 시계를 인자로 받는 순수 함수라 가짜 시계로 시험한다.
//!
//! 알리는 것과 보이는 것까지만 한다 — 병합도 삭제도 push도 하지 않는다.

use super::*;

/// 보고했는데 검증 기록이 이만큼 없으면 알린다 (a).
pub const VERIFY_OVERDUE_MS: i64 = 2 * 60 * 60 * 1000;
/// 검증됐는데 병합 기록이 이만큼 없으면 알린다 (b).
pub const MERGE_OVERDUE_MS: i64 = 2 * 60 * 60 * 1000;
/// git으로는 main에 들어갔는데 원장에 병합 기록이 이만큼 없으면 알린다 (c).
pub const MERGED_UNRECORDED_GRACE_MS: i64 = 30 * 60 * 1000;
/// 병합된 작업의 폴더가 세션 없이 이만큼 남아 있으면 알린다 (d).
pub const CLEANABLE_AFTER_MS: i64 = 60 * 60 * 1000;
/// 풀리지 않는 상태는 이만큼마다 한 번 다시 알린다.
pub const RENOTIFY_MS: i64 = 6 * 60 * 60 * 1000;
/// 한 시도에게 뒤처짐을 알리는 횟수의 상한.
pub const DRIFT_PER_DISPATCH_MAX: usize = 10;
/// 이보다 오래된 과업은 알리지 않는다 — 옛 기록이 한꺼번에 알림이 되지 않게.
pub const HORIZON_MS: i64 = 3 * 24 * 60 * 60 * 1000;
/// 코디네이터가 읽기만 하는 일 — 검수 — 에 붙이는 제목의 표식 (t-78890). 커밋을 넘기지 않은
/// 과업에만 읽는다: 제목만으로는 커밋을 숨기지 않는다. 낱말로 맞춘다(`title_says_review`):
/// 영어 표식은 대소문자를 가리지 않고 「preview」·「reviewer」에 걸리지 않는다.
pub const REVIEW_TITLE_MARKS: &[&str] = &["검수", "읽기 전용", "read-only", "review", "audit"];
/// 넘김이 검수인 까닭 — 워커가 `reportKind: review`라고 말했다.
pub const REVIEW_BY_KIND: &str = "reportKind review";
/// 넘김이 검수인 까닭 — 커밋 없는 넘김에 제목이 검수라고 말한다.
pub const REVIEW_BY_TITLE: &str = "read-only title";
/// 커밋을 넘긴 검수의 검증 결과에 그 커밋을 적는 키 — 착지할 머리가 아니라 검수한 머리다.
pub const REVIEWED_HEAD_KEY: &str = "reviewedHead";

/// 창이 한 작업 폴더에 대해 본 git의 사실.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LandingWitness {
    pub checkout: String,
    pub branch: Option<String>,
    /// 분류가 선 머리.
    pub head: String,
    pub compare_ref: String,
    pub compare_oid: String,
    pub git: GitSays,
    /// 지금 그 폴더 안에 판(세션)이 있다.
    pub occupied: bool,
    /// 이 폴더에서 머리를 넘긴 과업의 다른 체크아웃 — 워커가 곁에 만들어 두고 넘길 때 거두지
    /// 않은 red·verify·증거용 체크아웃 — 으로, 창이 넘김을 처음 볼 때 본 경로들(t-45336).
    /// 비었으면 남긴 것이 없거나 창이 아직 보지 않았다.
    pub beside: Vec<String>,
}

/// git이 말한 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitSays {
    /// 모든 커밋이 비교 ref에 들어 있다. `at_ms`는 main이 담은 때(알 때만).
    Landed { at_ms: Option<i64> },
    /// 들어가지 않았다.
    Unlanded {
        behind: Option<u32>,
        far_behind: bool,
        conflict: Option<ConflictCell>,
    },
    /// 그 밖(커밋 없음·모름·기준 없음·실패): 아무 말도 하지 않는다.
    Silent,
}

/// 합쳐 본 결과의 칸: `total` 0은 깨끗함.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictCell {
    pub total: u32,
    pub files: Vec<String>,
}

/// 코디네이터에게 늦은 착지를 알리는 까닭 다섯 (t-22105 a~e), 그리고 넘길 때 남긴 체크아웃
/// (t-45336).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallReason {
    NoReview,
    NoMerge,
    MergedUnrecorded,
    Cleanable,
    LedgerMergedGitNot,
    /// 워커가 머리를 넘기면서 같은 과업의 다른 체크아웃을 곁에 남겼다 — 워커 공통 규칙
    /// 「과업당 워크트리 하나, 증거용 분리 체크아웃은 넘김 전에 스스로 거둔다」를 어긴 것.
    CheckoutsLeft,
}

impl StallReason {
    pub const fn word(self) -> &'static str {
        match self {
            Self::NoReview => "no_review",
            Self::NoMerge => "no_merge",
            Self::MergedUnrecorded => "merged_unrecorded",
            Self::Cleanable => "cleanable",
            Self::LedgerMergedGitNot => "ledger_merged_git_not",
            Self::CheckoutsLeft => "checkouts_left",
        }
    }
}

impl Run {
    /// 과업의 가장 새 시도가 넘긴 것이 검수 — 읽기만 하고 착지할 코드가 없는 일 — 인지, 그렇다면 어느
    /// 표식으로 아는지 (t-78890, VERDICT F1). 그 시도를 끝낸 `worker_done`이 payload에
    /// `reportKind: review`라고 말했거나, 커밋이 아닌 보고서를 넘겼는데 제목에
    /// [`REVIEW_TITLE_MARKS`]가 있다. `reportKind: review`는 커밋을 넘겼어도 검수다: 검수 워커가
    /// `head`로 넘기는 것은 검수한 남의 커밋이고, 그 검증에는 그 커밋이 검수 대상
    /// ([`REVIEWED_HEAD_KEY`])으로 적힌다.
    ///
    /// 늦은 착지 알림은 검수에게 착지를 묻지 않고, 코디네이터가 그 검수를 `verified`로 적을 때
    /// `nothingToLand`가 함께 적힌다. 둘 다 이 한 곳에서 읽는다.
    #[must_use]
    pub fn review_hand_in(&self, task: &Task) -> Option<&'static str> {
        let attempt = self.newest_attempt(&task.id)?;
        let source = attempt.source.as_deref()?;
        let said_review = self
            .messages()
            .iter()
            .filter(|held| {
                held.kind == MessageKind::WorkerDone
                    && held.dispatch.as_deref() == Some(attempt.id.as_str())
            })
            .any(|held| {
                serde_json::from_str::<serde_json::Value>(held.payload.as_str())
                    .ok()
                    .and_then(|payload| {
                        payload
                            .get(crate::hand_in::REPORT_KIND_KEY)
                            .and_then(serde_json::Value::as_str)
                            .and_then(crate::hand_in::ReportKind::parse)
                    })
                    == Some(crate::hand_in::ReportKind::Review)
            });
        if said_review {
            return Some(REVIEW_BY_KIND);
        }
        (commit_named(source).is_none() && title_says_review(&task.title))
            .then_some(REVIEW_BY_TITLE)
    }
}

/// 제목에 [`REVIEW_TITLE_MARKS`]의 하나가 낱말로 있는가 (t-78890 r2). 제목과 표식을 글자·숫자가
/// 아닌 것에서 끊어 낱말로 나누고, 표식의 낱말들이 제목에 잇달아 있어야 한다 — 「읽기 전용」은
/// 「읽기-전용」에도, 「read-only」는 「read only」에도 맞는다. 마지막 낱말이 ASCII가 아니면
/// 제목 낱말의 머리만 맞아도 된다: 한국어는 「검수를」·「전용으로」처럼 조사가 붙는다.
fn title_says_review(title: &str) -> bool {
    fn words(text: &str) -> Vec<String> {
        text.split(|one: char| !one.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect()
    }
    let title = words(title);
    REVIEW_TITLE_MARKS.iter().any(|mark| {
        let mark = words(mark);
        let Some((last, before)) = mark.split_last() else {
            return false;
        };
        title.windows(mark.len()).any(|seen| {
            seen.split_last().is_some_and(|(seen_last, seen_before)| {
                seen_before == before
                    && if last.is_ascii() {
                        seen_last == last
                    } else {
                        seen_last.starts_with(last.as_str())
                    }
            })
        })
    })
}

impl Ledger {
    /// 원장의 기록을 git의 사실과 맞대어, 늦은 착지는 코디네이터에게, 뒤처진 워커는 그
    /// 워커에게 알린다. 쓴 줄 수를 답한다.
    ///
    /// 읽기만 하고 아무것도 바꾸지 않는다: 병합도 삭제도 push도 없이 줄 하나를 쓸 뿐이다.
    /// 같은 상태는 다시 쓰지 않고(줄 자체가 장부다), 풀리지 않으면 [`RENOTIFY_MS`]마다 한 번.
    pub fn landing_watch(&mut self, witnesses: &[LandingWitness], now_ms: i64) -> usize {
        if now_ms < 0 {
            return 0;
        }
        let mut drafts: Vec<(String, Draft)> = Vec::new();
        for run in &self.runs {
            late_landings(run, witnesses, now_ms, &mut drafts);
            drifted_workers(run, witnesses, now_ms, &mut drafts);
        }
        let mut told = 0;
        for (run_id, draft) in drafts {
            if self.post(&run_id, draft, now_ms).is_ok() {
                told += 1;
            }
        }
        told
    }
}

/// 같은 커밋인가 — 한쪽이 줄임 이름(일곱 자리 이상)이어도 앞이 같으면 같다.
fn same_head(one: &str, other: &str) -> bool {
    let (Some(one), Some(other)) = (commit_named(one), commit_named(other)) else {
        return false;
    };
    let (one, other) = (one.to_ascii_lowercase(), other.to_ascii_lowercase());
    one.starts_with(&other) || other.starts_with(&one)
}

fn witness_of<'a>(
    witnesses: &'a [LandingWitness],
    checkout: Option<&str>,
) -> Option<&'a LandingWitness> {
    let checkout = checkout?;
    witnesses.iter().find(|held| held.checkout == checkout)
}

/// 이 과업·까닭·머리를 코디네이터에게 마지막으로 알린 때가 `again_after` 안인가.
fn told_lately(
    run: &Run,
    task: &str,
    reason: StallReason,
    head: &str,
    now_ms: i64,
    again_after: i64,
) -> bool {
    run.messages()
        .iter()
        .filter(|held| held.kind == MessageKind::LandingStalled)
        .filter(|held| held.task.as_deref() == Some(task))
        .filter(|held| {
            serde_json::from_str::<serde_json::Value>(held.body.as_str()).is_ok_and(|body| {
                body["reason"] == reason.word() && body["head"].as_str() == Some(head)
            })
        })
        .map(|held| held.created_ms)
        .max()
        .is_some_and(|last| now_ms.saturating_sub(last) < again_after)
}

fn next_step(reason: StallReason, compare_ref: &str) -> String {
    match reason {
        StallReason::NoReview => "the worker's report is waiting for a review: verify it with \
             `task-update --result '{\"verified\":true}' --attempt <dispatch> --source <head>`, \
             or say why not"
            .to_string(),
        StallReason::NoMerge => "verified, but nothing says it landed: land the head and write \
             `mergeHead`, or write `nothingToLand` if there is no code to land"
            .to_string(),
        StallReason::MergedUnrecorded => format!(
            "git has this head in {compare_ref}: record it with `task-update --result \
             '{{\"verified\":true,\"mergeHead\":\"<sha>\"}}'`"
        ),
        StallReason::Cleanable => "the work is in main and nobody is in the checkout: it can be \
             cleaned up from the inactive-workspace review"
            .to_string(),
        StallReason::LedgerMergedGitNot => format!(
            "the ledger says merged, but git does not have this head in {compare_ref}: check \
             what landed before anything is deleted"
        ),
        StallReason::CheckoutsLeft => "the worker handed in with other checkouts of this task \
             still standing beside its own (the rule is one checkout per task, and a checkout cut \
             for evidence goes before the hand-in): the window takes back the ones the handed-in \
             head holds that nobody uses, and the rest stay folded under the task's row"
            .to_string(),
    }
}

/// 한 과업에 대해 지금 알릴 까닭들(아직 알린 적 없는 것만).
fn late_landings(
    run: &Run,
    witnesses: &[LandingWitness],
    now_ms: i64,
    drafts: &mut Vec<(String, Draft)>,
) {
    for task in &run.tasks {
        if task.status != TaskStatus::Completed || task.closed.is_some() {
            continue;
        }
        let review = run.review_of(task);
        if review.unreviewable || review.nothing_to_land {
            continue;
        }
        let Some(attempt) = run.newest_attempt(&task.id) else {
            continue;
        };
        if attempt.succeeded != Some(true) {
            continue;
        }
        let Some(source) = attempt.source.as_deref() else {
            continue;
        };
        let done_ms = run
            .messages()
            .iter()
            .filter(|held| {
                held.kind == MessageKind::WorkerDone
                    && held.dispatch.as_deref() == Some(attempt.id.as_str())
            })
            .map(|held| held.created_ms)
            .max()
            .or(attempt.ended_ms);
        let Some(done_ms) = done_ms else {
            continue;
        };
        if now_ms.saturating_sub(done_ms) > HORIZON_MS {
            continue;
        }
        let worker = run.worker(&attempt.worker);
        let checkout = worker.and_then(|held| held.checkout.as_deref());
        let seen = witness_of(witnesses, checkout).filter(|held| same_head(&held.head, source));
        let git = seen.map(|held| &held.git);
        let ledger_merged_at = review
            .merged
            .then(|| run.completion_ms(task).unwrap_or(done_ms));
        let landed_at = match git {
            Some(GitSays::Landed { at_ms }) => Some(at_ms.unwrap_or(done_ms).max(done_ms)),
            _ => None,
        };
        let compare_ref = seen.map_or("", |held| held.compare_ref.as_str());

        let mut reasons: Vec<(StallReason, i64)> = Vec::new();
        if review.merged && matches!(git, Some(GitSays::Unlanded { .. })) {
            reasons.push((
                StallReason::LedgerMergedGitNot,
                ledger_merged_at.unwrap_or(done_ms),
            ));
        }
        if let Some(at) = landed_at
            && !review.merged
            && now_ms.saturating_sub(at) >= MERGED_UNRECORDED_GRACE_MS
        {
            reasons.push((StallReason::MergedUnrecorded, at));
        }
        if let (Some(witness), Some(merged_at)) = (seen, ledger_merged_at.or(landed_at))
            && !witness.occupied
        {
            let left = attempt.ended_ms.unwrap_or(done_ms).max(merged_at);
            if now_ms.saturating_sub(left) >= CLEANABLE_AFTER_MS {
                reasons.push((StallReason::Cleanable, left));
            }
        }
        // What the worker left beside its checkout when it handed in, as the window saw it — told
        // once, at the hand-in, whatever the window then took back (t-45336).
        let left: &[String] =
            witness_of(witnesses, checkout).map_or(&[][..], |held| held.beside.as_slice());
        if !left.is_empty() {
            reasons.push((StallReason::CheckoutsLeft, done_ms));
        }
        if review.verified
            && !review.merged
            && let Some(verified_at) = review.verified_ms
            && now_ms.saturating_sub(verified_at) >= MERGE_OVERDUE_MS
        {
            reasons.push((StallReason::NoMerge, verified_at));
        }
        // A review the coordinator wrote of this hand-in — `verified: false` with its reason too —
        // answers "nobody looked"; a new attempt or commit clears it (`Run::review_of`).
        if !review.verified
            && !review.written
            && now_ms.saturating_sub(done_ms) >= VERIFY_OVERDUE_MS
        {
            reasons.push((StallReason::NoReview, done_ms));
        }

        // A review lands no code (t-78890): it is told while nobody has read it and for what its
        // worker left beside it, never as unmerged, unrecorded or cleanable work.
        if run.review_hand_in(task).is_some() {
            reasons.retain(|(reason, _)| {
                matches!(reason, StallReason::NoReview | StallReason::CheckoutsLeft)
            });
        }
        for (reason, since_ms) in reasons {
            // A checkout left beside a hand-in is a fact about that hand-in, told once; what is late
            // is told again while it stays late.
            let again_after = if reason == StallReason::CheckoutsLeft {
                i64::MAX
            } else {
                RENOTIFY_MS
            };
            if told_lately(run, &task.id, reason, source, now_ms, again_after) {
                continue;
            }
            let mut told = serde_json::json!({
                "reason": reason.word(),
                "taskId": task.id,
                "title": task.display_name(),
                "workerId": attempt.worker,
                "dispatchId": attempt.id,
                "checkout": checkout,
                "branch": seen.and_then(|held| held.branch.clone()),
                "head": source,
                "sinceMs": since_ms,
                "observedAtMs": now_ms,
                "next": next_step(reason, compare_ref),
            });
            if reason == StallReason::CheckoutsLeft {
                told["beside"] = serde_json::json!(left);
            }
            drafts.push((
                run.id.clone(),
                Draft {
                    from: LEDGER_ITSELF.to_string(),
                    to: run.address(),
                    kind: MessageKind::LandingStalled,
                    body: told.to_string().into(),
                    subject: Text::default(),
                    priority: Priority::Normal,
                    payload: Text::default(),
                    thread: None,
                    task: Some(task.id.clone()),
                    dispatch: Some(attempt.id.clone()),
                },
            ));
        }
    }
}

/// 일하는 중인 워커의 브랜치가 멀리 뒤처졌거나 합치면 충돌하면 그 워커에게 알린다.
fn drifted_workers(
    run: &Run,
    witnesses: &[LandingWitness],
    now_ms: i64,
    drafts: &mut Vec<(String, Draft)>,
) {
    for worker in &run.workers {
        if !worker.state.is_live() || worker.taken_over {
            continue;
        }
        let Some(dispatch) = worker.dispatch.as_deref().and_then(|id| run.dispatch(id)) else {
            continue;
        };
        if !dispatch.is_open() {
            continue;
        }
        let Some(seen) = witness_of(witnesses, worker.checkout.as_deref()) else {
            continue;
        };
        let GitSays::Unlanded {
            behind,
            far_behind,
            conflict,
        } = &seen.git
        else {
            continue;
        };
        let clash = conflict.as_ref().filter(|cell| cell.total > 0);
        if !*far_behind && clash.is_none() {
            continue;
        }
        let earlier: Vec<serde_json::Value> = run
            .messages()
            .iter()
            .filter(|held| {
                held.kind == MessageKind::BranchDrifted
                    && held.dispatch.as_deref() == Some(dispatch.id.as_str())
            })
            .filter_map(|held| serde_json::from_str(held.body.as_str()).ok())
            .filter(|body: &serde_json::Value| body["head"].as_str() == Some(seen.head.as_str()))
            .collect();
        if earlier
            .iter()
            .any(|body| body["compareOid"].as_str() == Some(seen.compare_oid.as_str()))
            || earlier.len() >= DRIFT_PER_DISPATCH_MAX
        {
            continue;
        }
        let told = serde_json::json!({
            "workerId": worker.id,
            "dispatchId": dispatch.id,
            "taskId": dispatch.task,
            "checkout": seen.checkout,
            "branch": seen.branch,
            "head": seen.head,
            "compareRef": seen.compare_ref,
            "compareOid": seen.compare_oid,
            "behind": behind,
            "farBehind": far_behind,
            "conflict": clash.map(|cell| serde_json::json!({ "total": cell.total, "files": cell.files })),
            "observedAtMs": now_ms,
            "next": format!(
                "git fetch, then merge what {} has into this branch and run again the gates you \
                 already ran: they ran on a tree main no longer has",
                seen.compare_ref
            ),
        });
        drafts.push((
            run.id.clone(),
            Draft {
                from: LEDGER_ITSELF.to_string(),
                to: worker_address(&worker.id),
                kind: MessageKind::BranchDrifted,
                body: told.to_string().into(),
                subject: Text::default(),
                priority: Priority::Normal,
                payload: Text::default(),
                thread: None,
                task: Some(dispatch.task.clone()),
                dispatch: Some(dispatch.id.clone()),
            },
        ));
    }
}
