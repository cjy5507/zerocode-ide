//! 첫 실행을 한 번만 묻고, 이미 쓰던 사람에게는 아예 묻지 않는다.
//!
//! Orca의 판정은 필드 하나다 — `onboarding.closedAt === null`이면 마법사를
//! 띄우고, 아니면 안 띄운다(`shouldShowOnboarding`,
//! docs/reverse/orca-ui-inventory.md 1-ea). 불리언 대신 타임스탬프를 쓰는
//! 선택은 공짜로 "언제 닫았나"를 남기므로 그대로 가져왔다.
//!
//! 진짜 값은 그 한 줄이 아니라 **저장된 값이 없을 때** 무엇을 답하느냐에
//! 있다. Orca는 데이터 파일의 존재 여부로 두 사람을 가른다: 파일이 아예 없으면
//! 진짜 신규 설치라 기본값(`closedAt: null`)을 주고, 파일은 있는데 `onboarding`
//! 키만 없으면 온보딩 도입 이전부터 쓰던 사람이므로 **이미 끝낸 것으로
//! 조작한다**(`normalizeLoadedOnboardingState`). 이 구분이 없으면 업그레이드
//! 한 번이 모든 기존 사용자에게 마법사를 다시 띄운다.
//!
//! 그리고 `flowVersion`은 **재표시의 근거가 아니다**. 판이 올라가도 이미 닫힌
//! 상태는 닫힌 채로 남고, 버전은 오직 "중도 이탈한 사람을 어느 단계부터 다시
//! 보여줄까"를 정할 때만 읽힌다. 버전 비교로 마법사를 다시 여는 코드는 Orca에
//! 존재하지 않는다.
//!
//! 여기 있는 것은 전부 값에 대한 결정이다. 파일도, 시계도, 창도 이 모듈이
//! 만지지 않는다 — 저장은 셸의 몫이고, 무엇이 "보여야 할 상태"인지는 여기서
//! 한 번만 정해져서 창과 백엔드가 같은 답을 읽는다.

use serde::{Deserialize, Serialize};

/// 이 마법사 판의 번호. 단계 구성이 바뀌는 날 올린다.
pub const FLOW_VERSION: u32 = 1;

/// 마지막 단계의 번호. 선언된 단계 수와 같다 — 완료는 이 값으로 적힌다.
pub const FINAL_STEP: i32 = 4;

/// 아직 아무 단계도 끝내지 않았다.
pub const NO_STEP: i32 = -1;

/// 끝까지 갔다(또는 프로젝트 설정으로 건너뛰었다).
pub const OUTCOME_COMPLETED: &str = "completed";

/// 중간에 나갔다. 진행도는 지워지고 다음 실행에 다시 뜨지 않는다.
pub const OUTCOME_DISMISSED: &str = "dismissed";

/// 사람이 이미 한 일들, 이 창이 실제로 지켜본 것만.
///
/// Orca의 체크리스트는 열두 칸인데 그중 여덟은 어디에서도 `true`가 되지
/// 않는다(1-ea). 그래서 규칙은 하나다 — **켜는 코드가 있는 칸만 존재한다.**
/// 여기 있는 칸은 전부 창의 어떤 이벤트 지점 하나가 켜고, 전부 어딘가가
/// 읽는다(체크리스트 단계이거나 팁의 자동 완료 조건이다). 읽는 데가 없는 칸은
/// 저장 용량이지 사실이 아니다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checklist {
    /// 기본 에이전트를 골랐다.
    #[serde(default)]
    pub chose_agent: bool,
    /// 마법사를 끝내지 않고 나갔다.
    #[serde(default)]
    pub dismissed: bool,
    /// 시험 알림이 실제로 도착했다 — 마법사의 알림 단계가 켠다.
    #[serde(default)]
    pub notified: bool,
    /// 같은 워크스페이스에서 두 번째 에이전트를 띄웠다.
    ///
    /// **첫 번째**가 아니라 두 번째인 것에 이유가 있다: 이 창은 부팅·프로젝트
    /// 전환·워크스페이스 활성화마다 기본 에이전트를 자동으로 띄우므로,
    /// "첫 에이전트를 실행했다"는 칸은 아무도 누르지 않아도 언제나 켜져 있다.
    /// 언제나 켜진 칸은 아무것도 가르치지 않는다.
    #[serde(default)]
    pub ran_second_agent: bool,
    /// diff를 열어 봤다.
    #[serde(default)]
    pub reviewed_diff: bool,
    /// 풀 리퀘스트를 만들었다.
    #[serde(default)]
    pub opened_pr: bool,
    /// 빠른 열기를 써 봤다 — 팔레트 팁의 자동 완료 조건.
    #[serde(default)]
    pub used_palette: bool,
    /// 패널 폭을 자기 손으로 바꿨다 — 패널 팁의 자동 완료 조건.
    #[serde(default)]
    pub shaped_sidebar: bool,
    /// 파일을 열어 봤다 — 편집기 팁의 자동 완료 조건.
    #[serde(default)]
    pub opened_file: bool,
}

/// 창이 켤 수 있는 칸의 이름들. 창은 이 문자열로 말하고, 모르는 이름은
/// [`marked`]가 **거절한다**.
pub const MARK_NOTIFIED: &str = "notified";
pub const MARK_SECOND_AGENT: &str = "ran_second_agent";
pub const MARK_REVIEWED_DIFF: &str = "reviewed_diff";
pub const MARK_OPENED_PR: &str = "opened_pr";
pub const MARK_USED_PALETTE: &str = "used_palette";
pub const MARK_SHAPED_SIDEBAR: &str = "shaped_sidebar";
pub const MARK_OPENED_FILE: &str = "opened_file";

/// 이 판이 아는 칸 전부, 창이 쓰는 이름 그대로.
pub const MARKS: &[&str] = &[
    MARK_NOTIFIED,
    MARK_SECOND_AGENT,
    MARK_REVIEWED_DIFF,
    MARK_OPENED_PR,
    MARK_USED_PALETTE,
    MARK_SHAPED_SIDEBAR,
    MARK_OPENED_FILE,
];

/// 첫 실행 경험의 전부, 저장된 그대로.
///
/// 모든 필드가 `#[serde(default)]`인 것은 의도다. 한 칸이 못 읽히는 파일이
/// 통째로 버려지면 그 사람은 기본값 — 곧 `closed_at: None` — 을 받고 마법사를
/// 다시 본다. 한 칸의 손상이 "처음 오신 분이군요"가 되어서는 안 된다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Onboarding {
    #[serde(default = "shipped_flow_version")]
    pub flow_version: u32,
    /// 닫힌 시각. `None` 하나가 "아직 보여야 한다"의 전부다.
    #[serde(default)]
    pub closed_at: Option<i64>,
    /// 어떻게 닫혔나. 문자열로 든다 — 모르는 값을 만나면 그 칸만 버리고
    /// 나머지는 살린다.
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default = "no_step")]
    pub last_completed_step: i32,
    #[serde(default)]
    pub checklist: Checklist,
    /// 시작 체크리스트를 사이드바에서 치웠다.
    ///
    /// `checklist.dismissed`와 **다른 사실이다**: 저쪽은 마법사를 도중에
    /// 나갔다는 뜻이고 이쪽은 사이드바의 한 줄을 치웠다는 뜻이다. Orca도 둘을
    /// 따로 든다(`checklist.dismissed` vs `ui.setupGuideSidebarDismissed`).
    #[serde(default)]
    pub guide_dismissed: bool,
    /// 코치마크 투어가 이 프로필에 **자동으로** 뜰 수 있나. 최초 1회 정해져
    /// 영구히 남는다(Orca의 `contextualToursAutoEligible`) — 마법사를 본 적 없는
    /// 사람, 곧 업그레이드로 백필된 기존 사용자에게는 영영 뜨지 않는다.
    /// `None`은 "아직 안 정했다"이고, 그 값은 부팅이 한 번만 채운다.
    #[serde(default)]
    pub tours_auto: Option<bool>,
    /// 이미 뜬 적 있는 투어. 더해지기만 한다.
    #[serde(default)]
    pub tours_seen: Vec<String>,
    /// 이미 뜬 적 있는 팁.
    #[serde(default)]
    pub tips_seen: Vec<String>,
    /// 기능 투어에서 읽은 갈래.
    #[serde(default)]
    pub wall_seen: Vec<String>,
}

fn shipped_flow_version() -> u32 {
    FLOW_VERSION
}

fn no_step() -> i32 {
    NO_STEP
}

impl Default for Onboarding {
    fn default() -> Self {
        Self {
            flow_version: FLOW_VERSION,
            closed_at: None,
            outcome: None,
            last_completed_step: NO_STEP,
            checklist: Checklist::default(),
            guide_dismissed: false,
            tours_auto: None,
            tours_seen: Vec::new(),
            tips_seen: Vec::new(),
            wall_seen: Vec::new(),
        }
    }
}

/// 이미 끝낸 것으로 적힌 상태.
///
/// 업그레이드로 처음 이 기능을 만난 사람에게 주는 답이고, 개발용 억제 스위치가
/// 쓸 답이기도 하다. 완료로 **조작**하는 것이지 완료를 기록하는 것이 아니므로
/// 체크리스트는 비어 있다 — 이 사람은 실제로 아무 단계도 밟지 않았다.
#[must_use]
pub fn completed_at(now: i64) -> Onboarding {
    Onboarding {
        flow_version: FLOW_VERSION,
        closed_at: Some(now),
        outcome: Some(OUTCOME_COMPLETED.to_string()),
        last_completed_step: FINAL_STEP,
        checklist: Checklist::default(),
        ..Onboarding::default()
    }
}

/// 디스크에서 읽은 것을 이 실행이 쓸 상태로.
///
/// `profile_existed`는 **우리 상태 디렉터리가 이미 있었는가**이고, 이 프로세스가
/// 무엇이든 쓰기 전에 재어야 한다(Orca의 `fileExistedOnLoad`). 그 답이
/// 신규 설치와 업그레이드를 가르는 유일한 근거다.
#[must_use]
pub fn on_load(stored: Option<Onboarding>, profile_existed: bool, now: i64) -> Onboarding {
    match stored {
        Some(found) => normalized(found),
        // 파일은 있었는데 이 키만 없다 = 이 기능 이전부터 쓰던 사람.
        None if profile_existed => completed_at(now),
        // 우리를 처음 여는 기계.
        None => Onboarding::default(),
    }
}

/// 마법사를 띄울까. 이 한 줄이 판정의 전부다.
#[must_use]
pub fn should_show(state: &Onboarding) -> bool {
    state.closed_at.is_none()
}

/// 다시 열었을 때 어느 단계부터 보여줄까.
///
/// 저장된 진행도의 **다음** 단계이며, 판이 다른 파일에서 온 값은
/// [`remap_legacy_last_completed_step`]이 먼저 우리 눈금으로 옮긴다.
#[must_use]
pub fn resume_step(state: &Onboarding) -> i32 {
    let mapped = remap_legacy_last_completed_step(
        state.flow_version,
        state.last_completed_step,
        state.outcome.as_deref(),
    );
    if mapped >= FINAL_STEP {
        return 0;
    }
    mapped.max(NO_STEP) + 1
}

/// 다른 판이 적어 둔 진행도를 이 판의 눈금으로.
///
/// Orca의 `remapLegacyOnboardingLastCompletedStep`을 이 판의 단계 수에 맞춘
/// 것이다. 두 규칙만 남는다: **끝냈다고 적힌 사람은 끝난 것**이고(마지막 두
/// 단계에 도달했으면 완료로 읽는다 — 판마다 마지막 단계가 갈라지는 자리다),
/// 판이 다르면 그 숫자가 우리 단계를 가리킨다고 믿을 수 없으므로 **마지막
/// 직전까지만** 인정한다. 다시 띄우는 일은 여기서 일어나지 않는다 — 이 함수는
/// `closed_at`을 읽지도 쓰지도 않는다.
#[must_use]
pub fn remap_legacy_last_completed_step(
    flow_version: u32,
    last_completed_step: i32,
    outcome: Option<&str>,
) -> i32 {
    if outcome == Some(OUTCOME_COMPLETED) && last_completed_step >= FINAL_STEP - 1 {
        return FINAL_STEP;
    }
    if flow_version != FLOW_VERSION {
        return last_completed_step.clamp(NO_STEP, FINAL_STEP - 1);
    }
    last_completed_step.clamp(NO_STEP, FINAL_STEP)
}

/// 저장된 값을 허용 범위 안으로.
///
/// 파일은 사람이 열어 고칠 수 있는 곳에 있고, 다른 판이 쓴 것일 수도 있다.
/// 범위 밖 단계는 잘리고, 모르는 결말은 **버려지되 닫힌 사실은 남는다** — 결말을
/// 못 읽었다고 해서 다 끝낸 사람에게 마법사를 다시 띄우는 것이 더 나쁜 실패다.
#[must_use]
pub fn normalized(raw: Onboarding) -> Onboarding {
    let outcome = raw
        .outcome
        .as_deref()
        .and_then(sanitized_outcome)
        .map(str::to_string);
    Onboarding {
        last_completed_step: remap_legacy_last_completed_step(
            raw.flow_version,
            raw.last_completed_step,
            outcome.as_deref(),
        ),
        flow_version: FLOW_VERSION,
        // 0 이하의 시각은 시각이 아니다. 닫힌 적 없음과 같이 읽는다.
        closed_at: raw.closed_at.filter(|at| *at > 0),
        outcome,
        checklist: raw.checklist,
        guide_dismissed: raw.guide_dismissed,
        tours_auto: raw.tours_auto,
        tours_seen: raw.tours_seen,
        tips_seen: raw.tips_seen,
        wall_seen: raw.wall_seen,
    }
}

/// 한 칸을 켠다. 켜기만 한다 — 끄는 말은 이 문을 지나지 않는다.
///
/// 모르는 이름은 [`sanitized_step`]과 같은 이유로 **거절한다**: 창이 못
/// 알아들을 이름을 보냈다는 것은 창의 버그이고, 가장 가까운 칸을 골라 켜면 그
/// 버그가 사람의 진행도로 굳는다.
#[must_use]
pub fn marked(current: &Onboarding, mark: &str) -> Option<Onboarding> {
    let mut checklist = current.checklist;
    match mark {
        MARK_NOTIFIED => checklist.notified = true,
        MARK_SECOND_AGENT => checklist.ran_second_agent = true,
        MARK_REVIEWED_DIFF => checklist.reviewed_diff = true,
        MARK_OPENED_PR => checklist.opened_pr = true,
        MARK_USED_PALETTE => checklist.used_palette = true,
        MARK_SHAPED_SIDEBAR => checklist.shaped_sidebar = true,
        MARK_OPENED_FILE => checklist.opened_file = true,
        _ => return None,
    }
    Some(Onboarding {
        checklist,
        ..current.clone()
    })
}

/// 사이드바의 체크리스트 한 줄을 치우거나 되돌린다.
#[must_use]
pub fn guide_dismissed(current: &Onboarding, dismissed: bool) -> Onboarding {
    Onboarding {
        guide_dismissed: dismissed,
        ..current.clone()
    }
}

/// 투어가 자동으로 뜰 수 있는 프로필인가를 **한 번만** 정한다.
///
/// 이미 정해져 있으면 그대로 둔다. 매 실행 다시 정하면 마법사를 끝낸 다음
/// 실행부터는 언제나 "자격 없음"이 되어 투어가 아무에게도 뜨지 않는다 —
/// `settle_onboarding`이 첫 실행 판정에서 겪는 것과 같은 함정이다.
#[must_use]
pub fn settled_tour_eligibility(current: &Onboarding) -> Option<Onboarding> {
    if current.tours_auto.is_some() {
        return None;
    }
    Some(Onboarding {
        tours_auto: Some(should_show(current)),
        ..current.clone()
    })
}

/// 이미 본 것으로 적는다. 목록은 더해지기만 하고 중복은 생기지 않는다.
///
/// 이미 들어 있으면 `None` — 같은 값을 다시 적는 파일 쓰기는 아무것도 바꾸지
/// 않으면서 실패할 수만 있다.
#[must_use]
pub fn seen(held: &[String], id: &str) -> Option<Vec<String>> {
    if held.iter().any(|one| one == id) {
        return None;
    }
    let mut next = held.to_vec();
    next.push(id.to_string());
    Some(next)
}

/// 우리가 아는 결말인가.
#[must_use]
pub fn sanitized_outcome(said: &str) -> Option<&'static str> {
    match said {
        OUTCOME_COMPLETED => Some(OUTCOME_COMPLETED),
        OUTCOME_DISMISSED => Some(OUTCOME_DISMISSED),
        _ => None,
    }
}

/// 우리가 아는 단계 번호인가. 범위 밖은 **자르지 않고 거절한다** — 창이 보낸
/// 값이고, 못 알아들은 값을 근사해서 저장하면 그 창의 버그가 진행도로 남는다.
#[must_use]
pub fn sanitized_step(step: i32) -> Option<i32> {
    (NO_STEP..=FINAL_STEP).contains(&step).then_some(step)
}

/// 한 단계를 끝냈다고 적을 때의 상태.
///
/// 체크는 켜지기만 한다. 같은 단계를 다시 지나며 `false`를 들고 오는 호출이
/// 이미 켜 둔 사실을 끄면, 그것은 진행이 아니라 후퇴다.
#[must_use]
pub fn stepped(current: &Onboarding, step: i32, chose_agent: bool) -> Onboarding {
    Onboarding {
        flow_version: FLOW_VERSION,
        last_completed_step: step.max(current.last_completed_step),
        checklist: Checklist {
            chose_agent: current.checklist.chose_agent || chose_agent,
            ..current.checklist
        },
        ..current.clone()
    }
}

/// 마법사가 닫힐 때의 상태.
///
/// 완료는 마지막 단계로, 이탈은 **진행도 초기화**로 적힌다(Orca의 `useCloseWith`
/// 표 그대로). 이탈한 사람이 다음에 이 화면을 다시 볼 일은 없으므로 남는
/// 진행도는 아무에게도 쓸모가 없고, `dismissed` 한 칸이 그 사람을 나중에
/// 알아보는 유일한 표시다.
#[must_use]
pub fn closing(current: &Onboarding, outcome: &str, now: i64) -> Option<Onboarding> {
    let outcome = sanitized_outcome(outcome)?;
    let dismissed = outcome == OUTCOME_DISMISSED;
    Some(Onboarding {
        flow_version: FLOW_VERSION,
        closed_at: Some(now),
        outcome: Some(outcome.to_string()),
        last_completed_step: if dismissed { NO_STEP } else { FINAL_STEP },
        checklist: Checklist {
            dismissed,
            ..current.checklist
        },
        ..current.clone()
    })
}

/// 다시 보기. 도움말의 숨은 항목 하나가 부른다.
///
/// 닫힘과 결말과 진행도를 모두 되돌린다 — 절반만 되돌리면 "다시 보기"가 이전
/// 이탈의 마지막 단계에서 시작하는, 아무도 요청하지 않은 재개가 된다.
#[must_use]
pub fn reopened(current: &Onboarding) -> Onboarding {
    Onboarding {
        flow_version: FLOW_VERSION,
        closed_at: None,
        outcome: None,
        last_completed_step: NO_STEP,
        checklist: Checklist {
            dismissed: false,
            ..current.checklist
        },
        // 본 적 있는 투어와 팁은 그대로 둔다. 마법사를 다시 보는 것과 이미
        // 읽은 코치마크를 다시 받는 것은 서로 다른 요청이고, 아무도 뒤엣것을
        // 하지 않았다.
        ..current.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 파일이 없다는 답 하나가 두 사람을 가른다.
    ///
    /// 이 세 줄이 이 모듈의 존재 이유다. 가운데 줄이 없으면 업그레이드 한 번이
    /// 모든 기존 사용자에게 마법사를 다시 띄운다.
    #[test]
    fn a_missing_record_means_a_new_machine_or_an_old_user_and_never_the_same_answer() {
        let fresh = on_load(None, false, 1_700_000_000_000);
        assert!(should_show(&fresh), "새 기계가 마법사를 못 본다");
        assert_eq!(fresh.closed_at, None);
        assert_eq!(fresh.last_completed_step, NO_STEP);

        let carried = on_load(None, true, 1_700_000_000_000);
        assert!(
            !should_show(&carried),
            "이미 쓰던 사람에게 마법사가 다시 떴다"
        );
        assert_eq!(carried.closed_at, Some(1_700_000_000_000));
        assert_eq!(carried.outcome.as_deref(), Some(OUTCOME_COMPLETED));
        assert_eq!(carried.last_completed_step, FINAL_STEP);

        let closed = Onboarding {
            closed_at: Some(42),
            outcome: Some(OUTCOME_DISMISSED.to_string()),
            ..Onboarding::default()
        };
        let kept = on_load(Some(closed), true, 1_700_000_000_000);
        assert!(!should_show(&kept));
        assert_eq!(kept.closed_at, Some(42), "닫힌 시각이 덮어써졌다");
    }

    /// 판이 올라가도 닫힌 것은 닫힌 채로 남는다.
    ///
    /// 재매핑은 재개 단계를 정할 뿐이고, 어디에서도 `closed_at`을 비우지
    /// 않는다 — 버전 비교로 마법사를 다시 여는 길이 있는지 묻는 시험이다.
    #[test]
    fn a_newer_flow_version_never_reopens_what_was_closed() {
        let old = Onboarding {
            flow_version: 0,
            closed_at: Some(99),
            outcome: Some(OUTCOME_COMPLETED.to_string()),
            last_completed_step: 9,
            ..Onboarding::default()
        };
        let now = on_load(Some(old), true, 1_700_000_000_000);

        assert!(!should_show(&now));
        assert_eq!(now.closed_at, Some(99));
        assert_eq!(now.flow_version, FLOW_VERSION, "판 번호가 갱신되지 않았다");
    }

    /// 다른 판이 적어 둔 진행도를 옮기는 표.
    #[test]
    fn a_foreign_flow_versions_progress_is_read_onto_this_ones_scale() {
        // 끝냈다고 적힌 사람은, 어느 판에서든 끝난 것이다.
        assert_eq!(
            remap_legacy_last_completed_step(0, FINAL_STEP - 1, Some(OUTCOME_COMPLETED)),
            FINAL_STEP
        );
        assert_eq!(
            remap_legacy_last_completed_step(7, 99, Some(OUTCOME_COMPLETED)),
            FINAL_STEP
        );
        // 판이 다르면 마지막 직전까지만 인정한다: 그 숫자가 우리 마지막 단계를
        // 가리킨다고 믿으면 아무 단계도 안 본 사람이 완료로 적힌다.
        assert_eq!(
            remap_legacy_last_completed_step(0, FINAL_STEP, None),
            FINAL_STEP - 1
        );
        assert_eq!(
            remap_legacy_last_completed_step(0, 99, None),
            FINAL_STEP - 1
        );
        assert_eq!(remap_legacy_last_completed_step(0, -8, None), NO_STEP);
        // 같은 판이면 그대로, 범위만 지킨다.
        assert_eq!(
            remap_legacy_last_completed_step(FLOW_VERSION, 2, None),
            2,
            "같은 판의 진행도가 잘렸다"
        );
        assert_eq!(
            remap_legacy_last_completed_step(FLOW_VERSION, 99, None),
            FINAL_STEP
        );
    }

    /// 저장된 값이 허용 범위 밖이면 거절되거나 잘린다.
    #[test]
    fn a_value_outside_the_range_is_refused_rather_than_stored() {
        assert_eq!(sanitized_step(NO_STEP), Some(NO_STEP));
        assert_eq!(sanitized_step(FINAL_STEP), Some(FINAL_STEP));
        assert_eq!(sanitized_step(FINAL_STEP + 1), None);
        assert_eq!(sanitized_step(-2), None);

        assert_eq!(sanitized_outcome("completed"), Some(OUTCOME_COMPLETED));
        assert_eq!(sanitized_outcome("dismissed"), Some(OUTCOME_DISMISSED));
        assert_eq!(sanitized_outcome("skipped"), None);
        assert_eq!(sanitized_outcome(""), None);

        // 모르는 결말은 그 칸만 버리고, 닫힌 사실은 남는다.
        let odd = normalized(Onboarding {
            closed_at: Some(5),
            outcome: Some("whatever".to_string()),
            ..Onboarding::default()
        });
        assert_eq!(odd.outcome, None);
        assert!(!should_show(&odd), "결말 한 칸 때문에 마법사가 다시 떴다");
        // 시각이 아닌 시각은 닫힌 적 없음과 같다.
        assert!(should_show(&normalized(Onboarding {
            closed_at: Some(0),
            ..Onboarding::default()
        })));
    }

    /// 완료와 이탈은 서로 다른 것을 남긴다.
    #[test]
    fn finishing_and_walking_out_are_written_down_differently() {
        let mid = stepped(&Onboarding::default(), 1, true);
        assert_eq!(mid.last_completed_step, 1);
        assert!(mid.checklist.chose_agent);
        // 체크는 켜지기만 한다.
        assert!(stepped(&mid, 2, false).checklist.chose_agent);
        // 진행도는 뒤로 가지 않는다 — 건너뛴 단계를 지나 되돌아온 화면이
        // 방금 지난 자리를 지워서는 안 된다.
        assert_eq!(stepped(&mid, 0, false).last_completed_step, 1);

        let done = closing(&mid, OUTCOME_COMPLETED, 1_700_000_000_000).expect("완료가 거절됐다");
        assert_eq!(done.last_completed_step, FINAL_STEP);
        assert_eq!(done.closed_at, Some(1_700_000_000_000));
        assert!(!done.checklist.dismissed);
        assert!(done.checklist.chose_agent, "고른 에이전트가 지워졌다");

        let left = closing(&mid, OUTCOME_DISMISSED, 1_700_000_000_000).expect("이탈이 거절됐다");
        assert_eq!(left.last_completed_step, NO_STEP);
        assert!(left.checklist.dismissed);

        assert!(
            closing(&mid, "abandoned", 1).is_none(),
            "모르는 결말이 적혔다"
        );
    }

    /// 창이 켜는 칸은 이름으로 오고, 모르는 이름은 거절된다.
    ///
    /// 근사해서 가장 가까운 칸을 켜면 창의 오타 하나가 사람의 진행도로 굳는다 —
    /// 그리고 그 진행도는 아무도 되돌릴 수 없다.
    #[test]
    fn a_box_no_gesture_answers_to_is_refused_rather_than_guessed() {
        let fresh = Onboarding::default();
        for mark in MARKS {
            assert!(
                marked(&fresh, mark).is_some(),
                "{mark}은(는) 목록에 있는데 거절된다"
            );
        }
        assert!(
            marked(&fresh, "rode_a_horse").is_none(),
            "모르는 칸이 켜졌다"
        );
        assert!(marked(&fresh, "").is_none());
        // 마법사만 켜는 둘은 이 문으로 오지 않는다: 하나는 단계 저장이, 다른
        // 하나는 닫기가 켠다.
        assert!(marked(&fresh, "chose_agent").is_none());
        assert!(marked(&fresh, "dismissed").is_none());

        // 켜기만 한다. 이미 켠 것을 지나며 다른 칸을 켜도 앞의 것은 남는다.
        let read = marked(&fresh, MARK_REVIEWED_DIFF).expect("칸이 거절됐다");
        let both = marked(&read, MARK_OPENED_PR).expect("칸이 거절됐다");
        assert!(both.checklist.reviewed_diff && both.checklist.opened_pr);
    }

    /// 다시 보기는 처음부터다.
    #[test]
    fn reopening_starts_at_the_first_step_and_not_where_somebody_left() {
        let left = closing(&Onboarding::default(), OUTCOME_DISMISSED, 5).expect("이탈");
        let again = reopened(&left);

        assert!(should_show(&again));
        assert_eq!(again.outcome, None);
        assert_eq!(again.last_completed_step, NO_STEP);
        assert!(!again.checklist.dismissed);
        assert_eq!(resume_step(&again), 0);
    }

    /// 중도 이탈이 아니라 중도 진행 상태에서 다시 열면 그다음 단계부터다.
    #[test]
    fn a_half_finished_flow_resumes_on_the_step_after_the_last_one_done() {
        assert_eq!(resume_step(&Onboarding::default()), 0);
        assert_eq!(resume_step(&stepped(&Onboarding::default(), 2, false)), 3);
        // 끝까지 간 상태를 다시 열면 처음부터 — 마지막 단계 다음은 없다.
        let done = closing(&Onboarding::default(), OUTCOME_COMPLETED, 5).expect("완료");
        assert_eq!(resume_step(&done), 0);
    }
}
