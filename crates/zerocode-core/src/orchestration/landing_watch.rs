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

/// 코디네이터에게 늦은 착지를 알리는 까닭 다섯 (t-22105 a~e).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallReason {
    NoReview,
    NoMerge,
    MergedUnrecorded,
    Cleanable,
    LedgerMergedGitNot,
}

impl StallReason {
    pub const fn word(self) -> &'static str {
        match self {
            Self::NoReview => "no_review",
            Self::NoMerge => "no_merge",
            Self::MergedUnrecorded => "merged_unrecorded",
            Self::Cleanable => "cleanable",
            Self::LedgerMergedGitNot => "ledger_merged_git_not",
        }
    }
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

/// 이 과업·까닭·머리를 코디네이터에게 마지막으로 알린 때가 [`RENOTIFY_MS`] 안인가.
fn told_lately(run: &Run, task: &str, reason: StallReason, head: &str, now_ms: i64) -> bool {
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
        .is_some_and(|last| now_ms.saturating_sub(last) < RENOTIFY_MS)
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
        if review.verified
            && !review.merged
            && let Some(verified_at) = review.verified_ms
            && now_ms.saturating_sub(verified_at) >= MERGE_OVERDUE_MS
        {
            reasons.push((StallReason::NoMerge, verified_at));
        }
        if !review.verified && now_ms.saturating_sub(done_ms) >= VERIFY_OVERDUE_MS {
            reasons.push((StallReason::NoReview, done_ms));
        }

        for (reason, since_ms) in reasons {
            if told_lately(run, &task.id, reason, source, now_ms) {
                continue;
            }
            let told = serde_json::json!({
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
