//! 늦은 착지를 코디네이터에게, 뒤처진 브랜치를 워커에게 알린다 (t-34501 2단계, t-22105).
//!
//! 원장은 워커가 보고했는지, 코디네이터가 검증하고 병합했다고 적었는지를 안다. git은 모른다.
//! 창은 git을 안다(사이드바의 분류). 이 파일은 둘을 **맞대어 보고**, 어긋나거나 오래 멈춘 것을
//! 알린다. 판정은 시계를 인자로 받는 순수 함수라 가짜 시계로 시험한다.
//!
//! 알리는 것과 보이는 것까지만 한다 — 병합도 삭제도 push도 하지 않는다.
//!
//! 이 파일은 빨강 단계의 빈 모양이다: 자료의 모양과 이름만 있고 판정은 비어 있다.

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
    pub fn landing_watch(&mut self, _witnesses: &[LandingWitness], _now_ms: i64) -> usize {
        0
    }
}
