//! 비활성 워크스페이스 삭제 — 어떤 체크아웃을 지워도 되는지 정하는 규칙.
//!
//! Orca의 `Delete Inactive Workspaces` 다이얼로그가 하는 판정을 그대로 옮긴
//! 것이고, 옮겨 온 것은 **규칙뿐**이다. 워크트리를 훑고 git을 부르고 파일의
//! mtime을 읽는 일은 창(`zerocode-shell`)이 한다. 여기 있는 것은 그렇게 모은
//! 사실 하나를 받아 "제안 / 리뷰 필요 / 제안되지 않음 / 무시됨" 중 하나를
//! 답하는, 시계도 디스크도 만지지 않는 함수들이다.
//!
//! 나눠 둔 이유는 하나다. 이 판정이 틀리면 **사람의 파일이 사라진다.** 경계값
//! (7일·30일), 강제 삭제가 필요한 조건, 지문이 바뀌면 무시가 풀린다는 약속 —
//! 셋 다 테스트가 붙잡고 있어야 하는 규칙이고, 셋 다 git 저장소를 만들지 않고는
//! 시험할 수 없는 코드 안에 있으면 아무도 시험하지 않게 된다.
//!
//! ## 티어는 사유와 방해물의 곱이다
//!
//! - **사유**([`Reason`])는 "왜 이걸 후보로 올렸나"이다. 사유가 하나도 없으면
//!   후보가 아니고, 창은 그런 워크트리를 아예 보내지 않는다.
//! - **방해물**([`Blocker`])은 "왜 지우면 안 되나"이다. 하나라도 있으면 제안하지
//!   않는다 — 제안이란 체크박스가 미리 켜져 있다는 뜻이고, 켜져 있는 체크박스는
//!   사람이 읽지 않는다.
//! - 사유가 있고 방해물이 없고 git이 **증명할 수 있게** 깨끗하면 그때만 제안한다.
//!   "더러운 것을 못 찾았다"와 "깨끗함을 증명했다"는 다른 말이고,
//!   [`GitEvidence::provably_clean`]이 붙잡는 것이 그 차이다.

use serde::{Deserialize, Serialize};

/// 분류 규칙의 판(版).
///
/// 지문([`fingerprint`])에 섞여 들어간다. 규칙이 바뀌면 예전 규칙 아래에서 눌러
/// 둔 "무시"는 스스로 풀려야 한다 — 지금은 위험하다고 볼 워크스페이스를, 예전
/// 규칙에서 한 번 무시했다는 이유로 영영 숨기는 것이 최악이다.
pub const CLASSIFIER_VERSION: u32 = 1;

/// 하루, 밀리초.
pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// 보관된 워크스페이스가 후보가 되는 유휴 기간.
pub const ARCHIVED_IDLE_DAYS: i64 = 7;

/// 아무 표시도 없이 그냥 오래 손대지 않은 워크스페이스가 후보가 되는 기간.
pub const IDLE_CLEAN_DAYS: i64 = 30;

/// 왜 이 워크스페이스가 목록에 올라왔는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reason {
    /// 보관된 채로 [`ARCHIVED_IDLE_DAYS`]일 넘게 조용하다.
    Archived,
    /// 보관 표시는 없지만 [`IDLE_CLEAN_DAYS`]일 넘게 조용하다.
    IdleClean,
}

impl Reason {
    /// 창이 스타일과 문구를 붙이는 이름.
    pub const fn slug(self) -> &'static str {
        match self {
            Reason::Archived => "archived",
            Reason::IdleClean => "idle-clean",
        }
    }
}

/// 왜 이 워크스페이스를 지우자고 **제안하면 안 되는가**.
///
/// 전부 하드 블로커다. 정도의 차이가 아니라 하나라도 있으면 제안하지 않는다는
/// 뜻이고, 그래서 이 열거형에는 "약한" 항목이 없다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Blocker {
    /// 저장소 자신의 체크아웃. git이 지우지도 못한다.
    MainWorktree,
    /// 누군가 일부러 잠갔다(`git worktree lock`).
    Pinned,
    /// 창이 지금 서 있는 워크스페이스.
    ActiveWorkspace,
    /// 이 체크아웃에서 아직 무언가 돌고 있다.
    RunningTerminal,
    /// 이 체크아웃의 문서 중 저장되지 않은 편집을 들고 있는 것이 있다.
    ///
    /// git이 모르는 유일한 방해물이다 — 타이핑은 아직 파일에 닿지 않았으므로
    /// `status`는 깨끗하다고 답하고, `git worktree remove`는 `--force` 없이도
    /// 그 디렉터리를 지운다. 아는 것은 창뿐이고, 그래서 이 사실은 창이 실어
    /// 보낸다. 판정은 그래도 여기서 한다.
    UnsavedEdits,
    /// 커밋되지 않은 변경이 있다.
    DirtyFiles,
    /// 어디에도 밀어 두지 않은 커밋이 있다.
    UnpushedCommits,
    /// 이 브랜치가 어디에 서 있는지 git이 답하지 못했다. 모른다는 것은
    /// 괜찮다는 뜻이 아니다.
    UnknownBase,
    /// `git status` 자체가 실패했다.
    GitStatusError,
    /// 사람이 "이건 놔둬"라고 말했다.
    Dismissed,
}

impl Blocker {
    /// 창이 스타일과 문구를 붙이는 이름.
    pub const fn slug(self) -> &'static str {
        match self {
            Blocker::MainWorktree => "main-worktree",
            Blocker::Pinned => "pinned",
            Blocker::ActiveWorkspace => "active-workspace",
            Blocker::RunningTerminal => "running-terminal",
            Blocker::UnsavedEdits => "unsaved-edits",
            Blocker::DirtyFiles => "dirty-files",
            Blocker::UnpushedCommits => "unpushed-commits",
            Blocker::UnknownBase => "unknown-base",
            Blocker::GitStatusError => "git-status-error",
            Blocker::Dismissed => "dismissed",
        }
    }

    /// 이 방해물이 있으면 제거에 `--force`가 필요한가.
    ///
    /// 잃을 것이 있다는 뜻의 방해물만 참이다. 잠긴 워크트리나 활성 워크스페이스는
    /// 지우면 안 되는 이유이지 강제로 지워야 하는 이유가 아니다.
    pub const fn needs_force(self) -> bool {
        matches!(
            self,
            Blocker::DirtyFiles
                | Blocker::UnpushedCommits
                | Blocker::UnknownBase
                | Blocker::GitStatusError
        )
    }
}

/// 목록의 어느 뷰에 들어가는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tier {
    /// 사유가 있고, 방해물이 없고, 깨끗함이 증명됐다. 체크박스가 미리 켜진다.
    Suggested,
    /// 후보이긴 한데 증명이 모자란다. 사람이 열어 보고 정한다.
    NeedsReview,
    /// 방해물이 있다. 목록에는 있되 제안하지 않는다.
    NotSuggested,
    /// 사람이 눌러 둔 것. 지문이 바뀌면 스스로 돌아온다.
    Ignored,
}

impl Tier {
    pub const fn slug(self) -> &'static str {
        match self {
            Tier::Suggested => "suggested",
            Tier::NeedsReview => "needs-review",
            Tier::NotSuggested => "not-suggested",
            Tier::Ignored => "ignored",
        }
    }
}

/// git에게 물어서 알아낸 것.
///
/// [`Default`]가 "깨끗함"이 아니라 **"모른다"** 인 것이 이 타입의 요점이다.
/// 증거를 모으다 만 값이 기본값으로 남으면, 그 워크스페이스는 아무도 확인하지
/// 않은 채로 제안 목록에 올라간다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitEvidence {
    /// `status --porcelain=v2 --untracked-files=all`이 센 파일 수.
    pub dirty_files: usize,
    /// 이 브랜치에 upstream이 있는가.
    pub has_upstream: bool,
    /// upstream보다 앞선 커밋 수.
    pub ahead: usize,
    /// upstream이 없을 때 `rev-list --count HEAD --not --remotes`가 0보다 컸다.
    pub unpushed_commits: bool,
    /// 그 물음에 git이 답하지 못했다.
    pub unknown_base: bool,
    /// `git status`가 실패했다.
    pub errored: bool,
}

impl Default for GitEvidence {
    fn default() -> Self {
        Self::unknown()
    }
}

impl GitEvidence {
    /// 아직 아무것도 묻지 않은 상태.
    pub const fn unknown() -> Self {
        Self {
            dirty_files: 0,
            has_upstream: false,
            ahead: 0,
            unpushed_commits: false,
            unknown_base: true,
            errored: false,
        }
    }

    /// 물었고, 잃을 것이 없다는 답을 받은 상태. 테스트와 창이 함께 쓴다.
    pub const fn clean() -> Self {
        Self {
            dirty_files: 0,
            has_upstream: true,
            ahead: 0,
            unpushed_commits: false,
            unknown_base: false,
            errored: false,
        }
    }

    /// **증명된** 깨끗함.
    ///
    /// 네 가지가 전부 아니라고 답했을 때만 참이다. 하나라도 모르면 거짓이고,
    /// 거짓이면 제거는 강제 삭제가 된다 — [`Verdict::force`]가 이 값의 부정인
    /// 것은 우연이 아니라 정의다.
    pub const fn provably_clean(&self) -> bool {
        !self.errored
            && self.dirty_files == 0
            && self.ahead == 0
            && !self.unpushed_commits
            && !self.unknown_base
    }
}

/// 한 워크트리에 대해 창이 모아 온 사실 전부.
///
/// 이 구조체가 분류의 입력이고, 여기 없는 것은 판정에 쓰이지 않는다.
///
/// [`Default`]는 파생이지만 무해하지 않다: [`GitEvidence`]의 기본값이 "모른다"
/// 이므로, 아무것도 채우지 않은 사실은 제안되지 않는 쪽으로 떨어진다. 그 성질은
/// 파생 하나에 얹혀 있으니 테스트가 붙잡는다.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorktreeFacts {
    /// 저장소 자신의 체크아웃.
    pub is_main: bool,
    /// `git worktree lock`으로 잠겼다.
    pub pinned: bool,
    /// 창이 지금 이 안에 서 있다.
    pub active: bool,
    /// 이 체크아웃에서 도는 레인이 살아 있다.
    pub running_terminal: bool,
    /// 이 체크아웃의 문서 하나가 저장되지 않은 타이핑을 들고 있다.
    ///
    /// 창만이 아는 사실이다. 채우지 않으면 거짓이고, 거짓은 "저장 안 된 편집이
    /// 없다"가 아니라 "묻지 않았다"에 가깝다 — 그래서 이 값을 채우지 않는
    /// 호출자는 자기가 무엇을 놓치는지 알아야 한다.
    pub unsaved_edits: bool,
    /// 보관된 워크스페이스다.
    ///
    /// ZeroCode에는 Orca의 "보관함"이 없으므로, 창은 이 자리에 자기가 가진 가장
    /// 가까운 사실을 넣는다: 자동화가 만들었고 그 실행이 끝난 것이 목격된
    /// 체크아웃 — 즉 아무도 남기기로 고르지 않은 워크스페이스. 그 사상(寫像)은
    /// 창의 몫이고, 여기서는 참/거짓 하나로만 읽는다.
    pub archived: bool,
    /// 마지막 활동 시각(에포크 밀리초). [`last_activity`]가 만든다.
    pub last_activity_ms: i64,
    /// 사람이 눌러 둔 무시가 아직 유효한가. [`dismissal_holds`]가 만든다.
    pub dismissed: bool,
    /// git에게 물어 알아낸 것.
    pub git: GitEvidence,
}

/// 한 워크트리에 대한 판정.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub tier: Tier,
    pub reasons: Vec<Reason>,
    pub blockers: Vec<Blocker>,
    /// 며칠 조용했나. 칩에 그대로 실린다.
    pub idle_days: i64,
    /// 제거할 때 `--force`가 필요한가.
    pub force: bool,
}

/// 마지막 활동 시각 — 저장된 것과 파일들이 말하는 것 중 **가장 나중**.
///
/// 최댓값인 이유: 어느 한쪽만 읽으면 살아 있는 워크스페이스가 죽은 것처럼
/// 보인다. 자동화 이력에만 물으면 사람이 손으로 작업한 30일이 안 보이고,
/// mtime에만 물으면 파일을 건드리지 않고 커밋만 읽던 날들이 안 보인다. 하나라도
/// "최근"이라고 말하면 최근인 것이 안전한 방향이다.
///
/// `None`은 "모른다"이고 0으로 접히지 않는다 — 읽지 못한 mtime이 1970년으로
/// 세어지면 그 워크스페이스는 언제나 후보가 된다.
pub fn last_activity(stored: Option<i64>, marks: &[Option<i64>]) -> i64 {
    marks
        .iter()
        .copied()
        .chain(std::iter::once(stored))
        .flatten()
        .max()
        .unwrap_or(0)
}

/// 며칠 조용했나.
///
/// 미래의 시각은 0일로 접는다. 시계가 어긋난 기계나 네트워크 마운트가 내일
/// 날짜의 mtime을 답하는 일은 흔하고, 그것을 음수 유휴로 계산하면 경계 비교가
/// 조용히 뒤집힌다.
pub fn idle_days(last_activity_ms: i64, now_ms: i64) -> i64 {
    (now_ms - last_activity_ms).max(0) / DAY_MS
}

/// 후보로 올릴 사유들. 비어 있으면 후보가 아니다.
pub fn reasons(archived: bool, idle_days: i64) -> Vec<Reason> {
    let mut found = Vec::new();
    if archived && idle_days >= ARCHIVED_IDLE_DAYS {
        found.push(Reason::Archived);
    }
    if idle_days >= IDLE_CLEAN_DAYS {
        found.push(Reason::IdleClean);
    }
    found
}

/// 제안을 막는 것들. 순서는 사람이 먼저 읽어야 하는 순서다.
pub fn blockers(facts: &WorktreeFacts) -> Vec<Blocker> {
    let mut found = Vec::new();
    if facts.is_main {
        found.push(Blocker::MainWorktree);
    }
    if facts.pinned {
        found.push(Blocker::Pinned);
    }
    if facts.active {
        found.push(Blocker::ActiveWorkspace);
    }
    if facts.running_terminal {
        found.push(Blocker::RunningTerminal);
    }
    if facts.unsaved_edits {
        found.push(Blocker::UnsavedEdits);
    }
    if facts.git.dirty_files > 0 {
        found.push(Blocker::DirtyFiles);
    }
    // upstream이 있는데 앞서 있는 것과, upstream이 없는데 어디에도 없는 커밋을
    // 들고 있는 것은 사람에게 같은 사실이다 — 밀어 두지 않은 작업이 있다.
    if facts.git.unpushed_commits || facts.git.ahead > 0 {
        found.push(Blocker::UnpushedCommits);
    }
    if facts.git.unknown_base {
        found.push(Blocker::UnknownBase);
    }
    if facts.git.errored {
        found.push(Blocker::GitStatusError);
    }
    if facts.dismissed {
        found.push(Blocker::Dismissed);
    }
    found
}

/// 사실 하나를 티어 하나로.
pub fn classify(facts: &WorktreeFacts, now_ms: i64) -> Verdict {
    let idle = idle_days(facts.last_activity_ms, now_ms);
    let reasons = reasons(facts.archived, idle);
    let blockers = blockers(facts);
    let clean = facts.git.provably_clean();
    let tier = if facts.dismissed {
        // 무시된 것은 "제안되지 않음"이 아니라 **따로 있는 서랍**이다. 사람이
        // 치워 둔 것을 못 지우는 것들 사이에 섞으면, 그 목록은 다시 읽히지
        // 않는다.
        Tier::Ignored
    } else if !blockers.is_empty() {
        Tier::NotSuggested
    } else if !reasons.is_empty() && clean {
        Tier::Suggested
    } else {
        Tier::NeedsReview
    };
    Verdict {
        tier,
        reasons,
        blockers,
        idle_days: idle,
        // 증명되지 않은 것은 전부 강제 삭제가 필요하다. 방해물 목록을 따로
        // 훑지 않는 이유는 두 답이 어긋날 수 있기 때문이고, 어긋나면 강제
        // 플래그 없이 지워질 수 있다는 뜻이 아니라 그 반대 — 아무 경고 없이
        // `--force`가 붙는다는 뜻이다.
        force: !clean,
    }
}

/// 사람이 무시한 그 순간의 워크스페이스.
///
/// 경로가 아니라 **상태**를 적어 둔다. 무시란 "지금 이 모습의 이 워크스페이스는
/// 놔둬"이지 "이 디렉터리는 영원히 묻지 마"가 아니다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dismissal {
    /// 워크트리의 안정된 이름 — 창은 경로를 쓴다.
    pub worktree_id: String,
    /// 무시한 순간의 [`fingerprint`].
    pub fingerprint: String,
    /// 그때의 [`CLASSIFIER_VERSION`].
    pub classifier_version: u32,
}

/// 무시 여부를 판가름하는 지문.
///
/// 브랜치·HEAD·깨끗함·활동한 날(일 단위)을 하나로 묶는다. 넷 중 하나라도 바뀌면
/// 다른 문자열이 되고, 다른 문자열이면 [`dismissal_holds`]가 거짓을 답해 그
/// 워크스페이스는 스스로 목록에 돌아온다.
///
/// 활동 시각을 **밀리초가 아니라 날짜로** 접는 것이 요점이다. 밀리초를 넣으면
/// 스캔이 mtime을 다시 읽을 때마다 지문이 흔들려 무시가 한 번도 유지되지
/// 못한다.
pub fn fingerprint(
    branch: Option<&str>,
    head: Option<&str>,
    provably_clean: bool,
    last_activity_ms: i64,
) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        CLASSIFIER_VERSION,
        branch.unwrap_or("-"),
        head.unwrap_or("-"),
        if provably_clean { "clean" } else { "dirty" },
        last_activity_ms.div_euclid(DAY_MS),
    )
}

/// 저장해 둔 무시가 아직 이 워크스페이스에 대한 말인가.
///
/// 판이 다르면 곧바로 거짓이다. 지문에 판이 이미 섞여 있으므로 이 비교는
/// 남는 것처럼 보이지만, 남겨 두는 편이 낫다: 지문의 짜임이 언젠가 바뀌어도
/// "예전 판의 무시는 무시가 아니다"라는 약속은 이 줄이 혼자 지킨다.
pub fn dismissal_holds(held: Option<&Dismissal>, fingerprint: &str) -> bool {
    held.is_some_and(|one| {
        one.classifier_version == CLASSIFIER_VERSION && one.fingerprint == fingerprint
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 유휴 30일을 이 시각 기준으로 만들어 주는 도우미.
    fn days_ago(now_ms: i64, days: i64) -> i64 {
        now_ms - days * DAY_MS
    }

    const NOW: i64 = 1_800_000_000_000;

    #[test]
    fn a_reason_needs_the_days_it_names() {
        // 7일 경계: 6일은 아니고, 7일은 맞다.
        assert_eq!(reasons(true, 6), vec![]);
        assert_eq!(reasons(true, 7), vec![Reason::Archived]);
        // 보관되지 않았으면 7일로는 아무것도 아니다.
        assert_eq!(reasons(false, 7), vec![]);
        // 30일 경계.
        assert_eq!(reasons(false, 29), vec![]);
        assert_eq!(reasons(false, 30), vec![Reason::IdleClean]);
        // 보관된 채 30일이면 둘 다 — 목록은 이유를 하나로 줄이지 않는다.
        assert_eq!(reasons(true, 30), vec![Reason::Archived, Reason::IdleClean]);
    }

    #[test]
    fn the_last_activity_is_the_latest_of_everything_that_spoke() {
        // 저장된 것이 더 최근이면 그것이 답이다.
        assert_eq!(last_activity(Some(500), &[Some(100), Some(300)]), 500);
        // 파일이 더 최근이면 파일이 답이다.
        assert_eq!(last_activity(Some(100), &[Some(900), Some(300)]), 900);
        // 읽지 못한 것은 0이 아니라 없는 것이다 — 하나라도 답하면 그 값이 산다.
        assert_eq!(last_activity(None, &[None, Some(7)]), 7);
        // 아무도 답하지 않으면 그때만 0.
        assert_eq!(last_activity(None, &[None, None]), 0);
    }

    #[test]
    fn a_clock_that_ran_backwards_does_not_make_a_negative_idle() {
        assert_eq!(idle_days(NOW + 5 * DAY_MS, NOW), 0);
        assert_eq!(idle_days(days_ago(NOW, 30), NOW), 30);
        // 하루에서 1밀리초 모자라면 아직 그 하루가 아니다.
        assert_eq!(idle_days(NOW - (DAY_MS - 1), NOW), 0);
    }

    #[test]
    fn a_clean_idle_workspace_is_the_only_thing_suggested() {
        let facts = WorktreeFacts {
            last_activity_ms: days_ago(NOW, 31),
            git: GitEvidence::clean(),
            ..WorktreeFacts::default()
        };
        let said = classify(&facts, NOW);
        assert_eq!(said.tier, Tier::Suggested);
        assert_eq!(said.reasons, vec![Reason::IdleClean]);
        assert!(said.blockers.is_empty());
        assert!(!said.force, "증명된 깨끗함에 강제 삭제가 붙었다");
    }

    #[test]
    fn a_hard_blocker_takes_a_workspace_out_of_the_suggestions() {
        let clean_and_idle = WorktreeFacts {
            last_activity_ms: days_ago(NOW, 60),
            git: GitEvidence::clean(),
            ..WorktreeFacts::default()
        };
        for (label, facts) in [
            (
                "main",
                WorktreeFacts {
                    is_main: true,
                    ..clean_and_idle.clone()
                },
            ),
            (
                "pinned",
                WorktreeFacts {
                    pinned: true,
                    ..clean_and_idle.clone()
                },
            ),
            (
                "active",
                WorktreeFacts {
                    active: true,
                    ..clean_and_idle.clone()
                },
            ),
            (
                "running",
                WorktreeFacts {
                    running_terminal: true,
                    ..clean_and_idle.clone()
                },
            ),
            (
                "unsaved-edits",
                WorktreeFacts {
                    unsaved_edits: true,
                    ..clean_and_idle.clone()
                },
            ),
            (
                "dirty",
                WorktreeFacts {
                    git: GitEvidence {
                        dirty_files: 3,
                        ..GitEvidence::clean()
                    },
                    ..clean_and_idle.clone()
                },
            ),
            (
                "ahead",
                WorktreeFacts {
                    git: GitEvidence {
                        ahead: 2,
                        ..GitEvidence::clean()
                    },
                    ..clean_and_idle.clone()
                },
            ),
            (
                "unpushed",
                WorktreeFacts {
                    git: GitEvidence {
                        has_upstream: false,
                        unpushed_commits: true,
                        ..GitEvidence::clean()
                    },
                    ..clean_and_idle.clone()
                },
            ),
            (
                "unknown-base",
                WorktreeFacts {
                    git: GitEvidence::unknown(),
                    ..clean_and_idle.clone()
                },
            ),
            (
                "status-error",
                WorktreeFacts {
                    git: GitEvidence {
                        errored: true,
                        ..GitEvidence::clean()
                    },
                    ..clean_and_idle.clone()
                },
            ),
        ] {
            let said = classify(&facts, NOW);
            assert_eq!(
                said.tier,
                Tier::NotSuggested,
                "{label}이(가) 있는데도 제안됐다: {said:?}"
            );
        }
    }

    /// FORCE 집합은 "잃을 것이 있다"고 말하는 넷뿐이다.
    #[test]
    fn force_is_exactly_the_absence_of_proof() {
        let idle = WorktreeFacts {
            last_activity_ms: days_ago(NOW, 40),
            ..WorktreeFacts::default()
        };
        for evidence in [
            GitEvidence {
                dirty_files: 1,
                ..GitEvidence::clean()
            },
            GitEvidence {
                ahead: 1,
                ..GitEvidence::clean()
            },
            GitEvidence {
                unpushed_commits: true,
                ..GitEvidence::clean()
            },
            GitEvidence::unknown(),
            GitEvidence {
                errored: true,
                ..GitEvidence::clean()
            },
        ] {
            let facts = WorktreeFacts {
                git: evidence,
                ..idle.clone()
            };
            assert!(
                classify(&facts, NOW).force,
                "증명되지 않았는데 강제 삭제가 필요 없다고 했다: {evidence:?}"
            );
            assert!(
                blockers(&facts).iter().any(|one| one.needs_force()),
                "강제 삭제가 필요한데 그 이유를 말해 주는 방해물이 없다: {evidence:?}"
            );
        }
        // 그리고 잠금·활성은 강제 삭제의 이유가 아니다.
        assert!(!Blocker::Pinned.needs_force());
        assert!(!Blocker::ActiveWorkspace.needs_force());
        assert!(!Blocker::MainWorktree.needs_force());
        assert!(!Blocker::RunningTerminal.needs_force());
        assert!(!Blocker::Dismissed.needs_force());
        // 저장 안 된 편집도 아니다. git은 그것을 모르므로 `--force`를 붙여도
        // 달라지는 것이 없고, 붙이면 **다른** 것들이 함께 버려진다.
        assert!(!Blocker::UnsavedEdits.needs_force());
    }

    /// 사유는 있는데 증명이 없으면 제안이 아니라 "리뷰 필요"다.
    #[test]
    fn an_unproven_candidate_waits_for_a_person() {
        let facts = WorktreeFacts {
            last_activity_ms: days_ago(NOW, 45),
            git: GitEvidence {
                // 방해물이 되지는 않지만 증명도 아닌 자리: upstream이 없고
                // 밀지 않은 커밋도 없다고 확인된 상태.
                has_upstream: false,
                unknown_base: false,
                ..GitEvidence::clean()
            },
            ..WorktreeFacts::default()
        };
        assert_eq!(classify(&facts, NOW).tier, Tier::Suggested);

        // 사유가 없으면 후보가 아니다 — 창은 이런 행을 보내지 않지만, 규칙
        // 자체도 그것을 제안으로 만들지 않는다.
        let fresh = WorktreeFacts {
            last_activity_ms: days_ago(NOW, 2),
            git: GitEvidence::clean(),
            ..WorktreeFacts::default()
        };
        let said = classify(&fresh, NOW);
        assert!(said.reasons.is_empty());
        assert_eq!(said.tier, Tier::NeedsReview);
    }

    /// 채우지 않은 사실은 안전한 쪽으로 떨어진다.
    #[test]
    fn an_unfilled_fact_is_never_a_suggestion() {
        let bare = WorktreeFacts::default();
        assert_eq!(bare.git, GitEvidence::unknown());
        assert!(!bare.git.provably_clean());
        // 유휴 기간이 아무리 길어도 증거가 없으면 제안이 아니다.
        let old = WorktreeFacts {
            last_activity_ms: days_ago(NOW, 400),
            ..WorktreeFacts::default()
        };
        assert_eq!(classify(&old, NOW).tier, Tier::NotSuggested);
        assert!(classify(&old, NOW).force);
    }

    #[test]
    fn a_dismissed_workspace_has_its_own_drawer() {
        let facts = WorktreeFacts {
            last_activity_ms: days_ago(NOW, 90),
            dismissed: true,
            git: GitEvidence::clean(),
            ..WorktreeFacts::default()
        };
        let said = classify(&facts, NOW);
        assert_eq!(said.tier, Tier::Ignored);
        assert!(said.blockers.contains(&Blocker::Dismissed));
    }

    #[test]
    fn a_dismissal_lets_go_when_the_workspace_moves() {
        let at = days_ago(NOW, 40);
        let mark = fingerprint(Some("wt/drain"), Some("abc123"), true, at);
        let held = Dismissal {
            worktree_id: "/tmp/wt/drain".to_string(),
            fingerprint: mark.clone(),
            classifier_version: CLASSIFIER_VERSION,
        };
        assert!(dismissal_holds(Some(&held), &mark));

        // 커밋이 하나 더 얹히면 다른 지문.
        let moved = fingerprint(Some("wt/drain"), Some("def456"), true, at);
        assert!(!dismissal_holds(Some(&held), &moved));
        // 더러워져도 다른 지문.
        let dirtied = fingerprint(Some("wt/drain"), Some("abc123"), false, at);
        assert!(!dismissal_holds(Some(&held), &dirtied));
        // 다른 날 손을 대도 다른 지문.
        let later = fingerprint(Some("wt/drain"), Some("abc123"), true, at + DAY_MS);
        assert!(!dismissal_holds(Some(&held), &later));
        // 같은 날 안에서 밀리초만 흔들리는 것은 같은 지문 — 아니면 무시가
        // 한 번도 유지되지 못한다.
        let same_day = fingerprint(Some("wt/drain"), Some("abc123"), true, at + 1_000);
        assert!(dismissal_holds(Some(&held), &same_day));
        // 아무것도 저장돼 있지 않으면 무시가 아니다.
        assert!(!dismissal_holds(None, &mark));
    }

    #[test]
    fn an_old_classifier_version_releases_what_it_pinned() {
        let mark = fingerprint(Some("wt/drain"), Some("abc123"), true, 0);
        let stale = Dismissal {
            worktree_id: "/tmp/wt/drain".to_string(),
            fingerprint: mark.clone(),
            classifier_version: CLASSIFIER_VERSION + 1,
        };
        assert!(!dismissal_holds(Some(&stale), &mark));
        // 그리고 판은 지문 자체에도 들어 있다.
        assert!(mark.starts_with(&format!("{CLASSIFIER_VERSION}|")));
    }

    #[test]
    fn every_slug_is_its_own() {
        let mut seen: Vec<&str> = vec![
            Blocker::MainWorktree.slug(),
            Blocker::Pinned.slug(),
            Blocker::ActiveWorkspace.slug(),
            Blocker::RunningTerminal.slug(),
            Blocker::UnsavedEdits.slug(),
            Blocker::DirtyFiles.slug(),
            Blocker::UnpushedCommits.slug(),
            Blocker::UnknownBase.slug(),
            Blocker::GitStatusError.slug(),
            Blocker::Dismissed.slug(),
        ];
        let held = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), held, "두 방해물이 같은 이름을 쓴다");
        // 그리고 이름은 wire 표현과 같아야 한다 — 창이 그것으로 스타일을 건다.
        assert_eq!(
            serde_json::to_string(&Blocker::UnknownBase).unwrap(),
            "\"unknown-base\""
        );
        assert_eq!(
            serde_json::to_string(&Tier::NeedsReview).unwrap(),
            "\"needs-review\""
        );
        assert_eq!(
            serde_json::to_string(&Reason::IdleClean).unwrap(),
            "\"idle-clean\""
        );
    }
}
