//! 스마트 입력 한 줄이 무엇인가 — **네트워크 없이** 답하는 부분만.
//!
//! 사람은 이 칸에 네 가지를 넣는다: 이름(`로그인 리다이렉트 고치기`), 번호
//! (`#1234`), 링크(GitHub·GitLab·Jira·Linear), 그리고 브랜치 이름. Orca는 이
//! 넷을 `isWorkItemLookupText` 하나로 가른 다음 **조회**로 넘어가지만, 조회는
//! 네트워크다. 여기 있는 것은 그 앞의 절반 — **모양만 보고** 종류·번호·이름
//! 씨앗을 정하는 부분이고, 이것만으로 워크스페이스 하나를 만들 수 있다.
//!
//! 그래서 이 모듈의 계약은 셋이다:
//!
//!   1. **판정은 순수하다.** 입력 문자열 하나가 들어오고 값 하나가 나간다.
//!      디스크도, 시계도, 소켓도 읽지 않는다 — 그래서 표 하나로 시험된다.
//!   2. **모르면 자유 텍스트다.** 링크처럼 생겼는데 우리가 아는 모양이
//!      아니면 그것은 이름이다. 사람이 친 것을 거부하는 화면보다 이름으로
//!      받아 주는 화면이 낫고, 조회가 붙는 슬라이스에서 같은 술어가 그대로
//!      쓰인다.
//!   3. **브랜치 슬러그와 다른 함수다.** [`slugify_for_workspace_name`]은
//!      `.`과 `_`, 유니코드 낱말을 남기고 48자에서 자른다(Orca
//!      `slugifyForWorkspaceName`). 워커 브랜치·체크아웃 이름은
//!      `zerocode_orchestrator::naming::slugify`가 ASCII 낱말만 남겨 만든다.
//!      하나로 합치면 둘 중 하나가 틀린다. 워크스페이스 이름과 브랜치
//!      이름은 서로 다른 두 이름이라는 사실이 그 두 함수다.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 이름 씨앗의 최대 길이. Orca의 48자 컷과 같은 값이고, 브랜치 슬러그의
/// 상한(`naming::MAX_SLUG_CHARS`)과 우연히 같은 것이 아니라 **같은 경로가
/// 두 이름을 다 담기 때문에** 같다.
pub const MAX_SEED_CHARS: usize = 48;

/// 알아본 작업 항목의 종류. 슬라이스 A는 조회를 하지 않으므로 이 값이 하는
/// 일은 배지 한 줄과 이름 씨앗뿐이다 — 그러나 종류를 지금 정확히 갈라 두어야
/// 조회가 붙는 날 이 판정기를 다시 쓰지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkItemKind {
    /// GitHub 이슈, 또는 번호만 적힌 `#1234`.
    Issue,
    /// GitHub 풀 리퀘스트.
    Pr,
    /// GitLab 머지 리퀘스트.
    MergeRequest,
    /// Jira 이슈 — 번호가 아니라 키(`ABC-9`)로 불린다.
    Jira,
    /// Linear 이슈. 역시 키로 불린다.
    Linear,
}

impl WorkItemKind {
    /// 이 종류의 기본 동사. Orca의 `ACTION_LABELS` 기본값과 같은 자리다 —
    /// PR/MR은 읽으러 가는 것이고(Review), 이슈는 고치러 가는 것이다.
    const fn verb(self) -> &'static str {
        match self {
            WorkItemKind::Pr | WorkItemKind::MergeRequest => "Review",
            _ => "Fix",
        }
    }
}

/// 입력에서 알아본 것 하나.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkItem {
    pub kind: WorkItemKind,
    /// 번호로 불리는 항목의 번호. Jira·Linear는 [`None`]이다.
    pub number: Option<u64>,
    /// 키로 불리는 항목의 키(`ABC-9`). 번호로 불리는 항목은 [`None`].
    pub key: Option<String>,
    /// `owner/repo` — 링크에서만 알 수 있다. 번호만 적힌 입력은 [`None`].
    pub repo: Option<String>,
}

/// Maximum number of orchestration attempts retained on one linked item.
pub const LINKED_ORCHESTRATION_HISTORY_MAX: usize = 64;

/// One provider-specific item attached to a local worktree.
///
/// Internally tagged and flattened by [`LinkedWorkItem`], so every renderer and
/// persistence consumer sees the same `provider` discriminator beside the
/// provider's own stable identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
pub enum LinkedWorkItemSource {
    Jira {
        site_id: String,
        key: String,
        url: String,
        title: String,
        status: String,
        updated: Option<String>,
    },
}

/// Jira writes that may follow lifecycle events for this linked worktree.
///
/// Every action is opt-in. Transition ids come from Jira's own available-
/// transitions response for this exact issue; ZeroCode never guesses workflow
/// names such as "Done" or assumes two Jira projects share one transition id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JiraSyncPolicy {
    pub start_transition_id: Option<String>,
    pub success_transition_id: Option<String>,
    pub failure_transition_id: Option<String>,
    pub comment_on_success: bool,
    pub comment_on_failure: bool,
}

impl JiraSyncPolicy {
    #[must_use]
    pub fn is_inert(&self) -> bool {
        self.start_transition_id.is_none()
            && self.success_transition_id.is_none()
            && self.failure_transition_id.is_none()
            && !self.comment_on_success
            && !self.comment_on_failure
    }

    pub fn validate(&self) -> Result<(), String> {
        for id in [
            self.start_transition_id.as_deref(),
            self.success_transition_id.as_deref(),
            self.failure_transition_id.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if !valid_opaque_word(id, 128) {
                return Err("Jira transition id is not a bounded opaque word".to_string());
            }
        }
        Ok(())
    }
}

/// The real Run/Task/Dispatch ids observed after a linked coordinator starts
/// workers. They are observations, never ids predicted by the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LinkedOrchestration {
    pub requested: bool,
    pub run_ids: Vec<String>,
    pub task_ids: Vec<String>,
    pub dispatch_ids: Vec<String>,
}

impl LinkedOrchestration {
    pub fn observe(&mut self, run: &str, task: Option<&str>, dispatch: Option<&str>) {
        push_bounded_unique(&mut self.run_ids, run);
        if let Some(task) = task {
            push_bounded_unique(&mut self.task_ids, task);
        }
        if let Some(dispatch) = dispatch {
            push_bounded_unique(&mut self.dispatch_ids, dispatch);
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        for value in self
            .run_ids
            .iter()
            .chain(self.task_ids.iter())
            .chain(self.dispatch_ids.iter())
        {
            if !valid_opaque_word(value, 128) {
                return Err("orchestration link contains an invalid id".to_string());
            }
        }
        if self.run_ids.len() > LINKED_ORCHESTRATION_HISTORY_MAX
            || self.task_ids.len() > LINKED_ORCHESTRATION_HISTORY_MAX
            || self.dispatch_ids.len() > LINKED_ORCHESTRATION_HISTORY_MAX
        {
            return Err("orchestration link history exceeds its bound".to_string());
        }
        Ok(())
    }
}

/// One durable worktree-to-item association.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkedWorkItem {
    pub link_id: String,
    pub repository_id: String,
    pub worktree_id: String,
    /// Routing hint for the current machine. Identity comes from the Git
    /// directories above; this path may be refreshed after a worktree move.
    pub worktree_path: String,
    pub source: LinkedWorkItemSource,
    #[serde(default)]
    pub sync: JiraSyncPolicy,
    #[serde(default)]
    pub orchestration: LinkedOrchestration,
    pub linked_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Why a Jira synchronization event exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JiraSyncTrigger {
    WorkStarted,
    WorkerDone,
}

/// What is known about one external Jira write.
///
/// `Unknown` is intentionally terminal until a person retries: a disconnected
/// HTTP client cannot prove whether Jira applied the request before the answer
/// was lost, and an automatic retry could duplicate a comment or transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "message", rename_all = "snake_case")]
pub enum JiraSyncActionState {
    NotRequested,
    Pending,
    Applied,
    Refused(String),
    Unknown(String),
}

impl JiraSyncActionState {
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }

    /// A finished action a person may choose to re-attempt: `Refused` (Jira
    /// declined it) or `Unknown` (the answer was lost). `Applied` and
    /// `NotRequested` are never retried — one is done, the other was never
    /// asked for. Retrying is always an explicit human act; see the type note.
    #[must_use]
    pub const fn is_retryable(&self) -> bool {
        matches!(self, Self::Refused(_) | Self::Unknown(_))
    }
}

/// Durable outbox row for one lifecycle-to-Jira synchronization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JiraSyncEvent {
    pub event_id: String,
    pub link_id: String,
    pub trigger: JiraSyncTrigger,
    /// The orchestration message id for worker completion, or the deterministic
    /// link-start id for a worktree launch.
    pub source_id: String,
    pub succeeded: Option<bool>,
    pub summary: String,
    pub transition_id: Option<String>,
    pub transition: JiraSyncActionState,
    pub comment: JiraSyncActionState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl JiraSyncEvent {
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.transition.is_pending() || self.comment.is_pending()
    }

    /// Whether any requested action finished in a state a person may retry.
    #[must_use]
    pub fn has_retryable(&self) -> bool {
        self.transition.is_retryable() || self.comment.is_retryable()
    }

    pub fn validate(&self) -> Result<(), String> {
        if !valid_opaque_word(&self.event_id, 160)
            || !valid_opaque_word(&self.link_id, 128)
            || !valid_opaque_word(&self.source_id, 160)
        {
            return Err("Jira sync event identity is invalid".to_string());
        }
        if self.summary.len() > 8192 || self.summary.chars().any(|one| one == '\0') {
            return Err("Jira sync summary is invalid".to_string());
        }
        if self
            .transition_id
            .as_deref()
            .is_some_and(|id| !valid_opaque_word(id, 128))
        {
            return Err("Jira sync transition id is invalid".to_string());
        }
        if self.created_at_ms < 0 || self.updated_at_ms < self.created_at_ms {
            return Err("Jira sync event timestamp is invalid".to_string());
        }
        Ok(())
    }
}

impl LinkedWorkItem {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_opaque_word(&self.link_id, 128)
            || !valid_opaque_word(&self.repository_id, 128)
            || !valid_opaque_word(&self.worktree_id, 128)
        {
            return Err("linked work item identity is invalid".to_string());
        }
        if !Path::new(&self.worktree_path).is_absolute()
            || self.worktree_path.len() > 4096
            || self.worktree_path.chars().any(char::is_control)
        {
            return Err("linked work item path is invalid".to_string());
        }
        if self.linked_at_ms < 0 || self.updated_at_ms < self.linked_at_ms {
            return Err("linked work item timestamp is invalid".to_string());
        }
        match &self.source {
            LinkedWorkItemSource::Jira {
                site_id,
                key,
                url,
                title,
                status,
                updated,
            } => {
                if !valid_opaque_word(site_id, 128) || !crate::jira::valid_issue_key(key) {
                    return Err("linked Jira identity is invalid".to_string());
                }
                let parsed =
                    url::Url::parse(url).map_err(|_| "linked Jira URL is invalid".to_string())?;
                if parsed.scheme() != "https" || parsed.host_str().is_none() {
                    return Err("linked Jira URL must be HTTPS".to_string());
                }
                for value in [title.as_str(), status.as_str()] {
                    if value.len() > 1024 || value.chars().any(char::is_control) {
                        return Err("linked Jira snapshot is invalid".to_string());
                    }
                }
                if updated
                    .as_deref()
                    .is_some_and(|value| value.len() > 128 || value.chars().any(char::is_control))
                {
                    return Err("linked Jira update stamp is invalid".to_string());
                }
            }
        }
        self.sync.validate()?;
        self.orchestration.validate()
    }

    #[must_use]
    pub fn jira_site_and_key(&self) -> (&str, &str) {
        match &self.source {
            LinkedWorkItemSource::Jira { site_id, key, .. } => (site_id, key),
        }
    }

    #[must_use]
    pub fn jira_url(&self) -> &str {
        match &self.source {
            LinkedWorkItemSource::Jira { url, .. } => url,
        }
    }
}

/// Deterministic identity for a retry of the same link request.
#[must_use]
pub fn jira_link_id(worktree_id: &str, site_id: &str, key: &str) -> String {
    let mut digest = Sha256::new();
    for (label, value) in [
        (b"worktree".as_slice(), worktree_id.as_bytes()),
        (b"site".as_slice(), site_id.as_bytes()),
        (b"issue".as_slice(), key.as_bytes()),
    ] {
        digest.update((label.len() as u64).to_le_bytes());
        digest.update(label);
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    }
    format!("jira-link-{:x}", digest.finalize())
}

/// Stable outbox identity for the same lifecycle signal and linked item.
#[must_use]
pub fn jira_sync_event_id(link_id: &str, source_id: &str) -> String {
    let mut digest = Sha256::new();
    for (label, value) in [
        (b"link".as_slice(), link_id.as_bytes()),
        (b"source".as_slice(), source_id.as_bytes()),
    ] {
        digest.update((label.len() as u64).to_le_bytes());
        digest.update(label);
        digest.update((value.len() as u64).to_le_bytes());
        digest.update(value);
    }
    format!("jira-sync-{:x}", digest.finalize())
}

fn valid_opaque_word(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .chars()
            .all(|glyph| !glyph.is_control() && !glyph.is_whitespace())
}

fn push_bounded_unique(values: &mut Vec<String>, value: &str) {
    if values.iter().any(|held| held == value) {
        return;
    }
    if values.len() == LINKED_ORCHESTRATION_HISTORY_MAX {
        values.remove(0);
    }
    values.push(value.to_string());
}

impl WorkItem {
    /// 사람이 읽는 이름 — `PR 123`, `ABC-9`.
    #[must_use]
    pub fn label(&self) -> String {
        match (&self.key, self.number) {
            (Some(key), _) => key.clone(),
            (None, Some(number)) => format!("{} {number}", short_kind(self.kind)),
            (None, None) => short_kind(self.kind).to_string(),
        }
    }
}

const fn short_kind(kind: WorkItemKind) -> &'static str {
    match kind {
        WorkItemKind::Issue => "Issue",
        WorkItemKind::Pr => "PR",
        WorkItemKind::MergeRequest => "MR",
        WorkItemKind::Jira => "Jira",
        WorkItemKind::Linear => "Linear",
    }
}

/// 스마트 칸 한 줄에 대한 답 전체. 창이 배지와 이름에 쓰는 것이 이것 하나다.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameSeed {
    /// 알아본 항목, 또는 자유 텍스트라 아무것도 아님.
    pub item: Option<WorkItem>,
    /// 배지에 적히는 짧은 이름(`PR 123`). 자유 텍스트면 빈 문자열.
    pub label: String,
    /// 사람이 읽는 워크스페이스 이름(`Review PR 123`).
    pub display_name: String,
    /// 경로와 브랜치의 재료가 되는 슬러그(`review-pr-123`).
    pub seed_name: String,
}

/// 텍스트 하나를 씨앗으로. 이 모듈의 유일한 입구다.
#[must_use]
pub fn seed_from_text(text: &str) -> NameSeed {
    let trimmed = text.trim();
    let item = read_work_item(trimmed);
    let display_name = match &item {
        Some(found) => intent_name_for(trimmed, found),
        None => title_from_text(trimmed),
    };
    NameSeed {
        label: item.as_ref().map(WorkItem::label).unwrap_or_default(),
        seed_name: slugify_for_workspace_name(&display_name),
        display_name,
        item,
    }
}

/// 이 텍스트는 조회 대상인가 — Orca `isWorkItemLookupText`의 우리 쪽 이름.
#[must_use]
pub fn is_lookup_text(text: &str) -> bool {
    read_work_item(text.trim()).is_some()
}

/// 자동으로 지은 이름을 덮어써도 되는가 — Orca
/// `shouldApplyWorkspaceSourceAutoName`.
///
/// 셋 중 하나면 참이다: 이름이 비었거나, 직전에 **우리가** 지어 넣은 그대로거나,
/// 이름 칸 자체가 조회 텍스트일 때. 사람이 손으로 고친 이름은 어떤 링크를
/// 붙여넣어도 살아남는다 — 이 함수가 지키는 것이 그 한 가지다.
///
/// 붙여넣은 **텍스트는 보지 않는다.** 셋 다 이름 칸에 대한 물음이고, 답을
/// 바꾸는 것은 거기 적혀 있는 값뿐이다 — 받아만 두고 안 읽는 인자는 "링크에
/// 따라 답이 달라진다"는 거짓말이 된다.
#[must_use]
pub fn should_apply_auto_name(current: &str, last_auto: Option<&str>) -> bool {
    let current = current.trim();
    current.is_empty()
        || last_auto.is_some_and(|last| last.trim() == current)
        || is_lookup_text(current)
}

/// 이름이 하나도 없을 때 쓰는 명사들.
///
/// Orca는 해양생물을 쓴다. 우리는 광물이다 — 같은 자리에 같은 성질(짧고,
/// ASCII이고, 서로 안 헷갈리고, 브랜치 이름으로 읽어도 부끄럽지 않은)을 가진
/// **우리 목록**을 두는 것이 요점이고, 남의 목록을 베끼는 것은 그 요점이
/// 아니다.
pub const FALLBACK_NAMES: [&str; 24] = [
    "amber",
    "basalt",
    "cobalt",
    "dolomite",
    "ember",
    "flint",
    "granite",
    "halite",
    "indigo",
    "jasper",
    "kaolin",
    "lapis",
    "malachite",
    "nickel",
    "obsidian",
    "pumice",
    "quartz",
    "rutile",
    "slate",
    "topaz",
    "umber",
    "verdite",
    "wolfram",
    "zircon",
];

/// `pick`번째 대체 이름. 나머지 연산이라 어떤 수가 와도 목록 안에 떨어진다 —
/// 이름을 고르는 자리에서 `unwrap`이 나오는 것이 이 함수가 없을 때의 모습이다.
#[must_use]
pub fn fallback_name(pick: u64) -> &'static str {
    FALLBACK_NAMES[(pick % FALLBACK_NAMES.len() as u64) as usize]
}

/* ---- 판정기 ------------------------------------------------------------- */

/// `#1234`·번호·링크 넷 중 무엇인가. 아무것도 아니면 [`None`] — 그것이
/// "이건 이름이다"라는 답이다.
#[must_use]
pub fn read_work_item(text: &str) -> Option<WorkItem> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // 한 줄짜리 입력만 항목일 수 있다. 문단을 붙여넣은 사람은 작업을 붙여넣은
    // 것이고, 그 안에 링크가 섞여 있어도 워크스페이스의 이름은 문단이 정한다.
    if text.lines().count() > 1 {
        return None;
    }
    if let Some(number) = issue_or_pr_number(text) {
        return Some(WorkItem {
            kind: WorkItemKind::Issue,
            number: Some(number),
            key: None,
            repo: None,
        });
    }
    read_link(text)
}

/// `#1234` 또는 `1234`. Orca `parseGitHubIssueOrPRNumber`.
fn issue_or_pr_number(text: &str) -> Option<u64> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// URL 하나를 호스트와 경로 마디로 가른다. `url` 크레이트를 들이지 않는 이유는
/// 여기서 필요한 것이 **모양 판별**뿐이기 때문이다 — 질의·조각·인코딩은
/// 아무것도 안 정한다.
fn split_url(text: &str) -> Option<(String, Vec<String>)> {
    let rest = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    // 질의와 조각은 경로가 아니다. 붙어 있으면 마지막 마디를 오염시킨다.
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut parts = rest.splitn(2, '/');
    let host = parts.next().unwrap_or_default().to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let path = parts
        .next()
        .unwrap_or_default()
        .split('/')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    Some((host, path))
}

fn read_link(text: &str) -> Option<WorkItem> {
    let (host, path) = split_url(text)?;
    // Jira는 호스트가 사이트마다 다르므로 **경로 모양**으로 알아본다:
    // `…/browse/ABC-9`. Orca `parseJiraIssueUrl`도 같은 자리를 본다.
    if let Some(at) = path.iter().position(|part| part == "browse")
        && let Some(key) = path.get(at + 1).and_then(|raw| jira_key(raw))
    {
        return Some(WorkItem {
            kind: WorkItemKind::Jira,
            number: None,
            key: Some(key),
            repo: None,
        });
    }
    if host.ends_with("linear.app") {
        // `/<팀>/issue/<KEY>/…`
        if let Some(at) = path.iter().position(|part| part == "issue")
            && let Some(key) = path.get(at + 1)
        {
            return Some(WorkItem {
                kind: WorkItemKind::Linear,
                number: None,
                key: Some(key.to_ascii_uppercase()),
                repo: None,
            });
        }
        return None;
    }
    // GitLab은 `-` 마디가 프로젝트 경로와 항목을 가른다(`/g/s/p/-/issues/5`),
    // 그래서 중첩 그룹이 몇 겹이든 같은 규칙으로 읽힌다.
    if let Some(at) = path.iter().position(|part| part == "-") {
        let kind = match path.get(at + 1).map(String::as_str) {
            Some("issues" | "work_items") => WorkItemKind::Issue,
            Some("merge_requests") => WorkItemKind::MergeRequest,
            _ => return None,
        };
        let number = path.get(at + 2)?.parse().ok()?;
        return Some(WorkItem {
            kind,
            number: Some(number),
            key: None,
            repo: (at >= 2).then(|| path[..at].join("/")),
        });
    }
    // 남은 것은 GitHub 모양 — `/<owner>/<repo>/(issues|pull)/<n>`. 호스트를
    // github.com으로 못 박지 않는 이유는 GitHub Enterprise가 같은 경로 모양을
    // 쓰기 때문이고, 그것이 Orca의 `GH_ITEM_PATH_RE`도 경로만 보는 이유다.
    if path.len() >= 4 {
        let kind = match path[2].as_str() {
            "issues" => WorkItemKind::Issue,
            "pull" => WorkItemKind::Pr,
            _ => return None,
        };
        let number = path[3].parse().ok()?;
        return Some(WorkItem {
            kind,
            number: Some(number),
            key: None,
            repo: Some(format!("{}/{}", path[0], path[1])),
        });
    }
    None
}

/// `ABC-9` 모양인가 — Orca `JIRA_ISSUE_KEY_PATTERN`. 키는 대문자로 정규화된다.
fn jira_key(raw: &str) -> Option<String> {
    let (project, number) = raw.split_once('-')?;
    let mut chars = project.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
        return None;
    }
    if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(raw.to_ascii_uppercase())
}

/* ---- 이름 짓기 ---------------------------------------------------------- */

/// 알아본 항목 하나의 이름 — `Review PR 123`, `Fix ABC-9`.
///
/// 제목은 아직 없다(조회가 다음 슬라이스다). 그래서 이름은 **동사 + 식별자**로
/// 짓는다: 사람이 사이드바에서 그 줄을 알아보는 데 필요한 최소한이고, 제목이
/// 붙는 날 같은 자리에 제목이 들어온다.
fn intent_name_for(text: &str, item: &WorkItem) -> String {
    let verb = action_verb(text).unwrap_or_else(|| item.kind.verb());
    format!("{verb} {}", item.label())
}

/// 자유 텍스트의 이름 — 불용어를 걷어 내고 최대 다섯 낱말, 타이틀 케이스.
/// Orca `getWorkspaceIntentName`의 자유 텍스트 가지.
fn title_from_text(text: &str) -> String {
    let words: Vec<String> = text
        .split_whitespace()
        .filter(|word| !is_stop_word(word))
        .take(MAX_NAME_WORDS)
        .map(title_case)
        .collect();
    if words.is_empty() {
        return text.trim().to_string();
    }
    words.join(" ")
}

/// 이름에 남는 낱말 수. 다섯은 Orca의 `compactWords`(4~5)와 같은 자리이고,
/// 48자 컷이 그 뒤를 받는다.
const MAX_NAME_WORDS: usize = 5;

/// 이름에서 걷어 내는 낱말. 영어의 관사·전치사만 — 한국어는 조사가 붙어
/// 낱말에 녹아 있으므로 목록으로 걷어 낼 수 있는 것이 없고, 억지로 자르면
/// 이름이 아니라 조각이 된다.
const STOP_WORDS: [&str; 12] = [
    "a", "an", "the", "to", "for", "of", "in", "on", "at", "and", "or", "please",
];

fn is_stop_word(word: &str) -> bool {
    let bare = word.trim_matches(|ch: char| !ch.is_alphanumeric());
    STOP_WORDS
        .iter()
        .any(|stop| bare.eq_ignore_ascii_case(stop))
}

/// 동사 하나를 알아본다 — Orca `ACTION_LABELS`. 첫 낱말만 본다: 문장 가운데의
/// "fix"는 이 작업이 무엇인지가 아니라 무엇에 대한 것인지를 말한다.
fn action_verb(text: &str) -> Option<&'static str> {
    let first = text.split_whitespace().next()?.to_ascii_lowercase();
    let first = first.trim_matches(|ch: char| !ch.is_ascii_alphabetic());
    match first {
        "fix" | "resolve" | "repair" => Some("Fix"),
        "debug" | "diagnose" => Some("Debug"),
        "review" | "inspect" => Some("Review"),
        "implement" | "build" => Some("Implement"),
        "investigate" | "explore" => Some("Investigate"),
        "add" | "create" => Some("Add"),
        "update" | "change" => Some("Update"),
        "refactor" | "clean" => Some("Refactor"),
        "test" => Some("Test"),
        _ => None,
    }
}

/// 첫 글자만 올린다. 이미 대문자로 쓰인 낱말(`API`, `PR`)은 건드리지 않는다 —
/// 사람이 대문자로 적은 것은 이름이지 문장이 아니다.
fn title_case(word: &str) -> String {
    if word.chars().any(char::is_uppercase) {
        return word.to_string();
    }
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// 워크스페이스 이름용 슬러그 — Orca `slugifyForWorkspaceName`.
///
/// 브랜치 슬러그와 **다른 함수다**: `.`과 `_`가 살아남고, `..`은 `.` 하나로
/// 접히고, 48자에서 잘린 뒤 꼬리의 구두점이 다시 정리된다.
#[must_use]
pub fn slugify_for_workspace_name(text: &str) -> String {
    let mut out = String::new();
    let mut separator_pending = false;
    for ch in text.chars() {
        // 알파뉴머릭·`.`·`_`·`-`만 남는다. 워커 브랜치·체크아웃
        // 슬러그와 달리 유니코드 낱말을 살린다 — 사람이 보는 워크스페이스
        // 이름과 git·파일시스템이 보는 ASCII 이름은 서로 다른 계약이다.
        if ch.is_alphanumeric() || ch == '.' || ch == '_' {
            if separator_pending && !out.is_empty() {
                out.push('-');
            }
            separator_pending = false;
            out.extend(ch.to_lowercase());
        } else {
            separator_pending = true;
        }
    }
    let capped: String = out.chars().take(MAX_SEED_CHARS).collect();
    let collapsed = capped.replace("..", ".");
    collapsed
        .trim_matches(|ch| ch == '-' || ch == '.' || ch == '_')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 표 하나가 이 모듈의 계약 전부다.
    #[test]
    fn the_five_shapes_are_read_without_a_network() {
        let cases: [(&str, WorkItemKind, Option<u64>, Option<&str>); 7] = [
            ("#123", WorkItemKind::Issue, Some(123), None),
            ("4567", WorkItemKind::Issue, Some(4567), None),
            (
                "https://github.com/o/r/pull/7",
                WorkItemKind::Pr,
                Some(7),
                None,
            ),
            (
                "https://github.com/o/r/issues/9",
                WorkItemKind::Issue,
                Some(9),
                None,
            ),
            (
                "https://gitlab.com/g/s/p/-/merge_requests/5",
                WorkItemKind::MergeRequest,
                Some(5),
                None,
            ),
            (
                "https://acme.atlassian.net/browse/abc-9",
                WorkItemKind::Jira,
                None,
                Some("ABC-9"),
            ),
            (
                "https://linear.app/team/issue/k-1/something",
                WorkItemKind::Linear,
                None,
                Some("K-1"),
            ),
        ];
        for (text, kind, number, key) in cases {
            let found = read_work_item(text).unwrap_or_else(|| panic!("{text} was not read"));
            assert_eq!(found.kind, kind, "{text}");
            assert_eq!(found.number, number, "{text}");
            assert_eq!(found.key.as_deref(), key, "{text}");
        }
    }

    /// 알아보지 못한 것은 오류가 아니라 이름이다.
    #[test]
    fn anything_else_is_a_name_rather_than_a_refusal() {
        for text in [
            "Fix the flaky test",
            "https://github.com/o/r",
            "https://example.com/browse/notakey",
            "https://github.com/o/r/pull/notanumber",
            "#12a",
            "",
            "  ",
            // 문단은 작업이지 항목이 아니다 — 안에 링크가 있어도.
            "Fix this\nhttps://github.com/o/r/pull/7",
        ] {
            assert!(
                read_work_item(text).is_none(),
                "{text:?} was read as an item"
            );
        }
    }

    /// 링크에서 저장소를 읽어 낸다 — 조회가 붙는 날 그대로 쓰인다.
    #[test]
    fn a_link_carries_the_repository_it_names() {
        let github = read_work_item("https://github.com/octo/repo/issues/3").expect("github");
        assert_eq!(github.repo.as_deref(), Some("octo/repo"));
        let gitlab =
            read_work_item("https://gitlab.com/group/sub/proj/-/issues/3").expect("gitlab");
        assert_eq!(gitlab.repo.as_deref(), Some("group/sub/proj"));
    }

    #[test]
    fn a_pr_is_reviewed_and_an_issue_is_fixed_unless_the_text_says_otherwise() {
        let pr = seed_from_text("https://github.com/o/r/pull/123");
        assert_eq!(pr.display_name, "Review PR 123");
        assert_eq!(pr.seed_name, "review-pr-123");
        assert_eq!(pr.label, "PR 123");

        let issue = seed_from_text("#42");
        assert_eq!(issue.display_name, "Fix Issue 42");

        // 사람이 동사를 적었으면 그 동사가 이긴다.
        let jira = seed_from_text("https://acme.atlassian.net/browse/ABC-9");
        assert_eq!(jira.display_name, "Fix ABC-9");
    }

    #[test]
    fn free_text_becomes_a_short_title_and_a_slug() {
        let seed = seed_from_text("fix the flaky drain gate test please");
        assert_eq!(seed.display_name, "Fix Flaky Drain Gate Test");
        assert_eq!(seed.seed_name, "fix-flaky-drain-gate-test");
        assert_eq!(seed.label, "");
        assert!(seed.item.is_none());
    }

    /// 이미 대문자로 적힌 낱말은 그대로 둔다.
    #[test]
    fn words_someone_capitalised_stay_capitalised() {
        assert_eq!(
            seed_from_text("update the API docs").display_name,
            "Update API Docs"
        );
    }

    #[test]
    fn the_workspace_slug_keeps_dots_and_underscores_and_stops_at_the_cap() {
        assert_eq!(
            slugify_for_workspace_name("v1.2.3 release_notes"),
            "v1.2.3-release_notes"
        );
        assert_eq!(slugify_for_workspace_name("a..b"), "a.b");
        assert_eq!(slugify_for_workspace_name("--edges--"), "edges");
        let long = slugify_for_workspace_name(&"word ".repeat(40));
        assert_eq!(long.chars().count(), MAX_SEED_CHARS);
        assert!(!long.ends_with('-'), "{long:?}");
        // 자른 자리가 구분자였을 때가 회귀하는 자리다.
        let edge = slugify_for_workspace_name(&format!("{} b", "a".repeat(MAX_SEED_CHARS - 1)));
        assert_eq!(edge, "a".repeat(MAX_SEED_CHARS - 1));
    }

    /// 한국어 이름은 그대로 살아남는다 — 브랜치 슬러그와 같은 성질.
    #[test]
    fn a_korean_name_survives_as_itself() {
        assert_eq!(
            slugify_for_workspace_name("드레인 게이트 봉인"),
            "드레인-게이트-봉인"
        );
    }

    /// 손으로 고친 이름은 어떤 링크가 와도 살아남는다.
    #[test]
    fn a_hand_written_name_is_never_overwritten() {
        assert!(should_apply_auto_name("", None));
        assert!(should_apply_auto_name("Review PR 7", Some("Review PR 7")));
        // 이름 칸 자체가 조회 텍스트일 때도 덮어쓴다 — 붙여넣기의 첫 프레임이다.
        assert!(should_apply_auto_name("#7", Some("Review PR 3")));
        assert!(!should_apply_auto_name("my own name", Some("Review PR 7")));
        assert!(!should_apply_auto_name("my own name", None));
    }

    /// 어떤 수가 와도 목록 안에 떨어진다.
    #[test]
    fn the_fallback_name_is_always_one_of_ours() {
        for pick in [0_u64, 1, 23, 24, u64::MAX] {
            assert!(FALLBACK_NAMES.contains(&fallback_name(pick)), "{pick}");
        }
        // 그리고 그 이름들은 전부 슬러그로 살아남는다 — 이름 하나가 브랜치
        // 하나이므로, 목록에 슬러그가 못 되는 낱말이 있으면 그 이름은 만들
        // 수 없는 워크스페이스다.
        for name in FALLBACK_NAMES {
            assert_eq!(slugify_for_workspace_name(name), name);
        }
    }
}
