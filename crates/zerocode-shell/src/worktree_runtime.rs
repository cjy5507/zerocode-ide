use super::*;

/// One worktree, as the sidebar lists it.
///
/// A projection rather than [`Worktree`] itself: the panel needs a string it
/// can put in the DOM and the one distinction it draws — the repository's own
/// checkout, which carries the default badge and can never be removed — not
/// git's full bookkeeping.
#[derive(Serialize)]
pub(super) struct SetupTerminal {
    pub(super) term: TermId,
    pub(super) launch_mode: SetupScriptLaunchMode,
}

/// Result of Orca's optional local-base maintenance during workspace create.
/// The exact status words cross the renderer boundary so a skipped safety
/// check can name its reason without exposing raw git output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct LocalBaseRefRefresh {
    pub(super) status: LocalBaseRefRefreshStatus,
    pub(super) base_ref: String,
    pub(super) local_branch: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) owner_worktree_path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum LocalBaseRefRefreshStatus {
    Updated,
    SkippedDirtyWorktree,
    SkippedNotFastForward,
    SkippedError,
}

#[derive(Serialize)]
pub(super) struct WorktreeEntry {
    pub(super) path: String,
    /// Short branch name, absent on a detached checkout. The panel shows the
    /// directory name then, rather than inventing one.
    pub(super) branch: Option<String>,
    /// What the branch was cut from, when the cut wrote it down
    /// (`branch.<name>.base`, [`record_creation_base`]). Absent on the clone
    /// itself, on a detached checkout, and on a branch somebody made at a
    /// shell — the card then shows the branch alone, as it always did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) base: Option<String>,
    pub(super) is_main: bool,
    /// A cataloged directory without git metadata. Orca calls this a folder
    /// workspace; it is still selectable and can own terminals and agents.
    pub(super) is_folder: bool,
    /// git believes the directory is gone; `git worktree prune` clears it.
    pub(super) prunable: bool,
    /// The one the file surfaces are showing. Decided here rather than by the
    /// webview comparing strings, because only this side knows what path the
    /// commands actually resolve against.
    pub(super) active: bool,
    /// The terminal already running this workspace's trusted setup command.
    /// Only the create response carries it; ordinary catalogue rows do not
    /// manufacture or replay setup work.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) setup_terminal: Option<SetupTerminal>,
    /// A setup terminal that could not be opened does not turn a successfully
    /// created checkout into a retryable create request. The row still lands
    /// and the renderer reports this one launch failure beside it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) setup_error: Option<String>,
    /// Present only when the optional local-base maintenance had a remote base
    /// to inspect. A skipped result is still a successful workspace create.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) local_base_ref_refresh: Option<LocalBaseRefRefresh>,
    /// Who made this checkout ([`zerocode_core::Ownership::slug`]).
    ///
    /// Decided here rather than by the window comparing paths, because the rule
    /// is the same rule the hide below is decided by and two copies of it would
    /// disagree the first time either changed.
    pub(super) ownership: &'static str,
    /// True when the external-worktree switch is what keeps this row off the
    /// list. Named as a fact about the row so the sidebar's filter is one
    /// expression over the catalog rather than a second copy of the rules.
    pub(super) external_hidden: bool,
    /// 이 체크아웃이 마지막으로 살아 있었던 시각, epoch 밀리초.
    ///
    /// `0`은 "읽을 자국이 하나도 없었다"이지 "1970년"이 아니다 — 정렬은 그것을
    /// 맨 뒤로 보내고, 그것이 이 값의 정직한 뜻이다.
    ///
    /// Orca가 워크스페이스 레코드에 들고 다니는 `lastActivityAt`의 자리다.
    /// 그쪽은 자기 스토어에 적어 두지만 여기에는 그런 스토어가 없고, 디스크가
    /// 이미 답을 들고 있다 — [`worktree_activity_marks`]가 정리 화면을 위해
    /// 벌써 읽는 다섯 개의 mtime이 그것이다. 두 화면이 같은 함수에 묻는다.
    pub(super) last_activity_ms: i64,
    /// A durable external work item attached to this checkout, when any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) linked_item: Option<zerocode_core::LinkedWorkItem>,
}

impl WorktreeEntry {
    pub(super) fn new(worktree: Worktree, active_root: &Path) -> Self {
        Self {
            active: worktree.path == active_root,
            path: worktree.path.to_string_lossy().into_owned(),
            branch: worktree.branch,
            base: None,
            is_main: worktree.is_main,
            is_folder: false,
            prunable: worktree.prunable,
            setup_terminal: None,
            setup_error: None,
            local_base_ref_refresh: None,
            // Answered by `decide_external_visibility` before this entry leaves
            // the catalog. Born as "ours and shown" so a caller that skips that
            // walk — `list_worktrees`, which is about one project's rows and not
            // about the sidebar's filter — cannot hide anything by omission.
            ownership: zerocode_core::Ownership::ZerocodeManaged.slug(),
            external_hidden: false,
            // Stamped by the catalogue walk, which is the only caller that
            // knows the host to ask. Born as "no mark read", never as a date.
            last_activity_ms: 0,
            linked_item: None,
        }
    }

    pub(super) fn folder(path: &Path, active_root: &Path) -> Self {
        Self {
            active: path == active_root,
            path: path.to_string_lossy().into_owned(),
            branch: None,
            base: None,
            is_main: true,
            is_folder: true,
            prunable: false,
            setup_terminal: None,
            setup_error: None,
            local_base_ref_refresh: None,
            ownership: zerocode_core::Ownership::ZerocodeManaged.slug(),
            external_hidden: false,
            last_activity_ms: 0,
            linked_item: None,
        }
    }
}

pub(super) fn decorate_worktree_link(
    entry: &mut WorktreeEntry,
    links: &[zerocode_core::LinkedWorkItem],
) {
    if entry.is_folder {
        return;
    }
    let path = Path::new(&entry.path);
    let identity = zerocode_core::git_dir::identity(path);
    let found = links.iter().find(|link| {
        identity.as_ref().is_some_and(|identity| {
            link.repository_id == identity.repository_id && link.worktree_id == identity.worktree_id
        }) || link.worktree_path == entry.path
    });
    if let Some(found) = found {
        let mut current = found.clone();
        // A routing hint follows an externally moved worktree in the report;
        // its durable identity above is what made the match.
        current.worktree_path.clone_from(&entry.path);
        entry.linked_item = Some(current);
    }
}

/// What one project has to say about worktrees this window did not make.
///
/// Four surfaces read this: the project menu's row (which way its label reads),
/// the dialog (state and count), the card that asks once, and the inbox. All
/// four are drawn from this one answer so they cannot disagree about a repository
/// they are all describing.
#[derive(Serialize)]
pub(super) struct ExternalWorktreeState {
    /// `"hide"` or `"show"` — the switch's effective position, default included.
    pub(super) visibility: &'static str,
    /// Whether this project predates the rollout, which is why its default is
    /// `show`. The window shows it nowhere; it is here because a report that
    /// cannot explain its own default is a report nobody can debug.
    pub(super) legacy: bool,
    /// Whether a real worktree listing stands behind the counts. False means
    /// **every count here is zero on purpose**, not "none found".
    pub(super) authoritative: bool,
    /// How many external worktrees are on the list right now.
    pub(super) shown: usize,
    /// The external worktrees this switch is keeping off the list, by path.
    pub(super) hidden: Vec<String>,
    /// Whether the ask-once card should be drawn for this project.
    pub(super) prompt: bool,
    /// The hidden paths that appeared AFTER the last time somebody chose to
    /// hide — the inbox's whole point. Empty unless the inbox is being offered.
    pub(super) inbox: Vec<String>,
}

/// One stable repository block in the sidebar catalog.
#[derive(Serialize)]
pub(super) struct ProjectEntry {
    pub(super) path: String,
    pub(super) name: String,
    pub(super) worktrees: Vec<WorktreeEntry>,
    /// This project's answer about worktrees made outside this window.
    pub(super) external: ExternalWorktreeState,
    /// The colour of this repository's mark, `None` for one nobody has marked.
    ///
    /// `None` means DRAW NOTHING and not "draw the neutral one" — the sidebar's
    /// own note and [`zerocode_core::repo_mark`] both turn on that, so the
    /// absence has to survive the wire rather than being resolved to a default
    /// on the way out.
    pub(super) mark_color: Option<String>,
    /// Which repository on the internet this is — `owner/repository`.
    ///
    /// The original's project row carries this as its right-hand detail, and
    /// falls back to the literal word `Project` when it does not know
    /// (`getProjectDetail`, `lib/new-workspace-project-options.ts:86-94`).
    /// [`None`] is that fallback: a repository with no remote, or one whose
    /// remote nobody can resolve to a page.
    pub(super) slug: Option<String>,
}

/// `owner/repository` for the repository rooted at `root`, read not asked.
///
/// Two reads of two small files and no process, which is what lets this sit on
/// a road that runs on every workspace click. The original does not pay even
/// that: it stores the identity on the repo record when the repository is
/// added and projects it from there (`repo.upstream` →
/// `getProjectProviderIdentity`). Reading is the cheaper trade HERE because we
/// have no such record to migrate, and because a remote that was renamed since
/// the project was added answers correctly without anybody re-adding it.
pub(super) fn project_slug(root: &Path) -> Option<String> {
    let shared = zerocode_core::git_dir::common_of(root)?;
    let (_, url) = zerocode_core::git_config::primary_remote(&shared.join("config"))?;
    // The parsed path, whatever shape the host gives it. The original only
    // ever shows a GitHub `owner/repo` because its identity is GitHub-only,
    // and the same two segments come back for GitLab, Bitbucket and an
    // Enterprise install — so the restriction would cost those rows their
    // answer and buy nothing.
    Some(remote_repo::parse_remote_repo(&url)?.path)
}

/// What removing a worktree would destroy — what a confirmation has to show
/// before it asks (F7).
#[derive(Serialize)]
pub(super) struct PendingLossReport {
    /// One `git status --porcelain` line each, so the dialog can show exactly
    /// what git would say.
    pub(super) uncommitted: Vec<String>,
    /// Paths git was told to ignore. Separate because `git status` never
    /// mentions them and `git worktree remove` deletes them without a word —
    /// the `.env` nobody can regenerate is in this list, not the one above.
    pub(super) ignored: Vec<String>,
    /// What the dialog compares against before it discards anything.
    ///
    /// The two lists above are for reading, not for deciding. They are escaped
    /// so one path reads as one line, but a person's safety must not rest on
    /// an escaping rule staying perfect: this is built from the raw fields and
    /// no filename can imitate it.
    pub(super) fingerprint: String,
}

/// The worktree `requested` names, or `None` when it names none of them.
///
/// The webview does not get to choose a directory. [`open_lane`] starts a
/// process in one and [`remove_worktree`] deletes one, so a path is accepted
/// only by finding it on git's own list. Compared as paths rather than
/// strings, so a trailing slash or a `.` segment cannot miss a match that is
/// really there — and `..`, which `Path` does not resolve, simply fails to
/// match, which is the safe direction.
pub(super) fn matching_worktree<'a>(
    known: &'a [Worktree],
    requested: &str,
) -> Option<&'a Worktree> {
    let requested = Path::new(requested);
    known
        .iter()
        .find(|worktree| worktree.path.as_path() == requested)
}

/// Resolve a repository only when it already belongs to the sidebar catalog.
pub(super) fn known_project_orchestrator(
    config_root: &Path,
    requested: &str,
) -> Result<Orchestrator, String> {
    let requested = PathBuf::from(requested)
        .canonicalize()
        .map_err(|_| NOT_A_REPOSITORY.to_string())?;
    for stored in stored_projects(config_root) {
        let Ok(orchestrator) = Orchestrator::open(&stored) else {
            continue;
        };
        if orchestrator.repo_root() == requested {
            return Ok(orchestrator);
        }
    }
    Err(NOT_A_REPOSITORY.to_string())
}

/// Aggregate reads skip catalogued non-repository folders without dropping
/// legitimate repositories. Unknown paths still fail the catalog boundary.
pub(super) fn known_project_repositories(
    config_root: &Path,
    requested: &[String],
) -> Result<Vec<(String, PathBuf)>, String> {
    let known: Vec<PathBuf> = stored_projects(config_root)
        .into_iter()
        .filter_map(|path| PathBuf::from(path).canonicalize().ok())
        .collect();
    let mut homes = Vec::new();
    for name in requested {
        let canonical = PathBuf::from(name)
            .canonicalize()
            .map_err(|_| NOT_A_REPOSITORY.to_string())?;
        if !known.contains(&canonical) {
            return Err(NOT_A_REPOSITORY.to_string());
        }
        if let Ok(orchestrator) = Orchestrator::open(&canonical) {
            let root = orchestrator.repo_root().to_path_buf();
            if !homes.iter().any(|(_, held)| held == &root) {
                homes.push((name.clone(), root));
            }
        }
    }
    if homes.is_empty() {
        return Err(NOT_A_REPOSITORY.to_string());
    }
    Ok(homes)
}

/// Resolve a worktree through git's lists for repositories already cataloged.
pub(super) fn known_worktree_context(
    config_root: &Path,
    requested: &str,
) -> Result<(Orchestrator, Worktree), String> {
    for stored in stored_projects(config_root) {
        let Ok(orchestrator) = Orchestrator::open(&stored) else {
            continue;
        };
        let Ok(known) = orchestrator.list() else {
            continue;
        };
        if let Some(worktree) = matching_worktree(&known, requested).cloned() {
            return Ok((orchestrator, worktree));
        }
    }
    Err(NOT_A_WORKTREE.to_string())
}

/// A non-git folder is selectable only when its canonical path is already in
/// the catalog. This is the folder-workspace equivalent of asking git for its
/// worktree list: the webview cannot turn an arbitrary path into a process cwd.
pub(super) fn matching_folder_workspace(known: &[String], requested: &str) -> Option<PathBuf> {
    let requested = PathBuf::from(requested).canonicalize().ok()?;
    if !requested.is_dir() || Orchestrator::open(&requested).is_ok() {
        return None;
    }
    known.iter().find_map(|stored| {
        let stored = PathBuf::from(stored).canonicalize().ok()?;
        (stored == requested).then_some(stored)
    })
}

pub(super) fn known_folder_workspace(
    config_root: &Path,
    requested: &str,
) -> Result<PathBuf, String> {
    matching_folder_workspace(&stored_projects(config_root), requested)
        .ok_or_else(|| NOT_A_WORKTREE.to_string())
}

pub(super) enum KnownWorkspace {
    Git(Orchestrator, Worktree),
    Folder(PathBuf),
}

pub(super) fn known_workspace_context(
    config_root: &Path,
    requested: &str,
) -> Result<KnownWorkspace, String> {
    if let Ok((orchestrator, worktree)) = known_worktree_context(config_root, requested) {
        return Ok(KnownWorkspace::Git(orchestrator, worktree));
    }
    known_folder_workspace(config_root, requested).map(KnownWorkspace::Folder)
}

/// Resolve the stored workspace directory for one repository.
///
/// Absolute paths are shared across projects. Relative paths are deliberately
/// repository-relative, matching Orca's per-project form. The one supported
/// home shorthand is expanded here, at the filesystem boundary, rather than
/// when settings are serialized.
pub(super) fn workspace_directory_for_repo(
    repo_root: &Path,
    prefs: &WorkspaceCreationPrefs,
) -> Result<PathBuf, String> {
    let configured = prefs.directory.trim();
    if configured.is_empty() {
        return Err("워크스페이스 디렉터리를 입력하세요".to_string());
    }
    let path = if configured == "~" {
        workspace_home_directory().ok_or_else(|| "홈 디렉터리를 찾을 수 없습니다".to_string())?
    } else if let Some(relative) = configured
        .strip_prefix("~/")
        .or_else(|| configured.strip_prefix("~\\"))
    {
        workspace_home_directory()
            .ok_or_else(|| "홈 디렉터리를 찾을 수 없습니다".to_string())?
            .join(relative)
    } else {
        PathBuf::from(configured)
    };
    Ok(if path.is_absolute() {
        path
    } else {
        repo_root.join(path)
    })
}

pub(super) fn workspace_home_directory() -> Option<PathBuf> {
    let key = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The exact root handed to `git worktree add`.
pub(super) fn configured_worktree_root(
    repo_root: &Path,
    prefs: &WorkspaceCreationPrefs,
) -> Result<PathBuf, String> {
    let root = workspace_directory_for_repo(repo_root, prefs)?;
    if !prefs.nest_workspaces {
        return Ok(root);
    }
    let repo_name = repo_root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let repo_name = repo_name.strip_suffix(".git").unwrap_or(&repo_name);
    Ok(root.join(if repo_name.is_empty() {
        "repo"
    } else {
        repo_name
    }))
}

pub(super) fn apply_workspace_creation_prefs(
    orchestrator: Orchestrator,
    prefs: &WorkspaceCreationPrefs,
) -> Result<Orchestrator, String> {
    let root = configured_worktree_root(orchestrator.repo_root(), prefs)?;
    Ok(orchestrator.with_worktree_root(root))
}

pub(super) fn catalog_work_item_links(
    settings: &settings::SettingsRepository,
) -> Vec<zerocode_core::LinkedWorkItem> {
    match work_item_store::report(settings) {
        Ok(report) => report.links,
        Err(error) => {
            eprintln!("zerocode-shell: linked work items are unavailable: {error}");
            Vec::new()
        }
    }
}

/// The directory git keeps one entry per linked worktree in.
///
/// `.git` is a directory in a main checkout and a file naming
/// `<common>/worktrees/<name>` in a linked one. Both lead to the same shared
/// `worktrees` directory, which is why a project opened at either end of a
/// repository answers the same.
pub(super) fn linked_worktrees_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    let held = std::fs::metadata(&dot_git).ok()?;
    if held.is_dir() {
        return Some(dot_git.join("worktrees"));
    }
    // `gitdir: /repo/.git/worktrees/<name>` — the shared directory is its
    // parent, and the file is the only thing a linked checkout has to say.
    let said = std::fs::read_to_string(&dot_git).ok()?;
    let named = said.strip_prefix("gitdir:")?.trim();
    Path::new(named).parent().map(std::path::Path::to_path_buf)
}

/// Which phase a creation somebody is watching has reached.
///
/// Three words, and they are the ones a person can act on: `fetching` is the
/// one that can take thirty seconds on a cold repository, and a card that
/// sat on "creating" for all of it would be a card that lies about what is
/// slow.
#[derive(Serialize, Clone)]
pub(super) struct CreationPhase {
    pub(super) id: String,
    pub(super) phase: &'static str,
}

/* ---- 새 워크트리의 이름과 기준 ----------------------------------------------
 *
 * 만들기 다이얼로그가 묻는 것들의 백엔드. 규칙은 전부 여기 있고, 창은 답을
 * 그린다 — 브랜치 접두사 세 모드, 기준 브랜치의 사다리, 그리고 git이 판정하는
 * 것은 git에게 묻는다는 원칙(`check-ref-format`)이 그것이다.
 *
 * git 호출은 하나도 빠짐없이 [`Host::vcs`]를 지난다. `git worktree`만 예외이고
 * 그것은 orchestrator의 것이다 — 이 파일이 git 프로세스를 직접 띄우기 시작하는
 * 순간 호스트 경계는 이미 없는 것이다. */

/// 브랜치 접두사를 무엇으로 정하는가 — Orca `settings.branchPrefix`의 세 값.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum BranchPrefixMode {
    /// 이 저장소의 `user.name`을 슬러그로. Orca의 기본값이고 우리 것이기도
    /// 하다 — 남의 브랜치와 내 브랜치가 한 목록에 섞이는 것을 가르는 유일한
    /// 표시가 이것이다.
    #[default]
    GitUsername,
    /// 사람이 적은 낱말.
    Custom,
    /// 접두사 없음. 브랜치는 이름 그대로다.
    None,
}

/// 창이 읽고 쓰는 만들기 설정. 전역이다 — Orca도 `settings.branchPrefix`를
/// 저장소별이 아니라 앱 단위로 둔다.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorktreePrefs {
    #[serde(default)]
    pub(super) branch_prefix: BranchPrefixMode,
    /// `custom` 모드일 때 쓰이는 낱말. 다른 모드에서도 지워지지 않는다 —
    /// 모드를 잠깐 바꿔 본 사람이 자기가 적은 낱말을 잃으면 안 된다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) custom_prefix: Option<String>,
}

pub(super) fn stored_worktree_prefs(legacy: &LegacySettings<'_>) -> WorktreePrefs {
    legacy
        .read(legacy_settings_file::WORKTREE_PREFS)
        .unwrap_or_default()
}

/// 설정과 이 저장소의 git 사용자 이름에서 접두사 하나.
///
/// 순수 함수인 것이 요점이다 — git을 부르는 자리는 [`git_username`] 하나이고,
/// 세 모드의 규칙은 여기서 표로 시험된다.
///
/// 두 실패가 서로 다르다: **git-username은 강등한다**(이름을 못 읽거나 그
/// 이름이 ref로 못 쓰이면 접두사 없이 만든다 — 이름을 못 읽었다고 워크스페이스
/// 만들기를 거절하는 것은 사람이 고칠 수 없는 거절이다). **custom은
/// 거절한다** — 그 낱말은 사람이 방금 적은 것이고, 조용히 무시하면 자기가 적은
/// 접두사가 안 붙은 브랜치를 보게 된다. Orca도 같은 자리를 같은 두 방향으로
/// 가른다(`computeValidatedBranchName`).
pub(super) fn resolve_branch_prefix(
    prefs: &WorktreePrefs,
    git_username: Option<&str>,
) -> Result<Option<String>, String> {
    use zerocode_orchestrator::naming::{clean_prefix, prefix_is_usable};
    match prefs.branch_prefix {
        BranchPrefixMode::None => Ok(None),
        BranchPrefixMode::Custom => {
            let typed = prefs.custom_prefix.as_deref().unwrap_or_default();
            if !prefix_is_usable(typed) {
                return Err(format!(
                    "브랜치 접두사 `{typed}`은(는) git이 받지 않는 이름입니다"
                ));
            }
            Ok(Some(clean_prefix(typed)))
        }
        BranchPrefixMode::GitUsername => Ok(git_username
            .filter(|name| prefix_is_usable(name))
            .map(clean_prefix)),
    }
}

/// 이 저장소가 아는 커밋 작성자 이름, 브랜치 마디로 쓸 수 있게 슬러그로.
///
/// 저장소에서 묻는다(`-C repo`), 전역에서가 아니라 — 회사 저장소와 개인
/// 저장소에 서로 다른 이름을 둔 사람의 브랜치는 그 저장소의 이름 아래 나야
/// 한다.
pub(super) fn git_username(host: &Host, repo_root: &Path) -> Option<String> {
    let said = host.vcs().text(repo_root, &["config", "user.name"]).ok()?;
    let slug = zerocode_orchestrator::naming::slugify(said.trim());
    (slug != zerocode_orchestrator::naming::FALLBACK_SLUG).then_some(slug)
}

/// 이 이름을 브랜치로 쓸 수 있는가 — **git에게 묻는다**.
///
/// 우리가 판정하지 않는 유일한 이유가 전부다: ref 이름의 규칙은 git의 것이고,
/// 여기에 두 번째 의견을 두면 그 둘은 언젠가 어긋난다. 선두 `-`만 먼저 막는데,
/// 그것은 판정이 아니라 **인자로 오해되는 것**을 막는 것이다.
pub(super) fn check_branch_name(host: &Host, repo_root: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("브랜치 이름이 비어 있습니다".to_string());
    }
    if name.starts_with('-') {
        return Err(format!("`{name}`은(는) 브랜치 이름으로 쓸 수 없습니다"));
    }
    host.vcs()
        .text(repo_root, &["check-ref-format", "--branch", name])
        .map(|_| ())
}

/// 스마트 칸 한 줄이 무엇인가. 판정은 [`zerocode_core::workitem`]의 것이고,
/// 이 문은 그것과 두 가지 답을 함께 돌려준다.
#[derive(Serialize)]
pub(super) struct WorkItemReport {
    #[serde(flatten)]
    pub(super) seed: zerocode_core::NameSeed,
    /// 이름이 하나도 없을 때 쓸 낱말. 다이얼로그가 열릴 때 미리 받아 두었다가
    /// 자리 표시자로 보여 주고, 빈 채로 제출되면 그대로 이름이 된다.
    pub(super) fallback: &'static str,
    /// 파생한 이름을 이름 칸에 **써도 되는가** — Orca
    /// `shouldApplyWorkspaceSourceAutoName`.
    ///
    /// 창이 스스로 판정하지 않는 이유가 이 필드의 존재 이유다: 그 규칙이
    /// 웹뷰에도 한 벌 생기면 둘은 언젠가 갈라지고, 갈라진 날의 증상은 **사람이
    /// 손으로 고친 이름이 링크 하나에 지워지는 것**이다. 창은 지금 칸에 있는
    /// 값과 자기가 마지막으로 써 넣은 값을 보내고, 답은 여기서 온다.
    pub(super) apply_auto_name: bool,
}

/* ---- 만들기 다이얼로그의 GitHub 탭 -------------------------------------------
 *
 * 목록은 [`gh`] 모듈 하나를 지난다 — 우리 토큰은 없고, 사람이 이미 로그인해 둔
 * `gh`가 곧 자격 증명이다(Orca도 같은 모양이다: `ghExecFileAsync`).
 *
 * PR을 고르면 두 걸음이다. 먼저 [`resolve_pr_base`]가 PR head를 가져와 커밋
 * 하나로 만들고, 그 다음 [`create_worktree`]가 그 커밋에서 **PR의 head 이름
 * 그대로** 브랜치를 판다. 두 걸음인 것이 Orca와 같은 이유로 옳다: fetch는
 * 네트워크이고 만들기는 디스크이며, 실패했을 때 사람이 다시 눌러야 하는 것은
 * 둘 중 하나뿐이다. */

/// GitHub 읽기가 왜 실패했나. `jira::Failure`와 **같은 모양**인 것이 요점이다 —
/// 창은 갈래(`kind`)를 보고 조용한 줄과 시끄러운 줄을 가르지, 문장을 뒤지지
/// 않는다.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GhFailure {
    pub(super) kind: &'static str,
    pub(super) message: String,
}

impl From<gh::GhError> for GhFailure {
    fn from(error: gh::GhError) -> Self {
        Self {
            kind: error.reason(),
            message: error.detail().unwrap_or_default().to_string(),
        }
    }
}

impl GhFailure {
    /// 이쪽 실수. `gh`가 한 말이 아니므로 갈래도 `gh`의 것이 아니다.
    pub(super) fn ours(message: String) -> Self {
        Self {
            kind: "unreadable",
            message,
        }
    }
}

/// 렌더러가 말한 종류를 백엔드의 열거로 — 모르는 낱말은 추측 없이 거절.
/// 낱말의 표는 `gh.rs`에 하나뿐이다(`ItemKind::from_word`).
pub(super) fn work_item_kind(said: Option<&str>) -> Result<gh::ItemKind, GhFailure> {
    said.and_then(gh::ItemKind::from_word)
        .ok_or_else(|| GhFailure::ours("작업 항목 종류는 issue 또는 pr입니다".to_string()))
}

/* ---- 작업 페이지의 GitLab 목록(1-g56b) --------------------------------------
 *
 * GitHub 쪽 바로 위와 같은 모양이고, 다른 것은 하나다: `glab`이 한 말은 이
 * 경계를 넘지 않는다. 인증 상태를 찍으면서 토큰 줄을 함께 내는 CLI라, 갈래만
 * 건너고 문장은 창이 제 언어로 짓는다(1-g56a의 그 이유 그대로). */

/// GitLab 읽기가 왜 실패했나. `GhFailure`와 **같은 모양**인 것이 요점이다 —
/// 창은 갈래(`kind`)를 보고 어느 문장을 세울지 정하지, 문장을 뒤지지 않는다.
///
/// `message`가 차는 것은 **이쪽 실수**일 때뿐이다. `glab`의 실패는 갈래로만
/// 건너므로 그 자리가 비어 있고, 비어 있다는 사실이 곧 「이건 저쪽이 한 말이
/// 아니다」라는 표시다.
#[derive(Debug, Clone, Serialize)]
pub(super) struct GlabFailure {
    pub(super) kind: &'static str,
    pub(super) message: String,
}

impl GlabFailure {
    /// 이쪽 실수. `glab`이 한 말이 아니므로 갈래도 `glab`의 것이 아니다.
    pub(super) fn ours(message: String) -> Self {
        Self {
            kind: "unreadable",
            message,
        }
    }
}

impl From<glab::GlabError> for GlabFailure {
    fn from(error: glab::GlabError) -> Self {
        Self {
            kind: match error {
                glab::GlabError::Missing => "missing",
                // 예산을 넘겼거나 파이프가 끊긴 것은 GitLab이 답한 「아니오」가
                // 아니다 — 사람이 할 일이 따로 없으므로 일반 갈래로 간다.
                glab::GlabError::Refused(_) => "failed",
                glab::GlabError::Denied(denial) => denial.kind(),
            },
            message: String::new(),
        }
    }
}

/* ---- 항목 하나가 제 얼굴로 서는 자리(1-g56c) -------------------------------- */

/// 창이 말한 종류를 백엔드의 열거로 — 모르는 낱말은 추측 없이 거절한다.
/// 짐작한 종류는 번호를 공유하는 **다른 항목**을 여는 길이다.
pub(super) fn gitlab_item_kind(said: Option<&str>) -> Result<glab::GlabKind, GlabFailure> {
    glab::GlabKind::from_word(said.unwrap_or_default())
        .ok_or_else(|| GlabFailure::ours("작업 항목 종류는 issue 또는 mr입니다".to_string()))
}

/// PR 하나를 어디서 체크아웃할 것인가 — Orca의 `worktrees:resolvePrBase`
/// (`resolveGitHubPrStartPoint`, out/main/index.js@6338694).
#[derive(Debug, Clone, Serialize)]
pub(super) struct PrStartPoint {
    /// `worktree add`가 받을 시작점. **커밋 하나**다 — Orca도 remote-tracking
    /// ref를 `rev-parse`해서 sha로 넘긴다(@6343500 부근). 이름으로 넘기면
    /// 만들기와 fetch 사이에 누가 force-push한 순간 다른 커밋을 자른다.
    pub(super) start_point: String,
    /// 브랜치 이름은 PR의 head 이름 **그대로**다. 접두사를 붙이지 않는다 —
    /// Orca는 이 경로에서만 `branchNameOverride = headRefName`을 싣고(스펙 §2),
    /// 접두사를 붙이면 그 브랜치는 PR이 가리키는 브랜치가 아니게 된다.
    pub(super) branch: String,
    /// 이 워크스페이스가 무엇과 비교되는가 —
    /// `refs/remotes/<remote>/<PR의 base>`.
    pub(super) compare_base: Option<String>,
    /// 여기서 고친 것을 **어디로 미는가**. Orca `GitPushTarget`
    /// (`worktree/types.ts:177-183`)의 두 필드이고, 같은 저장소의 PR에도 실린다
    /// — 업스트림을 적어 두면 ahead/behind가 첫 화면부터 맞는다.
    pub(super) push_remote: String,
    pub(super) push_branch: String,
    /// 그 원격이 **포크**인가. 창이 그대로 돌려주고, 만들기가 그 값으로 실패의
    /// 대접을 정한다([`set_push_upstream`]).
    pub(super) push_fork: bool,
    /// PR에 "Allow edits from maintainers"가 꺼져 있으면 `Some(false)`.
    /// 워크스페이스는 그래도 만들어지고, 나중의 push가 GitHub에 거절당할 수
    /// 있다는 사실만 창이 미리 말한다(`fork-push-warning.ts:6-7`).
    pub(super) maintainer_can_modify: Option<bool>,
}

/// 만들기가 받는 푸시 대상 — [`PrStartPoint`]의 세 필드가 그대로 돌아온 것.
///
/// 인자 셋이 아니라 하나인 이유: 셋은 **함께여야 뜻이 있다**. 원격만 오고
/// 브랜치가 없으면 업스트림을 적을 수 없고, `fork`가 빠지면 실패했을 때의
/// 대접을 정할 수 없다.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct PushTargetArg {
    pub(super) remote: String,
    pub(super) branch: String,
    #[serde(default)]
    pub(super) fork: bool,
}

/// 원격이 그 브랜치를 모른다고 답했을 때.
///
/// 병합 뒤 지워진 브랜치이거나, 포크의 head인데 메타데이터가 same-repo라고
/// 말했거나다(Orca도 후자를 상정한다: "missing fork metadata can make a fork PR
/// look like a same-repo branch", `pr-start-point.ts:203-204`). Orca는 여기서
/// `refs/pull/<n>/head`로 폴백하고 그 ref는 둘 다 계속 가리킨다 — 우리는 포크를
/// **원격으로 더해** 가져오므로 그 폴백이 닿지 않는 자리는 하나 남는다: 포크가
/// 지워진 PR. 그때는 밀 곳도 없으므로 사실을 말하고 멈춘다.
pub(super) const PR_HEAD_NOT_ON_REMOTE: &str =
    "이 원격에 그 PR의 head 브랜치가 없습니다 — 포크에서 온 PR이거나 브랜치가 지워졌습니다";

/// 포크의 head 저장소가 사라졌을 때.
///
/// GitHub은 PR을 계속 보여 주고 `refs/pull/<n>/head`도 계속 풀리지만, **밀
/// 곳이 없다** — 그리고 밀 곳이 포크 리뷰의 전제다. Orca도 여기서 `null`을
/// 돌려주고(`pull-request-push-target.ts:122-124`), 그 뒤 워크스페이스는 push
/// 대상 없이 만들어진다. 우리는 만들지 않는다: 업스트림 없는 체크아웃에
/// `push.autoSetupRemote`가 켜져 있으면 첫 push가 **base 저장소에** 기여자의
/// 브랜치 이름으로 새 브랜치를 만든다.
pub(super) const FORK_HEAD_GONE: &str =
    "이 PR의 head 저장소가 없습니다 — 포크가 지워졌거나 볼 수 없어 밀 곳이 없습니다";

/// 포크 원격 이름을 몇 번까지 다르게 지어 볼 것인가.
///
/// Orca는 2부터 99까지 세고 포기한다(`ensureUniqueRemoteName`,
/// `worktree-push-target-setup.ts:63-69`). 같은 이름의 백 번째 포크 원격은
/// 이름이 모자란 것이 아니라 무언가 잘못된 것이다.
pub(super) const FORK_REMOTE_TRIES: u32 = 100;

/// 이 저장소에 이미 그 포크를 가리키는 원격이 있는가.
///
/// Orca `findRemoteForUrl`(`worktree-push-target-setup.ts:11-46`)과 같은 판정.
/// **URL 문자열이 아니라 신원으로** 본다 — 같은 포크가 https와 ssh 두 철자로
/// 적혀 있어도 한 원격이고, 문자열로만 보면 같은 포크를 두 번 더한다. 철자
/// 동일은 마지막 그물로 남긴다(우리 파서가 모르는 호스트).
pub(super) fn remote_for_identity(
    known: &[(String, String)],
    identity: &str,
    url: &str,
) -> Option<String> {
    known
        .iter()
        .find(|(_, spelled)| {
            remote_repo::parse_remote_repo(spelled)
                .is_some_and(|repo| repo.path.eq_ignore_ascii_case(identity))
                || spelled == url
        })
        .map(|(name, _)| name.clone())
}

/// 그 이름이 비어 있으면 그대로, 아니면 뒤에 숫자를 붙여서.
pub(super) fn unique_remote_name(taken: &[String], preferred: &str) -> Option<String> {
    let used = |name: &str| taken.iter().any(|one| one == name);
    if !used(preferred) {
        return Some(preferred.to_string());
    }
    (2..FORK_REMOTE_TRIES)
        .map(|suffix| format!("{preferred}-{suffix}"))
        .find(|candidate| !used(candidate))
}

/// 포크에서 온 PR의 head가 어디 있고, 어디로 밀 것인가.
///
/// Orca는 이 일을 둘로 나눠 둔다 — GitHub에 묻는 반쪽
/// (`getPullRequestPushTarget`)과 git으로 원격을 더하는 반쪽
/// (`prepareWorktreePushTargetWithExec`) — 그리고 뒤의 반쪽을 만들기 시점으로
/// 미룬다. 여기서는 한자리다: **가져오려면 어차피 원격이 있어야 하고**, 포크의
/// head는 그 원격에서 온다. 미뤄서 얻는 것이 없고, 미루면 시작점을
/// `refs/pull/<n>/head`로 따로 가져오는 두 번째 길이 생긴다.
pub(super) fn resolve_fork_push_target(
    host: &Host,
    root: &Path,
    base_remote: &str,
    number: u64,
) -> Result<(String, gh::PushHead), String> {
    let base_url = host
        .vcs()
        .text(root, &["remote", "get-url", base_remote])
        .map_err(|error| error.to_string())?;
    let base_url = base_url.trim().to_string();
    let base = remote_repo::parse_remote_repo(&base_url)
        .ok_or_else(|| format!("{base_remote}의 주소를 읽지 못했습니다: {base_url}"))?;
    let head = gh::fetch_push_head(root, &base.path, number)
        .map_err(|error| {
            error
                .detail()
                .unwrap_or("gh가 답하지 않았습니다")
                .to_string()
        })?
        .ok_or_else(|| FORK_HEAD_GONE.to_string())?;

    // 포크라고 적혀 있었는데 같은 저장소이면 더할 원격이 없다. Orca도 origin과
    // 신원이 같으면 `{remoteName:'origin'}`으로 답한다(:125-134).
    if head.identity() == base.path.to_ascii_lowercase() {
        return Ok((base_remote.to_string(), head));
    }

    let listed = host
        .vcs()
        .text(root, &["remote"])
        .map_err(|error| error.to_string())?;
    let names: Vec<String> = listed
        .lines()
        .map(str::trim)
        .filter(|one| !one.is_empty())
        .map(str::to_string)
        .collect();
    let known: Vec<(String, String)> = names
        .iter()
        .filter_map(|name| {
            host.vcs()
                .text(root, &["remote", "get-url", name])
                .ok()
                .map(|url| (name.clone(), url.trim().to_string()))
        })
        .collect();
    let url = gh::pick_push_url(Some(&base_url), &head.clone_url, &head.ssh_url);
    if let Some(existing) = remote_for_identity(&known, &head.identity(), &url) {
        return Ok((existing, head));
    }
    let name = unique_remote_name(&names, &gh::sanitize_remote_name(&head.owner, &head.repo))
        .ok_or_else(|| format!("{}의 원격 이름을 지을 수 없습니다", head.identity()))?;
    host.vcs()
        .text(root, &["remote", "add", &name, &url])
        .map_err(|error| format!("포크를 원격으로 더하지 못했습니다: {error}"))?;
    Ok((name, head))
}

/// 포크에서 온 MR은 아직 시작점을 못 준다 — 그리고 **못 준다고 말한다**.
///
/// 조용히 기본 브랜치에서 자르는 것이 이 자리의 진짜 위험이다: 리뷰의 이름을
/// 달고, 리뷰의 코드는 하나도 없는 워크스페이스가 만들어진다. 사람은 그것을
/// 열어 보기 전까지 모른다.
///
/// GitHub 쪽은 이 갈래를 [`resolve_fork_push_target`]으로 넘긴다 — 포크를
/// 원격으로 더해서 가져오고, 그 원격이 곧 밀 곳이 된다. GitLab에는 그 대응물이
/// 아직 없다: 원본은 포크 MR의 head를 `refs/merge-requests/<iid>/head`에서
/// 가져오고 **밀 곳은 주지 않는다**(`orca-runtime.ts:26465-26475`). 밀 곳 없는
/// 체크아웃에 `push.autoSetupRemote`가 켜져 있으면 첫 push가 base 저장소에
/// 남의 브랜치를 만든다 — GitHub 쪽에서 [`FORK_HEAD_GONE`]이 막고 있는 바로 그
/// 사고다. 그 도로를 제대로 짓기 전에는 거절이 맞다.
pub(super) const MR_FROM_A_FORK: &str =
    "포크에서 온 MR입니다 — head가 다른 프로젝트에 있어 아직 시작점을 만들지 못합니다";

/// 새 브랜치를 어디서 자를 것인가 — Orca `resolveWorktreeCreateBase`의 사다리.
///
/// 셋을 순서대로 묻는다: 이번에 사람이 적은 것, 이 저장소가 기억하는 것, 그리고
/// 저장소의 기본 브랜치. 셋 다 답이 없으면 [`None`]이고 그것은 오류가 아니라
/// `HEAD`다 — 커밋이 하나도 없는 저장소에서 기본 브랜치를 요구하면 첫
/// 워크스페이스를 만들 수 없다.
pub(super) fn resolve_create_base(
    host: &Host,
    repo_root: &Path,
    explicit: Option<&str>,
    pinned: Option<&str>,
) -> Option<String> {
    if let Some(base) = explicit.map(str::trim).filter(|one| !one.is_empty()) {
        return Some(base.to_string());
    }
    // 기억해 둔 기준은 **아직 있을 때만** 쓴다. 지워진 브랜치를 기준으로
    // 넘기면 git이 거절하고, 사람은 자기가 적지도 않은 이름 때문에 거절당한다.
    if let Some(base) = pinned
        .map(str::trim)
        .filter(|one| !one.is_empty() && ref_exists(host, repo_root, one))
    {
        return Some(base.to_string());
    }
    default_base_probe(host, repo_root)
}

/// 이 저장소의 기본 브랜치 — Orca `resolveDefaultBaseRefFromProbes`.
///
/// 원격이 말하는 것이 먼저다(`refs/remotes/origin/HEAD`). 그것이 없는
/// 저장소에서만 흔한 이름 넷을 차례로 물어본다 — 짐작이지만 **있는 ref만**
/// 답하므로 없는 이름을 기준으로 넘기는 일은 없다.
pub(super) fn default_base_probe(host: &Host, repo_root: &Path) -> Option<String> {
    if let Ok(said) = host.vcs().text(
        repo_root,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) && let Some(short) = said
        .trim()
        .strip_prefix("refs/remotes/")
        .filter(|short| !short.is_empty())
    {
        return Some(short.to_string());
    }
    ["main", "master", "trunk", "develop"]
        .into_iter()
        .find(|name| ref_exists(host, repo_root, name))
        .map(str::to_string)
}

/// 이 ref가 커밋을 가리키는가. `^{commit}`을 붙이는 이유는 태그와 나무를
/// 기준으로 받아 두었다가 `worktree add`에서 거절당하는 것을 여기서 거르기
/// 위해서다.
pub(super) fn ref_exists(host: &Host, repo_root: &Path, reference: &str) -> bool {
    host.vcs()
        .text(
            repo_root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{reference}^{{commit}}"),
            ],
        )
        .is_ok_and(|said| !said.trim().is_empty())
}

/// 기준을 자르기 전에 원격을 한 번 새로 고친다. **실패해도 계속 간다.**
///
/// 비행기 안에서 워크스페이스를 못 만드는 것이 이 함수가 없을 때의 모습이다.
/// 로컬에 그 ref가 이미 있으면 오래된 기준으로 만드는 것이 만들지 못하는
/// 것보다 낫고, 없으면 어차피 그 다음 줄의 git이 자기 말로 거절한다.
pub(super) fn refresh_base(host: &Host, repo_root: &Path, base: &str) {
    // 원격을 가리키는 이름일 때만 부른다. `main`을 자르는 데 네트워크는 필요
    // 없고, 필요 없는 fetch는 만들기마다 붙는 몇 초다.
    let Some((remote, _)) = base.split_once('/') else {
        return;
    };
    if remote.is_empty() {
        return;
    }
    let _ = host.vcs().text_within(
        repo_root,
        &["fetch", "--quiet", remote],
        CREATE_FETCH_BUDGET,
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RemoteTrackingBase {
    pub(super) base_ref: String,
    pub(super) remote_ref: String,
    pub(super) local_branch: String,
}

/// Turn either `origin/main` or `refs/remotes/origin/main` into the one remote
/// branch whose local peer may be maintained. A local name such as `main` is
/// deliberately not guessed into `origin/main`.
pub(super) fn remote_tracking_base(base: &str) -> Option<RemoteTrackingBase> {
    let short = base
        .trim()
        .strip_prefix("refs/remotes/")
        .unwrap_or(base.trim());
    let (remote, local_branch) = short.split_once('/')?;
    if remote.is_empty() || local_branch.is_empty() {
        return None;
    }
    Some(RemoteTrackingBase {
        base_ref: format!("{remote}/{local_branch}"),
        remote_ref: format!("refs/remotes/{remote}/{local_branch}"),
        local_branch: local_branch.to_string(),
    })
}

/// Safely fast-forward the local peer of a remote workspace base.
///
/// There is no force path. A local-only commit, a missing ref, a dirty owner
/// worktree, or any observation that changes before the compare-and-swap turns
/// into a skipped result while workspace creation continues from the fetched
/// remote base.
pub(super) fn refresh_local_base_ref_for_worktree_create(
    host: &Host,
    orchestrator: &Orchestrator,
    repo_root: &Path,
    base: &str,
) -> Option<LocalBaseRefRefresh> {
    let base = remote_tracking_base(base)?;
    let result = |status, owner_worktree_path: Option<&Path>| LocalBaseRefRefresh {
        status,
        base_ref: base.base_ref.clone(),
        local_branch: base.local_branch.clone(),
        owner_worktree_path: owner_worktree_path.map(|path| path.to_string_lossy().into_owned()),
    };
    let local_ref = format!("refs/heads/{}", base.local_branch);
    let commit = |reference: &str| {
        host.vcs()
            .text(
                repo_root,
                &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
            )
            .ok()
            .map(|oid| oid.trim().to_string())
            .filter(|oid| !oid.is_empty())
    };
    let Some(local_oid) = commit(&local_ref) else {
        return Some(result(
            LocalBaseRefRefreshStatus::SkippedNotFastForward,
            None,
        ));
    };
    let Some(remote_oid) = commit(&base.remote_ref) else {
        return Some(result(
            LocalBaseRefRefreshStatus::SkippedNotFastForward,
            None,
        ));
    };
    if host
        .vcs()
        .text(
            repo_root,
            &["merge-base", "--is-ancestor", &local_oid, &remote_oid],
        )
        .is_err()
    {
        return Some(result(
            LocalBaseRefRefreshStatus::SkippedNotFastForward,
            None,
        ));
    }

    let worktrees = match orchestrator.list() {
        Ok(worktrees) => worktrees,
        Err(_) => {
            return Some(result(LocalBaseRefRefreshStatus::SkippedError, None));
        }
    };
    if let Some(owner) = worktrees
        .iter()
        .find(|worktree| worktree.branch.as_deref() == Some(base.local_branch.as_str()))
    {
        let owner_path = owner.path.as_path();
        let tracked_status = match host.vcs().text(
            owner_path,
            &["status", "--porcelain", "--untracked-files=no"],
        ) {
            Ok(status) => status,
            Err(_) => {
                return Some(result(
                    LocalBaseRefRefreshStatus::SkippedError,
                    Some(owner_path),
                ));
            }
        };
        if !tracked_status.trim().is_empty() {
            return Some(result(
                LocalBaseRefRefreshStatus::SkippedDirtyWorktree,
                Some(owner_path),
            ));
        }
        let still_on_branch = host
            .vcs()
            .text(owner_path, &["symbolic-ref", "--quiet", "--short", "HEAD"])
            .is_ok_and(|branch| branch.trim() == base.local_branch);
        let still_on_commit = host
            .vcs()
            .text(owner_path, &["rev-parse", "--verify", "HEAD^{commit}"])
            .is_ok_and(|oid| oid.trim() == local_oid);
        if !still_on_branch || !still_on_commit {
            return Some(result(
                LocalBaseRefRefreshStatus::SkippedError,
                Some(owner_path),
            ));
        }
        let status = if host
            .vcs()
            .text(owner_path, &["reset", "--hard", &remote_oid])
            .is_ok()
        {
            LocalBaseRefRefreshStatus::Updated
        } else {
            LocalBaseRefRefreshStatus::SkippedError
        };
        return Some(result(status, Some(owner_path)));
    }

    let status = if host
        .vcs()
        .text(
            repo_root,
            &["update-ref", &local_ref, &remote_oid, &local_oid],
        )
        .is_ok()
    {
        LocalBaseRefRefreshStatus::Updated
    } else {
        LocalBaseRefRefreshStatus::SkippedError
    };
    Some(result(status, None))
}

/// 만들기 하나가 fetch에 쓸 수 있는 시간. Orca의
/// `CREATE_BASE_FALLBACK_FETCH_TIMEOUT_MS`와 같은 60초 — 큰 저장소의 첫
/// fetch가 그 안에 끝나고, 응답 없는 원격 하나가 다이얼로그를 영원히 붙잡지는
/// 못하는 값이다.
pub(super) const CREATE_FETCH_BUDGET: Duration = Duration::from_secs(60);

/// 이 브랜치가 무엇에서 났는지 git의 config에 적는다.
pub(super) fn record_creation_base(host: &Host, worktree: &Path, branch: &str, base: &str) {
    let _ = host.vcs().text(
        worktree,
        &["config", "--local", &format!("branch.{branch}.base"), base],
    );
}

/// The branch a checkout is standing on, or `None` when it is detached.
pub(super) fn checked_out_branch(host: &Host, root: &Path) -> Option<String> {
    host.vcs()
        .text(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|said| said.trim().to_string())
        .filter(|said| !said.is_empty())
}

/// Write down what a cut the window did not dialog for was cut from.
///
/// `create_worktree` records its own base, with the compare base a person may
/// have typed. The ledger's worker checkouts and the automations' cut from
/// `HEAD` or a job's base branch with nobody typing — and a card that said
/// `wt/t-3` and nothing else did not say whether it grew from `main` or from
/// somebody's feature branch ("어떤 기반의 브랜치로 시작됐는지 표시"). `HEAD`
/// is written as the branch it stood for at the cut, because that is the fact
/// a person wants to read back; a detached leader writes nothing rather than
/// the word.
pub(super) fn record_cut_base(host: &Host, from: &Path, cut: &Worktree, start_point: Option<&str>) {
    let Some(branch) = cut.branch.as_deref() else {
        return;
    };
    let base = match start_point {
        Some(named) => Some(named.to_string()),
        None => checked_out_branch(host, from),
    };
    if let Some(base) = base {
        record_creation_base(host, &cut.path, branch, &base);
    }
}

/// Stamp rows with the base their branch was cut from, when one was written.
///
/// The bases come from the repository snapshot ([`Orchestrator::creation_bases`]
/// reads the shared config file the `open` already located), never from a
/// `git config` process of this side's own — one road for the fact, on the
/// catalogue and on the worktree list alike.
pub(super) fn attach_creation_bases_from(
    entries: &mut [WorktreeEntry],
    bases: &HashMap<String, String>,
) {
    if bases.is_empty() {
        return;
    }
    for entry in entries {
        entry.base = entry
            .branch
            .as_deref()
            .and_then(|branch| bases.get(branch).cloned());
    }
}

/// 첫 `git push`가 업스트림을 스스로 만들게 한다 — **아직 아무도 말하지 않은
/// 저장소에서만.**
///
/// 이미 값이 있으면 손대지 않는다. 이것은 사람의 설정이고, 워크스페이스를
/// 하나 만들었다는 이유로 남의 설정을 덮어쓰는 것은 이 창이 할 일이 아니다.
/// 새로 자른 브랜치가 어디를 따라가는가.
///
/// Orca `configureCreatedWorktreePushTargetWithExec`
/// (`worktree-push-target-setup.ts:113-124`)의 한 문장 —
/// `branch --set-upstream-to <원격>/<브랜치>`.
///
/// 답은 **적혔는가**이고, 부르는 쪽이 그 답으로 다음을 정한다. 포크의
/// 업스트림을 적지 못한 채 [`ensure_push_auto_setup_remote`]의 편의를 주면
/// `git push` 한 번이 **base 저장소에** 기여자의 브랜치 이름으로 새 브랜치를
/// 만든다 — PR은 그대로 둔 채. 업스트림 없는 push가 거절당하는 편이 낫다.
pub(super) fn set_push_upstream(
    host: &Host,
    worktree: &Path,
    branch: &str,
    target: &PushTargetArg,
) -> bool {
    let upstream = format!("{}/{}", target.remote, target.branch);
    host.vcs()
        .text(
            worktree,
            &["branch", "--set-upstream-to", &upstream, branch],
        )
        .is_ok()
}

pub(super) fn ensure_push_auto_setup_remote(host: &Host, worktree: &Path) {
    let already = host
        .vcs()
        .text(worktree, &["config", "--get", "push.autoSetupRemote"])
        .is_ok_and(|said| !said.trim().is_empty());
    if already {
        return;
    }
    let _ = host.vcs().text(
        worktree,
        &["config", "--local", "push.autoSetupRemote", "true"],
    );
}

/// Link the directories the repository asks every workspace to share.
///
/// Best effort by design: a stale name in a repository file must not fail a
/// workspace git has already created, so each entry answers for itself and a
/// failure goes to the log rather than to the person — there is nothing they
/// can do mid-create about somebody else's project file
/// (`worktree-symlinks.ts:348-352`).
pub(super) fn share_project_directories(repo_root: &Path, workspace: &Path) {
    let Some(file) = script::read_project_file(repo_root) else {
        return;
    };
    if file.shared_directories.is_empty() {
        return;
    }
    let done = worktree_shared::link_shared(repo_root, workspace, &file.shared_directories);
    for (entry, outcome) in file.shared_directories.iter().zip(done) {
        if let worktree_shared::Linked::Failed(why) = outcome {
            eprintln!("zerocode-shell: could not share `{entry}`: {why}");
        }
    }
}

/// Take those links away again, so `git worktree remove` sees a clean checkout.
///
/// Without this, every ordinary delete of a workspace with shared directories
/// fails with "it has changed files, use force": a symlink into the primary's
/// `node_modules` is untracked as far as git is concerned
/// (`worktree-symlinks.ts:389-400`).
pub(super) fn unshare_project_directories(repo_root: &Path, workspace: &Path) {
    let Some(file) = script::read_project_file(repo_root) else {
        return;
    };
    if file.shared_directories.is_empty() {
        return;
    }
    worktree_shared::unlink_shared(workspace, &file.shared_directories);
}

/// Resolve the trusted setup command for a checkout that has just been made.
///
/// The decision is three-way on purpose: the policy can say "ask", and a
/// caller who has not asked must not have one picked for it
/// (`shouldRunSetupForCreate` throws in the same case). `run_setup` is the
/// answer when there is one.
pub(super) fn prepare_new_worktree_setup(
    repository: &settings::SettingsRepository,
    config_root: &Path,
    repo_root: &Path,
    worktree: &Path,
    run_setup: Option<bool>,
) -> Result<Option<String>, String> {
    let settings = stored_project_script_settings(repository, repo_root, Some(worktree))?;
    let allowed = run_setup
        .or_else(|| zerocode_core::runs_setup_on_create(settings.setup_run_policy))
        .unwrap_or(false);
    if !allowed {
        return Ok(None);
    }
    // Read from the WORKTREE: a branch may carry its own setup, and the
    // checkout being prepared is the one whose instructions apply.
    let Some(file) = script::read_project_file(worktree) else {
        return Ok(None);
    };
    // The trust gate, enforced where it cannot be routed around. The window
    // asks before it calls this, but the dialog is the user experience and this
    // is the guarantee: a gate that only lived in the webview would be one bug
    // — a throw, a call site added later that forgot to ask — away from running
    // a stranger's script. The repository's half is dropped when nobody has
    // approved it; the LOCAL half is left alone, because that one was written
    // on this machine by the person it would run as. The hash is taken over
    // this checkout's file, so a branch carrying a setup script nobody approved
    // is refused even in a repository that was approved for another one.
    let file = if repo_trust_allows(config_root, repo_root, worktree) {
        file
    } else {
        zerocode_core::ProjectFile {
            setup: None,
            ..file
        }
    };
    let Some(said) = zerocode_core::effective_script(
        &file,
        settings.local_setup.as_deref(),
        zerocode_core::ProjectScript::Setup,
        settings.source,
    ) else {
        return Ok(None);
    };
    Ok(Some(said))
}

/// Start the already-authorized setup command in a real terminal owned by the
/// new checkout. The renderer receives only the resulting terminal id: it can
/// place or close the pane, but it cannot replace the command with arbitrary
/// text or route a trusted command into another directory.
pub(super) fn spawn_setup_terminal(
    state: &State<'_, AppState>,
    repo_root: &Path,
    worktree: &Path,
    command: &str,
) -> Result<TermId, String> {
    let term = state.take_term_id();
    let env = script::setup_environment(repo_root, worktree);
    let mut pty = PtyLane::spawn(
        script::terminal_shell(),
        &script::terminal_shell_args(),
        Some(worktree),
        &env,
        PTY_BIRTH_ROWS,
        PTY_BIRTH_COLS,
    )
    .map_err(|error| format!("준비 터미널을 시작할 수 없습니다: {error}"))?;
    note_window_event(
        state.local_data_root(),
        &format!("term {term} spawned {}", script::terminal_shell()),
    );
    let mut input = command.as_bytes().to_vec();
    if !input.ends_with(b"\r") {
        input.push(b'\r');
    }
    pty.write_input(&input)
        .map_err(|error| format!("준비 스크립트를 터미널에 보낼 수 없습니다: {error}"))?;
    state.hold_terminal(term, pty);
    state.cadence().wake();
    Ok(term)
}

pub(super) const NOT_A_REPOSITORY: &str = "이 프로젝트는 git 저장소가 아닙니다";
pub(super) const NOT_A_WORKTREE: &str = "이 저장소의 워크트리가 아닙니다";

/* ---- 비활성 워크스페이스 삭제 (Orca의 Delete Inactive Workspaces) ---- */

/// 사람이 눌러 둔 무시를 담아 두는 곳. 워크트리 경로 하나에 기록 하나.
pub(super) fn workspace_dismissals_file(config_root: &Path) -> PathBuf {
    config_root.join(artifact_file::WORKSPACE_DISMISSALS)
}

pub(super) fn stored_workspace_dismissals(
    config_root: &Path,
) -> BTreeMap<String, CleanupDismissal> {
    std::fs::read_to_string(workspace_dismissals_file(config_root))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub(super) fn write_workspace_dismissals(
    config_root: &Path,
    held: &BTreeMap<String, CleanupDismissal>,
) -> Result<(), String> {
    let file = workspace_dismissals_file(config_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_string_pretty(held).map_err(|error| error.to_string())?;
    std::fs::write(&file, text).map_err(|error| error.to_string())
}

/// git에게 한 번 물을 때 기다려 주는 시간.
///
/// 여덟 초는 넉넉하고 유한하다. 스캔은 저장소 전부를 훑으므로, 네트워크
/// 마운트 위의 체크아웃 하나가 답하지 않는다고 해서 다이얼로그 전체가 서
/// 있으면 안 된다 — 답하지 못한 것은 `unknown-base`로 남고, 모른다는 것은
/// 이 화면에서 "지우지 말자"와 같은 말이다.
pub(super) const GIT_EVIDENCE_TIMEOUT: Duration = Duration::from_secs(8);

/// 한 번에 도는 git의 수.
///
/// 셋. 하나면 스무 개짜리 저장소에서 사람이 기다리고, 스물이면 노트북의
/// 디스크가 스무 갈래로 갈린다.
pub(super) const GIT_EVIDENCE_LANES: usize = 3;

/// 저장된 무시 하나. 창은 이것을 그대로 돌려보내 "무시" 버튼을 만든다.
pub(super) type CleanupDismissal = zerocode_core::workspace_cleanup::Dismissal;

/// 다이얼로그의 한 줄. 뷰를 그리는 데 필요한 것 전부가 여기 있다.
#[derive(Serialize)]
pub(super) struct CleanupRow {
    /// 워크트리의 안정된 이름 — 경로다. 무시 기록의 키이기도 하다.
    pub(super) id: String,
    /// 사람이 알아보는 이름: 브랜치, 없으면 폴더 이름.
    pub(super) name: String,
    pub(super) path: String,
    /// 이 체크아웃을 소유한 저장소의 루트와 그 이름.
    pub(super) project: String,
    pub(super) project_name: String,
    pub(super) branch: Option<String>,
    pub(super) head: Option<String>,
    pub(super) tier: zerocode_core::workspace_cleanup::Tier,
    pub(super) reasons: Vec<zerocode_core::workspace_cleanup::Reason>,
    pub(super) blockers: Vec<zerocode_core::workspace_cleanup::Blocker>,
    /// 에포크 밀리초. 창이 자기 로케일로 문장을 만든다.
    pub(super) last_activity_ms: i64,
    pub(super) idle_days: i64,
    pub(super) git: CleanupGit,
    /// 제거할 때 `--force`가 필요한가. `!provably_clean`과 같은 값이다.
    pub(super) force: bool,
    pub(super) fingerprint: String,
}

/// 한 줄이 실은 git 요약.
#[derive(Serialize)]
pub(super) struct CleanupGit {
    pub(super) dirty_files: usize,
    pub(super) ahead: usize,
    pub(super) has_upstream: bool,
    pub(super) provably_clean: bool,
}

/// 한 번의 스캔.
#[derive(Serialize)]
pub(super) struct CleanupReport {
    pub(super) rows: Vec<CleanupRow>,
    /// 이 스캔이 기준으로 삼은 시각. 창이 "방금 봤다"를 말할 때 쓴다.
    pub(super) scanned_at_ms: i64,
    /// 이 답을 만든 규칙의 판. 창은 "무시"를 적을 때 이 값을 되돌려 보낸다 —
    /// 자기가 지어낸 숫자를 보내지 않게 하려는 것이고, 어느 쪽이든 Rust가
    /// 다시 확인하므로 무시는 이 값 하나로 성립하지 않는다.
    pub(super) classifier_version: u32,
}

/// 한 워크트리에 대해 git에게 물어서 알아낸 것.
///
/// 두 번 묻는다. 첫 물음은 언제나 하고, 둘째는 첫 답이 "깨끗한데 upstream이
/// 없다"일 때만 한다 — 그 조합만이 "밀어 두지 않은 커밋이 있는가"라는 물음을
/// 아직 열어 두기 때문이다.
///
/// **호스트를 받는다.** 이 워크트리가 어느 기계에 있는지는 스캔이 한 번 정하고
/// 여기까지 들고 온다 — 이 함수가 다시 묻지 않는다. `GIT_EVIDENCE_TIMEOUT`은
/// 여전히 이 창의 정책이라 여기서 넘긴다: 예산은 화면이 정하고, 죽이는 일은
/// 경계가 한다.
pub(super) fn cleanup_git_evidence(
    host: &Host,
    worktree: &Path,
) -> zerocode_core::workspace_cleanup::GitEvidence {
    use zerocode_core::workspace_cleanup::GitEvidence;

    let errored = GitEvidence {
        errored: true,
        ..GitEvidence::unknown()
    };
    // `core.quotePath=false`: 한글 파일 이름이 `\355\225\234`으로 돌아오면
    // 세는 데는 지장이 없지만, 같은 명령을 다른 화면이 재사용할 때 읽을 수 없는
    // 경로가 된다. 세는 값과 보여 줄 값을 다른 명령으로 나누지 않는다.
    let Some(text) = host.vcs().text_within(
        worktree,
        &[
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
        ],
        GIT_EVIDENCE_TIMEOUT,
    ) else {
        return errored;
    };

    let mut dirty_files = 0usize;
    let mut has_upstream = false;
    let mut ahead = 0usize;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            has_upstream = !rest.trim().is_empty();
            continue;
        }
        if let Some(rest) = line.strip_prefix("# branch.ab ") {
            // `+2 -0` — 앞선 수가 먼저다. 뒤진 수는 이 화면의 물음이 아니다:
            // 원격이 앞서 있다고 해서 이 체크아웃이 무언가를 들고 있는 것은
            // 아니다.
            ahead = rest
                .split_whitespace()
                .next()
                .and_then(|one| one.strip_prefix('+'))
                .and_then(|count| count.parse().ok())
                .unwrap_or(0);
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // v2의 나머지 줄은 한 줄이 한 파일이다 — 이름이 바뀐 것(`2 `)도 경로
        // 두 개를 한 줄에 싣는다.
        dirty_files += 1;
    }

    let mut unpushed_commits = false;
    let mut unknown_base = false;
    if dirty_files == 0 && !has_upstream {
        match host.vcs().text_within(
            worktree,
            &["rev-list", "--count", "HEAD", "--not", "--remotes"],
            GIT_EVIDENCE_TIMEOUT,
        ) {
            // 셀 수 없었던 것은 0이 아니다. 파싱에 실패한 답을 0으로 읽으면
            // 밀어 두지 않은 작업을 들고 있는 체크아웃이 제안 목록의 맨 위에
            // 체크된 채로 올라간다.
            Some(said) => match said.trim().parse::<u64>() {
                Ok(0) => {}
                Ok(_) => unpushed_commits = true,
                Err(_) => unknown_base = true,
            },
            None => unknown_base = true,
        }
    }

    GitEvidence {
        dirty_files,
        has_upstream,
        ahead,
        unpushed_commits,
        unknown_base,
        errored: false,
    }
}

/// 연결된 워크트리의 진짜 git 디렉터리.
///
/// 워크트리의 `.git`은 보통 `gitdir: …` 한 줄이 든 파일이고, 브랜치를 옮기거나
/// 커밋을 하면 바뀌는 것은 저 너머의 디렉터리다. 그것을 따라가지 않으면 활동
/// 시각은 체크아웃한 날에 멈춰 있다.
///
/// 규칙은 창의 것이고, 그것이 딛는 두 번의 접근은 경계 너머다 — `is_dir`과
/// `read_to_string`. 그래서 이 함수는 [`Fs`]를 받는다.
pub(super) fn linked_gitdir(fs: &dyn Fs, worktree: &Path) -> Option<PathBuf> {
    let dot = worktree.join(".git");
    if fs.is_dir(&dot) {
        return Some(dot);
    }
    let text = fs.read_to_string(&dot)?;
    let named = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))?
        .trim();
    let named = PathBuf::from(named);
    Some(if named.is_absolute() {
        named
    } else {
        worktree.join(named)
    })
}

/// 이 체크아웃이 "언제 살아 있었는가"를 말해 주는 파일들.
pub(super) fn worktree_activity_marks(host: &Host, worktree: &Path) -> Vec<Option<i64>> {
    let fs = host.fs();
    let mut marks = vec![
        fs.modified_ms(worktree),
        fs.modified_ms(&worktree.join(".git")),
    ];
    if let Some(gitdir) = linked_gitdir(fs, worktree) {
        marks.push(fs.modified_ms(&gitdir));
        marks.push(fs.modified_ms(&gitdir.join("HEAD")));
        marks.push(fs.modified_ms(&gitdir.join("logs").join("HEAD")));
    }
    marks
}

/// 후보를 가르기 전에 모아 둔, 한 워크트리에 대한 사실.
pub(super) struct CleanupSubject {
    pub(super) worktree: Worktree,
    pub(super) project: PathBuf,
    pub(super) project_name: String,
    pub(super) facts: zerocode_core::workspace_cleanup::WorktreeFacts,
}

/// 영속 저장이 이 체크아웃에 대해 아는 마지막 시각.
///
/// 자동화 이력이 그 저장이다. 이 창이 날짜를 적어 두는 유일한 워크스페이스
/// 기록이고, 파일의 mtime이 놓치는 것 — 손을 대지 않은 채 그 안에서 무언가
/// 돌았다는 사실 — 을 아는 유일한 곳이다.
pub(super) fn cleanup_stored_activity(runs: &[AutomationRun], worktree: &str) -> Option<i64> {
    runs.iter()
        .filter(|run| run.root == worktree || run.made_worktree.as_deref() == Some(worktree))
        .flat_map(|run| [Some(run.at_epoch_ms), run.ended_at_epoch_ms])
        .flatten()
        .max()
}

/// 이 체크아웃이 "보관된" 것으로 읽히는가.
///
/// ZeroCode에는 Orca의 보관함이 없다. 가장 가까운 사실은 자동화가 만들었고 그
/// 실행이 끝나는 것을 이 창이 본 체크아웃이다 — 사람이 만든 것이 아니고,
/// 만든 일도 끝났고, 그런데도 아무도 남기기로 고르지 않은 워크스페이스.
///
/// **끝난 것이 목격돼야 한다.** 아직 열려 있는 실행이 하나라도 있으면 보관이
/// 아니다: 창을 껐다 켜서 끝을 못 본 실행은 영영 열린 채로 남고, 그것을
/// 보관으로 읽으면 지금 돌고 있는 작업이 7일 후 제안 목록에 오른다.
pub(super) fn cleanup_looks_archived(runs: &[AutomationRun], worktree: &str) -> bool {
    let mut born = false;
    for run in runs {
        if run.made_worktree.as_deref() != Some(worktree) {
            continue;
        }
        born = true;
        if run.ended_at_epoch_ms.is_none() {
            return false;
        }
    }
    born
}

/* ---- 공간 (Orca의 Space 페이지) ----
 *
 * 판정 축이 위와 다르다. 저 다이얼로그는 **유휴**를 묻고 이 화면은 **크기**를
 * 묻는다 — 그래서 화면이 둘이고 목록을 만드는 규칙도 둘이다. 같은 것은 하나,
 * "지워도 되는가"뿐이고 그 답은 위의 분류기가 한다.
 *
 * 재는 방법이 Orca와 다르다. Orca는 워크트리마다 `du -k -d 1`을 스폰하고 그
 * 출력을 파싱한다(`main/index.js:147766`). 여기서는 **Rust가 직접 훑는다**.
 * 얻는 것 셋:
 *
 *   1. **프로세스가 없다.** 워크트리 스무 개면 `du` 스무 번이고, 그 각각이
 *      fork + exec + 파이프 + 텍스트 파싱이다.
 *   2. **취소가 kill이 아니다.** `du`를 멈추는 방법은 신호를 보내는 것뿐이고,
 *      신호와 종료 사이에는 아무도 모르는 시간이 있다. 여기서는 순회가
 *      플래그를 읽고 **자기가 멈춘다** — 다음 디렉터리를 열기 전에.
 *   3. **상한이 안에 있다.** `du`에는 "십만 항목에서 그만"이 없다. 우리는
 *      세면서 세고, 넘으면 그 행만 unavailable이 된다.
 */

pub(super) use zerocode_core::workspace_space as space;

/// 한 번에 재는 워크스페이스의 수.
///
/// 셋. Orca의 `WORKTREE_SCAN_CONCURRENCY`와 같은 값이고, 같은 이유다 — 하나면
/// 저장소 스무 개짜리 기계에서 사람이 기다리고, 스물이면 디스크 헤드가 스무
/// 갈래로 갈려 전부 느려진다.
pub(super) const SPACE_WORKTREE_LANES: usize = 3;

/// 한 워크스페이스 안에서 동시에 훑는 top-level 항목의 수.
///
/// 여덟. 워크트리 하나의 depth-1 항목은 보통 `node_modules` 하나가 나머지 전부의
/// 열 배이므로, 직렬로 재면 그 하나가 그 행의 시간이 된다. Orca의
/// `LOCAL_FS_CONCURRENCY`는 48이지만 그것은 `du`가 죽었을 때의 폴백 경로이고,
/// 이쪽은 언제나 도는 길이라 워크스페이스 셋 × 여덟 = 스물넷이 동시에 도는 것을
/// 상한으로 잡았다.
pub(super) const SPACE_ENTRY_LANES: usize = 8;

/// 삭제 후보의 git 증거를 채울 때 동시에 도는 수.
///
/// 여섯. Orca의 `GIT_STATUS_REFRESH_CONCURRENCY`와 같다. 위의 스캔(셋)보다 큰
/// 것은 이 일이 **사람이 보고 있는 동안 뒤에서** 도는 일이라서다 — "git 미확인"
/// 배지를 지우는 것이 목적이고, 늦게 지워지면 지우지 않은 것과 같다.
pub(super) const SPACE_GIT_LANES: usize = 6;

/// 진행률을 내보내는 최소 간격.
///
/// 첫 소식과 마지막 소식은 이 문을 지나지 않는다. 100ms마다 거르지 않으면
/// 워크스페이스 백 개짜리 기계에서 이 채널 하나가 웹뷰의 프레임을 먹는다.
pub(super) const SPACE_PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 이미 도는 스캔이 있을 때의 답.
pub(super) const SPACE_ALREADY: &str = "이미 공간을 훑고 있습니다";

/// 지금 도는 스캔이 있는가. [`ScanFlag`]가 어떻게 끝나든 내려 준다.
pub(super) static SPACE_SCANNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 사람이 취소를 눌렀는가.
///
/// **프로세스를 죽이지 않는다.** 순회가 디렉터리 하나를 열기 전에 이것을 읽고
/// 스스로 멈춘다 — 그래서 취소는 즉시이고, 반쯤 죽은 자식이 남지 않는다.
pub(super) static SPACE_CANCEL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 한 워크스페이스를 훑는 동안 쓰는 몫.
///
/// 둘이 따로인 이유는 두 모양이 다른 쪽을 먼저 치기 때문이다: 넓은 트리는 항목
/// 수를, 경로가 깊고 긴 트리는 바이트를 먼저 친다. 워크스페이스마다 새로 만든다
/// — 상한에 닿는 것은 **그 행**이지 스캔 전체가 아니다.
pub(super) struct SpaceBudget {
    pub(super) entries: std::sync::atomic::AtomicUsize,
    pub(super) retained: std::sync::atomic::AtomicUsize,
}

impl SpaceBudget {
    pub(super) fn fresh() -> Self {
        Self {
            entries: std::sync::atomic::AtomicUsize::new(0),
            retained: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

/// 한 순회가 지금 들고 있는 경로 바이트.
///
/// 어떻게 끝나든 — 상한, 취소, 성공, 패닉 — 스스로 반납한다. 반납을 잊으면 다음
/// 워크스페이스가 남의 몫을 물려받아 아무 이유 없이 `unavailable`이 된다.
pub(super) struct SpaceHold<'a> {
    pub(super) budget: &'a SpaceBudget,
    pub(super) bytes: usize,
}

impl SpaceHold<'_> {
    pub(super) fn take(&mut self, bytes: usize) -> Result<(), space::Limit> {
        use std::sync::atomic::Ordering;
        self.bytes += bytes;
        let now = self.budget.retained.fetch_add(bytes, Ordering::Relaxed) + bytes;
        if now > space::MAX_RETAINED_BYTES {
            return Err(space::Limit::Memory);
        }
        Ok(())
    }

    pub(super) fn give(&mut self, bytes: usize) {
        use std::sync::atomic::Ordering;
        self.bytes -= bytes;
        self.budget.retained.fetch_sub(bytes, Ordering::Relaxed);
    }
}

impl Drop for SpaceHold<'_> {
    fn drop(&mut self) {
        self.budget
            .retained
            .fetch_sub(self.bytes, std::sync::atomic::Ordering::Relaxed);
    }
}

/// 한 디렉터리 아래 전부의 크기 — 프로세스 하나 없이, 멈추라면 멈추면서.
///
/// 재귀가 아니라 스택이다. 깊은 트리에서 재귀는 이 스레드의 스택을 넘고, 넘으면
/// 창이 죽는다.
///
/// 링크를 따라가지 않는 것이 중요한데, 그 약속은 이제 경계가 지킨다
/// ([`Fs::read_dir`]) — 따라가면 워크트리 밖의 바이트를 이 워크스페이스의 것으로
/// 세고, 링크가 자기 위를 가리키면 순회가 끝나지 않는다.
///
/// 열지 못한 디렉터리는 0으로 지나간다. 권한 없는 자리 하나 때문에 9GB짜리 행을
/// 통째로 실패로 만들지 않는다 — 루트를 열지 못하는 경우는 이 함수에 닿기 전에
/// 갈린다.
///
/// 설명을 읽지 못한 항목도 **상한에는 센다**. 셀 수 없는 것을 세지 않으면 상한이
/// 조용히 헐거워진다.
pub(super) fn space_walk(
    fs: &dyn Fs,
    root: &Path,
    budget: &SpaceBudget,
) -> Result<u64, space::Limit> {
    use std::sync::atomic::Ordering;

    let mut hold = SpaceHold { budget, bytes: 0 };
    hold.take(root.as_os_str().len())?;
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    let mut total = 0u64;
    while let Some(dir) = stack.pop() {
        // 멈추라는 말은 다음 디렉터리를 열기 전에 읽는다. 신호를 보내고 죽기를
        // 기다리는 것이 아니라, 순회가 자기 발로 선다.
        if SPACE_CANCEL.load(Ordering::SeqCst) {
            return Err(space::Limit::Cancelled);
        }
        hold.give(dir.as_os_str().len());
        let Ok(listing) = fs.read_dir(&dir) else {
            continue;
        };
        for found in listing {
            if budget.entries.fetch_add(1, Ordering::Relaxed) + 1 > space::MAX_SCAN_ENTRIES {
                return Err(space::Limit::Entries);
            }
            let Some(about) = found.about else {
                continue;
            };
            if about.is_dir {
                hold.take(found.path.as_os_str().len())?;
                stack.push(found.path);
            } else {
                total += about.disk_bytes;
            }
        }
    }
    Ok(total)
}

/// 한 워크스페이스를 잰 결과.
pub(super) enum SpaceMeasured {
    Ok(Vec<space::Entry>, u64),
    Stopped(space::Status),
}

/// 워크스페이스 하나의 depth-1 항목마다 크기를 잰다.
///
/// 호스트를 받는다 — 이 워크스페이스가 어느 기계에 있는지는 스캔이 한 번 정하고,
/// 여기와 [`space_walk`]는 그 답을 받아 쓰기만 한다.
pub(super) fn space_measure(host: &Host, root: &Path) -> SpaceMeasured {
    let fs = host.fs();
    let listing = match fs.read_dir(root) {
        Ok(listing) => listing,
        Err(refused) => {
            return SpaceMeasured::Stopped(match refused {
                // git은 아직 이 워크트리를 알고 있는데 디렉터리가 없다. 흔한
                // 상태이고(손으로 지운 체크아웃), 실패가 아니라 그 자체가 사실이다.
                host::ReadDir::Missing => space::Status::Missing,
                host::ReadDir::NoAccess => space::Status::NoAccess,
                host::ReadDir::Failed => space::Status::Failed,
            });
        }
    };
    let children: Vec<(String, bool, u64)> = listing
        .into_iter()
        .filter_map(|found| {
            let name = found.name();
            let about = found.about?;
            Some((name, about.is_dir, about.disk_bytes))
        })
        .collect();

    let budget = SpaceBudget::fresh();
    let mut sized: Vec<Result<space::Entry, space::Status>> = Vec::with_capacity(children.len());
    for chunk in children.chunks(SPACE_ENTRY_LANES) {
        let mut asked = std::thread::scope(|scope| {
            let asking: Vec<_> = chunk
                .iter()
                .map(|(name, is_dir, own)| {
                    let budget = &budget;
                    scope.spawn(move || {
                        if !*is_dir {
                            return Ok(space::Entry {
                                name: name.clone(),
                                size_bytes: *own,
                                kind: space::Kind::File,
                            });
                        }
                        space_walk(fs, &root.join(name), budget)
                            .map(|bytes| space::Entry {
                                name: name.clone(),
                                // 디렉터리 자신의 블록도 그 아래의 것이다.
                                size_bytes: bytes + *own,
                                kind: space::Kind::Dir,
                            })
                            // 취소는 상태를 만들지 않는다 — 취소된 스캔의 행은
                            // 애초에 실리지 않으므로, 여기서 무엇으로 접히든
                            // 아무도 읽지 않는다.
                            .map_err(|limit| limit.status().unwrap_or(space::Status::Failed))
                    })
                })
                .collect();
            asking
                .into_iter()
                .map(|one| one.join().unwrap_or(Err(space::Status::Failed)))
                .collect::<Vec<_>>()
        });
        sized.append(&mut asked);
    }

    let mut entries = Vec::with_capacity(sized.len());
    for one in sized {
        match one {
            Ok(entry) => entries.push(entry),
            // 한 항목이 상한에 닿으면 그 **행**이 unavailable이다. 나머지를
            // 더해 보여 주면 사람이 읽는 숫자가 "이 워크스페이스의 크기"가
            // 아니라 "이 워크스페이스에서 셀 수 있었던 만큼"이 된다.
            Err(status) => return SpaceMeasured::Stopped(status),
        }
    }
    let total: u64 = entries.iter().map(|one| one.size_bytes).sum();
    SpaceMeasured::Ok(space::cap_entries(entries), total)
}

/// 진행률의 문. 몇 개를 셌는지와 마지막으로 말한 때를 함께 쥔다.
pub(super) struct SpaceTally<'a> {
    pub(super) app: &'a AppHandle,
    pub(super) total: usize,
    pub(super) inner: Mutex<(usize, Option<Instant>)>,
}

impl SpaceTally<'_> {
    /// 한 워크스페이스를 다 쟀다. 100ms 문을 지나야 말한다.
    pub(super) fn stepped(&self, name: &str) {
        self.say(name, true, false);
    }

    /// 첫 소식과 마지막 소식. 문을 지나지 않는다 — 스캔이 한 번의 깜빡임으로
    /// 끝나도 시작과 끝은 언제나 전해진다.
    pub(super) fn announced(&self, name: &str) {
        self.say(name, false, true);
    }

    pub(super) fn say(&self, name: &str, step: bool, force: bool) {
        use std::sync::atomic::Ordering;
        let mut held = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if step {
            held.0 += 1;
        }
        let now = Instant::now();
        if !force
            && held
                .1
                .is_some_and(|at| now.duration_since(at) < SPACE_PROGRESS_INTERVAL)
        {
            return;
        }
        held.1 = Some(now);
        let done = held.0;
        drop(held);
        let _ = self.app.emit(
            "space:progress",
            SpaceProgress {
                state: if SPACE_CANCEL.load(Ordering::SeqCst) {
                    "cancelling"
                } else {
                    "scanning"
                },
                done,
                total: self.total,
                name: name.to_string(),
            },
        );
    }
}

/// 창이 진행 문구를 만드는 데 필요한 것 전부.
#[derive(Serialize, Clone)]
pub(super) struct SpaceProgress {
    /// `"scanning"` / `"cancelling"`.
    pub(super) state: &'static str,
    pub(super) done: usize,
    pub(super) total: usize,
    /// 지금 잰 워크스페이스의 이름.
    pub(super) name: String,
}

/// 목록에 오르기 전의 한 워크스페이스.
pub(super) struct SpaceSubject {
    pub(super) worktree: Worktree,
    pub(super) name: String,
    pub(super) project: PathBuf,
    pub(super) project_name: String,
}

/// 답하지 못한 저장소. 목록의 행이 아니라 배너 한 줄이 된다.
#[derive(Serialize)]
pub(super) struct SpaceRepoError {
    pub(super) path: String,
    pub(super) name: String,
    pub(super) error: String,
}

/// 표의 한 행.
#[derive(Serialize)]
pub(super) struct SpaceRow {
    /// 안정된 이름 — 경로다.
    pub(super) id: String,
    pub(super) name: String,
    pub(super) path: String,
    pub(super) project: String,
    pub(super) project_name: String,
    pub(super) branch: Option<String>,
    pub(super) size_bytes: u64,
    /// 위 숫자를 사람이 읽는 말로. 규칙은 [`space::format_bytes`] 하나다.
    pub(super) size_text: String,
    pub(super) status: space::Status,
    /// 이 워크스페이스의 top-level 항목들, 큰 것부터.
    pub(super) entries: Vec<SpaceEntry>,
    pub(super) is_main: bool,
    pub(super) is_active: bool,
    pub(super) last_activity_ms: i64,
    /// 왜 지우면 안 되는가. 분류기가 만든 목록 그대로다.
    pub(super) blockers: Vec<zerocode_core::workspace_cleanup::Blocker>,
    pub(super) force: bool,
    /// 체크박스가 켜질 수 있는가.
    pub(super) ready: bool,
    /// git에게 이미 물었는가. 거짓이면 배지가 "git 미확인"이다.
    pub(super) git_checked: bool,
    pub(super) dirty_files: usize,
    pub(super) ahead: usize,
    /// 이 체크아웃에서 도는 레인의 수. 호버 카드의 한 줄.
    pub(super) lanes: usize,
}

/// 한 행의 top-level 항목 하나.
#[derive(Serialize)]
pub(super) struct SpaceEntry {
    /// [`space::Kind::Other`]이면 빈 문자열 — 이름은 창이 자기 언어로 붙인다.
    pub(super) name: String,
    pub(super) size_bytes: u64,
    pub(super) size_text: String,
    pub(super) kind: space::Kind,
}

impl SpaceEntry {
    pub(super) fn new(entry: space::Entry) -> Self {
        Self {
            name: entry.name,
            size_bytes: entry.size_bytes,
            size_text: space::format_bytes(entry.size_bytes),
            kind: entry.kind,
        }
    }
}

/// 한 번의 스캔.
#[derive(Serialize)]
pub(super) struct SpaceReport {
    pub(super) rows: Vec<SpaceRow>,
    pub(super) repo_errors: Vec<SpaceRepoError>,
    pub(super) scanned_at_ms: i64,
    /// 잰 것 전부의 합.
    pub(super) total_bytes: u64,
    pub(super) total_text: String,
    /// 다 잰 워크스페이스의 수와, 이 스캔이 재려던 수.
    pub(super) scanned_count: usize,
    pub(super) total_count: usize,
    /// 사람이 도중에 멈췄는가. 참이면 위 두 수가 다르고, 그것이 정상이다.
    pub(super) cancelled: bool,
}

/// git에게 물은 뒤의 한 행. 판정은 여기서도 분류기가 한다.
#[derive(Serialize)]
pub(super) struct SpaceGitRow {
    pub(super) id: String,
    pub(super) blockers: Vec<zerocode_core::workspace_cleanup::Blocker>,
    pub(super) force: bool,
    pub(super) ready: bool,
    pub(super) git_checked: bool,
    pub(super) dirty_files: usize,
    pub(super) ahead: usize,
}

/// What the task board's worker roster says of one checkout (t-6588): how far
/// its branch is past the base it was cut from and how many files are changed
/// in it — what a coordinator typed `git rev-list`/`git status` for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct DeskCheckout {
    pub(super) path: String,
    pub(super) branch: Option<String>,
    /// What the branch was cut from, as the cut wrote it down.
    pub(super) base: Option<String>,
    /// Commits on the branch its base does not have — the reclaim sweep's own
    /// count (`Orchestrator::commits_beyond`). `None`: nobody recorded the
    /// base, or git could not compare.
    pub(super) beyond_base: Option<usize>,
    /// Changed files, the cleanup screen's own count (`cleanup_git_evidence`).
    /// `None`: git did not answer.
    pub(super) dirty_files: Option<usize>,
}

/// The most checkouts one ask reads. A roster is a handful of workers; the
/// bound keeps a webview list from becoming a git storm.
pub(super) const DESK_CHECKOUTS_MAX: usize = 32;

/// The roster's git facts for `paths`, each a checkout this window catalogues
/// — the same listing `workspace_space_git` walks (stored projects and the
/// active one, one `worktree list` each), so no string from the webview
/// reaches git. Read in series: a roster is a handful of trees.
pub(super) fn desk_checkout_facts(
    config_root: &Path,
    active_project: &Path,
    paths: &[String],
) -> Vec<DeskCheckout> {
    let wanted: HashSet<PathBuf> = paths
        .iter()
        .take(DESK_CHECKOUTS_MAX)
        .map(PathBuf::from)
        .collect();
    let mut projects = stored_projects(config_root);
    projects.push(active_project.to_string_lossy().into_owned());
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut facts = Vec::new();
    for stored in projects {
        let Ok(orchestrator) = Orchestrator::open(Path::new(&stored)) else {
            continue;
        };
        if !seen.insert(orchestrator.repo_root().to_path_buf()) {
            continue;
        }
        let bases = orchestrator.creation_bases();
        for worktree in orchestrator.list().unwrap_or_default() {
            if !wanted.contains(&worktree.path) {
                continue;
            }
            let base = worktree
                .branch
                .as_deref()
                .and_then(|branch| bases.get(branch))
                .cloned();
            let beyond_base = match (worktree.branch.as_deref(), base.as_deref()) {
                (Some(branch), Some(base)) => orchestrator
                    .commits_beyond(&worktree.path, base, branch)
                    .ok()
                    .flatten(),
                _ => None,
            };
            let git = cleanup_git_evidence(&Host::for_workspace(&worktree.path), &worktree.path);
            facts.push(DeskCheckout {
                path: worktree.path.to_string_lossy().into_owned(),
                branch: worktree.branch.clone(),
                base,
                beyond_base,
                dirty_files: (!git.errored).then_some(git.dirty_files),
            });
        }
    }
    facts
}

/// 이 창이 이 체크아웃 안에서 열어 두고 있는 터미널의 수 — 두 레지스트리를
/// **한 함수로** 센 하나의 답.
///
/// 두 개인 것이 결함의 뿌리였다. `zo` 레인은 `LaneRegistry`에 있고, 에이전트
/// 터미널 — 리더, 팀메이트, 원장이 소환한 워커 — 는 `TerminalRegistry`에 있으며
/// 자기가 선 나무를 `team_envs`에 적어 둔다. 청소 화면 둘이 앞의 것만 세는 바람에
/// **일하고 있는 에이전트의 체크아웃이 방해물 하나 없는 행으로** 삭제 목록에
/// 올랐다. 세는 자리가 둘이면 언젠가 갈라지고, 갈라지는 방향은 언제나 지우는
/// 쪽이다.
///
/// 임의의 OS 프로세스는 세지 않는다. 이 답은 "이 창이 관리하는 터미널"의
/// 사실이고, 판보다 오래 사는 자식(`nohup`, 데몬)까지 세려면 pid/프로세스트리가
/// 따로 필요하다 — 그런 사례가 실제로 관측되기 전에는 짓지 않는다.
pub(super) fn occupied_checkouts(state: &AppState) -> HashMap<PathBuf, usize> {
    let mut counts: HashMap<PathBuf, usize> = HashMap::new();
    {
        let registry = state.registry();
        for lane in registry.lanes() {
            if lane.state == zerocode_core::LaneState::Exited {
                continue;
            }
            if let Some(id) = lane.worktree_id.clone() {
                *counts.entry(PathBuf::from(id)).or_default() += 1;
            }
        }
    }
    // Snapshot, then ask — never both locks at once. `team_envs` and the
    // terminal registry are taken in this order by every other road, and a
    // `(term, path)` pair is plain data that answers the same question with
    // neither guard held. A term recycled between the two reads makes an old
    // path look busy for one pass, which KEEPS a directory; there is no
    // arrangement of these two reads that deletes one.
    let seated: Vec<(TermId, PathBuf)> = {
        let envs = state.team_envs();
        envs.iter()
            .filter_map(|(term, env)| Some((*term, pane_worktree(env)?)))
            .collect()
    };
    for (term, path) in seated {
        if state.terminals().contains_key(&term) {
            *counts.entry(path).or_default() += 1;
        }
    }
    counts
}

/// How many terminals this window is holding inside ONE checkout — the last
/// question every removal door asks.
///
/// Compared with [`same_worktree_path`] rather than by `==`, because the path
/// a pane recorded and the path git lists can differ by a symlink or a
/// trailing separator, and the two ways that comparison can be wrong are not
/// symmetric: matching too much keeps a directory somebody can free in a
/// second, and matching too little deletes the ground under a working agent.
pub(super) fn checkout_occupancy(state: &AppState, path: &Path) -> usize {
    occupancy_of(&occupied_checkouts(state), path)
}

/// How many of an already-counted set sit in ONE checkout.
///
/// The rule every removal door and the reclaim sweep share, in one place so
/// that they cannot drift: two spellings of the same question is how the two
/// registries came to disagree in the first place.
pub(super) fn occupancy_of(counted: &HashMap<PathBuf, usize>, path: &Path) -> usize {
    counted
        .iter()
        .filter(|(held, _)| same_worktree_path(held, path))
        .map(|(_, count)| count)
        .sum()
}

/// 창이 실어 보낸 "저장되지 않은 편집을 들고 있는 체크아웃"들.
///
/// 창만이 아는 사실이라 창이 보낸다. 판정은 그래도 분류기가 한다 — 이 집합은
/// [`zerocode_core::workspace_cleanup::WorktreeFacts`]의 칸 하나로 들어갈 뿐,
/// 여기서 무엇을 결정하지 않는다.
pub(super) fn space_unsaved(sent: Option<Vec<String>>) -> HashSet<PathBuf> {
    sent.unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

// The existing catalog stamp stays pure; its command also drains observed path moves.
pub(crate) fn worktree_stamp_of(roots: Vec<String>) -> String {
    let mut said = String::new();
    for root in roots {
        said.push_str(&root);
        said.push(':');
        if let Some(dir) = linked_worktrees_dir(Path::new(&root))
            && let Ok(entries) = std::fs::read_dir(&dir)
        {
            let mut names: Vec<String> = entries
                .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
                .collect();
            names.sort_unstable();
            said.push_str(&names.join(","));
        }
        said.push(';');
    }
    said
}
