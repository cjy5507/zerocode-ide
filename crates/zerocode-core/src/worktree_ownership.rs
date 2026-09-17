//! 외부 워크트리 가시성 — **이 창이 만들지 않은** git 워크트리를 사이드바에
//! 들일지 정하는 규칙.
//!
//! 축을 먼저 못 박아야 한다. 이것은 워크스페이스 숨김이 아니고, 사이드바 항목
//! 토글도 아니다. `git worktree add`로 **밖에서** 만들어진 체크아웃을 이 창의
//! 목록에 들일지 말지, **저장소마다 하나씩** 있는 스위치다. 전역 자동화 숨김
//! (`hide_automation_workspaces`)과는 판정 근거도 저장 단위도 다르다 — 그쪽은
//! 자동화 실행 원장이 근거이고 전역 불리언 하나이며, 이쪽은 워크트리의 **출처**가
//! 근거이고 프로젝트별 필드다. 겹치는 것은 "사이드바에서 행이 사라진다"는 결과
//! 하나뿐이라, 두 필터는 창에서 **한 체인**으로 합쳐진다(각자 `if`를 갖는 순간
//! "왜 안 보이지"의 원인이 두 곳으로 갈린다).
//!
//! ## 무엇이 이 판정을 위험하게 만드는가
//!
//! 잘못 숨기면 **사람이 자기 작업을 잃어버렸다고 생각한다.** 그래서 모든 분기가
//! 한 방향으로 기운다: 확신이 없으면 보인다. `unknown-legacy`가 존재하는 이유가
//! 그것이고([`Ownership::UnknownLegacy`]), 롤아웃 규칙이 존재하는 이유도
//! 그것이다([`is_legacy`]) — 기능이 들어온 날 기존 사용자의 목록이 갑자기 비면
//! 그것은 기능이 아니라 사고다.
//!
//! ## 순수하다
//!
//! 여기에는 디스크도 시계도 git도 없다. 창(`zerocode-shell`)이 워크트리를 훑어
//! [`Facts`]를 만들고, 저장된 답([`Stored`])과 함께 넘긴다. 여기 있는 것은 그
//! 둘을 받아 네 분류 중 하나와 `bool` 하나를 답하는 함수들이다.

use serde::{Deserialize, Serialize};

/// 이 기능이 이 제품에 들어온 시각, epoch 밀리초 (2026-08-11T00:00:00Z).
///
/// 기본값을 가르는 유일한 선이다. 이 시각 **이전에** 목록에 들어온 프로젝트는
/// 계속 보이고(`show`), 이후에 추가된 프로젝트는 기본으로 숨긴다(`hide`).
/// 없으면 업그레이드한 사람의 사이드바가 어느 날 아침 비어 있게 된다.
pub const VISIBILITY_ROLLOUT_MS: i64 = 1_786_406_400_000;

/// 에이전트 도구가 저장소 안에 자기 체크아웃을 파는 자리들.
///
/// 목록인 이유는 축이 "**어떤** 에이전트 도구의 스크래치인가"가 아니라 "에이전트
/// 도구의 스크래치인가"이기 때문이다 — 도구마다 이름이 다르고, 새 도구가 하나
/// 늘면 여기에 한 줄이 는다. 지금 실재하는 것은 하나다(Claude Code의
/// `EnterWorktree`가 `.claude/worktrees/<name>`에 판다).
///
/// 경로 조각으로 비교된다. 문자열 `contains`로 보면 `.claudex/worktrees-old`가
/// 걸린다 — 사람의 폴더를 영구히 숨기는 종류의 오판이다.
pub const AGENT_SCRATCH_DIRS: [&str; 1] = [".claude/worktrees"];

/// 이 워크트리를 누가 만들었나.
///
/// 판정 순서가 곧 정의다([`classify`]) — 한 워크트리가 둘에 해당할 수 있고,
/// 그때 무엇으로 부르는지가 보일지 말지를 가른다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ownership {
    /// 이 창의 오케스트레이터가 만들었다. 언제나 보인다.
    ZerocodeManaged,
    /// 에이전트 도구가 저장소 안에 판 스크래치. 사람이 관리하는 물건이 아니다.
    AgentScratch,
    /// 우리 것처럼 생긴 자리에 있으나 우리가 만들었다고 말할 근거가 없다 —
    /// 그리고 아무것으로도 분류하지 못한 것 전부. **보이는 쪽**으로 기운다.
    UnknownLegacy,
    /// 밖에서 만들어진 것. 이 스위치가 말하는 대상이다.
    External,
}

impl Ownership {
    /// 창이 문구와 스타일을 붙이는 이름. serde 표현과 같은 철자다.
    pub const fn slug(self) -> &'static str {
        match self {
            Ownership::ZerocodeManaged => "zerocode-managed",
            Ownership::AgentScratch => "agent-scratch",
            Ownership::UnknownLegacy => "unknown-legacy",
            Ownership::External => "external",
        }
    }
}

/// 스위치의 두 자리.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Hide,
    Show,
}

impl Visibility {
    pub const fn slug(self) -> &'static str {
        match self {
            Visibility::Hide => "hide",
            Visibility::Show => "show",
        }
    }
}

/// 한 워크트리에 대해 창이 잰 사실.
///
/// 경로는 창이 이미 정규화해 넘긴다(카탈로그가 모든 경로를 `canonicalize`한다).
/// 여기서 다시 만지지 않는 이유는 정규화가 디스크를 읽는 일이고, 이 모듈은 읽지
/// 않기 때문이다.
#[derive(Debug, Clone, Copy)]
pub struct Facts<'a> {
    pub path: &'a str,
    /// 짧은 브랜치 이름. 분리된 HEAD면 `None`.
    pub branch: Option<&'a str>,
}

/// 이 저장소에서 **우리가** 만든 것을 알아보는 두 지문.
///
/// 실제 메타 구조를 코드에서 확인한 결과다. `zerocode-orchestrator`가 워크트리를
/// 만들 때 남기는 것은 레코드가 아니라 **자리와 이름** 둘이다:
/// [`Orchestrator::worktree_root`]가 파는 디렉터리
/// (`~/.zerocode/worktrees/<slug>-<hash>/`)와
/// [`Orchestrator::branch_prefix`]가 붙이는 브랜치 이름공간(`wt/…`).
/// git 워크트리에는 우리가 필드를 얹을 자리가 없으므로 이 둘이 우리가 가진
/// 전부이고, 둘 다 정상 경로에서는 우리 생성기만 만든다.
///
/// [`Orchestrator::worktree_root`]: https://docs.rs/zerocode-orchestrator
/// [`Orchestrator::branch_prefix`]: https://docs.rs/zerocode-orchestrator
#[derive(Debug, Clone, Copy)]
pub struct Ours<'a> {
    /// 이 저장소의 워크트리가 사는 디렉터리. 오케스트레이터를 열지 못했으면 빈
    /// 문자열 — 그때는 **아무것도 external이라고 부르지 않는다**(근거가 없다).
    pub worktree_root: &'a str,
    /// 예전에 워크트리가 살던 자리들 — 사람이 설정을 바꾸기 **전**의 배치.
    ///
    /// 이것이 없으면 워크스페이스 디렉터리를 바꾸거나 Nest 를 켜는 순간 기존
    /// 워크스페이스가 어느 지문에도 안 걸린다. 브랜치 접두사가 살려 주는 경우도
    /// 있지만 접두사 모드가 `none` 인 사람에게는 그 그물조차 없고, 그때 결과는
    /// **사이드바에서 자기 작업이 사라지는 것**이다. Orca 도 같은 이유로
    /// `workspaceDirHistory` 를 들고 다닌다(`ownership.ts:33-74`).
    ///
    /// 이 목록은 판정을 **넓히기만** 한다 — 무엇도 새로 숨기지 않는다.
    pub past_roots: &'a [String],
    /// 브랜치 이름공간, `/` 없이(`"wt"`).
    pub branch_prefix: &'a str,
}

/// 프로젝트별로 저장되는 여섯 가지. 전부 없어도 되는 값이고, 없을 때의 뜻이
/// 각각 다르다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stored {
    /// 사람이 스위치를 만졌다면 그 답. 만지지 않았으면 `None`이고, 기본값은
    /// [`is_legacy`]가 정한다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    /// 이 프로젝트가 롤아웃 이전 사람인지, **한 번 굳힌** 답. 스위치를 처음
    /// 만질 때 적힌다 — 굳히지 않으면 사람이 `show`를 골랐다가 되돌린 순간
    /// 기본값이 `hide`로 바뀌어 목록이 비어 버린다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy: Option<bool>,
    /// 최초 1회 인라인 카드를 본 시각. 있으면 그 카드는 다시 뜨지 않는다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_dismissed_at: Option<i64>,
    /// "다시 보지 않기"를 누른 시각. 있으면 인박스가 영구히 조용해진다 —
    /// 그러나 [`import_clears_suppression`] 때문에 되돌릴 수 있다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery_suppressed_at: Option<i64>,
    /// "지금 숨긴 것들"의 기준선. 이 목록에 **없는** 숨은 경로만 인박스에
    /// 올라온다 — 한 번 숨겼다는 것은 그때 있던 것들에 대한 답이고, 그 뒤에
    /// 새로 생긴 것에 대한 답은 아니다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inbox_baseline_paths: Vec<String>,
    /// 개별로 들여온 경로들. 스위치가 `hide`여도 이것들은 보인다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imported_paths: Vec<String>,
}

/// 이 프로젝트가 롤아웃 이전 사람인가.
///
/// 굳혀 둔 답이 있으면 그것이 답이다. 없으면 목록에 들어온 시각으로 가른다 —
/// 모르는 시각(`None`)은 **이전**으로 읽는다. 이 기능이 생기기 전에 있던
/// 프로젝트에는 시각이 적혀 있을 수 없고, 그것이 정확히 보호해야 하는 사람이다.
///
/// **Orca와 한 곳 다르다.** 원본은 `visibility === undefined`도 legacy의 사유로
/// 세는데(`isLegacyRepoForExternalWorktreeVisibility`), 그러면 저장된 답이 없는
/// 모든 저장소가 legacy가 되어 `hide` 기본값이 **어떤 입력으로도 도달할 수
/// 없다** — 롤아웃 상수도 `addedAt`도 읽는 곳이 없는 저장 용량이 된다. 읽는 곳이
/// 없는 칸은 두지 않는다는 이 저장소의 규칙(1-eb)에 따라 그 사유만 뺐다. 남은
/// 두 사유는 원본 그대로다.
pub fn is_legacy(stored: &Stored, added_at_ms: Option<i64>) -> bool {
    if let Some(said) = stored.legacy {
        return said;
    }
    added_at_ms.is_none_or(|at| at < VISIBILITY_ROLLOUT_MS)
}

/// 이 프로젝트의 스위치가 실제로 어느 자리에 있는가.
///
/// 저장된 답이 이기고, 없으면 롤아웃이 정한다.
pub fn effective_visibility(stored: &Stored, added_at_ms: Option<i64>) -> Visibility {
    if let Some(said) = stored.visibility {
        return said;
    }
    if is_legacy(stored, added_at_ms) {
        Visibility::Show
    } else {
        Visibility::Hide
    }
}

/// 경로 `path`가 `root` 아래에 있는가 — 조각 단위로.
///
/// `starts_with`만으로 보면 `/repo-old`가 `/repo` 아래라고 답한다. 같은 경로
/// 자신은 아래가 아니다: 저장소의 워크트리 디렉터리 그 자체는 워크트리가 아니다.
fn under(path: &str, root: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    let root = root.trim_end_matches('/');
    let Some(rest) = path.strip_prefix(root) else {
        return false;
    };
    rest.starts_with('/') && rest.len() > 1
}

/// 이 워크트리가 우리 오케스트레이터가 만든 것인가.
///
/// 두 지문 중 **하나라도** 맞으면 참이다(Orca의 `hasStrongOrcaMetadata`도 OR로
/// 묻는다). 오판의 방향이 안전한 쪽이라 그렇게 둔다: 남의 것을 우리 것으로
/// 잘못 부르면 그 행은 **언제나 보인다**(성가시다), 우리 것을 남의 것으로 잘못
/// 부르면 그 행은 **사라질 수 있다**(사람이 작업을 잃었다고 생각한다).
pub fn is_zerocode_managed(facts: Facts<'_>, ours: Ours<'_>) -> bool {
    if under(facts.path, ours.worktree_root) {
        return true;
    }
    // 예전 자리도 우리 자리다. 사람이 설정을 바꿨다는 사실이 이미 만든 작업의
    // 출처를 바꾸지는 않는다.
    if ours
        .past_roots
        .iter()
        .any(|root| under(facts.path, root.as_str()))
    {
        return true;
    }
    if ours.branch_prefix.is_empty() {
        return false;
    }
    facts
        .branch
        .is_some_and(|branch| branch.starts_with(&format!("{}/", ours.branch_prefix)))
}

/// 이 워크트리가 에이전트 도구의 스크래치인가.
///
/// 저장소 어디든 [`AGENT_SCRATCH_DIRS`] 중 하나 아래면 참이다. 저장소 루트를
/// 받지 않는 이유는 에이전트가 **워크트리 안에서도** 판다는 것이다 — 루트에
/// 고정하면 중첩된 스크래치를 놓친다.
pub fn is_agent_scratch(facts: Facts<'_>) -> bool {
    AGENT_SCRATCH_DIRS.iter().any(|dir| {
        let needle = format!("/{dir}/");
        facts.path.contains(&needle)
    })
}

/// 네 분류. 순서가 규칙이다.
///
/// 1. 우리가 만든 것 — 다른 무엇보다 먼저 묻는다. 우리 워크트리 디렉터리 안에
///    에이전트가 뭔가 팠다 해도 그것은 우리 자리다.
/// 2. 에이전트 스크래치 — 사람이 관리하는 물건이 아니다.
/// 3. 우리 것처럼 생긴 평평한 자리(`~/.zerocode/worktrees/` 바로 아래의 남의
///    저장소 칸) — 예전 배치가 남긴 것일 수 있으므로 `external`이라고 단정하지
///    않는다.
/// 4. external이라고 말할 근거가 있으면 external. 없으면 다시 legacy —
///    **모른다는 것은 숨겨도 된다는 뜻이 아니다.**
pub fn classify(facts: Facts<'_>, ours: Ours<'_>) -> Ownership {
    if is_zerocode_managed(facts, ours) {
        return Ownership::ZerocodeManaged;
    }
    if is_agent_scratch(facts) {
        return Ownership::AgentScratch;
    }
    if under_flat_worktree_area(facts, ours) {
        return Ownership::UnknownLegacy;
    }
    if can_classify_as_external(facts, ours) {
        return Ownership::External;
    }
    Ownership::UnknownLegacy
}

/// 이 저장소의 칸은 아니지만 우리 워크트리 영역 안에 있는가.
///
/// `~/.zerocode/worktrees/<slug>-<hash>`의 **부모**가 그 영역이다. 거기 있는
/// 체크아웃은 우리가 판 것이 거의 확실하지만 지금 배치로는 이 저장소 것이라고
/// 말할 수 없다 — Orca의 `isUnderFlatOrUntrustedOrcaRoot`가 같은 자리를 본다.
fn under_flat_worktree_area(facts: Facts<'_>, ours: Ours<'_>) -> bool {
    let root = ours.worktree_root.trim_end_matches('/');
    let Some(cut) = root.rfind('/') else {
        return false;
    };
    if cut == 0 {
        return false;
    }
    under(facts.path, &root[..cut])
}

/// external이라고 부를 근거가 있는가.
///
/// 절대 경로이고, 우리 워크트리 자리가 어디인지 이 창이 알 때만. 오케스트레이터를
/// 열지 못한 저장소(빈 `worktree_root`)에서는 무엇도 external이 아니다 — 비교할
/// 기준이 없는데 "밖"이라고 부르는 것은 근거 없이 숨기는 일이다.
fn can_classify_as_external(facts: Facts<'_>, ours: Ours<'_>) -> bool {
    !ours.worktree_root.is_empty() && facts.path.starts_with('/')
}

/// 이 워크트리가 사이드바에 보이는가 — 이 기능의 전부.
///
/// 분기 순서가 곧 약속이다:
/// - 저장소 자신의 체크아웃은 **언제나** 보인다. 창이 서 있는 자리를 숨기는
///   필터는 깨진 목록으로 읽힌다.
/// - 우리가 만든 것은 언제나 보인다. 스위치가 말하는 대상이 아니다.
/// - 개별로 들여온 경로는 언제나 보인다. 사람이 그 경로를 이름으로 지목했다.
/// - 에이전트 스크래치는 스위치와 무관하게 보이지 않는다. 위 세 문이 아니면
///   `show`로도 나오지 않는다 — 그리고 창은 이것들을 애초에 가져오기 목록에
///   올리지 않으므로([`is_user_facing`]) 위의 import 문으로 들어올 길도 없다.
/// - legacy 저장소의 분류 불가 항목은 보인다.
/// - 나머지가 스위치의 몫이다.
pub fn should_show(
    facts: Facts<'_>,
    ours: Ours<'_>,
    stored: &Stored,
    added_at_ms: Option<i64>,
    is_selected_checkout: bool,
) -> bool {
    if is_selected_checkout {
        return true;
    }
    let ownership = classify(facts, ours);
    if ownership == Ownership::ZerocodeManaged {
        return true;
    }
    if is_imported(facts.path, stored) {
        return true;
    }
    if ownership == Ownership::AgentScratch {
        return false;
    }
    if ownership == Ownership::UnknownLegacy && is_legacy(stored, added_at_ms) {
        return true;
    }
    effective_visibility(stored, added_at_ms) == Visibility::Show
}

/// 이 경로를 사람이 개별로 들여왔는가.
pub fn is_imported(path: &str, stored: &Stored) -> bool {
    stored
        .imported_paths
        .iter()
        .any(|held| same_path(held, path))
}

/// 두 경로가 같은 곳을 말하는가. 끝의 `/` 하나만 무시한다 — 대소문자를 접지
/// 않는 이유는 이 창이 사는 두 플랫폼에서 파일 이름이 대소문자를 구분하는
/// 것으로 다루기 때문이다.
fn same_path(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}

/// 이 워크트리가 **사람에게 보여 줄 만한** 외부 워크트리인가.
///
/// 카운트와 목록이 세는 대상이다. 저장소 자신의 체크아웃, 우리가 만든 것,
/// 에이전트 스크래치는 세지 않는다 — 앞의 둘은 이 스위치의 대상이 아니고,
/// 마지막은 사람이 손댈 물건이 아니다.
pub fn is_user_facing(facts: Facts<'_>, ours: Ours<'_>, is_selected_checkout: bool) -> bool {
    if is_selected_checkout {
        return false;
    }
    !matches!(
        classify(facts, ours),
        Ownership::ZerocodeManaged | Ownership::AgentScratch
    )
}

/// 인박스를 내밀어야 하는가.
///
/// 셋 다 참일 때만. 영구 억제가 걸려 있지 않고, 최초 1회 카드는 이미 봤고,
/// 스위치가 `hide`에 있을 때 — 즉 **"숨김"을 이미 고른 사람에게만** 새로 생긴
/// 것을 다시 묻는다.
pub fn should_offer_inbox(stored: &Stored, added_at_ms: Option<i64>) -> bool {
    if stored.discovery_suppressed_at.is_some() {
        return false;
    }
    if stored.prompt_dismissed_at.is_none() {
        return false;
    }
    effective_visibility(stored, added_at_ms) == Visibility::Hide
}

/// 최초 1회 카드를 내밀어야 하는가.
///
/// 아직 그 카드를 본 적이 없고, 영구 억제도 걸려 있지 않고, 스위치가 `hide`에
/// 있을 때. 숨은 것이 실제로 있는지는 창이 카운트로 판단한다 — 0개일 때 "숨은
/// 워크트리 0개"라고 말하는 카드는 아무것도 알리지 않는다.
pub fn should_offer_prompt(stored: &Stored, added_at_ms: Option<i64>) -> bool {
    stored.prompt_dismissed_at.is_none()
        && stored.discovery_suppressed_at.is_none()
        && effective_visibility(stored, added_at_ms) == Visibility::Hide
}

/// 숨은 것들 중 기준선에 **없는** 경로 — 인박스가 보여 줄 것.
///
/// 차집합이 이 기능의 값이다. 한 번 "숨김"을 골라도 그 뒤에 새로 만들어진 외부
/// 워크트리는 다시 올라온다. 기준선에 있는 것은 이미 답한 것이므로 조용하다.
pub fn inbox_paths<'a>(
    hidden: &[&'a str],
    stored: &Stored,
    added_at_ms: Option<i64>,
) -> Vec<&'a str> {
    if !should_offer_inbox(stored, added_at_ms) {
        return Vec::new();
    }
    hidden
        .iter()
        .copied()
        .filter(|path| {
            !stored
                .inbox_baseline_paths
                .iter()
                .any(|held| same_path(held, path))
        })
        .collect()
}

/// 기준선에 경로들을 더한다 — 이미 있는 것은 그대로.
///
/// Orca의 `mergeExternalWorktreeInboxPaths`와 같은 일을 한다. 순서를 지키고
/// 중복을 만들지 않는다: 기준선이 같은 경로를 두 번 들면 그 목록은 아무 질문에도
/// 답하지 못한다.
pub fn merge_baseline(stored: &mut Stored, additions: &[&str]) {
    for path in additions {
        if path.is_empty() {
            continue;
        }
        if stored
            .inbox_baseline_paths
            .iter()
            .any(|held| same_path(held, path))
        {
            continue;
        }
        stored.inbox_baseline_paths.push((*path).to_string());
    }
}

/// 경로들을 개별로 들여온다 — 그리고 **영구 억제를 풀어 준다**.
///
/// 되돌릴 길이 없는 숨기기는 숨기기가 아니라 삭제다. "다시 보지 않기"를 누른
/// 사람이 나중에 워크트리 하나를 가져오면 그 행동 자체가 "다시 보여 달라"이므로,
/// 억제는 여기서 풀린다(Orca의 다이얼로그 `Import`도 같은 자리에서 같은 일을
/// 한다).
pub fn import_clears_suppression(stored: &mut Stored, paths: &[&str]) {
    for path in paths {
        if path.is_empty() || is_imported(path, stored) {
            continue;
        }
        stored.imported_paths.push((*path).to_string());
    }
    stored.discovery_suppressed_at = None;
}

/// 스위치를 옮긴다.
///
/// 처음 만질 때 legacy를 굳히고, `show`로 옮기면 억제를 푼다. 세 가지가 이 한
/// 함수에 같이 있는 이유는 셋이 한 행동의 부분이기 때문이다 — 따로 두면 호출자
/// 중 하나가 반드시 하나를 잊는다. 반대로 `hide`는 억제를 **만들지 않는다**:
/// 영구 억제는 자기 확인 다이얼로그를 가진 별개의 행동이다.
pub fn set_visibility(stored: &mut Stored, added_at_ms: Option<i64>, said: Visibility) {
    if stored.legacy.is_none() {
        stored.legacy = Some(is_legacy(stored, added_at_ms));
    }
    stored.visibility = Some(said);
    if said == Visibility::Show {
        stored.discovery_suppressed_at = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/home/dev/.zerocode/worktrees/api-abcd1234";

    fn ours() -> Ours<'static> {
        Ours {
            worktree_root: ROOT,
            past_roots: &[],
            branch_prefix: "wt",
        }
    }

    fn at(path: &str) -> Facts<'_> {
        Facts { path, branch: None }
    }

    /// 우리가 만든 것이 무엇보다 먼저 판정된다.
    ///
    /// 우리 워크트리 디렉터리 안에 `.claude/worktrees/`가 있을 수 있다(에이전트가
    /// 워크스페이스 안에서 또 판다). 스크래치를 먼저 물으면 그 행이 영구히
    /// 사라지고, 사라진 것은 우리가 만든 워크스페이스다.
    #[test]
    fn managed_is_asked_before_scratch() {
        let nested = format!("{ROOT}/drain-gate/.claude/worktrees/probe");
        assert_eq!(classify(at(&nested), ours()), Ownership::ZerocodeManaged);
        // 그리고 저장소 안의 스크래치는 스크래치다.
        assert_eq!(
            classify(at("/home/dev/api/.claude/worktrees/probe"), ours()),
            Ownership::AgentScratch
        );
    }

    /// 두 지문 각각으로 우리 것이 된다 — 자리, 그리고 브랜치 이름공간.
    #[test]
    fn managed_is_recognised_by_place_or_by_name() {
        assert!(is_zerocode_managed(
            at(&format!("{ROOT}/drain-gate")),
            ours()
        ));
        assert!(is_zerocode_managed(
            Facts {
                path: "/somewhere/else/side",
                branch: Some("wt/drain-gate"),
            },
            ours()
        ));
        // 이름공간이 이름의 시작이어야 한다. `wtf/x`는 우리 것이 아니다.
        assert!(!is_zerocode_managed(
            Facts {
                path: "/somewhere/else/side",
                branch: Some("wtf/drain-gate"),
            },
            ours()
        ));
        // 그리고 디렉터리 그 자체는 워크트리가 아니다.
        assert!(!is_zerocode_managed(at(ROOT), ours()));
        // 이름이 겹치는 이웃 디렉터리도 아니다.
        assert!(!is_zerocode_managed(
            at("/home/dev/.zerocode/worktrees/api-abcd1234-old/side"),
            ours()
        ));
    }

    /// 스크래치 판정은 경로 조각으로 한다.
    #[test]
    fn scratch_is_matched_by_path_segment() {
        assert!(is_agent_scratch(at("/repo/.claude/worktrees/probe")));
        assert!(is_agent_scratch(at("/repo/sub/.claude/worktrees/a/b")));
        // 이름이 비슷한 남의 폴더는 스크래치가 아니다 — 이 오판은 사람의
        // 디렉터리를 영구히 숨긴다.
        assert!(!is_agent_scratch(at("/repo/.claudex/worktrees-old/probe")));
        assert!(!is_agent_scratch(at("/repo/claude/worktrees/probe")));
        // 디렉터리 자신은 워크트리가 아니다.
        assert!(!is_agent_scratch(at("/repo/.claude/worktrees")));
    }

    /// 우리 영역 안의 남의 칸은 external이 아니라 legacy다.
    #[test]
    fn the_flat_worktree_area_reads_as_legacy() {
        assert_eq!(
            classify(at("/home/dev/.zerocode/worktrees/other-99999999/x"), ours()),
            Ownership::UnknownLegacy
        );
        // 그 밖은 external이다.
        assert_eq!(
            classify(at("/home/dev/scratch/by-hand"), ours()),
            Ownership::External
        );
    }

    /// 우리 자리를 모르면 무엇도 external이 아니다.
    ///
    /// 오케스트레이터를 열지 못한 저장소(폴더 프로젝트)가 그렇다. 비교 기준이
    /// 없는데 "밖"이라고 부르는 것은 근거 없이 숨기는 일이므로, 그런 항목은
    /// external이 아니라 legacy로 떨어지고 — 그래서 롤아웃 이전 프로젝트에서는
    /// 계속 보인다. 그리고 폴더 프로젝트의 유일한 행은 자기 체크아웃이므로
    /// 어느 설정에서도 사라지지 않는다.
    /// 설정을 바꿔도 이미 만든 워크스페이스는 우리 것으로 남는다.
    ///
    /// 이 테스트가 지키는 실패는 조용하다: 브랜치 접두사를 `none` 으로 둔
    /// 사람이 워크스페이스 디렉터리를 바꾸면, 옛 자리의 체크아웃이 어느
    /// 지문에도 안 걸려 external 이 되고 사이드바에서 사라진다. 사람은 그것을
    /// "작업을 잃었다" 로 읽는다.
    #[test]
    fn a_workspace_made_under_the_old_setting_is_still_ours() {
        const WAS: &str = "/home/dev/zerocode/workspaces";
        let old_place = format!("{WAS}/wt-login-fix");
        let past = [WAS.to_string()];

        // 접두사가 없으면 브랜치 그물도 없다 — 경로가 유일한 근거다.
        let moved = Ours {
            worktree_root: ROOT,
            past_roots: &past,
            branch_prefix: "",
        };
        assert!(
            is_zerocode_managed(at(&old_place), moved),
            "설정을 바꾸자 예전 워크스페이스가 남의 것이 됐다"
        );
        assert_eq!(classify(at(&old_place), moved), Ownership::ZerocodeManaged);

        // 이력이 없으면 바로 그 실패가 재현된다 — 이 줄이 위 단언의 값을 잰다.
        let forgetful = Ours {
            worktree_root: ROOT,
            past_roots: &[],
            branch_prefix: "",
        };
        assert!(!is_zerocode_managed(at(&old_place), forgetful));

        // 그리고 이력은 넓히기만 한다: 정말 밖에 있는 것은 여전히 밖이다.
        assert!(!is_zerocode_managed(
            at("/home/dev/elsewhere/by-hand"),
            moved
        ));
    }

    #[test]
    fn nothing_is_external_when_we_do_not_know_our_own_place() {
        let blind = Ours {
            worktree_root: "",
            past_roots: &[],
            branch_prefix: "wt",
        };
        assert_eq!(
            classify(at("/home/dev/scratch/by-hand"), blind),
            Ownership::UnknownLegacy
        );
        let stored = Stored::default();
        assert!(should_show(
            at("/home/dev/scratch/by-hand"),
            blind,
            &stored,
            None,
            false
        ));
        // 폴더 프로젝트가 그리는 한 줄. 숨김을 골랐고 억제까지 걸어도 남는다.
        let shut = Stored {
            visibility: Some(Visibility::Hide),
            legacy: Some(false),
            discovery_suppressed_at: Some(1),
            ..Stored::default()
        };
        assert!(should_show(
            at("/home/dev/folder-project"),
            blind,
            &shut,
            Some(VISIBILITY_ROLLOUT_MS + 1),
            true
        ));
    }

    /// 롤아웃 이전에 들어온 프로젝트는 계속 보이고, 이후는 기본으로 숨긴다.
    ///
    /// 이 단언이 없으면 기능이 들어온 날 기존 사용자의 사이드바가 빈다 — 그리고
    /// 그것은 버그로 보이지 않고 데이터 손실로 보인다.
    #[test]
    fn a_project_from_before_the_rollout_keeps_showing_its_external_worktrees() {
        let stored = Stored::default();
        for before in [
            None,
            Some(0),
            Some(VISIBILITY_ROLLOUT_MS - 1),
            Some(VISIBILITY_ROLLOUT_MS - 86_400_000),
        ] {
            assert_eq!(
                effective_visibility(&stored, before),
                Visibility::Show,
                "{before:?}에 들어온 프로젝트의 목록이 갑자기 비었다"
            );
            assert!(is_legacy(&stored, before));
        }
        for after in [Some(VISIBILITY_ROLLOUT_MS), Some(VISIBILITY_ROLLOUT_MS + 1)] {
            assert_eq!(
                effective_visibility(&stored, after),
                Visibility::Hide,
                "{after:?}에 들어온 프로젝트가 기본으로 남의 워크트리를 들였다"
            );
            assert!(!is_legacy(&stored, after));
        }
        // 그리고 상수는 정말 이 기능의 도입 시각이다.
        assert_eq!(VISIBILITY_ROLLOUT_MS, 1_786_406_400_000);
    }

    /// 저장된 답이 롤아웃을 이긴다 — 굳힌 legacy도.
    #[test]
    fn what_the_person_said_wins() {
        let said = Stored {
            visibility: Some(Visibility::Hide),
            ..Stored::default()
        };
        assert_eq!(effective_visibility(&said, None), Visibility::Hide);
        let frozen = Stored {
            legacy: Some(true),
            ..Stored::default()
        };
        assert_eq!(
            effective_visibility(&frozen, Some(VISIBILITY_ROLLOUT_MS + 1)),
            Visibility::Show
        );
        let frozen_new = Stored {
            legacy: Some(false),
            ..Stored::default()
        };
        assert_eq!(effective_visibility(&frozen_new, None), Visibility::Hide);
    }

    /// 스위치를 처음 만질 때 legacy가 굳는다.
    ///
    /// 굳지 않으면 롤아웃 이전 사람이 `show`를 골랐다가 되돌린 순간 기본값이
    /// `hide`가 되어 목록이 빈다 — 자기가 한 일로 보이지 않는 손실이다.
    #[test]
    fn the_first_touch_freezes_which_side_of_the_rollout_this_project_is_on() {
        let mut stored = Stored::default();
        set_visibility(&mut stored, None, Visibility::Show);
        assert_eq!(stored.legacy, Some(true));
        set_visibility(&mut stored, None, Visibility::Hide);
        assert_eq!(
            stored.legacy,
            Some(true),
            "굳힌 답이 두 번째 손길에 바뀌었다"
        );
        assert_eq!(effective_visibility(&stored, None), Visibility::Hide);
        stored.visibility = None;
        assert_eq!(
            effective_visibility(&stored, None),
            Visibility::Show,
            "되돌린 사람의 목록이 비었다"
        );
    }

    /// `show`로 옮기면 영구 억제가 풀린다.
    #[test]
    fn showing_again_lifts_the_permanent_suppression() {
        let mut stored = Stored {
            discovery_suppressed_at: Some(10),
            ..Stored::default()
        };
        set_visibility(&mut stored, None, Visibility::Show);
        assert_eq!(stored.discovery_suppressed_at, None);
        // 숨기기는 억제를 만들지 않는다 — 그것은 별개의 확인을 받은 행동이다.
        let mut hiding = Stored {
            discovery_suppressed_at: Some(10),
            ..Stored::default()
        };
        set_visibility(&mut hiding, None, Visibility::Hide);
        assert_eq!(hiding.discovery_suppressed_at, Some(10));
    }

    /// 가져오기가 억제를 풀어 준다 — 되돌릴 길 없는 숨기기는 삭제다.
    #[test]
    fn importing_one_worktree_lifts_the_suppression() {
        let mut stored = Stored {
            discovery_suppressed_at: Some(10),
            ..Stored::default()
        };
        import_clears_suppression(&mut stored, &["/home/dev/scratch/by-hand"]);
        assert_eq!(stored.discovery_suppressed_at, None);
        assert!(is_imported("/home/dev/scratch/by-hand", &stored));
        // 두 번 가져와도 목록은 한 번만 담는다.
        import_clears_suppression(&mut stored, &["/home/dev/scratch/by-hand/"]);
        assert_eq!(stored.imported_paths.len(), 1);
    }

    /// 들여온 경로는 `hide`에서도 보인다.
    #[test]
    fn an_imported_path_shows_while_the_rest_stay_hidden() {
        let stored = Stored {
            visibility: Some(Visibility::Hide),
            imported_paths: vec!["/home/dev/scratch/kept".to_string()],
            ..Stored::default()
        };
        assert!(should_show(
            at("/home/dev/scratch/kept"),
            ours(),
            &stored,
            None,
            false
        ));
        assert!(!should_show(
            at("/home/dev/scratch/other"),
            ours(),
            &stored,
            None,
            false
        ));
    }

    /// 창이 서 있는 체크아웃은 무슨 설정에서도 보인다.
    #[test]
    fn the_checkout_the_window_stands_in_is_never_hidden() {
        let stored = Stored {
            visibility: Some(Visibility::Hide),
            discovery_suppressed_at: Some(1),
            ..Stored::default()
        };
        assert!(should_show(
            at("/home/dev/scratch/by-hand"),
            ours(),
            &stored,
            None,
            true
        ));
        // 그리고 그것은 가져오기 목록에 오르지 않는다.
        assert!(!is_user_facing(
            at("/home/dev/scratch/by-hand"),
            ours(),
            true
        ));
    }

    /// 우리가 만든 것은 이 스위치가 만지지 않는다.
    #[test]
    fn what_we_made_is_not_what_this_switch_talks_about() {
        let hidden = Stored {
            visibility: Some(Visibility::Hide),
            ..Stored::default()
        };
        assert!(should_show(
            at(&format!("{ROOT}/drain-gate")),
            ours(),
            &hidden,
            None,
            false
        ));
        assert!(!is_user_facing(
            at(&format!("{ROOT}/drain-gate")),
            ours(),
            false
        ));
    }

    /// 에이전트 스크래치는 스위치가 어느 자리에 있어도 보이지 않는다.
    #[test]
    fn agent_scratch_is_never_shown_by_the_switch() {
        for said in [Visibility::Hide, Visibility::Show] {
            let stored = Stored {
                visibility: Some(said),
                ..Stored::default()
            };
            assert!(
                !should_show(
                    at("/home/dev/api/.claude/worktrees/probe"),
                    ours(),
                    &stored,
                    None,
                    false
                ),
                "{said:?}에서 에이전트 스크래치가 사이드바에 올랐다"
            );
        }
        // legacy 저장소에서도 아니다 — legacy 관용은 분류 불가 항목에만 준다.
        let legacy = Stored {
            legacy: Some(true),
            ..Stored::default()
        };
        assert!(!should_show(
            at("/home/dev/api/.claude/worktrees/probe"),
            ours(),
            &legacy,
            None,
            false
        ));
        // 그리고 세지도 않으므로 가져오기로 들어올 문이 없다.
        assert!(!is_user_facing(
            at("/home/dev/api/.claude/worktrees/probe"),
            ours(),
            false
        ));
    }

    /// legacy 저장소의 분류 불가 항목은 보인다.
    #[test]
    fn a_legacy_project_keeps_showing_what_nobody_could_classify() {
        let hidden_legacy = Stored {
            visibility: Some(Visibility::Hide),
            legacy: Some(true),
            ..Stored::default()
        };
        let flat = "/home/dev/.zerocode/worktrees/other-99999999/x";
        assert!(should_show(at(flat), ours(), &hidden_legacy, None, false));
        // 롤아웃 이후 프로젝트에서는 스위치가 답한다.
        let hidden_new = Stored {
            visibility: Some(Visibility::Hide),
            legacy: Some(false),
            ..Stored::default()
        };
        assert!(!should_show(at(flat), ours(), &hidden_new, None, false));
    }

    /// 스위치 하나가 external 전부를 가른다.
    #[test]
    fn the_switch_answers_for_the_plain_external_worktree() {
        let path = "/home/dev/scratch/by-hand";
        for (said, shown) in [(Visibility::Show, true), (Visibility::Hide, false)] {
            let stored = Stored {
                visibility: Some(said),
                ..Stored::default()
            };
            assert_eq!(should_show(at(path), ours(), &stored, None, false), shown);
        }
    }

    /// 인박스는 셋 다 참일 때만 열린다.
    #[test]
    fn the_inbox_opens_only_for_someone_who_already_chose_hide() {
        let ready = Stored {
            visibility: Some(Visibility::Hide),
            prompt_dismissed_at: Some(5),
            ..Stored::default()
        };
        assert!(should_offer_inbox(&ready, None));
        // 최초 카드를 아직 안 봤다.
        let fresh = Stored {
            visibility: Some(Visibility::Hide),
            ..Stored::default()
        };
        assert!(!should_offer_inbox(&fresh, None));
        assert!(should_offer_prompt(&fresh, None));
        // 영구 억제가 걸렸다.
        let quiet = Stored {
            discovery_suppressed_at: Some(9),
            ..ready.clone()
        };
        assert!(!should_offer_inbox(&quiet, None));
        assert!(!should_offer_prompt(&quiet, None));
        // 스위치가 `show`에 있으면 물을 것이 없다.
        let showing = Stored {
            visibility: Some(Visibility::Show),
            ..ready.clone()
        };
        assert!(!should_offer_inbox(&showing, None));
        assert!(!should_offer_prompt(&showing, None));
    }

    /// 기준선 차집합 — 새로 생긴 것만 올라온다.
    ///
    /// 이 기능의 값 대부분이 이 한 줄에 있다. 차집합이 아니라 전체를 보여 주면
    /// 사람은 같은 목록을 매번 다시 치우게 되고, 그러면 두 번째부터는 읽지 않는다.
    #[test]
    fn only_what_appeared_after_the_hiding_comes_back() {
        let mut stored = Stored {
            visibility: Some(Visibility::Hide),
            prompt_dismissed_at: Some(5),
            ..Stored::default()
        };
        merge_baseline(&mut stored, &["/scratch/one", "/scratch/two"]);
        let hidden = ["/scratch/one", "/scratch/two", "/scratch/three"];
        assert_eq!(
            inbox_paths(&hidden, &stored, None),
            vec!["/scratch/three"],
            "이미 답한 것들이 인박스에 다시 올랐다"
        );
        // 끝의 `/` 하나로 같은 경로가 새것처럼 보이지 않는다.
        let slashed = ["/scratch/one/", "/scratch/four"];
        assert_eq!(inbox_paths(&slashed, &stored, None), vec!["/scratch/four"]);
        // 기준선은 중복을 만들지 않는다.
        merge_baseline(&mut stored, &["/scratch/one", "/scratch/three"]);
        assert_eq!(stored.inbox_baseline_paths.len(), 3);
        assert_eq!(inbox_paths(&hidden, &stored, None), Vec::<&str>::new());
    }

    /// 인박스를 내밀 자격이 없으면 차집합도 계산하지 않는다.
    #[test]
    fn a_suppressed_project_has_no_inbox_however_many_appeared() {
        let quiet = Stored {
            visibility: Some(Visibility::Hide),
            prompt_dismissed_at: Some(5),
            discovery_suppressed_at: Some(9),
            ..Stored::default()
        };
        assert_eq!(
            inbox_paths(&["/scratch/new"], &quiet, None),
            Vec::<&str>::new()
        );
        // 그리고 가져오기 하나가 그 문을 다시 연다.
        let mut lifted = quiet.clone();
        import_clears_suppression(&mut lifted, &["/scratch/new"]);
        assert_eq!(
            inbox_paths(&["/scratch/other"], &lifted, None),
            vec!["/scratch/other"]
        );
    }

    /// 네 분류의 이름은 창이 읽는 철자와 같다.
    #[test]
    fn every_ownership_has_one_spelling() {
        let names: Vec<&str> = [
            Ownership::ZerocodeManaged,
            Ownership::AgentScratch,
            Ownership::UnknownLegacy,
            Ownership::External,
        ]
        .iter()
        .map(|one| one.slug())
        .collect();
        assert_eq!(
            names,
            [
                "zerocode-managed",
                "agent-scratch",
                "unknown-legacy",
                "external"
            ]
        );
        for name in &names {
            assert!(
                !name.contains("orca"),
                "`{name}`이 남의 제품 이름을 들고 있다"
            );
        }
        assert_eq!(Visibility::Hide.slug(), "hide");
        assert_eq!(Visibility::Show.slug(), "show");
    }

    /// 저장 레코드는 빈 채로 파일에 흔적을 남기지 않는다.
    ///
    /// `{}`가 파일에 있으면 그것은 "이 저장소에 대해 뭔가 말했다"로 읽힌다.
    #[test]
    fn an_untouched_project_serialises_to_nothing() {
        let empty = serde_json::to_string(&Stored::default()).expect("직렬화");
        assert_eq!(empty, "{}");
        let read: Stored = serde_json::from_str("{}").expect("역직렬화");
        assert_eq!(read, Stored::default());
        // 그리고 필드 이름은 창이 읽는 철자다.
        let full = serde_json::to_string(&Stored {
            visibility: Some(Visibility::Show),
            legacy: Some(true),
            prompt_dismissed_at: Some(1),
            discovery_suppressed_at: Some(2),
            inbox_baseline_paths: vec!["/a".to_string()],
            imported_paths: vec!["/b".to_string()],
        })
        .expect("직렬화");
        for spelt in [
            "\"visibility\":\"show\"",
            "\"legacy\":true",
            "\"prompt_dismissed_at\":1",
            "\"discovery_suppressed_at\":2",
            "\"inbox_baseline_paths\":[\"/a\"]",
            "\"imported_paths\":[\"/b\"]",
        ] {
            assert!(full.contains(spelt), "`{spelt}`이 사라졌다:\n{full}");
        }
    }
}
